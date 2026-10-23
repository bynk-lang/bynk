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
const EXEMPT: [(&str, &str); 3] = [
    // A matrix job: its legs are dropped from `test_os` instead.
    ("test", "legs dropped from test_os"),
    // Never runs in the queue; on a push it follows `test_os`'s Linux leg.
    ("coverage", "follows test_os"),
    // The complement of `test`'s gate: `all` is true on every push, so it
    // never runs on `main` at all.
    ("drift", "never runs on push"),
];

struct Job {
    key: String,
    name: Option<String>,
    needs_changes: bool,
    condition: String,
}

/// The workflow's jobs, read line by line in the same shape `ci-green`'s
/// audit reads them: two-space job keys under `jobs:`, four-space fields, and
/// a folded `if: >-` continued on deeper-indented lines.
fn jobs() -> Vec<Job> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/ci.yml");
    let yaml = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let mut jobs: Vec<Job> = Vec::new();
    let mut in_jobs = false;
    let mut in_if = false;
    for line in yaml.lines() {
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
                condition: String::new(),
            });
            in_if = false;
            continue;
        }
        let Some(job) = jobs.last_mut() else { continue };
        if in_if && line.starts_with("      ") {
            job.condition.push(' ');
            job.condition.push_str(line.trim());
            continue;
        }
        in_if = false;
        if let Some(name) = line.strip_prefix("    name: ") {
            job.name = Some(name.to_string());
        } else if line == "    needs: changes" {
            job.needs_changes = true;
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
