//! #2010: editor surfaces over `let <pattern> … else …`. The parser desugars it
//! into a hidden `__letElse<offset>` binding and a `match`, so these tests pin
//! two things: the names the author's pattern binds behave like `let`
//! bindings (inlay hint, local, go-to-definition, semantic token), and the
//! hidden binder surfaces nowhere.

use bynk_lsp::index_queries;
use std::fs;
use std::path::PathBuf;

fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-let-else-{test_name}-{}",
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

const SRC: &str = "\
commons demo.steps

fn total(r: Result[Int, String], o: Option[Int]) -> Int {
  let Ok(count) = r else { 0 }
  let Some(extra) = o else match {
    None => count,
  }
  count + extra
}
";

const PATH: &str = "demo/steps.bynk";

fn offset_of(needle: &str, n: usize) -> usize {
    SRC.match_indices(needle).nth(n).expect("needle present").0
}

fn analyse(test_name: &str) -> bynk_ide::ProjectDiagnostics {
    let root = setup_project(test_name, &[(PATH, SRC)]);
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let errors: Vec<String> = result
        .files
        .iter()
        .flat_map(|f| &f.diagnostics)
        .chain(&result.unattributed)
        .filter(|d| d.severity == bynk_syntax::Severity::Error)
        .map(|d| format!("{}: {}", d.error.category, d.error.message))
        .collect();
    assert!(
        errors.is_empty(),
        "the fixture should check cleanly: {errors:?}"
    );
    result
}

#[test]
fn pattern_names_get_inlay_hints_and_the_hidden_binder_none() {
    let result = analyse("hints");
    let hints = result
        .hints
        .get(&PathBuf::from(PATH))
        .expect("the file has hints");
    let at = |needle: &str| {
        let start = offset_of(needle, 0);
        hints
            .iter()
            .find(|h| h.span.start == start)
            .map(|h| h.label.clone())
    };
    assert_eq!(at("count").as_deref(), Some(": Int"));
    assert_eq!(at("extra").as_deref(), Some(": Int"));
    assert!(
        hints.iter().all(|h| h.span.start < h.span.end),
        "no hint may sit on the hidden binder's empty span: {hints:?}"
    );
}

#[test]
fn pattern_names_are_locals_and_the_hidden_binder_is_not() {
    let result = analyse("locals");
    let locals = result
        .locals
        .get(&PathBuf::from(PATH))
        .expect("the file has locals");
    let names: Vec<&str> = locals.iter().map(|l| l.name.as_str()).collect();
    assert!(
        names.contains(&"count") && names.contains(&"extra"),
        "{names:?}"
    );
    assert!(
        locals.iter().all(|l| !l.name.starts_with("__")),
        "the hidden binder must not be a local: {names:?}"
    );
    // A pattern name is in scope for the rest of the block, as a `let` is.
    let count = locals.iter().find(|l| l.name == "count").unwrap();
    let use_at = offset_of("count", 2);
    assert!(count.scope.start <= use_at && use_at < count.scope.end);
}

#[test]
fn a_pattern_names_use_goes_to_its_definition() {
    let result = analyse("definition");
    let locals = result
        .locals
        .get(&PathBuf::from(PATH))
        .expect("the file has locals");
    let use_at = offset_of("extra", 1) + 1;
    let def = bynk_ide::locals_nav::local_definition_at(locals, SRC, use_at)
        .expect("the use of `extra` resolves");
    assert_eq!(def.start, offset_of("extra", 0));
}

#[test]
fn semantic_tokens_cover_pattern_names_and_none_is_empty() {
    let result = analyse("tokens");
    let path = PathBuf::from(PATH);
    let locals = result.locals.get(&path).expect("the file has locals");
    let local_tokens = bynk_ide::locals_nav::local_token_sites(locals, SRC);
    // Occurrence 0 — the name in the pattern `Ok(count)` — is the definition
    // (a declaration token); the uses, in the second step's `else match` and in
    // the tail, are plain variable tokens.
    for n in 0..3 {
        let at = offset_of("count", n);
        let declaration = n == 0;
        assert!(
            local_tokens
                .iter()
                .any(|(s, decl)| s.start == at && *decl == declaration),
            "`count` occurrence {n} should be a variable token \
             (declaration: {declaration}): {local_tokens:?}"
        );
    }
    let tokens =
        index_queries::semantic_tokens(&result.index, &local_tokens, &[], &path, SRC, None);
    assert!(
        tokens.iter().all(|t| t.length > 0),
        "no empty tokens: {tokens:?}"
    );
}
