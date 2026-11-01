//! v0.43 slice 3: LSP features must reach *inside* interpolation holes. A hole
//! is an ordinary expression with real spans (slice 1), so go-to-definition,
//! references, and semantic tokens fall out of the binding index — these tests
//! pin that the index actually records hole-interior references (and would
//! catch a future walker that stopped recursing into holes).

use bynk_lsp::index_queries;
use std::fs;
use std::path::PathBuf;

fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-interp-{test_name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create test root");
    for (rel, contents) in files {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&p, contents).expect("write file");
    }
    root.canonicalize().unwrap_or(root)
}

/// A commons whose `greet` interpolates a call to `shout` and a use of the
/// local `name` — both *inside* a `\(…)` hole.
const SRC: &str = "\
commons demo.text

fn shout(s: String) -> String {
  s
}

fn greet(name: String) -> String {
  \"Hi, \\(shout(name))!\"
}
";

fn offset_of(text: &str, needle: &str, n: usize) -> usize {
    let mut from = 0;
    let mut remaining = n;
    loop {
        let at = text[from..].find(needle).expect("needle present") + from;
        if remaining == 0 {
            return at;
        }
        remaining -= 1;
        from = at + 1;
    }
}

#[test]
fn go_to_definition_reaches_into_a_hole() {
    let root = setup_project("def", &[("demo/text.bynk", SRC)]);
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let index = &result.index;
    let path = PathBuf::from("demo/text.bynk");

    // The `shout` call is the 2nd occurrence of "shout" (1st is the fn def).
    let call_off = offset_of(SRC, "shout", 1) + 1; // mid-identifier
    let (_key, def) = index_queries::definition_at(index, &path, call_off)
        .expect("the `shout` call inside the hole resolves to a definition");
    // The definition is the `shout` fn header (1st occurrence).
    let def_off = offset_of(SRC, "shout", 0);
    assert!(
        def.span.start <= def_off + 5 && def.span.end >= def_off,
        "definition span {:?} should cover the `shout` declaration at {def_off}",
        def.span
    );
}

#[test]
fn references_include_the_hole_call_site() {
    let root = setup_project("refs", &[("demo/text.bynk", SRC)]);
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let index = &result.index;
    let path = PathBuf::from("demo/text.bynk");

    // From the `shout` definition, references must include the in-hole call.
    let def_off = offset_of(SRC, "shout", 0);
    let sites = index_queries::sites_for(index, &path, def_off, false)
        .expect("`shout` is an indexed symbol");
    let call_off = offset_of(SRC, "shout", 1);
    assert!(
        sites
            .iter()
            .any(|s| s.span.start <= call_off && s.span.end > call_off),
        "references should include the call inside the hole at {call_off}; got {:?}",
        sites.iter().map(|s| s.span).collect::<Vec<_>>()
    );
}

#[test]
fn hover_type_is_recorded_for_a_hole_expression() {
    // Hover-of-value reads `expr_types`, populated by the checker as it types
    // each hole's expression (slice 1). The `name` use inside `\(shout(name))`
    // must carry its `String` type.
    let root = setup_project("hover", &[("demo/text.bynk", SRC)]);
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let rel = PathBuf::from("demo/text.bynk");
    let (_p, entries) = result
        .expr_types
        .iter()
        .find(|(p, _)| **p == rel)
        .expect("file is analysed");
    // 2nd "name": the use inside the hole (1st is the parameter declaration).
    let use_off = offset_of(SRC, "name", 1) + 1;
    let ty = bynk_check::expr_types::type_at_offset(entries, use_off, &result.ty_intern)
        .expect("the hole expression `name` has a recorded type");
    // T3.6b: `expr_types` hands out a `TyId` — resolve it through the round's
    // intern table before asking what shape it is.
    let display = ty.display(&result.ty_intern);
    assert!(
        display.contains("String"),
        "expected the hole use to type as String; got {display}"
    );
}

#[test]
fn semantic_tokens_cover_a_hole_symbol() {
    let root = setup_project("sem", &[("demo/text.bynk", SRC)]);
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let index = &result.index;
    let path = PathBuf::from("demo/text.bynk");

    let tokens = index_queries::semantic_tokens(index, &[], &[], &[], &path, SRC, None);
    assert!(
        !tokens.is_empty(),
        "the `shout` call inside the hole should yield at least one semantic token"
    );
}
