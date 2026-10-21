//! Events track, slice 3c (#980): the cross-build schema registry's document
//! shape and (de)serialisation.
//!
//! P4.0 (#1113, [DECISION A]): this module used to also hold `reconcile` —
//! the `bynk_check`-coupled half that diffs a project's live event shapes
//! against this document (it reads `UnitTable`, which stays in `bynk-emit`
//! pending P4.1's `bynk-check` entry point). Only the pure, disk-free
//! document shape and its parse/serialize moved here; `reconcile` and its
//! helpers (`snapshot`, `canon_type`, `Reconciled`, …) stay in `bynk-emit`'s
//! own `project/schema_registry.rs`, and reach `SchemaRegistry`'s otherwise-
//! private `version`/`events` only through [`SchemaRegistry::new`],
//! [`SchemaRegistry::get`], and [`SchemaRegistry::insert`] — never raw field
//! access across the crate boundary.
//!
//! #1078: this module touches no disk. `parse`/`serialize` are pure —
//! `bynk.schema.lock`'s content comes in through
//! `CompileOptions::schema_registry`'s `SchemaLock::On { existing }` and goes
//! out through `ProjectOutput::schema_lock`; `bynk-driver`'s `schema_lock`
//! module owns the actual read/atomic-write. `parse` takes a
//! `project_root: &Path` (#1085 review) purely to name the file in a
//! corruption message — never for I/O.
//!
//! State-migrations S0 (#2204): the lock is `version = 2` and also records
//! each agent class's stored state shape under `[agents]` (track
//! `design/tracks/state-migrations.md` §3.1). A v1 lock reads as v2 with no
//! agents; the version bump exists so an older toolchain *refuses* a v2 lock
//! instead of parsing it, dropping the unknown `[agents]` table (there is no
//! `deny_unknown_fields`) and writing the lock back without it.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub fn lock_version() -> u32 {
    2
}

/// The last lock version without `[agents]`. [`parse`] reads it as the
/// current version with an empty agent table; the next build writes it back
/// at [`lock_version`]. `check`/`test` read it and never write (ADR 0493).
const LEGACY_EVENTS_ONLY_VERSION: u32 = 1;

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchemaRegistry {
    // No serde `default`: a registry with no `version` is not a fresh
    // project, it is corruption (a truncated write), and must fail the read
    // rather than silently re-baseline every event's history — the same
    // argument `bynk.deploy.lock`'s `DeployLock` makes for the identical
    // field.
    version: u32,
    #[serde(default)]
    events: BTreeMap<String, EventEntry>,
    /// Keyed `<unit>.<Agent>`. Skipped when empty, so a project with no
    /// agents writes exactly the events-only document it always did, apart
    /// from the version line.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    agents: BTreeMap<String, AgentEntry>,
}

impl SchemaRegistry {
    /// A fresh, empty registry at the current lock version — what
    /// `bynk-emit`'s `reconcile` starts from and fills in per-event via
    /// [`Self::insert`] as it walks the project's units. [DECISION A]: kept
    /// private-fielded so `version`/`events` stay encapsulated across the
    /// `bynk-emit` ↔ `bynk-project` boundary the way they always were
    /// in-crate.
    pub fn new() -> Self {
        SchemaRegistry {
            version: lock_version(),
            events: BTreeMap::new(),
            agents: BTreeMap::new(),
        }
    }

    /// This registry's entry for `key` (`<unit>.<EventName>`), when one
    /// exists — what `reconcile` diffs a unit's live event shape against.
    pub fn get(&self, key: &str) -> Option<&EventEntry> {
        self.events.get(key)
    }

    /// Record `entry` under `key`, overwriting any prior entry — what
    /// `reconcile` calls once per event as it rebuilds the updated document.
    pub fn insert(&mut self, key: String, entry: EventEntry) {
        self.events.insert(key, entry);
    }

    /// This registry's entry for agent class `key` (`<unit>.<Agent>`).
    pub fn get_agent(&self, key: &str) -> Option<&AgentEntry> {
        self.agents.get(key)
    }

    /// Record agent class `entry` under `key`, overwriting any prior entry.
    pub fn insert_agent(&mut self, key: String, entry: AgentEntry) {
        self.agents.insert(key, entry);
    }

    /// Raise agent class `key`'s floor to `floor`, dropping the snapshots
    /// below it (state-migrations S4, #2259 Decision F). The floor only rises:
    /// a `floor` at or below the recorded one changes nothing and returns
    /// `Ok(false)`. A `floor` above the class's current version, or a class the
    /// registry does not record, is refused.
    pub fn raise_floor(&mut self, key: &str, floor: i64) -> Result<bool, String> {
        let Some(entry) = self.agents.get_mut(key) else {
            return Err(format!("`bynk.schema.lock` records no agent `{key}`"));
        };
        if floor > entry.version {
            return Err(format!(
                "cannot raise agent `{key}`'s floor to {floor}: its current version is {}",
                entry.version
            ));
        }
        if floor <= entry.floor {
            return Ok(false);
        }
        entry.floor = floor;
        entry.shapes.retain(|s| s.version >= floor);
        Ok(true)
    }

    /// Every recorded agent class, in key order — what the agent reconcile
    /// walks to find classes that disappeared from source.
    pub fn agents(&self) -> impl Iterator<Item = (&String, &AgentEntry)> {
        self.agents.iter()
    }
}

/// One agent class's recorded state history (track §3.1).
///
/// `version` is the class's current shape version: every accepted change
/// bumps it, additive or not, so the S1 `__v` marker always names exactly one
/// entry in `shapes`. `floor` is the oldest version any deployed instance may
/// still hold; only a migration sweep (S4) raises it, and `shapes` keeps one
/// snapshot per version from `floor` to `version`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentEntry {
    pub version: i64,
    pub floor: i64,
    pub shapes: Vec<AgentShape>,
    /// State-migrations S3 (#2248, #2253 review): the old names this class was
    /// renamed from (`agent New replaces Old`), once a build moved their
    /// entries here. While the declaration stays in source, it tells an
    /// already-applied rename from a mistyped one. Additive and omitted when
    /// empty, so the lock format is unchanged for every class never renamed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renamed_from: Vec<String>,
}

impl AgentEntry {
    /// The entry's own invariants: `1 <= floor <= version`, and exactly one
    /// snapshot for every version from `floor` to `version`, in order. A
    /// hand-edit or a bad merge of two branches that each bumped the class can
    /// break them; reconciling against such an entry would compare with the
    /// wrong shape (#2204 review), so [`parse`] refuses it as corruption.
    fn validate(&self) -> Result<(), String> {
        if self.floor < 1 || self.floor > self.version {
            return Err(format!(
                "has floor {} outside 1..={}",
                self.floor, self.version
            ));
        }
        let versions: Vec<i64> = self.shapes.iter().map(|s| s.version).collect();
        let expected: Vec<i64> = (self.floor..=self.version).collect();
        if versions != expected {
            return Err(format!(
                "records shapes for versions {versions:?}, not one per version {}..={}",
                self.floor, self.version
            ));
        }
        Ok(())
    }

    /// The oldest stored version a build at this entry's `version` can read
    /// forward without a migration step: the smallest `m >= floor` such that
    /// every snapshot after `m`, up to `version`, was an additive change
    /// (state-migrations S1, #2211 Decision B). A record at `m` or later loads
    /// by zero-merge (ADR 0124 D4); one below it has no path.
    pub fn oldest_readable(&self) -> i64 {
        self.oldest_readable_to(self.version)
    }

    /// [`Self::oldest_readable`] measured against `target` instead of the
    /// current version: the oldest version whose path to `target` is entirely
    /// additive. A `migrate from target` step covers every version from there
    /// up to `target` (state-migrations S2, #2224 Decision C).
    pub fn oldest_readable_to(&self, target: i64) -> i64 {
        let mut m = target;
        while m > self.floor && self.shapes.iter().any(|s| s.version == m && s.additive) {
            m -= 1;
        }
        m
    }

    /// The snapshot at the entry's current `version`.
    pub fn current(&self) -> Option<&AgentShape> {
        self.shape(self.version)
    }

    /// The snapshot recorded at `version`, if the entry still holds it.
    pub fn shape(&self, version: i64) -> Option<&AgentShape> {
        self.shapes.iter().find(|s| s.version == version)
    }
}

/// The stored shape of one agent class at one version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentShape {
    pub version: i64,
    /// Whether the change *into* this version was additive (a baseline is
    /// not: nothing preceded it).
    pub additive: bool,
    pub fields: Vec<StoreShape>,
}

/// One `store` field's stored shape. `ty` is the structural rendering of the
/// field's type with every refinement predicate stripped; `refinement` is
/// the same walk with predicates kept (an opaque type's included). A change
/// to `ty` is non-additive; a change to `refinement` alone is not.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoreShape {
    pub name: String,
    pub kind: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub refinement: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventEntry {
    pub schema: i64,
    pub fields: Vec<FieldShape>,
}

/// A shallow, per-field snapshot of an event's current shape — deliberately
/// **not** `bynk-check/src/contract.rs`'s `canon_named_in`: that renders a
/// field as `name: type` with no signal for default-presence, so an additive
/// change (new field, has a default) and a breaking one (new field, no
/// default) perturb it identically. This snapshot exists to tell those two
/// apart, one field at a time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldShape {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub default: bool,
}

/// Parse `bynk.schema.lock`'s content. `existing: None` means the project
/// has no lock file yet — a fresh project's first reconciliation, baselined
/// rather than compared, exactly as a missing file always meant back when
/// this function read the file itself. It must mean *verified absent*, never
/// "content unavailable" for some other reason — see `SchemaLock::On`'s doc
/// comment, which states that invariant on the type the caller constructs.
///
/// `Some(text)` that is empty/unparseable/a different lock version is
/// corruption, not a fresh project, and fails hard (`ledger.rs`'s own
/// argument for the identical case) — never silently re-baselines.
///
/// `project_root` is used only to name the file in an error message — #1078
/// made this function disk-free (and so path-blind) for the content itself;
/// #1085 review restored the location context a corruption diagnostic lost
/// as a result, without reintroducing any `fs`/`Path` I/O here.
pub fn parse(existing: Option<&str>, project_root: &Path) -> Result<SchemaRegistry, String> {
    let Some(text) = existing else {
        return Ok(SchemaRegistry::new());
    };
    let path = project_root.join("bynk.schema.lock");
    if text.trim().is_empty() {
        return Err(format!(
            "schema registry `{}` is empty or truncated (corrupt); refusing \
             to treat it as a fresh project — restore it from version control",
            path.display()
        ));
    }
    let reg: SchemaRegistry = toml::from_str(text).map_err(|e| {
        format!(
            "schema registry `{}` is corrupt ({e}) — restore it from version control",
            path.display()
        )
    })?;
    let mut reg = reg;
    if reg.version == LEGACY_EVENTS_ONLY_VERSION && reg.agents.is_empty() {
        reg.version = lock_version();
    }
    if reg.version != lock_version() {
        return Err(format!(
            "unsupported schema registry version {} (`{}`)",
            reg.version,
            path.display()
        ));
    }
    for (key, entry) in &reg.agents {
        if let Err(why) = entry.validate() {
            return Err(format!(
                "schema registry `{}` is corrupt: agent `{key}` {why} — restore it from \
                 version control",
                path.display()
            ));
        }
    }
    Ok(reg)
}

/// Serialize a reconciled registry to `bynk.schema.lock`'s TOML form —
/// what `bynk-driver` writes to disk, atomically, only on a fully clean
/// build (`compile_project`'s own gate). No disk access here; see this
/// module's doc comment for why. Cannot fail for this data shape (string
/// keys, no floats) — `toml::to_string_pretty` only errors on inputs this
/// struct never produces.
pub fn serialize(reg: &SchemaRegistry) -> String {
    toml::to_string_pretty(reg).expect("SchemaRegistry always serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(name: &str, ty: &str, default: bool) -> FieldShape {
        FieldShape {
            name: name.to_string(),
            ty: ty.to_string(),
            default,
        }
    }

    fn test_root() -> &'static Path {
        Path::new("/project")
    }

    #[test]
    fn new_registry_is_empty_at_the_current_lock_version() {
        let reg = SchemaRegistry::new();
        assert_eq!(reg.version, lock_version());
        assert!(reg.events.is_empty());
    }

    #[test]
    fn insert_then_get_round_trips_an_entry() {
        let mut reg = SchemaRegistry::new();
        let entry = EventEntry {
            schema: 2,
            fields: vec![shape("orderId", "String", false)],
        };
        reg.insert("commerce.order.PaymentConfirmed".to_string(), entry.clone());
        assert_eq!(reg.get("commerce.order.PaymentConfirmed"), Some(&entry));
        assert_eq!(reg.get("no.such.key"), None);
    }

    #[test]
    fn parse_of_absent_content_is_an_empty_registry() {
        let reg = parse(None, test_root()).unwrap();
        assert!(reg.events.is_empty());
    }

    #[test]
    fn parse_round_trips_serialized_content() {
        let mut reg = SchemaRegistry::new();
        reg.insert(
            "commerce.order.PaymentConfirmed".to_string(),
            EventEntry {
                schema: 2,
                fields: vec![shape("orderId", "String", false)],
            },
        );
        let body = serialize(&reg);
        assert_eq!(parse(Some(&body), test_root()).unwrap(), reg);
    }

    #[test]
    fn a_truncated_file_is_corruption_not_a_fresh_project() {
        let err = parse(Some("   \n"), test_root()).unwrap_err();
        assert!(err.contains("restore it from version control"));
        // Not a full path match (`\` vs `/` makes that Windows-fragile) —
        // just proving the filename made it into the message at all.
        assert!(err.contains("bynk.schema.lock"));
    }

    #[test]
    fn an_unparseable_file_is_corruption() {
        let err = parse(Some("not valid toml {{{"), test_root()).unwrap_err();
        assert!(err.contains("corrupt"));
        assert!(err.contains("bynk.schema.lock"));
    }

    #[test]
    fn a_v1_lock_reads_as_the_current_version_with_no_agents() {
        let v1 = "version = 1\n\n[events]\n";
        let reg = parse(Some(v1), test_root()).unwrap();
        assert_eq!(reg.version, lock_version());
        assert!(reg.agents.is_empty());
        assert!(serialize(&reg).starts_with("version = 2"));
    }

    #[test]
    fn a_v1_lock_that_somehow_carries_agents_is_refused() {
        let text = "version = 1\n\n[agents.\"u.A\"]\nversion = 1\nfloor = 1\nshapes = []\n";
        let err = parse(Some(text), test_root()).unwrap_err();
        assert!(err.contains("unsupported schema registry version 1"));
    }

    #[test]
    fn agent_entries_round_trip() {
        let mut reg = SchemaRegistry::new();
        reg.insert_agent(
            "shop.Cart".to_string(),
            AgentEntry {
                version: 2,
                floor: 1,
                shapes: vec![
                    AgentShape {
                        version: 1,
                        additive: false,
                        fields: vec![StoreShape {
                            name: "owner".to_string(),
                            kind: "Cell".to_string(),
                            ty: "String".to_string(),
                            refinement: "String".to_string(),
                        }],
                    },
                    AgentShape {
                        version: 2,
                        additive: true,
                        fields: vec![],
                    },
                ],
                renamed_from: Vec::new(),
            },
        );
        let body = serialize(&reg);
        assert_eq!(parse(Some(&body), test_root()).unwrap(), reg);
        assert_eq!(
            reg.get_agent("shop.Cart")
                .unwrap()
                .current()
                .unwrap()
                .version,
            2
        );
    }

    #[test]
    fn an_agent_entry_with_no_snapshot_for_its_version_is_corruption() {
        let text = "version = 2\n\n[agents.\"u.A\"]\nversion = 2\nfloor = 1\n\n\
            [[agents.\"u.A\".shapes]]\nversion = 1\nadditive = false\nfields = []\n";
        let err = parse(Some(text), test_root()).unwrap_err();
        assert!(err.contains("corrupt"), "{err}");
        assert!(err.contains("agent `u.A`"), "{err}");
    }

    #[test]
    fn an_agent_entry_with_its_floor_above_its_version_is_corruption() {
        let text = "version = 2\n\n[agents.\"u.A\"]\nversion = 1\nfloor = 2\nshapes = []\n";
        let err = parse(Some(text), test_root()).unwrap_err();
        assert!(err.contains("floor 2"), "{err}");
    }

    #[test]
    fn oldest_readable_walks_back_over_additive_snapshots_only() {
        let shape = |version, additive| AgentShape {
            version,
            additive,
            fields: vec![],
        };
        let entry = |version, floor, shapes| AgentEntry {
            version,
            floor,
            shapes,
            renamed_from: Vec::new(),
        };
        // Baseline only.
        assert_eq!(entry(1, 1, vec![shape(1, false)]).oldest_readable(), 1);
        // 1 -> 2 -> 3, all additive: everything back to the floor reads.
        let all = entry(3, 1, vec![shape(1, false), shape(2, true), shape(3, true)]);
        assert_eq!(all.oldest_readable(), 1);
        // 2 -> 3 non-additive (an S2 step): only 3 reads without one.
        let broken = entry(3, 1, vec![shape(1, false), shape(2, true), shape(3, false)]);
        assert_eq!(broken.oldest_readable(), 3);
        // 1 -> 2 non-additive, 2 -> 3 additive: 2 and 3 read.
        let mid = entry(3, 1, vec![shape(1, false), shape(2, false), shape(3, true)]);
        assert_eq!(mid.oldest_readable(), 2);
        // A raised floor bounds the walk.
        let floored = entry(3, 2, vec![shape(2, true), shape(3, true)]);
        assert_eq!(floored.oldest_readable(), 2);
    }

    #[test]
    fn raising_the_floor_drops_older_snapshots_and_stays_valid() {
        let snap = |version| AgentShape {
            version,
            additive: false,
            fields: Vec::new(),
        };
        let mut reg = SchemaRegistry::new();
        reg.insert_agent(
            "u.A".to_string(),
            AgentEntry {
                version: 3,
                floor: 1,
                shapes: vec![snap(1), snap(2), snap(3)],
                renamed_from: Vec::new(),
            },
        );
        assert_eq!(reg.raise_floor("u.A", 3), Ok(true));
        let entry = reg.get_agent("u.A").unwrap();
        assert_eq!(entry.floor, 3);
        assert_eq!(entry.shapes, vec![snap(3)]);
        // The edited lock reads back as valid.
        let text = serialize(&reg);
        assert_eq!(parse(Some(&text), test_root()).unwrap(), reg);
        // The floor never falls, and never passes the current version.
        assert_eq!(reg.raise_floor("u.A", 2), Ok(false));
        assert!(
            reg.raise_floor("u.A", 4)
                .unwrap_err()
                .contains("current version is 3")
        );
        assert!(reg.raise_floor("u.B", 1).is_err());
    }

    #[test]
    fn a_registry_with_no_agents_writes_no_agents_table() {
        assert!(!serialize(&SchemaRegistry::new()).contains("agents"));
    }

    #[test]
    fn an_unsupported_lock_version_is_corruption() {
        let err = parse(Some("version = 99\n"), test_root()).unwrap_err();
        assert!(err.contains("unsupported schema registry version 99"));
    }
}
