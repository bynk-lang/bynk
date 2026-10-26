//! #2476: the names `textDocument/inlineValue` hands the debug adapter are the
//! variables the emitter really declares. The language server maps a Bynk
//! binding to its emitted name through `bynk_check::emitted_names`; this
//! compiles the same source and checks that mapping against the TypeScript.

use bynk_ide::locals_nav::inline_value_sites;
use bynk_syntax::span::Span;
use std::path::Path;

const FILE: &str = "m.bynk";

fn emitted_ts(src: &str) -> String {
    let out = bynk_emit::project::compile_in_memory(
        src,
        bynk_emit::project::BuildTarget::Bundle,
        Default::default(),
    )
    .unwrap_or_else(|_| panic!("fixture should compile:\n{src}"));
    out.artefacts
        .docs
        .iter()
        .find(|(path, _)| path.ends_with("inline.ts"))
        .map(|(_, doc)| doc.text())
        .expect("the fixture's own module is emitted")
}

#[test]
fn inline_value_names_are_the_emitted_declarations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/inline_values/src");
    let result = bynk_ide::diagnose_project(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );
    let text = result
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().ends_with(FILE))
        .expect("analysed")
        .text
        .clone();
    let locals = result
        .locals
        .iter()
        .find(|(p, _)| p.to_string_lossy().ends_with(FILE))
        .map(|(_, l)| l.clone())
        .expect("locals");

    let stop = text.rfind("total").unwrap();
    let sites = inline_value_sites(&locals, Span::new(0, text.len()), Span::new(stop, stop));
    let names: Vec<(&str, &str)> = sites
        .iter()
        .map(|s| (&text[s.range.start..s.range.end], s.variable.as_str()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("class", "__id_class"),
            ("n", "n"),
            ("doubled", "doubled"),
            ("total", "total"), // the first; the re-let has no knowable name
        ]
    );

    let ts = emitted_ts(&text);
    for (bynk, emitted) in &names {
        let declared =
            ts.contains(&format!("const {emitted} =")) || ts.contains(&format!("{emitted}: "));
        assert!(declared, "`{bynk}` → `{emitted}` is not declared in:\n{ts}");
    }
    // The re-let really is renamed to a throwaway the source cannot predict.
    assert!(
        ts.contains("const __r"),
        "re-let is emitted under a fresh name:\n{ts}"
    );
}
