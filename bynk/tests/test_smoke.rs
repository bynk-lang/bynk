//! #2658: `bynk test --smoke` synthesises a `system` suite from a project's
//! `from http` routes and runs it in place of the project's own suites, in
//! process or against a local `bynk dev`.
//!
//! The project is `fixtures/smoke`: one route that answers a 500, one whose
//! path parameter is a `Matches` type `Val` cannot fabricate, and a suite of
//! the project's own that must not run. The runs need `node` (and, against
//! `dev`, `wrangler` or `npx`) and the sibling `bynkc` binary; without them a
//! test is skipped, and a non-empty `BYNK_REQUIRE_WORKERD` makes the `dev`
//! skip a failure.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            copy_dir(&path, &to.join(entry.file_name()));
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}

struct Project {
    dir: PathBuf,
    bynk: PathBuf,
    bynkc: PathBuf,
}

impl Project {
    /// A fresh copy of the fixture, or `None` when `bynkc` is not built.
    fn new(name: &str) -> Option<Self> {
        let bynk = PathBuf::from(env!("CARGO_BIN_EXE_bynk"));
        let bynkc = bynk.with_file_name("bynkc");
        if !bynkc.exists() {
            eprintln!("skipping: the `bynkc` binary is not built beside `bynk`");
            return None;
        }
        let dir = std::env::temp_dir().join(format!("bynk-smoke-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/smoke"),
            &dir.join("proj"),
        );
        Some(Project { dir, bynk, bynkc })
    }

    fn test(&self, args: &[&str]) -> Output {
        Command::new(&self.bynk)
            .arg("test")
            .arg(self.dir.join("proj"))
            .arg("--output")
            .arg(self.dir.join("out"))
            .args(args)
            .arg("--allow-skew")
            .env("BYNK_BYNKC", &self.bynkc)
            .output()
            .expect("run bynk test")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn document(o: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stderr = String::from_utf8_lossy(&o.stderr);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("not a JSON document ({e})\nstdout:\n{stdout}\nstderr:\n{stderr}")
    })
}

/// The `(name, outcome)` of every case in the document.
fn outcomes(doc: &serde_json::Value) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = doc["suites"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["cases"].as_array().into_iter().flatten())
        .map(|c| {
            (
                c["name"].as_str().unwrap_or_default().to_string(),
                c["outcome"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    all.sort();
    all
}

/// What both runs report: every case of the routes `Val` can fill passes but
/// the 500, the `Matches` route is skipped, and the project's own case is not
/// there.
fn assert_smoke_outcomes(doc: &serde_json::Value) {
    let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    };
    assert_eq!(
        outcomes(doc),
        pairs(&[
            ("smoke: GET /boom answers", "fail"),
            ("smoke: GET /code/:c answers", "skipped"),
            ("smoke: GET /ping answers", "pass"),
            ("smoke: POST /tag answers", "skipped"),
            ("smoke: POST /tag rejects a malformed body", "pass"),
            ("smoke: PUT /cart/:sku answers", "pass"),
            ("smoke: PUT /cart/:sku refuses no credential", "pass"),
            ("smoke: PUT /cart/:sku rejects a malformed body", "pass"),
        ]),
        "{doc:#}"
    );
    assert_eq!(doc["passed"], 5, "{doc:#}");
    assert_eq!(doc["failed"], 1, "{doc:#}");
    assert_eq!(doc["skipped"], 2, "{doc:#}");
}

#[test]
fn print_writes_the_synthesised_suite_and_names_the_skip() {
    let Some(p) = Project::new("print") else {
        return;
    };
    let o = p.test(&["--smoke", "--print"]);
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(stdout.contains("// src/shop/api.smoke.bynk"), "{stdout}");
    assert!(stdout.contains("suite shop.api as system {"), "{stdout}");
    assert!(
        stdout.contains(
            r#"let r <- api.PUT("/cart/:sku", Val[Sku], Val[Item]) by User(Val[UserId])"#
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            r#"// skipped `shop.api` case "smoke: GET /code/:c answers" [bynk.val.needs_pin]"#
        ),
        "{stdout}"
    );
    // Nothing ran, and nothing was written into the project.
    assert!(!p.dir.join("out").exists());
    assert!(!p.dir.join("proj/src/shop/api.smoke.bynk").exists());
}

#[test]
fn a_smoke_run_in_process_calls_every_route_and_leaves_the_projects_suites_out() {
    if !have("node") {
        eprintln!("skipping: `node` is not available");
        return;
    }
    let Some(p) = Project::new("in-process") else {
        return;
    };
    let o = p.test(&["--smoke", "--format", "json"]);
    let doc = document(&o);
    assert!(!o.status.success(), "the 500 fails the run\n{doc:#}");
    assert_smoke_outcomes(&doc);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        stderr.contains("smoke: 6 cases from 5 routes, 2 skipped"),
        "{stderr}"
    );
}

#[test]
fn a_smoke_run_against_dev_calls_every_route_on_a_real_runtime() {
    if !have("node") || !(have("wrangler") || have("npx")) {
        eprintln!("skipping: `node`, and `wrangler` or `npx`, are not available");
        if std::env::var("BYNK_REQUIRE_WORKERD").is_ok_and(|v| !v.is_empty()) {
            panic!("BYNK_REQUIRE_WORKERD is set but `node` or `wrangler` is missing");
        }
        return;
    }
    let Some(p) = Project::new("dev") else {
        return;
    };
    let o = p.test(&["--smoke", "--format", "json", "--against", "dev"]);
    let doc = document(&o);
    assert!(!o.status.success(), "the 500 fails the run\n{doc:#}");
    assert_eq!(doc["transport"], "remote:dev", "{doc}");
    assert_smoke_outcomes(&doc);
}

#[test]
fn a_project_with_no_http_route_has_nothing_to_smoke() {
    let Some(p) = Project::new("empty") else {
        return;
    };
    std::fs::write(
        p.dir.join("proj/src/shop/api.bynk"),
        "context shop.api\n\nfn double(x: Int) -> Int {\n  x * 2\n}\n",
    )
    .unwrap();
    std::fs::remove_file(p.dir.join("proj/tests/api.test.bynk")).unwrap();
    let o = p.test(&["--smoke"]);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "{stderr}");
    assert!(
        stderr.contains("`--smoke` found no `from http` route to call"),
        "{stderr}"
    );
}

#[test]
fn print_and_smoke_unsafe_need_smoke() {
    let Some(p) = Project::new("flags") else {
        return;
    };
    for flag in ["--print", "--smoke-unsafe"] {
        let o = p.test(&[flag]);
        assert!(!o.status.success(), "{flag} without --smoke is refused");
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert!(stderr.contains("--smoke"), "{flag}: {stderr}");
    }
}
