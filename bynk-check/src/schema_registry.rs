//! Events track, slice 3c (#980): the cross-build schema registry's
//! reconciliation logic — verifying a project's live event shapes against a
//! committed `bynk.schema.lock`, keyed by qualified event name, storing each
//! event's field shape (name, type, whether the field has a default) and its
//! current schema version. Every build reconciles the live shape against the
//! stored one and either leaves the version alone, bumps it (a purely
//! additive change), or fails the build (a change that isn't safely
//! additive).
//!
//! P4.0 (#1113, [DECISION A]): the document shape itself — `SchemaRegistry`,
//! `EventEntry`, `FieldShape`, and their `parse`/`serialize` — moved to
//! `bynk_project::schema_registry`; `bynk-emit/src/project/schema_registry.rs`
//! re-exports it there, unchanged, for every existing `schema_registry::X`
//! call site in `project.rs`.
//!
//! P5.3 (`design/tracks/semantics-in-the-checker.md` §6): [`reconcile`] and
//! its helpers moved here from that same `bynk-emit` module — category 1 of
//! `bynk-check/src/analysis.rs`'s own seven-category residual-gap accounting
//! ("gap in name only" at the time: `reconcile` only ever ran under
//! `SchemaLock::On`, and the analyse path always passed `SchemaLock::Off`, so
//! relocating it changed nothing observable — see that module's doc for why
//! R3.5 still required the move; #2081 later gave the analyse path the
//! caller's lock). `bynk-emit`'s `run_checks` is now this function's caller, not its
//! owner, the same way P4.0/P4.1 turned `project.rs` into a caller of
//! `bynk-project`/`bynk-check` throughout.
//!
//! P5.5 (§6, §3.2's "eighth site"): [`parse_or_diagnose`] moved here too —
//! `bynk.project.schema_registry_corrupt`, a real `CompileError::new`
//! construction the seven-category accounting above didn't cover (it isn't a
//! whole-project *check*, it's the read step immediately before
//! [`reconcile`]), found and scoped by name in §3.2 rather than left for a
//! future sweep to rediscover. `bynk_project::schema_registry::parse` itself
//! — the document-shape parse — stays in `bynk-project` (P4.0); only the
//! diagnostic construction on its error path moves, the same split
//! `reconcile`'s own move drew between document shape and checking.

use std::collections::BTreeMap;

use bynk_syntax::ast::{
    AgentDecl, EventDecl, Expr, ExprKind, MigrateDecl, MigrateItem, TypeBody, TypeRef,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use bynk_project::schema_registry::{
    AgentEntry, AgentShape, EventEntry, FieldShape, SchemaRegistry, StoreShape,
};

use crate::symbols::UnitTable;

/// Parse `bynk.schema.lock`'s content, diagnosing a corrupt file as
/// `bynk.project.schema_registry_corrupt` rather than handing the caller a
/// bare `String` to wrap itself. `existing`/`project_root` are exactly
/// `bynk_project::schema_registry::parse`'s own parameters — `None` (no lock
/// file yet) always succeeds with a fresh, empty registry.
pub fn parse_or_diagnose(
    existing: Option<&str>,
    project_root: &std::path::Path,
) -> Result<SchemaRegistry, CompileError> {
    bynk_project::schema_registry::parse(existing, project_root).map_err(|msg| {
        CompileError::new("bynk.project.schema_registry_corrupt", Span::default(), msg)
    })
}

/// A shallow, per-field snapshot of an event's current shape — deliberately
/// **not** `bynk-check/src/contract.rs`'s `canon_named_in`: that renders a
/// field as `name: type` with no signal for default-presence, so an additive
/// change (new field, has a default) and a breaking one (new field, no
/// default) perturb it identically. This snapshot exists to tell those two
/// apart, one field at a time.
fn snapshot(event: &EventDecl) -> Vec<FieldShape> {
    let mut fields: Vec<FieldShape> = event
        .body
        .fields
        .iter()
        .map(|f| FieldShape {
            name: f.name.name.clone(),
            ty: canon_type(&f.type_ref),
            default: f.init.is_some(),
        })
        .collect();
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    fields
}

/// A stable surface-form rendering of a `TypeRef`, for the registry's own
/// stored shape only. Deliberately **not** `bynk-fmt`'s `type_ref_to_string`
/// (private, and neither `bynk-emit` nor `bynk-check` depends on `bynk-fmt`)
/// — and deliberately not required to match either the formatter's or the
/// checker's own rendering byte-for-byte: this string is compared only
/// against itself, across builds, never displayed to a user. Exhaustive, no
/// wildcard arm, so a new `TypeRef` variant is a compile error here rather
/// than a silent "no signal" gap in the registry's diffing.
fn canon_type(t: &TypeRef) -> String {
    match t {
        TypeRef::Base(b, _) => b.name().to_string(),
        TypeRef::Named(id) => id.name.clone(),
        TypeRef::Result(a, b, _) => format!("Result[{}, {}]", canon_type(a), canon_type(b)),
        TypeRef::Option(t, _) => format!("Option[{}]", canon_type(t)),
        TypeRef::Effect(t, _) => format!("Effect[{}]", canon_type(t)),
        TypeRef::HttpResult(t, _) => format!("HttpResult[{}]", canon_type(t)),
        TypeRef::QueueResult(_) => "QueueResult".to_string(),
        TypeRef::Asset(_) => "Asset".to_string(),
        TypeRef::RequestHeaders(_) => "RequestHeaders".to_string(),
        TypeRef::TokenClaims(_) => "TokenClaims".to_string(),
        TypeRef::BoundaryRejection(_) => "BoundaryRejection".to_string(),
        TypeRef::List(t, _) => format!("List[{}]", canon_type(t)),
        TypeRef::Map(k, v, _) => format!("Map[{}, {}]", canon_type(k), canon_type(v)),
        TypeRef::Query(t, _) => format!("Query[{}]", canon_type(t)),
        TypeRef::Stream(t, _) => format!("Stream[{}]", canon_type(t)),
        TypeRef::Connection(t, _) => format!("Connection[{}]", canon_type(t)),
        TypeRef::History(t, _) => format!("History[{}]", canon_type(t)),
        TypeRef::ValidationError(_) => "ValidationError".to_string(),
        TypeRef::JsonError(_) => "JsonError".to_string(),
        TypeRef::Unit(_) => "()".to_string(),
        TypeRef::Fn(params, ret, _) => format!(
            "({}) -> {}",
            params.iter().map(canon_type).collect::<Vec<_>>().join(", "),
            canon_type(ret)
        ),
        TypeRef::App { name, args, .. } => format!(
            "{}[{}]",
            name.name,
            args.iter().map(canon_type).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// The outcome of reconciling one event against its (possibly absent)
/// registry entry.
enum Reconciled {
    /// No prior entry: baseline silently at `declared` (the event's current
    /// `@schema(N)`, or `1`).
    Baseline { fields: Vec<FieldShape> },
    /// Shape unchanged since the stored entry.
    Unchanged {
        fields: Vec<FieldShape>,
        stored: i64,
    },
    /// Every added field carries a default; nothing removed or retyped.
    Additive {
        fields: Vec<FieldShape>,
        bumped: i64,
    },
    /// A field was removed, retyped, added without a default, or lost a
    /// default it previously had.
    NonAdditive {
        removed: Vec<String>,
        retyped: Vec<String>,
        added_without_default: Vec<String>,
        lost_default: Vec<String>,
    },
}

fn reconcile_one(current: &[FieldShape], stored: Option<&EventEntry>) -> Reconciled {
    let Some(stored) = stored else {
        return Reconciled::Baseline {
            fields: current.to_vec(),
        };
    };
    if current == stored.fields.as_slice() {
        return Reconciled::Unchanged {
            fields: current.to_vec(),
            stored: stored.schema,
        };
    }

    fn by_name(fields: &[FieldShape]) -> BTreeMap<&str, &FieldShape> {
        fields.iter().map(|f| (f.name.as_str(), f)).collect()
    }
    let old = by_name(&stored.fields);
    let new = by_name(current);

    let removed: Vec<String> = old
        .keys()
        .filter(|n| !new.contains_key(*n))
        .map(|n| n.to_string())
        .collect();
    let retyped: Vec<String> = old
        .iter()
        .filter_map(|(n, old_field)| {
            new.get(n)
                .filter(|new_field| new_field.ty != old_field.ty)
                .map(|_| n.to_string())
        })
        .collect();
    let added_without_default: Vec<String> = new
        .iter()
        .filter(|(n, f)| !old.contains_key(*n) && !f.default)
        .map(|(n, _)| n.to_string())
        .collect();
    // A field present in both, same type, that had a default and no longer
    // does: an older wire event that omitted this key (relying on the
    // default) can no longer decode — the same forward-compatibility break
    // as a field added without one, just approached from the other side.
    let lost_default: Vec<String> = old
        .iter()
        .filter_map(|(n, old_field)| {
            new.get(n).filter(|new_field| {
                new_field.ty == old_field.ty && old_field.default && !new_field.default
            })
        })
        .map(|f| f.name.clone())
        .collect();

    if removed.is_empty()
        && retyped.is_empty()
        && added_without_default.is_empty()
        && lost_default.is_empty()
    {
        Reconciled::Additive {
            fields: current.to_vec(),
            bumped: stored.schema + 1,
        }
    } else {
        Reconciled::NonAdditive {
            removed,
            retyped,
            lost_default,
            added_without_default,
        }
    }
}

/// Reconcile every event across every unit against the given registry.
/// Returns the updated registry document and the effective schema version
/// for each event, keyed the same way as the document itself
/// (`<unit>.<EventName>`). Diagnostics for a mismatched `@schema(N)` or a
/// non-additive change are pushed onto `errors`.
pub fn reconcile(
    existing: &SchemaRegistry,
    unit_tables: &std::collections::HashMap<String, UnitTable>,
    errors: &mut Vec<CompileError>,
) -> (SchemaRegistry, std::collections::HashMap<String, i64>) {
    let mut updated = SchemaRegistry::new();
    let mut effective = std::collections::HashMap::new();

    let mut units: Vec<_> = unit_tables.iter().collect();
    units.sort_by_key(|(name, _)| *name);

    for (unit_name, table) in units {
        let mut events: Vec<_> = table.events.iter().collect();
        events.sort_by_key(|(name, _)| *name);
        for (event_name, event) in events {
            let key = format!("{unit_name}.{event_name}");
            let fields = snapshot(event);
            let declared = event.schema_version();
            let annotation_span = event
                .annotations
                .iter()
                .find(|a| a.name.name == "schema")
                .map(|a| a.span);
            let has_annotation = annotation_span.is_some();

            let (effective_version, entry) = match reconcile_one(&fields, existing.get(&key)) {
                Reconciled::Baseline { fields } => (
                    declared,
                    EventEntry {
                        schema: declared,
                        fields,
                    },
                ),
                Reconciled::Unchanged { fields, stored } => {
                    if has_annotation && declared != stored {
                        errors.push(mismatch_error(
                            event_name,
                            annotation_span.unwrap(),
                            declared,
                            stored,
                        ));
                    }
                    (
                        stored,
                        EventEntry {
                            schema: stored,
                            fields,
                        },
                    )
                }
                Reconciled::Additive { fields, bumped } => {
                    if has_annotation && declared != bumped {
                        errors.push(mismatch_error(
                            event_name,
                            annotation_span.unwrap(),
                            declared,
                            bumped,
                        ));
                    }
                    (
                        bumped,
                        EventEntry {
                            schema: bumped,
                            fields,
                        },
                    )
                }
                Reconciled::NonAdditive {
                    removed,
                    retyped,
                    added_without_default,
                    lost_default,
                } => {
                    errors.push(non_additive_error(
                        event,
                        event_name,
                        &removed,
                        &retyped,
                        &added_without_default,
                        &lost_default,
                    ));
                    // Keep the old entry as-is: a build that fails here
                    // must not perturb the registry (`compile_project`
                    // skips the write on any error too — this just keeps
                    // the in-memory document consistent with that).
                    let old = existing
                        .get(&key)
                        .expect("a NonAdditive verdict only fires against a stored entry")
                        .clone();
                    (old.schema, old)
                }
            };

            effective.insert(key.clone(), effective_version);
            updated.insert(key, entry);
        }
    }

    (updated, effective)
}

fn mismatch_error(
    event_name: &str,
    span: bynk_syntax::span::Span,
    declared: i64,
    computed: i64,
) -> CompileError {
    CompileError::new(
        "bynk.event.schema_version_mismatch",
        span,
        format!(
            "`{event_name}`'s `@schema({declared})` disagrees with the schema \
             registry, which computes version {computed} from the event's \
             build history"
        ),
    )
    .with_note(format!(
        "update the annotation to `@schema({computed})`, or remove it to let \
         the compiler track the version automatically"
    ))
}

fn non_additive_error(
    event: &EventDecl,
    event_name: &str,
    removed: &[String],
    retyped: &[String],
    added_without_default: &[String],
    lost_default: &[String],
) -> CompileError {
    let mut parts = Vec::new();
    if !removed.is_empty() {
        parts.push(format!("field(s) removed: {}", removed.join(", ")));
    }
    if !retyped.is_empty() {
        parts.push(format!("field(s) retyped: {}", retyped.join(", ")));
    }
    if !added_without_default.is_empty() {
        parts.push(format!(
            "field(s) added without a default: {}",
            added_without_default.join(", ")
        ));
    }
    if !lost_default.is_empty() {
        parts.push(format!(
            "field(s) lost their default: {}",
            lost_default.join(", ")
        ));
    }
    CompileError::new(
        "bynk.event.non_additive_schema_change",
        event.span,
        format!(
            "`{event_name}` changed in a way the schema registry cannot \
             evolve additively — {}",
            parts.join("; ")
        ),
    )
    .with_note(
        "an additive change adds only fields that carry a default; give a \
         breaking change a new event type name instead",
    )
}

// ---------------------------------------------------------------------------
// State-migrations S0 (#2204; the retired track's summary in
// `design/archive/retired-tracks.md`, §3.1, §3.4): the agent-state half of the
// registry. Every agent class's stored shape is
// recorded per version under `[agents]`, and a change deployed data cannot be
// read through is refused here, before it can orphan anything.
// ---------------------------------------------------------------------------

/// The stored shape of one agent class: one [`StoreShape`] per `store` field,
/// sorted by name (stored state is a JSON object, so order is invisible).
/// Each type is rendered twice by `contract.rs`'s canonical walk (track
/// §3.1, #2204 Decision A): structurally, and with refinements.
fn agent_snapshot(
    agent: &AgentDecl,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Vec<StoreShape> {
    let mut fields: Vec<StoreShape> = agent
        .store_fields
        .iter()
        .map(|f| {
            let (structure, full): (Vec<String>, Vec<String>) = f
                .kind
                .args
                .iter()
                .map(|a| crate::contract::canon_storage_type(a, types))
                .unzip();
            StoreShape {
                name: f.name.name.clone(),
                kind: f.kind.head.name.clone(),
                ty: structure.join(", "),
                refinement: full.join(", "),
            }
        })
        .collect();
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    fields
}

/// How one agent class's live shape relates to its recorded current shape.
#[derive(Debug, PartialEq, Eq)]
enum AgentChange {
    Unchanged,
    /// Fields added, and/or refinements changed with the structure intact.
    Additive {
        refined: Vec<String>,
    },
    NonAdditive {
        removed: Vec<String>,
        added: Vec<String>,
        retyped: Vec<String>,
        rekinded: Vec<String>,
    },
}

fn classify_agent_change(current: &[StoreShape], stored: &[StoreShape]) -> AgentChange {
    if current == stored {
        return AgentChange::Unchanged;
    }
    let old: BTreeMap<&str, &StoreShape> = stored.iter().map(|f| (f.name.as_str(), f)).collect();
    let new: BTreeMap<&str, &StoreShape> = current.iter().map(|f| (f.name.as_str(), f)).collect();
    let mut removed = Vec::new();
    let mut retyped = Vec::new();
    let mut rekinded = Vec::new();
    let mut refined = Vec::new();
    for (name, o) in &old {
        match new.get(name) {
            None => removed.push(name.to_string()),
            Some(n) if n.kind != o.kind => rekinded.push(name.to_string()),
            Some(n) if n.ty != o.ty => retyped.push(name.to_string()),
            Some(n) if n.refinement != o.refinement => refined.push(name.to_string()),
            Some(_) => {}
        }
    }
    let added: Vec<String> = new
        .keys()
        .filter(|n| !old.contains_key(*n))
        .map(|n| n.to_string())
        .collect();
    if removed.is_empty() && retyped.is_empty() && rekinded.is_empty() {
        AgentChange::Additive { refined }
    } else {
        AgentChange::NonAdditive {
            removed,
            added,
            retyped,
            rekinded,
        }
    }
}

/// The rename the author most likely made, when it is unambiguous: exactly
/// one removed field and exactly one added field share a storage kind and a
/// structure, and neither matches any other candidate. Never applied — only
/// named in the diagnostic's help (track §3.4: a pairing is never guessed).
fn unambiguous_rename<'a>(
    removed: &'a [String],
    added: &'a [String],
    stored: &[StoreShape],
    current: &[StoreShape],
) -> RenameHint<'a> {
    let shape_of = |fields: &[StoreShape], name: &str| {
        fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| (f.kind.clone(), f.ty.clone()))
    };
    let mut pairs = Vec::new();
    let mut ambiguous = false;
    for r in removed {
        let r_shape = shape_of(stored, r);
        let candidates: Vec<&String> = added
            .iter()
            .filter(|a| shape_of(current, a) == r_shape)
            .collect();
        match candidates.as_slice() {
            [] => {}
            [a] => {
                let rivals = removed
                    .iter()
                    .filter(|other| shape_of(stored, other) == r_shape)
                    .count();
                if rivals == 1 {
                    pairs.push((r.as_str(), a.as_str()));
                } else {
                    ambiguous = true;
                }
            }
            _ => ambiguous = true,
        }
    }
    match (ambiguous, pairs.as_slice()) {
        (true, _) => RenameHint::Ambiguous,
        (false, [one]) => RenameHint::One(one.0, one.1),
        _ => RenameHint::None,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RenameHint<'a> {
    None,
    One(&'a str, &'a str),
    Ambiguous,
}

/// The way past a refusal until the migration slices land (#2204 Decision C).
/// The toolchain cannot know whether a class was ever deployed — the deploy
/// ledger records no Durable Object classes (ADR 0194 D1, ADR 0443) — so the
/// note asks the author.
fn never_deployed_note(key: &str) -> String {
    format!(
        "if this class was never deployed, delete its `[agents.\"{key}\"]` entry from \
         `bynk.schema.lock` to record the new shape as a fresh baseline; if it was deployed, \
         widen rather than narrow, or stage the change through a new field (ADR 0124 D3), or \
         declare a `migrate from N` step that carries the old data forward"
    )
}

/// The two facts the emitter needs to stamp and check an agent's stored
/// state (state-migrations S1, #2211 Decision B): the class's lock `version`,
/// and the oldest stored version it reads forward without a migration step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentVersions {
    pub version: i64,
    pub oldest_readable: i64,
    /// State-migrations S2 (#2224 Decision C): each valid `migrate from N`
    /// step, sorted by `N`.
    pub steps: Vec<MigrationStep>,
}

/// What the emitter needs to run one `migrate from N` step (#2224 Decision D).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationStep {
    /// The stored versions the step covers: `from` and every older version,
    /// down to `lo`, whose path to `from` is entirely additive (Decision C).
    pub lo: i64,
    pub from: i64,
    /// Version `from`'s fields the current shape keeps with the same
    /// structure: carried over as they are.
    pub carried: Vec<String>,
    /// Every field version `from` recorded: a stored key outside this set
    /// (and `__v`) is an unknown key, kept rather than dropped.
    pub known: Vec<String>,
    /// The old type's `Map[K, V]` fields that read a store `Map` (persisted as
    /// a `Record`), rather than a `Cell` holding a map value.
    pub store_maps: Vec<String>,
}

/// The storage kinds a migration step can neither read nor assign: they have
/// no value type (#2224 Decision A).
fn kind_has_no_value(kind: &str) -> bool {
    matches!(kind, "Set" | "Log" | "Cache")
}

/// The fields a step reads off its `old` parameter (`old.f` in its body).
fn step_reads(step: &MigrateDecl) -> std::collections::BTreeSet<String> {
    fn walk(e: &Expr, old: &str, out: &mut std::collections::BTreeSet<String>) {
        if let ExprKind::FieldAccess { receiver, field } = &e.kind
            && matches!(&receiver.kind, ExprKind::Ident(id) if id.name == old)
        {
            out.insert(field.name.clone());
        }
        for c in bynk_syntax::ast::expr_children(e) {
            walk(c, old, out);
        }
    }
    let mut out = std::collections::BTreeSet::new();
    if let Some(old) = &step.old {
        for item in &step.items {
            match item {
                MigrateItem::Let(l) => walk(&l.value, &old.name.name, &mut out),
                MigrateItem::Assign(a) => walk(&a.value, &old.name.name, &mut out),
                MigrateItem::Drop(_) => {}
            }
        }
    }
    out
}

/// Check one `migrate from N` step against the lock (#2224 Decisions A, C): its
/// version is recorded and below the current one, each field of its old type
/// matches version N's recorded field on structure (refinement-only
/// differences warn), each `drop` names a real field, and it accounts for
/// every change from version N to the current shape. Returns whether it holds;
/// diagnostics go onto `errors`.
fn check_step(
    agent: &AgentDecl,
    step: &MigrateDecl,
    entry: &AgentEntry,
    current: &[StoreShape],
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
    errors: &mut Vec<CompileError>,
) -> Option<StepPlanParts> {
    let n = step.from;
    // State-migrations S4 (#2259 Decision G): a complete sweep raised the
    // floor past N. Versions are numbered contiguously from 1, so N was
    // recorded once, but no stored instance can hold it now: the step is dead
    // code, not an error. Its snapshot was dropped with the floor rise.
    if n >= 1 && n < entry.floor {
        errors.push(
            CompileError::new(
                "bynk.agent.migrate_below_floor",
                step.from_span,
                format!(
                    "`migrate from {n}`: agent `{}`'s floor is {}, so no stored instance can \
                     hold version {n}; this step can be deleted",
                    agent.name.name, entry.floor
                ),
            )
            .with_note("`bynk migrate sweep` raised the floor; the step emits nothing"),
        );
        return None;
    }
    let Some(snapshot) = entry
        .shape(n)
        .filter(|_| n >= entry.floor && n < entry.version)
    else {
        errors.push(
            CompileError::new(
                "bynk.agent.migrate_unknown_version",
                step.from_span,
                format!(
                    "`migrate from {n}`: the lock records agent `{}` at versions {}..={}, so a \
                     step can read a version from {} to {}",
                    agent.name.name,
                    entry.floor,
                    entry.version,
                    entry.floor,
                    entry.version - 1
                ),
            )
            .with_note("a step reads an older recorded version; the current version needs no step"),
        );
        return None;
    };
    let stored: BTreeMap<&str, &StoreShape> = snapshot
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f))
        .collect();
    let now: BTreeMap<&str, &StoreShape> = current.iter().map(|f| (f.name.as_str(), f)).collect();
    let mut ok = true;
    // The old type's fields that read a store `Map` (a `Record` on disk), as
    // opposed to a `Cell` holding a map value (already in its value wire form).
    let mut store_maps: Vec<String> = Vec::new();

    // The old type's fields against version N, on structure.
    if let Some(old) = &step.old
        && let TypeRef::Named(id) = &old.type_ref
        && let Some(decl) = types.get(&id.name)
        && let TypeBody::Record(r) = &decl.body
    {
        for f in &r.fields {
            // A `Map[K, V]` field names either a store `Map` of `K, V` or a
            // `Cell` whose value is a map: version N's recorded kind decides
            // (#2243 review). Every other type names a `Cell`.
            let recorded_kind = stored.get(f.name.name.as_str()).map(|s| s.kind.as_str());
            let (kind, ty, full) = match &f.type_ref {
                TypeRef::Map(k, v, _) if recorded_kind != Some("Cell") => {
                    let (ks, kf) = crate::contract::canon_storage_type(k, types);
                    let (vs, vf) = crate::contract::canon_storage_type(v, types);
                    ("Map", format!("{ks}, {vs}"), format!("{kf}, {vf}"))
                }
                other => {
                    let (s, full) = crate::contract::canon_storage_type(other, types);
                    ("Cell", s, full)
                }
            };
            if kind == "Map" {
                store_maps.push(f.name.name.clone());
            }
            match stored.get(f.name.name.as_str()) {
                Some(s) if s.kind == kind && s.ty == ty => {
                    if s.refinement != full {
                        errors.push(
                            CompileError::new(
                                "bynk.agent.refinement_changed",
                                f.span,
                                format!(
                                    "`{}.{}` differs from version {n}'s recorded field only in \
                                     its refinement",
                                    id.name, f.name.name
                                ),
                            )
                            .with_note(
                                "the step still reads the stored value; its output is validated \
                                 against the current type on load",
                            ),
                        );
                    }
                }
                Some(s) => {
                    ok = false;
                    errors.push(
                        CompileError::new(
                            "bynk.agent.migrate_shape_mismatch",
                            f.span,
                            format!(
                                "`{}.{}` is declared as `{kind}` of `{ty}`, but version {n} \
                                 recorded it as `{}` of `{}`",
                                id.name, f.name.name, s.kind, s.ty
                            ),
                        )
                        .with_note(
                            "declare the old field exactly as version N stored it, \
                             declaring a copy of any nested type that has changed since",
                        ),
                    );
                }
                None => {
                    ok = false;
                    errors.push(CompileError::new(
                        "bynk.agent.migrate_shape_mismatch",
                        f.span,
                        format!(
                            "`{}.{}`: agent `{}` had no field `{}` at version {n}",
                            id.name, f.name.name, agent.name.name, f.name.name
                        ),
                    ));
                }
            }
        }
    }

    let mut dropped = std::collections::BTreeSet::new();
    let mut assigned = std::collections::BTreeSet::new();
    for item in &step.items {
        match item {
            MigrateItem::Drop(d) => {
                let name = d.field.name.as_str();
                if !stored.contains_key(name) && !now.contains_key(name) {
                    ok = false;
                    errors.push(CompileError::new(
                        "bynk.agent.migrate_shape_mismatch",
                        d.field.span,
                        format!(
                            "`drop {name}`: agent `{}` has no field `{name}` at version {n} or now",
                            agent.name.name
                        ),
                    ));
                }
                dropped.insert(name.to_string());
            }
            MigrateItem::Assign(a) => {
                assigned.insert(a.target.name.clone());
            }
            MigrateItem::Let(_) => {}
        }
    }
    let reads = step_reads(step);

    // Coverage against the current shape (#2224 Decision C).
    let mut unread: Vec<String> = Vec::new();
    let mut unassigned: Vec<String> = Vec::new();
    for (name, s) in &stored {
        match now.get(name) {
            None => {
                if !reads.contains(*name) && !dropped.contains(*name) {
                    unread.push(name.to_string());
                }
            }
            Some(c) if c.kind != s.kind || c.ty != s.ty => {
                let no_value = kind_has_no_value(&c.kind) || kind_has_no_value(&s.kind);
                let covered = assigned.contains(*name) || (no_value && dropped.contains(*name));
                if !covered {
                    unassigned.push(name.to_string());
                }
            }
            Some(_) => {}
        }
    }
    if !unread.is_empty() || !unassigned.is_empty() {
        ok = false;
        let mut parts = Vec::new();
        if !unread.is_empty() {
            parts.push(format!(
                "removed field(s) neither read (`old.f`) nor dropped: {}",
                unread.join(", ")
            ));
        }
        if !unassigned.is_empty() {
            parts.push(format!(
                "changed field(s) not assigned: {}",
                unassigned.join(", ")
            ));
        }
        errors.push(
            CompileError::new(
                "bynk.agent.migrate_incomplete",
                step.keyword.merge(step.from_span),
                format!(
                    "`migrate from {n}` does not account for every change from version {n} to \
                     the current shape — {}",
                    parts.join("; ")
                ),
            )
            .with_note(
                "read a removed field through `old` or `drop` it; assign a changed field (a \
                 `Set`, `Log` or `Cache` field can only be dropped, which starts it afresh)",
            ),
        );
    }
    if !ok {
        return None;
    }
    let carried = stored
        .iter()
        .filter(|(name, s)| {
            now.get(*name)
                .is_some_and(|c| c.kind == s.kind && c.ty == s.ty)
        })
        .map(|(name, _)| name.to_string())
        .collect();
    let known = stored.keys().map(|n| n.to_string()).collect();
    Some(StepPlanParts {
        carried,
        known,
        store_maps,
    })
}

/// What a passing [`check_step`] hands to [`MigrationStep`].
#[derive(Debug, Clone, Default)]
struct StepPlanParts {
    carried: Vec<String>,
    known: Vec<String>,
    store_maps: Vec<String>,
}

/// Reconcile every agent class across every unit against `existing`, writing
/// each class's entry into `updated` (the document [`reconcile`] already
/// rebuilt for events). Diagnostics go onto `errors`; a refused change keeps
/// the class's stored entry untouched, as a refused event change does.
///
/// Returns each live class's [`AgentVersions`], keyed `<unit>.<Agent>`, read
/// from the *reconciled* entry — so a read-only `check`/`test` reports the
/// version the next build will record.
pub fn reconcile_agents(
    existing: &SchemaRegistry,
    updated: &mut SchemaRegistry,
    unit_tables: &std::collections::HashMap<String, UnitTable>,
    unit_uses: &std::collections::HashMap<String, Vec<String>>,
    errors: &mut Vec<CompileError>,
) -> std::collections::HashMap<String, AgentVersions> {
    let mut versions = std::collections::HashMap::new();
    let mut live = std::collections::BTreeSet::new();
    // State-migrations S3 (#2248, Decision C): recorded classes a declaration
    // accounts for — renamed away, or removed — so they are not reported
    // `class_removed` and are not carried forward under their old key.
    let mut accounted = std::collections::BTreeSet::new();
    let mut units: Vec<_> = unit_tables.iter().collect();
    units.sort_by_key(|(name, _)| *name);
    for (unit_name, table) in units {
        let types = crate::symbols::combined_types_for(unit_name, unit_tables, unit_uses);
        // A `removed agent` accounts for its recorded entry, which is dropped.
        // A name the lock doesn't record needs nothing: after the build that
        // drops it the lock can't tell "already removed" from "never
        // recorded", and Cloudflare treats a `deleted` tombstone for a
        // namespace that doesn't exist as stale, with no effect.
        for r in &table.removed_agents {
            let key = format!("{unit_name}.{}", r.name.name);
            if existing.get_agent(&key).is_some() {
                accounted.insert(key);
            }
        }
        let mut agents: Vec<_> = table.agents.iter().collect();
        agents.sort_by_key(|(name, _)| *name);
        for (agent_name, agent) in agents {
            let key = format!("{unit_name}.{agent_name}");
            live.insert(key.clone());
            let current = agent_snapshot(agent, &types);
            // The step that carries the stored version across this build's
            // non-additive change, when one is declared and checked here.
            let mut transition_step: Option<(i64, Option<StepPlanParts>)> = None;
            // State-migrations S3 (#2248, Decisions B, C): a rename takes the
            // old class's recorded history as this class's own.
            let mut history = existing.get_agent(&key);
            let mut renamed_now: Option<String> = None;
            if let Some(old) = &agent.replaces
                && !table.agents.contains_key(&old.name)
            {
                let old_key = format!("{unit_name}.{}", old.name);
                match (existing.get_agent(&old_key), history) {
                    // Already applied: an earlier build moved the entry here,
                    // and the declaration stays until Cloudflare reports its
                    // tombstone stale. `renamed_from` tells that apart from a
                    // mistyped old name (#2253 review).
                    (None, Some(h)) if h.renamed_from.contains(&old.name) => {}
                    (None, _) => errors.push(
                        CompileError::new(
                            "bynk.agent.replaces_unknown",
                            old.span,
                            format!(
                                "agent `{agent_name}` replaces `{}`, but `bynk.schema.lock` \
                                 records no class `{old_key}`",
                                old.name
                            ),
                        )
                        .with_note(
                            "a rename moves a recorded class's data and history; check the old \
                             name, and that it was built with this context",
                        ),
                    ),
                    (Some(_), Some(_)) => errors.push(
                        CompileError::new(
                            "bynk.agent.class_conflict",
                            agent.name.span,
                            format!(
                                "agent `{agent_name}` replaces `{}`, but `{key}` already has its \
                                 own recorded history",
                                old.name
                            ),
                        )
                        .with_note(
                            "renaming onto a recorded class would merge two histories (and \
                             Cloudflare refuses a rename onto an existing namespace)",
                        ),
                    ),
                    (Some(old_entry), None) => {
                        accounted.insert(old_key);
                        history = Some(old_entry);
                        renamed_now = Some(old.name.clone());
                    }
                }
            }
            let entry = match history {
                None => AgentEntry {
                    version: 1,
                    floor: 1,
                    shapes: vec![AgentShape {
                        version: 1,
                        additive: false,
                        fields: current.clone(),
                    }],
                    renamed_from: Vec::new(),
                },
                Some(stored_entry) => {
                    // `parse` refuses an entry whose `version` names no
                    // snapshot (`bynk.project.schema_registry_corrupt`), so
                    // there is always one to compare against.
                    let stored = stored_entry
                        .current()
                        .map(|s| s.fields.as_slice())
                        .expect("a parsed agent entry has a snapshot at its version");
                    match classify_agent_change(&current, stored) {
                        AgentChange::Unchanged => stored_entry.clone(),
                        AgentChange::Additive { refined } => {
                            for name in &refined {
                                errors.push(refinement_changed_warning(agent, name));
                            }
                            let mut next = stored_entry.clone();
                            next.version += 1;
                            next.shapes.push(AgentShape {
                                version: next.version,
                                additive: true,
                                fields: current.clone(),
                            });
                            next
                        }
                        AgentChange::NonAdditive {
                            removed,
                            added,
                            retyped,
                            rekinded,
                        } => {
                            // State-migrations S2 (#2224 Decision C): a
                            // `migrate from <stored version>` step that checks
                            // against the lock carries the change.
                            let from = stored_entry.version;
                            match agent.migrations.iter().find(|m| m.from == from) {
                                Some(step) => {
                                    let mut probe = stored_entry.clone();
                                    probe.version += 1;
                                    let plan =
                                        check_step(agent, step, &probe, &current, &types, errors);
                                    let accepted = plan.is_some();
                                    transition_step = Some((from, plan));
                                    if accepted {
                                        probe.shapes.push(AgentShape {
                                            version: probe.version,
                                            additive: false,
                                            fields: current.clone(),
                                        });
                                        probe
                                    } else {
                                        stored_entry.clone()
                                    }
                                }
                                None => {
                                    let hint =
                                        unambiguous_rename(&removed, &added, stored, &current);
                                    errors.push(non_additive_state_error(
                                        agent, &key, from, stored, &removed, &retyped, &rekinded,
                                        &hint,
                                    ));
                                    stored_entry.clone()
                                }
                            }
                        }
                    }
                }
            };
            // A rename applied by this build is remembered on the entry.
            let mut entry = entry;
            if let Some(old) = renamed_now
                && !entry.renamed_from.contains(&old)
            {
                entry.renamed_from.push(old);
            }
            // Every other step is checked against the reconciled entry and
            // the current shape; each valid one covers a range of versions.
            let mut steps = Vec::new();
            for step in &agent.migrations {
                let plan = match &transition_step {
                    Some((from, parts)) if *from == step.from => {
                        parts.clone().filter(|_| entry.version > step.from)
                    }
                    _ => check_step(agent, step, &entry, &current, &types, errors),
                };
                if let Some(parts) = plan {
                    steps.push(MigrationStep {
                        lo: entry.oldest_readable_to(step.from),
                        from: step.from,
                        carried: parts.carried,
                        known: parts.known,
                        store_maps: parts.store_maps,
                    });
                }
            }
            steps.sort_by_key(|s| s.from);
            let oldest_readable = entry.oldest_readable();
            for v in entry.floor..entry.version {
                let covered =
                    v >= oldest_readable || steps.iter().any(|s| s.lo <= v && v <= s.from);
                if !covered {
                    errors.push(
                        CompileError::new(
                            "bynk.agent.version_without_path",
                            agent.name.span,
                            format!(
                                "agent `{}`'s stored version {v} has no path to version {}: no \
                                 `migrate` step covers it, and its changes since were not all \
                                 additive",
                                agent.name.name, entry.version
                            ),
                        )
                        .with_note(format!(
                            "a stored record at version {v} faults on load (`NoMigrationPath`); \
                             add a `migrate from {v}` step, or ignore this if no instance can \
                             still hold it"
                        )),
                    );
                }
            }
            versions.insert(
                key.clone(),
                AgentVersions {
                    version: entry.version,
                    oldest_readable,
                    steps,
                },
            );
            updated.insert_agent(key, entry);
        }
    }

    for (key, entry) in existing.agents() {
        if live.contains(key) || accounted.contains(key) {
            continue;
        }
        errors.push(
            CompileError::new(
                "bynk.agent.class_removed",
                Span::default(),
                format!(
                    "agent class `{key}` is recorded in `bynk.schema.lock` but no longer \
                     declared — a deployment's stored instances of it would be orphaned"
                ),
            )
            .with_note(format!(
                "declare what became of it: `agent New replaces {class}` renames it (its data \
                 moves), `removed agent {class}` deletes its data; if `{key}` was never \
                 deployed, you can instead delete its `[agents.\"{key}\"]` entry from \
                 `bynk.schema.lock`",
                class = key.rsplit('.').next().unwrap_or(key)
            )),
        );
        updated.insert_agent(key.clone(), entry.clone());
    }
    versions
}

fn refinement_changed_warning(agent: &AgentDecl, field: &str) -> CompileError {
    let span = agent
        .store_fields
        .iter()
        .find(|f| f.name.name == field)
        .map_or(agent.name.span, |f| f.span);
    CompileError::new(
        "bynk.agent.refinement_changed",
        span,
        format!(
            "the refinement on `{}.{field}`'s stored type changed; a stored value that no \
             longer satisfies it faults on load (`RehydrationViolation`)",
            agent.name.name
        ),
    )
    .with_note(
        "widening a refinement is safe; if this narrows it, migrate the stored data before \
         deploying",
    )
}

#[allow(clippy::too_many_arguments)]
fn non_additive_state_error(
    agent: &AgentDecl,
    key: &str,
    stored_version: i64,
    stored_fields: &[StoreShape],
    removed: &[String],
    retyped: &[String],
    rekinded: &[String],
    hint: &RenameHint<'_>,
) -> CompileError {
    let mut parts = Vec::new();
    if !removed.is_empty() {
        parts.push(format!("field(s) removed: {}", removed.join(", ")));
    }
    if !retyped.is_empty() {
        parts.push(format!("field(s) retyped: {}", retyped.join(", ")));
    }
    if !rekinded.is_empty() {
        parts.push(format!(
            "field(s) changed storage kind: {}",
            rekinded.join(", ")
        ));
    }
    let mut err = CompileError::new(
        "bynk.agent.non_additive_state_change",
        agent.name.span,
        format!(
            "agent `{}`'s stored state changed in a way deployed data cannot be read \
             through — {}",
            agent.name.name,
            parts.join("; ")
        ),
    );
    match hint {
        RenameHint::One(from, to) => {
            err = err.with_note(format!(
                "`{from}` was removed and `{to}` added with the same stored shape: if this \
                 is a rename, deployed `{from}` values are not carried over to `{to}` — they \
                 would read as `{to}`'s zero"
            ));
            // State-migrations S2 (#2224): name the step that carries it. A
            // `Cell` whose stored type is a plain name spells the old type
            // exactly; anything structural is described instead.
            let old_ty = format!("{}V{stored_version}", agent.name.name);
            let shape = stored_fields.iter().find(|f| f.name == *from);
            err = err.with_note(match shape {
                Some(s) if s.kind == "Cell" && s.ty.chars().all(|c| c.is_alphanumeric()) => {
                    format!(
                        "to carry it, declare `type {old_ty} = {{ {from}: {} }}` and add \
                     `migrate from {stored_version} (old: {old_ty}) {{ {to} := old.{from} }}` to \
                     the agent",
                        s.ty
                    )
                }
                _ => format!(
                    "to carry it, declare a record type `{old_ty}` with `{from}` as version \
                     {stored_version} stored it, and add `migrate from {stored_version} (old: \
                     {old_ty}) {{ {to} := old.{from} }}` to the agent"
                ),
            });
        }
        RenameHint::Ambiguous => {
            err = err.with_note(
                "several removed and added fields share a stored shape, so which became \
                 which cannot be inferred; it will have to be declared",
            );
        }
        RenameHint::None => {}
    }
    err.with_note(never_deployed_note(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_syntax::ast::{
        Annotation, AnnotationArg, BaseType, Expr, ExprId, ExprKind, Ident, RecordBody, Trivia,
    };
    use bynk_syntax::span::Span;
    use std::collections::HashMap as StdHashMap;

    fn ident(name: &str) -> Ident {
        Ident {
            name: name.to_string(),
            span: Span::default(),
        }
    }

    fn int_lit(value: i64) -> Expr {
        Expr {
            id: ExprId::SYNTHETIC,
            kind: ExprKind::IntLit {
                value,
                lexeme: value.to_string(),
            },
            span: Span::default(),
        }
    }

    fn field(name: &str, ty: TypeRef, has_default: bool) -> bynk_syntax::ast::RecordField {
        bynk_syntax::ast::RecordField {
            documentation: None,
            trivia: Default::default(),
            name: ident(name),
            type_ref: ty,
            refinement: None,
            init: has_default.then(|| int_lit(0)),
            span: Span::default(),
        }
    }

    fn schema_annotation(n: i64) -> Annotation {
        Annotation {
            name: ident("schema"),
            args: vec![AnnotationArg {
                label: None,
                value: int_lit(n),
                span: Span::default(),
            }],
            span: Span::default(),
        }
    }

    fn event(
        name: &str,
        annotations: Vec<Annotation>,
        fields: Vec<bynk_syntax::ast::RecordField>,
    ) -> EventDecl {
        EventDecl {
            name: ident(name),
            annotations,
            body: RecordBody {
                trailing_comments: Default::default(),
                fields,
                span: Span::default(),
            },
            documentation: None,
            span: Span::default(),
            trivia: Trivia::default(),
        }
    }

    fn int_ty() -> TypeRef {
        TypeRef::Base(BaseType::Int, Span::default())
    }
    fn string_ty() -> TypeRef {
        TypeRef::Base(BaseType::String, Span::default())
    }

    fn shape(name: &str, ty: &str, default: bool) -> FieldShape {
        FieldShape {
            name: name.to_string(),
            ty: ty.to_string(),
            default,
        }
    }

    // -- canon_type -----------------------------------------------------

    #[test]
    fn canon_type_renders_base_and_generic_shapes() {
        assert_eq!(canon_type(&int_ty()), "Int");
        assert_eq!(
            canon_type(&TypeRef::Option(Box::new(string_ty()), Span::default())),
            "Option[String]"
        );
        assert_eq!(
            canon_type(&TypeRef::List(Box::new(int_ty()), Span::default())),
            "List[Int]"
        );
    }

    // -- reconcile_one ----------------------------------------------------

    #[test]
    fn no_entry_baselines_silently() {
        let current = vec![shape("orderId", "String", false)];
        match reconcile_one(&current, None) {
            Reconciled::Baseline { fields } => assert_eq!(fields, current),
            _ => panic!("expected Baseline"),
        }
    }

    #[test]
    fn unchanged_shape_keeps_stored_version() {
        let current = vec![shape("orderId", "String", false)];
        let stored = EventEntry {
            schema: 2,
            fields: current.clone(),
        };
        match reconcile_one(&current, Some(&stored)) {
            Reconciled::Unchanged { stored: v, .. } => assert_eq!(v, 2),
            _ => panic!("expected Unchanged"),
        }
    }

    #[test]
    fn additive_field_with_default_bumps_version() {
        let old = vec![shape("orderId", "String", false)];
        let new = vec![
            shape("orderId", "String", false),
            shape("region", "Region", true),
        ];
        let stored = EventEntry {
            schema: 1,
            fields: old,
        };
        match reconcile_one(&new, Some(&stored)) {
            Reconciled::Additive { bumped, .. } => assert_eq!(bumped, 2),
            _ => panic!("expected Additive"),
        }
    }

    #[test]
    fn field_removed_is_non_additive() {
        let old = vec![
            shape("orderId", "String", false),
            shape("region", "Region", false),
        ];
        let new = vec![shape("orderId", "String", false)];
        let stored = EventEntry {
            schema: 1,
            fields: old,
        };
        match reconcile_one(&new, Some(&stored)) {
            Reconciled::NonAdditive { removed, .. } => {
                assert_eq!(removed, vec!["region".to_string()])
            }
            _ => panic!("expected NonAdditive"),
        }
    }

    #[test]
    fn field_retyped_is_non_additive() {
        let old = vec![shape("orderId", "String", false)];
        let new = vec![shape("orderId", "Int", false)];
        let stored = EventEntry {
            schema: 1,
            fields: old,
        };
        match reconcile_one(&new, Some(&stored)) {
            Reconciled::NonAdditive { retyped, .. } => {
                assert_eq!(retyped, vec!["orderId".to_string()])
            }
            _ => panic!("expected NonAdditive"),
        }
    }

    #[test]
    fn field_added_without_default_is_non_additive() {
        let old = vec![shape("orderId", "String", false)];
        let new = vec![
            shape("orderId", "String", false),
            shape("region", "Region", false),
        ];
        let stored = EventEntry {
            schema: 1,
            fields: old,
        };
        match reconcile_one(&new, Some(&stored)) {
            Reconciled::NonAdditive {
                added_without_default,
                ..
            } => assert_eq!(added_without_default, vec!["region".to_string()]),
            _ => panic!("expected NonAdditive"),
        }
    }

    #[test]
    fn a_field_losing_its_default_is_non_additive() {
        // An older wire event that omitted `region` (relying on the default)
        // could no longer decode once the default is gone — the same
        // forward-compatibility break as adding a field without one, from
        // the other direction.
        let old = vec![
            shape("orderId", "String", false),
            shape("region", "String", true),
        ];
        let new = vec![
            shape("orderId", "String", false),
            shape("region", "String", false),
        ];
        let stored = EventEntry {
            schema: 1,
            fields: old,
        };
        match reconcile_one(&new, Some(&stored)) {
            Reconciled::NonAdditive { lost_default, .. } => {
                assert_eq!(lost_default, vec!["region".to_string()])
            }
            _ => panic!("expected NonAdditive"),
        }
    }

    // -- reconcile (full pipeline, with real EventDecls) -------------------

    fn table_with(events: Vec<(&str, EventDecl)>) -> UnitTable {
        UnitTable {
            kind: None,
            removed_agents: Vec::new(),
            types: StdHashMap::new(),
            fns: StdHashMap::new(),
            methods: StdHashMap::new(),
            capabilities: StdHashMap::new(),
            providers: StdHashMap::new(),
            services: StdHashMap::new(),
            agents: StdHashMap::new(),
            actors: StdHashMap::new(),
            exported_capabilities: Default::default(),
            events: events
                .into_iter()
                .map(|(n, e)| (n.to_string(), e))
                .collect(),
            flattened_caps: StdHashMap::new(),
            assets: StdHashMap::new(),
            derived: StdHashMap::new(),
        }
    }

    #[test]
    fn reconcile_baselines_a_brand_new_event_at_its_declared_annotation() {
        let e = event(
            "PaymentConfirmed",
            vec![schema_annotation(3)],
            vec![field("orderId", string_ty(), false)],
        );
        let mut units = StdHashMap::new();
        units.insert(
            "commerce.order".to_string(),
            table_with(vec![("PaymentConfirmed", e)]),
        );
        let existing = SchemaRegistry::new();
        let mut errors = Vec::new();
        let (updated, effective) = reconcile(&existing, &units, &mut errors);
        assert!(errors.is_empty(), "a first-ever compile must not error");
        assert_eq!(effective.get("commerce.order.PaymentConfirmed"), Some(&3));
        assert_eq!(
            updated
                .get("commerce.order.PaymentConfirmed")
                .unwrap()
                .schema,
            3
        );
    }

    #[test]
    fn reconcile_rejects_a_mismatched_schema_annotation_on_an_unchanged_event() {
        let e = event(
            "PaymentConfirmed",
            vec![schema_annotation(5)],
            vec![field("orderId", string_ty(), false)],
        );
        let mut units = StdHashMap::new();
        units.insert(
            "commerce.order".to_string(),
            table_with(vec![("PaymentConfirmed", e)]),
        );
        let mut existing = SchemaRegistry::new();
        existing.insert(
            "commerce.order.PaymentConfirmed".to_string(),
            EventEntry {
                schema: 2,
                fields: vec![shape("orderId", "String", false)],
            },
        );
        let mut errors = Vec::new();
        let (_, effective) = reconcile(&existing, &units, &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].category, "bynk.event.schema_version_mismatch");
        // The registry's computed version wins as the effective one, not the
        // author's stale annotation.
        assert_eq!(effective.get("commerce.order.PaymentConfirmed"), Some(&2));
    }

    #[test]
    fn reconcile_auto_bumps_an_unannotated_additive_change() {
        let e = event(
            "OrderCancelled",
            vec![],
            vec![
                field("orderId", string_ty(), false),
                field("reason", string_ty(), true),
            ],
        );
        let mut units = StdHashMap::new();
        units.insert(
            "commerce.order".to_string(),
            table_with(vec![("OrderCancelled", e)]),
        );
        let mut existing = SchemaRegistry::new();
        existing.insert(
            "commerce.order.OrderCancelled".to_string(),
            EventEntry {
                schema: 1,
                fields: vec![shape("orderId", "String", false)],
            },
        );
        let mut errors = Vec::new();
        let (updated, effective) = reconcile(&existing, &units, &mut errors);
        assert!(errors.is_empty());
        assert_eq!(effective.get("commerce.order.OrderCancelled"), Some(&2));
        assert_eq!(
            updated.get("commerce.order.OrderCancelled").unwrap().schema,
            2
        );
    }

    #[test]
    fn reconcile_rejects_a_non_additive_change_and_keeps_the_old_entry() {
        let e = event(
            "OrderCancelled",
            vec![],
            vec![field("orderId", string_ty(), false)],
        );
        let mut units = StdHashMap::new();
        units.insert(
            "commerce.order".to_string(),
            table_with(vec![("OrderCancelled", e)]),
        );
        let mut existing = SchemaRegistry::new();
        existing.insert(
            "commerce.order.OrderCancelled".to_string(),
            EventEntry {
                schema: 4,
                fields: vec![
                    shape("orderId", "String", false),
                    shape("reason", "String", false),
                ],
            },
        );
        let mut errors = Vec::new();
        let (updated, effective) = reconcile(&existing, &units, &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].category, "bynk.event.non_additive_schema_change");
        assert_eq!(effective.get("commerce.order.OrderCancelled"), Some(&4));
        assert_eq!(
            updated.get("commerce.order.OrderCancelled").unwrap().schema,
            4
        );
    }

    #[test]
    fn a_stale_key_for_a_renamed_event_is_dropped_silently() {
        // The old table had `PaymentConfirmed`; the new one only has
        // `PaymentConfirmedV2` (the track's prescribed rename-for-breaking-
        // change path). No diagnostic, and the old key does not survive into
        // the updated document.
        let e = event(
            "PaymentConfirmedV2",
            vec![],
            vec![field("orderId", string_ty(), false)],
        );
        let mut units = StdHashMap::new();
        units.insert(
            "commerce.order".to_string(),
            table_with(vec![("PaymentConfirmedV2", e)]),
        );
        let mut existing = SchemaRegistry::new();
        existing.insert(
            "commerce.order.PaymentConfirmed".to_string(),
            EventEntry {
                schema: 3,
                fields: vec![shape("orderId", "String", false)],
            },
        );
        let mut errors = Vec::new();
        let (updated, _) = reconcile(&existing, &units, &mut errors);
        assert!(errors.is_empty());
        assert!(updated.get("commerce.order.PaymentConfirmed").is_none());
        assert!(updated.get("commerce.order.PaymentConfirmedV2").is_some());
    }

    // `serialize`, and `parse`'s own parsing rules, are
    // `bynk_project::schema_registry`'s (P4.0, #1113) — covered by that
    // crate's own tests; nothing left to duplicate here.

    // -- parse_or_diagnose (P5.5) ------------------------------------------

    #[test]
    fn parse_or_diagnose_passes_through_a_valid_registry() {
        let mut reg = SchemaRegistry::new();
        reg.insert(
            "commerce.order.PaymentConfirmed".to_string(),
            EventEntry {
                schema: 1,
                fields: vec![shape("orderId", "String", false)],
            },
        );
        let text = bynk_project::schema_registry::serialize(&reg);
        let parsed = parse_or_diagnose(Some(&text), std::path::Path::new("/tmp"))
            .expect("a freshly serialized registry must parse");
        assert_eq!(
            parsed
                .get("commerce.order.PaymentConfirmed")
                .map(|e| e.schema),
            Some(1)
        );
    }

    #[test]
    fn parse_or_diagnose_reports_a_corrupt_registry_under_its_own_code() {
        let err = parse_or_diagnose(Some("not valid toml {{{"), std::path::Path::new("/tmp"))
            .expect_err("garbage content must not parse");
        assert_eq!(err.category, "bynk.project.schema_registry_corrupt");
    }

    #[test]
    fn parse_or_diagnose_with_no_existing_content_baselines_empty() {
        let parsed = parse_or_diagnose(None, std::path::Path::new("/tmp"))
            .expect("no lock file yet is not corruption");
        assert!(parsed.get("anything.at_all").is_none());
    }
}
