//! Back-half paydown 1B.3a (#4047, Decision C): the spliced test-runtime artifacts reach the
//! emitter only through the generated `test_runtime_modules.rs` table.
//!
//! The artifacts (`bynk-emit/src/testgen/test_runtime/*.ts`) are written by `bundle.mjs` from the
//! modules under `bynk-emit/runtime/src/testing/`. If a Rust file `include_str!`ed one directly,
//! it would bypass the table the harness's `TestRuntimeModule` is checked against, and the
//! dependency order and the type-check guard would not cover it.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits in the repo root")
        .to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The path with `/` separators, so a comparison holds on Windows.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Whether `source` reads a `test_runtime/` artifact with `include_str!`.
fn reads_an_artifact(source: &str) -> bool {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .any(|line| line.contains("include_str!") && line.contains("test_runtime/"))
}

#[test]
fn only_the_generated_table_includes_a_test_runtime_artifact() {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("bynk-emit/src"), &mut files);
    assert!(!files.is_empty(), "no Rust under bynk-emit/src");
    let mut table_reads = 0;
    let mut stray = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).unwrap();
        let name = slashed(file.strip_prefix(&root).unwrap_or(file));
        // The `include_str!` may be on its own line in a multi-line call.
        let multi = source.contains("include_str!(\n") && source.contains("test_runtime/");
        if name.ends_with("bynk-emit/src/testgen/test_runtime_modules.rs") {
            table_reads += source.matches("include_str!(\"test_runtime/").count();
        } else if reads_an_artifact(&source) || multi {
            stray.push(name);
        }
    }
    assert!(table_reads > 0, "the generated table reads no artifact");
    assert!(
        stray.is_empty(),
        "these files read a test-runtime artifact without the generated table:\n{}",
        stray.join("\n")
    );
}

#[test]
fn the_recognition_ignores_comments_and_other_includes() {
    assert!(reads_an_artifact(
        "let t = include_str!(\"test_runtime/clock.ts\");"
    ));
    assert!(!reads_an_artifact(
        "// include_str!(\"test_runtime/clock.ts\")"
    ));
    assert!(!reads_an_artifact(
        "let t = include_str!(\"other/clock.ts\");"
    ));
}
