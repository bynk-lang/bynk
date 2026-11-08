//! #3086: the `tsx` fallback honours the run timeout `bynk test --mutate` sets.
//!
//! With no `tsc` on `PATH`, `run_test` falls back to `tsx`; a `tsx` that outlives
//! `run_timeout` (with a child holding the pipes, as `tsx` does with `node`) must
//! be killed and the run must fail promptly, not wait out the child. This is its
//! own test binary because it replaces `PATH` for the process.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use bynk_driver::test_runner::{TestArgs, run_test};
use clap::Parser;

#[derive(Parser)]
struct Wrapper {
    #[command(flatten)]
    args: TestArgs,
}

#[test]
fn a_tsx_that_outlives_the_run_timeout_is_killed() {
    let scratch = std::env::temp_dir().join(format!("bynk-tsx-timeout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let bin = scratch.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    // A `tsx` that answers the `--version` probe, and otherwise starts a child
    // holding its pipes and waits on it.
    let tsx = bin.join("tsx");
    std::fs::write(
        &tsx,
        "#!/bin/sh\n[ \"$1\" = --version ] && { echo 4.0.0; exit 0; }\n: > \"$0.ran\"\n/bin/sleep 30 &\nwait\n",
    )
    .unwrap();
    std::fs::set_permissions(&tsx, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Only the fake `tsx` is on PATH, so there is no `tsc` to prefer.
    // SAFETY: this binary holds a single test, so nothing else reads the
    // environment concurrently.
    unsafe { std::env::set_var("PATH", &bin) };

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bynk-tests/tests/fixtures/positive/106_context_test_multiple_cases/src");
    let mut args = Wrapper::try_parse_from([
        "test",
        fixture.to_str().unwrap(),
        // `--mutate` runs the suites in JSON mode, the only one the timeout bounds.
        "--format",
        "json",
        "--output",
        scratch.join("out").to_str().unwrap(),
    ])
    .expect("parse")
    .args;
    args.run_timeout = Some(Duration::from_secs(1));

    let started = Instant::now();
    let code = run_test("bynk", args);
    let elapsed = started.elapsed();
    let reached = bin.join("tsx.ran").exists();
    let _ = std::fs::remove_dir_all(&scratch);

    // The fake must have run its long path: a run that failed earlier (no
    // runner found, would not start, did not compile) is not this test.
    assert!(
        reached,
        "the fake tsx never ran its long path; the run failed early ({elapsed:?})"
    );
    assert!(
        elapsed >= Duration::from_secs(1),
        "the run failed after {elapsed:?}, before the 1s timeout"
    );
    assert!(
        code == ExitCode::FAILURE,
        "a timed-out run must fail (took {elapsed:?})"
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "the run waited {elapsed:?}: the tsx tree was not killed at the 1s timeout"
    );
}
