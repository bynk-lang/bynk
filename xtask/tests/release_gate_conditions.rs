//! v0.603.9's tag push finished green and published nothing: `plan` dropped every
//! test leg, so `test` was skipped, and every job downstream of it carried a bare
//! `if: github.event_name == 'push'`, which GitHub ANDs with an implicit
//! `success()` — false when any job in the dependency chain was skipped. These
//! guards keep that from recurring: each tag-push job names its own `needs`
//! results explicitly, and `release-complete` fails the run unless all of them
//! succeeded.

use std::path::PathBuf;

/// Jobs that build or publish on a tag push.
const RELEASE_JOBS: [&str; 8] = [
    "binaries",
    "web-assets",
    "registry-core-wasm",
    "editor-contract",
    "github-release",
    "canary",
    "publish-crates",
    "publish-npm",
];

fn release_yaml() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/release.yml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

/// The body of top-level job `name`: from its `  name:` line to the next job.
fn job<'a>(yaml: &'a str, name: &str) -> &'a str {
    let head = format!("\n  {name}:\n");
    let start = yaml
        .find(&head)
        .unwrap_or_else(|| panic!("no job `{name}`"))
        + 1;
    let rest = &yaml[start + head.len() - 1..];
    let end = rest
        .lines()
        .position(|l| {
            l.starts_with("  ")
                && !l.starts_with("   ")
                && l.trim_end().ends_with(':')
                && !l.trim_start().starts_with('#')
        })
        .map_or(rest.len(), |n| {
            rest.lines().take(n).map(|l| l.len() + 1).sum()
        });
    &rest[..end]
}

fn if_line(body: &str) -> &str {
    body.lines()
        .find(|l| l.starts_with("    if:"))
        .unwrap_or_else(|| panic!("job has no `if:`"))
}

#[test]
fn release_jobs_do_not_rely_on_the_implicit_success() {
    let yaml = release_yaml();
    for name in RELEASE_JOBS {
        let cond = if_line(job(&yaml, name));
        assert!(
            cond.contains("!cancelled()"),
            "{name}: `{cond}` has no status-check function, so GitHub adds an implicit \
             success() that a skipped `test` (every leg already passed in ci.yml) turns false"
        );
    }
}

#[test]
fn release_jobs_require_each_of_their_needs() {
    let yaml = release_yaml();
    for name in RELEASE_JOBS {
        let body = job(&yaml, name);
        let cond = if_line(body);
        let needs = body
            .lines()
            .find_map(|l| l.strip_prefix("    needs:"))
            .unwrap_or_else(|| panic!("{name} has no `needs:`"))
            .trim()
            .trim_matches(|c| c == '[' || c == ']');
        for dep in needs.split(',').map(str::trim).filter(|d| !d.is_empty()) {
            assert!(
                cond.contains(&format!("needs.{dep}.result == 'success'")),
                "{name}: `if:` must require needs.{dep}.result == 'success'"
            );
        }
    }
}

#[test]
fn release_complete_waits_on_every_release_job_and_always_runs() {
    let yaml = release_yaml();
    let body = job(&yaml, "release-complete");
    assert!(
        if_line(body).contains("always()"),
        "release-complete must run even when upstream jobs were skipped or failed"
    );
    for name in RELEASE_JOBS
        .into_iter()
        .chain(["verify", "tests-passed", "release-notes"])
    {
        assert!(
            body.contains(&format!("      - {name}\n")),
            "release-complete must list `{name}` in `needs`, or that job can be skipped silently"
        );
    }
}
