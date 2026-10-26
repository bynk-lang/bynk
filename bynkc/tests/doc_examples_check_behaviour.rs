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

#[test]
fn the_text_output_names_the_fence_too() {
    let root = project("text", UNIT);
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("test")
        .arg(&root)
        .arg("--output")
        .arg(root.join("out"))
        .env("NO_COLOR", "1")
        .output()
        .expect("bynkc runs");
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains("bynk.types.let_annotation_mismatch"),
        "{text}"
    );
    assert!(text.contains("calc.bynk:5:"), "{text}");
    assert!(!text.contains(".doc-examples.bynk"), "{text}");
}

#[test]
fn an_explicit_paths_layout_checks_examples_too() {
    let root = project("paths", UNIT);
    std::fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"p\"\n\n[paths]\ninclude = [\"src\"]\n",
    )
    .unwrap();
    let (ok, text) = bynkc_test(&root, "json");
    assert!(!ok, "{text}");
    assert!(text.contains("calc.bynk:5:"), "{text}");
}
