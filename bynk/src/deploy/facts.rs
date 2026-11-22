//! #3073: the facts a deployed Worker version carries about itself, so a later
//! `bynk revert` can judge a rollback without rebuilding anything.
//!
//! Each push sets one plain-text variable, [`FACTS_VAR`], to a compact JSON
//! [`Facts`] record (wrangler's `--var`, which applies to the environment being
//! deployed). It holds no secret: migration file names, contract hashes,
//! schema versions, ids, class names, like the committed lock. It lives on the
//! Worker *version*, so it is on every machine and in CI, and it is read back
//! from the version by the revert (never from a local build tree, which a CI
//! checkout does not have). A version without one, or one too large for a
//! variable, is simply not revertible.

use super::*;

/// The variable's name on the Worker.
pub(crate) const FACTS_VAR: &str = "BYNK_DEPLOY_FACTS";

/// Cloudflare caps a variable's size (5 KB as documented); stay well inside.
/// A record over this is not written, and that version cannot be reverted.
pub(crate) const MAX_FACTS_BYTES: usize = 4000;

const FACTS_VERSION: u32 = 1;

/// What a version was built against.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Facts {
    pub(crate) v: u32,
    /// D1 migration files by binding, in the order wrangler applies them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) migrations: BTreeMap<String, Vec<String>>,
    /// Agent class (`<context>.<Agent>`) → the schema version this build
    /// writes (its `__v`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) agents: BTreeMap<String, i64>,
    /// The storage layout this build writes (`bynk_emit::STORAGE_LAYOUT`), recorded
    /// only when it is beyond the original wholesale layout. `None` is layout 1, so
    /// a record from a build that did not know about layouts reads the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) layout: Option<i64>,
    /// The Durable Object classes the push declared, sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) classes: Vec<String>,
    /// The contract hash this Worker provides, by `on call` service.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) provides: BTreeMap<String, String>,
    /// The contracts it was compiled against: dependency → service → hash.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) expects: Expects,
    /// Foreign callers' contracts: actor → service → hash.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) foreign: BTreeMap<String, BTreeMap<String, String>>,
    /// The resource id each binding was pushed with (KV namespace id, D1
    /// database id, rate-limit namespace id).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) ids: BTreeMap<String, String>,
    /// #3562: the contexts a `[deploy.workers]` group's Worker held, in the manifest's
    /// order; empty for a context on its own Worker. A revert compares the two
    /// versions' members, since rolling a group back to a version with other
    /// members would take a context's code out from under it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) contexts: Vec<String>,
}

impl Facts {
    /// The storage layout this build writes; an unrecorded one is the original.
    pub(crate) fn layout(&self) -> i64 {
        self.layout.unwrap_or(1)
    }

    /// The compact JSON a push sets, or `None` when it would not fit.
    pub(crate) fn encode(&self) -> Option<String> {
        let text = serde_json::to_string(self).ok()?;
        (text.len() <= MAX_FACTS_BYTES).then_some(text)
    }

    /// A version's record as read back. Unreadable or of another version is
    /// `None`, which a revert reads as "no facts recorded".
    pub(crate) fn decode(text: &str) -> Option<Facts> {
        let facts: Facts = serde_json::from_str(text).ok()?;
        (facts.v == FACTS_VERSION).then_some(facts)
    }
}

/// What a pushed build records about itself. `agent_versions` is the schema
/// lock's `<context>.<Agent>` → version for this Worker's classes.
pub(crate) fn build_facts(
    worker: &str,
    environment: &str,
    declared: &Resources,
    manifest: &ContractsManifest,
    lock: &DeployLock,
    agent_versions: &BTreeMap<String, i64>,
    layout: &bynk_project::worker_layout::WorkerLayout,
) -> Facts {
    let mut ids = BTreeMap::new();
    // 1D.1 (#3929): each converted kind's bound ids.
    kind_bound_ids(
        LedgerView::of(lock, environment),
        worker,
        declared,
        &mut ids,
    );
    for binding in &declared.ratelimits {
        if let Some(id) = recorded_ratelimit(lock, worker, binding, environment) {
            ids.insert(binding.clone(), id.to_string());
        }
    }
    let mut classes: Vec<String> = declared
        .durable_objects
        .iter()
        .map(|d| d.class.clone())
        .collect();
    classes.sort();
    Facts {
        v: FACTS_VERSION,
        migrations: declared
            .d1_databases
            .iter()
            .filter(|d| !d.migrations.is_empty())
            .map(|d| (d.binding.clone(), d.migrations.clone()))
            .collect(),
        agents: agent_versions
            .iter()
            .filter(|(class, _)| crate::migrate::worker_of(layout, class) == worker)
            .map(|(c, v)| (c.clone(), *v))
            .collect(),
        layout: (bynk_emit::STORAGE_LAYOUT > 1).then_some(bynk_emit::STORAGE_LAYOUT),
        classes,
        provides: manifest.provides.clone(),
        expects: manifest.expects.clone(),
        foreign: manifest.foreign.clone(),
        ids,
        contexts: declared.members.clone(),
    }
}

/// The `--var` argument pair that sets the record on a push.
pub(crate) fn var_args(facts: &Facts) -> Vec<String> {
    match facts.encode() {
        Some(text) => vec!["--var".to_string(), format!("{FACTS_VAR}:{text}")],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Facts {
        Facts {
            v: FACTS_VERSION,
            migrations: BTreeMap::from([(
                "D1_ORDERS".to_string(),
                vec!["0001_init.sql".to_string()],
            )]),
            agents: BTreeMap::from([("shop.Cart".to_string(), 2)]),
            classes: vec!["Cart".to_string()],
            provides: BTreeMap::from([("quote".to_string(), "ab12".to_string())]),
            ids: BTreeMap::from([("KV".to_string(), "kv-1".to_string())]),
            ..Default::default()
        }
    }

    #[test]
    fn facts_round_trip_and_carry_no_empty_sections() {
        let f = sample();
        let text = f.encode().unwrap();
        assert!(
            !text.contains("expects") && !text.contains("foreign"),
            "{text}"
        );
        assert_eq!(Facts::decode(&text), Some(f));
    }

    #[test]
    fn a_layout_is_recorded_only_beyond_the_original_one() {
        // Layout 1 adds nothing to the record, so the record is as it was before layouts.
        let plain = sample();
        assert!(!plain.encode().unwrap().contains("layout"));
        assert_eq!(plain.layout(), 1);
        // A later layout round-trips.
        let later = Facts {
            layout: Some(2),
            ..sample()
        };
        let text = later.encode().unwrap();
        assert!(text.contains(r#""layout":2"#), "{text}");
        assert_eq!(Facts::decode(&text).map(|f| f.layout()), Some(2));
        // A record written before layouts existed reads as layout 1.
        assert_eq!(Facts::decode(r#"{"v":1}"#).map(|f| f.layout()), Some(1));
    }

    #[test]
    fn an_unknown_version_or_garbage_is_no_facts() {
        assert_eq!(Facts::decode("not json"), None);
        assert_eq!(Facts::decode(r#"{"v":99}"#), None);
        assert_eq!(Facts::decode("{}"), None);
    }

    #[test]
    fn a_record_over_the_limit_is_not_written() {
        let mut f = sample();
        for i in 0..400 {
            f.provides
                .insert(format!("service{i}"), "0123456789abcdef".into());
        }
        assert!(f.encode().is_none());
        assert!(var_args(&f).is_empty());
    }

    #[test]
    fn var_args_set_the_variable_in_wranglers_key_colon_value_form() {
        let args = var_args(&sample());
        assert_eq!(args[0], "--var");
        assert!(
            args[1].starts_with("BYNK_DEPLOY_FACTS:{\"v\":1"),
            "{args:?}"
        );
    }

    #[test]
    fn build_facts_gathers_ids_migrations_and_this_workers_agents() {
        let mut lock = DeployLock::default();
        lock.record::<Kv>("default", "shop", "KV", KvNamespace::of("kv-1"));
        let declared = Resources {
            needs_kv: true,
            d1_databases: vec![D1Decl {
                binding: "D1_ORDERS".into(),
                name: "shop-orders".into(),
                migrations: vec!["0001_init.sql".into()],
            }],
            durable_objects: vec![DurableObject {
                class: "Cart".into(),
                storage: "sqlite".into(),
            }],
            ..Default::default()
        };
        let versions =
            BTreeMap::from([("shop.Cart".to_string(), 3), ("other.Thing".to_string(), 9)]);
        let f = build_facts(
            "shop",
            "default",
            &declared,
            &ContractsManifest::default(),
            &lock,
            &versions,
            &bynk_project::worker_layout::WorkerLayout::identity(),
        );
        assert_eq!(f.ids.get("KV").map(String::as_str), Some("kv-1"));
        assert_eq!(f.migrations["D1_ORDERS"], ["0001_init.sql"]);
        assert_eq!(f.agents, BTreeMap::from([("shop.Cart".to_string(), 3)]));
        assert_eq!(f.classes, ["Cart"]);
    }

    /// #3686: a group's Worker carries its members' agent versions, which the
    /// ledger and `bynk migrate` key by the group's directory.
    #[test]
    fn a_group_workers_facts_carry_its_members_agent_versions() {
        let groups = bynk_project::worker_groups::WorkerGroups::from([(
            "store".to_string(),
            bynk_project::worker_groups::WorkerGroup {
                contexts: vec!["shop.cart".to_string()],
                ..Default::default()
            },
        )]);
        let versions = BTreeMap::from([
            ("shop.cart.Cart".to_string(), 3),
            ("other.Thing".to_string(), 9),
        ]);
        let f = build_facts(
            "store",
            "default",
            &Resources::default(),
            &ContractsManifest::default(),
            &DeployLock::default(),
            &versions,
            &bynk_project::worker_layout::WorkerLayout::from_groups(&groups),
        );
        assert_eq!(
            f.agents,
            BTreeMap::from([("shop.cart.Cart".to_string(), 3)])
        );
    }
}
