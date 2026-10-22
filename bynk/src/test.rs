//! `bynk test` — discover and run a project's test declarations.
//!
//! Delegates to the `bynkc` the driver resolves (`BYNK_BYNKC` → PATH →
//! sibling-of-`bynk`) and shells `bynkc test`, forwarding every flag verbatim
//! (v0.138, #487). Unlike `check`/`fmt`, `test` delegates *always*, not only
//! under an override: it orchestrates external `tsc`/`node`, so it is a
//! subprocess regardless, and routing it through the driver's resolution is the
//! whole point — an editor or developer inherits that resolution instead of
//! locating `bynkc` themselves (the fix direction for #486). The trade-off (a
//! `bynkc` binary must be present, and the driver↔compiler skew surface stays)
//! is accepted; the driver at least resolves it more richly than any editor.
//! #1675: and it acts on that skew before delegating — see
//! [`crate::compiler::skew_gate`].

use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;

use bynk_driver::test_runner::TestArgs;

use crate::compiler::{Compiler, SkewAsker, skew_gate};

/// Run `bynk test` by shelling the resolved `bynkc`. When no `bynkc` could be
/// located, point the developer at `bynk doctor` rather than emitting a raw
/// spawn error.
///
/// Wave 5 §5.4 (findings #40/#72): `args` is the one contract `bynkc test`
/// and `bynk test` both flatten (`bynk_driver::test_runner::TestArgs`) —
/// this function's whole body is re-spelling its fields back out as argv
/// tokens for the shell-out, so a field added there needs no matching change
/// here beyond this list.
pub fn run(compiler: &Compiler, args: TestArgs, allow_skew: bool) -> ExitCode {
    let Some(bynkc) = compiler.path.as_deref() else {
        eprintln!(
            "bynk test: no `bynkc` compiler found (looked at $BYNK_BYNKC, PATH, and next to `bynk`)."
        );
        eprintln!("  Run `bynk doctor --only test` for the exact remedy.");
        return ExitCode::FAILURE;
    };
    // #1675: `test` always runs a second compiler, so act on its skew from the
    // driver: a minor skew warns, a major one refuses unless allowed.
    let asker = SkewAsker {
        command: Some("test"),
        has_flag: true,
    };
    if !skew_gate(compiler, asker, allow_skew) {
        return ExitCode::FAILURE;
    }

    // #2207 (decision C): at a workspace root, test every member in order,
    // each under a header. Every member runs, and the run fails if any does.
    if let Some(workspace) = bynk_driver::workspace_at(&args.input) {
        let workspace = match workspace {
            Ok(w) => w,
            Err(e) => {
                eprintln!(
                    "bynk test: {}: {e}",
                    bynk_driver::display_path(Path::new(""), &args.input.join("bynk.toml"))
                );
                return ExitCode::FAILURE;
            }
        };
        if let Some(refusal) = workspace_refusal(&args) {
            eprintln!("bynk test: {refusal}");
            return ExitCode::FAILURE;
        }
        let mut failed = false;
        for member in &workspace.members {
            let member = bynk_driver::member_dir(&args.input, member);
            println!("{}", bynk_driver::member_header(&member));
            // Flushed before the child writes to the same stdout.
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let member_args = TestArgs {
                input: member,
                ..args.clone()
            };
            failed |= run_one(bynkc, member_args) != ExitCode::SUCCESS;
        }
        return if failed {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        };
    }
    run_one(bynkc, args)
}

/// Why `args` can't run across a workspace's members, if they can't: one JSON
/// document, one output directory and one deployment each belong to a single
/// project.
fn workspace_refusal(args: &TestArgs) -> Option<&'static str> {
    if args.format == bynk_driver::test_runner::TestFormat::Json {
        Some("`--format json` reports one project — run it inside a member of this workspace")
    } else if args.output.is_some() {
        Some("`--output` names one project's output — run it inside a member of this workspace")
    } else if args.against.is_some() {
        Some("`--against` runs one project's suites — run it inside a member of this workspace")
    } else {
        None
    }
}

/// Shell `bynkc test` for one project.
fn run_one(bynkc: &Path, args: TestArgs) -> ExitCode {
    let mut argv: Vec<OsString> = vec!["test".into(), args.input.into_os_string()];
    if let Some(output) = args.output {
        argv.push("--output".into());
        argv.push(output.into_os_string());
    }
    if args.no_run {
        argv.push("--no-run".into());
    }
    argv.push("--format".into());
    argv.push(args.format.as_bynkc_arg().into());
    if args.inspect {
        argv.push("--inspect".into());
    }
    if let Some(seed) = args.seed {
        argv.push("--seed".into());
        argv.push(seed.into());
    }
    for suite in args.suite {
        argv.push("--suite".into());
        argv.push(suite.into());
    }
    for case in args.case {
        argv.push("--case".into());
        argv.push(case.into());
    }
    if args.coverage {
        argv.push("--coverage".into());
    }
    // #2138: a run against a named target, or against a local `bynk dev`,
    // which `bynkc` starts through this binary.
    let starts_dev = args.against.as_deref() == Some(bynk_driver::remote::DEV);
    if let Some(target) = args.against {
        argv.push("--against".into());
        argv.push(target.into());
        // `--settle` has a default, so it is always set; it means something,
        // and `bynkc` accepts it, only with `--against`.
        if let Some(settle) = args.settle {
            argv.push("--settle".into());
            argv.push(settle.into());
        }
    }
    if args.skip_unsupported {
        argv.push("--skip-unsupported".into());
    }
    if starts_dev {
        let me = match std::env::current_exe() {
            Ok(me) => me,
            Err(e) => {
                eprintln!("bynk test: could not find this `bynk` binary to start `bynk dev`: {e}");
                return ExitCode::FAILURE;
            }
        };
        return crate::shell::delegate_with_env(
            bynkc,
            argv,
            &[(bynk_driver::remote::DEV_COMMAND_ENV, me.as_os_str())],
        );
    }

    crate::shell::delegate(bynkc, argv)
}
