//! Constrained generics slice 0: applying `bynk.generics.missing_eq_bound`'s
//! suggested fixes to a program written for ADR 0430's inferred bounds yields a
//! program that compiles with the same accept/reject outcome at every call site.
//!
//! Each case is the **pre-migration** source of an ADR 0430 fixture
//! (`1688_*`, `1652_structural_equality`), written with no bounds. The test
//! applies every machine-applicable suggestion, as `source.fixAll.bynk` does, until
//! none is left, then pins what remains: nothing for the fixtures that accepted,
//! and the call-site `not_comparable` the fixtures that rejected still raise.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_syntax::error::Applicability;

/// Apply `edits` (span → replacement, non-overlapping) to `text`.
fn apply(text: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort();
    let mut out = String::new();
    let mut last = 0;
    for (start, end, replacement) in edits {
        out.push_str(&text[last..start]);
        out.push_str(&replacement);
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

/// Diagnose `files` (path → source) as one project: the sources are written
/// under `root` and read back the way the language server's tests do.
fn diagnose(root: &Path, files: &HashMap<PathBuf, String>) -> bynk_ide::ProjectDiagnostics {
    for (path, text) in files {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let overlay = bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
        root.to_path_buf(),
    ));
    bynk_ide::diagnose_project(root, &overlay)
}

/// The categories of every diagnostic, sorted, as `path: category`.
fn categories(result: &bynk_ide::ProjectDiagnostics) -> Vec<String> {
    let mut out: Vec<String> = result
        .files
        .iter()
        .flat_map(|f| {
            f.diagnostics.iter().map(|d| {
                format!(
                    "{}: {}",
                    f.source_path.to_string_lossy().replace('\\', "/"),
                    d.error.category
                )
            })
        })
        .collect();
    out.sort();
    out
}

/// Apply every `missing_eq_bound` fix, round after round, until none is left.
/// Returns the migrated sources and the rounds it took.
fn migrate(root: &Path, mut files: HashMap<PathBuf, String>) -> (HashMap<PathBuf, String>, usize) {
    for round in 0.. {
        let result = diagnose(root, &files);
        let mut applied = false;
        for f in &result.files {
            let mut edits: Vec<(usize, usize, String)> = Vec::new();
            for d in &f.diagnostics {
                if d.error.category != "bynk.generics.missing_eq_bound" {
                    continue;
                }
                let [s] = d.error.suggestions.as_slice() else {
                    panic!("`missing_eq_bound` must carry exactly one suggestion");
                };
                assert_eq!(s.applicability, Applicability::MachineApplicable);
                for (span, text) in &s.edits {
                    let edit = (span.start, span.end, text.clone());
                    if !edits.contains(&edit) {
                        edits.push(edit);
                    }
                }
            }
            if !edits.is_empty() {
                let path = root.join(&f.source_path);
                let migrated = apply(&f.text, edits);
                files.insert(path, migrated);
                applied = true;
            }
        }
        if !applied {
            return (files, round);
        }
        assert!(round < 8, "the fixes did not converge");
    }
    unreachable!()
}

fn project(files: &[(&str, &str)]) -> (PathBuf, HashMap<PathBuf, String>) {
    // A scratch directory per call; the PR-unique name keeps parallel runs apart.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "bynk-eq-bound-migration-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let map = files
        .iter()
        .map(|(p, s)| (root.join(p), (*s).to_string()))
        .collect();
    (root, map)
}

/// The migrated source of `file` after every fix, and the rounds it took.
fn migrated(files: &[(&str, &str)], file: &str) -> (String, usize) {
    let (root, map) = project(files);
    let (done, rounds) = migrate(&root, map);
    let text = done[&root.join(file)].clone();
    let _ = std::fs::remove_dir_all(&root);
    (text, rounds)
}

/// The number of `Eq` bounds the program gains, and the outcome after the fixes.
fn run(files: &[(&str, &str)], bounds_added: &[&str], after: &[&str]) {
    let (root, map) = project(files);
    let first = categories(&diagnose(&root, &map));
    assert!(
        first
            .iter()
            .any(|c| c.ends_with("bynk.generics.missing_eq_bound")),
        "the unbounded source is rejected with `missing_eq_bound`: {first:?}"
    );
    let eq_count =
        |m: &HashMap<PathBuf, String>| m.values().map(|t| t.matches("Eq").count()).sum::<usize>();
    let before = eq_count(&map);
    let (migrated, _) = migrate(&root, map);
    assert_eq!(
        eq_count(&migrated) - before,
        bounds_added.len(),
        "one `Eq` per generic that compares: {bounds_added:?}"
    );
    let got = categories(&diagnose(&root, &migrated));
    let want: Vec<String> = after.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(got, want, "outcome after the fixes");
    // The result is stable: a second migration changes nothing.
    let (again, rounds) = migrate(&root, migrated.clone());
    assert_eq!(rounds, 0);
    assert_eq!(again, migrated);
    let _ = std::fs::remove_dir_all(&root);
}

const SAME_FN: &str = "commons m\n\nfn same[T](a: T, b: T) -> Bool { a == b }\n\nfn g(f: (Int) -> Int, h: (Int) -> Int) -> Bool { same(f, h) }\n";

#[test]
fn a_function_instantiated_with_functions_is_still_rejected_at_the_call() {
    run(
        &[("m.bynk", SAME_FN)],
        &["same"],
        &["m.bynk: bynk.types.not_comparable"],
    );
}

#[test]
fn a_transitive_bound_is_added_to_every_link_and_still_rejected_at_the_call() {
    let src = "commons m\n\nfn same[T](a: T, b: T) -> Bool { a == b }\n\nfn twice[U](a: U, b: U) -> Bool { same(a, b) }\n\nfn k(xs: List[(Int) -> Int]) -> Bool { twice(xs, xs) }\n";
    run(
        &[("m.bynk", src)],
        &["same", "twice"],
        &["m.bynk: bynk.types.not_comparable"],
    );
    let (text, rounds) = migrated(&[("m.bynk", src)], "m.bynk");
    assert_eq!(rounds, 2, "the call-site fix needs `same` migrated first");
    assert!(text.contains("fn same[T: Eq]"), "{text}");
    assert!(text.contains("fn twice[U: Eq]"), "{text}");
}

#[test]
fn a_receiver_parameter_gets_a_where_clause_and_the_call_is_still_rejected() {
    let src = "commons m\n\ntype Box[A] = { value: A }\n\nfn Box.has(self, x: A) -> Bool { self.value == x }\n\nfn dec(n: Int) -> Int { n - 1 }\n\nfn g(b: Box[(Int) -> Int]) -> Bool { b.has(dec) }\n";
    run(
        &[("m.bynk", src)],
        &["A"],
        &["m.bynk: bynk.types.not_comparable"],
    );
    let (text, _) = migrated(&[("m.bynk", src)], "m.bynk");
    assert!(
        text.contains("fn Box.has(self, x: A) -> Bool where A: Eq {"),
        "{text}"
    );
}

#[test]
fn comparable_instantiations_and_non_comparing_generics_still_compile() {
    let src = "commons demo.gen\n\nfn same[T](a: T, b: T) -> Bool { a == b }\n\nfn twice[U](a: U, b: U) -> Bool { same(a, b) }\n\nfn apply[A, B](f: (A) -> B, x: A) -> B { f(x) }\n\nfn pick[T](a: T, b: T, first: Bool) -> T {\n  if first { a } else { b }\n}\n\nfn countdown[T](a: T, n: Int) -> Bool {\n  if n == 0 { same(a, a) } else { countdown(a, n - 1) }\n}\n\nfn inc(n: Int) -> Int { n + 1 }\n\nfn dec(n: Int) -> Int { n - 1 }\n\nfn applyTen(f: (Int) -> Int) -> Int { f(10) }\n\nfn applyPicked(first: Bool) -> Int {\n  applyTen(pick[(Int) -> Int](inc, dec, first))\n}\n";
    run(
        &[("demo/gen.bynk", src)],
        &["same", "twice", "countdown"],
        &[],
    );
    let (text, _) = migrated(&[("demo/gen.bynk", src)], "demo/gen.bynk");
    assert!(
        text.contains("fn pick[T](a: T"),
        "`pick` stays unbounded: {text}"
    );
    assert!(
        text.contains("fn apply[A, B]("),
        "`apply` stays unbounded: {text}"
    );
}

#[test]
fn a_receiver_method_that_compares_accepts_comparable_receivers() {
    let src = "commons demo.box\n\ntype Box[A] = { value: A }\n\nfn Box.has(self, x: A) -> Bool { self.value == x }\n\nfn Box.apply(self, n: Int) -> Int { 0 }\n\nfn hasThree(n: Int) -> Bool {\n  let b = Box { value: 3 }\n  b.has(n)\n}\n\nfn inc(n: Int) -> Int { n + 1 }\n\nfn boxedFnApplied(f: (Int) -> Int) -> Int {\n  let b = Box { value: f }\n  b.apply(1)\n}\n";
    run(&[("demo/box.bynk", src)], &["A"], &[]);
}

#[test]
fn a_bound_added_across_units_is_checked_in_the_importer() {
    let lib = "commons m.lib\n\nfn same[T](a: T, b: T) -> Bool { a == b }\n";
    let app = "commons m.app\n\nuses m.lib\n\nfn g(f: (Int) -> Int) -> Bool { same(f, f) }\n\nfn n(x: Int) -> Bool { same(x, x) }\n";
    run(
        &[("m/lib.bynk", lib), ("m/app.bynk", app)],
        &["same"],
        &["m/app.bynk: bynk.types.not_comparable"],
    );
}

#[test]
fn a_two_hop_chain_through_another_units_uses_is_checked_in_the_importer() {
    let core = "commons m.core\n\nfn eq2[T](a: T, b: T) -> Bool { a == b }\n";
    let lib = "commons m.lib\n\nuses m.core\n\nfn same[T](a: T, b: T) -> Bool { eq2(a, b) }\n";
    let app = "commons m.app\n\nuses m.lib\n\nfn g(f: (Int) -> Int) -> Bool { same(f, f) }\n";
    run(
        &[
            ("m/core.bynk", core),
            ("m/lib.bynk", lib),
            ("m/app.bynk", app),
        ],
        &["eq2", "same"],
        &["m/app.bynk: bynk.types.not_comparable"],
    );
}

#[test]
fn a_local_comparing_helper_does_not_change_what_an_imported_generic_requires() {
    let lib = "commons m.lib\n\nfn same[T](a: T, b: T) -> Bool { 1 == 1 }\n";
    let app = "commons m.app\n\nuses m.lib\n\nfn helper[T](a: T, b: T) -> Bool { a == b }\n\nfn g(f: (Int) -> Int) -> Bool { same(f, f) }\n";
    // `same` never compares its `T`, so functions are fine; only `helper` needs `Eq`.
    run(
        &[("m/lib.bynk", lib), ("m/app.bynk", app)],
        &["helper"],
        &[],
    );
}
