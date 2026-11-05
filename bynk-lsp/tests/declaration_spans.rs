//! #619 (v0.168): go-to-definition's same-file rung answers for an `actor`.
//!
//! `find_declaration_span` is rung 3 of go-to-definition's ladder (index →
//! locals → **same-file** → cross-file). It listed an arm per `CommonsItem`
//! variant and omitted `Actor`, and a `_ => {}` catch-all swallowed the miss —
//! so the `User` in `by u: User` resolved to nothing.
//!
//! The index rung above it resolves `SymbolKind::Actor` and normally answers
//! first, which is why this went unseen: it bites only where the index has not
//! resolved the offset — an unanalysed or mid-edit buffer, or a file outside the
//! analysed project. That reachability is also what these tests must work
//! around: driving the ladder would measure the index rung, not this one, so
//! they call `find_declaration_span` directly.
//!
//! Per ADR 0190 D6 the fixtures are *reference* offsets in real projects, read
//! from real `diagnose_project` output — `examples/todo/src/todos.bynk` for the
//! actor (the file the gap reproduces in), the compiler's own method-index
//! fixture for the divergence [`a_method_still_answers_to_its_bare_name`] pins.
//! `Backend::identifier_at` is transport-bound (it reads the open-document map)
//! and cannot be called from here, so [`name_at`] reproduces its one relevant
//! step — the token covering the offset — to turn an offset into the name the
//! rung is asked for.
//!
//! bynk-lsp is a binary crate: include the pure module directly (the pattern
//! `legend_drift.rs` established).
//!
//! This file is `exclude`d from the published `bynk-lsp` crate (Cargo.toml) —
//! `examples/todo` and the compiler's fixtures live outside this crate's
//! package, so a standalone `cargo test` on the published crate must not see
//! it. Skipping when a root is absent is not the alternative: ADR 0190 D6
//! requires these to read real output, and a skip could pass vacuously in-repo.

use bynk_lsp::symbols;
use std::path::Path;

/// The analysed text of `file` under `<crate>/{rel}` — real project output, so
/// a fixture that stopped analysing cannot quietly pass these as empty text.
fn analysed(rel: &str, file: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    // Content-ownership track (#1086) slice 3: bynk-testkit's complete
    // sources map, proved here as one of the representative call sites,
    // instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let r = bynk_ide::diagnose_project(&root, &sources);
    let f = r
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().ends_with(file))
        .unwrap_or_else(|| panic!("{file} analysed"));
    let errors: Vec<_> = f.diagnostics.iter().map(|d| d.error.category).collect();
    assert!(errors.is_empty(), "{file} has diagnostics: {errors:?}");
    f.text.clone()
}

/// The byte offset of `needle` at or after `anchor` — the reference site, as
/// distinct from the declaration of the same name earlier in the file.
fn at(text: &str, anchor: &str, needle: &str) -> usize {
    let a = text
        .find(anchor)
        .unwrap_or_else(|| panic!("`{anchor}` is not in the fixture"));
    a + text[a..]
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` is not at `{anchor}`"))
}

/// The identifier the cursor at `offset` sits on — the name go-to-definition
/// asks the rung for. See the module doc on why this is not `identifier_at`.
fn name_at(text: &str, offset: usize) -> String {
    let tokens = bynk_syntax::lexer::tokenize(text).expect("the fixture lexes");
    let t = tokens
        .iter()
        .find(|t| t.span.start <= offset && offset < t.span.end)
        .expect("a token covers the offset");
    text[t.span.start..t.span.end].to_string()
}

/// The gap: go-to-definition on the `User` in `by u: User` jumps to `actor
/// User`. Observed: nothing — the rung had no `Actor` arm.
#[test]
fn an_actor_reference_finds_its_declaration() {
    let text = analysed("../examples/todo/src", "todos.bynk");
    let name = name_at(&text, at(&text, "by u: User", "User"));
    assert_eq!(name, "User", "the reference offset names the actor");

    let span = symbols::find_declaration_span(&text, &name)
        .expect("`by u: User` resolves to the `actor User` declaration");
    assert_eq!(&text[span.start..span.end], "User");
    assert_eq!(
        span.start,
        at(&text, "actor User", "User"),
        "the jump lands on the declaration, not back on the reference"
    );
}

/// `type UserId` is declared *before* `actor User` and shares its prefix, so
/// this pins that the actor answers by an exact name — a prefix or substring
/// match would jump to the type, silently and plausibly.
#[test]
fn an_actor_is_not_confused_with_a_type_sharing_its_prefix() {
    let text = analysed("../examples/todo/src", "todos.bynk");
    for (name, decl) in [("User", "actor User"), ("UserId", "type UserId")] {
        let span = symbols::find_declaration_span(&text, name)
            .unwrap_or_else(|| panic!("`{name}` resolves"));
        assert_eq!(span.start, at(&text, decl, name), "`{name}` -> `{decl}`");
    }
}

/// ADR 0191 D2's deliberate divergence, pinned so the actor fix cannot quietly
/// take it with it: this rung matches a **method** by its bare name (`bump`
/// finds `fn Counter.bump`, the first declared), because bare-identifier
/// go-to-definition depends on it. `describe_item` guards on `FnName::Free`
/// instead and answers nothing here — the two disagree on purpose.
#[test]
fn a_method_still_answers_to_its_bare_name() {
    let text = analysed(
        "../bynk-tests/tests/fixtures/positive/216_method_index/src",
        "shop.bynk",
    );
    let span = symbols::find_declaration_span(&text, "bump").expect("a bare method name resolves");
    assert_eq!(
        span.start,
        at(&text, "fn Counter.bump", "bump"),
        "the first `bump` declared answers, as it did before the actor arm"
    );
    assert!(
        symbols::describe_symbol(&text, "bump").is_none(),
        "ADR 0191 D2: hover's renderer does *not* match a method by its bare \
         name, and this rung deliberately still does"
    );
}
