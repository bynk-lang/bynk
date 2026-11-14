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
pub fn run(
    input: PathBuf,
    format: CheckFormatArg,
    fix: Option<Vec<String>>,
    emit: bool,
    each: bool,
) -> ExitCode {
    let short = format == CheckFormatArg::Short;
    if emit && !input.is_dir() {
        eprintln!("bynk check: `--emit` takes a project directory, not a single file");
        return ExitCode::FAILURE;
    }
    let checked = bynk_driver::run_check("bynk", &input, short, fix.as_deref(), each);
    // #3068: `--emit` type-checks the generated TypeScript, but only for a
    // program that passed the check; a failed check has nothing to emit.
    if emit && checked == ExitCode::SUCCESS {
        return if bynk_driver::tsc_report::check_emit_all("bynk", &input, short, each) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    checked
}
