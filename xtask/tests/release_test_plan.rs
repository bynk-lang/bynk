//! #3391: `scripts/ci/release-test-plan.sh` drops the release's test legs that
//! ci.yml already passed on the tagged commit, and fails closed otherwise. A
//! stub `gh` serves canned API answers (one JSON file per URL, run through the
//! script's real `--jq` filter by `jq`).
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/ci/release-test-plan.sh")
}

const LINUX: &str = "Test suite (ubuntu-latest)";
const SYSTEM: &str = "System tests (Linux)";
const MAC: &str = "Test suite (macos-latest)";
const WIN: &str = "Test suite (windows-latest)";

fn slug(url: &str) -> String {
    url.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

struct Fixture {
    dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("bynk-release-plan-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let gh = dir.join("gh");
        std::fs::write(
            &gh,
            r#"#!/bin/sh
shift # `api`
url=$1; q=""; prev=""
for a in "$@"; do [ "$prev" = "--jq" ] && q=$a; prev=$a; done
f="$STUB_DIR/$(printf %s "$url" | tr -c 'A-Za-z0-9' _).json"
[ -f "$f" ] || exit 1
jq -r "$q" "$f"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        Fixture { dir }
    }

    fn put(&self, url: &str, json: &str) {
        std::fs::write(self.dir.join(format!("{}.json", slug(url))), json).unwrap();
    }

    /// A ci.yml run for `sha` under `event`, with the given (job, conclusion) pairs.
    fn run(&self, sha: &str, event: &str, id: u32, jobs: &[(&str, &str)]) {
        self.put(
            &format!(
                "repos/o/r/actions/workflows/ci.yml/runs?head_sha={sha}&event={event}&branch=main&per_page=1"
            ),
            &format!(r#"{{"workflow_runs":[{{"id":{id}}}]}}"#),
        );
        let list: Vec<String> = jobs
            .iter()
            .map(|(n, c)| format!(r#"{{"name":"{n}","conclusion":"{c}"}}"#))
            .collect();
        self.put(
            &format!("repos/o/r/actions/runs/{id}/jobs?per_page=100"),
            &format!(r#"{{"jobs":[{}]}}"#, list.join(",")),
        );
    }

    fn commit(&self, sha: &str, subject: &str, author: &str, parent: &str) {
        self.put(
            &format!("repos/o/r/commits/{sha}"),
            &format!(
                r#"{{"commit":{{"message":"{subject}\n\nbody","author":{{"name":"{author}"}}}},"parents":[{{"sha":"{parent}"}}]}}"#
            ),
        );
    }

    /// The `matrix` output, or None when `jq` is unavailable.
    fn plan(&self, event: &str, sha: &str) -> Option<String> {
        if Command::new("jq").arg("--version").output().is_err() {
            if cfg!(target_os = "linux") {
                panic!("jq is required");
            }
            return None;
        }
        let out_file = self.dir.join("out");
        let path = format!("{}:{}", self.dir.display(), std::env::var("PATH").unwrap());
        let out = Command::new("bash")
            .arg(script())
            .env("PATH", path)
            .env("STUB_DIR", &self.dir)
            .env("REPO", "o/r")
            .env("SHA", sha)
            .env("EVENT", event)
            .env("GITHUB_OUTPUT", &out_file)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = std::fs::read_to_string(&out_file).unwrap();
        Some(text.trim().strip_prefix("matrix=").unwrap().to_string())
    }
}

const ALL: &str = r#"["ubuntu-latest","macos-latest","windows-latest"]"#;

#[test]
fn a_fully_green_push_run_leaves_nothing_to_run() {
    let f = Fixture::new("green");
    f.commit("aaa", "feat: x", "someone", "p");
    f.run(
        "aaa",
        "push",
        1,
        &[
            (LINUX, "success"),
            (SYSTEM, "success"),
            (MAC, "success"),
            (WIN, "success"),
        ],
    );
    if let Some(m) = f.plan("push", "aaa") {
        assert_eq!(m, "[]");
    }
}

#[test]
fn only_the_legs_without_a_green_result_run() {
    let f = Fixture::new("partial");
    f.commit("aaa", "feat: x", "someone", "p");
    f.run(
        "aaa",
        "push",
        1,
        &[
            (LINUX, "success"),
            (SYSTEM, "success"),
            (MAC, "success"),
            (WIN, "failure"),
        ],
    );
    if let Some(m) = f.plan("push", "aaa") {
        assert_eq!(m, r#"["windows-latest"]"#);
    }
}

#[test]
fn a_linux_system_tier_gap_runs_every_leg() {
    // The OS legs run `ci-os`, which leans on the Linux system tier for the
    // tsc/node passes: without it they stand in for nothing.
    let f = Fixture::new("nosystem");
    f.commit("aaa", "feat: x", "someone", "p");
    f.run(
        "aaa",
        "push",
        1,
        &[(LINUX, "success"), (MAC, "success"), (WIN, "success")],
    );
    if let Some(m) = f.plan("push", "aaa") {
        assert_eq!(m, ALL);
    }
}

#[test]
fn no_run_and_lookup_errors_run_every_leg() {
    let f = Fixture::new("norun");
    if let Some(m) = f.plan("push", "aaa") {
        assert_eq!(m, ALL);
    }
}

#[test]
fn a_manual_dispatch_always_runs_every_leg() {
    let f = Fixture::new("dispatch");
    f.commit("aaa", "feat: x", "someone", "p");
    f.run(
        "aaa",
        "push",
        1,
        &[
            (LINUX, "success"),
            (SYSTEM, "success"),
            (MAC, "success"),
            (WIN, "success"),
        ],
    );
    if let Some(m) = f.plan("workflow_dispatch", "aaa") {
        assert_eq!(m, ALL);
    }
}

#[test]
fn the_queue_run_answers_for_jobs_the_push_run_skipped() {
    let f = Fixture::new("queue");
    f.commit("aaa", "feat: x", "someone", "p");
    // The push run has the OS legs; the queue passed the Linux pair instead.
    f.run("aaa", "push", 1, &[(MAC, "success"), (WIN, "success")]);
    f.run(
        "aaa",
        "merge_group",
        2,
        &[(LINUX, "success"), (SYSTEM, "success")],
    );
    if let Some(m) = f.plan("push", "aaa") {
        assert_eq!(m, "[]");
    }
}

#[test]
fn a_failed_push_leg_is_not_overridden_by_the_queue() {
    let f = Fixture::new("override");
    f.commit("aaa", "feat: x", "someone", "p");
    f.run(
        "aaa",
        "push",
        1,
        &[
            (LINUX, "failure"),
            (SYSTEM, "success"),
            (MAC, "success"),
            (WIN, "success"),
        ],
    );
    f.run(
        "aaa",
        "merge_group",
        2,
        &[(LINUX, "success"), (SYSTEM, "success")],
    );
    if let Some(m) = f.plan("push", "aaa") {
        assert_eq!(m, ALL);
    }
}

#[test]
fn a_stamp_commit_falls_back_to_its_parent_for_jobs_its_run_skipped() {
    let f = Fixture::new("stamp");
    f.commit(
        "sss",
        "chore(stamp): assign version",
        "bynk-stamp[bot]",
        "ppp",
    );
    // The stamp's own run is Linux only; the parent's run has the rest.
    f.run("sss", "push", 1, &[(LINUX, "success"), (SYSTEM, "success")]);
    f.run(
        "ppp",
        "push",
        2,
        &[
            (LINUX, "success"),
            (SYSTEM, "success"),
            (MAC, "success"),
            (WIN, "success"),
        ],
    );
    if let Some(m) = f.plan("push", "sss") {
        assert_eq!(m, "[]");
    }
}

#[test]
fn a_lookalike_stamp_commit_does_not_borrow_its_parents_runs() {
    let f = Fixture::new("lookalike");
    f.commit("sss", "chore(stamp): assign version", "someone-else", "ppp");
    f.run("sss", "push", 1, &[(LINUX, "success"), (SYSTEM, "success")]);
    f.run(
        "ppp",
        "push",
        2,
        &[
            (LINUX, "success"),
            (SYSTEM, "success"),
            (MAC, "success"),
            (WIN, "success"),
        ],
    );
    if let Some(m) = f.plan("push", "sss") {
        assert_eq!(m, r#"["macos-latest","windows-latest"]"#);
    }
}

#[test]
fn a_failed_job_on_a_stamp_commit_is_not_overridden_by_its_parent() {
    let f = Fixture::new("stampfail");
    f.commit(
        "sss",
        "chore(stamp): assign version",
        "bynk-stamp[bot]",
        "ppp",
    );
    f.run("sss", "push", 1, &[(LINUX, "failure"), (SYSTEM, "success")]);
    f.run(
        "ppp",
        "push",
        2,
        &[
            (LINUX, "success"),
            (SYSTEM, "success"),
            (MAC, "success"),
            (WIN, "success"),
        ],
    );
    if let Some(m) = f.plan("push", "sss") {
        assert_eq!(m, ALL);
    }
}

#[test]
fn an_in_progress_or_cancelled_job_is_not_a_pass() {
    for (tag, conclusion) in [("inprogress", "null"), ("cancelled", "\"cancelled\"")] {
        let f = Fixture::new(tag);
        f.commit("aaa", "feat: x", "someone", "p");
        f.put(
            "repos/o/r/actions/workflows/ci.yml/runs?head_sha=aaa&event=push&branch=main&per_page=1",
            r#"{"workflow_runs":[{"id":1}]}"#,
        );
        f.put(
            "repos/o/r/actions/runs/1/jobs?per_page=100",
            &format!(
                r#"{{"jobs":[{{"name":"{LINUX}","conclusion":"success"}},{{"name":"{SYSTEM}","conclusion":"success"}},{{"name":"{MAC}","conclusion":"success"}},{{"name":"{WIN}","conclusion":{conclusion}}}]}}"#
            ),
        );
        if let Some(m) = f.plan("push", "aaa") {
            assert_eq!(m, r#"["windows-latest"]"#, "{tag}");
        }
    }
}
