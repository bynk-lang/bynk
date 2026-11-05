//! Every operation of every first-party binding behaves to its contract, on every
//! platform (standard-library S2, #1952).
//!
//! The tests themselves are `node --test` files beside the bindings, in
//! `bynk-std/test/`: one `<binding>.test.mjs` per binding, with the surface
//! bindings sharing `surface.mjs`. This driver compiles each platform's
//! `stdlib_scaffold` to JavaScript, stages those files at the output root (where
//! the bindings' own imports resolve), and runs them.
//!
//! It then enforces coverage. Every test is named `<binding> <Provider>.<method>:
//! <claim>`, and every method of every `export class … implements <Capability>`
//! in a binding's source must have at least one passing test of that name. An
//! operation added without a test fails here (R-13).
//!
//! Skips loudly without `node`; `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure, as the other Node-driven suites do.

mod require;
mod stdlib_scaffold;
mod tools;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

/// `bynk-std/test/`, where the behavioural tests live.
fn test_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../bynk-std/test")
}

/// `(Provider, method)` for every public method of every exported provider
/// class in a binding's TypeScript source.
fn operations(source: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut class: Option<String> = None;
    for line in source.lines() {
        if let Some(rest) = line.strip_prefix("export class ") {
            class = rest
                .split_whitespace()
                .next()
                .filter(|_| rest.contains(" implements "))
                .map(str::to_string);
            continue;
        }
        let Some(class) = &class else { continue };
        if let Some(rest) = line.strip_prefix("  async ") {
            let method: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !method.is_empty() {
                out.push((class.clone(), method));
            }
        }
    }
    out
}

/// The names of the passing and failing tests in a TAP report.
fn tap_results(tap: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut passed = BTreeSet::new();
    let mut failed = BTreeSet::new();
    for line in tap.lines() {
        if let Some(rest) = line.strip_prefix("not ok ") {
            if let Some((_, name)) = rest.split_once(" - ") {
                failed.insert(name.trim().to_string());
            }
        } else if let Some(rest) = line.strip_prefix("ok ")
            && let Some((_, name)) = rest.split_once(" - ")
        {
            passed.insert(name.trim().to_string());
        }
    }
    (passed, failed)
}

#[test]
fn every_binding_operation_is_tested_and_passes() {
    if !tools::tool_exists("node") {
        tools::skip(
            "BINDING BEHAVIOUR",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "`node` is not on PATH",
        );
        return;
    }
    let dir = stdlib_scaffold::scratch("binding-behaviour");
    let shared: Vec<PathBuf> = fs::read_dir(test_dir())
        .expect("bynk-std/test exists")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "mjs"))
        .collect();

    let mut uncovered = Vec::new();
    for platform in stdlib_scaffold::PLATFORMS {
        let out = stdlib_scaffold::compile(platform, &dir, true);
        for file in &shared {
            fs::copy(file, out.join(file.file_name().unwrap())).unwrap();
        }
        let bindings = stdlib_scaffold::bindings(platform);
        let test_files: Vec<String> = bindings
            .iter()
            .map(|(path, _)| format!("{}.test.mjs", stdlib_scaffold::stem(path)))
            .collect();
        for file in &test_files {
            assert!(
                test_dir().join(file).is_file(),
                "[{}] no behavioural tests for this binding: add bynk-std/test/{file}",
                platform.as_str()
            );
        }

        let run = tools::base_command("node")
            .args(["--test", "--test-reporter=tap"])
            .args(&test_files)
            .current_dir(&out)
            .output()
            .expect("run node --test");
        let tap = String::from_utf8_lossy(&run.stdout);
        let (passed, failed) = tap_results(&tap);
        assert!(
            run.status.success() && failed.is_empty(),
            "[{}] binding tests failed: {failed:?}\n{tap}\n{}",
            platform.as_str(),
            String::from_utf8_lossy(&run.stderr)
        );

        for (path, source) in &bindings {
            let stem = stdlib_scaffold::stem(path);
            let ops = operations(source);
            assert!(
                !ops.is_empty(),
                "[{}] found no operations in {path}",
                platform.as_str()
            );
            for (provider, method) in ops {
                let prefix = format!("{stem} {provider}.{method}:");
                if !passed.iter().any(|name| name.starts_with(&prefix)) {
                    uncovered.push(format!("[{}] {prefix}", platform.as_str()));
                }
            }
        }
    }
    assert!(
        uncovered.is_empty(),
        "these binding operations have no passing test (name a test `<binding> \
         <Provider>.<method>: …` in bynk-std/test/):\n{}",
        uncovered.join("\n")
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The operation scan reads the shapes the bindings use, and nothing else: a
/// private helper, a constructor and a field are not operations.
#[test]
fn operations_are_the_public_provider_methods() {
    let source = "export class WorkersKv implements Kv {\n  private env?: unknown;\n  constructor(env?: unknown) {}\n  private ns(): X {}\n  async get(key: string): Promise<X> {}\n  async dedup<T>(key: string): Promise<X> {}\n}\nconst HELPER = 1;\nexport class Unrelated {\n  async ignored(): Promise<void> {}\n}\n";
    assert_eq!(
        operations(source),
        [
            ("WorkersKv".to_string(), "get".to_string()),
            ("WorkersKv".to_string(), "dedup".to_string()),
        ]
    );
    let (passed, failed) = tap_results(
        "TAP version 13\n# Subtest: a b.c: d\nok 1 - a b.c: d\n  ---\n  duration_ms: 1\n  ...\nnot ok 2 - e f.g: h\n1..2\n",
    );
    assert_eq!(passed, BTreeSet::from(["a b.c: d".to_string()]));
    assert_eq!(failed, BTreeSet::from(["e f.g: h".to_string()]));
}
