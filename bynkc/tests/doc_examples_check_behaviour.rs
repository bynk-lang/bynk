//! #2461: `bynkc test` type-checks the fenced `bynk` examples in doc comments.
//! A broken example fails the run with a diagnostic at the offending line of
//! the fence; an `ignore` fence, a fence in another language, and a one-line
//! doc are left alone. A passing example costs the run nothing: the fixture
//! `2461_doc_examples_checked` pins that the examples are checked, not run.

use std::path::PathBuf;
use std::process::Command;

const UNIT: &str = "\
---
Doubles a number.

```bynk
let x: Int = \"oops\"
expect double(2) == 4
```

```bynk ignore
let y: Int = \"ignored\"
```

```ts
const z: number = \"not bynk\";
```
---
commons demo.calc

fn double(n: Int) -> Int {
  n * 2
}
";

const SUITE: &str = "\
suite demo.calc {
  case \"double doubles\" {
    expect double(21) == 42
  }
}
";

fn project(tag: &str, unit: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("doc-examples-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src/demo")).unwrap();
    std::fs::create_dir_all(root.join("src/tests")).unwrap();
    std::fs::write(root.join("bynk.toml"), "[project]\nname = \"p\"\n").unwrap();
    std::fs::write(root.join("src/demo/calc.bynk"), unit).unwrap();
    std::fs::write(root.join("src/tests/calc.test.bynk"), SUITE).unwrap();
    root
}

fn bynkc_test(root: &PathBuf, format: &str) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["test", "--no-run", "--format", format])
        .arg(root)
        .arg("--output")
        .arg(root.join("out"))
        .env("NO_COLOR", "1")
        .output()
        .expect("bynkc runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn a_broken_example_fails_the_run_at_the_fence() {
    let root = project("broken", UNIT);
    let (ok, text) = bynkc_test(&root, "json");
    assert!(!ok, "a broken example must fail the run: {text}");
    assert!(
        text.contains("bynk.types.let_annotation_mismatch"),
        "{text}"
    );
    // Line 5 is the `let x: Int = "oops"` inside the fence; the diagnostic
    // names the real file and that line, not the synthetic suite.
    assert!(text.contains("calc.bynk:5:"), "{text}");
    assert!(!text.contains(".doc-examples.bynk"), "{text}");
    // The `ignore` and `ts` fences never reach the checker.
    assert!(!text.contains("ignored"), "{text}");
}

#[test]
fn a_correct_example_passes_and_is_not_a_case() {
    let unit = UNIT.replace("let x: Int = \"oops\"", "let x: Int = 4");
    let root = project("fixed", &unit);
    let (ok, text) = bynkc_test(&root, "json");
    assert!(ok, "{text}");
    assert!(
        !text.contains("doc example"),
        "doc examples are checked, not discovered as cases: {text}"
    );
    assert!(text.contains("double doubles"), "{text}");
}

#[test]
fn a_declaration_fence_is_checked_as_declarations() {
    let unit = UNIT
        .replace("let x: Int = \"oops\"", "let x: Int = 4")
        .replace(
            "```ts",
            "```bynk\nfn doubled(n: Int) -> Int { double(n) }\n```\n\n```ts",
        );
    let ok_root = project("items-ok", &unit);
    let (ok, text) = bynkc_test(&ok_root, "json");
    assert!(ok, "{text}");

    let broken = unit.replace("{ double(n) }", "{ double(n) + \"x\" }");
    let root = project("items-broken", &broken);
    let (ok, text) = bynkc_test(&root, "json");
    assert!(!ok, "{text}");
    assert!(text.contains("bynk.types.type_mismatch"), "{text}");
    // The `fn doubled` line is line 14 of the file, inside the fence.
    assert!(text.contains("calc.bynk:14:"), "{text}");
}

#[test]
fn a_fence_that_is_neither_statements_nor_declarations_is_a_parse_error_at_the_fence() {
    let unit = UNIT.replace("let x: Int = \"oops\"", "this is nonsense (");
    let root = project("nonsense", &unit);
    let (ok, text) = bynkc_test(&root, "json");
    assert!(!ok, "{text}");
    assert!(text.contains("bynk.parse."), "{text}");
    // The fence is lines 4-7; the parse error is reported inside it.
    assert!(text.contains("calc.bynk:6:"), "{text}");
}

#[test]
fn a_declaration_fence_in_a_context_is_not_checked_but_statements_are() {
    let root = project("context", UNIT);
    std::fs::write(
        root.join("src/demo/shop.bynk"),
        "---\n```bynk\nfn total(o: Order) -> Int { o.cents }\n```\n\n```bynk\nlet x: Int = \"oops\"\n```\n---\ncontext demo.shop\n\ntype Order = { cents: Int }\n",
    )
    .unwrap();
    let unit = UNIT.replace("let x: Int = \"oops\"", "let x: Int = 4");
    std::fs::write(root.join("src/demo/calc.bynk"), unit).unwrap();
    let (ok, text) = bynkc_test(&root, "json");
    assert!(!ok, "{text}");
    // Only the statement fence is reported; `Order` is not a false error.
    assert!(text.contains("shop.bynk:7:"), "{text}");
    assert!(!text.contains("Order"), "{text}");
}

/// A `bynk test` fence is run, so these need the runner `behaviour_fixtures.rs`
/// does: skipped on Windows, and where no `tsc` and `node` are on the path.
fn can_run() -> bool {
    if cfg!(windows) {
        eprintln!("skipping on Windows: `bynkc test` runner detection is Unix-only");
        return false;
    }
    let on_path = |name: &str| {
        std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
    };
    if !(on_path("node") && (on_path("tsc") || on_path("tsx"))) {
        eprintln!("skipping: no TypeScript runner on PATH");
        return false;
    }
    true
}

fn run_json(root: &PathBuf) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["test", "--format", "json"])
        .arg(root)
        .arg("--output")
        .arg(root.join("out"))
        .env("NO_COLOR", "1")
        .output()
        .expect("bynkc runs");
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "no JSON document ({e}): {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

#[test]
fn a_test_fence_runs_and_a_failure_names_the_real_line() {
    if !can_run() {
        return;
    }
    // Line 4 is a passing `bynk test` fence; line 8 a failing one (its
    // `expect` is on line 10); line 13 is checked but not run.
    let unit = "\
---
Doubles a number.

```bynk test
expect double(2) == 4
```

```bynk test
let n = double(3)
expect n == 7
```

```bynk
expect double(2) == 5
```
---
commons demo.calc

fn double(n: Int) -> Int {
  n * 2
}
";
    let root = project("run", unit);
    let doc = run_json(&root);
    assert_eq!(doc["passed"], 2, "{doc}");
    assert_eq!(doc["failed"], 1, "{doc}");
    let cases: Vec<&serde_json::Value> = doc["suites"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["cases"].as_array().unwrap())
        .collect();
    let named = |name: &str| {
        cases
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no case {name}: {doc}"))
    };
    assert_eq!(named("doc example calc.bynk:4")["outcome"], "pass");
    let failed = named("doc example calc.bynk:8");
    assert_eq!(failed["outcome"], "fail");
    // The failing `expect` is reported in the real file, at its own line.
    assert_eq!(failed["location"]["path"], "src/demo/calc.bynk", "{failed}");
    assert_eq!(failed["location"]["line"], 10, "{failed}");
    assert!(
        !failed.to_string().contains("doc-examples"),
        "the synthetic file never shows: {failed}"
    );
    // The checked-only example is not a case.
    assert!(
        !cases
            .iter()
            .any(|c| c["name"] == "doc example calc.bynk:13")
    );
}

#[test]
fn discovery_lists_a_test_fence_at_its_fence() {
    let unit = UNIT
        .replace("let x: Int = \"oops\"", "let x: Int = 4")
        .replace("```bynk\nlet x", "```bynk test\nlet x");
    let root = project("discover", &unit);
    let (ok, text) = bynkc_test(&root, "json");
    assert!(ok, "{text}");
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let case = doc["suites"][0]["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "doc example calc.bynk:4")
        .unwrap_or_else(|| panic!("no doc case: {text}"));
    assert_eq!(case["location"]["path"], "src/demo/calc.bynk", "{case}");
    assert_eq!(case["location"]["line"], 4, "{case}");
}
