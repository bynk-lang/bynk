//! #2203 (DECISION E): each retired comment or doc spelling is an error with a
//! machine-applicable fix. For each negative fixture of a legacy form, applying
//! every fix gives the migrated source, which compiles cleanly; the formatter
//! (DECISION F) gives the same migration.

use std::path::Path;

use bynk_syntax::error::{Applicability, CompileError};

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/negative")
        .join(name)
        .join("input.bynk");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The lex error, or every parse error, `src` raises.
fn errors(src: &str) -> Vec<CompileError> {
    match bynk_syntax::lexer::tokenize(src) {
        Ok(tokens) => bynk_syntax::parser::parse_units(&tokens, src)
            .err()
            .unwrap_or_default(),
        Err(e) => vec![e],
    }
}

/// `src` with every error's machine-applicable fix applied.
fn fixed(src: &str) -> String {
    let mut edits: Vec<_> = errors(src)
        .iter()
        .flat_map(|e| {
            let fixes: Vec<_> = e
                .suggestions
                .iter()
                .filter(|s| s.applicability == Applicability::MachineApplicable)
                .collect();
            assert_eq!(fixes.len(), 1, "one fix for {}", e.category);
            fixes[0].edits.clone()
        })
        .collect();
    edits.sort_by_key(|(span, _)| span.start);
    let mut out = String::new();
    let mut last = 0;
    for (span, text) in edits {
        out.push_str(&src[last..span.start]);
        out.push_str(&text);
        last = span.end;
    }
    out.push_str(&src[last..]);
    out
}

#[test]
fn each_legacy_form_is_fixed_by_its_suggestion() {
    for (name, want) in [
        (
            "2203_legacy_comment",
            "// A legacy comment.\ncommons probe\n\nfn answer() -> Int { 42 }  // the answer\n",
        ),
        (
            "2203_legacy_doc_line",
            "commons probe\n\n--- The answer.\nfn answer() -> Int { 42 }\n",
        ),
        (
            "2203_slash_doc",
            "commons probe\n\n--- The answer.\nfn answer() -> Int { 42 }\n",
        ),
        (
            "2203_consecutive_doc_lines",
            "commons probe\n\n---\nThe answer.\nAnd a second line.\n---\nfn answer() -> Int { 42 }\n",
        ),
    ] {
        let src = fixture(name);
        let out = fixed(&src);
        assert_eq!(out, want, "{name}");
        assert!(errors(&out).is_empty(), "{name}: {:?}", errors(&out));
        bynkc::compile(&out, "probe.bynk").unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
}

/// DECISION F: `bynk fmt` rewrites the legacy forms the same way.
#[test]
fn the_formatter_migrates_the_legacy_forms() {
    let opts = bynk_fmt::FormatOptions::default();
    for name in ["2203_legacy_comment", "2203_legacy_doc_line"] {
        let src = fixture(name);
        let out = bynk_fmt::format_source(&src, &opts).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        // The fixer only rewrites comment syntax; the formatter also lays the
        // body out, so compare against the fixed source formatted.
        let want = bynk_fmt::format_source(&fixed(&src), &opts)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(out, want, "{name}");
    }
}
