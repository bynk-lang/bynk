//! `bynk fmt` — format `.bynk` source files in place.
//!
//! Runs the formatter in-process over `bynk-fmt` (v0.138, #487), mirroring
//! `bynk fmt` byte-for-byte: each input is formatted and rewritten only when it
//! changes; `--check` reports non-canonical files without writing; `-` reads
//! stdin and writes the formatted result to stdout. The body is
//! [`bynk_driver::run_fmt`], shared with `bynk`.

use std::process::ExitCode;

use bynk_driver::FmtArgs;

/// Run `bynk fmt`.
pub fn run(args: FmtArgs) -> ExitCode {
    bynk_driver::run_fmt("bynk", &args)
}
