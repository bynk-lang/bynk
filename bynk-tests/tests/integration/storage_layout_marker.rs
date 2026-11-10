//! The coupling between the storage layout the emitter writes and
//! `bynk_emit::STORAGE_LAYOUT` (#3189, per-entry storage track).
//!
//! `bynk deploy` records `STORAGE_LAYOUT` in each version's facts, and
//! `bynk deploy revert` refuses a rollback to an older layout over stored data on
//! the strength of it. If the emitter began writing a `__layout` marker without the
//! constant being raised to match, the preflight would stay silent over a rollback
//! that orphans data. This test fails in that case, and in the opposite one (the
//! constant raised with no writer). It reads the committed emission snapshots, so
//! it needs no toolchain.
//!
//! Today the constant is 1, the wholesale layout, and nothing writes a marker.

use std::path::{Path, PathBuf};

fn typescript_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            typescript_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "ts") {
            out.push(p);
        }
    }
}

/// The numbers written as a `__layout` marker in some emitted source: every
/// `__layout` that is followed by `:` or `=` and a number. The runtime's own
/// `__checkStoredLayout` guard contains no lowercase `__layout`, so it is not one.
fn layout_writes(source: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("__layout") {
        let after = rest[at + "__layout".len()..].trim_start_matches(['"', '\'']);
        let after = after.trim_start();
        if let Some(value) = after.strip_prefix([':', '=']) {
            let digits: String = value
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(n) = digits.parse() {
                out.push(n);
            }
        }
        rest = &rest[at + "__layout".len()..];
    }
    out
}

#[test]
fn the_layout_marker_the_emitter_writes_is_the_one_the_deploy_facts_record() {
    let expected = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive");
    let mut files = Vec::new();
    typescript_files(&expected, &mut files);
    assert!(!files.is_empty(), "no emission snapshots found");

    let mut writers = Vec::new();
    for f in &files {
        let source = std::fs::read_to_string(f).unwrap();
        for n in layout_writes(&source) {
            writers.push((f.clone(), n));
        }
    }

    for (file, n) in &writers {
        assert_eq!(
            *n,
            bynk_emit::STORAGE_LAYOUT,
            "{} writes a `__layout` of {n}, but `bynk_emit::STORAGE_LAYOUT` is {}: raise or lower \
             the constant with the writer, or `bynk deploy revert` will not refuse a rollback \
             across the layout",
            file.display(),
            bynk_emit::STORAGE_LAYOUT
        );
    }
    if bynk_emit::STORAGE_LAYOUT > 1 {
        assert!(
            !writers.is_empty(),
            "`bynk_emit::STORAGE_LAYOUT` is {} but no emitted agent writes that layout marker",
            bynk_emit::STORAGE_LAYOUT
        );
    } else {
        assert!(
            writers.is_empty(),
            "an emitted agent writes a `__layout` marker, but `bynk_emit::STORAGE_LAYOUT` is still 1"
        );
    }
}

#[test]
fn the_marker_scan_finds_writes_and_ignores_the_guard() {
    assert_eq!(
        layout_writes("const r = { ...s, __layout: 2, __v: 1 };"),
        vec![2]
    );
    assert_eq!(layout_writes(r#"{ "__layout": 3 }"#), vec![3]);
    assert_eq!(layout_writes("s.__layout = 2;"), vec![2]);
    // The guard's name, a read, and a comment are not writes.
    assert!(layout_writes("__checkStoredLayout(\"A\", stored);").is_empty());
    assert!(layout_writes("if (stored.__layout === undefined) return;").is_empty());
    assert!(layout_writes("// the __layout marker").is_empty());
}
