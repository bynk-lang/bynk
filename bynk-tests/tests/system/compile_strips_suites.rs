//! #1821 (ADR 0147 D3): `bynk compile` strips every `suite` — never
//! type-checked for the build, never emitted to the deployable — while
//! `bynk test` compiles and runs them. `compile` used to emit `out/tests/`, and
//! a project with a `system` suite then failed `tsc` on a bundle build (its
//! test module imports the workers layout).

use std::path::{Path, PathBuf};
use std::process::Command;

fn project(tag: &str, suite: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("strip-suites-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src/shop")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"m\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/shop/maths.bynk"),
        "commons shop.maths\n\nfn double(n: Int) -> Int { n + n }\n",
    )
    .unwrap();
    std::fs::write(root.join("tests/maths.bynk"), suite).unwrap();
    root
}

fn bynk(cwd: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .output()
        .expect("bynk runs");
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn compile_emits_no_suite() {
    let root = project(
        "emits",
        "suite shop.maths {\n  case \"doubles\" { expect double(3) == 6 }\n}\n",
    );
    for target in ["bundle", "workers"] {
        let out = format!("out-{target}");
        let (ok, text) = bynk(&root, &["compile", "--target", target, "-o", &out, "."]);
        assert!(ok, "{target}: {text}");
        assert!(
            !root.join(&out).join("tests").exists(),
            "{target}: compile emitted `tests/`"
        );
    }
}

#[test]
fn a_broken_suite_does_not_fail_compile_but_check_reports_it() {
    let root = project(
        "broken",
        "suite shop.maths {\n  case \"x\" { expect nosuch(3) == 6 }\n}\n",
    );
    let (ok, text) = bynk(&root, &["compile", "-o", "out", "."]);
    assert!(ok, "the build never type-checks a suite: {text}");
    let (ok, text) = bynk(&root, &["check", "--format", "short", "."]);
    assert!(!ok, "check still checks suites");
    assert!(text.contains("bynk.resolve.unknown_function"), "{text}");
}

/// ADR 0147 D5: a unit and its suite in one file. The unit is emitted; the
/// suite is not.
#[test]
fn an_atomic_file_ships_its_unit_and_drops_its_suite() {
    let root = project("atomic", "");
    std::fs::remove_file(root.join("tests/maths.bynk")).unwrap();
    std::fs::write(
        root.join("src/shop/maths.bynk"),
        "commons shop.maths {\n  fn double(n: Int) -> Int { n + n }\n}\n\n\
         suite shop.maths {\n  case \"doubles\" { expect double(3) == 6 }\n}\n",
    )
    .unwrap();
    let (ok, text) = bynk(&root, &["compile", "-o", "out", "."]);
    assert!(ok, "{text}");
    assert!(
        root.join("out/shop/maths.ts").exists(),
        "the commons is emitted"
    );
    assert!(!root.join("out/tests").exists(), "the suite is not");
}

/// A file that does not parse fails the build even if it holds a suite: it
/// cannot be told apart from production source. Only well-formed suites are
/// stripped unchecked.
#[test]
fn a_suite_that_does_not_parse_still_fails_compile() {
    let root = project(
        "syntax",
        "suite shop.maths {\n  case \"x\" { expect double(\n}\n",
    );
    let (ok, text) = bynk(&root, &["compile", "-o", "out", "."]);
    assert!(!ok, "a malformed file fails the build: {text}");
    assert!(text.contains("bynk.parse."), "{text}");
}

/// Every file under `dir`, relative path to bytes, for a tree comparison.
fn tree(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// #2834: a suite's helper `fn`s are test-only. The deployable output is
/// byte-for-byte the same whether or not a suite declares one.
#[test]
fn a_suite_helper_leaves_the_deployable_unchanged() {
    let without = project(
        "helper-without",
        "suite shop.maths {\n  case \"doubles\" { expect double(3) == 6 }\n}\n",
    );
    let with = project(
        "helper-with",
        "suite shop.maths {\n  fn six() -> Int { double(3) }\n\n  case \"doubles\" { expect six() == 6 }\n}\n",
    );
    for target in ["bundle", "workers"] {
        let out = format!("out-{target}");
        for root in [&without, &with] {
            let (ok, text) = bynk(root, &["compile", "--target", target, "-o", &out, "."]);
            assert!(ok, "{target}: {text}");
        }
        let (a, b) = (tree(&without.join(&out)), tree(&with.join(&out)));
        assert!(!a.is_empty(), "{target}: nothing was emitted");
        let differing: Vec<_> = a
            .keys()
            .chain(b.keys())
            // A source map names the project's own path, which differs here.
            .filter(|k| k.extension().is_none_or(|e| e != "map"))
            .filter(|k| a.get(*k) != b.get(*k))
            .collect();
        assert!(
            differing.is_empty(),
            "{target}: a suite helper changed the deployable: {differing:?}"
        );
    }
}

/// #2841: a suite's `fixture` is test input. The deployable output is
/// byte-for-byte the same whether or not a suite reads a fixture file.
#[test]
fn a_suite_fixture_leaves_the_deployable_unchanged() {
    let without = project(
        "fixture-without",
        "suite shop.maths {\n  case \"doubles\" { expect double(3) == 6 }\n}\n",
    );
    let with = project(
        "fixture-with",
        "suite shop.maths {\n  fixture note = \"fixtures/note.txt\"\n\n  case \"reads\" { expect note.length() > 0 }\n}\n",
    );
    std::fs::create_dir_all(with.join("fixtures")).unwrap();
    std::fs::write(with.join("fixtures/note.txt"), "a captured payload\n").unwrap();
    for target in ["bundle", "workers"] {
        let out = format!("out-{target}");
        for root in [&without, &with] {
            let (ok, text) = bynk(root, &["compile", "--target", target, "-o", &out, "."]);
            assert!(ok, "{target}: {text}");
        }
        let (a, b) = (tree(&without.join(&out)), tree(&with.join(&out)));
        assert!(!a.is_empty(), "{target}: nothing was emitted");
        let differing: Vec<_> = a
            .keys()
            .chain(b.keys())
            .filter(|k| k.extension().is_none_or(|e| e != "map"))
            .filter(|k| a.get(*k) != b.get(*k))
            .collect();
        assert!(
            differing.is_empty(),
            "{target}: a suite fixture changed the deployable: {differing:?}"
        );
        // And the fixture's text is in no file of it.
        for (path, bytes) in &b {
            assert!(
                !String::from_utf8_lossy(bytes).contains("a captured payload"),
                "{target}: {} carries the fixture's text",
                path.display()
            );
        }
    }
}

/// A fixture over 4 MiB is `bynk.fixture.too_large`; one at the cap is read.
#[test]
fn a_fixture_over_four_mib_is_refused() {
    let suite = "suite shop.maths {\n  fixture big = \"fixtures/big.txt\"\n\n  case \"reads\" { expect big.length() > 0 }\n}\n";
    let root = project("fixture-cap", suite);
    std::fs::create_dir_all(root.join("fixtures")).unwrap();
    std::fs::write(root.join("fixtures/big.txt"), vec![b'a'; 4 * 1024 * 1024]).unwrap();
    let (ok, text) = bynk(&root, &["check", "--format", "short", "."]);
    assert!(ok, "a fixture at the cap is read: {text}");
    std::fs::write(
        root.join("fixtures/big.txt"),
        vec![b'a'; 4 * 1024 * 1024 + 1],
    )
    .unwrap();
    let (ok, text) = bynk(&root, &["check", "--format", "short", "."]);
    assert!(!ok, "{text}");
    assert!(text.contains("bynk.fixture.too_large"), "{text}");
}

/// A fixture nothing uses is a warning, not an error.
#[test]
fn an_unused_fixture_is_a_warning() {
    let root = project(
        "fixture-unused",
        "suite shop.maths {\n  fixture note = \"fixtures/note.txt\"\n\n  case \"doubles\" { expect double(3) == 6 }\n}\n",
    );
    std::fs::create_dir_all(root.join("fixtures")).unwrap();
    std::fs::write(root.join("fixtures/note.txt"), "x").unwrap();
    let (ok, text) = bynk(&root, &["check", "--format", "short", "."]);
    assert!(ok, "{text}");
    assert!(text.contains("bynk.fixture.unused"), "{text}");
}
