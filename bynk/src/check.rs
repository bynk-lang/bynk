//! `bynk check` — type-check a `.bynk` file or project without writing compiled output.
//!
//! Runs the linked pipeline in-process (v0.138, #487): a directory routes
//! through [`bynk_driver::project::compile_project`], a single file through
//! [`bynk_emit::compile_with_warnings`] — exactly `bynk check`'s two branches, so the
//! output is identical. The body is [`bynk_driver::run_check`], shared with `bynk`.

use bynk_driver::{CheckOutput, emsg};
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
    environment: &str,
) -> ExitCode {
    let output = match format {
        CheckFormatArg::Rich => CheckOutput::Rich,
        CheckFormatArg::Short => CheckOutput::Short,
        CheckFormatArg::Json => CheckOutput::Json,
        CheckFormatArg::Ndjson => CheckOutput::Ndjson,
    };
    let short = output == CheckOutput::Short;
    if emit && !input.is_dir() {
        emsg!("bynk check: error: `--emit` takes a project directory, not a single file");
        return ExitCode::FAILURE;
    }
    // #4123: `--emit`'s findings are `tsc`'s, which have no JSON form here.
    if emit && output.is_machine() {
        emsg!(
            "bynk check: error: `--emit` reports `tsc` diagnostics, which `--format json` and `--format ndjson` do not carry; use `--format rich` or `short` with it"
        );
        return ExitCode::FAILURE;
    }
    let checked = bynk_driver::run_check("bynk", &input, output, fix.as_deref(), each, environment);
    // #3826: `--emit` builds the project a directory inside one belongs to, as the check did.
    let input = if each {
        input
    } else {
        bynk_driver::enclosing_check_project(&input).unwrap_or(input)
    };
    // #3068: `--emit` type-checks the generated TypeScript, but only for a
    // program that passed the check; a failed check has nothing to emit.
    if emit && checked == ExitCode::SUCCESS {
        return if bynk_driver::tsc_report::check_emit_all("bynk", &input, short, each, environment)
        {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    checked
}
