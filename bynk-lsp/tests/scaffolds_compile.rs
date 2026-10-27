//! v0.121 (ADR 0157): the scaffold-compiles test.
//!
//! The LSP's scaffold catalogue (`completion::SNIPPETS`) is lexed and parsed
//! against the current grammar, after stripping snippet tab-stop syntax to a
//! compilable skeleton. A scaffold that no longer compiles fails the build;
//! this is the test that would have caught the retired `test "…" { }` snippet.
//!
//! The VS Code extension's own static catalogue lived here too (ADR 0157's
//! second half). It now lives with the extension (bynk-lang/vscode-bynk),
//! which parses every snippet with the released `bynkc` in its CI.

mod support;

// bynk-lsp is a binary crate: include the pure module directly (the pattern
// `legend_drift.rs` established). `symbols` satisfies completion's one
// `crate::symbols` reference.

use bynk_lsp::completion;
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;

fn assert_scaffold_parses(name: &str, source: &str) {
    let tokens = tokenize(source).unwrap_or_else(|e| panic!("{name}: lex error: {e:?}"));
    let (unit, errs) = parse_unit_with_recovery(&tokens, source);
    assert!(
        unit.is_some() && errs.is_empty(),
        "{name}: scaffold no longer parses against the current grammar\n\
         --- skeleton ---\n{source}\n--- errors ---\n{errs:?}"
    );
}

#[test]
fn lsp_snippets_compile() {
    for &(name, body) in completion::SNIPPETS {
        let stripped = support::strip_snippet_placeholders(body);
        let source = support::wrap_for_parse(name, &stripped);
        assert_scaffold_parses(name, &source);
    }
}

/// v0.125 (slice 4): the scaffold refresh's own coverage floor. Every language
/// construct the slice set out to de-stale must appear in at least one scaffold
/// across the catalogue — proof the fill happened and cannot silently regress.
#[test]
fn the_catalogue_covers_the_refreshed_constructs() {
    // Distinctive keyword tokens introduced by slice 4's fill (the refined-type
    // fill is witnessed by `where`, the opaque fill by `opaque`).
    const CONSTRUCTS: &[&str] = &[
        "suite",
        "case",
        "property",
        "expect",
        "for",
        "actor",
        "invariant",
        "transition",
        "requires",
        "ensures",
        "given",
        "opaque",
        "where",
        "uses",
        "consumes",
        "adapter",
    ];

    // The union of every scaffold body's word tokens.
    let mut tokens = std::collections::HashSet::new();
    let mut add = |body: &str| {
        for tok in body.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if !tok.is_empty() {
                tokens.insert(tok.to_string());
            }
        }
    };
    for &(_, body) in completion::SNIPPETS {
        add(body);
    }

    for kw in CONSTRUCTS {
        assert!(
            tokens.contains(*kw),
            "no scaffold exercises `{kw}` — the slice-4 refresh regressed"
        );
    }
}
