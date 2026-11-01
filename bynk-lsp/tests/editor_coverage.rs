//! v0.121 (ADR 0156): the editor-currency coverage test.
//!
//! Every lowercase-initial `bynk-syntax::keywords::KEYWORDS` entry must have
//! a completion doc *and* a hover path. This is the tooth that would have
//! caught the testing-track keywords (`requires`/`ensures`/`transition`/
//! `expect`/…) landing with no hover coverage — a new keyword with no hover
//! path now fails a test, not a silent gap. The semantic-token legend clause
//! of ADR 0156 is already covered by `legend_drift.rs`; not duplicated here.

// bynk-lsp is a binary crate: include the pure modules directly (the pattern
// `legend_drift.rs` established). `symbols` satisfies completion's one
// `crate::symbols` reference.

use bynk_lsp::{completion, symbols};
use bynk_syntax::keywords::{CONTEXTUAL_KEYWORDS, KEYWORDS};

fn lowercase_keywords() -> impl Iterator<Item = &'static str> {
    KEYWORDS
        .iter()
        .map(|k| k.word)
        .filter(|w| w.chars().next().is_some_and(char::is_lowercase))
}

#[test]
fn every_lowercase_keyword_has_a_completion_doc() {
    for word in lowercase_keywords() {
        assert!(
            completion::keyword_doc(word).is_some(),
            "{word}: no completion doc in the `keywords` registry"
        );
    }
}

#[test]
fn every_lowercase_keyword_has_a_hover_path() {
    for word in lowercase_keywords() {
        // A minimal fixture with the keyword as a bare token — enough to
        // exercise `describe_keyword_at`'s lexical fallback without needing a
        // syntactically valid surrounding construct (hover works mid-edit).
        let source = format!("context Scaffold\n\n{word}");
        let offset = source.rfind(word).expect("word is in the fixture");
        assert!(
            symbols::describe_keyword_at(&source, offset).is_some(),
            "{word}: no hover path — describe_keyword_at returned None"
        );
    }
}

/// v0.137.0 (ADR 0161): the same floor over the *contextual* keywords
/// (`key`/`store`) — lexed as identifiers, so absent from `KEYWORDS` and unseen
/// by `describe_keyword_at`. Each must resolve to a hover in the one construct
/// that makes it a keyword (an `agent` body). This is the tooth that would have
/// caught #476, where `key`/`store` fell through every hover path.
#[test]
fn every_contextual_keyword_has_a_hover_path() {
    // A minimal agent exercising both contextual keywords in the same body.
    // State-migrations S2 (#2224): `migrate` and `drop` are keywords only in
    // an agent's migration step.
    // State-migrations S3 (#2248): `replaces` after an agent's name, and
    // `removed` before `agent` at item position.
    let source = "context Scaffold\n\nagent A replaces Z {\n  key id: String\n  store items: Cell[Int] = 0\n  migrate from 1 {\n    drop gone\n  }\n  on call read() -> Effect[Int] {\n    Effect.pure(0)\n  }\n}\n\nremoved agent Y\n";
    // #2137: `advance` is a keyword only in a test case's statement position.
    let case = "suite Scaffold {\n  case \"t\" {\n    advance Clock by 1.minutes\n  }\n}\n";
    for kw in CONTEXTUAL_KEYWORDS.iter().filter(|k| k.word == "advance") {
        let offset = case.find(kw.word).expect("`advance` is in the fixture");
        assert!(
            symbols::describe_advance_at(case, offset).is_some(),
            "advance: no hover path — describe_advance_at returned None"
        );
    }
    // #2640: `par` is a keyword only before `{` of a `par { … }` block.
    let block = "context Scaffold\n\nservice s {\n  on call() -> Effect[Int] {\n    par {\n      let a <- f()\n      let b <- g()\n    }\n    Effect.pure(a + b)\n  }\n}\n";
    for kw in CONTEXTUAL_KEYWORDS.iter().filter(|k| k.word == "par") {
        let offset = block.find(kw.word).expect("`par` is in the fixture");
        assert!(
            symbols::describe_par_at(block, offset).is_some(),
            "par: no hover path — describe_par_at returned None"
        );
    }
    // #2618: `private` is a keyword only before a declaration keyword.
    let modifier = "commons Scaffold\n\nprivate fn f() -> Int { 0 }\n";
    for kw in CONTEXTUAL_KEYWORDS.iter().filter(|k| k.word == "private") {
        let offset = modifier.find(kw.word).expect("`private` is in the fixture");
        assert!(
            symbols::describe_private_at(modifier, offset).is_some(),
            "private: no hover path — describe_private_at returned None"
        );
    }
    // Constrained generics slice 1: `derives` is a keyword only in a type
    // declaration's trailing `derives Ord` clause.
    let derives = "context Scaffold\n\ntype Priority = enum { Low, High } derives Ord\n";
    for kw in CONTEXTUAL_KEYWORDS.iter().filter(|k| k.word == "derives") {
        let offset = derives.find(kw.word).expect("`derives` is in the fixture");
        assert!(
            symbols::describe_derives_at(derives, offset).is_some(),
            "derives: no hover path — describe_derives_at returned None"
        );
    }
    for kw in CONTEXTUAL_KEYWORDS
        .iter()
        .filter(|k| !matches!(k.word, "advance" | "par" | "derives" | "private"))
    {
        let offset = source
            .find(kw.word)
            .expect("contextual keyword is in the fixture");
        assert!(
            symbols::describe_agent_state_at(source, offset).is_some(),
            "{}: no hover path — describe_agent_state_at returned None",
            kw.word
        );
    }
}
