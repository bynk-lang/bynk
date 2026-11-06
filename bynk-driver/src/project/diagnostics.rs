use std::path::PathBuf;

use bynk_project::AttributedError;
use bynk_syntax::error::CompileError;

// P4.1 (#1115): `ProjectAnalysis`/`ContextSequenceInfo`/`ContextBoundaryInfo`
// relocated verbatim (Decision C) to `bynk-check/src/analysis.rs`, alongside
// the new `bynk-check`-native analysis entry point that also needs to build
// one. Re-exported here so `crate::project::{ProjectAnalysis, ...}` (this
// module's own `pub use` chain, `project.rs:80`) — and every downstream
// caller, including `bynk-ide`'s `crate::project::ProjectAnalysis`
// import — keeps resolving unchanged. `Mode`/`ErrorSink`/`ProjectFailure`
// below are unaffected: `Mode` and `ProjectFailure` are pipeline-driving
// facts specific to `bynk-emit`'s own `run_checks` callers
// (`compile_project`/`compile_in_memory`/`check_project`/
// `analyse_in_memory_with_types`), not project-model or checker output.
pub use bynk_check::analysis::{ContextBoundaryInfo, ContextSequenceInfo, ProjectAnalysis};
// `ErrorSink` relocated to `bynk-check::project_model` too — every
// `phase_*` function it moved alongside takes `&mut ErrorSink`, so the type
// had to travel with them (the same "shared logic pulls its own types down
// with it" reasoning `UnitTable`/`ConsumedType` already went through when
// `symbols.rs` moved). Re-exported at this crate-private path so every
// existing `bynk-emit`-internal call site is unchanged.
pub(crate) use bynk_check::project_model::ErrorSink;

/// Internal: do the work, given a source root (for commons/contexts) and a
/// test root (for test units). When both roots are the same path the
/// behaviour is identical to the v0.4+ single-tree layout. When they differ
/// — v0.9.1's split-paths mode — sources and tests are discovered separately
/// and the new `inconsistent_test_path` check fires.
/// v0.24 (ADR 0052): how the project pipeline is driven. `Build` preserves
/// the CLI contract exactly (bail at the structural and pre-emit gates);
/// `Analyse` never bails after discovery, skips all emission, and lets
/// independent unit groups resolve/check past another group's errors.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Build,
    Analyse,
}

/// v0.24: a failed build with its attribution and snapshots intact — what
/// the CLI renders rich (ariadne source context per file); the plain
/// `compile_project*` wrappers flatten it to the pre-v0.24 error list.
pub struct ProjectFailure {
    pub errors: Vec<AttributedError>,
    pub snapshots: Vec<(PathBuf, String)>,
    /// #1772: the project or tree root the build was given, as the caller
    /// spelled it. `snapshots` and each error's `source_path` are keyed by
    /// identity path, relative to this root; a renderer shows
    /// `display_root.join(identity)`, the path as typed from the working
    /// directory. Empty for an in-memory build.
    pub display_root: PathBuf,
}

impl ProjectFailure {
    /// The pre-v0.24 contract: collection-ordered, attribution dropped.
    pub fn flatten(self) -> Vec<CompileError> {
        self.errors.into_iter().map(|a| a.error).collect()
    }
}
