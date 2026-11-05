//! The By Example snippets gate (documentation track, slice 4).
//!
//! Every `site/src/snippets/*.bynk` is a bite-size, self-contained `commons`
//! program shown in the "Bynk by Example" gallery with an "Open in playground"
//! link. This gate compiles each one, so a snippet the docs invite you to open
//! and run can never fall out of step with the compiler (track §6/§10).

use std::fs;
use std::path::PathBuf;

fn snippets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../site/src/snippets")
}

#[test]
fn every_snippet_compiles() {
    let dir = snippets_dir();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("read site/src/snippets")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "bynk"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no .bynk snippets under site/src/snippets"
    );

    let mut failures = Vec::new();
    for file in &files {
        let source = fs::read_to_string(file).unwrap();
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        if let Err(errs) = bynkc::compile(&source, &name) {
            let msg = errs
                .iter()
                .map(|e| format!("{}: {}", e.category, e.message))
                .collect::<Vec<_>>()
                .join("; ");
            failures.push(format!("{name}: {msg}"));
        }
    }
    assert!(
        failures.is_empty(),
        "snippet compilation gate failed:\n  {}",
        failures.join("\n  ")
    );
}
