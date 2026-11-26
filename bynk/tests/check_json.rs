//! #4123: `bynk check --format json` and `--format ndjson`, over the real binary.
//!
//! The schema is a pinned surface: one project that exercises every part of it
//! (an error, a warning and a hint; a label in another file; a machine and a
//! suggested fix; a UTF-16 column that is not a character count) is goldened in
//! both shapes. The rest pins what a CI consumer relies on around the document:
//! the exit status is `rich`'s, diagnostics are the whole of stdout and stderr
//! stays empty, a clean project is an empty document, and a run that cannot
//! start writes nothing to stdout.
//!
//! Goldens are blessed with `BYNK_BLESS=1 cargo test -p bynk`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bynk() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("check-json")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// `bynk <args>` in `cwd`: (exit code, stdout, stderr).
fn run(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bynk())
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn bless_or_assert(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("BYNK_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden {}; regenerate with BYNK_BLESS=1 cargo test -p bynk",
            path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "golden {name} drifted; re-bless with BYNK_BLESS=1 cargo test -p bynk"
    );
}

const MANIFEST: &str = "[package]\nname = \"acme/shop\"\n";

/// A type error with an astral character and an accented one before it on the
/// same line, so the UTF-16 column differs from both the character column and
/// the byte offset.
const GLYPH: &str = "commons glyph\n\nfn tag() -> String { \"🦀é\" + 1 }\n";

/// A comparison of a type that does not declare `derives Ord`: its fix is a
/// complete edit that changes meaning, so it is `suggested`, not `machine`.
const ORDER: &str = "\
commons order

type Point = { x: Int, y: Int }

fn before(a: Point, b: Point) -> Bool {
    a < b
}
";

/// A `---` documentation block a blank line separates from what follows: a
/// warning, which does not fail the run.
const WARN: &str = "\
commons warn

---
orphaned by the blank line below
---

fn useIt() -> Int {
  1
}
";

/// A nested `match` staircase, offered only as a hint.
const STAIRS: &str = "\
commons stairs

fn read(a: Option[Int], b: Option[Int]) -> Result[Int, String] {
  match a {
    None => Err(\"a\"),
    Some(x) => match b {
      None => Err(\"b\"),
      Some(y) => Ok(x + y),
    },
  }
}
";

/// The handler uses `Beta` without listing it: a machine-applicable fix.
const RELAY: &str = "\
context relay

capability Beta {
  fn ping() -> Effect[Int]
}

provides Beta = BetaImpl {
  fn ping() -> Effect[Int] {
    2
  }
}

agent Counter {
  key name: String

  store hits: Cell[Int]

  on call ping() -> Effect[Int] {
    Beta.ping()
  }
}
";

fn money(n: u8) -> String {
    format!("commons money\n\nfn one() -> Int {{\n\t{n}\n}}\n")
}

/// The project behind the goldens.
fn kitchen_sink(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("bynk.toml"), MANIFEST);
    write(&root.join("src/glyph.bynk"), GLYPH);
    write(&root.join("src/order.bynk"), ORDER);
    write(&root.join("src/warn.bynk"), WARN);
    write(&root.join("src/stairs.bynk"), STAIRS);
    write(&root.join("src/relay.bynk"), RELAY);
    // Two files of one unit: the duplicate's label lies in the other file.
    write(&root.join("src/money/a.bynk"), &money(1));
    write(&root.join("src/money/b.bynk"), &money(2));
    root
}

fn clean_project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("bynk.toml"), MANIFEST);
    write(&root.join("src/ok.bynk"), &money(1).replace("money", "ok"));
    root
}

#[test]
fn json_golden() {
    let root = kitchen_sink("json-golden");
    let (code, out, err) = run(&root, &["check", "--format", "json"]);
    assert_eq!(code, 1, "{err}");
    assert_eq!(err, "", "diagnostics are stdout's alone");
    bless_or_assert("check-json-project.json", &out);
}

#[test]
fn ndjson_golden() {
    let root = kitchen_sink("ndjson-golden");
    let (code, out, err) = run(&root, &["check", "--format", "ndjson"]);
    assert_eq!(code, 1, "{err}");
    assert_eq!(err, "", "diagnostics are stdout's alone");
    bless_or_assert("check-ndjson-project.ndjson", &out);
}

#[test]
fn the_exit_status_is_the_one_rich_and_short_give() {
    let root = kitchen_sink("exit-status");
    for format in ["rich", "short", "json", "ndjson"] {
        let (code, _, _) = run(&root, &["check", "--format", format]);
        assert_eq!(code, 1, "{format} on a project with errors");
    }
    let clean = clean_project("exit-status-clean");
    for format in ["rich", "short", "json", "ndjson"] {
        let (code, _, err) = run(&clean, &["check", "--format", format]);
        assert_eq!(code, 0, "{format} on a clean project: {err}");
    }
}

#[test]
fn a_clean_project_is_an_empty_document_and_a_lone_summary_line() {
    let root = clean_project("clean");
    let (_, json, _) = run(&root, &["check", "--format", "json"]);
    let doc: serde_json::Value = serde_json::from_str(&json).expect("one JSON document");
    assert_eq!(doc["version"], 1);
    assert_eq!(doc["diagnostics"], serde_json::json!([]));
    assert_eq!(
        doc["summary"],
        serde_json::json!({ "errors": 0, "warnings": 0, "hints": 0 })
    );
    let (_, nd, _) = run(&root, &["check", "--format", "ndjson"]);
    assert_eq!(
        nd,
        "{\"type\":\"summary\",\"version\":1,\"errors\":0,\"warnings\":0,\"hints\":0}\n"
    );
}

#[test]
fn warnings_and_hints_are_in_the_document_and_do_not_fail_the_run() {
    let root = scratch("warnings-only");
    write(&root.join("bynk.toml"), MANIFEST);
    write(&root.join("src/warn.bynk"), WARN);
    write(&root.join("src/stairs.bynk"), STAIRS);
    let (code, out, err) = run(&root, &["check", "--format", "json"]);
    assert_eq!(code, 0, "{err}");
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let severities: Vec<&str> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["severity"].as_str().unwrap())
        .collect();
    assert!(severities.contains(&"warning"), "{out}");
    assert!(severities.contains(&"hint"), "{out}");
    assert_eq!(doc["summary"]["errors"], 0);
    // A terminal hides the hint; the machine format does not.
    let (_, _, shown) = run(&root, &["check"]);
    assert!(!shown.contains("let_else_staircase"), "{shown}");
}

#[test]
fn diagnostics_are_in_file_then_position_order() {
    let root = kitchen_sink("ordering");
    let (_, out, _) = run(&root, &["check", "--format", "json"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let keys: Vec<(String, u64)> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["file"].as_str().unwrap().to_string(),
                d["start"]["byteOffset"].as_u64().unwrap(),
            )
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "{out}");
    // Shown as typed from the working directory, with `/` on every platform.
    assert!(keys.iter().all(|(f, _)| f.starts_with("src/")), "{keys:?}");
}

#[test]
fn columns_count_utf16_units_and_the_byte_offset_counts_bytes() {
    let root = kitchen_sink("utf16");
    let (_, out, _) = run(&root, &["check", "--format", "json"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let glyph = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["file"] == "src/glyph.bynk")
        .expect("a diagnostic in glyph.bynk");
    // The error spans the literal `"🦀é"`: five UTF-16 units (the crab is two),
    // against four characters and eight bytes.
    let (start, end) = (&glyph["start"], &glyph["end"]);
    let bytes = start["byteOffset"].as_u64().unwrap() as usize
        ..end["byteOffset"].as_u64().unwrap() as usize;
    let text = &GLYPH[bytes];
    assert_eq!(text, "\"🦀é\"");
    let width = end["column"].as_u64().unwrap() - start["column"].as_u64().unwrap();
    assert_eq!(width, 5, "UTF-16 code units");
    assert_eq!(text.chars().count(), 4);
    assert_eq!(text.len(), 8);
    assert_eq!(start["line"], 3);
}

#[test]
fn a_label_in_another_file_names_that_file() {
    let root = kitchen_sink("label-file");
    let (_, out, _) = run(&root, &["check", "--format", "json"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let dup = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "bynk.resolve.duplicate_fn")
        .expect("the duplicate function");
    assert_eq!(dup["file"], "src/money/b.bynk");
    assert_eq!(dup["labels"][0]["file"], "src/money/a.bynk");
    assert_eq!(dup["labels"][0]["message"], "previously declared here");
}

#[test]
fn a_fix_says_whether_a_machine_may_apply_it() {
    let root = kitchen_sink("fix-applicability");
    let (_, out, _) = run(&root, &["check", "--format", "json"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let applicability_of = |code: &str| {
        let d = doc["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["code"] == code)
            .unwrap_or_else(|| panic!("no {code} in {out}"));
        d["fixes"][0]["applicability"].as_str().unwrap().to_string()
    };
    assert_eq!(
        applicability_of("bynk.given.undeclared_capability"),
        "machine"
    );
    assert_eq!(applicability_of("bynk.types.not_orderable"), "suggested");
}

#[test]
fn a_workspace_is_one_document_with_paths_from_where_it_ran() {
    let root = scratch("workspace");
    write(
        &root.join("bynk.toml"),
        "[workspace]\nmembers = [\"a\", \"b\"]\n",
    );
    for member in ["a", "b"] {
        write(
            &root.join(member).join("bynk.toml"),
            &format!("[package]\nname = \"acme/{member}\"\n"),
        );
        write(
            &root.join(member).join(format!("src/{member}.bynk")),
            &format!("commons {member}\n\nfn f() -> Int {{ true }}\n"),
        );
    }
    let (code, out, err) = run(&root, &["check", "--format", "json"]);
    assert_eq!(code, 1, "{err}");
    assert_eq!(err, "", "no member headers in a machine format: {err}");
    let doc: serde_json::Value = serde_json::from_str(&out).expect("one document");
    let files: Vec<&str> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["file"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["a/src/a.bynk", "b/src/b.bynk"], "{out}");
    assert_eq!(doc["summary"]["errors"], 2);
}

#[test]
fn a_single_file_checks_to_the_same_shape() {
    let root = scratch("single-file");
    write(&root.join("order.bynk"), ORDER);
    let (code, out, err) = run(&root, &["check", "--format", "json", "order.bynk"]);
    assert_eq!(code, 1, "{err}");
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["diagnostics"][0]["code"], "bynk.types.not_orderable");
    assert_eq!(doc["diagnostics"][0]["file"], "order.bynk");
}

#[test]
fn a_run_that_cannot_start_writes_nothing_to_stdout() {
    let root = scratch("cannot-start");
    for format in ["json", "ndjson"] {
        let (code, out, err) = run(&root, &["check", "--format", format, "missing.bynk"]);
        assert_eq!(code, 1, "{err}");
        assert_eq!(
            out, "",
            "{format}: no document for a run that never started"
        );
        assert!(err.contains("missing.bynk"), "{err}");
    }
}

#[test]
fn emit_has_no_machine_format() {
    let root = clean_project("emit");
    for format in ["json", "ndjson"] {
        let (code, out, err) = run(&root, &["check", "--emit", "--format", format]);
        assert_eq!(code, 1);
        assert_eq!(out, "");
        assert!(err.contains("`--emit`") && err.contains("tsc"), "{err}");
    }
}
