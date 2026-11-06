//! `bynk check` — type-check a `.bynk` file or project without writing output.
//!
//! Runs the linked pipeline in-process (v0.138, #487): a directory routes
//! through [`bynk_driver::project::compile_project`], a single file through
//! [`bynk_emit::compile_with_warnings`] — exactly `bynk check`'s two branches, so the
//! output is identical. The body is [`bynk_driver::run_check`], shared with `bynk`.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::cli::CheckFormatArg;

/// Run `bynk check`.
pub fn run(input: PathBuf, format: CheckFormatArg, fix: bool) -> ExitCode {
    bynk_driver::run_check("bynk", &input, format == CheckFormatArg::Short, fix)
}
