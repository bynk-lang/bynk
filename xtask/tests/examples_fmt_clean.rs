//! Drift guard: #3174 — every `.bynk` file under `examples/` is exactly what
//! `bynk fmt` produces with the canonical (default) style.
//!
//! The examples are the corpus the Book and READMEs point newcomers at, and
//! `bynk new` scaffolds in the canonical style (tabs). If the examples drift
//! from the formatter's output, the first project someone scaffolds and the
//! examples they then read look different, and running `bynk fmt` on an example
//! rewrites it. Fix a failure with `cargo run -p bynk -- fmt examples`.

use std::path::{Path, PathBuf};

use bynk_fmt::{FormatOptions, format_source};

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples")
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "bynk") {
            out.push(path);
        }
    }
}

#[test]
fn examples_are_canonically_formatted() {
    let mut files = Vec::new();
    collect(&examples(), &mut files);
    files.sort();
    assert!(!files.is_empty(), "found no .bynk files under examples/");

    // The guard formats with the defaults, so no example may override them.
    for manifest in files.iter().filter_map(|f| {
        f.ancestors()
            .map(|a| a.join("bynk.toml"))
            .find(|m| m.is_file())
    }) {
        let text = std::fs::read_to_string(&manifest).expect("read bynk.toml");
        assert!(
            !text.contains("[fmt]"),
            "{} sets [fmt]; examples must use the canonical style",
            manifest.display()
        );
    }

    let mut dirty = Vec::new();
    for file in &files {
        let src = std::fs::read_to_string(file).expect("read example");
        match format_source(&src, &FormatOptions::default()) {
            Ok(out) if out == src => {}
            Ok(_) => dirty.push(file.display().to_string()),
            Err(_) => dirty.push(format!("{} (does not parse)", file.display())),
        }
    }
    assert!(
        dirty.is_empty(),
        "not canonically formatted (run `cargo run -p bynk -- fmt examples`):\n{}",
        dirty.join("\n")
    );
}
