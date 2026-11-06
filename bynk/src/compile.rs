//! `bynk compile` — compile a project (or one file) to TypeScript or JavaScript.
//!
//! Runs in-process (#2998, E1): the body is [`bynk_driver::run_compile`], the
//! same one `bynk compile` runs, so the output is identical and no `bynk`
//! binary needs to be installed. stdin is inherited, which is how
//! `--sources-stdin` (what "Bynk: Show Compiled Output" uses) reaches it.

use std::process::ExitCode;

use bynk_driver::CompileArgs;

/// Run `bynk compile`.
pub fn run(args: CompileArgs) -> ExitCode {
    bynk_driver::run_compile("bynk", args)
}
