//! Bynk v0.3 compiler library.
//!
//! Compiles `.bynk` commons source into TypeScript modules.
//!
//! Pipeline: lex → parse → resolve → check → emit.
//!
//! v0.3 introduces multi-file commons and the `uses` mechanism. A "project"
//! is a directory containing one or more commons; a commons is either a
//! single `.bynk` file or a directory of `.bynk` files that share a
//! `commons name` header. See [`compile_project`].
//!
//! The single-string entrypoint [`compile`] remains for v0–v0.2 fixtures
//! and any single-file commons that does not declare `uses` against another
//! commons.

pub mod cli;

// `write_output`/`write_document` moved down into `bynk-driver` (#1047,
// R2.3/T0.7 residue) — every caller was already at driver level. Re-exported
// here so `bynkc::write_output`/`bynkc::write_document` resolve. (`write_document`
// was `write_compiled_file` until P7.6, #1309/#1310 — renamed once `CompiledFile`
// itself was gone, so the name it takes now, `Document`, stayed true.)
pub use bynk_driver::{write_document, write_output};

// R10.4 residue (#1048, ADR 0312's reasoning applied to the three re-exports
// T-D1 didn't scope): `bynk_driver::{coverage, test_json}`,
// `bynk_syntax::{ast, diagnostics, error, keywords, lexer, parser, span}` and
// `bynk_fmt as fmt` were whole-module/whole-crate re-exports with no in-repo
// consumer besides `bynkc`'s own integration tests, which now import
// `bynk_syntax`/`bynk_driver`/`bynk_fmt` directly — the correct import in any
// case, same as T-D1. `CompileError` stays: `compile`/`compile_with_warnings`
// below return it, so it is genuinely part of the published API.
pub use bynk_syntax::CompileError;
pub use bynk_syntax::error::Severity;

// The diagnostic renderers moved down into the `bynk-render` crate (slice 6):
// ariadne human + the `short` line form over `CompileError`. Re-export them
// so `bynkc`'s binary, the diagnostic transcripts, and the tests resolve
// unchanged. The `ProjectFailure` flatteners (below) stay here and delegate.
pub use bynk_render::{
    print_errors, print_errors_short, print_project_errors, render_errors, render_errors_plain,
    render_errors_short, render_project_errors, without_hints,
};

pub use bynk_check::firstparty::Platform;

// The Node floor moved to `bynk-emit` (slice 7) so the `bynk` driver can read it
// without depending on the `bynkc` crate. Re-export it so `bynkc::NODE_MAJOR_FLOOR`
// and the `cli.rs` doc-links resolve unchanged.
pub use bynk_emit::project::{
    Artefacts, AttributedError, BuildTarget, CompileOptions, DiscoveredCase, DiscoveredSuite,
    Document, ImportExt, ProjectFailure, ProjectOutput, ProjectPaths, ProjectPathsError, Roots,
    SchemaLock, TestLocation, compile_project, sibling_path, try_read_project_paths,
    try_read_workers_config,
};
pub use bynk_emit::{Compiled, NODE_MAJOR_FLOOR, compile, compile_with_warnings};

// In-browser track (ADR 0137): strip-only TS→JS, re-exported so the CLI, the API,
// and tests share one entry point. `strip_project_to_js` moved into `bynk-strip`
// in slice 3 so the wasm entry can reuse it without depending on `bynkc`.
pub use bynk_strip::{StripError, strip_project_to_js, strip_types};

/// v0.24 (ADR 0052 rider) / ADR 0100: the project-failure flattening layer.
/// #521: the implementation is shared with the `bynk` driver in
/// [`bynk_driver`]; these re-exports keep `bynkc`'s public API (and its
/// callers) unchanged.
pub use bynk_driver::{
    ProjectOptionsError, print_project_failure, print_project_failure_short,
    print_project_warnings, project_failure_short_lines,
};
