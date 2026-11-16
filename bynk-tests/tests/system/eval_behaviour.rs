//! #3585: `bynk eval` evaluates a snippet in a unit's scope and prints its
//! value, as JSON, with the project's unsaved buffers honoured.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const UNIT: &str = "\
context demo

consumes bynk { Fetch, Request, Method, Response }

type Percent = Int where InRange(0, 100)

fn double(n: Int) -> Int {
  n * 2
}

fn spin(n: Int) -> Int {
  spin(n + 1)
}

service relay {
  on call(url: String) -> Effect[Int] given Fetch {
    let res <- Fetch.send(Request {
      method: Get,
      url: url,
      contentType: None,
      authorization: None,
      headers: Map.empty(),
      body: None,
    })
    match res {
      Ok(r) => Effect.pure(r.status)
      Err(_) => Effect.pure(-1)
    }
  }
}
";

fn project(tag: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("eval-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("bynk.toml"), "[project]\nname = \"p\"\n").unwrap();
    std::fs::write(root.join("src/demo.bynk"), UNIT).unwrap();
    root
}

/// Run `bynk eval` in `root` with `args`, `stdin` on its standard input: the
/// exit status and the document it printed.
fn eval(root: &Path, args: &[&str], stdin: &str) -> (bool, serde_json::Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("eval")
        .arg(root)
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bynk runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let doc = serde_json::from_str(text.trim()).unwrap_or_else(|e| {
        panic!(
            "not a document ({e}): {text}{}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.success(), doc)
}

#[test]
fn an_expression_prints_its_value_and_type() {
    let root = project("value");
    let (ok, doc) = eval(&root, &["--unit", "demo", "--expr", "double(21)"], "");
    assert!(ok, "{doc}");
    assert_eq!(
        doc,
        serde_json::json!({"value": 42, "type": "Int", "diagnostics": []})
    );
}

#[test]
fn a_snippet_on_stdin_with_earlier_cells_as_a_preamble() {
    let root = project("stdin");
    let (ok, doc) = eval(
        &root,
        &["--unit", "demo"],
        "let x = 4\nlet y = double(x)\ny + 1\n",
    );
    assert!(ok, "{doc}");
    assert_eq!(doc["value"], 9);
}

#[test]
fn a_refined_construction_that_fails_is_a_value_not_an_error() {
    let root = project("refined");
    let (ok, doc) = eval(&root, &["--unit", "demo", "--expr", "Percent.of(500)"], "");
    assert!(ok, "{doc}");
    assert_eq!(doc["value"]["kind"], "Err");
    assert_eq!(doc["type"], "Result[Percent, ValidationError]");
}

#[test]
fn a_snippet_that_does_not_check_reports_diagnostics_at_its_own_lines() {
    let root = project("diag");
    let (ok, doc) = eval(
        &root,
        &[
            "--unit",
            "demo",
            "--expr",
            "let a = 1\nlet x: Int = \"no\"\nx",
        ],
        "",
    );
    assert!(!ok);
    assert_eq!(doc["value"], serde_json::Value::Null);
    let d = &doc["diagnostics"][0];
    assert_eq!(d["code"], "bynk.types.let_annotation_mismatch");
    assert_eq!((d["line"].as_u64(), d["col"].as_u64()), (Some(2), Some(14)));
    assert!(d.get("file").is_none(), "{d}");
}

const FETCH: &str = "\
consumes bynk { Fetch, Request, Method, Response }
";

const SEND: &str = "\
let res <- Fetch.send(Request { method: Get, url: \"https://example.com\", contentType: None, authorization: None, headers: Map.empty(), body: None })
res";

#[test]
fn a_capability_needs_a_stub() {
    let root = project("stub");
    let snippet = format!("{FETCH}{SEND}");
    let (ok, doc) = eval(&root, &["--unit", "demo", "--expr", &snippet], "");
    assert!(!ok, "{doc}");
    assert_eq!(doc["error"]["kind"], "runtime");
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(message.contains("`Fetch.send` is not stubbed"), "{message}");
}

#[test]
fn a_stub_in_the_preamble_answers() {
    let root = project("stubbed");
    let stub = "stub Fetch.send(_) returns Ok(Response { status: 204, headers: Map.empty(), body: \"\" })\n";
    let snippet = format!("{FETCH}{stub}{SEND}");
    let (ok, doc) = eval(&root, &["--unit", "demo", "--expr", &snippet], "");
    assert!(ok, "{doc}");
    assert_eq!(doc["value"]["value"]["status"], 204);
}

#[test]
fn a_snippet_that_does_not_end_times_out() {
    let root = project("timeout");
    let (ok, doc) = eval(
        &root,
        &["--unit", "demo", "--timeout", "5", "--expr", "spin(0)"],
        "",
    );
    assert!(!ok);
    assert_eq!(doc["error"]["kind"], "timeout");
}

#[test]
fn unsaved_buffers_are_what_is_evaluated() {
    let root = project("buffers");
    let file = std::path::absolute(root.join("src/demo.bynk")).unwrap();
    let edited = UNIT.replace("n * 2", "n * 7");
    let buffers = serde_json::json!({ file.to_string_lossy(): edited }).to_string();
    let (ok, doc) = eval(
        &root,
        &["--unit", "demo", "--sources-stdin", "--expr", "double(2)"],
        &buffers,
    );
    assert!(ok, "{doc}");
    assert_eq!(doc["value"], 14);
}

#[test]
fn stdin_cannot_be_both_the_buffers_and_the_snippet() {
    let root = project("both");
    let (ok, doc) = eval(&root, &["--unit", "demo", "--sources-stdin"], "{}");
    assert!(!ok);
    assert_eq!(doc["error"]["kind"], "setup");
}

#[test]
fn an_unknown_unit_is_refused() {
    let root = project("unknown");
    let (ok, doc) = eval(&root, &["--unit", "nope", "--expr", "1"], "");
    assert!(!ok);
    assert_eq!(doc["error"]["kind"], "setup");
}

fn write_suite(root: &Path, text: &str) {
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(root.join("tests/demo.bynk"), text).unwrap();
}

#[test]
fn the_projects_own_suites_are_left_out_by_default() {
    let root = project("own_suites");
    write_suite(
        &root,
        "suite demo {\n  case \"broken\" {\n    let x: Int = \"no\"\n    expect x == 1\n  }\n}\n",
    );
    let (ok, doc) = eval(&root, &["--unit", "demo", "--expr", "double(21)"], "");
    assert!(ok, "{doc}");
    assert_eq!(doc["value"], 42);
}

#[test]
fn a_named_suite_is_built_in_so_its_stubs_apply() {
    let root = project("named");
    write_suite(
        &root,
        "suite demo {\n  consumes bynk { Fetch, Request, Method, Response }\n  stub Fetch.send(_) returns Ok(Response { status: 204, headers: Map.empty(), body: \"\" })\n}\n",
    );
    let snippet = format!("{FETCH}{SEND}");
    let (ok, doc) = eval(
        &root,
        &["--unit", "demo", "--suite", "demo", "--expr", &snippet],
        "",
    );
    assert!(ok, "{doc}");
    assert_eq!(doc["value"]["value"]["status"], 204);
}

#[test]
fn a_named_suite_that_does_not_compile_is_reported() {
    let root = project("named_broken");
    write_suite(
        &root,
        "suite demo {\n  case \"b\" {\n    let x: Int = \"no\"\n  }\n}\n",
    );
    let (ok, doc) = eval(
        &root,
        &["--unit", "demo", "--suite", "demo", "--expr", "1"],
        "",
    );
    assert!(!ok);
    assert!(
        doc["diagnostics"][0]["file"]
            .as_str()
            .unwrap()
            .ends_with("demo.bynk"),
        "{doc}"
    );
}

#[test]
fn a_suite_that_is_not_the_unit_is_refused() {
    let root = project("other_suite");
    let (ok, doc) = eval(
        &root,
        &["--unit", "demo", "--suite", "other", "--expr", "1"],
        "",
    );
    assert!(!ok);
    assert_eq!(doc["error"]["kind"], "setup");
    assert!(doc.get("value").is_none(), "{doc}");
}

#[test]
fn dependencies_that_cannot_be_prepared_still_print_a_document() {
    let root = project("deps");
    // A git dependency that is nowhere, offline.
    let manifest = "[project]\nname = \"p\"\n\n[dependencies]\nghost = { git = \"https://invalid.invalid/ghost.git\", rev = \"main\" }\n";
    std::fs::write(root.join("bynk.toml"), manifest).unwrap();
    let (ok, doc) = eval(&root, &["--unit", "demo", "--offline", "--expr", "1"], "");
    assert!(!ok);
    assert_eq!(doc["error"]["kind"], "setup", "{doc}");
}
