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
//! A writer is recognised two ways, because the marker may be written as a literal
//! (`__layout: 2`) or through the runtime's own vocabulary (`[LAYOUT_KEY]:
//! LAYOUT_PER_ENTRY`, the `EntryOverlay`/`splitWholesale` helpers), which the
//! snapshots reference by name and the runtime module (not in the snapshots) defines.
//! Either way the layout it writes must be `bynk_emit::STORAGE_LAYOUT`, and the
//! per-entry layout number the runtime defines must be the one the constant takes
//! once anything writes it.
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

/// The names of the runtime's per-entry vocabulary (`bynk-emit/runtime/src/entries.ts`):
/// emitted code that refers to one is wiring the per-entry layout in, whether or not
/// it spells the marker as a literal.
const WRITER_VOCABULARY: &[&str] = &[
    "LAYOUT_PER_ENTRY",
    "LAYOUT_KEY",
    "splitWholesale",
    "EntryOverlay",
    "stampLayout",
];

/// The names in [`WRITER_VOCABULARY`] an emitted source mentions.
fn writer_vocabulary(source: &str) -> Vec<&'static str> {
    WRITER_VOCABULARY
        .iter()
        .copied()
        .filter(|name| source.contains(name))
        .collect()
}

/// `export const <name> = <number>;` in the runtime's entries module.
fn runtime_constant(name: &str) -> i64 {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bynk-emit/runtime/src/entries.ts");
    let source = std::fs::read_to_string(&path).unwrap();
    let needle = format!("export const {name} = ");
    let at = source
        .find(&needle)
        .unwrap_or_else(|| panic!("{} no longer defines `{name}`", path.display()));
    source[at + needle.len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or_else(|_| panic!("`{name}` is not a plain number in {}", path.display()))
}

#[test]
fn the_layout_marker_the_emitter_writes_is_the_one_the_deploy_facts_record() {
    let expected = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive");
    let mut files = Vec::new();
    typescript_files(&expected, &mut files);
    assert!(!files.is_empty(), "no emission snapshots found");

    let per_entry = runtime_constant("LAYOUT_PER_ENTRY");
    let wholesale = runtime_constant("LAYOUT_WHOLESALE");
    // The constant is always a layout the runtime defines.
    assert!(
        [wholesale, per_entry].contains(&bynk_emit::STORAGE_LAYOUT),
        "`bynk_emit::STORAGE_LAYOUT` is {}, which is neither the wholesale layout ({wholesale}) nor \
         the per-entry layout ({per_entry}) the runtime defines",
        bynk_emit::STORAGE_LAYOUT
    );

    // (file, the layout it writes). A literal marker writes that number; a reference to
    // the per-entry vocabulary writes the runtime's per-entry layout.
    let mut writers: Vec<(PathBuf, i64)> = Vec::new();
    for f in &files {
        let source = std::fs::read_to_string(f).unwrap();
        for n in layout_writes(&source) {
            writers.push((f.clone(), n));
        }
        if !writer_vocabulary(&source).is_empty() {
            writers.push((f.clone(), per_entry));
        }
    }

    for (file, n) in &writers {
        assert_eq!(
            *n,
            bynk_emit::STORAGE_LAYOUT,
            "{} writes layout {n}, but `bynk_emit::STORAGE_LAYOUT` is {}: raise or lower \
             the constant with the writer, or `bynk deploy revert` will not refuse a rollback \
             across the layout",
            file.display(),
            bynk_emit::STORAGE_LAYOUT
        );
    }
    if bynk_emit::STORAGE_LAYOUT > 1 {
        assert!(
            !writers.is_empty(),
            "`bynk_emit::STORAGE_LAYOUT` is {} but no emitted agent writes that layout",
            bynk_emit::STORAGE_LAYOUT
        );
    } else {
        assert!(
            writers.is_empty(),
            "an emitted agent writes a layout, but `bynk_emit::STORAGE_LAYOUT` is still 1"
        );
    }
}

#[test]
fn the_runtime_defines_the_layouts_the_constant_may_take() {
    assert_eq!(runtime_constant("LAYOUT_WHOLESALE"), 1);
    assert!(runtime_constant("LAYOUT_PER_ENTRY") > 1);
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

#[test]
fn the_computed_key_form_is_a_writer_through_the_vocabulary() {
    // The literal scan cannot see this form; the vocabulary scan can.
    let computed = "const r = { ...s, [__LAYOUT_KEY]: __LAYOUT_PER_ENTRY };";
    assert!(layout_writes(computed).is_empty());
    assert_eq!(
        writer_vocabulary(computed),
        vec!["LAYOUT_PER_ENTRY", "LAYOUT_KEY"]
    );
    assert_eq!(
        writer_vocabulary("const o = new __EntryOverlay(storage);"),
        vec!["EntryOverlay"]
    );
    // The guard and an unrelated name are not the vocabulary.
    assert!(writer_vocabulary("__checkStoredLayout(\"A\", stored);").is_empty());
    assert!(writer_vocabulary("const layout = 1;").is_empty());
}
