//! #2201: every adapter's `binding … requires { … }` entries fold into one
//! `out/package.json`. Two adapters requiring the same package at the same
//! range merge silently; at different ranges the build is refused with
//! `bynk.requires.conflicting_range`, never last-insert-wins.
//!
//! Built from `negative/2201_requires_conflicting_range` (adapters `tokens`
//! at `jose ^5` and `tokens2` at `jose ^4`), copied into its own directory
//! under `CARGO_TARGET_TMPDIR` so a case can rewrite a range.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/negative/2201_requires_conflicting_range/src")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// The fixture's project in a fresh directory, with `tokens2`'s `jose` range
/// set to `range`.
fn project(name: &str, range: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&fixture(), &dir.join("src"));
    let tokens2 = dir.join("src/tokens2.bynk");
    let source = std::fs::read_to_string(&tokens2).unwrap();
    std::fs::write(&tokens2, source.replace("\"^4\"", &format!("\"{range}\""))).unwrap();
    dir
}

fn compile(dir: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["compile", "src", "-o", "out"])
        .current_dir(dir)
        .output()
        .expect("run bynkc");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

#[test]
fn different_ranges_for_one_package_are_refused_naming_both_adapters() {
    let dir = project("requires-range-conflict", "^4");
    let (ok, out) = compile(&dir);
    assert!(!ok, "conflicting ranges must fail the build, got:\n{out}");
    assert!(out.contains("bynk.requires.conflicting_range"), "{out}");
    assert!(
        out.contains(
            "adapter `tokens2` requires `jose` at `^4`, but adapter `tokens` requires it at `^5`"
        ),
        "{out}"
    );
    // The primary span is the second entry, in `tokens2.bynk`; the note names
    // where the first one is.
    assert!(out.contains("tokens2.bynk"), "{out}");
    assert!(out.contains("(in `tokens.bynk`)"), "{out}");
    assert!(
        !dir.join("out/package.json").exists(),
        "no package.json on a refused build"
    );
}

#[test]
fn identical_ranges_for_one_package_merge_silently() {
    let dir = project("requires-range-identical", "^5");
    let (ok, out) = compile(&dir);
    assert!(ok, "identical ranges must merge, got:\n{out}");
    let package = std::fs::read_to_string(dir.join("out/package.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&package).unwrap();
    assert_eq!(
        json["dependencies"],
        serde_json::json!({ "jose": "^5" }),
        "{package}"
    );
}
