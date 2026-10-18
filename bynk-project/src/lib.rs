//! Bynk's project model (#1113, P4.0): discovery, the unit dependency graph,
//! path resolution, cross-unit consistency checks, and the schema-registry
//! document's read/write halves.
//!
//! Extracted from `bynk-emit/src/project.rs` and its `project/` submodule
//! tree so `bynk-emit` becomes a *dependent* of this crate rather than owning
//! this code directly (`design/tracks/project-model.md` §3, §6 row P4.0).
//! `bynk-check`/`bynk-syntax`/grammar/AST/emitter language surface is
//! untouched — a crate-boundary and internal-representation move, not a
//! language change.
//!
//! What did **not** move (stays in `bynk-emit`, per the slice's own scope):
//! `schema_registry::reconcile` and its helpers (`bynk_check`-coupled via
//! `UnitTable`, pending P4.1's `bynk-check` entry point); `diagnostics::Mode`,
//! `ErrorSink`, `ProjectAnalysis`, `ProjectFailure`, `ContextSequenceInfo`,
//! `ContextBoundaryInfo` (facts about how the pipeline is driven/consumed,
//! not about the project itself); `symbols` (`UnitTable`, `bynk_check`-coupled
//! via `resolver::MethodTable`); `validate`/`tests_emit` (checking and test
//! codegen, not project modelling).
//!
//! The public surface below is enumerated by hand (R10.4-style) — no blanket
//! `pub use module::*`.

pub mod assets;
pub mod consistency;
pub mod discovery;
pub mod graph;
pub mod json;
#[cfg(feature = "schema")]
pub mod manifest_schema;
pub mod parse_cache;
pub mod paths;
pub mod roots;
pub mod schema_registry;

mod diagnostics;

pub use consistency::{
    check_directory_kind_consistency, check_directory_name_consistency,
    check_group_kind_consistency, check_group_module_docs, check_path_name_alignment,
};
pub use diagnostics::{AttributedError, LabelFile};
pub use discovery::{
    ParsedFile, RecoveredSources, case_effective_tier, check_file_directory_conflicts,
    discover_bynk_files, discover_project_files, parse_sources, parse_sources_recovering,
    read_adapter_binding, read_source, suite_effective_tier_is_system,
};
pub use graph::{detect_consumes_cycles, detect_provider_dependency_cycles};
pub use json::json_string;
pub use paths::{
    MANIFEST_TABLES, ProjectPaths, ProjectPathsError, UnstableConfig, WorkersConfig,
    check_manifest, check_manifest_str, commons_dir_for, duplicate_compatibility_flag_warning,
    is_multi_file_layout, is_unpinned_range, normalize_rel, read_unstable_config,
    renamed_unit_name, render_package_json, try_read_project_paths, try_read_project_paths_with,
    try_read_unstable_config, try_read_unstable_config_with, try_read_workers_config,
    try_read_workers_config_with, ts_output_path, unit_path_matches, worker_dir_name,
    worker_file_source_path, worker_handlers_output_path, worker_handlers_source_path,
};
pub use roots::{Roots, SchemaLock, UnitKind};
