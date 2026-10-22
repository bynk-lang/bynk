//! `wrangler.toml` generation per Worker (v0.8 §4.4 / §4.5).
//!
//! Each context becomes a Cloudflare Worker with its own wrangler config.
//! Service Bindings are declared for every consumed context. Durable
//! Object bindings + `exports` entries are declared for every agent.

use crate::emitter::toml_doc::{TomlBlock, TomlDocument, TomlEntry, TomlValue};
use crate::project::worker_dir_name;
pub(crate) use bynk_check::worker_bindings::{BindingSource, WorkerBinding};

/// Compile-time pinned compatibility date. Cloudflare uses this to lock
/// Workers runtime behaviour. When to bump it, and the gate a bump must pass,
/// is the policy in `design/bynk-release-discipline.md` ("Part 3 — The Workers
/// compatibility date", #1677): reviewed at each release, and moved only after
/// the workerd smokes pass on the new date.
pub const COMPATIBILITY_DATE: &str = "2026-07-01";

/// #1890: the compatibility flags every emitted Worker carries, ahead of the
/// project's own (`bynk.toml`'s `[workers] compatibility_flags`).
///
/// `global_fetch_strictly_public` makes `fetch()` to a URL go out over the
/// public internet even when the URL is another Worker on the same account
/// or zone. Without it Cloudflare refuses such a fetch (error 1042, or a
/// fast `404` from a `workers.dev` sibling), so a Bynk `Fetch` to a sibling
/// service's public URL failed where the same URL worked from outside. In
/// Bynk, `Fetch` to a URL means over the public internet, which is exactly
/// this flag's behaviour. Worker-to-Worker calls inside one project use
/// Service Bindings, which the flag leaves alone.
///
/// Reviewed with [`COMPATIBILITY_DATE`] (`design/bynk-release-discipline.md`,
/// Part 3): a date bump can turn a flag on by default, which makes it
/// redundant here.
pub const DEFAULT_COMPATIBILITY_FLAGS: &[&str] = &["global_fetch_strictly_public"];

/// #1890: the flags a Worker's `wrangler.toml` carries, and the duplicates
/// dropped on the way. [`DEFAULT_COMPATIBILITY_FLAGS`] come first, then
/// `extra` (the manifest's list) in order. A name already present, whether a
/// default or earlier in `extra`, is dropped and returned in the second list,
/// once per extra occurrence, for the caller to warn about. Names are not
/// otherwise checked.
pub fn compatibility_flags(extra: &[String]) -> (Vec<String>, Vec<String>) {
    let mut flags: Vec<String> = DEFAULT_COMPATIBILITY_FLAGS
        .iter()
        .map(|f| f.to_string())
        .collect();
    let mut duplicates = Vec::new();
    for flag in extra {
        if flags.contains(flag) {
            duplicates.push(flag.clone());
        } else {
            flags.push(flag.clone());
        }
    }
    (flags, duplicates)
}

/// #1732: the oldest wrangler whose bundled `workerd` serves
/// [`COMPATIBILITY_DATE`]. An older `workerd` refuses a newer date outright
/// (`wrangler dev` exits: "This Worker requires compatibility date …"), so
/// `bynk doctor` warns about a wrangler below this. Set by the same
/// compatibility-date review that sets the date (Part 3's lag rule keeps it at
/// least three months old). `bynk-emit/tests/compat_date_policy.rs` checks the
/// policy doc names both.
pub const WRANGLER_MIN: &str = "4.107.0";

/// Events track, slice 0 (spine #936, ADR 0284): the class name of a
/// publishing context's fan-out Durable Object (`emitter::events_fanout`).
/// Shared with `emitter::workers` (the `Env` field name + `deps.
/// __eventsDispatch` call it drives) and `emitter::events_fanout` (the class
/// this name must actually export) so the three can never drift apart.
///
/// Double-underscore-prefixed, matching every other compiler-synthesised
/// identifier in emitted output (`__events`, `__eventsDispatch`,
/// `__makeLedger`, …) — a Bynk `agent` name can never start with `_` (a
/// parse error, checked directly: `agent _Foo { … }` fails with
/// `expected identifier after \`agent\`, found \`_\``), so an agent
/// coincidentally named the same as this synthetic class is structurally
/// impossible, not merely unlikely.
pub const EVENTS_FANOUT_CLASS_NAME: &str = "__EventsFanout";

/// Deploy-time sentinel in generated Worker configuration. The driver replaces
/// this with the persistent Cloudflare KV namespace id immediately before a
/// remote Wrangler command runs.
pub const KV_NAMESPACE_ID_PLACEHOLDER: &str = "<KV_NAMESPACE_ID>";

/// The bare `Kv`'s binding (`KV`), re-exported for `bynk deploy`, which reads
/// generated configs and tells the bare namespace from derived instances by
/// it (S4b, #1967).
pub use bynk_check::firstparty::KV_BINDING_NAME;

/// #1883, slice 1b: the `[assets]` block for a Worker that ships static files.
/// `directory` is the Worker's own assets directory
/// ([`crate::project::WORKER_ASSETS_DIR`]), beside — never containing — its
/// compiled code. The three routing settings are set explicitly rather than
/// inherited (#1883 [DECISION C], checked against the wrangler reference):
///
/// - `run_worker_first = false`: a matching asset is served without running
///   the Worker, which is what makes an asset public by construction.
/// - `not_found_handling = "none"`: a request no asset matches falls through
///   to the Worker, so every handler stays reachable.
/// - `html_handling = "none"`: an asset is served only at its exact path (the
///   default would also serve `foo.html` at `/foo`), so the route-conflict
///   check can compare paths exactly.
pub(crate) fn assets_block() -> TomlBlock {
    TomlBlock::table(
        "assets",
        vec![
            TomlEntry::kv(
                "directory",
                TomlValue::str(format!("./{}", crate::project::WORKER_ASSETS_DIR)),
            ),
            TomlEntry::kv("run_worker_first", TomlValue::Bool(false)),
            TomlEntry::kv("not_found_handling", TomlValue::str("none")),
            TomlEntry::kv("html_handling", TomlValue::str("none")),
        ],
    )
}

/// #2321: the explicit module rule for a Worker whose adapter closure reaches
/// an included WebAssembly module: `**/*.wasm` is compiled by Wrangler to a
/// `WebAssembly.Module` (the shim's `import module from "./x.wasm"`). Wrangler's
/// default rules already say this; stating it keeps the dependency visible and
/// stops it changing under us. `fallthrough = true` leaves the other default
/// rules in force.
pub(crate) fn wasm_rule_block() -> TomlBlock {
    TomlBlock::array_table(
        "rules",
        vec![
            TomlEntry::kv("type", TomlValue::str("CompiledWasm")),
            TomlEntry::kv("globs", TomlValue::Array(vec![TomlValue::str("**/*.wasm")])),
            TomlEntry::kv("fallthrough", TomlValue::Bool(true)),
        ],
    )
}

/// #1883, slice 1b: the `_headers` file written into a Worker's assets
/// directory beside its files. Every served path carries its content hash, so
/// a file under [`bynk_check::assets::SERVED_PREFIX`] never changes and may
/// be cached for a year. Without it the platform's default is `max-age=0,
/// must-revalidate`. The glob is built from the prefix, so the two cannot
/// drift (review of #1904).
pub(crate) fn asset_headers() -> String {
    format!(
        "{}*\n  Cache-Control: public, max-age=31536000, immutable\n",
        bynk_check::assets::SERVED_PREFIX
    )
}

/// State-migrations S3 (#2248): one context's agent class renames
/// (`(old, new)`) and removals, as `exports` tombstones.
#[derive(Debug, Default)]
pub(crate) struct ClassTombstones {
    pub renamed: Vec<(String, String)>,
    pub deleted: Vec<String>,
}

impl ClassTombstones {
    /// From the context's declarations: each agent's `replaces`, and each
    /// `removed agent`, skipping an old name still declared live (the checker
    /// already refuses that).
    pub(crate) fn of(table: &bynk_check::symbols::UnitTable) -> Self {
        let mut renamed: Vec<(String, String)> = table
            .agents
            .values()
            .filter_map(|a| {
                let old = a.replaces.as_ref()?;
                (!table.agents.contains_key(&old.name))
                    .then(|| (old.name.clone(), a.name.name.clone()))
            })
            .collect();
        renamed.sort();
        let mut deleted: Vec<String> = table
            .removed_agents
            .iter()
            .filter(|r| !table.agents.contains_key(&r.name.name))
            .map(|r| r.name.name.clone())
            .collect();
        deleted.sort();
        deleted.dedup();
        ClassTombstones { renamed, deleted }
    }
}

/// P6.x cutover slice 2 (#1191, narrowed from #1187): `crons`/`queues` arrive
/// pre-collected, sorted and deduped by the caller (`project.rs`'s own
/// `emit_wrangler_toml` call site) rather than being walked here off
/// `table.services`. Both used to match a handler's cron-schedule kind and a
/// service's queue-binding protocol directly, straight off the syntax tree —
/// this file's entire raw-syntax footprint, per #1191's own grounding.
/// Relocating just these two matches removed `wrangler.rs` from the
/// `ast_importers` probe outright — no `bynk_ir` equivalent exists to route
/// through instead: nothing builds a project-wide service IR at the call site
/// (#1191's Framing), and after #1542's Slice D2 nothing builds one anywhere.
///
/// P7.3 (#1303): builds a [`TomlDocument`] rather than writing text directly
/// — the caller (`project.rs`) prints it via `emitter::toml_doc::
/// print_toml_document`. What gets built, in what order, under what gates,
/// is unchanged; only the construction shape moved from direct `writeln!`
/// into pushing typed blocks. String-escaping moved with it: every value
/// below is escaped unconditionally by the printer now, not selectively by
/// this function (`emitter/toml_doc.rs`'s own module doc has the reasoning).
pub(crate) fn emit_wrangler_toml(
    context: &str,
    // #1944: every binding this Worker declares, [`worker_bindings`]' list —
    // the same one its `Env` interface and the duplicate check read.
    bindings: &[WorkerBinding],
    // v0.10a: every `on cron "expr"` schedule in the context, sorted+deduped.
    // Sorting is load-bearing, not cosmetic (review of #1192): the caller
    // walks `table.services`, a `HashMap`, so unsorted input makes
    // `wrangler.toml` non-reproducible across runs.
    crons: &[String],
    // v0.10b/v0.44: every `from queue("name")` service's bound queue name,
    // sorted+deduped (same reproducibility requirement as `crons`).
    queues: &[String],
    // #1890: the merged flag list, [`compatibility_flags`]' first result.
    // The same for every Worker in the project, so the caller merges (and
    // warns about duplicates) once, not per Worker.
    flags: &[String],
    // State-migrations S3 (#2248): the context's declared class renames and
    // removals, each emitted as an `exports` tombstone.
    tombstones: &ClassTombstones,
) -> TomlDocument {
    let name = worker_dir_name(context);
    let mut doc = TomlDocument::new(
        "Generated by bynkc — do not edit by hand.",
        vec![
            TomlEntry::kv("name", TomlValue::str(name)),
            TomlEntry::kv("main", TomlValue::str("index.ts")),
            TomlEntry::kv("compatibility_date", TomlValue::str(COMPATIBILITY_DATE)),
            TomlEntry::kv(
                "compatibility_flags",
                TomlValue::Array(flags.iter().map(TomlValue::str).collect()),
            ),
        ],
    );

    let mut class_names: Vec<&str> = Vec::new();
    for binding in bindings {
        let name = TomlValue::str(binding.name.clone());
        let block = match &binding.source {
            BindingSource::Service { context } => TomlBlock::array_table(
                "services",
                vec![
                    TomlEntry::kv("binding", name),
                    TomlEntry::kv("service", TomlValue::str(worker_dir_name(context))),
                ],
            ),
            BindingSource::Kv => TomlBlock::array_table(
                "kv_namespaces",
                vec![
                    TomlEntry::kv("binding", name),
                    TomlEntry::with_comment(
                        "id",
                        TomlValue::str(KV_NAMESPACE_ID_PLACEHOLDER),
                        "set at deploy time",
                    ),
                ],
            ),
            // S4a (#1966): an instance's own placeholder, named for its binding,
            // so deploy (#1967) can mint and materialise each one apart.
            BindingSource::KvInstance { .. } => TomlBlock::array_table(
                "kv_namespaces",
                vec![
                    TomlEntry::kv("binding", name),
                    TomlEntry::with_comment(
                        "id",
                        TomlValue::str(kv_instance_id_placeholder(&binding.name)),
                        "set at deploy time",
                    ),
                ],
            ),
            // P5 (#2323): the bucket's derived name, which `bynk deploy`
            // environment-qualifies; R2 bucket names are scoped to the
            // account, so the name is a function of the Worker and the
            // capability alone.
            BindingSource::R2Instance { jurisdiction, .. } => {
                let mut entries = vec![
                    TomlEntry::kv("binding", name),
                    TomlEntry::kv(
                        "bucket_name",
                        TomlValue::str(r2_bucket_name(&worker_dir_name(context), &binding.name)),
                    ),
                ];
                if let Some(jurisdiction) = jurisdiction {
                    entries.push(TomlEntry::kv(
                        "jurisdiction",
                        TomlValue::str(jurisdiction.clone()),
                    ));
                }
                TomlBlock::array_table("r2_buckets", entries)
            }
            // Interop I5 (#1972): the Worker as the declaration names it,
            // not a project context's `worker_dir_name`.
            BindingSource::ForeignService { worker, .. } => TomlBlock::array_table(
                "services",
                vec![
                    TomlEntry::kv("binding", name),
                    TomlEntry::kv("service", TomlValue::str(worker.clone())),
                ],
            ),
            BindingSource::Agent { class_name } | BindingSource::EventsFanout { class_name } => {
                class_names.push(class_name);
                TomlBlock::array_table(
                    "durable_objects.bindings",
                    vec![
                        TomlEntry::kv("name", name),
                        TomlEntry::kv("class_name", TomlValue::str(class_name.clone())),
                    ],
                )
            }
        };
        doc.push_block(block);
    }
    // #1796: each class is declared in Cloudflare's declarative `exports`
    // map, not a `[[migrations]]` list. Migrations are picked by tag: Wrangler
    // uploads only the ones after the Worker's applied tag, so the single
    // fixed `v1` this used to emit registered the classes of the *first*
    // deploy and no class added after it (a new agent, or the fan-out class
    // arriving with a context's first `emit`). `exports` has no tag. On every
    // deploy Cloudflare compares the declared set with the Worker's
    // namespaces and creates what's missing, so the config stays a pure
    // function of the source and the ledger still records nothing (ADR 0194
    // D1's principle). Wrangler reads `exports` from 4.107.0, which is
    // [`WRANGLER_MIN`], and `wrangler dev` takes each class's backend from
    // it too.
    //
    // Every class is SQLite-backed (#1779, ADR 0438): the emitted agent uses
    // only `storage.get`/`storage.put` on its single `"state"` key, which a
    // SQLite-backed class supports unchanged, and the fan-out class uses no
    // storage at all. A Worker whose classes an older Bynk created
    // key-value-backed (`new_classes`, ≤ 0.309.10, Workers Paid only: the
    // Free plan refuses that backend with code 10097) is refused
    // (`storage_type_mismatch`, per Cloudflare's `exports` docs), because
    // Cloudflare can't change a backend in place. That is a documented
    // pre-1.0 break: such a Worker is torn down and redeployed. A Worker
    // whose classes the `new_sqlite_classes` migration created takes them
    // over into `exports` with their state (#1809, live).
    //
    // State-migrations S3 (#2248): a class leaves only through a declaration.
    // `agent New replaces Old` emits a `renamed` tombstone (Cloudflare moves
    // Old's stored data to New) and `removed agent Old` a `deleted` one (the
    // data is destroyed, after a named confirmation at deploy). A class that
    // simply disappears from source still fails the deploy with Cloudflare's
    // `orphaned_provisioned_namespace` (confirmed live in #1809) — and, before
    // that, `bynk check` with `bynk.agent.class_removed`. A tombstoned class
    // gets no binding.
    for class_name in class_names {
        doc.push_block(TomlBlock::keyed_table(
            "exports",
            class_name.to_string(),
            vec![
                TomlEntry::kv("type", TomlValue::str("durable-object")),
                TomlEntry::kv("storage", TomlValue::str("sqlite")),
            ],
        ));
    }
    let mut tombstones_sorted: Vec<(&str, Option<&str>)> = tombstones
        .renamed
        .iter()
        .map(|(old, new)| (old.as_str(), Some(new.as_str())))
        .chain(tombstones.deleted.iter().map(|old| (old.as_str(), None)))
        .collect();
    tombstones_sorted.sort();
    for (old, renamed_to) in tombstones_sorted {
        let mut entries = vec![TomlEntry::kv("type", TomlValue::str("durable-object"))];
        match renamed_to {
            Some(new) => {
                entries.push(TomlEntry::kv("state", TomlValue::str("renamed")));
                entries.push(TomlEntry::kv("renamed_to", TomlValue::str(new)));
            }
            None => entries.push(TomlEntry::kv("state", TomlValue::str("deleted"))),
        }
        doc.push_block(TomlBlock::keyed_table("exports", old.to_string(), entries));
    }

    // v0.10a: cron triggers. Cloudflare uses a single `[triggers]` table with a
    // `crons` array aggregating every `on cron` schedule in the context.
    // Already sorted+deduped by the caller (#1191).
    if !crons.is_empty() {
        let quoted = crons.iter().map(TomlValue::str).collect();
        doc.push_block(TomlBlock::table(
            "triggers",
            vec![TomlEntry::kv("crons", TomlValue::Array(quoted))],
        ));
    }

    // v0.10b: queue consumers. Each `on queue "name"` becomes a
    // `[[queues.consumers]]` binding. Already sorted+deduped by the caller
    // (#1191).
    for name in queues {
        doc.push_block(TomlBlock::array_table(
            "queues.consumers",
            vec![
                TomlEntry::kv("queue", TomlValue::str(name.clone())),
                TomlEntry::kv("max_batch_size", TomlValue::Int(10)),
            ],
        ));
    }

    doc
}

/// Every binding a Worker's `wrangler.toml` declares, in emission order:
/// Service Bindings by target, then those to foreign Workers by capability,
/// then the KV namespaces, then the Durable Objects by class name. [`emit_wrangler_toml`] writes exactly this list,
/// the Worker's `Env` interface (`emitter::workers`) types it, and the
/// duplicate check (`bynk_check::worker_bindings`) reads it, so a new kind of
/// binding is declared, typed and checked from one place (#1944). Each kind
/// derives its names independently, so two can meet (an agent `Kv` and the KV
/// namespace are both `KV`); names are never adjusted to avoid that, because
/// deployed Workers depend on them. Two members with one name would also be
/// invalid TypeScript in `Env`, so the duplicate check must run first.
pub(crate) fn worker_bindings(
    // The context's agents (`UnitTable::agents`' keys), in any order.
    agents: &[String],
    consumes: &[String],
    needs_kv: bool,
    // The context's derived instances, each its binding with a
    // `KvInstance` (S4a, #1966) or `ForeignService` (interop I5, #1972)
    // source, in capability order.
    instances: &[WorkerBinding],
    uses_emit: bool,
) -> Vec<WorkerBinding> {
    let mut bindings = Vec::new();
    let mut sorted_consumes: Vec<&String> = consumes.iter().collect();
    sorted_consumes.sort();
    bindings.extend(sorted_consumes.into_iter().map(|t| service_binding(t)));
    // Interop I5 (#1972): Service Bindings to foreign Workers, beside the
    // ones to project contexts.
    bindings.extend(
        instances
            .iter()
            .filter(|b| matches!(b.source, BindingSource::ForeignService { .. }))
            .cloned(),
    );
    if needs_kv {
        bindings.push(kv_binding());
    }
    bindings.extend(
        instances
            .iter()
            .filter(|b| matches!(b.source, BindingSource::KvInstance { .. }))
            .cloned(),
    );
    // P5 (#2323): the R2 buckets, after the KV namespaces.
    bindings.extend(
        instances
            .iter()
            .filter(|b| matches!(b.source, BindingSource::R2Instance { .. }))
            .cloned(),
    );
    // Events track, slice 0 (spine #936, ADR 0284): a context whose handlers
    // emit gets its own fan-out DO among the agents' — Cloudflare only cares
    // that `index.ts` (this Worker's `main`) exports a class with this name,
    // not which generated file it came from. Sources by provenance, not by
    // name: an agent that happened to share the fan-out's class name is
    // still an agent.
    let mut objects: Vec<(&str, WorkerBinding)> = agents
        .iter()
        .map(|class_name| (class_name.as_str(), agent_binding(class_name)))
        .collect();
    if uses_emit {
        objects.push((EVENTS_FANOUT_CLASS_NAME, events_fanout_binding()));
    }
    objects.sort_by(|a, b| a.0.cmp(b.0));
    bindings.extend(objects.into_iter().map(|(_, binding)| binding));
    bindings
}

/// #1950: the bindings a handler's `deps.env` type exposes on Workers — the
/// projection of [`worker_bindings`] handlers reach directly, which
/// `BindingSource`'s variant docs state per kind. `consumed` is the consumed
/// contexts the handler calls (#1961), `agents` every local agent when the
/// handler constructs one (else empty); order as in [`worker_bindings`].
///
/// Built by the same constructors, so each name and source is the Worker's
/// own: a called context is never an adapter (an adapter may not declare a
/// `service`, `bynk.adapter.disallowed_item`), and the Worker binds every
/// context any of its bodies calls, so it has a Service Binding; every local
/// agent has a Durable Object binding. The
/// KV namespace, the events fan-out and subscribers' Service Bindings never
/// reach handlers.
pub(crate) fn handler_bindings(consumed: &[String], agents: &[String]) -> Vec<WorkerBinding> {
    let mut consumed: Vec<&String> = consumed.iter().collect();
    consumed.sort();
    let mut agents: Vec<&String> = agents.iter().collect();
    agents.sort();
    consumed
        .into_iter()
        .map(|t| service_binding(t))
        .chain(agents.into_iter().map(|a| agent_binding(a)))
        .collect()
}

/// The TypeScript type a binding has on the Worker's `env`, for both its
/// `Env` interface and a handler's `deps.env`.
pub(crate) fn binding_ts_type(source: &BindingSource) -> &'static str {
    match source {
        BindingSource::Service { .. } | BindingSource::ForeignService { .. } => "__ServiceBinding",
        BindingSource::Kv | BindingSource::KvInstance { .. } => "__KVNamespace",
        BindingSource::R2Instance { .. } => "__R2Bucket",
        BindingSource::Agent { .. } | BindingSource::EventsFanout { .. } => {
            "__DurableObjectNamespace"
        }
    }
}

// #1958: the constructors below are the only callers of the two name
// functions, so every place that declares, types or reads a binding (the
// emitted `env.X`, the fan-out's routing table, the integration harness)
// gets its name from the one constructor for its kind. A constructor is
// `pub(crate)` exactly when something outside this file reads its name.

/// The Service Binding to context `target`'s Worker.
pub(crate) fn service_binding(target: &str) -> WorkerBinding {
    WorkerBinding {
        name: consumed_binding_name(target),
        source: BindingSource::Service {
            context: target.to_string(),
        },
    }
}

/// Interop I5 (#1972): the status of the stand-in for a foreign Worker that
/// `bynk dev` binds by default and the integration test harness binds always.
/// 503, not a 2xx: the stand-in served nothing, and code that ignores the
/// status should not mistake it for the Worker's answer.
pub const FOREIGN_SERVICE_DOUBLE_STATUS: u16 = 503;

/// Interop I5 (#1972): the body of that stand-in's every response, saying
/// what it is. `bynk dev` adds how to reach the real Worker instead.
pub fn foreign_service_double_body(binding: &str, worker: &str) -> String {
    format!(
        "bynk: `{binding}` is bound to a stand-in for Worker `{worker}`, which is not part of this project."
    )
}

/// S4a (#1966): the deploy-time id placeholder of a derived KV instance's
/// `[[kv_namespaces]]` entry: `KV_SESSIONS` → `<KV_SESSIONS_ID>`.
pub fn kv_instance_id_placeholder(binding: &str) -> String {
    format!("<{binding}_ID>")
}

/// The bare KV namespace binding the cloudflare adapter's `Kv` reads. Its one
/// TypeScript read is the default of `WorkersKv`'s `binding` parameter in
/// `bynk-std`'s `cloudflare.binding.ts`, pinned to `KV_BINDING_NAME` by a test.
fn kv_binding() -> WorkerBinding {
    WorkerBinding {
        name: bynk_check::firstparty::KV_BINDING_NAME.to_string(),
        source: BindingSource::Kv,
    }
}

/// The Durable Object binding of agent `class_name`.
pub(crate) fn agent_binding(class_name: &str) -> WorkerBinding {
    WorkerBinding {
        name: agent_binding_name(class_name),
        source: BindingSource::Agent {
            class_name: class_name.to_string(),
        },
    }
}

/// The Durable Object binding of the events fan-out class.
pub(crate) fn events_fanout_binding() -> WorkerBinding {
    WorkerBinding {
        name: agent_binding_name(EVENTS_FANOUT_CLASS_NAME),
        source: BindingSource::EventsFanout {
            class_name: EVENTS_FANOUT_CLASS_NAME.to_string(),
        },
    }
}

/// Service Binding identifier for a consumed context: uppercase with
/// underscores. `commerce.payment` → `COMMERCE_PAYMENT`.
fn consumed_binding_name(target: &str) -> String {
    target.replace('.', "_").to_uppercase()
}

/// Durable Object binding identifier for an agent class. We use the
/// class name in screaming snake case so handlers can grab it by a
/// predictable name (`OrderEntity` → `ORDER_ENTITY`), the casing derived
/// capabilities' bindings share.
fn agent_binding_name(class_name: &str) -> String {
    bynk_check::worker_bindings::screaming_snake(class_name)
}

// ---------------------------------------------------------------------------
// P7.4 (#1305): structural post-emission patches — closes R7.6 ("downstream
// consumers couple to nodes, never to emitted text") and R8.20 ("deploy-time
// placeholders are typed, not textual"). `bynk deploy`/`bynk dev --remote`
// and `--emit js` both need to rewrite one field of an *already-emitted*
// `wrangler.toml` after compilation — not build one from scratch (that's
// `emit_wrangler_toml`/`TomlDocument`, P7.3's own construction-side tree).
//
// This is the read-an-existing-document side, and needs a different tool
// than `TomlDocument`'s own from-scratch builder or a plain `toml::Table`
// parse: both discard everything they don't model (`toml::Table` round-trips
// through a `BTreeMap` with no comment/order trivia at all — a first attempt
// using it here silently dropped the "Generated by bynkc" banner and every
// inline comment, and re-sorted the whole document alphabetically, on every
// patch). `toml_edit::DocumentMut` is format-preserving: parse, mutate the
// one value that actually changed, re-serialise, and everything else survives
// byte-for-byte. That's the real shape "immune to a reformat, scoped to the
// actual field" needs — not just "found the right field," but "touched
// nothing else."
// ---------------------------------------------------------------------------

/// Set `item`'s value to `s`, preserving whatever leading/trailing
/// whitespace and comments already decorated it — e.g. the KV namespace
/// id's own `# set at deploy time` note survives materialisation (stale
/// once set, but that's the same text a caller would have seen under the
/// old substring-replace behaviour too; not this slice's concern to change).
fn set_string_preserving_decor(item: &mut toml_edit::Item, s: &str) {
    let decor = item
        .as_value()
        .map(|v| v.decor().clone())
        .unwrap_or_default();
    let mut new_value = toml_edit::Value::from(s);
    *new_value.decor_mut() = decor;
    *item = toml_edit::Item::Value(new_value);
}

/// The deploy-time placeholder a `[[kv_namespaces]]` entry for `binding`
/// carries until `bynk deploy` materialises it: [`KV_NAMESPACE_ID_PLACEHOLDER`]
/// for the bare `KV`, [`kv_instance_id_placeholder`] for a derived instance
/// (S4b, #1967).
pub fn kv_id_placeholder(binding: &str) -> String {
    if binding == bynk_check::firstparty::KV_BINDING_NAME {
        KV_NAMESPACE_ID_PLACEHOLDER.to_string()
    } else {
        kv_instance_id_placeholder(binding)
    }
}

/// S4b (#1967): the account-facing title stem of a derived instance's KV
/// namespace, `<worker>-<capability, kebab>`: `shop-cart` and `KV_SESSIONS`
/// give `shop-cart-sessions`, `KV_PRICE_CACHE` gives `shop-cart-price-cache`.
/// Read off the binding, which is a function of the capability's name, so a
/// renamed capability is a new title. `bynk deploy` environment-qualifies it.
pub fn kv_instance_title(worker: &str, binding: &str) -> String {
    let prefix = format!("{}_", bynk_check::firstparty::KV_BINDING_NAME);
    let stem = binding.strip_prefix(&prefix).unwrap_or(binding);
    format!("{worker}-{}", stem.to_ascii_lowercase().replace('_', "-"))
}

/// P5 (#2323): the account-facing name of an `R2` instance's bucket,
/// `<worker>-<capability, kebab>`: `shop-cart` and `R2_ARCHIVES` give
/// `shop-cart-archives`. R2 bucket names are scoped to the account (not the
/// Worker), so the Worker's name carries the uniqueness. `bynk deploy`
/// environment-qualifies it.
pub fn r2_bucket_name(worker: &str, binding: &str) -> String {
    let prefix = format!("{}_", bynk_check::firstparty::R2_BINDING_PREFIX);
    let stem = binding.strip_prefix(&prefix).unwrap_or(binding);
    format!("{worker}-{}", stem.to_ascii_lowercase().replace('_', "-"))
}

/// A generated `wrangler.toml`'s `[[r2_buckets]]` entries as `(binding,
/// bucket_name, jurisdiction)`, in file order; an entry missing a binding or
/// a name is skipped.
pub fn r2_bucket_entries(text: &str) -> Result<Vec<(String, String, Option<String>)>, String> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    Ok(doc
        .get("r2_buckets")
        .and_then(|i| i.as_array_of_tables())
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some((
                t.get("binding")?.as_str()?.to_string(),
                t.get("bucket_name")?.as_str()?.to_string(),
                t.get("jurisdiction")
                    .and_then(|j| j.as_str())
                    .map(str::to_string),
            ))
        })
        .collect())
}

/// P5 (#2323): point `[[r2_buckets]]` entries at their deploy-time bucket
/// names, structurally, by binding: each entry whose binding `names` lists gets
/// that `bucket_name`. The generated name is the unqualified derived one
/// ([`r2_bucket_name`]); an environment qualifies it. Unchanged text when
/// nothing differs, as [`materialise_kv_namespace_ids`].
pub fn materialise_r2_bucket_names(
    text: &str,
    names: &std::collections::BTreeMap<String, String>,
) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    let mut changed = false;
    if let Some(entries) = doc
        .get_mut("r2_buckets")
        .and_then(|i| i.as_array_of_tables_mut())
    {
        for entry in entries.iter_mut() {
            let Some(name) = entry
                .get("binding")
                .and_then(|b| b.as_str())
                .and_then(|b| names.get(b))
                .cloned()
            else {
                continue;
            };
            if let Some(item) = entry.get_mut("bucket_name")
                && item.as_str() != Some(name.as_str())
            {
                set_string_preserving_decor(item, &name);
                changed = true;
            }
        }
    }
    Ok(if changed {
        doc.to_string()
    } else {
        text.to_string()
    })
}

/// The bindings of a generated `wrangler.toml`'s `[[kv_namespaces]]` entries
/// whose `id` is still that binding's deploy-time placeholder
/// ([`kv_id_placeholder`]), in file order: what [`materialise_kv_namespace_ids`]
/// would patch. Exposed so a caller can learn which ids it needs before doing
/// the (possibly fallible) work of finding them, as `bynk dev --remote` does
/// before it reads the ledger. Empty for a config with no KV, or one already
/// materialised.
pub fn kv_bindings_needing_ids(text: &str) -> Result<Vec<String>, String> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    Ok(kv_entries(&doc)
        .filter(|(binding, id)| *id == kv_id_placeholder(binding))
        .map(|(binding, _)| binding.to_string())
        .collect())
}

/// Materialise deploy-time KV namespace ids in a generated `wrangler.toml`'s
/// text, structurally, by binding (S4b, #1967). Each `[[kv_namespaces]]` entry
/// whose binding has an id in `ids`, and whose `id` is currently *exactly* that
/// binding's placeholder ([`kv_id_placeholder`]), gets the id; the decor around
/// the value (the `# set at deploy time` note) is kept.
///
/// Any other state — no `[[kv_namespaces]]` section, an entry already
/// materialised, a binding `ids` doesn't name — is left alone, and when nothing
/// changes `text` is returned unchanged (byte-for-byte: the document is never
/// re-serialised in that case, so there's nothing for a caller to mistake for
/// a real write). It is not a general "overwrite with the given id" operation:
/// re-provisioning is a different feature (#1305's Decision B).
pub fn materialise_kv_namespace_ids(
    text: &str,
    ids: &std::collections::BTreeMap<String, String>,
) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    let mut changed = false;
    if let Some(entries) = doc
        .get_mut("kv_namespaces")
        .and_then(|i| i.as_array_of_tables_mut())
    {
        for entry in entries.iter_mut() {
            let Some(binding) = entry.get("binding").and_then(|b| b.as_str()) else {
                continue;
            };
            let Some(id) = ids.get(binding) else {
                continue;
            };
            let placeholder = kv_id_placeholder(binding);
            if let Some(item) = entry.get_mut("id")
                && item.as_str() == Some(placeholder.as_str())
            {
                set_string_preserving_decor(item, id);
                changed = true;
            }
        }
    }
    Ok(if changed {
        doc.to_string()
    } else {
        text.to_string()
    })
}

/// Interop I5 (#1972): point Service Bindings at other Workers, structurally,
/// by binding: each `[[services]]` entry whose binding `targets` names gets
/// that `service`. `bynk dev` uses it to bind a foreign Worker to its local
/// stand-in, which runs under a name of its own so that it never answers for
/// the real Worker in wrangler's shared dev registry. Unchanged text when
/// nothing matches, as [`materialise_kv_namespace_ids`].
pub fn retarget_services(
    text: &str,
    targets: &std::collections::BTreeMap<String, String>,
) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    let mut changed = false;
    if let Some(entries) = doc
        .get_mut("services")
        .and_then(|i| i.as_array_of_tables_mut())
    {
        for entry in entries.iter_mut() {
            let Some(target) = entry
                .get("binding")
                .and_then(|b| b.as_str())
                .and_then(|b| targets.get(b))
                .cloned()
            else {
                continue;
            };
            if let Some(item) = entry.get_mut("service")
                && item.as_str() != Some(target.as_str())
            {
                set_string_preserving_decor(item, &target);
                changed = true;
            }
        }
    }
    Ok(if changed {
        doc.to_string()
    } else {
        text.to_string()
    })
}

/// Each `[[kv_namespaces]]` entry's `(binding, id)`, in file order; an entry
/// missing either is skipped.
fn kv_entries(doc: &toml_edit::DocumentMut) -> impl Iterator<Item = (&str, &str)> {
    doc.get("kv_namespaces")
        .and_then(|i| i.as_array_of_tables())
        .into_iter()
        .flatten()
        .filter_map(|t| Some((t.get("binding")?.as_str()?, t.get("id")?.as_str()?)))
}

#[cfg(test)]
mod patch_tests {
    use super::*;

    /// A full, realistic `wrangler.toml` — every stanza `emit_wrangler_toml`
    /// can produce, in its real order, with its real banner and inline
    /// comment (mirrors `bynkc/tests/fixtures/positive/
    /// 372_kv_agent_queue_workers/expected/workers/ops-hub/wrangler.toml`
    /// byte-for-byte). Review of #1305, finding 2: every test below asserts
    /// **exact** patched text against this fixture, not just "the changed
    /// field has the right value after a re-parse" — a re-parse is blind by
    /// construction to whatever the printer discarded, which is exactly how
    /// finding 1 (the whole-document alphabetical re-sort, dropped banner)
    /// survived a green suite the first time around.
    const REPRESENTATIVE: &str = "\
# Generated by bynkc — do not edit by hand.
name = \"ops-hub\"
main = \"index.ts\"
compatibility_date = \"2026-07-01\"
compatibility_flags = [\"global_fetch_strictly_public\"]

[[services]]
binding = \"PAYMENT\"
service = \"payment\"

[[kv_namespaces]]
binding = \"KV\"
id = \"<KV_NAMESPACE_ID>\" # set at deploy time

[[durable_objects.bindings]]
name = \"JOB_LEDGER\"
class_name = \"JobLedger\"

[exports.JobLedger]
type = \"durable-object\"
storage = \"sqlite\"

[[queues.consumers]]
queue = \"job-intake\"
max_batch_size = 10
";

    fn ids(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(b, i)| (b.to_string(), i.to_string()))
            .collect()
    }

    /// A Worker with the bare namespace and two derived instances (S4b,
    /// #1967), as the emitter writes it (fixture `1966_derived_kv_workers`
    /// plus the bare `KV`).
    const INSTANCES: &str = "\
[[kv_namespaces]]
binding = \"KV\"
id = \"<KV_NAMESPACE_ID>\" # set at deploy time

[[kv_namespaces]]
binding = \"KV_PRICE_CACHE\"
id = \"<KV_PRICE_CACHE_ID>\" # set at deploy time

[[kv_namespaces]]
binding = \"KV_SESSIONS\"
id = \"<KV_SESSIONS_ID>\" # set at deploy time
";

    /// Interop I5 (#1972): only the named bindings move; the rest of the
    /// file, comments included, is untouched, and a no-op is byte-identical.
    #[test]
    fn retarget_services_moves_only_the_named_bindings() {
        let text = "name = \"shop-billing\"\n\n[[services]]\nbinding = \"SHOP_FRONT\"\nservice = \"shop-front\"\n\n[[services]]\nbinding = \"SERVICE_BILLING\"\nservice = \"legacy-billing\" # foreign\n";
        let targets = std::collections::BTreeMap::from([(
            "SERVICE_BILLING".to_string(),
            "bynk-stand-in-shop-billing-legacy-billing".to_string(),
        )]);
        let out = retarget_services(text, &targets).unwrap();
        assert_eq!(
            out,
            text.replace(
                "service = \"legacy-billing\"",
                "service = \"bynk-stand-in-shop-billing-legacy-billing\""
            )
        );
        assert_eq!(retarget_services(&out, &targets).unwrap(), out);
        assert_eq!(
            retarget_services(text, &std::collections::BTreeMap::new()).unwrap(),
            text
        );
        assert!(retarget_services("not [ toml", &targets).is_err());
    }

    #[test]
    fn materialise_kv_namespace_ids_changes_only_the_bare_id() {
        let expected = REPRESENTATIVE.replacen("<KV_NAMESPACE_ID>", "abc123", 1);
        assert_eq!(
            materialise_kv_namespace_ids(REPRESENTATIVE, &ids(&[("KV", "abc123")])).unwrap(),
            expected
        );
        // The `# set at deploy time` note survives — the old text-replace
        // behaviour left it in place too (it only replaced the placeholder
        // substring, not the whole line), and this slice isn't the one
        // deciding whether that note should still be there post-materialisation.
        assert!(expected.contains("id = \"abc123\" # set at deploy time"));
    }

    /// S4b (#1967): each instance's placeholder gets its own id, and only its
    /// own: an id for one binding never lands in another's entry.
    #[test]
    fn materialise_kv_namespace_ids_patches_each_binding_apart() {
        let all = materialise_kv_namespace_ids(
            INSTANCES,
            &ids(&[
                ("KV", "bare"),
                ("KV_SESSIONS", "sess"),
                ("KV_PRICE_CACHE", "price"),
            ]),
        )
        .unwrap();
        assert_eq!(
            all,
            INSTANCES
                .replacen("<KV_NAMESPACE_ID>", "bare", 1)
                .replacen("<KV_PRICE_CACHE_ID>", "price", 1)
                .replacen("<KV_SESSIONS_ID>", "sess", 1)
        );
        let one =
            materialise_kv_namespace_ids(INSTANCES, &ids(&[("KV_SESSIONS", "sess")])).unwrap();
        assert_eq!(one, INSTANCES.replacen("<KV_SESSIONS_ID>", "sess", 1));
        assert_eq!(
            kv_bindings_needing_ids(&one).unwrap(),
            ["KV", "KV_PRICE_CACHE"]
        );
    }

    /// A placeholder belongs to its binding: the bare placeholder under an
    /// instance's binding is not that instance's to patch.
    #[test]
    fn materialise_kv_namespace_ids_matches_each_bindings_own_placeholder() {
        let text = "[[kv_namespaces]]\nbinding = \"KV_SESSIONS\"\nid = \"<KV_NAMESPACE_ID>\"\n";
        assert_eq!(
            materialise_kv_namespace_ids(text, &ids(&[("KV_SESSIONS", "sess")])).unwrap(),
            text
        );
        assert!(kv_bindings_needing_ids(text).unwrap().is_empty());
    }

    #[test]
    fn materialise_kv_namespace_ids_is_immune_to_reformatting() {
        let text = "[[kv_namespaces]]\nbinding = \"KV\"\nid = \"<KV_NAMESPACE_ID>\" # set at deploy time\n";
        let patched = materialise_kv_namespace_ids(text, &ids(&[("KV", "abc123")])).unwrap();
        assert_eq!(
            patched,
            "[[kv_namespaces]]\nbinding = \"KV\"\nid = \"abc123\" # set at deploy time\n"
        );
    }

    #[test]
    fn materialise_kv_namespace_ids_is_a_no_op_without_a_kv_namespaces_section() {
        let text = "name = \"api\"\nmain = \"index.ts\"\n";
        let patched = materialise_kv_namespace_ids(text, &ids(&[("KV", "abc123")])).unwrap();
        assert_eq!(patched, text);
    }

    #[test]
    fn materialise_kv_namespace_ids_is_a_no_op_once_already_materialised() {
        let text = "[[kv_namespaces]]\nbinding = \"KV\"\nid = \"already-real\"\n";
        let patched = materialise_kv_namespace_ids(text, &ids(&[("KV", "abc123")])).unwrap();
        assert_eq!(patched, text);
    }

    #[test]
    fn kv_bindings_needing_ids_matches_materialise_kv_namespace_ids_own_gate() {
        assert_eq!(
            kv_bindings_needing_ids(INSTANCES).unwrap(),
            ["KV", "KV_PRICE_CACHE", "KV_SESSIONS"]
        );
        assert!(
            kv_bindings_needing_ids("name = \"api\"\n")
                .unwrap()
                .is_empty()
        );
        let already_real = "[[kv_namespaces]]\nbinding = \"KV\"\nid = \"already-real\"\n";
        assert!(kv_bindings_needing_ids(already_real).unwrap().is_empty());
    }

    #[test]
    fn kv_instance_title_is_the_worker_then_the_capability_in_kebab_case() {
        assert_eq!(
            kv_instance_title("shop-cart", "KV_SESSIONS"),
            "shop-cart-sessions"
        );
        assert_eq!(
            kv_instance_title("shop-cart", "KV_PRICE_CACHE"),
            "shop-cart-price-cache"
        );
    }

    #[test]
    fn kv_bindings_needing_ids_rejects_invalid_toml() {
        assert!(kv_bindings_needing_ids("not = valid = toml = at = all").is_err());
    }

    #[test]
    fn materialise_kv_namespace_ids_rejects_invalid_toml() {
        assert!(
            materialise_kv_namespace_ids("not = valid = toml = at = all", &ids(&[("KV", "x")]))
                .is_err()
        );
    }
}

#[cfg(test)]
mod compatibility_flags_tests {
    use super::*;

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// #1890: the default set alone, when the manifest adds nothing.
    #[test]
    fn the_default_is_global_fetch_strictly_public() {
        assert_eq!(
            DEFAULT_COMPATIBILITY_FLAGS,
            ["global_fetch_strictly_public"]
        );
        let (flags, duplicates) = compatibility_flags(&[]);
        assert_eq!(flags, ["global_fetch_strictly_public"]);
        assert!(duplicates.is_empty());
    }

    /// #1890: the manifest's flags follow the default, in manifest order;
    /// names are not checked.
    #[test]
    fn extra_flags_follow_the_default_in_order() {
        let (flags, duplicates) =
            compatibility_flags(&strings(&["nodejs_compat", "a_future_flag"]));
        assert_eq!(
            flags,
            [
                "global_fetch_strictly_public",
                "nodejs_compat",
                "a_future_flag"
            ]
        );
        assert!(duplicates.is_empty());
    }

    /// #1890: a repeat, of a default or of an earlier extra, is dropped and
    /// reported once per repeat.
    #[test]
    fn duplicates_are_dropped_and_reported() {
        let (flags, duplicates) = compatibility_flags(&strings(&[
            "nodejs_compat",
            "global_fetch_strictly_public",
            "nodejs_compat",
            "nodejs_compat",
        ]));
        assert_eq!(flags, ["global_fetch_strictly_public", "nodejs_compat"]);
        assert_eq!(
            duplicates,
            [
                "global_fetch_strictly_public",
                "nodejs_compat",
                "nodejs_compat"
            ]
        );
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;
    use bynk_check::worker_bindings::duplicate_bindings;

    fn names(bindings: &[WorkerBinding]) -> Vec<&str> {
        bindings.iter().map(|b| b.name.as_str()).collect()
    }

    /// Emission order: Service Bindings by target, the KV namespace, then
    /// the Durable Objects by class name, the fan-out among them.
    #[test]
    fn worker_bindings_lists_every_kind_in_emission_order() {
        let bindings = worker_bindings(
            &[],
            &["shop.payment".to_string(), "audit".to_string()],
            true,
            &[],
            true,
        );
        assert_eq!(
            names(&bindings),
            ["AUDIT", "SHOP_PAYMENT", "KV", "___EVENTS_FANOUT"]
        );
        assert!(duplicate_bindings(&bindings).is_empty());
    }

    /// #1950: the handler projection is drawn from the Worker's own list —
    /// each binding identical, name and source, a Service Binding and an
    /// agent's alike — and leaves out the KV namespace and the events fan-out.
    #[test]
    fn handler_bindings_are_a_subset_of_the_worker_bindings() {
        let worker = worker_bindings(
            &["OrderEntity".to_string()],
            &["shop.payment".to_string(), "audit".to_string()],
            true,
            &[],
            true,
        );
        let handler = handler_bindings(&["shop.payment".to_string()], &["OrderEntity".to_string()]);
        assert_eq!(names(&handler), ["SHOP_PAYMENT", "ORDER_ENTITY"]);
        for binding in &handler {
            assert!(worker.contains(binding), "{binding:?} not in {worker:?}");
        }
    }

    /// A consumed context `kv` and the KV namespace are both `KV`: the
    /// later one is paired with the earlier.
    #[test]
    fn a_shared_name_pairs_the_later_binding_with_the_earlier() {
        let bindings = worker_bindings(&[], &["kv".to_string()], true, &[], false);
        let duplicates = duplicate_bindings(&bindings);
        assert_eq!(duplicates.len(), 1);
        let (earlier, later) = duplicates[0];
        assert_eq!(
            earlier.source,
            BindingSource::Service {
                context: "kv".to_string()
            }
        );
        assert_eq!(later.source, BindingSource::Kv);
    }
}
