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
//!
//! This file is the facade; the children hold the stages of one build:
//! `options` (what a build is asked for), `entry` (the on-disk entry points),
//! `in_memory` (the editor, wasm and test entry points), `pipeline` (what is
//! checked, in what order: `run_checks`), `unit` (how one unit is checked and
//! emitted), `output` (the files a successful build stages) and `agent_deps`
//! (a Durable Object's rebuilt `given` deps). `diagnostics`, `remote_tests`,
//! `schema_registry` and `worker_bindings` are older children.

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
mod env_report;
mod in_memory;
mod options;
mod output;
mod pass_through;
mod pipeline;
mod remote_tests;
mod schema_registry;
#[cfg(test)]
mod tests;
mod unit;
mod worker_bindings;

pub use agent_deps::AgentDepsPlan;
pub use bynk_emit::emit_model::{
    Artefacts, DiscoveredCase, DiscoveredSuite, Document, ImportExt, ProjectOutput, RemoteSkip,
    RemoteTests, TestLocation, WORKER_ASSETS_DIR, is_worker_asset_path, is_worker_migration_path,
    node_wasm_shim, sibling_path, wasm_shim_path, worker_assets_dir,
};
pub use entry::{
    D1SqlRef, EmailForward, ProjectCheck, check_project, checked_callees, compile_project,
    parse_own_units,
};
pub use env_report::env_const_report;
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
