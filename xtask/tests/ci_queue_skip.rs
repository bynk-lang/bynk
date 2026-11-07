//! Drift guard: the push run on `main` skips a job when the merge queue
//! already passed it on the same commit (ci.yml's `queue` step). Each job does
//! that by looking up its OWN `name:` in `needs.changes.outputs.queue_passed`,
//! so the name in its `if:` is a hand-kept copy.
//!
//! A stale copy is harmless (the job just runs again), but a copy pasted from
//! another job is not: that job would be skipped on `main` because a DIFFERENT
//! job passed, and the backstop would quietly stop covering it. This guard
//! turns that into a failing test, and makes leaving a gated job out of the
//! scheme a recorded decision rather than an oversight.

use std::path::PathBuf;

/// The clause a job's `if:` carries, up to the quoted job name.
const CLAUSE: &str = "!contains(fromJSON(needs.changes.outputs.queue_passed), '";

/// Jobs gated on `changes` that deliberately carry no clause, and why.
const EXEMPT: [(&str, &str); 6] = [
    // A matrix job: its legs are dropped from `test_os` instead.
    ("test", "legs dropped from test_os"),
    // A matrix job whose `name:` interpolates the leg, so there is no single
    // name to look up; it runs only on installer-script edits and the schedule.
    ("installers-smoke", "matrix; rare trigger"),
    // Runs on the nightly only: never in the queue, never on a push.
    ("coverage", "nightly only"),
    // The complement of `test`'s gate: `all` is true on every push, so it
    // never runs on `main` at all.
    ("drift", "never runs on push"),
    // The aggregators run on `always()`; skipping them would skip the gate.
    ("ci-fast", "aggregator"),
    ("ci-green", "aggregator"),
];

struct Job {
    key: String,
    name: Option<String>,
    needs_changes: bool,
    uses_rust_cache: bool,
    /// The `changes` output `runs-on` reads, when it is one of the self-hosted
    /// ones (`linux_runner` or `linux_runner_extra`), i.e. the job can run on
    /// the self-hosted runners.
    runner: Option<&'static str>,
    /// Every line of the job, for the self-hosted step checks.
    lines: Vec<String>,
    condition: String,
}

/// The workflow's jobs, read line by line in the same shape `ci-green`'s
/// audit reads them: two-space job keys under `jobs:`, four-space fields, and
/// a folded `if: >-` continued on deeper-indented lines. `needs` is read in
/// all three forms: a scalar, a flow list and a block list.
fn jobs() -> Vec<Job> {
    let mut jobs: Vec<Job> = Vec::new();
    let mut in_jobs = false;
    let mut in_if = false;
    let mut in_needs = false;
    for line in workflow().lines() {
        if line == "jobs:" {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        if let Some(key) = line
            .strip_prefix("  ")
            .and_then(|l| l.strip_suffix(':'))
            .filter(|k| !k.starts_with([' ', '#']) && !k.contains(' '))
        {
            jobs.push(Job {
                key: key.to_string(),
                name: None,
                needs_changes: false,
                uses_rust_cache: false,
                runner: None,
                lines: Vec::new(),
                condition: String::new(),
            });
            in_if = false;
            in_needs = false;
            continue;
        }
        let Some(job) = jobs.last_mut() else { continue };
        if line.contains("uses: Swatinem/rust-cache@") {
            job.uses_rust_cache = true;
        }
        job.lines.push(line.to_string());
        if line.starts_with("    runs-on:") {
            job.runner = if line.contains("needs.changes.outputs.linux_runner_extra") {
                Some("linux_runner_extra")
            } else if line.contains("needs.changes.outputs.linux_runner") {
                Some("linux_runner")
            } else {
                None
            };
        }
        if in_if && line.starts_with("      ") {
            job.condition.push(' ');
            job.condition.push_str(line.trim());
            continue;
        }
        in_if = false;
        if in_needs {
            if let Some(item) = line.trim().strip_prefix("- ") {
                job.needs_changes |= item == "changes";
                continue;
            }
            if line.trim().is_empty() || line.trim().starts_with('#') {
                continue;
            }
            in_needs = false;
        }
        if let Some(name) = line.strip_prefix("    name: ") {
            job.name = Some(name.to_string());
        } else if let Some(needs) = line.strip_prefix("    needs:") {
            let needs = needs.trim();
            in_needs = needs.is_empty();
            job.needs_changes = needs
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .any(|n| n.trim() == "changes");
        } else if let Some(cond) = line.strip_prefix("    if: ") {
            if cond == ">-" {
                in_if = true;
            } else {
                job.condition = cond.to_string();
            }
        }
    }
    assert!(!jobs.is_empty(), "no jobs parsed out of ci.yml");
    jobs
}

fn workflow() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/ci.yml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

#[test]
fn every_queue_skip_names_its_own_job() {
    let mut checked = 0;
    for job in jobs() {
        for (i, _) in job.condition.match_indices(CLAUSE) {
            let rest = &job.condition[i + CLAUSE.len()..];
            let quoted = &rest[..rest.find('\'').expect("unterminated job name")];
            assert_eq!(
                Some(quoted),
                job.name.as_deref(),
                "ci.yml job `{}` skips when the queue passed `{quoted}`, which is not its \
                 own `name:` — it would be skipped on `main` because another job passed",
                job.key
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no job in ci.yml carries the queue-skip clause"
    );
}

#[test]
fn every_gated_job_takes_part_or_is_exempt() {
    let jobs = jobs();
    let missing: Vec<&str> = jobs
        .iter()
        .filter(|j| j.needs_changes && !j.condition.contains(CLAUSE))
        .map(|j| j.key.as_str())
        .filter(|k| !EXEMPT.iter().any(|(e, _)| e == k))
        .collect();
    assert!(
        missing.is_empty(),
        "ci.yml job(s) {missing:?} are gated on `changes` but never consult \
         `queue_passed`, so the push run on `main` repeats them even when the merge \
         queue passed them on the same commit. Add `&& {CLAUSE}<its name>')` to the \
         job's `if:`, or list it in EXEMPT here with the reason."
    );
    for (key, _) in EXEMPT {
        assert!(
            jobs.iter().any(|j| j.key == key && j.needs_changes),
            "EXEMPT names `{key}`, which is no longer a job gated on `changes`"
        );
    }
}

/// The `queue` step skips nothing while `main` lacks a Rust cache any skipped
/// job would have saved (`RUST_CACHES`). A job that starts using rust-cache
/// without joining that list would lose its `main` cache to LRU eviction with
/// nothing refilling it.
#[test]
fn rust_caches_names_every_job_that_saves_one_on_main() {
    let yaml = workflow();
    let listed = yaml
        .lines()
        .find_map(|l| l.trim().strip_prefix("RUST_CACHES: \""))
        .and_then(|l| l.strip_suffix('"'))
        .expect("ci.yml's `queue` step sets RUST_CACHES on one line");
    let mut listed_jobs: Vec<&str> = listed
        .split_whitespace()
        .map(|c| c.rsplit_once('-').expect("`<job>-<os>`").0)
        .collect();
    listed_jobs.sort_unstable();
    listed_jobs.dedup();
    let mut expected: Vec<String> = jobs()
        .into_iter()
        .filter(|j| j.uses_rust_cache && !["drift", "coverage"].contains(&j.key.as_str()))
        .map(|j| j.key)
        .collect();
    expected.sort_unstable();
    assert_eq!(
        listed_jobs, expected,
        "RUST_CACHES in ci.yml's `queue` step must name every job that uses \
         Swatinem/rust-cache (bar `drift` and `coverage`, which never run on a push)"
    );
    for os in ["test-Linux", "test-Windows_NT", "test-Darwin"] {
        assert!(
            listed.split_whitespace().any(|c| c == os),
            "RUST_CACHES lacks `{os}`, one leg of the `test` matrix"
        );
    }
}

/// A job on the self-hosted runners skips rust-cache, so `main` never has its
/// cache. The `queue` step must not wait for one (`SELF_HOSTED_CACHES`, built
/// by the `runner` step), or every push to `main` would read it as missing and
/// run everything. Each group's list must name exactly its jobs that use
/// rust-cache (bar `drift`, which never runs on a push).
#[test]
fn self_hosted_caches_name_every_self_hosted_job_with_a_rust_cache() {
    let yaml = workflow();
    for (output, var) in [
        ("linux_runner", "LINUX_RUNNER_CACHES"),
        ("linux_runner_extra", "LINUX_RUNNER_EXTRA_CACHES"),
    ] {
        let listed = yaml
            .lines()
            .find_map(|l| l.trim().strip_prefix(&format!("{var}: \"")))
            .and_then(|l| l.strip_suffix('"'))
            .unwrap_or_else(|| panic!("ci.yml's `runner` step sets {var} on one line"));
        let mut listed_jobs: Vec<&str> = listed
            .split_whitespace()
            .map(|c| c.strip_suffix("-Linux").expect("`<job>-Linux`"))
            .collect();
        listed_jobs.sort_unstable();
        let mut expected: Vec<String> = jobs()
            .into_iter()
            .filter(|j| j.runner == Some(output) && j.uses_rust_cache && j.key != "drift")
            .map(|j| j.key)
            .collect();
        expected.sort_unstable();
        assert_eq!(
            listed_jobs, expected,
            "{var} on ci.yml's `runner` step must name every job that uses \
             Swatinem/rust-cache and whose `runs-on` reads `{output}`"
        );
    }
}

/// A job that can run on the self-hosted runners must use the shared
/// self-hosted checkout and clean steps (defined once with YAML anchors) and
/// skip every rust-cache step there. A plain `actions/checkout` runs
/// `git clean -ffdx`, which deletes the warm target/ every other job on that
/// runner relies on.
#[test]
fn every_self_hosted_job_keeps_target_warm() {
    let jobs: Vec<Job> = jobs().into_iter().filter(|j| j.runner.is_some()).collect();
    assert!(
        jobs.iter()
            .any(|j| j.key == "test" && j.runner == Some("linux_runner")),
        "the `test` job's Linux leg reads `linux_runner`"
    );
    for job in &jobs {
        let has = |step: &str| {
            job.lines.iter().any(|l| {
                let l = l.trim();
                l == format!("- *{step}") || l == format!("- &{step}")
            })
        };
        for step in ["self_hosted_checkout", "self_hosted_clean"] {
            assert!(
                has(step),
                "`{}` can run self-hosted but lacks `{step}`",
                job.key
            );
        }
        assert!(
            !job.lines
                .iter()
                .any(|l| l.contains("uses: actions/checkout@")
                    && !l.trim_start().starts_with("uses:")),
            "`{}` can run self-hosted but has its own `actions/checkout` step",
            job.key
        );
        for (i, line) in job.lines.iter().enumerate() {
            if line.contains("uses: Swatinem/rust-cache@") {
                assert_eq!(
                    job.lines.get(i + 1).map(|l| l.trim()),
                    Some("if: runner.environment != 'self-hosted'"),
                    "`{}`'s rust-cache step must skip the self-hosted runners",
                    job.key
                );
            }
        }
    }
}

/// The trust rule for the self-hosted runners (no fork or Dependabot PRs) is
/// the security boundary for machines that keep state between jobs, so it
/// lives in one place: the `runner` step on `changes`. A job that names the
/// self-hosted labels itself would bypass it.
#[test]
fn the_self_hosted_labels_appear_only_in_the_runner_step() {
    let yaml = workflow();
    let uses: Vec<&str> = yaml
        .lines()
        .filter(|l| l.contains("\"self-hosted\""))
        .collect();
    assert_eq!(
        uses.len(),
        1,
        "ci.yml must name the self-hosted labels only in `changes`' `runner` \
         step; other jobs take `needs.changes.outputs.linux_runner`: {uses:#?}"
    );
}
