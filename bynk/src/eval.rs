//! `bynk eval` (#3585): evaluate a snippet in a unit's scope in the project and
//! print the value it ends in as one JSON document. The work is
//! [`bynk_driver::eval::evaluate`]; this reads the snippet and the unsaved
//! buffers and prints the result.

use std::io::Read as _;
use std::process::ExitCode;
use std::time::Duration;

use crate::cli::EvalCommand;
use bynk_driver::eval::{DEFAULT_TIMEOUT, EvalReport, EvalRequest, evaluate};

/// The seconds `--timeout` defaults to.
pub fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT.as_secs()
}

/// Print `report` as the document on stdout. The exit code says whether there
/// was a value.
fn print(report: &EvalReport) -> ExitCode {
    match serde_json::to_string(report) {
        Ok(text) => println!("{text}"),
        Err(e) => {
            eprintln!("bynk eval: could not render the result: {e}");
            return ExitCode::FAILURE;
        }
    }
    if report.succeeded() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// An evaluation that could not be set up, in the document's own shape.
fn refuse(message: impl Into<String>) -> ExitCode {
    print(&bynk_driver::eval::EvalOutcome::Error(message.into()).into())
}

/// Run `bynk eval`.
pub fn run(args: EvalCommand) -> ExitCode {
    let EvalCommand {
        input,
        unit,
        expr,
        sources_stdin,
        suite,
        timeout,
    } = args;
    // Stdin has one use: the sources document or the snippet.
    if sources_stdin && expr.is_none() {
        return refuse(
            "`--sources-stdin` reads the unsaved buffers from stdin, so the snippet must come from `--expr`",
        );
    }
    let mut stdin = String::new();
    if (sources_stdin || expr.is_none())
        && let Err(e) = std::io::stdin().read_to_string(&mut stdin)
    {
        return refuse(format!("could not read stdin: {e}"));
    }
    let (snippet, buffers) = if sources_stdin {
        match bynk_driver::source_overlay::parse_overlay(&stdin) {
            Ok(buffers) => (expr.unwrap_or_default(), buffers),
            Err(e) => return refuse(e.to_string()),
        }
    } else {
        (expr.unwrap_or(stdin), Default::default())
    };
    if snippet.trim().is_empty() {
        return refuse("there is nothing to evaluate: give `--expr`, or a snippet on stdin");
    }
    let timeout = Duration::from_secs(timeout);
    print(
        &evaluate(&EvalRequest {
            input,
            unit,
            snippet,
            buffers,
            suites: suite.into_iter().collect(),
            timeout,
        })
        .into(),
    )
}
