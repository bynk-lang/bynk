use super::agent_deps::plan_agent_given_deps;
use super::diagnostics::ErrorSink;
use super::diagnostics::Mode;
use super::options::ProgressEvent;
use super::options::ProgressSink;
use super::schema_registry;
use super::unit::check_unit_files;
use super::unit::project_mentions_encode_plain;
use super::worker_bindings;
use bynk_check::assets::phase_asset_routes;
use bynk_check::check_pipeline::{self};
use bynk_check::checker::{ExprId, Types};
use bynk_check::expr_types::ExprTypeSink;
use bynk_check::firstparty::Platform;
use bynk_check::hints::HintSink;
use bynk_check::index::RefSink;
use bynk_check::locals::LocalsSink;
use bynk_check::project_model::BuildTarget;
use bynk_check::project_model::{self, AdapterBinding};
use bynk_check::requirements::RequirementSink;
use bynk_check::symbols::UnitTable;
use bynk_check::uses_view;
use bynk_emit::emit_model::ImportExt;
use bynk_emit::emit_model::RemoteTests;
use bynk_emit::emit_model::SchemaVersions;
use bynk_emit::emit_model::StagedFile;
use bynk_emit::emit_model::harness_clock_contexts;
use bynk_emit::emitter;
use bynk_emit::emitter::commons_barrel::emit_commons_barrel;
use bynk_emit::testgen::{
    IntegrationD1, RunnableTest, TestAssetFiles, process_integration_tests, process_tests,
};
use bynk_model::EventSubscriberShape;
use bynk_project::SchemaLock;
use bynk_project::UnitKind;
use bynk_project::assets;
use bynk_project::discovery::ParsedFile;
use bynk_project::paths::commons_dir_for;
use bynk_project::worker_layout::WorkerLayout;
use bynk_syntax::error::CompileError;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// v0.119 (ADR 0155): the agents a `for all run: History[Agent]` property drives,
/// scanned across every test suite in the project. `emit_agent` gates the
/// exported `__bynkDriveHistory_<Agent>` driver on membership, so a non-targeted
/// agent's emission is unchanged.
fn collect_history_target_agents(parsed: &[ParsedFile]) -> HashSet<String> {
    parsed
        .iter()
        .flat_map(|pf| pf.history_target_agent_names())
        .map(String::from)
        .collect()
}

/// The outcome of the shared check pipeline (regions 1+2's shared work),
/// before whichever caller applies its own divergent exit — `compile_project`/
/// `compile_in_memory` project it into a `Result<ProjectOutput, ProjectFailure>`
/// via `finish_build`; `check_project`/`analyse_in_memory_with_types` read
/// straight off it instead. #1541 deleted the fifth of these five callers
/// (`analyse_project_with`, which alone read `hints`/`requirements`/`refs`
/// back out) along with those three fields — they're still populated during
/// checking (the shared `bynk_check` phase functions this file calls need the
/// sinks regardless of who reads them after), just no longer carried out on
/// this enum.
#[allow(clippy::large_enum_variant)]
pub(super) enum RunChecks {
    /// Discovery/parse failed, or (build mode) the structural gate bailed:
    /// only diagnostics, no checked program. Index is not assembled here.
    Bailed {
        errors: ErrorSink,
        snapshots: Vec<(PathBuf, String)>,
        locals: LocalsSink,
        exprs: ExprTypeSink,
    },
    /// All phases ran (per-unit checks + tests + platform-lock done).
    Checked {
        errors: ErrorSink,
        snapshots: Vec<(PathBuf, String)>,
        locals: LocalsSink,
        exprs: ExprTypeSink,
        parsed: Vec<ParsedFile>,
        compiled: Vec<StagedFile>,
        runnable_tests: Vec<RunnableTest>,
        integration_outputs: Vec<StagedFile>,
        integration_runnables: Vec<RunnableTest>,
        groups: BTreeMap<String, Vec<usize>>,
        kinds: BTreeMap<String, UnitKind>,
        unit_uses: HashMap<String, Vec<String>>,
        unit_consumes: HashMap<String, Vec<String>>,
        unit_consumes_aliases: HashMap<String, HashMap<String, String>>,
        unit_tables: HashMap<String, UnitTable>,
        // #2252: per context, the types it takes from a consumed context —
        // the names whose codecs `emit_boundary_helpers` emits under the same
        // rule — so the entry validates one through that codec.
        unit_consumed_context_types: HashMap<String, HashSet<String>>,
        // #1187's slice 6 plumbing: each unit's own `Callee` classification,
        // merged across its files (`ExprId` is a single project-wide
        // counter, `project_model.rs`'s `next_expr_id`, so merging different
        // files' maps never collides) — checked, resolved data the pre-check
        // `unit_tables` above cannot carry. Exists so a later, project-wide
        // pass (`build_output`/`emit_composition_root`) can read an
        // already-resolved `Callee::Capability`/`Callee::Cross` instead of
        // re-deriving the same fact by walking raw AST method-call syntax —
        // `check_unit_files`'s own per-file `CheckedProgram` was previously
        // built and dropped before any such later pass ever ran. Filtered at
        // merge time (`check_unit_files`'s own `unit_callees.extend` call,
        // review of #1202) to only `Callee::Cross` and
        // `Callee::Capability{cap:"Events",op:"emit"}` — the two variants
        // `unit_table_uses_emit`/`called_cross_context_services` actually
        // read today; widen the filter (or drop it) the moment a future
        // reader needs a different variant, rather than paying to retain
        // every call site's full classification project-wide for the rest
        // of the build on spec.
        unit_callees: HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
        unit_flattened: HashMap<String, HashMap<String, String>>,
        adapter_bindings: HashMap<String, AdapterBinding>,
        npm_deps: std::collections::BTreeMap<String, String>,
        target: BuildTarget,
        // #3580: whether this build records per-request traces (`bynk dev
        // --trace-events`); read by `build_output` for the Worker entry.
        trace_requests: bool,
        inspect_agents: bool,
        // Events track, slice 3c (#980): the reconciled registry document,
        // ready for `finish_build` to serialize onto
        // `ProjectOutput::schema_lock`. `None` when `schema_registry` was
        // `SchemaLock::Off` (#1078) — nothing for a caller to persist.
        schema_registry: Option<schema_registry::SchemaRegistry>,
        // #1883, slice 1b: the bytes of each declared asset file, by
        // normalised path ([`bynk_check::assets::read_declared_files`]), for
        // the Workers' files. `None` when no folder was listed.
        asset_bytes: Option<BTreeMap<String, Vec<u8>>>,
        // P4b (#2608): each `D1` database's migrations, copied beside its
        // Worker's `wrangler.toml`.
        d1_migrations: bynk_check::d1_migrations::D1Migrations,
        // #3562: where each context's Worker is, as the checks above read it.
        layout: WorkerLayout,
        // 1C.1a: the plan and binding model, derived once; `None` while the
        // program has errors.
        composed: Option<ComposedBuild>,
        // State-migrations S4 (#2259): each agent class's reconciled versions
        // and valid steps, keyed `<unit>.<Agent>`, for `bynk migrate status`.
        // Empty when the build has no lock.
        agent_versions: HashMap<String, bynk_check::schema_registry::AgentVersions>,
    },
}

/// A project's unit groups, kinds and parsed files, as [`resolve_derived`] reads them.
type DerivedInputs<'a> = (
    &'a BTreeMap<String, Vec<usize>>,
    &'a BTreeMap<String, UnitKind>,
    &'a [ParsedFile],
);

/// Resolve each context's derived capabilities, and read the Worker layout they
/// resolve against. Groups are a Workers topology: any other target has each
/// context alone.
fn resolve_derived(
    target: BuildTarget,
    (project_root, overlay): (&Path, &HashMap<PathBuf, String>),
    (groups, kinds, parsed): DerivedInputs<'_>,
    unit_tables: &mut HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
) -> WorkerLayout {
    let layout = match target {
        BuildTarget::Workers => WorkerLayout::for_project(project_root, overlay),
        _ => WorkerLayout::identity(),
    };
    bynk_check::project_model::phase_resolve_derived(
        groups,
        kinds,
        parsed,
        &layout,
        unit_tables,
        errors,
    );
    layout
}

/// The `D1` side of [`run_checks`]: each instance's migrations directory,
/// checked and read (P4b, #2608; a directory no instance owns is a warning), and
/// the in-memory SQLite those migrations build, which every literal D1 statement
/// is prepared against as its unit is checked (#3070; `[check] d1_sql = false`
/// turns it off). A project that opts in to the PGlite `Postgres` double
/// (`[test] postgres = "pglite"`, #3321) hands its `schema.sql`, when it has one,
/// to the test emitters under [`PG_TEST_KEY`](bynk_check::d1_migrations::PG_TEST_KEY),
/// after the SQLite check, which reads migrations only.
fn d1_phase(
    migration_files: Option<&assets::AssetFolder>,
    (kinds, unit_tables, layout): (
        &BTreeMap<String, UnitKind>,
        &HashMap<String, UnitTable>,
        &WorkerLayout,
    ),
    (project_root, overlay): (&Path, &HashMap<PathBuf, String>),
    errors: &mut ErrorSink,
) -> (
    bynk_check::d1_migrations::D1Migrations,
    Option<Arc<bynk_check::d1_sql::D1Sql>>,
) {
    let mut migrations = bynk_check::d1_migrations::phase_d1_migrations(
        migration_files,
        &bynk_check::d1_migrations::declared_databases(kinds, unit_tables, layout),
        errors,
    );
    let d1_sql = if bynk_project::read_check_config(project_root, overlay).d1_sql {
        bynk_check::d1_sql::D1Sql::build(&migrations, layout, errors)
    } else {
        None
    };
    if bynk_project::read_test_postgres(project_root) == bynk_project::TestPostgres::Pglite {
        let schema = std::fs::read_to_string(project_root.join(bynk_project::PG_SCHEMA_FILE))
            .ok()
            .map(|sql| {
                vec![bynk_check::d1_migrations::MigrationFile {
                    name: bynk_project::PG_SCHEMA_FILE.to_string(),
                    sql,
                }]
            })
            .unwrap_or_default();
        migrations.insert(bynk_check::d1_migrations::PG_TEST_KEY.to_string(), schema);
    }
    (migrations, d1_sql)
}

/// The manifest checks that need the project's units, read as the editor reads
/// them (`bynk_check::analysis::analyse_project`): `[package] exports` names
/// only commons and adapters the project declares (#2207, packaging slice 1,
/// decision D), and `[deploy.workers]` groups only contexts it declares (#3562).
fn manifest_unit_diagnostics(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
    kinds: &BTreeMap<String, UnitKind>,
    (mode, target): (Mode, BuildTarget),
    errors: &mut ErrorSink,
) {
    let bundle_build = mode == Mode::Build && target == BuildTarget::Bundle;
    let found = bynk_check::exports::manifest_diagnostics(Some(project_root), overlay, kinds)
        .into_iter()
        .chain(bynk_check::worker_groups::manifest_diagnostics(
            Some(project_root),
            overlay,
            kinds,
        ));
    for err in found {
        errors.push_for(Some(Path::new(bynk_check::stability::MANIFEST)), err);
    }
    // #3562: a bundle has every context in one program, so a group means
    // nothing to it; say so rather than build as if it were used.
    if bundle_build
        && bynk_project::worker_groups::try_read_worker_groups_with(project_root, overlay)
            .is_ok_and(|groups| !groups.is_empty())
    {
        errors.push_for(
            Some(Path::new(bynk_check::stability::MANIFEST)),
            bynk_syntax::error::CompileError::new(
                "bynk.deploy.workers_ignored_on_bundle",
                bynk_syntax::span::Span::default(),
                "`[deploy.workers]` groups are ignored by a `bundle` build, which has every context in one program",
            )
            .with_note("set `[build] target = \"workers\"`, or pass `--target workers`, to build the groups as Workers"),
        );
    }
}

/// The parsed files, each unit's files, and each unit's kind: what
/// [`platform_lock_phase`] reads the units' sources from.
type LockSources<'a> = (
    &'a [ParsedFile],
    &'a BTreeMap<String, Vec<usize>>,
    &'a BTreeMap<String, UnitKind>,
);

/// Each unit's symbol table, `consumes`, `consumes` aliases and flattened names:
/// what [`platform_lock_phase`] walks the provider closure with.
type LockTables<'a> = (
    &'a HashMap<String, UnitTable>,
    &'a HashMap<String, Vec<String>>,
    &'a HashMap<String, HashMap<String, String>>,
    &'a HashMap<String, HashMap<String, String>>,
);

/// v0.19 (decisions 0017/0024): platform-lock enforcement. A deployment unit
/// whose in-process closure reaches a platform-native capability is locked to
/// that platform; the selected `--platform` must match. A unit is a context
/// under `workers`, or a `[deploy.workers]` group of them (#3562, which share
/// a Worker). Run only on otherwise-clean programs: the closure walk recurses
/// the provider graph, whose acyclicity the earlier checks establish. P5.3:
/// relocated to `bynk_check::project_model::phase_platform_lock` — this crate is
/// a caller, not an owner.
fn platform_lock_phase(
    target: BuildTarget,
    platform: Platform,
    (project_root, overlay): (&Path, &HashMap<PathBuf, String>),
    (parsed, groups, kinds): LockSources<'_>,
    (unit_tables, unit_consumes, unit_consumes_aliases, unit_flattened): LockTables<'_>,
    errors: &mut ErrorSink,
) {
    if !errors.is_empty() {
        return;
    }
    // A malformed `[deploy.workers]` is reported when the project loads, before
    // this phase runs, so a table that cannot be read here is not a silent
    // loss of grouping.
    let worker_groups =
        bynk_project::worker_groups::try_read_worker_groups_with(project_root, overlay)
            .unwrap_or_default();
    project_model::phase_platform_lock(
        target,
        platform,
        parsed,
        groups,
        kinds,
        unit_tables,
        unit_consumes,
        unit_consumes_aliases,
        unit_flattened,
        &worker_groups,
        errors,
    );
}

/// Each adapter binding's module specifier, for the test passes: the platform bindings (`bynk` and
/// `bynk.*`) are not imported by a test module, `bynk.crypto` is. Moved out of `run_checks`
/// unchanged, so that it does not grow past its function-length ceiling (1B.2a, #4028).
fn test_binding_modules(
    adapter_bindings: &HashMap<String, AdapterBinding>,
) -> HashMap<String, String> {
    adapter_bindings
        .iter()
        .filter(|(name, _)| {
            let first_party = bynk_check::firstparty::BYNK_UNIT;
            name.as_str() == bynk_check::firstparty::CRYPTO_UNIT
                || (name.as_str() != first_party
                    && !name
                        .strip_prefix(first_party)
                        .is_some_and(|rest| rest.starts_with('.')))
        })
        .map(|(name, b)| {
            (
                name.clone(),
                emitter::ts_specifier(&b.output_path.with_extension("")),
            )
        })
        .collect()
}

/// Reconcile agent state shapes and `derives Ord` orders against the lock,
/// attributing a refused change to the declaring file. Returns each live
/// agent's versions.
fn reconcile_agents_and_orders(
    existing: &schema_registry::SchemaRegistry,
    updated: &mut schema_registry::SchemaRegistry,
    units: (
        &HashMap<String, bynk_check::symbols::UnitTable>,
        &HashMap<String, Vec<String>>,
    ),
    parsed: &[ParsedFile],
    errors: &mut ErrorSink,
) -> HashMap<String, bynk_check::schema_registry::AgentVersions> {
    let mut agent_errors: Vec<CompileError> = Vec::new();
    let versions = bynk_check::schema_registry::reconcile_agents(
        existing,
        updated,
        units.0,
        units.1,
        &mut agent_errors,
    );
    errors.extend_attributed_by_span(parsed, agent_errors);
    let mut order_errors: Vec<CompileError> = Vec::new();
    bynk_check::ord_order::reconcile_orders(existing, updated, parsed, &mut order_errors);
    errors.extend_attributed_by_span(parsed, order_errors);
    versions
}

/// Reconcile the lock against the declarations (events, agent state shapes and `derives Ord`
/// orders), when the caller pre-read it. Returns the versions the lock gives, the reconciled
/// document, and the lock as it was read, which the Analytics mappings reconcile against once the
/// units are checked (#4105).
fn reconcile_lock(
    schema_registry: &SchemaLock,
    project_root: &Path,
    units: (
        &HashMap<String, bynk_check::symbols::UnitTable>,
        &HashMap<String, Vec<String>>,
    ),
    parsed: &[ParsedFile],
    errors: &mut ErrorSink,
) -> (
    SchemaVersions,
    Option<schema_registry::SchemaRegistry>,
    Option<schema_registry::SchemaRegistry>,
) {
    let mut versions = SchemaVersions::default();
    let SchemaLock::On { existing } = schema_registry else {
        return (versions, None, None);
    };
    match bynk_check::schema_registry::parse_or_diagnose(existing.as_deref(), project_root) {
        Ok(existing_reg) => {
            let mut schema_errors: Vec<CompileError> = Vec::new();
            let (mut updated, effective) =
                bynk_check::schema_registry::reconcile(&existing_reg, units, &mut schema_errors);
            errors.extend_for(None, schema_errors);
            // State-migrations S0 (#2204) and #3956: agent state shapes and the order of each
            // `derives Ord` type, in the same document.
            versions.agents =
                reconcile_agents_and_orders(&existing_reg, &mut updated, units, parsed, errors);
            versions.events = effective;
            (versions, Some(updated), Some(existing_reg))
        }
        Err(err) => {
            errors.push_for(None, err);
            (versions, None, None)
        }
    }
}

/// #4105: the column mapping of each `Analytics` instance written with `writeValue`, against the
/// lock's `[analytics]`. Known only once the units are checked, so it runs after them.
fn reconcile_analytics_mappings(
    existing: &Option<schema_registry::SchemaRegistry>,
    updated: &mut Option<schema_registry::SchemaRegistry>,
    (unit_callees, parsed): (
        &HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
        &[ParsedFile],
    ),
    errors: &mut ErrorSink,
) {
    let (Some(existing), Some(updated)) = (existing, updated) else {
        return;
    };
    let mut analytics_errors: Vec<CompileError> = Vec::new();
    bynk_check::analytics_lock::reconcile_analytics(
        existing,
        updated,
        unit_callees,
        &mut analytics_errors,
    );
    errors.extend_attributed_by_span(parsed, analytics_errors);
}

/// The rewrites that run right after parsing, before any check: service
/// defaults (ADR 0147), then #3941's `env(NAME)` constants, which take the
/// default environment's values.
fn normalize_parsed(
    parsed: &mut [ParsedFile],
    project_root: &Path,
    overlay: &std::collections::HashMap<PathBuf, String>,
    environment: &str,
    errors: &mut ErrorSink,
) {
    project_model::normalize_service_defaults(parsed);
    project_model::resolve_env_consts_for(parsed, project_root, overlay, environment, errors);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_checks(
    // R3.9 (#1113): one `(root, prefix)` pair per `Roots::trees` entry, not a
    // hardcoded primary/secondary pair — every `include` tree is walked, not
    // just the first one or two.
    trees: &[(PathBuf, PathBuf)],
    target: BuildTarget,
    platform: Platform,
    import_ext: ImportExt,
    mode: Mode,
    overlay: &HashMap<PathBuf, String>,
    // v0.113: absolute subtrees to skip during discovery (author `exclude` plus
    // the tool's `out`/`node_modules` caches). Empty for in-memory builds.
    excludes: &[PathBuf],
    // v0.108 (in-browser track, slice 3): when `Some`, the source files are
    // supplied directly, one file list per `trees` entry — and filesystem
    // discovery is skipped. The wasm/REPL entry feeds an in-memory
    // single-module project this way (the source itself rides in `overlay`);
    // `None` keeps the on-disk discovery walk for the CLI and the LSP.
    discovered: Option<Vec<Vec<PathBuf>>>,
    // v0.115: emit the function-contract call-site guard (dev/test profile).
    contracts: bool,
    trace_requests: bool,
    inspect_agents: bool,
    typed_capability_calls: bool,
    // #1821 (ADR 0147 D3): check and emit `suite`s. Off for a deployable build.
    tests: bool,
    // Events track, slice 3c (#980): `On` reconciles `bynk.schema.lock` from its pre-read
    // content (#1078: no disk access here); `Off` skips it. See `CompileOptions::schema_registry`.
    schema_registry: &SchemaLock,
    // #2138: a remote test build (`CompileOptions::remote_tests`); `None`
    // for every other build.
    remote_tests: Option<&RemoteTests>,
    // #1085 review: only for `schema_registry::parse`'s corruption message —
    // naming *which* project's lock file is corrupt, now that #1078 made
    // `bynk-emit` disk-free (and so path-blind) for this file.
    // The project's directory, and the environment whose `bynk.toml` values the
    // build resolves `env(…)` constants from (#3941).
    (project_root, environment): (&Path, &str),
    // #1883, slice 1b: the listed `assets/` folder
    // (`CompileOptions::asset_files`). `None` when no filesystem was
    // consulted: a check then skips the existence check, a build treats it as
    // empty.
    asset_files: Option<&assets::AssetFolder>,
    // #2387, slice 2: the listed `embeds/` folder
    // (`CompileOptions::embed_files`), with the same `None` meaning.
    embed_files: Option<&assets::EmbedFolder>,
    // #2841: the listed `fixtures/` folder (`CompileOptions::fixture_files`),
    // with the same `None` meaning.
    fixture_files: Option<&assets::FixtureFolder>,
    // P4b (#2608): the listed `migrations/` folder
    // (`CompileOptions::migration_files`). `None` when no filesystem was
    // consulted: nothing is checked, and no test database is migrated.
    migration_files: Option<&assets::AssetFolder>,
    // #2842: the project's `snapshots/` files (`CompileOptions::snapshot_files`).
    snapshot_files: Option<&BTreeMap<String, String>>,
    // #2223: resolve `project_root`'s path dependencies and fold them in
    // (`bynk_check::packages::apply_graph`). Only a manifest-backed project
    // has any; an in-memory build never does.
    packages: bool,
    tys: &Arc<Types>,
    // #3100: report each context as it compiles (`CompileOptions::progress`).
    progress: Option<&ProgressSink>,
) -> RunChecks {
    let mut errors = ErrorSink::new();
    // S3 (#1963): the project's `[unstable]` opt-in, read as the editor reads it
    // (`bynk_check::analysis::analyse_project`), so the two never disagree.
    let policy = Arc::new(bynk_check::stability::StabilityPolicy::for_project(
        Some(project_root),
        overlay,
    ));
    for err in policy.manifest_diagnostics() {
        errors.push_for(Some(Path::new(bynk_check::stability::MANIFEST)), err);
    }
    // v0.25 (ADR 0053): binding edges, recorded at the resolution sites — by
    // `check_file_core` and several `project_model::phase_*` resolution
    // calls below, all of which require this sink unconditionally.
    // `bynk_check::analysis::analyse_project` threads the same sinks through
    // its own call to those functions and does read its copy back out (into
    // `ProjectAnalysis.index`, via `assemble_index`). Here, #1541 deleted the
    // one caller (`analyse_project_with`) that drained this copy back out via
    // `RunChecks`, so it's populated and discarded — never surfaced by this
    // enum anymore.
    let mut refs = RefSink::new();
    // v0.27 (ADR 0056): inferred-type inlay hints, recorded at the checker's
    // binding sites. A sink (not part of the checker's Ok payload) so hints
    // survive the per-file error-`continue`s. Same #1541 residue as `refs`:
    // populated, never read back out of `RunChecks`.
    let mut hints = HintSink::new();
    let mut locals = LocalsSink::new();
    // v0.99: the capability-requirement ledger — recorded at the checker's
    // capability-consuming sites. Same #1541 residue as `refs`/`hints`.
    let mut requirements = RequirementSink::new();
    // v0.30.2 (ADR 0063): per-file expression types, so `.`-member completion
    // can type a receiver — on the error paths too, as best-effort partial
    // types (ADR 0094). Carried like `hints`.
    let mut exprs = ExprTypeSink::new();
    let mut snapshots: Vec<(PathBuf, String)> = Vec::new();

    // -- 1. Discovery (skipped when sources are supplied in memory). --
    let file_lists = match discovered {
        Some(files) => files,
        None => match project_model::phase_discovery(trees, excludes, &mut errors) {
            Ok(files) => files,
            Err(()) => {
                return RunChecks::Bailed {
                    errors,
                    snapshots,
                    locals,
                    exprs,
                };
            }
        },
    };
    // #1077/#1081 review: `no_sources`/file-directory-conflict checks run on
    // every tree's file list regardless of provenance — see
    // `check_discovered_files`'s own doc.
    if project_model::check_discovered_files(trees, &file_lists, &mut errors).is_err() {
        return RunChecks::Bailed {
            errors,
            snapshots,
            locals,
            exprs,
        };
    }

    // -- 2. Parse every file. --
    let (mut parsed, injected, broken) = match project_model::phase_parse(
        trees,
        &file_lists,
        overlay,
        &mut errors,
        &mut snapshots,
    ) {
        Ok(out) => out,
        Err(()) => {
            return RunChecks::Bailed {
                errors,
                snapshots,
                locals,
                exprs,
            };
        }
    };
    // -- 2a. #2223: fold the project's path dependencies in. --
    if packages {
        bynk_check::packages::apply_graph(
            Some(project_root),
            overlay,
            &mut parsed,
            tests,
            &mut errors,
        );
    }

    // -- 2b. Normalize service-level `by`/`given` defaults (v0.155). A service
    //        header default is injected into every handler that omits its own
    //        clause, so every downstream phase (grouping, checking, validation,
    //        emission) reads canonical handlers with no special-casing. `parsed`
    //        is indexed (not cloned) by later phases, so mutating it here reaches
    //        them all. The parsed AST that `bynk fmt` produces is untouched (it
    //        parses independently), so the terse inheriting source round-trips.
    normalize_parsed(&mut parsed, project_root, overlay, environment, &mut errors);
    // #1821 (ADR 0147 D3): a deployable build strips every suite, here, right
    // after parsing, so no later pass (the reserved-namespace and
    // function-type checks walk every parsed unit) sees one: never
    // type-checked for the build, never emitted. A file holding a unit and
    // its suite is two parsed units, so the unit stays. A file that does not
    // parse at all has already failed above: it cannot be told apart from
    // production source.
    if !tests {
        parsed.retain(|pf| !matches!(pf.kind(), UnitKind::Test | UnitKind::Integration));
    }
    project_model::phase_test_only_commons(&mut parsed, tests, &mut errors); // #3984
    // #3243: `parsed` is final here. Register its files once, so the per-unit
    // attribution in `check_unit_files` does not re-register every file for every unit.
    errors.register_parsed(&parsed);

    // -- 3. Group by (name, kind) and validate per-directory consistency.
    //       P5.2 (`design/tracks/semantics-in-the-checker.md` §6):
    //       `phase_group` now also confines function types to non-boundary
    //       positions directly, at the point its old optional hook used to
    //       fire — see that function's own doc comment. --
    let (groups, kinds, test_groups, integration_groups, adapter_bindings, npm_deps) =
        project_model::phase_group(&parsed, trees, platform, &injected, overlay, &mut errors);
    manifest_unit_diagnostics(project_root, overlay, &kinds, (mode, target), &mut errors);
    // -- 4. Build per-unit combined symbol tables. --
    // -- 4-5. Symbol tables, `uses` resolution, and (#3445) the constants named by
    //         literal-only annotation arguments, resolved ahead of every reader. --
    let (unit_tables, unit_uses) = project_model::phase_tables_uses_consts(
        &groups,
        &kinds,
        &mut parsed,
        &policy,
        &mut errors,
        &mut refs,
    );

    // -- 5b. Resolve `consumes` clauses (target must exist + be a context). --
    let (unit_consumes, unit_flattened, unit_type_aliases) = project_model::phase_resolve_consumes(
        &groups,
        &kinds,
        &parsed,
        &unit_tables,
        &policy,
        &mut errors,
        &mut refs,
    );
    let mut unit_tables = unit_tables;
    bynk_check::symbols::record_flattened_caps(
        &mut unit_tables,
        &unit_flattened,
        &unit_type_aliases,
    );
    let layout = resolve_derived(
        target,
        (project_root, overlay),
        (&groups, &kinds, &parsed),
        &mut unit_tables,
        &mut errors,
    );
    // #4045: `uses` views: selection, renames, aliases, qualified names, shared names.
    uses_view::phase_use_views(&unit_uses, &parsed, &groups, &mut unit_tables, &mut errors);
    let unit_tables = unit_tables;
    // #3477: each `Container`'s `@build` directory holds a Dockerfile.
    bynk_check::containers::phase_container_builds(
        &groups,
        &kinds,
        &parsed,
        &unit_tables,
        project_root,
        &mut errors,
    );

    // -- 5b'. Collect `consumes` aliases (v0.6 §3.1). Each consuming context
    //         has an alias map: alias → consumed-context qualified name.
    //         Detect alias-alias conflicts here; alias-vs-local-decl conflicts
    //         are checked once the local symbol tables are built (step 6+).
    let unit_consumes_aliases =
        project_model::phase_consumes_aliases(&groups, &kinds, &parsed, &unit_tables, &mut errors);

    // -- 5b''. v0.173 (ADR 0196 D1): warn where a `bynk.Secrets` read names its
    //          secret with a computed expression. P5.5
    //          (`design/tracks/semantics-in-the-checker.md` §6, §9): relocated
    //          to `bynk_check::project_model::phase_secrets_computed_name` —
    //          this is now a caller, not an owner, the same move as this
    //          function's neighbours above. See that function's own doc for
    //          why raising it here no longer reaches the editor by itself.
    project_model::phase_secrets_computed_name(
        target,
        &parsed,
        &groups,
        &kinds,
        &unit_flattened,
        &mut errors,
    );
    // #1822, #3328, #2306: the project-wide topology checks.
    project_model::phase_topology(target, &parsed, &groups, &kinds, &unit_tables, &mut errors);
    // #2959: on Workers, the `on open` transfer key must be one the edge can
    // evaluate before the body runs.
    project_model::phase_workers_ws_open_key(
        target,
        &parsed,
        &groups,
        &kinds,
        &unit_tables,
        &unit_uses,
        &mut errors,
    );
    // #2355 follow-up: on Workers, a capability whose provider constructs
    // another context's agents cannot be built by this context's Worker.
    project_model::phase_workers_provider_agent_cross_context(
        target,
        &parsed,
        &groups,
        &kinds,
        &unit_tables,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_flattened,
        &mut errors,
    );

    // -- 5c. Detect `consumes` cycles. --
    project_model::phase_detect_consumes_cycles(&groups, &parsed, &unit_consumes, &mut errors);

    // -- 6.1. #2618: a public signature naming a private type. --
    project_model::phase_private_in_public(&parsed, &groups, &mut errors);

    // -- 6a0. #1883, slice 1b: every declared asset names a file in the
    //         `assets/` folder. A build with no folder read checks against an
    //         empty one; a check with none skips this. Then the declared files
    //         alone are read, through the folder's reader.
    let no_assets = BTreeSet::new();
    let available_assets: Option<&BTreeSet<String>> = match (asset_files, mode) {
        (Some(folder), _) => Some(folder.files()),
        (None, Mode::Build) => Some(&no_assets),
        (None, _) => None,
    };
    bynk_check::assets::phase_asset_files(&parsed, available_assets, &mut errors);
    let asset_bytes = asset_files
        .map(|folder| bynk_check::assets::read_declared_files(&parsed, folder, &mut errors));
    // -- 6a0'. #2387, slice 2: likewise every declared embed names a file in
    //          `embeds/`, then the declared files alone are read, as text.
    let no_embeds = BTreeSet::new();
    let available_embeds: Option<&BTreeSet<String>> = match (embed_files, mode) {
        (Some(folder), _) => Some(folder.files()),
        (None, Mode::Build) => Some(&no_embeds),
        (None, _) => None,
    };
    bynk_check::embeds::phase_embed_files(&parsed, available_embeds, &mut errors);
    let embed_text = read_embeds(
        embed_files,
        (&parsed, &unit_tables, &unit_uses),
        &policy,
        &mut snapshots,
        &mut errors,
    );
    // -- 6a0'''. #2841: likewise every suite `fixture` names a file in
    //           `fixtures/`, then the declared files alone are read, as text.
    let no_fixtures = BTreeSet::new();
    let available_fixtures: Option<&BTreeSet<String>> = match (fixture_files, mode) {
        (Some(folder), _) => Some(folder.files()),
        (None, Mode::Build) => Some(&no_fixtures),
        (None, _) => None,
    };
    bynk_check::fixtures::phase_fixture_files(&parsed, available_fixtures, &mut errors);
    let fixture_text = fixture_files
        .map(|folder| bynk_check::fixtures::read_declared_fixtures(&parsed, folder, &mut errors))
        .unwrap_or_default();
    // -- 6a0''. P4b (#2608), #3070, #3321: D1 migrations, and the schemas D1 SQL is checked against.
    let (d1_migrations, d1_sql) = d1_phase(
        migration_files,
        (&kinds, &unit_tables, &layout),
        (project_root, overlay),
        &mut errors,
    );
    // #1883, slice 2: fixed asset paths against each Worker's routes.
    phase_asset_routes(&parsed, &groups, (&kinds, &layout), &unit_uses, &mut errors);
    // A deployable Bundle build (no tests: `bynk compile` without
    // `--target workers`) ships no files, so each asset's `.url` would 404:
    // say so. `bynk test` also builds a Bundle, with tests, and stays quiet.
    if mode == Mode::Build && target == BuildTarget::Bundle && !tests {
        for pf in &parsed {
            for warning in bynk_check::assets::bundle_not_served_warnings(pf.items()) {
                errors.push_for(Some(&pf.identity_path()), warning);
            }
        }
    }

    // -- 6a'. message-bundles slice 1 (#859): messages-block legality,
    //         @reference cardinality, within-block duplicate codes, and the
    //         `uses bynk.locale` dependency. Runs here (not in phase_group)
    //         because it needs `unit_uses`, resolved just above.
    //
    //         P5.0 (#1128, `design/tracks/semantics-in-the-checker.md` §6):
    //         relocated to `bynk-check::project_model` alongside the rest of
    //         this pipeline (P4.1's own move) — this is now a caller, not an
    //         owner, the same way P4.0/P4.1 turned this function into a
    //         caller of `bynk-project`/`bynk-check`.
    project_model::phase_messages_bundles(&parsed, &groups, &kinds, &unit_uses, &mut errors);

    // -- 6a''. Locale capability track, slice 2 (#882): a context reaching
    //          two or more message-bundle commons while consuming `Locale`
    //          has no single bundle to negotiate against. P5.0: relocated,
    //          see above.
    project_model::phase_locale_bundle_ambiguity(
        &parsed,
        &groups,
        &kinds,
        &unit_uses,
        &unit_flattened,
        &mut errors,
    );

    // -- 6a'''. Events track, slice 0 (spine #936): a `from Events(E)`
    //           subscription must name a real, declared event — needs
    //           `unit_tables` + `unit_consumes` together, so it runs here
    //           rather than in the per-context `check_service_protocols`.
    //
    //           P5.1 (#1130, `design/tracks/semantics-in-the-checker.md` §6):
    //           relocated to `bynk-check::project_model`, same move as
    //           P5.0's neighbours above.
    project_model::phase_event_subscriptions(
        &parsed,
        &groups,
        &kinds,
        &unit_tables,
        &unit_consumes,
        &unit_uses,
        &mut errors,
    );

    // -- 6b. Validate exports clauses (each name is a locally-declared type;
    //         no duplicates within or across opaque/transparent). --
    let exports_visibility = project_model::phase_validate_type_exports(
        &groups,
        &kinds,
        &parsed,
        &unit_tables,
        &mut errors,
        &mut refs,
    );

    // -- 6b'. Validate `exports capability { … }` clauses (v0.15 §4.1): each
    //          name must be a capability the context declares *and* provides. --
    project_model::phase_validate_capability_exports(
        &groups,
        &kinds,
        &parsed,
        &unit_tables,
        &mut errors,
        &mut refs,
    );

    // -- 6c. Validate that providers match their capabilities exactly. --
    project_model::phase_validate_providers(
        &unit_tables,
        &groups,
        &parsed,
        &broken,
        &mut errors,
        tys,
    );

    // -- 6d. Events track, slice 3c (#980): reconcile every event's shape
    //        against the committed schema registry. `schema_registry` is
    //        `SchemaLock::Off` for every in-memory/test/LSP/fixture caller
    //        (opt-in — see `CompileOptions::schema_registry`'s doc), in which
    //        case this is a no-op and every event falls back to today's
    //        `@schema(N)`-or-`1` behaviour. Must run before the per-unit loop
    //        below: `emit_unit` needs `schema_effective_versions` to mint the
    //        right `schemaVersion`, and by the time `RunChecks` reaches
    //        `finish_build` the TypeScript is already emitted. Only the
    //        *write* is deferred — to the caller, gated on a fully clean
    //        build (#1078: `bynk-emit` computes, never writes) — reconciliation
    //        itself happens here. P5.3: `reconcile` itself now lives in
    //        `bynk_check::schema_registry` (this crate is a caller, not an
    //        owner) — `SchemaRegistry` stays re-exported from this crate's
    //        own `schema_registry` module. P5.5: the corrupt-file diagnostic
    //        moved too — `bynk_check::schema_registry::parse_or_diagnose` is
    //        now this crate's caller-side of both the parse and the
    //        `bynk.project.schema_registry_corrupt` construction (§3.2's
    //        "eighth site").
    let (schema_effective_versions, mut schema_registry_doc, existing_registry) = reconcile_lock(
        schema_registry,
        project_root,
        (&unit_tables, &unit_uses),
        &parsed,
        &mut errors,
    );

    if !errors.is_empty() && mode == Mode::Build {
        return RunChecks::Bailed {
            errors,
            snapshots,
            locals,
            exprs,
        };
    }

    // -- 7. Build per-unit file index (which file declares which name). --
    let unit_file_index = project_model::phase_file_index(&groups, &parsed);

    // -- 7b (v0.29.4). Assemble the nine parallel per-unit maps into one record
    //          per unit. Driven by the `groups` keyset (the authority), so every
    //          group yields exactly one `UnitInfo` with all facets present. The
    //          producer maps are cloned, not moved, because the back half of the
    //          pipeline (tests, integration tests, platform-lock, composition
    //          root, the workers branch) still reads the originals.
    let unit_info = project_model::assemble_unit_info(
        &groups,
        &kinds,
        &unit_tables,
        &unit_uses,
        &unit_consumes,
        &unit_flattened,
        &unit_consumes_aliases,
        &exports_visibility,
        &unit_file_index,
    );

    // -- 8. For each unit, build the combined symbol space and run
    //       resolve+check per source file. --
    let mut compiled: Vec<StagedFile> = Vec::new();
    // #1187's slice 6 plumbing (see `RunChecks::Checked::unit_callees`'s own
    // doc comment) — one `Callee` map per unit, merged across that unit's
    // own files inside the loop below.
    let mut unit_callees: HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>> =
        HashMap::new();
    // P6.x (#1232): one `EventSubscriberShape` map per unit, merged across
    // that unit's own files inside the loop below — same shape as
    // `unit_callees` immediately above.
    let mut unit_event_subscriber_shapes: HashMap<String, HashMap<String, EventSubscriberShape>> =
        HashMap::new();
    let mut unit_consumed_context_types: HashMap<String, HashSet<String>> = HashMap::new();
    // #2719: the needs-context set, extended unit by unit in dependency order.
    let mut exec_plan = bynk_check::exec_plan::ExecPlan::default();

    // v0.119 (testing track slice 7, ADR 0155): a project-wide fold over every
    // parsed file, producing the identical `HashSet` regardless of which unit
    // or file is currently emitting — computed once here rather than once per
    // emitted file (`collect_history_target_agents` used to be called from
    // inside the per-file emit prologue).
    let history_target_agents = collect_history_target_agents(&parsed);
    // #1883, slice 1b: each declared asset's served URL path, by normalised
    // source path — what its `.url` constant holds. Empty when no folder was
    // read (a missing file is already `bynk.asset.missing_file`).
    let asset_served = asset_bytes
        .as_ref()
        .map(|files| bynk_check::assets::declared_served_paths(&parsed, files))
        .unwrap_or_default();

    // #1702 review: `uses` targets first, so a generic callee's compared type
    // parameters are known before an importer's calls are checked.
    bynk_check::checker::reset_compared_cache();
    // #2137: the contexts a `system` harness stands up as Workers; their
    // test-build `Clock` reads the harness clock. Empty in a deployable build,
    // which carries no suites.
    let harness_clock_contexts = if parsed.iter().any(|p| p.integration().is_some()) {
        let tables: HashMap<String, UnitTable> = unit_info
            .iter()
            .map(|(n, i)| (n.clone(), i.table.clone()))
            .collect();
        let consumes: HashMap<String, Vec<String>> = unit_info
            .iter()
            .map(|(n, i)| (n.clone(), i.consumes.clone()))
            .collect();
        harness_clock_contexts(&parsed, &tables, &consumes, &kinds)
    } else {
        HashSet::new()
    };
    // #3229: one set of cross-context views for the whole project, not one per unit.
    let views = check_pipeline::CrossContextViewsCache::default();
    for name in bynk_check::project_model::uses_first_order(unit_info.keys(), &unit_uses) {
        let info = &unit_info[name];
        let kind = info.kind;
        let indices = info.files.as_slice();
        let local_table = &info.table;
        // #3100: a build reports each context as it compiles.
        let report = progress.filter(|_| mode == Mode::Build && kind == UnitKind::Context);
        if let Some(sink) = report {
            sink(&ProgressEvent::Started(name.clone()));
        }
        // v0.24: skip resolve/check only when THIS group's composition
        // failed. In build mode the sink is empty here (the structural gate
        // bailed), so the delta equals the old global is_empty check; in
        // analyse mode one broken unit no longer suppresses every other
        // unit's semantic diagnostics.
        let group_error_baseline = errors.len();

        let (
            mut combined_types,
            combined_fns,
            mut combined_methods,
            mut imported_from,
            mut imported_from_kind,
        ) = project_model::compose_unit_symbols(name, local_table, &unit_info);
        let consumed_types = project_model::merge_consumed_exports(
            name,
            &parsed,
            &unit_info,
            &mut combined_types,
            &mut combined_methods,
            &mut imported_from,
            &mut imported_from_kind,
            &mut errors,
        );
        // #1807: close over the types the imported declarations reach.
        let reached_types = project_model::close_reachable_types(
            name,
            &parsed,
            &unit_info,
            &mut combined_types,
            &combined_fns,
            &mut combined_methods,
            &mut imported_from,
            &mut imported_from_kind,
            &mut errors,
        );

        if errors.len() > group_error_baseline {
            continue;
        }

        let local_names: HashSet<String> = local_table.types.keys().cloned().collect();

        // #2252: the rule `emit_boundary_helpers` roots a consumed type's
        // codecs by (#2036): imported from a context, and not declared here.
        if kind == UnitKind::Context {
            let from_contexts = imported_from_kind
                .iter()
                .filter(|(n, k)| **k == UnitKind::Context && !local_names.contains(*n))
                .filter(|(n, _)| {
                    imported_from
                        .get(*n)
                        .and_then(|owner| unit_info.get(owner))
                        .is_some_and(|i| i.kind == UnitKind::Context)
                })
                .map(|(n, _)| n.clone())
                .collect();
            unit_consumed_context_types.insert(name.clone(), from_contexts);
        }

        let local_methods_for_type = project_model::collect_unit_methods(indices, &parsed);

        // Per-context view information for the emitter and checker.
        let owning_context_for_emit = if kind == UnitKind::Context {
            Some(name.clone())
        } else {
            None
        };

        // #527: workers contexts get a DO-side deps plan for their agents'
        // `given` capabilities (the wire cannot carry providers).
        let agent_deps_plan = if matches!(target, BuildTarget::Workers) && kind == UnitKind::Context
        {
            plan_agent_given_deps(
                name,
                &unit_info,
                &adapter_bindings,
                indices.len() > 1,
                harness_clock_contexts.contains(name),
                &layout,
            )
        } else {
            None
        };

        check_unit_files(
            name,
            kind,
            &broken,
            indices,
            &parsed,
            &unit_info,
            &views,
            &combined_types,
            &combined_fns,
            &combined_methods,
            &local_names,
            &local_methods_for_type,
            &consumed_types,
            &imported_from,
            &imported_from_kind,
            &reached_types,
            &owning_context_for_emit,
            target,
            import_ext,
            contracts,
            trace_requests,
            inspect_agents,
            typed_capability_calls,
            agent_deps_plan.as_ref(),
            &history_target_agents,
            &asset_served,
            &embed_text,
            mode,
            &policy,
            &d1_sql,
            &mut errors,
            &mut refs,
            &mut hints,
            &mut locals,
            &mut exprs,
            &mut requirements,
            &mut compiled,
            &schema_effective_versions,
            tys,
            unit_callees.entry(name.clone()).or_default(),
            unit_event_subscriber_shapes
                .entry(name.clone())
                .or_default(),
            &mut exec_plan,
            &layout,
        );
        if let Some(sink) = report
            && errors.len() == group_error_baseline
        {
            sink(&ProgressEvent::Finished(name.clone()));
        }
    }

    // #2719: every unit is checked, so the needs-context set is final. Stamp it
    // on each unit's table, which is what the emission gates for service
    // handlers (the Worker entry, `compose`, the Bundle surface, the test
    // harness) already hold.
    let unit_tables: HashMap<String, UnitTable> = {
        let plan = Arc::new(exec_plan);
        unit_tables
            .into_iter()
            .map(|(k, mut t)| {
                t.exec_plan = Arc::clone(&plan);
                (k, t)
            })
            .collect()
    };

    // v0.7: process test declarations. Each `test commerce.X` group resolves
    // its target, validates mocks against the target's capability/consumed-
    // context shapes, type-checks bodies with the target's privileged view,
    // and emits a per-target TypeScript test module under `tests/`.
    let mut test_errors: Vec<CompileError> = Vec::new();
    // v0.132: barrel output paths emitted so far, shared across the unit- and
    // integration-test passes so a multi-file commons imported by both is
    // aggregated into `out/<name>.ts` exactly once.
    let mut emitted_barrels: HashSet<PathBuf> = HashSet::new();
    // #1820: a context split across files emits a module per file and none at
    // `out/<name>.ts`, which the bundle's composition root imports it from.
    // Its barrel is part of the build, not only of a test build; a suite that
    // imports the context reuses it. On workers the context's barrel is its
    // Worker's `handlers.ts` (`emit_worker_handlers_barrel`), and its files
    // aren't at `out/<name>/`, so its path here is claimed and none is
    // emitted: the test passes below emit only a commons' barrel.
    for (name, kind) in &kinds {
        if *kind != UnitKind::Context {
            continue;
        }
        if mode == Mode::Build && target == BuildTarget::Bundle {
            if let Some(barrel) = emit_commons_barrel(
                name,
                &groups,
                &parsed,
                import_ext,
                &mut emitted_barrels,
                &compiled,
            ) {
                compiled.push(barrel);
            }
        } else if target == BuildTarget::Workers {
            emitted_barrels.insert(commons_dir_for(name).with_extension("ts"));
        }
    }
    // #1883, slice 3: the declared assets, for a case's `svc.GET(asset)`.
    let no_asset_bytes = BTreeMap::new();
    let test_asset_files = TestAssetFiles {
        served: &asset_served,
        bytes: asset_bytes.as_ref().unwrap_or(&no_asset_bytes),
        fixtures: &fixture_text,
    };
    // #1975: a user adapter's binding module, for a test that builds the binding
    // for real. A first-party adapter's binding reaches the platform (the network,
    // a KV namespace), so a test keeps its double. `bynk.crypto` is the exception
    // (K4): its binding is Web Crypto over the test's own bytes, deterministic and
    // I/O-free, so a test runs it for real and a fake would check nothing.
    let test_binding_modules = test_binding_modules(&adapter_bindings);
    let (test_outputs, runnable_tests) = process_tests(
        &test_groups,
        &parsed,
        &kinds,
        &unit_tables,
        &exports_visibility,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_uses,
        &broken,
        &unit_flattened,
        &groups,
        import_ext,
        contracts,
        &mut emitted_barrels,
        &test_asset_files,
        snapshot_files,
        &d1_migrations,
        d1_sql.as_deref(),
        &test_binding_modules,
        &schema_effective_versions.events,
        &unit_callees,
        &unit_event_subscriber_shapes,
        project_mentions_encode_plain(&parsed),
        &mut test_errors,
        &mut refs,
        tys,
    );
    // #2138: a remote run runs only the `system` suites; the unit modules are
    // still written (a barrel they emit may be shared) but not listed.
    let runnable_tests = if remote_tests.is_some() {
        Vec::new()
    } else {
        runnable_tests
    };
    // #696/#1659: test-suite diagnostics are attributed to their owning file by
    // each error's span (`ErrorSink::extend_attributed_by_span`), so they render
    // with a file, line and span like every other diagnostic.
    extend_with_test_errors(&mut errors, &parsed, test_errors);

    compiled.extend(test_outputs);

    // v0.16: process integration tests. Each `test integration "name"` suite
    // validates its `wires` participants, type-checks each case body as a
    // cross-context call from a synthetic harness root that consumes every
    // participant, and emits a TypeScript module that stands the participants
    // up as in-process Workers and exercises the flow across the real wire.
    // #1959: each context's bindings, once (1C.1): the integration harness binds
    // them on each participant's `env`, the duplicate check and `build_output` read them.
    let event_subscribers =
        bynk_check::symbols::discover_event_subscribers(&unit_tables, &unit_consumes);
    let derived = errors.is_empty().then(|| {
        derive_bindings(
            (&kinds, &unit_tables, &unit_consumes, &unit_consumes_aliases),
            (
                &unit_callees,
                &unit_flattened,
                &unit_event_subscriber_shapes,
            ),
            (&event_subscribers, &layout, &groups),
        )
    });
    let mut integration_errors: Vec<CompileError> = Vec::new();
    let (integration_outputs, integration_runnables) = process_integration_tests(
        &integration_groups,
        &parsed,
        &kinds,
        &unit_tables,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_uses,
        &broken,
        &groups,
        &mut emitted_barrels,
        &test_asset_files,
        IntegrationD1::new(&d1_migrations, d1_sql.as_deref()),
        derived.as_ref().map(|d| d.bindings.by_context()),
        &event_subscribers,
        remote_tests,
        project_mentions_encode_plain(&parsed),
        &mut integration_errors,
        &mut refs,
        tys,
    );
    // #696/#1659: integration-suite diagnostics, attributed the same way.
    errors.extend_attributed_by_span(&parsed, integration_errors);

    project_model::phase_bundle_composition_unprovided(
        target,
        &parsed,
        &groups,
        &kinds,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_tables,
        (&unit_callees, &unit_uses, tys),
        &unit_flattened,
        &mut errors,
    );

    platform_lock_phase(
        target,
        platform,
        (project_root, overlay),
        (&parsed, &groups, &kinds),
        (
            &unit_tables,
            &unit_consumes,
            &unit_consumes_aliases,
            &unit_flattened,
        ),
        &mut errors,
    );
    // On Workers, each Worker's binding names are unique. Gated the same way:
    // the KV input walks the same provider graph.
    worker_bindings::phase_duplicate_bindings(
        target,
        (project_root, overlay),
        &parsed,
        &groups,
        derived.as_ref().map(|d| d.bindings.by_context()),
        &mut errors,
    );

    // v0.176 (#642): the `Bytes`-at-a-workers-boundary guard (ADR 0142 D8) is
    // retired here. It existed because the workers boundary carried its own
    // codec dispatch, which cast a `Bytes` to `JsonValue` on the way out while
    // base64-decoding it on the way in — so a `Bytes` mis-round-tripped, and a
    // diagnostic was better than silent corruption. That dispatch is gone: every
    // wire position now routes through `serialisation.rs`, whose `Bytes` arm
    // base64-encodes. The restriction has no remaining cause, and ADR 0142 D8's
    // deferral to "the roadmap's typed cross-context boundary fix" is discharged.

    reconcile_analytics_mappings(
        &existing_registry,
        &mut schema_registry_doc,
        (&unit_callees, &parsed),
        &mut errors,
    );

    RunChecks::Checked {
        errors,
        snapshots,
        locals,
        exprs,
        parsed,
        compiled,
        runnable_tests,
        integration_outputs,
        integration_runnables,
        groups,
        kinds,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
        unit_tables,
        unit_consumed_context_types,
        unit_callees,
        unit_flattened,
        adapter_bindings,
        npm_deps,
        target,
        trace_requests,
        inspect_agents,
        schema_registry: schema_registry_doc,
        asset_bytes,
        agent_versions: schema_effective_versions.agents,
        d1_migrations,
        layout,
        composed: derived,
    }
}

/// What `run_checks` derives once for the build (1C.1a, Decision C of #4031):
/// the composition plan, and every Worker's bindings decided from it. The
/// duplicate check, the integration harness and the Workers build all read
/// them, so the check sees exactly the bindings the build writes.
pub(super) struct ComposedBuild {
    pub(super) plan: emitter::ComposePlan,
    pub(super) bindings: emitter::WorkerBindings,
}

/// The build's [`ComposedBuild`]. Only on an otherwise clean program: the plan
/// walks the provider graph, whose acyclicity the earlier checks establish.
#[allow(clippy::type_complexity)]
fn derive_bindings(
    (kinds, unit_tables, unit_consumes, unit_consumes_aliases): (
        &BTreeMap<String, UnitKind>,
        &HashMap<String, UnitTable>,
        &HashMap<String, Vec<String>>,
        &HashMap<String, HashMap<String, String>>,
    ),
    (unit_callees, unit_flattened, unit_event_subscriber_shapes): (
        &HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
        &HashMap<String, HashMap<String, String>>,
        &HashMap<String, HashMap<String, EventSubscriberShape>>,
    ),
    (event_subscribers, layout, groups): (
        &BTreeMap<(String, String), Vec<(String, String)>>,
        &WorkerLayout,
        &BTreeMap<String, Vec<usize>>,
    ),
) -> ComposedBuild {
    let plan = emitter::ComposePlan::build(
        kinds,
        unit_consumes,
        unit_consumes_aliases,
        unit_tables,
        unit_callees,
        unit_flattened,
        event_subscribers,
        unit_event_subscriber_shapes,
    );
    debug_assert_plan_covers_groups(&plan, groups, kinds);
    let bindings = emitter::WorkerBindings::derive(&plan, unit_tables, layout);
    ComposedBuild { plan, bindings }
}

/// Makes that assumption explicit: the plan's contexts are the grouped ones.
fn debug_assert_plan_covers_groups(
    plan: &emitter::ComposePlan,
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
) {
    debug_assert!(
        plan.contexts().map(|(n, _)| n).eq(groups
            .keys()
            .filter(|n| kinds.get(*n) == Some(&UnitKind::Context))),
        "the compose plan's contexts are the grouped contexts"
    );
}

/// #4233: `process_tests` reports an internal fault (a checker fact a suite's
/// lowering needed and did not find) only when *its* error list is otherwise
/// clean, but that list is fresh. A program that already failed checking lacks
/// facts by construction (a unit with a broken file records no subscriber
/// shapes), so such a fault is noise on top of the user's own error: drop it
/// when the project or the test errors already hold an error.
fn extend_with_test_errors(
    errors: &mut ErrorSink,
    parsed: &[ParsedFile],
    mut test_errors: Vec<CompileError>,
) {
    let fault = bynk_emit::emitter::MISSING_CHECKER_FACT;
    let failed = !errors.is_empty()
        || test_errors.iter().any(|e| {
            e.category != fault
                && bynk_syntax::Severity::for_error(e) == bynk_syntax::Severity::Error
        });
    if failed {
        test_errors.retain(|e| e.category != fault);
    }
    errors.extend_attributed_by_span(parsed, test_errors);
}

/// What [`read_embeds`] checks templates against: the parsed files, the unit tables and each
/// unit's `uses`.
type EmbedUnits<'a> = (
    &'a [ParsedFile],
    &'a HashMap<String, UnitTable>,
    &'a HashMap<String, Vec<String>>,
);

/// #2387, slice 2: the text of every declared embed, read through the folder's
/// reader. Typed HTML (#3927, slice 1): then every `template` is checked against
/// its file's text (`bynk_check::templates`), and each template file joins
/// `snapshots` under its path, so a diagnostic in it renders at its line.
fn read_embeds(
    embed_files: Option<&assets::EmbedFolder>,
    (parsed, tables, uses): EmbedUnits<'_>,
    policy: &bynk_check::stability::StabilityPolicy,
    snapshots: &mut Vec<(PathBuf, String)>,
    errors: &mut ErrorSink,
) -> BTreeMap<String, String> {
    let embed_text = embed_files
        .map(|folder| bynk_check::embeds::read_declared_embeds(parsed, folder, errors))
        .unwrap_or_default();
    let units = bynk_check::templates::Units {
        parsed,
        tables,
        uses,
    };
    bynk_check::templates::phase_templates(&units, &embed_text, policy, errors);
    for pf in parsed {
        for item in pf.items() {
            if let bynk_syntax::ast::CommonsItem::Embed(e) = item
                && e.is_template()
            {
                let key = bynk_project::assets::normalise_asset_path(&e.path);
                if let Some(text) = embed_text.get(&key)
                    && !snapshots.iter().any(|(p, _)| p == Path::new(&key))
                {
                    snapshots.push((PathBuf::from(&key), text.clone()));
                }
            }
        }
    }
    embed_text
}
