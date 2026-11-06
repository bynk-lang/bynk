//! The formatter's round-trip-guard diagnostic renders against its source
//! without panicking (#735).
//!
//! This was a `bynk-fmt` unit test, which rendered the real diagnostic with
//! `bynk-render`. When D2 merged `bynk-render` into this crate it moved here, so
//! `bynk-fmt` needs no dev-dependency on the presentation layer. The guard's
//! constructor (`roundtrip_error`) is private to `bynk-fmt` and no valid source
//! trips the guard, so this builds the same `CompileError` the constructor does
//! (code `bynk.fmt.roundtrip`, default span, no labels); `bynk-fmt` keeps its own
//! `err.span.end <= src.len()` assertion on the real value.

use bynk_driver::render::render_errors;
use bynk_syntax::CompileError;
use bynk_syntax::span::Span;

#[test]
fn roundtrip_error_renders_against_source_without_panicking() {
    let err = CompileError {
        category: "bynk.fmt.roundtrip",
        span: Span::default(),
        message:
            "the formatter produced output that no longer parses — the file was left unchanged"
                .to_string(),
        labels: Vec::new(),
        notes: vec![
            "this is a formatter bug, not a problem with your source; please report it \
             with the file that triggered it"
                .to_string(),
        ],
        suggestions: Vec::new(),
    };
    let rendered = render_errors(std::slice::from_ref(&err), "commons x {}", "<test>");
    assert!(
        rendered.contains("bynk.fmt.roundtrip"),
        "diagnostic did not render: {rendered}"
    );
}
