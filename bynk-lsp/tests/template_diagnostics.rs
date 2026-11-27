//! #3927 (typed HTML, slice 1): the editor reads a declared template's file and reports its
//! errors at the template's own line, as the build does, with a related location at the
//! `template` declaration.

use std::path::{Path, PathBuf};

fn analysed(fixture: &str) -> bynk_ide::ProjectDiagnostics {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bynk-tests/tests/fixtures/negative")
        .join(fixture);
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    bynk_ide::diagnose_project(&root, &sources)
}

#[test]
fn a_template_error_is_shown_at_the_template_line() {
    let r = analysed("3927_template_unknown_placeholder");
    let file = r
        .files
        .iter()
        .find(|f| f.source_path == PathBuf::from("embeds/page.html"))
        .expect("the template file is one of the analysed files");
    let d = file
        .diagnostics
        .iter()
        .find(|d| d.error.category == "bynk.template.unknown_placeholder")
        .expect("the unknown placeholder is reported in the template file");
    assert_eq!(
        bynk_syntax::span::line_col(&file.text, d.error.span.start),
        (1, 5),
        "at `\\(titel)` in `<h1>\\(titel)</h1>`"
    );
    // The related location is the declaration, in the `.bynk` file.
    assert!(
        d.error.labels.iter().any(|(_, l)| l == "in this template"),
        "{:?}",
        d.error.labels
    );
    assert!(
        d.label_files
            .iter()
            .any(|f| matches!(f, bynk_project::LabelFile::Other(p) if p.ends_with("page.bynk"))),
        "the label lies in the declaring file: {:?}",
        d.label_files
    );
}

#[test]
fn an_unsaved_template_buffer_is_what_the_editor_checks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bynk-tests/tests/fixtures/negative/3927_template_unknown_placeholder");
    let mut sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    // The buffer fixes the misspelling: the template error goes, and with it the unused field.
    sources.insert(root.join("embeds/page.html"), "<h1>\\(title)</h1>\n".to_string());
    let r = bynk_ide::diagnose_project(&root, &sources);
    let template_errors: Vec<_> = r
        .files
        .iter()
        .flat_map(|f| f.diagnostics.iter())
        .filter(|d| d.error.category.starts_with("bynk.template."))
        .map(|d| d.error.category)
        .collect();
    assert!(template_errors.is_empty(), "{template_errors:?}");
}
