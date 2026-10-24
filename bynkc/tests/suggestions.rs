//! v0.26 (ADR 0054): the seed quick-fixes carry machine-applicable
//! [`Suggestion`]s whose `given`-clause edits are list-aware. The position
//! matrix below pins the **exact emitted text** for first / middle / last /
//! only positions (and add-to-existing / synthesise-absent) — fix
//! *correctness* is pinned here in `bynkc`, with no LSP involved.

use std::path::{Path, PathBuf};

use bynk_syntax::error::{Applicability, Suggestion};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/suggestions/src")
}

/// The handlers context with a parameterised `given` clause (pass a leading
/// space, e.g. `" given Alpha, Beta"`, or `""` for no clause) and body.
fn handlers_file(given: &str, body: &str) -> String {
    format!(
        "\
context app.handlers

capability Alpha {{
  fn ping() -> Effect[Int]
}}

capability Beta {{
  fn ping() -> Effect[Int]
}}

capability Gamma {{
  fn ping() -> Effect[Int]
}}

provides Alpha = AlphaImpl {{
  fn ping() -> Effect[Int] {{
    1
  }}
}}

provides Beta = BetaImpl {{
  fn ping() -> Effect[Int] {{
    2
  }}
}}

provides Gamma = GammaImpl {{
  fn ping() -> Effect[Int] {{
    3
  }}
}}

service handlers {{
  on call(x: Int) -> Effect[Int]{given} {{
{body}
  }}
}}
"
    )
}

/// Diagnose the project with `text` overlaid on `file`, returning that
/// file's diagnostics.
///
/// Content-ownership track (#1086) slice 5: a cross-context fixture needs
/// its *other* files readable too — `bynk-emit`'s disk-read fallback used
/// to fill those in silently; the base sources map must be complete before
/// `file`'s overlaid content lands on top.
fn diagnose_with(file: &str, text: &str) -> Vec<bynk_ide::Diagnostic> {
    let root = fixture_root();
    let mut overlay =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    overlay.insert(root.join(file), text.to_string());
    let result = bynk_ide::diagnose_project(&root, &overlay);
    result
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().replace('\\', "/") == file)
        .map(|f| f.diagnostics.clone())
        .unwrap_or_default()
}

/// Exactly one diagnostic of `category`, carrying exactly one
/// `MachineApplicable` suggestion — returned for application.
fn sole_suggestion(diags: &[bynk_ide::Diagnostic], category: &str) -> Suggestion {
    let matching: Vec<_> = diags
        .iter()
        .filter(|d| d.error.category == category)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one `{category}`; got: {:?}",
        diags.iter().map(|d| d.error.category).collect::<Vec<_>>()
    );
    let suggestions = &matching[0].error.suggestions;
    assert_eq!(suggestions.len(), 1, "expected exactly one suggestion");
    let s = suggestions[0].clone();
    assert_eq!(s.applicability, Applicability::MachineApplicable);
    s
}

/// The applied fix must itself diagnose clean — a suggestion that leaves
/// any diagnostic behind is not machine-applicable.
fn assert_clean(file: &str, text: &str) {
    let diags = diagnose_with(file, text);
    assert!(
        diags.is_empty(),
        "applied fix left diagnostics: {:?}",
        diags.iter().map(|d| d.error.category).collect::<Vec<_>>()
    );
}

/// Apply a suggestion's edits (span → replacement) to the source text.
fn apply(text: &str, s: &Suggestion) -> String {
    let mut edits = s.edits.clone();
    edits.sort_by_key(|(span, _)| span.start);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (span, replacement) in &edits {
        out.push_str(&text[last..span.start]);
        out.push_str(replacement);
        last = span.end;
    }
    out.push_str(&text[last..]);
    out
}

// -- remove unused capability: the position matrix --

const USES_BETA_GAMMA: &str = "    let b <- Beta.ping()\n    let g <- Gamma.ping()\n    b + g";
const USES_ALPHA_GAMMA: &str = "    let a <- Alpha.ping()\n    let g <- Gamma.ping()\n    a + g";
const USES_ALPHA_BETA: &str = "    let a <- Alpha.ping()\n    let b <- Beta.ping()\n    a + b";
const USES_ALPHA: &str = "    let a <- Alpha.ping()\n    a";
const USES_NONE: &str = "    42";

#[test]
fn remove_first_capability() {
    let text = handlers_file(" given Alpha, Beta, Gamma", USES_BETA_GAMMA);
    let s = sole_suggestion(
        &diagnose_with("app/handlers.bynk", &text),
        "bynk.given.unused_capability",
    );
    assert_eq!(s.message, "remove `Alpha` from the `given` clause");
    let fixed = apply(&text, &s);
    assert_eq!(fixed, handlers_file(" given Beta, Gamma", USES_BETA_GAMMA));
    assert_clean("app/handlers.bynk", &fixed);
}

#[test]
fn remove_middle_capability() {
    let text = handlers_file(" given Alpha, Beta, Gamma", USES_ALPHA_GAMMA);
    let s = sole_suggestion(
        &diagnose_with("app/handlers.bynk", &text),
        "bynk.given.unused_capability",
    );
    let fixed = apply(&text, &s);
    assert_eq!(
        fixed,
        handlers_file(" given Alpha, Gamma", USES_ALPHA_GAMMA)
    );
    assert_clean("app/handlers.bynk", &fixed);
}

#[test]
fn remove_last_capability() {
    let text = handlers_file(" given Alpha, Beta, Gamma", USES_ALPHA_BETA);
    let s = sole_suggestion(
        &diagnose_with("app/handlers.bynk", &text),
        "bynk.given.unused_capability",
    );
    let fixed = apply(&text, &s);
    assert_eq!(fixed, handlers_file(" given Alpha, Beta", USES_ALPHA_BETA));
    assert_clean("app/handlers.bynk", &fixed);
}

#[test]
fn remove_only_capability_drops_the_given_keyword() {
    let text = handlers_file(" given Alpha", USES_NONE);
    let s = sole_suggestion(
        &diagnose_with("app/handlers.bynk", &text),
        "bynk.given.unused_capability",
    );
    let fixed = apply(&text, &s);
    assert_eq!(fixed, handlers_file("", USES_NONE));
    assert_clean("app/handlers.bynk", &fixed);
}

// -- add capability to `given` --

#[test]
fn add_capability_after_existing_entries() {
    let text = handlers_file(" given Alpha", USES_ALPHA_BETA);
    let s = sole_suggestion(
        &diagnose_with("app/handlers.bynk", &text),
        "bynk.given.undeclared_capability",
    );
    assert_eq!(s.message, "add `Beta` to the `given` clause");
    let fixed = apply(&text, &s);
    assert_eq!(fixed, handlers_file(" given Alpha, Beta", USES_ALPHA_BETA));
    assert_clean("app/handlers.bynk", &fixed);
}

#[test]
fn add_capability_synthesises_an_absent_clause() {
    let text = handlers_file("", USES_ALPHA);
    let s = sole_suggestion(
        &diagnose_with("app/handlers.bynk", &text),
        "bynk.given.undeclared_capability",
    );
    let fixed = apply(&text, &s);
    assert_eq!(fixed, handlers_file(" given Alpha", USES_ALPHA));
    assert_clean("app/handlers.bynk", &fixed);
}

// -- cross-context (`B.Cap`) add --

fn crossuse_file(given: &str) -> String {
    format!(
        "\
context app.crossuse

consumes platform.time as Time

service crossuse {{
  on call() -> Effect[Int]{given} {{
    let t <- Time.Clock.now()
    t
  }}
}}
"
    )
}

#[test]
fn add_cross_context_capability_synthesises_an_absent_clause() {
    let text = crossuse_file("");
    let s = sole_suggestion(
        &diagnose_with("app/crossuse.bynk", &text),
        "bynk.given.undeclared_capability",
    );
    // The clause entry is the *canonical* context path (the diagnosis site
    // sees the resolved name, not the `as Time` alias spelling) — valid
    // alongside alias-style calls, as the clean re-diagnosis below proves.
    assert_eq!(s.message, "add `platform.time.Clock` to the `given` clause");
    let fixed = apply(&text, &s);
    assert_eq!(fixed, crossuse_file(" given platform.time.Clock"));
    assert_clean("app/crossuse.bynk", &fixed);
}

// -- #2402: a provider calling an agent handler --

/// A provider that calls an agent handler `given Beta`, with the provider's
/// own `given` clause parameterised.
fn agentcall_file(given: &str) -> String {
    format!(
        "\
context app.agentcall

capability Alpha {{
  fn ping() -> Effect[Int]
}}

capability Beta {{
  fn ping() -> Effect[Int]
}}

capability Relay {{
  fn ping() -> Effect[Int]
}}

provides Alpha = AlphaImpl {{
  fn ping() -> Effect[Int] {{
    1
  }}
}}

provides Beta = BetaImpl {{
  fn ping() -> Effect[Int] {{
    2
  }}
}}

agent Counter {{
  key name: String

  store hits: Cell[Int]

  on call ping() -> Effect[Int] given Beta {{
    Beta.ping()
  }}
}}

provides Relay = AgentRelay{given} {{
  fn ping() -> Effect[Int] {{
    let a <- Alpha.ping()
    let c <- Counter(\"x\").ping()
    Effect.pure(a + c)
  }}
}}
"
    )
}

#[test]
fn add_agent_handler_capability_to_a_provider_given() {
    let text = agentcall_file(" given Alpha");
    let s = sole_suggestion(
        &diagnose_with("app/agentcall.bynk", &text),
        "bynk.provider.agent_call_undeclared_capability",
    );
    assert_eq!(s.message, "add `Beta` to the `given` clause");
    let fixed = apply(&text, &s);
    assert_eq!(fixed, agentcall_file(" given Alpha, Beta"));
    assert_clean("app/agentcall.bynk", &fixed);
}

// -- the baseline fixtures themselves are clean --

#[test]
fn baseline_fixtures_carry_no_diagnostics() {
    let result = bynk_ide::diagnose_project(
        &fixture_root(),
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (fixture_root()).to_path_buf(),
        )),
    );
    for f in &result.files {
        assert!(
            f.diagnostics.is_empty(),
            "{} unexpectedly has diagnostics: {:?}",
            f.source_path.display(),
            f.diagnostics
                .iter()
                .map(|d| d.error.category)
                .collect::<Vec<_>>()
        );
    }
}

// -- v0.40 (ADR 0073): the InRange-swap quick-fix --

#[test]
fn inrange_swap_int_quick_fix_swaps_the_bounds() {
    let inverted = "commons app.refine\n\ntype Age = Int where InRange(120, 0)\n";
    let diags = diagnose_with("app/refine.bynk", inverted);
    let s = sole_suggestion(&diags, "bynk.types.inverted_range");
    // Two edits, one per bound.
    assert_eq!(s.edits.len(), 2);
    let fixed = apply(inverted, &s);
    assert!(
        fixed.contains("InRange(0, 120)"),
        "bounds swapped in place: {fixed:?}"
    );
    assert_clean("app/refine.bynk", &fixed);
}

#[test]
fn inrange_swap_float_quick_fix_preserves_lexemes() {
    let inverted = "commons app.refine\n\ntype Ratio = Float where InRange(5.2, 1.1)\n";
    let diags = diagnose_with("app/refine.bynk", inverted);
    let s = sole_suggestion(&diags, "bynk.types.inverted_range");
    let fixed = apply(inverted, &s);
    assert!(
        fixed.contains("InRange(1.1, 5.2)"),
        "float bounds swapped, lexemes preserved: {fixed:?}"
    );
    assert_clean("app/refine.bynk", &fixed);
}

// -- #48: field-access-on-refined → "remove `.raw`" quick-fix --

#[test]
fn refined_raw_quick_fix_drops_dot_raw() {
    // `.raw` on a refined value is a common mistake (it's an opaque-type
    // accessor); the fix is to use the value directly — refined widens to base.
    let src = "commons app.refine\n\n\
               type Subject = String where NonEmpty\n\n\
               fn greet(s: Subject) -> String {\n  s.raw\n}\n";
    let diags = diagnose_with("app/refine.bynk", src);
    let s = sole_suggestion(&diags, "bynk.types.field_access_on_non_record");
    let fixed = apply(src, &s);
    assert!(
        !fixed.contains(".raw") && fixed.contains("  s\n"),
        "`.raw` removed, leaving the bare value: {fixed:?}"
    );
    assert_clean("app/refine.bynk", &fixed);
}

// -- #852: add missing record field(s) --

/// Overlay for `app/refine.bynk`: a `Point { x: Int, y: Int }` record plus an
/// `fn mk()` whose tail expression is `body` (a `Point { … }` literal).
fn point_commons(body: &str) -> String {
    format!(
        "commons app.refine\n\ntype Point = {{ x: Int, y: Int }}\n\nfn mk() -> Point {{\n  {body}\n}}\n"
    )
}

/// The suggestions on the first `bynk.resolve.missing_field` diagnostic.
fn missing_field_suggestions(diags: &[bynk_ide::Diagnostic]) -> Vec<Suggestion> {
    diags
        .iter()
        .find(|d| d.error.category == "bynk.resolve.missing_field")
        .map(|d| d.error.suggestions.clone())
        .unwrap_or_default()
}

/// A fixed buffer must format without error and be fmt-idempotent (#852's
/// "round-trips through fmt" acceptance condition).
fn assert_fmt_roundtrips(fixed: &str) {
    let opts = bynk_fmt::FormatOptions::default();
    let once = bynk_fmt::format_source(fixed, &opts).expect("fixed buffer formats");
    let twice = bynk_fmt::format_source(&once, &opts).expect("fmt is idempotent");
    assert_eq!(once, twice, "fmt not idempotent on the fixed buffer");
}

#[test]
fn add_single_missing_record_field_appends_after_the_last() {
    let text = point_commons("Point { x: 1 }");
    let s = sole_suggestion(
        &diagnose_with("app/refine.bynk", &text),
        "bynk.resolve.missing_field",
    );
    assert_eq!(s.message, "add field `y`");
    let fixed = apply(&text, &s);
    assert_eq!(fixed, point_commons("Point { x: 1, y: 0 }"));
    assert_clean("app/refine.bynk", &fixed);
    assert_fmt_roundtrips(&fixed);
}

#[test]
fn add_all_missing_record_fields_into_an_empty_literal() {
    let text = point_commons("Point {}");
    let diags = diagnose_with("app/refine.bynk", &text);
    // Two missing fields → two diagnostics, each keyed on the type name.
    let missing = diags
        .iter()
        .filter(|d| d.error.category == "bynk.resolve.missing_field")
        .count();
    assert_eq!(missing, 2);
    let sugs = missing_field_suggestions(&diags);
    // The first diagnostic carries a single-field fix *and* the all-missing one.
    assert!(sugs.iter().any(|s| s.message == "add field `x`"));
    let all = sugs
        .iter()
        .find(|s| s.message == "add all missing fields")
        .expect("all-missing fix on the first diagnostic");
    let fixed = apply(&text, all);
    assert_eq!(fixed, point_commons("Point { x: 0, y: 0 }"));
    assert_clean("app/refine.bynk", &fixed);
    assert_fmt_roundtrips(&fixed);
}

#[test]
fn empty_literal_fix_is_canonical_despite_interior_spacing() {
    // A non-canonical empty literal (`Point {  }`) — the fix replaces the whole
    // ` { … }` tail, so the result is fmt-canonical regardless of the original
    // interior spacing (the review's cosmetic-idempotency note).
    let text = point_commons("Point {  }");
    let all = missing_field_suggestions(&diagnose_with("app/refine.bynk", &text))
        .into_iter()
        .find(|s| s.message == "add all missing fields")
        .expect("all-missing fix");
    let fixed = apply(&text, &all);
    // The spaced `{  }` collapses to the canonical record region — the
    // wrapper is identical on both sides, so this pins the fix's own output.
    assert_eq!(fixed, point_commons("Point { x: 0, y: 0 }"));
    assert_clean("app/refine.bynk", &fixed);
    assert_fmt_roundtrips(&fixed);
}

#[test]
fn add_field_defaults_cover_option_and_list() {
    // `Option` defaults to `None`, `List` to `[]`; both re-check clean.
    let text = "commons app.refine\n\ntype Bag = { tag: Option[String], items: List[Int] }\n\nfn mk() -> Bag {\n  Bag {}\n}\n";
    let diags = diagnose_with("app/refine.bynk", text);
    let all = missing_field_suggestions(&diags)
        .into_iter()
        .find(|s| s.message == "add all missing fields")
        .expect("all-missing fix");
    let fixed = apply(text, &all);
    assert!(
        fixed.contains("Bag { tag: None, items: [] }"),
        "defaults filled: {fixed:?}"
    );
    assert_clean("app/refine.bynk", &fixed);
    assert_fmt_roundtrips(&fixed);
}

#[test]
fn missing_field_of_a_named_type_offers_no_autofix() {
    // A named type (here a refined `Age`) has no synthesised default: the
    // single missing field's diagnostic stands with no machine-applicable fix,
    // and — since the set is not wholly defaultable — no all-missing fix is
    // offered on the sibling `note` diagnostic either.
    let text = "commons app.refine\n\ntype Age = Int where InRange(0, 120)\n\ntype P = { age: Age, note: String }\n\nfn mk() -> P {\n  P {}\n}\n";
    let diags = diagnose_with("app/refine.bynk", text);
    let age = diags
        .iter()
        .find(|d| {
            d.error.category == "bynk.resolve.missing_field" && d.error.message.contains("`age`")
        })
        .expect("the `age` missing-field diagnostic");
    assert!(
        age.error.suggestions.is_empty(),
        "no autofix for a named-typed field"
    );
    assert!(
        diags
            .iter()
            .flat_map(|d| &d.error.suggestions)
            .all(|s| s.message != "add all missing fields"),
        "no all-missing fix when part of the set is non-defaultable"
    );
}

#[test]
fn refined_field_access_carries_the_widen_note_without_an_autofix() {
    // A non-`.raw` field on a refined value still gets the prescriptive note,
    // but no machine-applicable fix (there's no single mechanical edit).
    let src = "commons app.refine\n\n\
               type Subject = String where NonEmpty\n\n\
               fn f(s: Subject) -> String {\n  s.size\n}\n";
    let diags = diagnose_with("app/refine.bynk", src);
    let d = diags
        .iter()
        .find(|d| d.error.category == "bynk.types.field_access_on_non_record")
        .expect("the field-access error");
    assert!(
        d.error
            .notes
            .iter()
            .any(|n| n.contains("refined value is usable")),
        "carries the widen note: {:?}",
        d.error.notes
    );
    assert!(
        d.error.suggestions.is_empty(),
        "no auto-fix for a non-`.raw` field"
    );
}
