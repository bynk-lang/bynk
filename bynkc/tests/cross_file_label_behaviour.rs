//! A diagnostic's secondary label that lies in a sibling file of a multi-file
//! unit renders in a snippet of *that* file. Before, the renderer drew every
//! label against the diagnostic's own file, so a duplicate service's
//! "previously declared here" — declared in `a_web.bynk` — was underlined in
//! `z_more.bynk`'s snippet, at the same byte offsets.
//!
//! Drives the real `bynkc` CLI on a throwaway project: `check` renders through
//! the project-check printer and `compile` through the project-failure printer.

use std::path::PathBuf;
use std::process::Command;

/// A throwaway on-disk project, removed on drop (including on panic).
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SERVICE: &str = "service api {\n  on call() -> Effect[Int] {\n    Effect.pure(1)\n  }\n}\n";

fn duplicate_service_project(tag: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "bynk_cross_file_label_{tag}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/app")).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    // The service sits on line 3 of `a_web.bynk` and line 6 of `z_more.bynk`.
    std::fs::write(
        dir.join("src/app/a_web.bynk"),
        format!("context app\n\n{SERVICE}"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/app/z_more.bynk"),
        format!("context app\n\n\n\n\n{SERVICE}"),
    )
    .unwrap();
    Scratch(dir)
}

fn run(subcommand: &str, project: &Scratch) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynkc"));
    cmd.arg(subcommand).arg(&project.0);
    if subcommand == "compile" {
        cmd.arg("--output").arg(project.0.join("out"));
    }
    let out = cmd
        .current_dir(&project.0)
        .env("NO_COLOR", "1")
        .output()
        .expect("run bynkc");
    assert!(
        !out.status.success(),
        "a duplicate service must fail `{subcommand}`"
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The report's snippets, each as its `file:line:col` header and body.
fn snippets(report: &str) -> Vec<(&str, &str)> {
    let mut parts: Vec<(&str, &str)> = Vec::new();
    let mut rest = report;
    while let Some(open) = rest.find("─[ ") {
        let after = &rest[open + "─[ ".len()..];
        let close = after.find(" ]").expect("a snippet header closes");
        let header = &after[..close];
        let body = &after[close..];
        let end = body.find("─[ ").unwrap_or(body.len());
        parts.push((header, &body[..end]));
        rest = &body[end..];
    }
    parts
}

fn assert_label_in_its_own_file(subcommand: &str) {
    let project = duplicate_service_project(subcommand);
    let stderr = run(subcommand, &project);
    let report = stderr
        .split("[bynk.resolve.duplicate_service]")
        .nth(1)
        .unwrap_or_else(|| panic!("expected a duplicate_service report, got:\n{stderr}"));
    let report = report.split("\n[bynk.").next().unwrap();
    let parts = snippets(report);
    assert_eq!(parts.len(), 2, "one snippet per file, got:\n{report}");
    let (primary, primary_body) = parts[0];
    let (sibling, sibling_body) = parts[1];
    assert!(
        primary.ends_with("src/app/z_more.bynk:6:9"),
        "got:\n{report}"
    );
    assert!(
        !primary_body.contains("previously declared here"),
        "the label must not be drawn in z_more.bynk's snippet, got:\n{report}"
    );
    assert!(
        sibling.ends_with("src/app/a_web.bynk:3:9"),
        "got:\n{report}"
    );
    assert!(
        sibling_body.contains(" 3 │ service api {")
            && sibling_body.contains("previously declared here"),
        "the label must be drawn on a_web.bynk's line 3, got:\n{report}"
    );
}

#[test]
fn check_renders_a_sibling_files_label_in_that_file() {
    assert_label_in_its_own_file("check");
}

#[test]
fn compile_renders_a_sibling_files_label_in_that_file() {
    assert_label_in_its_own_file("compile");
}

/// #1956: a label into a first-party unit renders in a snippet of that unit's
/// own source. `headers` is declared on `Request` in `bynk-std`'s `bynk.bynk`;
/// its span carried no file identity, so it counted as the error's own file and
/// was drawn at the same byte offsets in the user's — underlining unrelated
/// code whenever the file was long enough to contain them.
fn missing_headers_project(tag: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "bynk_firstparty_label_{tag}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/app")).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let padding: String = (1..=150)
        .map(|i| format!("// padding line {i}\n"))
        .collect();
    let web = format!(
        "context app\n\nuses app.util\nconsumes bynk {{ Fetch }}\n\n{padding}\
service api from http {{
  on GET(\"/\") () -> Effect[HttpResult[String]] by Visitor given Fetch {{
    let res <- Fetch.send(Request {{
      method: Get, url: \"https://example.com\",
      contentType: None, authorization: None,
      body: None,
    }})
    Ok(greeting())
  }}
}}
"
    );
    // The issue's shape: the declaration's offset in `bynk.bynk` lies inside
    // this file, so drawing it here would underline real (unrelated) text.
    let decl = STDLIB.find("headers: Map").unwrap();
    assert!(
        decl < web.len(),
        "web.bynk must be long enough to contain the stray offset"
    );
    std::fs::write(dir.join("src/app/web.bynk"), web).unwrap();
    std::fs::write(
        dir.join("src/app/util.bynk"),
        "commons app.util\n\nfn greeting() -> String { \"hi\" }\n",
    )
    .unwrap();
    Scratch(dir)
}

const STDLIB: &str = include_str!("../../bynk-std/lib/core/bynk.bynk");

fn assert_firstparty_label_in_its_own_source(subcommand: &str) {
    let project = missing_headers_project(subcommand);
    let stderr = run(subcommand, &project);
    let report = stderr
        .split("[bynk.resolve.missing_field]")
        .nth(1)
        .unwrap_or_else(|| panic!("expected a missing_field report, got:\n{stderr}"));
    let report = report.split("\n[bynk.").next().unwrap();
    let parts = snippets(report);
    assert_eq!(parts.len(), 2, "one snippet per file, got:\n{report}");
    let (primary, primary_body) = parts[0];
    let (stdlib, stdlib_body) = parts[1];
    assert!(
        primary.ends_with("src/app/web.bynk:158:27"),
        "got:\n{report}"
    );
    assert!(
        !primary_body.contains("field declared here"),
        "the label must not be drawn in web.bynk's snippet, got:\n{report}"
    );
    // Line and column of the declaration in `bynk.bynk`, counted in chars: the
    // header must agree with the snippet even after the file's non-ASCII text.
    let decl = STDLIB.find("headers: Map").unwrap();
    let line = STDLIB[..decl].matches('\n').count() + 1;
    let col = STDLIB[..decl].rsplit('\n').next().unwrap().chars().count() + 1;
    assert_eq!(stdlib, format!("bynk.bynk:{line}:{col}"), "got:\n{report}");
    assert!(
        stdlib_body.contains(&format!(" {line} │ "))
            && stdlib_body.contains("headers: Map[String, String],")
            && stdlib_body.contains("field declared here"),
        "the label must be drawn on bynk.bynk's `headers` line, got:\n{report}"
    );
}

#[test]
fn check_renders_a_firstparty_label_in_that_units_source() {
    assert_firstparty_label_in_its_own_source("check");
}

#[test]
fn compile_renders_a_firstparty_label_in_that_units_source() {
    assert_firstparty_label_in_its_own_source("compile");
}
