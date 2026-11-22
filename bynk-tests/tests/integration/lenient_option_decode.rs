//! #1887: the lenient `Option` decoder's emitted shape. The behaviour is proved at runtime by the
//! `1887_lenient_option_decode` fixture; these pin the rule order the round-trip invariant depends
//! on (absent or `null`, then the exact tagged forms, then a bare value) and the nesting an
//! `Option[Option[Int]]` relies on.
//!
//! 1B.2b (#4028): moved from `bynk-emit` (`emitter/serialisation.rs`), as the tests drive the
//! driver's in-memory pipeline.

#[path = "../support/emit_pipeline.rs"]
mod emit_pipeline;
use emit_pipeline::emit_project;

/// The text a failed bare `Option` value appends to its `expected`. Pinned here as a literal;
/// `serialisation::tests::option_forms_suffix_spelling` ties it to the emitter's own constant.
const OPTION_FORMS_SUFFIX: &str =
    " | null | {\"kind\": \"None\"} | {\"kind\": \"Some\", \"value\": ...}";

const SRC: &str = "commons d.opt\n\n\
        type N = { n: Option[Option[Int]] }\n\n\
        fn dec(s: String) -> Bool {\n  match Json.decode[N](s) {\n    Ok(_) => true\n    Err(_) => false\n  }\n}\n";

/// The emitted function `name`, from its signature to its closing brace.
fn function_body<'a>(text: &'a str, name: &str) -> &'a str {
    let start = text
        .find(&format!("function {name}("))
        .unwrap_or_else(|| panic!("`{name}` is emitted:\n{text}"));
    let len = text[start..].find("\n}\n").expect("the function closes");
    &text[start..start + len]
}

/// The byte offset of `needle` in `body`, which must contain it.
fn at(body: &str, needle: &str) -> usize {
    body.find(needle)
        .unwrap_or_else(|| panic!("`{needle}` is in:\n{body}"))
}

fn emitted() -> String {
    let out = emit_project(
        &[("d/opt.bynk", SRC)],
        bynk_check::project_model::BuildTarget::Bundle,
    );
    out.text("d/opt.ts").to_string()
}

#[test]
fn the_tagged_forms_are_tried_before_a_bare_value() {
    let text = emitted();
    let outer = function_body(&text, "__deserialise_Option_Option_Int");
    let absent = at(outer, "if (json === undefined || json === null) {");
    let none = at(
        outer,
        "if (obj[\"kind\"] === \"None\" && __keys.length === 1) {",
    );
    let some = at(
        outer,
        "if (obj[\"kind\"] === \"Some\" && __keys.length === 2 && \"value\" in obj) {",
    );
    let tagged = at(
        outer,
        "__option_value_Option_Int(obj[\"value\"], `${path}.value`)",
    );
    let bare = at(outer, "const __b = __option_value_Option_Int(json, path);");
    assert!(
        absent < none && none < some && some < tagged && tagged < bare,
        "{outer}"
    );
}

#[test]
fn the_value_decoder_delegates_to_the_inner_option_codec() {
    let text = emitted();
    // Module-local: only this module's `Option` decoder calls it.
    assert!(!text.contains("export function __option_value_"), "{text}");
    let value = function_body(&text, "__option_value_Option_Int");
    at(value, "__deserialise_Option_Int(raw, at)");
    // The inner `Option[Int]` decoder is lenient in turn, so a bare `1`
    // nests to `Some(Some(1))`.
    let inner = function_body(&text, "__deserialise_Option_Int");
    at(inner, "const __b = __option_value_Int(json, path);");
}

#[test]
fn a_failed_bare_value_names_every_option_form_only_at_its_own_path() {
    let text = emitted();
    let inner = function_body(&text, "__deserialise_Option_Int");
    at(
        inner,
        "if (__b.error.kind === \"StructuralMismatch\" && __b.error.path === path && !__b.error.expected.endsWith(",
    );
    at(inner, &OPTION_FORMS_SUFFIX.replace('"', "\\\""));
}
