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
            ("smoke: GET /clash answers", "pass"),
            ("smoke: GET /code/:c answers", "skipped"),
            ("smoke: GET /hello/:name answers", "pass"),
            ("smoke: GET /ping answers", "pass"),
            ("smoke: POST /tag answers", "skipped"),
            ("smoke: POST /tag rejects a malformed body", "pass"),
            ("smoke: PUT /cart/:sku answers", "pass"),
            ("smoke: PUT /cart/:sku refuses no credential", "pass"),
            ("smoke: PUT /cart/:sku rejects a malformed body", "pass"),
        ]),
        "{doc:#}"
    );
    assert_eq!(doc["passed"], 7, "{doc:#}");
    assert_eq!(doc["failed"], 1, "{doc:#}");
    assert_eq!(doc["skipped"], 2, "{doc:#}");
    assert_clash_warning(doc);
}

/// #2685: `/clash` answers a `409` that its handler's body does not declare. The
/// case still passes, and carries the warning; no other case does.
fn assert_clash_warning(doc: &serde_json::Value) {
    let cases: Vec<&serde_json::Value> = doc["suites"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["cases"].as_array().into_iter().flatten())
        .filter(|c| c.get("warnings").is_some())
        .collect();
    assert_eq!(cases.len(), 1, "{doc:#}");
    let case = cases[0];
    assert_eq!(case["name"], "smoke: GET /clash answers", "{doc:#}");
    assert_eq!(case["outcome"], "pass", "{doc:#}");
    let warnings = case["warnings"].as_array().expect("an array");
    assert_eq!(warnings.len(), 1, "{doc:#}");
    let w = &warnings[0];
    assert_eq!(w["code"], "bynk.test.smoke_undeclared_status", "{doc:#}");
    assert_eq!(w["route"], "GET /clash", "{doc:#}");
    assert_eq!(w["status"], 409, "{doc:#}");
    assert_eq!(w["declared"], serde_json::json!([200]), "{doc:#}");
    assert!(
        w["message"]
            .as_str()
            .is_some_and(|m| m.contains("answered 409 Conflict") && m.contains("declares 200")),
        "{doc:#}"
    );
}

/// The stderr line a run writes for the `/clash` warning.
const CLASH_LINE: &str = "warning[bynk.test.smoke_undeclared_status]: `GET /clash` (shop.api, service `api`) answered 409 Conflict, which it does not declare; it declares 200";

#[test]
fn an_undeclared_status_is_a_warning_on_stderr_in_the_human_format() {
    if !have("node") {
        eprintln!("skipping: `node` is not available");
        return;
    }
    let Some(p) = Project::new("warn-human") else {
        return;
    };
    let o = p.test(&["--smoke"]);
    let stderr = String::from_utf8_lossy(&o.stderr);
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(stderr.contains(CLASH_LINE), "{stderr}\n{stdout}");
    // A warning is not a failure: the case passes, and only the 500 fails.
    assert!(stdout.contains("✓ smoke: GET /clash answers"), "{stdout}");
    assert!(stdout.contains("7 passed, 1 failed"), "{stdout}");
}

#[test]
fn an_undeclared_status_is_a_warning_in_json_and_on_stderr() {
    if !have("node") {
        eprintln!("skipping: `node` is not available");
        return;
    }
    let Some(p) = Project::new("warn-json") else {
        return;
    };
    let o = p.test(&["--smoke", "--format", "json"]);
    assert_clash_warning(&document(&o));
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains(CLASH_LINE), "{stderr}");
}

#[test]
fn an_undeclared_status_is_a_warning_on_the_case_event_in_ndjson() {
    if !have("node") {
        eprintln!("skipping: `node` is not available");
        return;
    }
    let Some(p) = Project::new("warn-ndjson") else {
        return;
    };
    let o = p.test(&["--smoke", "--format", "ndjson"]);
    let stdout = String::from_utf8_lossy(&o.stdout);
    let events: Vec<serde_json::Value> = stdout
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    // The runner's `warning` line is not an event of the stream: the case it
    // belongs to carries it, between `origin` and `durationMs`.
    assert!(events.iter().all(|e| e["type"] != "warning"), "{stdout}");
    let line = stdout
        .lines()
        .find(|l| l.contains("smoke: GET /clash answers"))
        .expect("the case event");
    let warnings = line.find("\"warnings\":").expect("warnings on the event");
    assert!(line.find("\"tier\":").unwrap() < warnings, "{line}");
    assert!(warnings < line.find("\"durationMs\":").unwrap(), "{line}");
    let event: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(event["warnings"][0]["status"], 409, "{line}");
    assert_eq!(event["warnings"][0]["declared"], serde_json::json!([200]));
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains(CLASH_LINE), "{stderr}");
    // No other case carries one.
    assert_eq!(stdout.matches("\"warnings\":").count(), 1, "{stdout}");
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
        stdout.contains(r#"api.GET("/hello/:name", "a") by Visitor"#),
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
        stderr.contains("smoke: 8 cases from 7 routes, 2 skipped"),
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
