//! Drift guard: #1676 — every workflow that runs the workspace suite runs it
//! with the same harness and the same profile, so the retry and isolation
//! policy is decided once, in `.config/nextest.toml`'s `ci` profile.
//!
//! The three gates drifted once already: the PR gate ran nextest under `--profile
//! ci` (one process per test, one retry) while the release ran `cargo test`
//! (threads in a shared process, no retry), so a release could fail a test the
//! PR gate passed for a harness reason alone. This guard turns the next such
//! drift into a failing test rather than a comment that quietly became false.

use std::path::PathBuf;

/// The command every suite-running gate invokes.
const SUITE_COMMAND: &str = "cargo nextest run --workspace --locked --profile ci";

/// The workflows that run the workspace suite: the PR gate, the release gate,
/// and the bootstrap's verify job.
const SUITE_WORKFLOWS: [&str; 3] = ["ci.yml", "release.yml", "release-bootstrap.yml"];

fn workflow(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../.github/workflows")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

/// The non-comment lines of a workflow, so prose that *mentions* a command
/// (as these workflows' comments do) is never mistaken for a step that runs it.
fn run_lines(yaml: &str) -> Vec<&str> {
    yaml.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .collect()
}

#[test]
fn every_suite_gate_runs_nextest_under_the_ci_profile() {
    for name in SUITE_WORKFLOWS {
        let yaml = workflow(name);
        assert!(
            run_lines(&yaml).iter().any(|l| l.ends_with(SUITE_COMMAND)),
            "{name} must run the workspace suite as `{SUITE_COMMAND}` — the one \
             harness and profile every gate shares (.config/nextest.toml). A \
             release that needs a different policy gets a named profile there, \
             not a different command here."
        );
    }
}

#[test]
fn no_suite_gate_runs_the_workspace_suite_under_cargo_test() {
    for name in SUITE_WORKFLOWS {
        let yaml = workflow(name);
        let offenders: Vec<&str> = run_lines(&yaml)
            .into_iter()
            .filter(|l| l.contains("cargo test --workspace"))
            .collect();
        assert!(
            offenders.is_empty(),
            "{name} runs the workspace suite under `cargo test` (threads in one \
             process, no retry) — use `{SUITE_COMMAND}` like the other gates: \
             {offenders:?}"
        );
    }
}

/// The `ci` profile's worst case for one hung test, in minutes: killed at
/// `period × terminate-after`, then retried once and hung again. Read from
/// `.config/nextest.toml`'s `slow-timeout` and `retries` lines.
fn hang_budget_minutes() -> u64 {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.config/nextest.toml");
    let toml = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let line = |key: &str| {
        run_lines(&toml)
            .into_iter()
            .find(|l| l.starts_with(key))
            .unwrap_or_else(|| panic!("`.config/nextest.toml` has no `{key}` line"))
            .to_string()
    };
    let number_after = |s: &str, key: &str| -> u64 {
        let rest = &s[s.find(key).unwrap_or_else(|| panic!("no `{key}` in `{s}`")) + key.len()..];
        rest.trim_start_matches(|c: char| !c.is_ascii_digit())
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap_or_else(|_| panic!("no number after `{key}` in `{s}`"))
    };
    let slow = line("slow-timeout");
    let period_secs = number_after(&slow, "period");
    let terminate_after = number_after(&slow, "terminate-after");
    let retries = number_after(&line("retries"), "count");
    (period_secs * terminate_after * (retries + 1)).div_ceil(60)
}

/// Each workflow job that runs nextest under `--profile ci`, with its
/// `timeout-minutes` (`None` when it sets none). Jobs are the two-space keys
/// under `jobs:`; a step is the job's when it falls before the next such key.
fn ci_profile_jobs(yaml: &str) -> Vec<(String, Option<u64>)> {
    let mut out: Vec<(String, Option<u64>, bool)> = Vec::new();
    let mut in_jobs = false;
    for raw in yaml.lines() {
        if raw.trim_start().starts_with('#') {
            continue;
        }
        if raw == "jobs:" {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        if let Some(key) = raw.strip_prefix("  ")
            && !key.starts_with(' ')
            && let Some(name) = key.strip_suffix(':')
        {
            out.push((name.to_string(), None, false));
        } else if let Some(job) = out.last_mut() {
            let line = raw.trim();
            if let Some(m) = line.strip_prefix("timeout-minutes:") {
                job.1 = m.trim().parse().ok();
            }
            if line.contains("cargo nextest run") && line.contains("--profile ci") {
                job.2 = true;
            }
        }
    }
    out.into_iter()
        .filter(|(_, _, runs)| *runs)
        .map(|(name, minutes, _)| (name, minutes))
        .collect()
}

/// Every job that runs the `ci` profile leaves room for its worst hang: a
/// hung test killed by `slow-timeout`, retried, and hung again, plus the
/// build and the rest of the suite. Without that room the job's own
/// `timeout-minutes` cancels the run first, with the hung test unnamed and
/// every later job skipped: the v0.330.4 release (#1929).
#[test]
fn every_ci_profile_job_outlasts_a_hung_test_and_its_retry() {
    /// The build and the rest of the suite, alongside the hang. A cold
    /// Windows leg takes about 10 minutes end to end.
    const HEADROOM_MINUTES: u64 = 15;
    let need = hang_budget_minutes() + HEADROOM_MINUTES;
    let mut seen = 0;
    for name in SUITE_WORKFLOWS {
        for (job, minutes) in ci_profile_jobs(&workflow(name)) {
            seen += 1;
            let minutes = minutes.unwrap_or_else(|| {
                panic!("{name} job `{job}` runs the ci profile with no `timeout-minutes`")
            });
            assert!(
                minutes >= need,
                "{name} job `{job}` has `timeout-minutes: {minutes}`, but a hung test and \
                 its retry take {} minutes under `.config/nextest.toml`'s `slow-timeout` \
                 and `retries`, plus {HEADROOM_MINUTES} for the build and the rest of the \
                 suite: raise the job's limit to at least {need}, or tighten the profile",
                need - HEADROOM_MINUTES
            );
        }
    }
    assert!(
        seen >= SUITE_WORKFLOWS.len(),
        "found only {seen} ci-profile jobs"
    );
}

/// C4c (#2940): the PR gate runs the Linux suite as two parallel jobs, one per
/// profile, and every other OS (and the release gates) the whole suite. If a
/// profile's job went missing, its tests would run nowhere.
#[test]
fn the_pr_gate_runs_the_linux_tiers_as_parallel_jobs() {
    let yaml = workflow("ci.yml");
    let lines = run_lines(&yaml);
    for profile in ["ci-lower", "ci-system"] {
        assert!(
            lines.iter().any(
                |l| l.contains("cargo nextest run --workspace --locked --profile")
                    && l.ends_with(profile)
            ),
            "ci.yml must run `--profile {profile}` (the Linux tier split, C4c)"
        );
    }
    let jobs = ci_profile_jobs(&yaml);
    for job in ["test", "test-system", "test-ts7"] {
        assert!(
            jobs.iter().any(|(name, _)| name == job),
            "ci.yml job `{job}` must run nextest under a `ci` profile"
        );
    }
    // The system job must not wait for the unit job (a `needs:` edge would
    // serialise them and slow the green path); it watches it instead.
    assert!(
        lines
            .iter()
            .any(|l| l.contains("scripts/ci/run-unless-sibling-fails.sh")),
        "test-system must run under the sibling watcher"
    );
}
