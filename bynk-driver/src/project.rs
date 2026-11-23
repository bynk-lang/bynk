//! Multi-file project compilation (v0.3 §3.2 and §3.3, v0.4 §3.5).
//!
//! A "project" is a directory tree of `.bynk` source files. The dotted name
//! of a commons or context (e.g., `bynk.time`, `commerce.orders`) maps to a
//! path under the project root — either a single file (`bynk/time.bynk`) or
//! a directory of files all sharing the same header (`bynk/time/*.bynk`).
//!
//! v0.4: each file is one of two kinds — commons or context. Both kinds share
//! the same multi-file directory machinery; they differ in body content
//! (contexts have `consumes`/`exports`, types are nominally per-context), in
//! visibility (contexts export only the types listed), and in TypeScript
//! emission (contexts re-brand types from used commons).
//!
//! Compilation proceeds in two passes:
//!   1. **Discover and parse** every `.bynk` file. Group by qualified name
//!      and kind. Build a global symbol table where each unit contributes
//!      its declarations.
//!   2. **Resolve, type-check, and emit** each unit with full visibility of
//!      the units it transitively `uses` or `consumes`. Two passes keep
//!      `uses` cycles trivial — there is no order-of-evaluation, only
//!      declarative mixin.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bynk_check::assets::phase_asset_routes;
use bynk_check::check_pipeline::{self, prepare_unit_check_ctx};
use bynk_check::checker;
use bynk_check::checker::{ExprId, Types};
use bynk_check::d1_migrations::pglite_on;
use bynk_check::expr_types::ExprTypeSink;
use bynk_check::firstparty::FirstPartyCap;
use bynk_check::firstparty::{self, Platform};
use bynk_check::hints::HintSink;
use bynk_check::index::RefSink;
use bynk_check::locals::LocalsSink;
use bynk_check::project_model::{self, AdapterBinding, FnDecl, TypeDecl, UnitInfo, Visibility};
use bynk_check::requirements::RequirementSink;
use bynk_check::resolver::{self, MethodTable as ResolverMethodTable};
use bynk_emit::emitter;
use bynk_emit::emitter::commons_barrel::emit_commons_barrel;
use bynk_emit::testgen::{
    IntegrationD1, RunnableTest, TestAssetFiles, discovery_manifest, emit_test_main,
    process_integration_tests, process_tests,
};
use bynk_model::EventSubscriberShape;
use bynk_model::FnSig;
use bynk_model::{lower_attached_fn_sig_ir_from_types, lower_handler_given_ir};
use bynk_project::worker_layout::WorkerLayout;
use bynk_syntax::error::CompileError;

// P4.0 (#1113): discovery, the unit graph, path resolution, and cross-unit
// consistency checks moved to `bynk-project` — this crate is now a
// dependent, not an owner, of that code (`design/tracks/project-model.md`
// §6 row P4.0). P4.1 (#1115): `symbols` and the per-file `context_checks`
// subset of `validate` moved to `bynk-check` for the same reason — this
// crate reaches them as `bynk_check::symbols`/`bynk_check::context_checks`
// now. P5.0-P5.3 (`design/tracks/semantics-in-the-checker.md` §6) emptied
// `validate` out the same way, project-wide check by project-wide check,
// including the reconciliation half of `schema_registry` (now
// `bynk_check::schema_registry`). P5.4 moved `tests_emit`'s checking half
// (target/participant resolution, `stub` resolution, case/property body
// type-checking) to `bynk_check::test_suites` the same way — `tests_emit`
// stays here as a caller of it, holding only TypeScript emission plus the
// two functions' unchanged public shape. P5.5 (§5, §6): `validate` reached
// empty (its last occupant, `check_platform_lock`, left at P5.3) and its
// remaining diagnostic-emitting sites — the `bynk.secrets.computed_name`
// warning and the `bynk.project.schema_registry_corrupt` construction, both
// outside the seven-category accounting — relocated too (§3.2's "eighth
// site" and §9's open risk respectively); `validate.rs` and its module
// declaration are deleted, this track's own completion criterion (§5).
// Pipeline-driving types (`diagnostics`'s `Mode`/`ErrorSink`/
// `ProjectAnalysis`/`ProjectFailure`) stay here too.
mod agent_deps;
mod diagnostics;
mod entry;
#[cfg(test)]
mod entry_scope_tests;
mod in_memory;
mod options;
mod remote_tests;
mod schema_registry;
#[cfg(test)]
mod tests;
mod worker_bindings;

pub use agent_deps::AgentDepsPlan;
use agent_deps::plan_agent_given_deps;
use bynk_check::symbols::*;
pub(crate) use bynk_emit::emit_model::StagedFile;
use bynk_emit::emit_model::*;
pub use bynk_emit::emit_model::{
    Artefacts, DiscoveredCase, DiscoveredSuite, Document, ImportExt, ProjectOutput, RemoteSkip,
    RemoteTests, TestLocation, WORKER_ASSETS_DIR, is_worker_asset_path, is_worker_migration_path,
    node_wasm_shim, sibling_path, wasm_shim_path, worker_assets_dir,
};
use bynk_emit::emit_model::{WASM_MODULES_DTS, workers_wasm_shim};
use bynk_project::discovery::*;
use bynk_project::paths::*;
use diagnostics::*;
pub use entry::{D1SqlRef, EmailForward, ProjectCheck, check_project, compile_project};
pub use in_memory::{
    InMemoryAnalysis, InMemoryProjectAnalysis, analyse_in_memory, analyse_in_memory_project,
    analyse_in_memory_with_types, compile_files_in_memory, compile_files_in_memory_locked,
    compile_files_in_memory_remote, compile_in_memory, slash_key,
};
pub use options::{CompileOptions, ProgressEvent, ProgressSink};

// External facade: items referenced as `crate::project::X` from outside this
// module (emitter, main, lib) must stay reachable at that path.
pub use bynk_check::project_model::BuildTarget;
pub use bynk_check::schema_registry::{AgentVersions, MigrationStep};
pub use bynk_check::symbols::{FileDeclIndex, UnitTable};
// 1B.2a (#4028): the suffix is defined beside the test-harness generation that reads it.
pub use bynk_emit::testgen::SMOKE_SUITE_SUFFIX;
// #1883, slice 1b: the `assets/` folder helpers, for the driver and testkit.
pub use bynk_project::assets;
pub use bynk_project::{
    AttributedError, LabelFile, ProjectPaths, ProjectPathsError, Roots, SchemaLock, TestTarget,
    UnitKind, WorkersConfig, Workspace, check_manifest, check_manifest_str, dependency_trees,
    discover_project_files, is_workspace_root, load_workspace, try_read_mutate_exclude,
    try_read_project_paths, try_read_project_paths_with, try_read_test_targets,
    try_read_workers_config, try_read_workers_config_with, worker_dir_name,
    worker_handlers_output_path, worker_handlers_source_path,
};
pub use diagnostics::{ContextBoundaryInfo, ContextSequenceInfo, ProjectAnalysis, ProjectFailure};

/// #1890: one `bynk.project.duplicate_compatibility_flag` warning per
/// duplicate in the manifest's `[workers] compatibility_flags` (`extra`),
/// shared by [`finish_build`] and [`check_project`] so `compile` and `check`
/// report the same thing. Attributed to `bynk.toml` by its root-relative
/// identity path, like every other [`AttributedError`]; the manifest is never
/// a snapshot, so renderers print that path (or none) rather than a source
/// excerpt, and the message names `bynk.toml` itself.
fn compatibility_flag_warnings(extra: &[String]) -> Vec<AttributedError> {
    let (_, duplicates) = bynk_emit::compatibility_flags(extra);
    duplicates
        .into_iter()
        .map(|flag| {
            let default = bynk_emit::DEFAULT_COMPATIBILITY_FLAGS.contains(&flag.as_str());
            bynk_project::duplicate_compatibility_flag_warning(
                &flag,
                default,
                PathBuf::from("bynk.toml"),
            )
        })
        .collect()
}

/// Assemble a finished [`ProjectOutput`] (or a [`ProjectFailure`]) from a
/// [`RunChecks`] result — the shared tail of `compile_project` and
/// `compile_in_memory`.
///
/// #1890: `compatibility_flags` is the manifest's `[workers]` list
/// ([`CompileOptions::compatibility_flags`]). It is merged with the defaults
/// once here, for every Worker, and each duplicate it held becomes one
/// [`compatibility_flag_warnings`] warning, whatever the build target: the
/// duplicate is a fact about the manifest.
fn finish_build(
    run: RunChecks,
    import_ext: ImportExt,
    display_root: &Path,
    compatibility_flags: &[String],
    workers_dev: Option<bool>,
    // #3477: the directory the build is written to, when known.
    out_dir: Option<&Path>,
    // #2554: the pre-read `bynk.contracts.lock`; `None` is a project without
    // one, which accepts no older contract.
    contracts_lock: Option<&str>,
) -> Result<ProjectOutput, ProjectFailure> {
    // #2554: an unreadable contracts lock fails the build, as an unreadable
    // schema lock does: reading it as empty would have a callee silently stop
    // accepting the callers it was deployed to keep answering.
    let contracts_lock = match bynk_project::contracts_lock::parse(contracts_lock, display_root) {
        Ok(lock) => lock,
        Err(message) => {
            return Err(ProjectFailure {
                errors: vec![bynk_project::contracts_lock::corrupt_error(message)],
                snapshots: Vec::new(),
                display_root: display_root.to_path_buf(),
            });
        }
    };
    let (flags, _) = bynk_emit::compatibility_flags(compatibility_flags);
    let flag_warnings = compatibility_flag_warnings(compatibility_flags);
    match run {
        RunChecks::Bailed {
            errors, snapshots, ..
        } => Err(ProjectFailure {
            // ADR 0117: a failed build still renders any warnings it produced
            // (the sink yields errors then warnings).
            errors: errors.into_all().into_iter().chain(flag_warnings).collect(),
            snapshots,
            display_root: display_root.to_path_buf(),
        }),
        RunChecks::Checked {
            errors, snapshots, ..
        } if !errors.is_empty() => Err(ProjectFailure {
            errors: errors.into_all().into_iter().chain(flag_warnings).collect(),
            snapshots,
            display_root: display_root.to_path_buf(),
        }),
        RunChecks::Checked {
            errors,
            snapshots,
            parsed,
            compiled,
            runnable_tests,
            integration_outputs,
            integration_runnables,
            groups,
            kinds,
            unit_consumes,
            unit_consumes_aliases,
            unit_tables,
            unit_consumed_context_types,
            unit_callees,
            unit_uses,
            unit_flattened,
            adapter_bindings,
            npm_deps,
            target,
            trace_requests,
            inspect_agents,
            schema_registry,
            asset_bytes,
            agent_versions,
            d1_migrations,
            layout,
            composed,
            ..
        } => {
            let composed = composed.expect("a build with no errors has its compose plan");
            // #3477: a container's build paths are written from the output
            // directory, which has to be inside the project.
            let container_root = match worker_bindings::container_root(display_root, out_dir) {
                Ok(root) => root,
                Err(outside) => {
                    let errors = worker_bindings::container_output_errors(&unit_tables, &outside);
                    if !errors.is_empty() {
                        return Err(ProjectFailure {
                            errors,
                            snapshots,
                            display_root: display_root.to_path_buf(),
                        });
                    }
                    String::new()
                }
            };
            let mut out = build_output(
                parsed,
                compiled,
                runnable_tests,
                integration_outputs,
                integration_runnables,
                groups,
                kinds,
                unit_consumes,
                unit_consumes_aliases,
                unit_tables,
                &unit_consumed_context_types,
                unit_callees,
                unit_uses,
                unit_flattened,
                adapter_bindings,
                npm_deps,
                target,
                import_ext,
                &flags,
                workers_dev,
                &container_root,
                asset_bytes.as_ref(),
                &agent_versions,
                &contracts_lock,
                &d1_migrations,
                &layout,
                trace_requests,
                inspect_agents,
                composed,
            );
            // ADR 0117: surface non-failing warnings on the successful build
            // (errors is empty here — the guard arm above caught any).
            // `build_output` put the contracts lock's warnings here (#2554).
            let lock_warnings = std::mem::take(&mut out.warnings);
            out.warnings = errors.into_warnings();
            out.warnings.extend(flag_warnings);
            out.warnings.extend(lock_warnings);
            out.snapshots = snapshots;
            out.display_root = display_root.to_path_buf();
            // #1078: the reconciled registry, if this build had one on —
            // bynk-emit computes it, the caller persists it.
            out.schema_lock = schema_registry.map(|reg| schema_registry::serialize(&reg));
            Ok(out)
        }
    }
}

// P4.1 (#1115): `normalize_service_defaults`/`inject_service_defaults`
// relocated to `bynk-check::project_model` (called from both `run_checks`
// here and the new `bynk-check`-native analysis entry point, ahead of
// `phase_group` in both).

// P4.1 (#1115): `record_analyse_types` relocated to
// `bynk-check::check_pipeline` (called from `check_file_core`'s own
// error-path exits, plus every clean-path caller — `check_unit_files`'s
// `Mode::Analyse` branch below and the new entry point's own).

// P4.1 (#1115): the whole discovery->parse->group->resolve pipeline
// (`phase_discovery` through `phase_file_index`/`assemble_unit_info`, plus
// per-unit symbol composition — `compose_unit_symbols`/
// `merge_consumed_exports`/`collect_unit_methods`) relocated to
// `bynk-check::project_model` — see that module's own doc comment for why
// (the same `extract, don't duplicate` move `bynk-project` itself was, P4.0).
// `run_checks`, below, is now a caller of `project_model::phase_*` instead of
// owning this logic inline.

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

/// Phase 8e: build the emitter context for one checked source file and render
/// its TypeScript, pushing the result onto `compiled`. Reached only in build
/// mode (the caller's analyse-mode `continue` gates this off); the block is
/// straight-line with no `continue`s of its own.
#[allow(clippy::too_many_arguments)]
/// Emit-prologue tables that depend only on the *unit* (`name`/`unit_info`/
/// `target`) — never on which file within the unit is being emitted. Building
/// one of these once per unit, ahead of the per-file loop, replaces what used
/// to be an identical rebuild (several nested nested loops over `unit_info`)
/// on every emitted file of a multi-file context.
struct EmitUnitCtx {
    imported_methods: HashMap<String, Vec<FnSig>>,
    /// The workers-mode-rewritten view is the only one `emit_unit` reads —
    /// the pre-rewrite table is an intermediate of computing it, not exposed
    /// separately.
    imported_decl_paths_emit: HashMap<String, HashMap<String, PathBuf>>,
    exports_for_consumed: HashMap<String, HashMap<String, Visibility>>,
    file_decl_index: FileDeclIndex,
    /// #1820: a context split across files, built for Workers. Each file is
    /// emitted as its own module ([`worker_file_source_path`]) and
    /// `handlers.ts` re-exports them ([`emit_worker_handlers_barrel`]), rather
    /// than every file being written to `handlers.ts` in turn, each
    /// overwriting the last. `file_decl_index` then names each declaration's
    /// module, so one file imports what it uses from another.
    split_worker: bool,
    /// #3562: where each context's files go.
    layout: WorkerLayout,
}

fn build_emit_unit_ctx(
    name: &str,
    unit_info: &BTreeMap<String, UnitInfo>,
    hidden_types: &BTreeMap<String, String>,
    target: BuildTarget,
    tys: &Arc<Types>,
    split_worker: bool,
    layout: &WorkerLayout,
) -> EmitUnitCtx {
    let info = &unit_info[name];
    // v0.132.1 (#481): gather the attached methods of every `uses`-imported type
    // (one level, matching the symbol-table merge). `emit_context_rebrands`
    // forwards these onto the consumer's rebranded const so a call like
    // `Cents.fromInt(n)` type-checks. Sorted by method name for deterministic
    // emission (the resolver stores instance/static methods in `HashMap`s).
    //
    // P6.18: each method's own `params`/`return_type` now resolve to a real
    // `TyId` (`bynk_model::lower_attached_fn_sig_ir_from_types`) against the
    // *declaring* unit's own visible types, rather than carrying the raw
    // `FnDecl` (and its unresolved `TypeRef`s) all the way to
    // `emit_forwarded_methods`. `Free`-named entries (never present in
    // practice — `ResolverMethodTable` only ever collects attached methods,
    // `bynk-check/src/resolver.rs:47`) are skipped, matching
    // `emit_forwarded_methods`'s own pre-existing `FnName::Method` filter
    // one step earlier rather than lowering a signature nothing renders.
    let mut imported_methods: HashMap<String, Vec<FnSig>> = HashMap::new();
    for t in &info.uses {
        let Some(used) = unit_info.get(t) else {
            continue;
        };
        let used_types = bynk_check::symbols::combined_types_for_unit_info(t, unit_info);
        // #2751: only what an importer can reach — a `private` type's methods
        // (and a private method) would otherwise merge into a same-named
        // public type's forwarded const.
        for (type_name, mt) in &used.table.importable().methods {
            let entry = imported_methods.entry(type_name.clone()).or_default();
            entry.extend(lower_attached_fn_sig_ir_from_types(mt, &used_types, tys));
        }
    }
    // #1807: a hidden type's methods are forwarded like a `uses` type's,
    // resolved in its owning commons' scope.
    for (type_name, owner) in hidden_types {
        let Some(mt) = unit_info
            .get(owner)
            .and_then(|o| o.table.methods.get(type_name))
        else {
            continue;
        };
        let owner_types = bynk_check::symbols::combined_types_for_unit_info(owner, unit_info);
        imported_methods
            .entry(type_name.clone())
            .or_default()
            .extend(lower_attached_fn_sig_ir_from_types(mt, &owner_types, tys));
    }
    for decls in imported_methods.values_mut() {
        decls.sort_by_key(|sig| sig.name.clone());
    }
    let mut imported_decl_paths: HashMap<String, HashMap<String, PathBuf>> = HashMap::new();
    for t in &info.uses {
        if let Some(target_info) = unit_info.get(t) {
            let target_index = &target_info.file_index;
            let mut paths: HashMap<String, PathBuf> = HashMap::new();
            for (n, p) in &target_index.types {
                paths.insert(n.clone(), p.clone());
            }
            for (n, p) in &target_index.fns {
                paths.insert(n.clone(), p.clone());
            }
            imported_decl_paths.insert(t.clone(), paths);
        }
    }
    for t in &info.consumes {
        if let Some(target_info) = unit_info.get(t) {
            let target_index = &target_info.file_index;
            let mut paths: HashMap<String, PathBuf> = HashMap::new();
            // Only expose exported names — the emitter needs to know
            // which file declares them so it can render the import.
            let exports_for_target = &target_info.exports;
            for n in exports_for_target.keys() {
                if let Some(p) = target_index.types.get(n) {
                    paths.insert(n.clone(), p.clone());
                }
            }
            // #2887: an aliased export is imported by its original name, from
            // the file that declares it, but referred to by its alias.
            for (alias, a) in &info.table.consumed_type_aliases {
                if a.target == *t
                    && let Some(p) = target_index.types.get(&a.original)
                {
                    paths.insert(alias.clone(), p.clone());
                }
            }
            imported_decl_paths.insert(t.clone(), paths);
        }
    }
    // #1807: a hidden type is imported from its owning commons, which this
    // unit need not `uses` itself.
    for (type_name, owner) in hidden_types {
        if let Some(p) = unit_info
            .get(owner)
            .and_then(|o| o.file_index.types.get(type_name))
        {
            imported_decl_paths
                .entry(owner.clone())
                .or_default()
                .insert(type_name.clone(), p.clone());
        }
    }

    let exports_for_consumed = info
        .consumes
        .iter()
        .map(|t| {
            let mut exports = unit_info
                .get(t)
                .map(|i| i.exports.clone())
                .unwrap_or_default();
            // #2887: a renamed export is exported under its alias, here.
            for (alias, a) in &info.table.consumed_type_aliases {
                if a.target == *t
                    && let Some(vis) = exports.get(&a.original).copied()
                {
                    exports.insert(alias.clone(), vis);
                }
            }
            (t.clone(), exports)
        })
        .collect();

    // In workers mode, rewrite imported_decl_paths for consumed
    // contexts to point at the consumed Worker's handlers.ts.
    let mut imported_decl_paths_emit = imported_decl_paths.clone();
    if matches!(target, BuildTarget::Workers) {
        for (unit, decls) in imported_decl_paths.iter() {
            let target_kind = unit_info.get(unit).map(|i| i.kind);
            if target_kind == Some(UnitKind::Context) {
                let handlers_path = layout.handlers_source_path(unit);
                let mut rewritten = HashMap::new();
                for n in decls.keys() {
                    rewritten.insert(n.clone(), handlers_path.clone());
                }
                imported_decl_paths_emit.insert(unit.clone(), rewritten);
            }
        }
    }

    let mut file_decl_index = info.file_index.clone();
    if split_worker {
        let module = |p: &mut PathBuf| *p = layout.file_source_path(name, p);
        file_decl_index.types.values_mut().for_each(module);
        file_decl_index.fns.values_mut().for_each(module);
        file_decl_index.agents.values_mut().for_each(module);
        file_decl_index.capabilities.values_mut().for_each(module);
        file_decl_index
            .methods
            .values_mut()
            .flat_map(HashMap::values_mut)
            .for_each(module);
    }

    EmitUnitCtx {
        imported_methods,
        imported_decl_paths_emit,
        exports_for_consumed,
        file_decl_index,
        split_worker,
        layout: layout.clone(),
    }
}

/// #2991: whether any source file of the project mentions `Json.encodePlain`.
/// Textual on purpose: it only decides whether the generated `Option` encoders
/// carry their (inert unless raised) plain branch, so a mention in a comment
/// costs a few bytes and nothing else.
pub(crate) fn project_mentions_encode_plain(parsed: &[ParsedFile]) -> bool {
    parsed.iter().any(|p| p.source().contains("encodePlain"))
}

/// #1927: a `uses`-imported type this unit hides behind its own value is spelled as an inline
/// import type while the module is emitted (`EmitProjectCtx::type_spellings`).
fn hidden_type_spellings(
    name: &str,
    unit_info: &BTreeMap<String, UnitInfo>,
    unit_ctx: &EmitUnitCtx,
    source_path: &Path,
    import_ext: ImportExt,
) -> HashMap<String, String> {
    bynk_check::project_model::shadowed_imports(name, unit_info)
        .into_iter()
        .filter(|(_, s)| s.imported == "type")
        .map(|(type_name, s)| {
            let path = unit_ctx
                .imported_decl_paths_emit
                .get(&s.unit)
                .and_then(|m| m.get(&type_name))
                .cloned()
                .unwrap_or_else(|| EmitProjectCtx::commons_path(&s.unit));
            let spec =
                emitter::cross_commons_import_specifier_for_path(source_path, &path, import_ext);
            let spelling = format!("import(\"{spec}\").{type_name}");
            (type_name, spelling)
        })
        .collect()
}

/// 1A.34: emit the unit's module. A checker fact the emitter needed and did not find is a
/// compiler fault, reported as a `bynk.internal.missing_checker_fact` diagnostic at the `.bynk`
/// span of each (see [`report_internal_faults`]), and the unit's output is dropped (`None`).
fn emit_unit_program(
    program: &checker::CheckedProgram,
    emit_ctx: &EmitProjectCtx,
    pf: &ParsedFile,
    source_name: &str,
    errors: &mut ErrorSink,
) -> Option<(bynk_ts::TsProgram, Option<String>)> {
    match emitter::emit_project(program, emit_ctx, pf.source(), source_name) {
        Ok(emitted) => Some(emitted),
        Err(faults) => {
            report_internal_faults(errors, &pf.identity_path(), faults);
            None
        }
    }
}

/// 1A.34: turn the checker facts the emitter needed and did not find into
/// `bynk.internal.missing_checker_fact` diagnostics, one per fault, each at the
/// `.bynk` span of the construct being lowered and attributed to `file`.
fn report_internal_faults(
    errors: &mut ErrorSink,
    file: &Path,
    faults: Vec<emitter::InternalFault>,
) {
    errors.extend_for(
        Some(file),
        faults.into_iter().map(|f| f.into_compile_error()),
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_unit(
    name: &str,
    kind: UnitKind,
    pf: &ParsedFile,
    unit_ctx: &EmitUnitCtx,
    history_target_agents: &HashSet<String>,
    asset_served: &BTreeMap<String, String>,
    embed_text: &BTreeMap<String, String>,
    unit_info: &BTreeMap<String, UnitInfo>,
    imported_from: &HashMap<String, String>,
    imported_from_kind: &HashMap<String, UnitKind>,
    owning_context_for_emit: &Option<String>,
    cross_context_for_file: &resolver::CrossContextInfo,
    program: &checker::CheckedProgram,
    target: BuildTarget,
    import_ext: ImportExt,
    contracts: bool,
    trace_requests: bool,
    inspect_agents: bool,
    agent_deps_plan: Option<&AgentDepsPlan>,
    compiled: &mut Vec<StagedFile>,
    errors: &mut ErrorSink,
    schema_effective_versions: &SchemaVersions,
    plain_encode_in_project: bool,
) {
    let typed = program.program();
    // Build the emitter context.
    let info = &unit_info[name];
    let mut cross_context_info = cross_context_for_file.clone();
    // #1932: the names the consumer gives a consumed context's unexported
    // types' codecs, for the call sites, from the same function its codec
    // generator uses.
    for c in cross_context_info.consumed_contexts.clone() {
        if let Some(types) = cross_context_info.consumed_types.get(&c) {
            let renames = emitter::unexported_type_renames(
                &c,
                types,
                unit_ctx.exports_for_consumed.get(&c),
                &typed.types,
                imported_from_kind,
            );
            cross_context_info.consumed_type_renames.insert(c, renames);
        }
    }

    // v0.8: in workers mode, a context's *output* lands under
    // workers/<dashes>/handlers.ts. Use that path as the synthetic
    // source_path so the emitter's depth/relative-path logic and
    // imported_decl_paths produce correct relative imports.
    let workers_mode = matches!(target, BuildTarget::Workers);
    let emit_source_path = if unit_ctx.split_worker {
        unit_ctx.layout.file_source_path(name, &pf.source_path())
    } else if workers_mode && kind == UnitKind::Context {
        unit_ctx.layout.handlers_source_path(name)
    } else {
        pf.source_path()
    };

    // message-bundles slice 1 (#859): a `messages` block's generated `render`
    // needs `bynk.locale`'s own `render` in scope for its fallback rung, but
    // under a private alias — this file's own `export function render` would
    // otherwise collide with a plain `import { render }`. Injected as a hand-
    // written extra import line rather than through the usual reference-
    // collection path (`collect_external_references`/`record_name_ref`),
    // which has no per-name aliasing of its own and would emit a colliding,
    // unaliased `render`.
    // #1820: in a split context, only a file declaring one of the plan's
    // agents builds its deps, so only that file imports what they need.
    let mut extra_imports: Vec<ExtraImport> = agent_deps_plan
        .filter(|p| {
            !unit_ctx.split_worker
                || p.exprs.keys().any(|agent| {
                    unit_ctx.file_decl_index.agents.get(agent) == Some(&emit_source_path)
                })
        })
        .map(|p| p.imports.clone())
        .unwrap_or_default();
    if pf.declares_messages() {
        let render_path = unit_ctx
            .imported_decl_paths_emit
            .get("bynk.locale")
            .and_then(|m| m.get("render"))
            .cloned()
            .unwrap_or_else(|| EmitProjectCtx::commons_path("bynk.locale"));
        let import = emitter::cross_commons_import_specifier_for_path(
            &emit_source_path,
            &render_path,
            import_ext,
        );
        // #1697: the generated table and `render` name the `bynk.locale.types`
        // types under private aliases too, so a user's own `Message` or
        // `LocaleTag` in this unit (local shadows `uses`) can't capture them.
        let types_path = unit_ctx
            .imported_decl_paths_emit
            .get("bynk.locale.types")
            .and_then(|m| m.get("LocaleTag"))
            .cloned()
            .unwrap_or_else(|| EmitProjectCtx::commons_path("bynk.locale.types"));
        let types_import = emitter::cross_commons_import_specifier_for_path(
            &emit_source_path,
            &types_path,
            import_ext,
        );
        extra_imports.push(ExtraImport::Named {
            names: vec![
                "render as __bynkLocaleRender".to_string(),
                "renderArg as __bynkRenderArg".to_string(),
            ],
            from: import,
            type_only: false,
        });
        extra_imports.push(ExtraImport::Named {
            names: vec![
                "LocaleTag as __bynkLocaleTag".to_string(),
                "Message as __bynkMessage".to_string(),
                "MessageArg as __bynkMessageArg".to_string(),
            ],
            from: types_import,
            type_only: true,
        });
    }

    let agent_called_contexts: Vec<String> =
        emitter::agent_cross_context_targets(&info.table, |c| cross_context_info.resolve_prefix(c))
            .into_iter()
            .collect();
    let mut emit_ctx = EmitProjectCtx {
        source_path: emit_source_path,
        identity_path: pf.identity_path().to_string_lossy().into_owned(),
        commons_name: name.to_string(),
        file_decl_index: unit_ctx.file_decl_index.clone(),
        imported_from: imported_from.clone(),
        imported_from_kind: imported_from_kind.clone(),
        imported_decl_paths: unit_ctx.imported_decl_paths_emit.clone(),
        consumed_type_aliases: info
            .table
            .consumed_type_aliases
            .iter()
            .map(|(alias, a)| (alias.clone(), a.original.clone()))
            .collect(),
        unit_kind: kind,
        owning_context: owning_context_for_emit.clone(),
        exports_for_consumed: unit_ctx.exports_for_consumed.clone(),
        imported_methods: unit_ctx.imported_methods.clone(),
        cross_context: cross_context_info,
        target,
        local_agents: info.table.agents.keys().cloned().collect(),
        agent_given_deps: agent_deps_plan.map(|p| p.exprs.clone()).unwrap_or_default(),
        agent_given_deps_names: agent_deps_plan.map(|p| p.names.clone()).unwrap_or_default(),
        extra_imports,
        agent_method_givens: info
            .table
            .agents
            .iter()
            .map(|(agent, a)| {
                (
                    agent.clone(),
                    a.handlers
                        .iter()
                        .filter_map(|h| {
                            h.method_name
                                .as_ref()
                                .map(|m| (m.name.clone(), lower_handler_given_ir(h)))
                        })
                        .collect(),
                )
            })
            .collect(),
        agent_called_contexts,
        // v0.47: the context's actors (merged across files), so the Bearer
        // verification seam resolves even when the actor and handler are in
        // different files of the same context.
        actors: info.table.actors.clone(),
        // Events slice 3b (#978), verified by slice 3c (#980): resolved once
        // per unit, merged across files the same way `actors` is above. The
        // registry's reconciled version wins when present (it is the
        // auto-bumped or `@schema(N)`-verified truth); `decl.schema_version()`
        // is the fallback for when the registry is off, matching every
        // event's pre-3c behaviour exactly.
        agent_schema_versions: info
            .table
            .agents
            .keys()
            .filter_map(|agent_name| {
                schema_effective_versions
                    .agents
                    .get(&format!("{name}.{agent_name}"))
                    .map(|v| (agent_name.clone(), v.clone()))
            })
            .collect(),
        event_schema_versions: info
            .table
            .events
            .iter()
            .map(|(event_name, decl)| {
                let key = format!("{name}.{event_name}");
                let version = schema_effective_versions
                    .events
                    .get(&key)
                    .copied()
                    .unwrap_or_else(|| decl.schema_version());
                (event_name.clone(), version)
            })
            .collect(),
        consumed_adapters: info
            .consumes
            .iter()
            .filter(|t| unit_info.get(*t).map(|i| i.kind) == Some(UnitKind::Adapter))
            .cloned()
            .collect(),
        consumed_types_with_methods: info
            .consumes
            .iter()
            .filter_map(|t| unit_info.get(t))
            .filter(|i| i.kind == UnitKind::Context)
            .flat_map(|i| {
                i.table
                    .methods
                    .iter()
                    .filter(|(_, m)| !m.instance.is_empty() || !m.statics.is_empty())
                    .map(|(n, _)| n.clone())
            })
            .collect(),
        import_ext,
        contracts,
        // #3580: the file's source and project-relative path name each traced call
        // site; `None` (every build but `bynk dev --trace-events`) records nothing.
        trace: trace_requests.then(|| bynk_emit::emit_model::TraceLoc {
            source: std::sync::Arc::from(pf.source()),
            rel_path: pf.identity_path().to_string_lossy().into_owned(),
        }),
        inspect_agents,
        history_target_agents: history_target_agents.clone(),
        asset_served: asset_served.clone(),
        embed_text: embed_text.clone(),
        runtime_use: Default::default(),
        worker_layout: unit_ctx.layout.clone(),
        // #2620: the unit's own source, so a fault lowering names `path:line:col`.
        unit_source: Some(UnitSource::new(
            pf.source(),
            &pf.identity_path().to_string_lossy(),
        )),
        // #1927: filled below, once the import specifiers can be built from
        // `source_path`.
        type_spellings: HashMap::new(),
    };
    // #2858: any plain-JSON route in the project (project-wide, not per Worker;
    // the branch is inert unless `withPlainJson` sets the flag) switches the generated `Option`
    // encoders to carry the plain branch (a response type can live in a commons
    // another unit's route serialises through).
    // #2991: so does any `Json.encodePlain` call in the project's sources (a
    // textual over-approximation: the branch is inert unless a call raises the flag).
    emit_ctx.runtime_use.set_plain_json(
        plain_encode_in_project
            || unit_info
                .values()
                .any(|i| emitter::table_has_plain_json(&i.table)),
    );
    // v0.72: the map's `source` is the absolute path the compiler read the file
    // from, so an editor breakpoint set on the real `.bynk` resolves to the same
    // path the debugger loads (project-relative would resolve against the output
    // `.ts`'s directory — the wrong place). Synthetic units fall back to relative.
    let source_name = pf.map_source_name();
    // #1486: `emit_project` now returns the real `TsProgram` (printed once
    // at the write boundary via `Document::text`) plus the source map
    // `bynk_ts::print` already computed while building it — no more
    // `Verbatim`-wrapping a pre-rendered string.
    emit_ctx.type_spellings =
        hidden_type_spellings(name, unit_info, unit_ctx, &emit_ctx.source_path, import_ext);
    let Some((program, source_map)) =
        emit_unit_program(program, &emit_ctx, pf, &source_name, errors)
    else {
        return;
    };
    // Slice 3: the handler-label sidecar for this unit (ADR 0105) — names stack
    // frames by their Bynk operation. `None` for units with no handlers.
    let debug_metadata = emitter::collect_handler_labels(typed);
    let output_path = if unit_ctx.split_worker {
        ts_output_path(&unit_ctx.layout.file_source_path(name, &pf.source_path()))
    } else if workers_mode && kind == UnitKind::Context {
        unit_ctx.layout.handlers_output_path(name)
    } else {
        ts_output_path(&pf.source_path())
    };
    compiled.push(StagedFile {
        output_path,
        document: Document::Ts(program),
        source_map,
        debug_metadata,
    });
}

/// Phase 8d/8e: resolve + check (and, in build mode, emit) every source file in
/// one production unit. The per-file `continue`s stay internal to this loop, so
/// a file that fails resolution/checking is skipped without abandoning the unit.
///
/// P4.1 (#1115): the resolve+check+context-checks core — identical for both
/// `Mode`s except for the four `record_analyse_types` call sites — moved to
/// `bynk_check::check_pipeline::check_file_core` (see that module's own doc
/// comment), used by both this function and the new `bynk-check`-native
/// analysis entry point. This function now owns only: the per-unit
/// `EmitUnitCtx`/`prepare_unit_check_ctx` prelude, the `Mode`-conditional
/// exit (`Mode::Analyse` records and stops; `Mode::Build` proceeds to
/// `certify`+`emit_unit`) and the emission tail itself.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
fn check_unit_files(
    name: &str,
    kind: UnitKind,
    // #1710: each unit's declarations recovery skipped (`phase_parse`).
    broken: &project_model::BrokenDeclNames,
    indices: &[usize],
    parsed: &[ParsedFile],
    unit_info: &BTreeMap<String, UnitInfo>,
    // #3229: the project's cross-context views, built once across all units.
    views: &check_pipeline::CrossContextViewsCache,
    combined_types: &HashMap<String, Arc<TypeDecl>>,
    combined_fns: &HashMap<String, Arc<FnDecl>>,
    combined_methods: &HashMap<String, ResolverMethodTable>,
    local_names: &HashSet<String>,
    local_methods_for_type: &HashMap<String, Vec<FnDecl>>,
    consumed_types: &HashMap<String, ConsumedType>,
    imported_from: &HashMap<String, String>,
    imported_from_kind: &HashMap<String, UnitKind>,
    // #1807: the types this unit reaches through imported declarations
    // (#1824: and the shadows among them).
    reached_types: &project_model::ReachedTypes,
    owning_context_for_emit: &Option<String>,
    target: BuildTarget,
    import_ext: ImportExt,
    contracts: bool,
    trace_requests: bool,
    inspect_agents: bool,
    agent_deps_plan: Option<&AgentDepsPlan>,
    history_target_agents: &HashSet<String>,
    asset_served: &BTreeMap<String, String>,
    embed_text: &BTreeMap<String, String>,
    mode: Mode,
    policy: &Arc<bynk_check::stability::StabilityPolicy>,
    // #3070: the schemas the D1 migrations build, which literal D1 SQL is
    // prepared against.
    d1_sql: &Option<Arc<bynk_check::d1_sql::D1Sql>>,
    errors: &mut ErrorSink,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    exprs: &mut ExprTypeSink,
    requirements: &mut RequirementSink,
    compiled: &mut Vec<StagedFile>,
    // Events track, slice 3c (#980): each locally-declared event's *effective*
    // schema version, keyed `<unit>.<EventName>` — the schema registry's
    // reconciled value when the registry is on, empty (so every lookup falls
    // through to `EventDecl::schema_version()`) when it is off.
    schema_effective_versions: &SchemaVersions,
    tys: &Arc<Types>,
    // #1187's slice 6 plumbing — this unit's own accumulator; merged into
    // per file below, from each file's own certified `CheckedProgram`
    // (`RunChecks::Checked::unit_callees`'s own doc comment has the full
    // grounding for why this exists).
    unit_callees: &mut HashMap<ExprId, bynk_check::checker::Callee>,
    // P6.x (#1232): this unit's own declared event-subscriber service
    // shapes, keyed by service name — see `EventSubscriberShape`'s own doc
    // comment. Populated the same way as `unit_callees` above: merged from
    // each file's own `CheckedProgram` before it is dropped at the end of
    // this loop's iteration.
    event_subscriber_shapes: &mut HashMap<String, EventSubscriberShape>,
    // #2719: the build's needs-context set so far; this unit extends it.
    exec_plan: &mut bynk_check::exec_plan::ExecPlan,
    layout: &WorkerLayout,
) {
    // Emit-prologue tables invariant across every file of this unit — built
    // once here rather than once per file (see `EmitUnitCtx`).
    let split_worker =
        target == BuildTarget::Workers && kind == UnitKind::Context && indices.len() > 1;
    let unit_ctx = build_emit_unit_ctx(
        name,
        unit_info,
        &reached_types.hidden,
        target,
        tys,
        split_worker,
        layout,
    );
    let first_emitted = compiled.len();
    let mut check_ctx = prepare_unit_check_ctx(
        name,
        kind,
        broken,
        unit_info,
        views,
        combined_types,
        imported_from_kind,
        reached_types,
        policy,
    );
    check_ctx.d1_sql = d1_sql.clone();

    // #2719: check every file of the unit before emitting any. Whether a
    // function takes the hidden execution-context parameter depends on the
    // functions it calls, which may sit in a sibling file, so the unit's
    // needs-context set is settled between the two.
    let mut checked: Vec<(
        usize,
        bynk_check::checker::TypedCommons,
        bynk_check::resolver::CrossContextInfo,
    )> = Vec::new();
    for &i in indices {
        let pf = &parsed[i];
        let Some(check_pipeline::FileCheckResult {
            typed,
            cross_context: cross_context_for_file,
        }) = check_pipeline::check_file_core(
            name,
            kind,
            pf,
            unit_info,
            combined_types,
            combined_fns,
            combined_methods,
            local_names,
            local_methods_for_type,
            consumed_types,
            imported_from,
            &check_ctx,
            errors,
            refs,
            hints,
            locals,
            exprs,
            requirements,
            tys,
        )
        else {
            continue;
        };

        // Analyse mode stops at checked: emission is build-only. Capture the
        // file's expression types on the way out (Ok path only — this point is
        // past every per-file error exit inside `check_file_core`), for
        // `.`-member completion.
        if mode == Mode::Analyse {
            check_pipeline::record_analyse_types(
                exprs,
                &pf.identity_path(),
                pf.is_synthetic(),
                &typed.expr_types,
            );
        }
        checked.push((i, typed, cross_context_for_file));
    }
    let plan_errors = {
        let files: Vec<&bynk_check::checker::TypedCommons> =
            checked.iter().map(|(_, t, _)| t).collect();
        exec_plan.extend(&files)
    };
    let plan_failed = !plan_errors.is_empty();
    errors.extend_attributed(plan_errors);
    if mode == Mode::Analyse {
        // #3142: an analysis keeps just the D1 SQL sites, for `bynk test
        // --mutate`; emission below is build-only.
        for (_, typed, _) in &checked {
            unit_callees.extend(typed.callees.iter().filter_map(|(id, c)| {
                let keep = match c {
                    bynk_check::checker::Callee::D1Sql(_) => true,
                    bynk_check::checker::Callee::Const(_, site) => site.is_some(),
                    // #3355: an inbound email handler's `Forward(...)`.
                    bynk_check::checker::Callee::Ctor { sum, tag } => {
                        tag == "Forward" && sum.name.name == "EmailVerdict"
                    }
                    // `Events.emit`: whether the bundle has a composition
                    // root (`project_model::phase_bundle_composition_unprovided`),
                    // which `bynk check` decides as the build does.
                    c => project_model::is_events_emit(c),
                };
                keep.then(|| (*id, c.clone()))
            }));
        }
    }
    if plan_failed || mode == Mode::Analyse {
        return;
    }
    let exec_snapshot = Arc::new(exec_plan.clone());
    for (i, mut typed, cross_context_for_file) in checked {
        let pf = &parsed[i];
        typed.exec_plan = Arc::clone(&exec_snapshot);
        let program = checker::certify(typed, Vec::new()).unwrap_or_else(|_| {
            panic!("bynk internal error: unit already passed every per-unit gate above")
        });
        // #1187's slice 6 plumbing: merge this file's own resolved `Callee`
        // classification into the unit's accumulator before `program` (and
        // the `TypedCommons` it wraps) is dropped at the end of this
        // iteration — the only point in this pipeline that ever holds it.
        // Filtered to the two variants either reader actually matches on
        // (review of #1202): every other `Callee` variant would otherwise
        // sit retained project-wide, for the rest of the build, to answer
        // the questions its readers ask (since W30, whether a body calls
        // `Idempotency.claim`, which declares the implicit claims database) — a real `String`/`Arc` cost on a large
        // project with nothing reading the rest yet. Widen this filter (or
        // drop it) the moment a future reader needs a different variant.
        unit_callees.extend(program.program().callees.iter().filter_map(|(id, c)| {
            let keep = match c {
                bynk_check::checker::Callee::Cross { .. } => true,
                bynk_check::checker::Callee::Capability {
                    op, first_party, ..
                } => {
                    (*first_party == Some(FirstPartyCap::Events) && op == "emit")
                        || *first_party == Some(FirstPartyCap::Idempotency)
                }
                // W30 (#2840): an `Idempotency` call through a consumed context's qualified name.
                bynk_check::checker::Callee::CrossCap { first_party, .. } => {
                    *first_party == Some(FirstPartyCap::Idempotency)
                }
                _ => false,
            };
            keep.then(|| (*id, c.clone()))
        }));
        // P6.23 (review of #1254): captures this file's own
        // event-subscriber service shapes the same way as before —
        // before `program` is dropped at the end of this iteration, same
        // insertion point as `unit_callees.extend` above. P6.47 (#1137):
        // the walk itself (including the `ServiceProtocol::Events`
        // pre-filter guarding `lower_service_item_ir` — see that
        // function's own doc comment for why the guard stays) relocated to
        // `bynk_model::lower_event_subscriber_shapes_ir`, an excluded file.
        event_subscriber_shapes.extend(bynk_model::lower_event_subscriber_shapes_ir(&program));
        emit_unit(
            name,
            kind,
            pf,
            &unit_ctx,
            history_target_agents,
            asset_served,
            embed_text,
            unit_info,
            imported_from,
            imported_from_kind,
            owning_context_for_emit,
            &cross_context_for_file,
            &program,
            target,
            import_ext,
            contracts,
            trace_requests,
            inspect_agents,
            agent_deps_plan,
            compiled,
            errors,
            schema_effective_versions,
            project_mentions_encode_plain(parsed),
        );
    }
    if mode == Mode::Build && split_worker {
        let barrel = emitter::commons_barrel::emit_worker_handlers_barrel(
            name,
            indices,
            parsed,
            &compiled[first_emitted..],
            import_ext,
            layout,
        );
        compiled.push(barrel);
    }
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
enum RunChecks {
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

#[allow(clippy::too_many_arguments)]
fn run_checks(
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
    // #1821 (ADR 0147 D3): check and emit `suite`s. Off for a deployable build.
    tests: bool,
    // Events track, slice 3c (#980): `On` turns on `bynk.schema.lock`
    // reconciliation, with its pre-read content; `Off` (every in-memory/
    // test/LSP caller) skips it entirely. See `CompileOptions::schema_registry`
    // and `SchemaLock`. #1078: no disk access here — the caller pre-reads.
    schema_registry: &SchemaLock,
    // #2138: a remote test build (`CompileOptions::remote_tests`); `None`
    // for every other build.
    remote_tests: Option<&RemoteTests>,
    // #1085 review: only for `schema_registry::parse`'s corruption message —
    // naming *which* project's lock file is corrupt, now that #1078 made
    // `bynk-emit` disk-free (and so path-blind) for this file.
    project_root: &Path,
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
    project_model::normalize_service_defaults(&mut parsed);
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

    // -- 6. Name-conflict detection for uses imports (commons-only check). --
    project_model::phase_uses_name_conflicts(
        &unit_uses,
        &unit_tables,
        &parsed,
        &groups,
        &mut errors,
    );
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
    let embed_text = embed_files
        .map(|folder| bynk_check::embeds::read_declared_embeds(&parsed, folder, &mut errors))
        .unwrap_or_default();
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
    let mut schema_effective_versions = SchemaVersions::default();
    let mut schema_registry_doc: Option<schema_registry::SchemaRegistry> = None;
    if let SchemaLock::On { existing } = schema_registry {
        match bynk_check::schema_registry::parse_or_diagnose(existing.as_deref(), project_root) {
            Ok(existing_reg) => {
                let mut schema_errors: Vec<CompileError> = Vec::new();
                let (mut updated, effective) = bynk_check::schema_registry::reconcile(
                    &existing_reg,
                    &unit_tables,
                    &mut schema_errors,
                );
                errors.extend_for(None, schema_errors);
                // State-migrations S0 (#2204): agent state shapes, in the same
                // document. Attributed by span, so a refused change lands on
                // the agent's own file.
                let mut agent_errors: Vec<CompileError> = Vec::new();
                schema_effective_versions.agents = bynk_check::schema_registry::reconcile_agents(
                    &existing_reg,
                    &mut updated,
                    &unit_tables,
                    &unit_uses,
                    &mut agent_errors,
                );
                errors.extend_attributed_by_span(&parsed, agent_errors);
                schema_effective_versions.events = effective;
                schema_registry_doc = Some(updated);
            }
            Err(err) => {
                errors.push_for(None, err);
            }
        }
    }

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
    errors.extend_attributed_by_span(&parsed, test_errors);

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
        &unit_callees,
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

/// `package.json`: always `"type": "module"` (#2148), so the emitted tsconfig's
/// `module: NodeNext` checks and builds the output as the ES modules it is, under
/// Node's ESM rules; plus the adapter-declared (v0.17) and first-party (ADR 0799
/// C) npm dependencies when there are any.
fn package_json_file(npm_deps: &std::collections::BTreeMap<String, String>) -> StagedFile {
    StagedFile {
        output_path: PathBuf::from("package.json"),
        document: Document::Json(render_package_json(npm_deps)),
        source_map: None,
        debug_metadata: None,
    }
}

/// ADR 0799 C: a runtime module that imports a first-party npm package brings it,
/// at its exact pin, into `package.json`.
fn runtime_with_npm(
    reached: &BTreeSet<(String, String)>,
    mut npm_deps: std::collections::BTreeMap<String, String>,
) -> (
    Vec<(PathBuf, String)>,
    std::collections::BTreeMap<String, String>,
) {
    let runtime_files = emitter::runtime_files::closure_files(reached);
    // A module's name is its path under the runtime directory without the extension, so a module
    // in a subdirectory (`testing/pglite`) keeps it (1B.3b, #4047).
    let modules: Vec<String> = runtime_files
        .iter()
        .filter_map(|(path, _)| {
            let rel = path.strip_prefix(emitter::RUNTIME_DIR).ok()?;
            Some(rel.with_extension("").to_string_lossy().replace('\\', "/"))
        })
        .collect();
    for dep in firstparty::first_party_npm_for(modules.iter().map(String::as_str)) {
        npm_deps.insert(dep.package.to_string(), dep.version.to_string());
    }
    (runtime_files, npm_deps)
}

/// 1C.3a (#4040): the model a Workers build's files were written from, for the
/// commands that act on its bindings; none for a Bundle build.
fn workers_build(
    target: BuildTarget,
    bindings: emitter::WorkerBindings,
    layout: &WorkerLayout,
) -> Option<WorkersBuild> {
    matches!(target, BuildTarget::Workers).then(|| WorkersBuild {
        bindings,
        layout: layout.clone(),
    })
}

/// What `run_checks` derives once for the build (1C.1a, Decision C of #4031):
/// the composition plan, and every Worker's bindings decided from it. The
/// duplicate check, the integration harness and the Workers build all read
/// them, so the check sees exactly the bindings the build writes.
struct ComposedBuild {
    plan: emitter::ComposePlan,
    bindings: emitter::WorkerBindings,
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

/// v0.6 §6.3: emit a composition root when the project has at least one
/// context that consumes another context's service surface. The compose file
/// imports each context, instantiates its providers, assembles its deps
/// (capabilities + cross-context surfaces), and exports the top-level service
/// surface.
#[allow(clippy::too_many_arguments)]
fn emit_bundle_compose_root(
    compiled: &mut Vec<StagedFile>,
    plan: &emitter::ComposePlan,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_tables: &HashMap<String, UnitTable>,
    adapter_bindings: &HashMap<String, AdapterBinding>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    thread_env: bool,
) {
    if let Some(compose_program) = emitter::emit_composition_root(
        plan,
        unit_consumes,
        unit_consumes_aliases,
        unit_tables,
        adapter_bindings,
        unit_flattened,
        thread_env,
    ) {
        compiled.push(StagedFile {
            output_path: PathBuf::from("compose.ts"),
            document: Document::Ts(compose_program),
            source_map: None,
            debug_metadata: None,
        });
    }
}

/// Build-success tail (region 3): emit the composition/worker/runtime files
/// and assemble the final `ProjectOutput`. Reached only on build mode with a
/// clean error sink. Moved verbatim from the old pipeline; only the locals it
/// reads are now bound from the `Checked` variant.
#[allow(clippy::too_many_arguments)]
fn build_output(
    parsed: Vec<ParsedFile>,
    mut compiled: Vec<StagedFile>,
    mut runnable_tests: Vec<RunnableTest>,
    integration_outputs: Vec<StagedFile>,
    integration_runnables: Vec<RunnableTest>,
    groups: BTreeMap<String, Vec<usize>>,
    kinds: BTreeMap<String, UnitKind>,
    unit_consumes: HashMap<String, Vec<String>>,
    unit_consumes_aliases: HashMap<String, HashMap<String, String>>,
    unit_tables: HashMap<String, UnitTable>,
    unit_consumed_context_types: &HashMap<String, HashSet<String>>,
    unit_callees: HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
    // v0.177 (#643): needed to build each context's *own* combined type table,
    // so its contract hashes are computed from the same namespace a caller sees.
    unit_uses: HashMap<String, Vec<String>>,
    unit_flattened: HashMap<String, HashMap<String, String>>,
    adapter_bindings: HashMap<String, AdapterBinding>,
    npm_deps: std::collections::BTreeMap<String, String>,
    target: BuildTarget,
    import_ext: ImportExt,
    // #1890: the merged `wrangler.toml` flag list (`finish_build` merges it).
    compatibility_flags: &[String],
    workers_dev: Option<bool>,
    // #3477: the path back to the project root from a Worker's output
    // directory, for a container's build paths.
    container_root: &str,
    // #1883, slice 1b: the declared asset files' bytes; each Worker ships
    // the files its `uses` closure declares.
    asset_files: Option<&BTreeMap<String, Vec<u8>>>,
    // State-migrations S4 (#2259): each agent class's lock version, keyed
    // `<unit>.<Agent>`, for its Worker's `/_bynk/migrate/` route.
    agent_versions: &HashMap<String, bynk_check::schema_registry::AgentVersions>,
    // #2554: the older contracts each callee accepts, from `bynk.contracts.lock`.
    contracts_lock: &bynk_project::contracts_lock::ContractsLock,
    // P4b (#2608): each `D1` database's migrations.
    d1_migrations: &bynk_check::d1_migrations::D1Migrations,
    // #3562: where each context's Worker is.
    layout: &WorkerLayout,
    // #3580: wrap each Worker's `fetch` in the request recorder.
    trace_requests: bool,
    inspect_agents: bool,
    // 1C.1a: the plan and binding model `run_checks` derived.
    composed: ComposedBuild,
) -> ProjectOutput {
    // #2554: the lock's warnings, returned on `ProjectOutput::warnings` for
    // `finish_build` to merge with the checker's.
    let mut lock_warnings: Vec<AttributedError> = Vec::new();
    // #1883, slice 1b: each declared asset's served path, and the files the
    // Workers below stage into their assets directories (empty for a Bundle
    // build: only a Worker serves files).
    let asset_served = asset_files
        .map(|files| bynk_check::assets::declared_served_paths(&parsed, files))
        .unwrap_or_default();
    let mut staged_assets: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();

    // #1655 (runtime-semantics track S6): a `--target workers` build writes the
    // workers layout (`workers/<ctx>/…`), but a *unit* test module imports the
    // bundle layout (`./../<ctx>.js`), so `tsc` over the output failed
    // (TS2307). Those modules had no consumer either: `bynk test` compiles the
    // bundle target for them and overlays a workers compile *excluding*
    // `tests/`. A workers build therefore drops the unit modules. Integration
    // modules stand their participants up as real Workers and import this
    // layout, so they stay, and `tests/main.ts` runs just them.
    let workers = target == BuildTarget::Workers;
    if workers {
        compiled.retain(|f| !f.output_path.starts_with("tests"));
    }
    compiled.extend(integration_outputs);
    // #2695: the entry handlers of the project's own contexts (first-party
    // injected units are the toolchain's, not the user's), described from the
    // checked symbol tables before they are moved into emission.
    let entry_handlers: Vec<bynk_model::entry::EntryHandler> = groups
        .iter()
        .filter(|(unit, idxs)| {
            kinds.get(*unit) == Some(&UnitKind::Context)
                && idxs.iter().any(|i| !parsed[*i].is_synthetic())
        })
        .flat_map(|(unit, _)| bynk_model::entry::entry_handlers(unit, &unit_tables, &unit_uses))
        .collect();
    // The integration runnables follow the unit ones.
    let unit_count = runnable_tests.len();
    runnable_tests.extend(integration_runnables);

    // v0.67: the discovery manifest — built from the combined runnable set before
    // anything consumes it, so `--no-run --format json` lists suites/cases without
    // running. Ordered by the runner's sort key to match a run's suite order.
    // On `workers` it lists only what this build writes and `tests/main.ts` runs
    // (#1655, review of #1695): the integration suites. `bynk test` reads the
    // manifest from its bundle compile, so the unit suites are still listed
    // there.
    let discovered = if workers {
        discovery_manifest(&runnable_tests[unit_count..])
    } else {
        discovery_manifest(&runnable_tests)
    };

    // v0.16: emit the combined top-level test runner once both passes are done,
    // so `tests/main.ts` aggregates unit and integration suites together (on
    // `workers`, the integration suites only: #1655, above).
    let main_tests = if workers {
        &runnable_tests[unit_count..]
    } else {
        &runnable_tests[..]
    };
    if !main_tests.is_empty() {
        let main_program = emit_test_main(main_tests, import_ext, pglite_on(d1_migrations));
        compiled.push(StagedFile {
            output_path: PathBuf::from("tests/main.ts"),
            document: Document::Ts(main_program),
            source_map: None,
            debug_metadata: None,
        });
    }

    // v0.19 (decision 0025): does any context's in-process closure reach a
    // platform-native unit? Drives env threading (bundle) and the per-Worker
    // Env/`wrangler.toml` resource derivation (workers).
    let context_native = worker_bindings::contexts_native(
        &kinds,
        &unit_tables,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_flattened,
    );

    // Events track, slice 0 (spine #936): project-wide "who subscribes to
    // what", and 1B.1b (#3928) what each unit composes, both decided once in
    // `run_checks` for both targets; 1C.1a (#4031) adds each Worker's bindings,
    // with a container's build path from the project root set once (#3477).
    let (compose_plan, mut bindings) = (composed.plan, composed.bindings);
    bindings.locate_containers(container_root);
    // #2137: the contexts a `system` harness stands up (test build only).
    let harness_clock_contexts =
        harness_clock_contexts(&parsed, &unit_tables, &unit_consumes, &kinds);

    match target {
        BuildTarget::Bundle => emit_bundle_compose_root(
            &mut compiled,
            &compose_plan,
            &unit_consumes,
            &unit_consumes_aliases,
            &unit_tables,
            &adapter_bindings,
            &unit_flattened,
            // D1: thread `env` through composeApp only when a native
            // resource is consumed, so native-free programs are
            // byte-identical to v0.18 output.
            !context_native.is_empty(),
        ),
        BuildTarget::Workers => {
            // v0.8 §2.3: per-Worker entry point, compose.ts, and wrangler
            // configuration. One Worker per context.
            let emit = emitter::workers_emit::WorkersEmit {
                adapter_bindings: &adapter_bindings,
                agent_versions,
                asset_files,
                asset_served: &asset_served,
                bindings: &bindings,
                compatibility_flags,
                compose_plan: &compose_plan,
                contracts_lock,
                d1_migrations,
                groups: &groups,
                harness_clock_contexts: &harness_clock_contexts,
                import_ext,
                inspect_agents,
                kinds: &kinds,
                layout,
                parsed: &parsed,
                trace_requests,
                unit_callees: &unit_callees,
                unit_consumed_context_types,
                unit_consumes: &unit_consumes,
                unit_consumes_aliases: &unit_consumes_aliases,
                unit_flattened: &unit_flattened,
                unit_tables: &unit_tables,
                unit_uses: &unit_uses,
                workers_dev,
            };
            emit.emit_all(&mut compiled, &mut lock_warnings, &mut staged_assets);
        }
    }
    let workers = workers_build(target, bindings, layout);

    // v0.17: copy each adapter binding verbatim into the output, beside the
    // adapter's emitted interface module, so compose's import resolves and the
    // `tsc` gate checks the `implements` contract.
    let mut binding_names: Vec<&String> = adapter_bindings.keys().collect();
    binding_names.sort();
    for name in binding_names {
        let b = &adapter_bindings[name];
        compiled.push(StagedFile {
            output_path: b.output_path.clone(),
            document: Document::Ts(bynk_ts::TsProgram {
                stmts: vec![bynk_ts::TsStmt::verbatim(
                    bynk_ts::VerbatimOrigin::NotYetConverted,
                    emitter::runtime_files::rewrite_runtime_imports(&b.content),
                    None,
                )],
            }),
            source_map: None,
            debug_metadata: None,
        });
    }

    // #2321: each included WebAssembly module — its bytes beside the binding
    // (a module file, never served) and the loader shim the binding imports.
    let mut staged_modules: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    for b in adapter_bindings.values() {
        for inc in &b.includes {
            staged_modules.insert(inc.output_path.clone(), inc.bytes.clone());
            compiled.push(StagedFile {
                output_path: wasm_shim_path(&inc.output_path),
                document: Document::Ts(bynk_ts::TsProgram {
                    stmts: vec![bynk_ts::TsStmt::verbatim(
                        bynk_ts::VerbatimOrigin::NotYetConverted,
                        workers_wasm_shim(&inc.output_path),
                        None,
                    )],
                }),
                source_map: None,
                debug_metadata: None,
            });
        }
    }
    if !staged_modules.is_empty() {
        compiled.push(StagedFile {
            output_path: PathBuf::from("wasm-modules.d.ts"),
            document: Document::Ts(bynk_ts::TsProgram {
                stmts: vec![bynk_ts::TsStmt::verbatim(
                    bynk_ts::VerbatimOrigin::NotYetConverted,
                    WASM_MODULES_DTS.to_string(),
                    None,
                )],
            }),
            source_map: None,
            debug_metadata: None,
        });
    }

    // Runtime (ADR 0615): exactly the modules the emitted files import, closed
    // over the modules' own imports, under `__bynk_runtime/` at the root of
    // `out/` so every file's relative specifier resolves to it.
    // `tsconfig.json` is also at the root so `tsc -p out/tsconfig.json`
    // discovers every `.ts` file in the tree.
    let mut reached = BTreeSet::new();
    for f in &compiled {
        if let Document::Ts(program) = &f.document {
            emitter::runtime_files::reached_in(program, &mut reached);
        }
    }
    let (runtime_files, npm_deps) = runtime_with_npm(&reached, npm_deps);
    for (path, source) in runtime_files {
        compiled.push(StagedFile {
            output_path: path,
            document: Document::Ts(bynk_ts::TsProgram {
                stmts: vec![bynk_ts::TsStmt::verbatim(
                    bynk_ts::VerbatimOrigin::NotYetConverted,
                    source,
                    None,
                )],
            }),
            source_map: None,
            debug_metadata: None,
        });
    }
    compiled.push(package_json_file(&npm_deps));
    compiled.push(StagedFile {
        output_path: PathBuf::from("tsconfig.json"),
        document: Document::Json(emitter::emit_tsconfig()),
        source_map: None,
        debug_metadata: None,
    });

    // `Artefacts.docs` is a `BTreeMap`, so it iterates in `output_path` order
    // naturally (Decision D, #1309) — no explicit sort needed here the way
    // the old `Vec<CompiledFile>` (sorted by `source_path`) required.
    let mut docs: BTreeMap<PathBuf, Document> = BTreeMap::new();
    // #2554: lock entries for a context the project no longer has.
    for (context, services) in contracts_lock.contexts() {
        if kinds.get(context) != Some(&UnitKind::Context) {
            for service in services.keys() {
                lock_warnings.push(bynk_project::contracts_lock::stale_warning(
                    context,
                    service,
                    "the context is no longer in the project",
                ));
            }
        }
    }

    for f in compiled {
        if let Some(sm) = &f.source_map {
            docs.insert(
                sibling_path(&f.output_path, "map"),
                Document::SourceMap(sm.clone()),
            );
        }
        if let Some(dbg) = &f.debug_metadata {
            docs.insert(
                sibling_path(&f.output_path, "bynkdbg.json"),
                Document::DebugSidecar(dbg.clone()),
            );
        }
        docs.insert(f.output_path, f.document);
    }
    ProjectOutput {
        artefacts: Artefacts {
            docs,
            assets: staged_assets,
            modules: staged_modules,
        },
        discovered,
        snapshot_files: runnable_tests
            .iter()
            .flat_map(|t| t.snapshot_files.iter().cloned())
            .collect(),
        // Populated by `compile_project` from the run's warning sink (ADR 0117);
        // until then, the contracts lock's own (#2554).
        warnings: lock_warnings,
        // Populated by `finish_build` from the same `RunChecks::Checked` this
        // whole `ProjectOutput` was built from.
        snapshots: Vec::new(),
        display_root: PathBuf::new(),
        // Likewise (#1078) — `Some` only when the registry was on.
        schema_lock: None,
        entry_handlers,
        workers,
    }
}

// P5.3 review (#1133): `resolve_consume_prefix` and `handler_cross_caps` used
// to have their own copies here, byte-identical to `bynk-check::project_model`'s
// (neither builds TypeScript, so neither had the codegen coupling that keeps
// `instantiate_provider_ts_expr` in `bynk-emit`) — deleted, every call site repointed at
// `project_model::{resolve_consume_prefix, handler_cross_caps}`.
