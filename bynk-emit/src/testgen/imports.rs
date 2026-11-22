//! The runtime import list of a test module: folding bindings into it.
//!
//! Split out of `system_support.rs` when the test-harness generation moved into
//! `bynk-emit` (1B.2a, #4028), so that no file arrives over 2,000 production lines. The
//! predicted imports now have one home, for 1B.4 and 1B.5.

use super::*;
use crate::emitter::RuntimeUse;

/// #1486: `emitter/workers.rs`'s own `append_missing_bindings`'s sibling
/// duplicate — same reason that one and `emitter/workers_entry.rs`'s own copy
/// don't share a single implementation (each doc comment explains why for its
/// own file): folds `extra`'s comma-separated bindings (e.g.
/// [`crate::emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS`]) into `names` directly,
/// skipping anything already present under the same "strip an optional `type
/// ` prefix, compare bare names" dedup rule. `emit_test_module`/
/// `emit_integration_module`'s own runtime-import statement is now a real
/// `TsDecl::Import` built once every case has lowered (deferred, not spliced
/// into already-printed text the way the pre-#1486 code's own
/// `emitter::inject_runtime_imports` post-pass did — see each caller's own
/// doc comment) — this is that deferred construction's own merge step.
pub(super) fn append_missing_bindings(names: &mut Vec<String>, extra: &str) {
    fn bare(binding: &str) -> &str {
        binding
            .trim()
            .strip_prefix("type ")
            .unwrap_or(binding.trim())
    }
    let present: std::collections::HashSet<&str> = names.iter().map(|s| bare(s)).collect();
    let mut to_add: Vec<String> = Vec::new();
    for b in extra.split(',').map(str::trim) {
        if !b.is_empty() && !present.contains(bare(b)) && !to_add.iter().any(|a| bare(a) == bare(b))
        {
            to_add.push(b.to_string());
        }
    }
    names.extend(to_add);
}

/// Fold every conditional runtime helper a test module's lowered bodies reached
/// for (as recorded on `ru`) into its runtime import's `names`.
///
/// The one list behind both the unit-suite and the integration-suite modules
/// (#3398): each used to carry its own copy, and a helper added to one flag
/// chain only (`__bynkPgRow`, for the typed `Postgres` reads) left the other
/// without it. A new `RuntimeUse` flag that a test body can set is added here.
pub(super) fn append_runtime_use_imports(names: &mut Vec<String>, ru: &RuntimeUse) {
    if ru.boundary_codec() {
        append_missing_bindings(names, emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS);
    }
    if ru.json_codec() {
        append_missing_bindings(names, emitter::JSON_CODEC_RUNTIME_IMPORTS);
    }
    if ru.bytes() {
        append_missing_bindings(names, emitter::BYTES_RUNTIME_IMPORTS);
    }
    if ru.bytes_ops() {
        append_missing_bindings(names, emitter::BYTES_OPS_RUNTIME_IMPORTS);
    }
    if ru.time_ops() || ru.pg_rows() {
        append_missing_bindings(names, &emitter::time_runtime_imports(ru));
    }
    if ru.iso_date() {
        append_missing_bindings(names, emitter::ISO_DATE_RUNTIME_IMPORTS);
    }
    if ru.yaml() {
        append_missing_bindings(names, emitter::YAML_RUNTIME_IMPORTS);
    }
    if ru.settle_all() {
        append_missing_bindings(names, emitter::SETTLE_ALL_RUNTIME_IMPORTS);
    }
    append_missing_bindings(names, emitter::effect_runtime_imports(ru));
    if ru.fault() {
        append_missing_bindings(names, emitter::FAULT_RUNTIME_IMPORTS);
    }
    if ru.url_ops() {
        append_missing_bindings(names, emitter::URL_OPS_RUNTIME_IMPORTS);
    }
    if ru.request_headers() {
        append_missing_bindings(names, emitter::REQUEST_HEADERS_RUNTIME_IMPORTS);
    }
    if ru.request_cookies() {
        append_missing_bindings(names, emitter::REQUEST_COOKIES_RUNTIME_IMPORTS);
    }
    if ru.session_signing() {
        append_missing_bindings(names, emitter::SESSION_SIGNING_RUNTIME_IMPORTS);
    }
    if ru.request_query() {
        append_missing_bindings(names, emitter::REQUEST_QUERY_RUNTIME_IMPORTS);
    }
    if ru.query_string() {
        append_missing_bindings(names, emitter::QUERY_STRING_RUNTIME_IMPORTS);
    }
    if ru.form_codec() {
        append_missing_bindings(names, emitter::FORM_CODEC_RUNTIME_IMPORTS);
    }
    if ru.multipart_form() {
        append_missing_bindings(names, emitter::MULTIPART_FORM_RUNTIME_IMPORTS);
    }
    if ru.cookie_string() {
        append_missing_bindings(names, emitter::COOKIE_STRING_RUNTIME_IMPORTS);
    }
    if ru.request_header_pairs() {
        append_missing_bindings(names, ", __requestHeaderPairs");
    }
    if ru.request_query_search() {
        append_missing_bindings(names, ", __requestQuerySearch");
    }
    if ru.token_claims() {
        append_missing_bindings(names, ", __tokenClaims");
    }
    if ru.test_connection() {
        append_missing_bindings(names, ", TestConnection");
    }
    if ru.eq() {
        append_missing_bindings(names, emitter::EQ_RUNTIME_IMPORTS);
    }
    // #1934: a case that compares a typed http outcome with `Ok(…)`.
    if ru.http_result() {
        append_missing_bindings(names, "HttpResult");
    }
    // #2839: a case that compares a queue verdict with a bare variant.
    if ru.queue_result() {
        append_missing_bindings(names, "QueueResult");
    }
    if ru.int() {
        append_missing_bindings(names, emitter::INT_RUNTIME_IMPORTS);
    }
    append_missing_bindings(names, &ru.cmp_imports());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_bindings_that_are_absent() {
        let mut names = vec!["Ok".to_string(), "Err".to_string()];
        append_missing_bindings(&mut names, emitter::BYTES_RUNTIME_IMPORTS);
        assert!(names.iter().any(|n| n == "__bynkBytesEqual"), "{names:?}");
        assert!(
            names.iter().any(|n| n == "__bynkBytesDecodeUtf8"),
            "{names:?}"
        );
    }

    #[test]
    fn skips_bindings_already_present() {
        let mut names = vec![
            "Ok".to_string(),
            "Err".to_string(),
            "type Result".to_string(),
        ];
        append_missing_bindings(&mut names, emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS);
        assert_eq!(
            names.iter().filter(|n| n.as_str() == "Ok").count(),
            1,
            "`Ok` was already present and must not repeat: {names:?}"
        );
        assert_eq!(
            names,
            vec!["Ok", "Err", "type Result", "type __BoundaryError"],
        );
    }

    /// The bare name is what collides, so `type BoundaryError` must match an
    /// existing `BoundaryError`.
    #[test]
    fn matches_a_type_prefixed_group_binding_against_a_bare_one() {
        let mut names = vec![
            "Ok".to_string(),
            "Err".to_string(),
            "type Result".to_string(),
            "__BoundaryError".to_string(),
        ];
        let before = names.clone();
        append_missing_bindings(&mut names, emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS);
        assert_eq!(
            names, before,
            "every binding was already present, so `names` is untouched"
        );
    }

    /// …and the other direction: a bare group binding against an existing
    /// `type`-prefixed one. This is the case that would regress if `bare`
    /// were applied to only one side of the comparison.
    #[test]
    fn matches_a_bare_group_binding_against_a_type_prefixed_one() {
        let mut names = vec!["type Ok".to_string(), "type Err".to_string()];
        let before = names.clone();
        append_missing_bindings(&mut names, ", Ok, Err");
        assert_eq!(
            names, before,
            "a bare binding must match an existing `type`-prefixed one: {names:?}"
        );
    }

    /// The two calls run back to back over the same `names`, so the second
    /// sees the first's own output as `present` — the overlap between the
    /// groups (`Ok`, `Err`, `type Result`) must not double up.
    #[test]
    fn composes_across_two_sequential_calls() {
        let mut names = vec![
            "Ok".to_string(),
            "Err".to_string(),
            "type Result".to_string(),
        ];
        append_missing_bindings(&mut names, emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS);
        append_missing_bindings(&mut names, emitter::JSON_CODEC_RUNTIME_IMPORTS);
        assert_eq!(
            names,
            vec![
                "Ok",
                "Err",
                "type Result",
                "type __BoundaryError",
                "type __JsonValue",
                "type DecodeError",
            ],
            "the shared bindings must be added once: {names:?}"
        );
    }
}
