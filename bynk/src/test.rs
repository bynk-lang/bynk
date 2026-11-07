//! `bynk test` — discover and run a project's test declarations.
//!
//! Runs in-process (#2998, E1): the body is [`bynk_driver::test_runner::run_test`],
//! the one `bynk test` runs, so no separate compiler binary is needed and no second
//! compiler can skew from this one. What stays a child process is what always
//! was: `tsc`/`node` for the runner, and the `bynk dev` that `--against dev`
//! starts. This module adds what only `bynk` can supply: the workspace loop,
//! the smoke run's declared-status table (walked by `bynk-ide`), and the path
//! of this binary for `--against dev`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bynk_driver::test_filter::TestFilter;
use bynk_driver::test_runner::TestArgs;

/// Run `bynk test`.
pub fn run(args: TestArgs) -> ExitCode {
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
        let members: Vec<PathBuf> = workspace
            .members
            .iter()
            .map(|member| bynk_driver::member_dir(&args.input, member))
            .collect();
        // #2273: a `--suite`/`--case` value may live in one member only, so
        // each member runs as part of the selection, and the values are judged
        // here against every member's suites.
        let filter = TestFilter::new(&args.suite, &args.case);
        if !filter.is_empty()
            && let Some(available) = bynk_driver::test_runner::discover_cases("bynk", &members)
        {
            let unmatched = filter.unmatched_in(&available);
            if !unmatched.is_empty() {
                unmatched.report("bynk");
                return ExitCode::FAILURE;
            }
        }
        let mut failed = false;
        for member in members {
            println!("{}", bynk_driver::member_header(&member));
            // Flushed before the child writes to the same stdout.
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let member_args = TestArgs {
                input: member,
                partial_selection: !filter.is_empty(),
                ..args.clone()
            };
            failed |= run_one(member_args) != ExitCode::SUCCESS;
        }
        return if failed {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        };
    }
    run_one(args)
}

/// Why `args` can't run across a workspace's members, if they can't: one JSON
/// document, one output directory and one deployment each belong to a single
/// project.
fn workspace_refusal(args: &TestArgs) -> Option<&'static str> {
    if args.format == bynk_driver::test_runner::TestFormat::Json {
        Some("`--format json` reports one project — run it inside a member of this workspace")
    } else if args.format == bynk_driver::test_runner::TestFormat::Ndjson {
        Some("`--format ndjson` streams one project — run it inside a member of this workspace")
    } else if args.output.is_some() {
        Some("`--output` names one project's output — run it inside a member of this workspace")
    } else if args.against.is_some() {
        Some("`--against` runs one project's suites — run it inside a member of this workspace")
    } else {
        None
    }
}

/// Run the suites of one project.
fn run_one(mut args: TestArgs) -> ExitCode {
    let input = args.input.clone();
    // #2138: a run against a local `bynk dev` starts it through this binary.
    if args.against.as_deref() == Some(bynk_driver::remote::DEV) {
        match std::env::current_exe() {
            Ok(path) => args.dev_command = Some(path),
            Err(e) => {
                eprintln!("bynk test: could not find this `bynk` binary to start `bynk dev`: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    // #2685: the statuses each route can answer, which a smoke case's answer is
    // compared against. The walk is `bynk-ide`'s, so `bynk` makes it and hands
    // the runner the result; the table is only for a run that calls routes.
    // It goes in a file, since a large project's table could exceed what an
    // environment variable can hold (`E2BIG`), and the runner reads a file.
    let declared_file = (args.smoke && !args.print).then(|| {
        let table = crate::openapi::declared_statuses(&input);
        let path = std::env::temp_dir().join(format!(
            "bynk-smoke-declared-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::write(&path, serde_json::to_string(&table).unwrap_or_default()).map(|()| path)
    });
    match declared_file {
        Some(Ok(path)) => args.declared_statuses = Some(path),
        Some(Err(e)) => {
            eprintln!(
                "bynk test: could not write the smoke table, so undeclared statuses are not checked: {e}"
            );
        }
        None => {}
    }
    let declared = args.declared_statuses.clone();
    let code = bynk_driver::test_runner::run_test("bynk", args);
    if let Some(path) = &declared {
        let _ = std::fs::remove_file(path);
    }
    code
}
