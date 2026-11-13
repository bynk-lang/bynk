//! `wrangler.toml` generation per Worker (v0.8 §4.4 / §4.5).
//!
//! Each context becomes a Cloudflare Worker with its own wrangler config.
//! Service Bindings are declared for every consumed context. Durable
//! Object bindings + `exports` entries are declared for every agent.

use crate::emitter::toml_doc::{TomlBlock, TomlDocument, TomlEntry, TomlValue};
pub use bynk_check::worker_bindings::{BindingSource, WorkerBinding};
use bynk_project::worker_dir_name;

/// Compile-time pinned compatibility date. Cloudflare uses this to lock
/// Workers runtime behaviour. When to bump it, and the gate a bump must pass,
/// is the policy in `design/bynk-release-discipline.md` ("Part 3 — The Workers
/// compatibility date", #1677): reviewed at each release, and moved only after
/// the workerd smokes pass on the new date.
pub const COMPATIBILITY_DATE: &str = "2026-08-04";

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
pub const WRANGLER_MIN: &str = "4.120.1";

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
/// ([`crate::emit_model::WORKER_ASSETS_DIR`]), beside — never containing — its
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
pub fn assets_block() -> TomlBlock {
    TomlBlock::table(
        "assets",
        vec![
            TomlEntry::kv(
                "directory",
                TomlValue::str(format!("./{}", crate::emit_model::WORKER_ASSETS_DIR)),
            ),
            TomlEntry::kv("run_worker_first", TomlValue::Bool(false)),
            TomlEntry::kv("not_found_handling", TomlValue::str("none")),
            TomlEntry::kv("html_handling", TomlValue::str("none")),
        ],
    )
}

/// L7 (#2414): `[cache]` turning Workers Cache on for a Worker whose context
/// declares an `EdgeCache` instance. Cloudflare then answers a `GET` or `HEAD`
/// from its edge cache when it holds a fresh response, without running the
/// Worker, and stores what the Worker sends as RFC 9111 allows: which is why
/// such a Worker sends `no-store` on every response but a `@cache` route's
/// `2xx` (`workers_entry`). A Worker without an instance gets no stanza.
pub fn edge_cache_block() -> TomlBlock {
    TomlBlock::table(
        "cache",
        vec![TomlEntry::kv("enabled", TomlValue::Bool(true))],
    )
}

/// #2321: the explicit module rule for a Worker whose adapter closure reaches
/// an included WebAssembly module: `**/*.wasm` is compiled by Wrangler to a
/// `WebAssembly.Module` (the shim's `import module from "./x.wasm"`). Wrangler's
/// default rules already say this; stating it keeps the dependency visible and
/// stops it changing under us. `fallthrough = true` leaves the other default
/// rules in force.
pub fn wasm_rule_block() -> TomlBlock {
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
pub fn asset_headers() -> String {
    format!(
        "{}*\n  Cache-Control: public, max-age=31536000, immutable\n",
        bynk_check::assets::SERVED_PREFIX
    )
}

/// State-migrations S3 (#2248): one context's agent class renames
/// (`(old, new)`) and removals, as `exports` tombstones.
#[derive(Debug, Default)]
pub struct ClassTombstones {
    pub renamed: Vec<(String, String)>,
    pub deleted: Vec<String>,
}

impl ClassTombstones {
    /// From the context's declarations: each agent's `replaces`, and each
    /// `removed agent`, skipping an old name still declared live (the checker
    /// already refuses that).
    pub fn of(table: &bynk_check::symbols::UnitTable) -> Self {
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

/// #3328: wrangler's **top-level** `addresses` (it applies to every
/// environment, so it is never copied into an `[env.*]` table). A Worker with no
/// email service has none, and wrangler then manages no routing rule for it.
fn addresses_entry(addresses: &[String]) -> Option<TomlEntry> {
    (!addresses.is_empty()).then(|| {
        TomlEntry::kv(
            "addresses",
            TomlValue::Array(addresses.iter().map(TomlValue::str).collect()),
        )
    })
}

/// P6.x cutover slice 2 (#1191, narrowed from #1187): `crons`/`queues` arrive
/// pre-collected, sorted and deduped by the caller (`project.rs`'s own
/// `emit_wrangler_toml` call site) rather than being walked here off
/// `table.services`. Both used to match a handler's cron-schedule kind and a
/// service's queue-binding protocol directly, straight off the syntax tree —
/// this file's entire raw-syntax footprint, per #1191's own grounding.
/// Relocating just these two matches removed `wrangler.rs` from the
/// `ast_importers` probe outright — no `bynk_model` equivalent exists to route
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
#[allow(clippy::too_many_arguments)]
pub fn emit_wrangler_toml(
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
    addresses: &[String], // #3328: the `from email` addresses
    // #1890: the merged flag list, [`compatibility_flags`]' first result.
    // The same for every Worker in the project, so the caller merges (and
    // warns about duplicates) once, not per Worker.
    flags: &[String],
    // `[workers] workers_dev`, the same for every Worker; `None` omits the key.
    workers_dev: Option<bool>,
    // State-migrations S3 (#2248): the context's declared class renames and
    // removals, each emitted as an `exports` tombstone.
    tombstones: &ClassTombstones,
    // P4b (#2608): each `D1` database's migrations; one with any gets a
    // `migrations_dir`.
    d1_migrations: &bynk_check::d1_migrations::D1Migrations,
) -> TomlDocument {
    let name = worker_dir_name(context);
    let mut root = vec![
        TomlEntry::kv("name", TomlValue::str(name)),
        TomlEntry::kv("main", TomlValue::str("index.ts")),
        TomlEntry::kv("compatibility_date", TomlValue::str(COMPATIBILITY_DATE)),
        TomlEntry::kv(
            "compatibility_flags",
            TomlValue::Array(flags.iter().map(TomlValue::str).collect()),
        ),
    ];
    root.extend(addresses_entry(addresses));
    if let Some(workers_dev) = workers_dev {
        root.push(TomlEntry::kv("workers_dev", TomlValue::Bool(workers_dev)));
    }
    let mut doc = TomlDocument::new("Generated by bynk — do not edit by hand.", root);

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
            // P3 (#2342): the limiter's own placeholder `namespace_id`, which
            // `bynk deploy` allocates in the ledger and materialises; the
            // limit and the window are configuration, not identity.
            BindingSource::RateLimitInstance { limit, period, .. } => TomlBlock::array_table(
                "ratelimits",
                vec![
                    TomlEntry::kv("name", name),
                    TomlEntry::with_comment(
                        "namespace_id",
                        TomlValue::str(ratelimit_namespace_placeholder(&binding.name)),
                        "set at deploy time",
                    ),
                    TomlEntry::kv(
                        "simple",
                        TomlValue::InlineTable(vec![
                            ("limit", TomlValue::Int(*limit)),
                            ("period", TomlValue::Int(*period)),
                        ]),
                    ),
                ],
            ),
            // P2 (#2578): the queue as the declaration names it; `bynk
            // deploy` environment-qualifies it, as it does a consumer's.
            BindingSource::QueueProducer { queue, .. } => TomlBlock::array_table(
                "queues.producers",
                vec![
                    TomlEntry::kv("binding", name),
                    TomlEntry::kv("queue", TomlValue::str(queue.clone())),
                ],
            ),
            // P3b (#2582): the dataset's derived name, which `bynk deploy`
            // environment-qualifies; the platform creates it on first write.
            BindingSource::AnalyticsDataset { .. } => TomlBlock::array_table(
                "analytics_engine_datasets",
                vec![
                    TomlEntry::kv("binding", name),
                    TomlEntry::kv(
                        "dataset",
                        TomlValue::str(analytics_dataset_name(
                            &worker_dir_name(context),
                            &binding.name,
                        )),
                    ),
                ],
            ),
            // P4a (#2602): the database's derived name, which `bynk deploy`
            // environment-qualifies, and an id placeholder it fills from the
            // ledger. Miniflare takes the placeholder as a local id, so
            // `bynk dev` runs with it.
            //
            // P4b (#2608): a database with migrations names their copy beside
            // this file, which `wrangler d1 migrations apply` reads.
            BindingSource::D1Database { .. } => d1_block(context, binding, name, d1_migrations),
            BindingSource::StoredSecret { store, secret, .. } => {
                stored_secret_block(name, store, secret)
            }
            BindingSource::Postgres { .. } | BindingSource::MySql { .. } => {
                hyperdrive_block(name, &binding.name)
            }
            BindingSource::Email { .. } => email_block(name, &binding.source),
            // #3479: the index's derived name, which `bynk deploy`
            // environment-qualifies and creates (never recreates). The binding
            // names the index; its dimensions, metric and metadata indexes are
            // the deploy's, not this stanza's.
            BindingSource::VectorizeIndex { .. } => vectorize_block(context, binding, name),
            // #3480: the account's instance, named as the declaration writes it
            // (in every environment). `bynk deploy` checks it exists and never
            // creates it.
            BindingSource::AiSearchInstance { instance, .. } => ai_search_block(name, instance),
            // #3300: a one-per-Worker capability's plain table, holding only
            // its fixed binding name (`[version_metadata]`).
            BindingSource::Singleton { capability } => {
                let table = bynk_check::firstparty::singleton(
                    bynk_check::firstparty::CLOUDFLARE_UNIT,
                    capability,
                )
                .expect("a `Singleton` binding names a registered singleton")
                .wrangler_table;
                TomlBlock::table(table, vec![TomlEntry::kv("binding", name)])
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
    // D1's principle). Wrangler reads `exports` from 4.107.0, so
    // [`WRANGLER_MIN`] is past it, and `wrangler dev` takes each class's backend from
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

/// P4a (#2602), P4b (#2608): a `D1` database's `[[d1_databases]]` entry: its
/// derived name, the placeholder id `bynk deploy` fills, and the copy of its
/// migrations beside the file when it has any (a project's own, or the
/// compiler's for the implicit idempotency database, W30 #2840).
fn d1_block(
    context: &str,
    binding: &WorkerBinding,
    name: TomlValue,
    d1_migrations: &bynk_check::d1_migrations::D1Migrations,
) -> TomlBlock {
    let database = d1_database_name(&worker_dir_name(context), &binding.name);
    let mut entries = vec![
        TomlEntry::kv("binding", name),
        TomlEntry::kv("database_name", TomlValue::str(database.clone())),
        TomlEntry::with_comment(
            "database_id",
            TomlValue::str(d1_database_id_placeholder(&binding.name)),
            "set at deploy time",
        ),
    ];
    if !bynk_check::d1_migrations::migrations_of(d1_migrations, &database, binding).is_empty() {
        entries.push(TomlEntry::kv(
            "migrations_dir",
            TomlValue::str(format!("{D1_MIGRATIONS_DIR}/{database}")),
        ));
    }
    TomlBlock::array_table("d1_databases", entries)
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
pub fn worker_bindings(
    // The context's agents (`UnitTable::agents`' keys), in any order.
    agents: &[String],
    consumes: &[String],
    needs_kv: bool,
    // The context's derived instances, each its binding with a
    // `KvInstance` (S4a, #1966) or `ForeignService` (interop I5, #1972)
    // source, in capability order.
    instances: &[WorkerBinding],
    // #3300: the one-per-Worker capabilities the closure reaches, each its
    // `Singleton` binding, in capability order.
    singletons: &[WorkerBinding],
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
    // P3 (#2342): the rate limiters, after the buckets.
    bindings.extend(
        instances
            .iter()
            .filter(|b| matches!(b.source, BindingSource::RateLimitInstance { .. }))
            .cloned(),
    );
    // P2 (#2578): the queue producers, after the rate limiters.
    bindings.extend(
        instances
            .iter()
            .filter(|b| matches!(b.source, BindingSource::QueueProducer { .. }))
            .cloned(),
    );
    // P3b (#2582): the Analytics Engine datasets, after the queue producers.
    bindings.extend(
        instances
            .iter()
            .filter(|b| matches!(b.source, BindingSource::AnalyticsDataset { .. }))
            .cloned(),
    );
    // P4a (#2602): the D1 databases, after the datasets.
    bindings.extend(
        instances
            .iter()
            .filter(|b| matches!(b.source, BindingSource::D1Database { .. }))
            .cloned(),
    );
    // #3301: the Secrets Store secrets, after the databases.
    bindings.extend(
        instances
            .iter()
            .filter(|b| matches!(b.source, BindingSource::StoredSecret { .. }))
            .cloned(),
    );
    // Hyperdrive H1 (#3318): the Postgres bindings, after the Secrets Store
    // secrets.
    bindings.extend(
        instances
            .iter()
            .filter(|b| {
                matches!(
                    b.source,
                    BindingSource::Postgres { .. } | BindingSource::MySql { .. }
                )
            })
            .cloned(),
    );
    // #3302: the outbound email bindings, after the Secrets Store secrets.
    bindings.extend(
        instances
            .iter()
            .filter(|b| matches!(b.source, BindingSource::Email { .. }))
            .cloned(),
    );
    // #3479: the Vectorize indexes, after the email bindings.
    bindings.extend(
        instances
            .iter()
            .filter(|b| matches!(b.source, BindingSource::VectorizeIndex { .. }))
            .cloned(),
    );
    // #3480: the AI Search instances, after the Vectorize indexes.
    bindings.extend(
        instances
            .iter()
            .filter(|b| matches!(b.source, BindingSource::AiSearchInstance { .. }))
            .cloned(),
    );
    // #3300: the one-per-Worker bindings, after the databases.
    bindings.extend(singletons.iter().cloned());
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

/// #3479: the `[[vectorize]]` entry of a `Vectorize` instance.
fn vectorize_block(context: &str, binding: &WorkerBinding, name: TomlValue) -> TomlBlock {
    TomlBlock::array_table(
        "vectorize",
        vec![
            TomlEntry::kv("binding", name),
            TomlEntry::kv(
                "index_name",
                TomlValue::str(vectorize_index_name(
                    &worker_dir_name(context),
                    &binding.name,
                )),
            ),
        ],
    )
}

/// #3480: the `[[ai_search]]` entry of an `AiSearch` instance.
fn ai_search_block(binding: TomlValue, instance: &str) -> TomlBlock {
    TomlBlock::array_table(
        "ai_search",
        vec![
            TomlEntry::kv("binding", binding),
            TomlEntry::kv("instance_name", TomlValue::str(instance.to_string())),
        ],
    )
}

/// #3301: the `[[secrets_store_secrets]]` entry of a `StoredSecret`: the secret
/// in the store the declaration names. The store is written as its name, which
/// doubles as its local id under `bynk dev`; `bynk deploy` links it (never
/// creating it) and replaces the name with the id it resolves.
fn stored_secret_block(binding: TomlValue, store: &str, secret: &str) -> TomlBlock {
    TomlBlock::array_table(
        "secrets_store_secrets",
        vec![
            TomlEntry::kv("binding", binding),
            TomlEntry::with_comment(
                "store_id",
                TomlValue::str(store.to_string()),
                "the store's name; its id is set at deploy time",
            ),
            TomlEntry::kv("secret_name", TomlValue::str(secret.to_string())),
        ],
    )
}

/// Hyperdrive H1 (#3318): the `[[hyperdrive]]` entry of a `Postgres` instance.
/// The configuration is made by `bynk deploy` (or adopted: `bynk.toml`'s
/// `[deploy.hyperdrive]`), so its `id` is a placeholder it replaces, as D1's
/// is. `bynk dev` needs `[dev.hyperdrive]` in `bynk.toml`, or
/// `CLOUDFLARE_HYPERDRIVE_LOCAL_CONNECTION_STRING_<BINDING>` in the
/// environment.
fn hyperdrive_block(name: TomlValue, binding: &str) -> TomlBlock {
    TomlBlock::array_table(
        "hyperdrive",
        vec![
            TomlEntry::kv("binding", name),
            TomlEntry::with_comment(
                "id",
                TomlValue::str(hyperdrive_id_placeholder(binding)),
                "set at deploy time",
            ),
        ],
    )
}

/// #3302: the `[[send_email]]` entry of an `Email` instance. Its key is `name`,
/// not `binding` (as `[[ratelimits]]`'s is), and each restriction is written
/// only when the declaration carries it: `destination_address` pins every
/// message to one recipient, and the two lists limit recipients and senders.
fn email_block(name: TomlValue, source: &BindingSource) -> TomlBlock {
    let BindingSource::Email {
        to,
        allowed_recipients,
        allowed_senders,
        ..
    } = source
    else {
        unreachable!("`email_block` takes an `Email` binding")
    };
    let to = to.as_deref();
    let list = |items: &[String]| TomlValue::Array(items.iter().map(TomlValue::str).collect());
    let mut entries = vec![TomlEntry::kv("name", name)];
    if let Some(to) = to {
        entries.push(TomlEntry::kv("destination_address", TomlValue::str(to)));
    }
    if !allowed_recipients.is_empty() {
        entries.push(TomlEntry::kv(
            "allowed_destination_addresses",
            list(allowed_recipients),
        ));
    }
    if !allowed_senders.is_empty() {
        entries.push(TomlEntry::kv(
            "allowed_sender_addresses",
            list(allowed_senders),
        ));
    }
    TomlBlock::array_table("send_email", entries)
}

/// Whether `source` is the one-per-Worker binding of `capability` (#3300,
/// #3478): the kind test for a singleton, whose runtime type and test double
/// differ per capability.
pub fn singleton_is(source: &BindingSource, capability: &str) -> bool {
    matches!(source, BindingSource::Singleton { capability: c } if c == capability)
}

/// A test of a binding's kind (`|s| matches!(s, BindingSource::Kv)`), as
/// the tables pairing each platform-resource kind with what it needs use.
pub type BindingKind = fn(&BindingSource) -> bool;

/// The TypeScript type a binding has on the Worker's `env`, for both its
/// `Env` interface and a handler's `deps.env`.
pub(crate) fn binding_ts_type(source: &BindingSource) -> &'static str {
    match source {
        BindingSource::Service { .. } | BindingSource::ForeignService { .. } => "__ServiceBinding",
        BindingSource::Kv | BindingSource::KvInstance { .. } => "__KVNamespace",
        BindingSource::R2Instance { .. } => "__R2Bucket",
        BindingSource::RateLimitInstance { .. } => "__RateLimiter",
        BindingSource::QueueProducer { .. } => "__QueueProducer",
        BindingSource::AnalyticsDataset { .. } => "__AnalyticsDataset",
        BindingSource::D1Database { .. } => "__D1Database",
        BindingSource::StoredSecret { .. } => "__SecretsStoreSecret",
        BindingSource::Email { .. } => "__SendEmail",
        BindingSource::VectorizeIndex { .. } => "__VectorizeIndex",
        BindingSource::AiSearchInstance { .. } => "__AiSearchInstance",
        BindingSource::Postgres { .. } | BindingSource::MySql { .. } => "__Hyperdrive",
        BindingSource::Singleton { capability } => match capability.as_str() {
            "Version" => "__WorkerVersionMetadata",
            "Ai" => "__WorkerAiBinding",
            other => unreachable!("singleton `{other}` has no `env` type"),
        },
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
pub fn service_binding(target: &str) -> WorkerBinding {
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

/// P3 (#2342): the placeholder `namespace_id` of rate limiter `binding`,
/// which `bynk deploy` replaces with the id the ledger allocates. Miniflare
/// takes it as it is, so `bynk dev` runs with it.
pub fn ratelimit_namespace_placeholder(binding: &str) -> String {
    format!("<{binding}_NAMESPACE>")
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

/// #3300: the binding of `bynk.cloudflare`'s one-per-Worker capability
/// `capability` (`Version` → `CF_VERSION_METADATA`), by the registry's fixed
/// name. Its one TypeScript read is the default of its provider's `binding`
/// parameter in `cloudflare.binding.ts`.
pub fn singleton_binding(capability: &str) -> WorkerBinding {
    let singleton =
        bynk_check::firstparty::singleton(bynk_check::firstparty::CLOUDFLARE_UNIT, capability)
            .unwrap_or_else(|| panic!("`{capability}` is not a one-per-Worker capability"));
    WorkerBinding {
        name: singleton.binding.to_string(),
        source: BindingSource::Singleton {
            capability: capability.to_string(),
        },
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
// using it here silently dropped the "Generated by bynk" banner and every
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

/// Cloudflare's longest index name, in bytes.
const VECTORIZE_INDEX_NAME_MAX: usize = 64;

/// Bytes kept free for `bynk deploy`'s `-<environment>` suffix (a hyphen and up
/// to 15 bytes of environment name), so the qualified name stays within the limit.
const VECTORIZE_ENV_SUFFIX_ROOM: usize = 16;

/// #3479: the account-facing name of a `Vectorize` instance's index,
/// `<worker>-<capability>` in kebab case: `shop-cart` and `VECTORIZE_DOCS` give
/// `shop-cart-docs`. Cloudflare takes a kebab-cased name of at most 64 bytes,
/// so a name longer than that, less room for the environment suffix, keeps its head and ends in a hash of the whole name (the
/// same inputs always give the same name). `bynk deploy` environment-qualifies
/// it (`shop-cart-docs-staging`).
pub fn vectorize_index_name(worker: &str, binding: &str) -> String {
    let prefix = format!("{}_", bynk_check::firstparty::VECTORIZE_BINDING_PREFIX);
    let stem = binding.strip_prefix(&prefix).unwrap_or(binding);
    let full = format!("{worker}-{}", stem.to_ascii_lowercase().replace('_', "-"));
    let budget = VECTORIZE_INDEX_NAME_MAX - VECTORIZE_ENV_SUFFIX_ROOM;
    if full.len() <= budget {
        return full;
    }
    // FNV-1a: stable across builds and platforms, unlike `DefaultHasher`.
    let hash = full.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    let suffix = format!("-{:08x}", hash as u32);
    let head = &full[..budget - suffix.len()];
    format!("{}{suffix}", head.trim_end_matches('-'))
}

/// P3b (#2582): the account-facing name of an `Analytics` instance's dataset,
/// `<worker>_<capability>` in snake case: `shop-cart` and `ANALYTICS_SIGNUPS`
/// give `shop_cart_signups`. Underscores, not R2's hyphens, because the dataset
/// is the table name in the SQL API, where a hyphen would need quoting in
/// every query. `bynk deploy` environment-qualifies it
/// (`shop_cart_signups_staging`).
pub fn analytics_dataset_name(worker: &str, binding: &str) -> String {
    let prefix = format!("{}_", bynk_check::firstparty::ANALYTICS_BINDING_PREFIX);
    let stem = binding.strip_prefix(&prefix).unwrap_or(binding);
    format!("{worker}_{stem}")
        .to_ascii_lowercase()
        .replace('-', "_")
}

/// #3479: the file beside a Worker's `wrangler.toml` that states each
/// `[[vectorize]]` binding's index configuration. `wrangler.toml`'s stanza
/// names only the index; its dimensions, metric and metadata indexes are what
/// `bynk deploy` creates it with and checks it against, and Cloudflare cannot
/// change them afterwards. Written only for a Worker with a `Vectorize`
/// instance, so any other project's output is unchanged.
pub const VECTORIZE_MANIFEST: &str = "bynk-vectorize.json";

/// The manifest's version. A reader refuses one it does not know.
pub const VECTORIZE_MANIFEST_VERSION: u32 = 1;

/// The `bynk-vectorize.json` of a Worker with `bindings`, or `None` when it
/// has no `Vectorize` instance.
pub fn emit_vectorize_manifest(bindings: &[WorkerBinding]) -> Option<String> {
    let entries: Vec<String> = bindings
        .iter()
        .filter_map(|b| match &b.source {
            BindingSource::VectorizeIndex {
                dimensions,
                metric,
                indexed,
                ..
            } => Some(format!(
                "    {{ \"binding\": {}, \"dimensions\": {dimensions}, \"metric\": {}, \"indexed\": [{}] }}",
                bynk_project::json_string(&b.name),
                bynk_project::json_string(metric),
                indexed
                    .iter()
                    .map(|i| bynk_project::json_string(i))
                    .collect::<Vec<_>>()
                    .join(", "),
            )),
            _ => None,
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    Some(format!(
        "{{\n  \"version\": {VECTORIZE_MANIFEST_VERSION},\n  \"vectorize\": [\n{}\n  ]\n}}\n",
        entries.join(",\n")
    ))
}

/// A generated `wrangler.toml`'s `[[vectorize]]` entries as `(binding,
/// index_name)`, in file order; an entry missing either is skipped.
pub fn vectorize_index_entries(text: &str) -> Result<Vec<(String, String)>, String> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    Ok(doc
        .get("vectorize")
        .and_then(|i| i.as_array_of_tables())
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some((
                t.get("binding")?.as_str()?.to_string(),
                t.get("index_name")?.as_str()?.to_string(),
            ))
        })
        .collect())
}

/// #3479: point `[[vectorize]]` entries at the indexes `bynk.toml` adopts,
/// structurally, by binding. An adopted index keeps the name it has on the
/// account (never the derived, environment-qualified one), so it is written
/// into the top-level entries for the default environment and into
/// `[env.<environment>]`'s for a named one. Unchanged text when nothing
/// differs, as [`materialise_r2_bucket_names`].
pub fn materialise_vectorize_index_names(
    text: &str,
    environment: &str,
    names: &std::collections::BTreeMap<String, String>,
) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    let mut changed = false;
    let mut found = std::collections::BTreeSet::new();
    let item = if environment == "default" {
        doc.get_mut("vectorize")
    } else {
        doc.get_mut("env")
            .and_then(|e| e.get_mut(environment))
            .and_then(|e| e.get_mut("vectorize"))
    };
    let mut set = |entry: &mut dyn toml_edit::TableLike| {
        let Some(binding) = entry.get("binding").and_then(|b| b.as_str()) else {
            return;
        };
        let Some(name) = names.get(binding).cloned() else {
            return;
        };
        found.insert(binding.to_string());
        if let Some(index) = entry.get_mut("index_name")
            && index.as_str() != Some(name.as_str())
        {
            set_string_preserving_decor(index, &name);
            changed = true;
        }
    };
    if let Some(item) = item {
        if let Some(tables) = item.as_array_of_tables_mut() {
            for table in tables.iter_mut() {
                set(table);
            }
        } else if let Some(array) = item.as_array_mut() {
            for value in array.iter_mut() {
                if let Some(table) = value.as_inline_table_mut() {
                    set(table);
                }
            }
        }
    }
    // An adopted binding with no stanza to name would bind the derived name.
    if let Some(missing) = names.keys().find(|b| !found.contains(*b)) {
        return Err(format!(
            "no `[[vectorize]]` entry for `{missing}` in {}",
            if environment == "default" {
                "the configuration".to_string()
            } else {
                format!("`[env.{environment}]`")
            }
        ));
    }
    Ok(if changed {
        doc.to_string()
    } else {
        text.to_string()
    })
}

/// P4b (#2608): the directory beside a Worker's `wrangler.toml` holding its
/// `D1` databases' migrations, one subdirectory per database, as the
/// project's own `migrations/` folder does.
pub const D1_MIGRATIONS_DIR: &str = "migrations";

/// Hyperdrive H5 (#3322): the file beside a Worker's `wrangler.toml` that says
/// which of its `[[hyperdrive]]` bindings are MySQL, and each one's `@tls`
/// mode. `wrangler.toml`'s stanza names neither (a Hyperdrive config's database
/// kind and TLS mode are the config's, not the binding's), and `bynk deploy`
/// needs both to make the config. It is written only for a Worker with a
/// `MySql` instance, so a `Postgres`-only project's output is unchanged, and it
/// holds names and modes, never a connection string.
pub const HYPERDRIVE_MANIFEST: &str = "bynk-hyperdrive.json";

/// The manifest's version. A reader refuses one it does not know.
pub const HYPERDRIVE_MANIFEST_VERSION: u32 = 1;

/// The `bynk-hyperdrive.json` of a Worker with `bindings`, or `None` when it
/// has no `MySql` instance. The `tls` field is the `@tls` mode as the source
/// wrote it (`required` when it wrote none).
pub fn emit_hyperdrive_manifest(bindings: &[WorkerBinding]) -> Option<String> {
    let entries: Vec<String> = bindings
        .iter()
        .filter_map(|b| match &b.source {
            BindingSource::MySql { tls, .. } => Some(format!(
                "    {{ \"binding\": {}, \"kind\": \"mysql\", \"tls\": {} }}",
                bynk_project::json_string(&b.name),
                bynk_project::json_string(tls.as_deref().unwrap_or("required")),
            )),
            _ => None,
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    Some(format!(
        "{{\n  \"version\": {HYPERDRIVE_MANIFEST_VERSION},\n  \"hyperdrive\": [\n{}\n  ]\n}}\n",
        entries.join(",\n")
    ))
}

/// P4a (#2602): the account-facing name of a `D1` instance's database. It
/// moved to the checker with P4b (#2608), which names each instance's
/// migrations directory by it.
pub use bynk_check::d1_migrations::d1_database_name;

/// P4a (#2602): the placeholder `database_id` of D1 binding `binding`, which
/// `bynk deploy` replaces with the id it records in the ledger.
pub fn d1_database_id_placeholder(binding: &str) -> String {
    format!("<{binding}_ID>")
}

/// Hyperdrive H1 (#3318): the placeholder `id` of Hyperdrive binding `binding`,
/// which `bynk deploy` replaces with the id of its Hyperdrive config.
pub fn hyperdrive_id_placeholder(binding: &str) -> String {
    format!("<{binding}_ID>")
}

/// P4a (#2602): a generated `wrangler.toml`'s `[[d1_databases]]` entries as
/// `(binding, database_name)`, in file order; an entry missing either is
/// skipped.
pub fn d1_database_entries(text: &str) -> Result<Vec<(String, String)>, String> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    Ok(doc
        .get("d1_databases")
        .and_then(|i| i.as_array_of_tables())
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some((
                t.get("binding")?.as_str()?.to_string(),
                t.get("database_name")?.as_str()?.to_string(),
            ))
        })
        .collect())
}

/// P4a (#2602): point `[[d1_databases]]` entries at their deployed databases,
/// structurally, by binding: each entry whose binding `databases` lists gets
/// that `(database_name, database_id)`. Unchanged text when nothing differs,
/// as [`materialise_r2_bucket_names`].
pub fn materialise_d1_databases(
    text: &str,
    databases: &std::collections::BTreeMap<String, (String, String)>,
) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    let mut changed = false;
    if let Some(entries) = doc
        .get_mut("d1_databases")
        .and_then(|i| i.as_array_of_tables_mut())
    {
        for entry in entries.iter_mut() {
            let Some((name, id)) = entry
                .get("binding")
                .and_then(|b| b.as_str())
                .and_then(|b| databases.get(b))
                .cloned()
            else {
                continue;
            };
            for (key, value) in [("database_name", name), ("database_id", id)] {
                if let Some(item) = entry.get_mut(key)
                    && item.as_str() != Some(value.as_str())
                {
                    set_string_preserving_decor(item, &value);
                    changed = true;
                }
            }
        }
    }
    Ok(if changed {
        doc.to_string()
    } else {
        text.to_string()
    })
}

/// Hyperdrive H2 (#3319): a generated `wrangler.toml`'s `[[hyperdrive]]`
/// entries as `(binding, id)`, in file order; an entry missing either is
/// skipped. In a freshly generated file `id` is the placeholder.
pub fn hyperdrive_entries(text: &str) -> Result<Vec<(String, String)>, String> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    Ok(doc
        .get("hyperdrive")
        .and_then(|i| i.as_array_of_tables())
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some((
                t.get("binding")?.as_str()?.to_string(),
                t.get("id")?.as_str()?.to_string(),
            ))
        })
        .collect())
}

/// Hyperdrive H2 (#3319): point `[[hyperdrive]]` entries at their configs,
/// structurally, by binding: each entry whose binding `ids` lists gets that
/// id. Unchanged text when nothing differs, as [`materialise_d1_databases`].
/// Only an id is written: the connection string is never an input here.
pub fn materialise_hyperdrive_ids(
    text: &str,
    ids: &std::collections::BTreeMap<String, String>,
) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    let mut changed = false;
    if let Some(entries) = doc
        .get_mut("hyperdrive")
        .and_then(|i| i.as_array_of_tables_mut())
    {
        for entry in entries.iter_mut() {
            let Some(id) = entry
                .get("binding")
                .and_then(|b| b.as_str())
                .and_then(|b| ids.get(b))
                .cloned()
            else {
                continue;
            };
            if let Some(item) = entry.get_mut("id")
                && item.as_str() != Some(id.as_str())
            {
                set_string_preserving_decor(item, &id);
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

/// #3301: a generated `wrangler.toml`'s `[[secrets_store_secrets]]` entries as
/// `(binding, store_id, secret_name)`, in file order; an entry missing any is
/// skipped. In a freshly generated file `store_id` is the store's *name*.
pub fn stored_secret_entries(text: &str) -> Result<Vec<(String, String, String)>, String> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    Ok(doc
        .get("secrets_store_secrets")
        .and_then(|i| i.as_array_of_tables())
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some((
                t.get("binding")?.as_str()?.to_string(),
                t.get("store_id")?.as_str()?.to_string(),
                t.get("secret_name")?.as_str()?.to_string(),
            ))
        })
        .collect())
}

/// #3301: point `[[secrets_store_secrets]]` entries at their stores' ids,
/// structurally, by binding. Unchanged text when nothing differs.
pub fn materialise_stored_secret_stores(
    text: &str,
    stores: &std::collections::BTreeMap<String, String>,
) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    let mut changed = false;
    if let Some(entries) = doc
        .get_mut("secrets_store_secrets")
        .and_then(|i| i.as_array_of_tables_mut())
    {
        for entry in entries.iter_mut() {
            let Some(id) = entry
                .get("binding")
                .and_then(|b| b.as_str())
                .and_then(|b| stores.get(b))
            else {
                continue;
            };
            if let Some(item) = entry.get_mut("store_id")
                && item.as_str() != Some(id.as_str())
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

/// P3 (#2342): the rate limiters whose `namespace_id` is still its
/// placeholder, by `name`, in file order.
pub fn ratelimits_needing_ids(text: &str) -> Result<Vec<String>, String> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    let mut out = Vec::new();
    if let Some(entries) = doc.get("ratelimits").and_then(|i| i.as_array_of_tables()) {
        for entry in entries.iter() {
            let Some(name) = entry.get("name").and_then(|b| b.as_str()) else {
                continue;
            };
            if entry.get("namespace_id").and_then(|i| i.as_str())
                == Some(ratelimit_namespace_placeholder(name).as_str())
            {
                out.push(name.to_string());
            }
        }
    }
    Ok(out)
}

/// P3 (#2342): write each rate limiter's allocated `namespace_id` over its
/// placeholder, structurally, by `name`, leaving its `simple = { … }` and
/// comment as they are. Unchanged text when nothing matches, as
/// [`materialise_kv_namespace_ids`].
pub fn materialise_ratelimit_namespace_ids(
    text: &str,
    ids: &std::collections::BTreeMap<String, String>,
) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    let mut changed = false;
    if let Some(entries) = doc
        .get_mut("ratelimits")
        .and_then(|i| i.as_array_of_tables_mut())
    {
        for entry in entries.iter_mut() {
            let Some(name) = entry.get("name").and_then(|b| b.as_str()) else {
                continue;
            };
            let Some(id) = ids.get(name) else {
                continue;
            };
            let placeholder = ratelimit_namespace_placeholder(name);
            if let Some(item) = entry.get_mut("namespace_id")
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

/// #3480: the config `bynk dev` serves, where each `[[ai_search]]` entry also
/// carries `remote = true`: AI Search has no local simulation, so wrangler
/// proxies the binding to the deployed instance. Only ever applied to the
/// development copy; the config `bynk deploy` reads never carries it. Unchanged
/// text when there is no such entry.
pub fn enable_remote_ai_search(text: &str) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("invalid wrangler.toml: {e}"))?;
    let mut changed = false;
    if let Some(entries) = doc
        .get_mut("ai_search")
        .and_then(|i| i.as_array_of_tables_mut())
    {
        for entry in entries.iter_mut() {
            if entry.get("remote").and_then(|r| r.as_bool()) != Some(true) {
                entry.insert("remote", toml_edit::value(true));
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
mod ai_search_dev_tests {
    use super::*;

    /// The development copy proxies each AI Search binding to the account;
    /// text with no such binding is returned unchanged.
    #[test]
    fn the_development_copy_proxies_each_ai_search_binding() {
        let text = "name = \"shop\"\n\n[[ai_search]]\nbinding = \"AI_SEARCH_KB\"\ninstance_name = \"support-docs\"\n\n[[ai_search]]\nbinding = \"AI_SEARCH_B\"\ninstance_name = \"b\"\n";
        let out = enable_remote_ai_search(text).unwrap();
        assert_eq!(out.matches("remote = true").count(), 2, "{out}");
        assert!(out.contains("instance_name = \"support-docs\""));
        assert_eq!(enable_remote_ai_search(&out).unwrap(), out);
        let plain = "name = \"shop\"\n";
        assert_eq!(enable_remote_ai_search(plain).unwrap(), plain);
        assert!(enable_remote_ai_search("not = [valid").is_err());
    }
}

#[cfg(test)]
mod vectorize_name_tests {
    use super::*;

    #[test]
    fn an_adopted_binding_with_no_stanza_is_an_error_not_a_silent_derived_name() {
        let names = std::collections::BTreeMap::from([(
            "VECTORIZE_DOCS".to_string(),
            "docs-prod".to_string(),
        )]);
        let text = "name = \"w\"\n\n[env.production]\nname = \"w-production\"\n";
        let err = materialise_vectorize_index_names(text, "production", &names).unwrap_err();
        assert!(err.contains("VECTORIZE_DOCS"), "{err}");
    }

    #[test]
    fn short_name_is_kebab_and_unhashed() {
        assert_eq!(
            vectorize_index_name("shop-cart", "VECTORIZE_DOCS"),
            "shop-cart-docs"
        );
    }

    #[test]
    fn long_name_keeps_room_for_the_environment_suffix_and_is_stable() {
        let long = "w".repeat(70);
        let a = vectorize_index_name(&long, "VECTORIZE_DOCS");
        assert_eq!(a, vectorize_index_name(&long, "VECTORIZE_DOCS"));
        assert!(a.len() + VECTORIZE_ENV_SUFFIX_ROOM <= VECTORIZE_INDEX_NAME_MAX);
        // FNV-1a pin: the hashed form must not change between builds.
        assert!(a.ends_with("-46a118f1"), "{a}");
    }

    #[test]
    fn head_does_not_end_in_a_doubled_hyphen() {
        // The cut lands on a hyphen: it is trimmed, not doubled.
        let worker = format!("{}-{}", "a".repeat(30), "b".repeat(30));
        let name = vectorize_index_name(&worker, "VECTORIZE_DOCS");
        assert!(!name.contains("--"), "{name}");
    }
}

#[cfg(test)]
mod patch_tests {
    use super::*;

    /// A full, realistic `wrangler.toml` — every stanza `emit_wrangler_toml`
    /// can produce, in its real order, with its real banner and inline
    /// comment (mirrors `bynk-tests/tests/fixtures/positive/
    /// 372_kv_agent_queue_workers/expected/workers/ops-hub/wrangler.toml`
    /// byte-for-byte). Review of #1305, finding 2: every test below asserts
    /// **exact** patched text against this fixture, not just "the changed
    /// field has the right value after a re-parse" — a re-parse is blind by
    /// construction to whatever the printer discarded, which is exactly how
    /// finding 1 (the whole-document alphabetical re-sort, dropped banner)
    /// survived a green suite the first time around.
    const REPRESENTATIVE: &str = "\
# Generated by bynk — do not edit by hand.
name = \"ops-hub\"
main = \"index.ts\"
compatibility_date = \"2026-08-04\"
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
            &[singleton_binding("Version")],
            true,
        );
        assert_eq!(
            names(&bindings),
            [
                "AUDIT",
                "SHOP_PAYMENT",
                "KV",
                "CF_VERSION_METADATA",
                "___EVENTS_FANOUT"
            ]
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
        let bindings = worker_bindings(&[], &["kv".to_string()], true, &[], &[], false);
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

#[cfg(test)]
mod vectorize_manifest_tests {
    use super::*;

    fn index(name: &str, indexed: &[&str]) -> WorkerBinding {
        WorkerBinding {
            name: name.to_string(),
            source: BindingSource::VectorizeIndex {
                capability: "Docs".to_string(),
                dimensions: 768,
                metric: "cosine".to_string(),
                indexed: indexed.iter().map(|s| s.to_string()).collect(),
            },
        }
    }

    /// #3479: the manifest states each index's configuration, and a Worker with
    /// no `Vectorize` instance gets no file.
    #[test]
    fn the_manifest_states_each_indexs_configuration() {
        assert_eq!(emit_vectorize_manifest(&[]), None);
        let text = emit_vectorize_manifest(&[
            index("VECTORIZE_DOCS", &["category:string", "year:number"]),
            index("VECTORIZE_PLAIN", &[]),
        ])
        .expect("two indexes");
        assert!(text.contains(
            "{ \"binding\": \"VECTORIZE_DOCS\", \"dimensions\": 768, \"metric\": \"cosine\", \"indexed\": [\"category:string\", \"year:number\"] }"
        ));
        assert!(text.contains("\"binding\": \"VECTORIZE_PLAIN\", \"dimensions\": 768, \"metric\": \"cosine\", \"indexed\": []"));
    }

    /// The index entries are read back from a generated config by binding.
    #[test]
    fn index_entries_are_read_from_the_config() {
        let toml = "name = \"shop\"\n\n[[vectorize]]\nbinding = \"VECTORIZE_DOCS\"\nindex_name = \"shop-docs\"\n";
        assert_eq!(
            vectorize_index_entries(toml).unwrap(),
            [("VECTORIZE_DOCS".to_string(), "shop-docs".to_string())]
        );
    }

    /// #3479: an adopted index's name replaces the derived one, in the top-level
    /// entries for the default environment and in the environment's block for a
    /// named one; a binding that is not adopted is left alone.
    #[test]
    fn adopted_index_names_are_written_where_the_environment_reads_them() {
        let top = "name = \"shop\"\n\n[[vectorize]]\nbinding = \"VECTORIZE_DOCS\"\nindex_name = \"shop-docs\"\n\n[[vectorize]]\nbinding = \"VECTORIZE_OTHER\"\nindex_name = \"shop-other\"\n";
        let names = std::collections::BTreeMap::from([(
            "VECTORIZE_DOCS".to_string(),
            "docs-prod".to_string(),
        )]);
        let patched = materialise_vectorize_index_names(top, "default", &names).unwrap();
        assert!(patched.contains("index_name = \"docs-prod\""), "{patched}");
        assert!(patched.contains("index_name = \"shop-other\""), "{patched}");
        assert_eq!(
            materialise_vectorize_index_names(top, "default", &Default::default()).unwrap(),
            top
        );
        let named = "name = \"shop\"\n\n[[env.staging.vectorize]]\nbinding = \"VECTORIZE_DOCS\"\nindex_name = \"shop-docs-staging\"\n";
        let patched = materialise_vectorize_index_names(named, "staging", &names).unwrap();
        assert!(patched.contains("index_name = \"docs-prod\""), "{patched}");
    }

    /// A derived name longer than Cloudflare's 64 bytes keeps its head and ends
    /// in a stable hash; a short one is as derived.
    #[test]
    fn the_derived_name_respects_the_length_limit() {
        assert_eq!(
            vectorize_index_name("shop-cart", "VECTORIZE_DOCS"),
            "shop-cart-docs"
        );
        let long = vectorize_index_name(&"a".repeat(80), "VECTORIZE_DOCS");
        assert_eq!(
            long.len(),
            VECTORIZE_INDEX_NAME_MAX - VECTORIZE_ENV_SUFFIX_ROOM
        );
        assert_eq!(
            long,
            vectorize_index_name(&"a".repeat(80), "VECTORIZE_DOCS")
        );
        assert_ne!(
            long,
            vectorize_index_name(&"a".repeat(81), "VECTORIZE_DOCS")
        );
    }
}

#[cfg(test)]
mod hyperdrive_manifest_tests {
    use super::*;

    fn binding(name: &str, source: BindingSource) -> WorkerBinding {
        WorkerBinding {
            name: name.to_string(),
            source,
        }
    }

    /// Hyperdrive H5 (#3322): the manifest lists the `MySql` bindings with their
    /// mode (`required` when none was written), and a Worker with only a
    /// `Postgres` one gets no file, so its output is unchanged.
    #[test]
    fn the_manifest_lists_mysql_bindings_only() {
        let postgres = binding(
            "HYPERDRIVE_MAIN",
            BindingSource::Postgres {
                capability: "Main".to_string(),
            },
        );
        assert_eq!(
            emit_hyperdrive_manifest(std::slice::from_ref(&postgres)),
            None
        );
        let bindings = [
            postgres,
            binding(
                "HYPERDRIVE_LEGACY",
                BindingSource::MySql {
                    capability: "Legacy".to_string(),
                    tls: Some("verify-identity".to_string()),
                },
            ),
            binding(
                "HYPERDRIVE_OTHER",
                BindingSource::MySql {
                    capability: "Other".to_string(),
                    tls: None,
                },
            ),
        ];
        let text = emit_hyperdrive_manifest(&bindings).expect("a MySql binding");
        assert!(
            text.contains(
                r#""binding": "HYPERDRIVE_LEGACY", "kind": "mysql", "tls": "verify-identity""#
            ),
            "{text}"
        );
        assert!(
            text.contains(r#""binding": "HYPERDRIVE_OTHER", "kind": "mysql", "tls": "required""#),
            "{text}"
        );
        assert!(!text.contains("HYPERDRIVE_MAIN"), "{text}");
        // Names and modes only: never a connection string or a URL.
        assert!(!text.contains("://") && !text.contains('@'), "{text}");
    }
}
