//! `bynk compile` — compile a project to TypeScript in a directory (#2471).
//!
//! Like `bynk test`, this always delegates: it shells `bynkc compile`, the
//! `bynkc` the driver resolves (`BYNK_BYNKC` -> PATH -> sibling-of-`bynk`), so an
//! editor inherits that resolution instead of locating `bynkc` itself. stdin and
//! stdout are inherited, which is how `--sources-stdin` reaches the compiler.
//! The only flags forwarded are the ones the editor needs; `bynkc compile`
//! keeps the rest (`--target`, `--platform`, `--emit`).

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::compiler::{Compiler, SkewAsker, skew_gate};

/// Run `bynk compile` by shelling the resolved `bynkc`.
pub fn run(
    compiler: &Compiler,
    input: PathBuf,
    output: PathBuf,
    sources_stdin: bool,
    allow_skew: bool,
) -> ExitCode {
    let Some(bynkc) = compiler.path.as_deref() else {
        eprintln!(
            "bynk compile: no `bynkc` compiler found (looked at $BYNK_BYNKC, PATH, and next to `bynk`)."
        );
        eprintln!("  Run `bynk doctor --only compile` for the exact remedy.");
        return ExitCode::FAILURE;
    };
    let asker = SkewAsker {
        command: Some("compile"),
        has_flag: true,
    };
    if !skew_gate(compiler, asker, allow_skew) {
        return ExitCode::FAILURE;
    }
    crate::shell::delegate(bynkc, argv(input, output, sources_stdin))
}

fn argv(input: PathBuf, output: PathBuf, sources_stdin: bool) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec![
        "compile".into(),
        input.into_os_string(),
        "--output".into(),
        output.into_os_string(),
    ];
    if sources_stdin {
        argv.push("--sources-stdin".into());
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_the_input_the_output_and_the_stdin_flag() {
        let a = argv("proj".into(), "out".into(), true);
        assert_eq!(
            a,
            ["compile", "proj", "--output", "out", "--sources-stdin"].map(OsString::from)
        );
        let b = argv("proj".into(), "out".into(), false);
        assert_eq!(b.len(), 4);
    }
}
