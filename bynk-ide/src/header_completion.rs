//! #3911: completion inside `@name("…")` on a field of a record an HTTP handler takes as its
//! `headers` parameter — the header names a service commonly reads.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::completion::{Completion, CompletionKind, for_each_unit, innermost_open_brace};
use bynk_syntax::ast::{CommonsItem, HandlerKind, SourceUnit, TypeRef};

/// Header names, lowercase as they are written in `@name`, and what each carries.
const COMMON_HEADERS: &[(&str, &str)] = &[
    ("accept", "the media types the client accepts"),
    ("accept-language", "the languages the client prefers"),
    (
        "authorization",
        "credentials (an actor's seam consumes this, so a handler does not see it)",
    ),
    ("cache-control", "caching directives"),
    ("content-length", "the size of the body, in octets"),
    ("content-type", "the media type of the body"),
    ("date", "when the request was made"),
    (
        "idempotency-key",
        "a client-chosen key that makes a retry safe",
    ),
    ("if-match", "a precondition on the resource's current ETag"),
    (
        "if-modified-since",
        "a precondition on the resource's modification time",
    ),
    (
        "if-none-match",
        "a precondition: the ETags the client already has",
    ),
    (
        "if-unmodified-since",
        "a precondition on the resource's modification time",
    ),
    ("origin", "the origin of a cross-origin request"),
    ("range", "the part of the resource requested"),
    ("referer", "the page the request came from"),
    ("user-agent", "the client software"),
    ("x-forwarded-for", "the client address, as a proxy saw it"),
    ("x-request-id", "a caller-chosen id to correlate logs"),
];

/// The header names to offer when the cursor at `offset` is inside the string of an `@name("…")`
/// on a field of a record that some handler of this source or project takes as `headers`, or `None`.
pub fn header_name_candidates_at(
    text: &str,
    offset: usize,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<Vec<Completion>> {
    let before = text.get(..offset)?;
    let line = before.rsplit('\n').next().unwrap_or("");
    let at = line.rfind("@name(\"")?;
    if line[at + 7..].contains('"') {
        return None;
    }
    // The record the cursor is in: the word before the innermost open brace.
    let open = innermost_open_brace(text, offset)?;
    let head = text[..open].trim_end().trim_end_matches('=').trim_end();
    let name = head
        .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()?;
    if !taken_as_headers(name, text, files) {
        return None;
    }
    Some(
        COMMON_HEADERS
            .iter()
            .map(|(n, doc)| {
                Completion::item(*n, CompletionKind::Constant, Some((*doc).to_string()))
            })
            .collect(),
    )
}

/// Whether some HTTP handler takes a record named `record` as its `headers` parameter.
fn taken_as_headers(
    record: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> bool {
    let mut taken = false;
    for_each_unit(doc_text, files, |unit| {
        let items = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            _ => return,
        };
        for item in items {
            if let CommonsItem::Service(svc) = item {
                taken |= svc.handlers.iter().any(|h| {
                    matches!(h.kind, HandlerKind::Http { .. })
                        && h.params.iter().any(|p| {
                            p.name.name == "headers"
                                && matches!(&p.type_ref, TypeRef::Named(id) if id.name == record)
                        })
                });
            }
        }
    });
    taken
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "context api\n\ntype Prefs = {\n  @name(\"x\") n: Int,\n}\n\ntype Up = {\n  @name(\"con\") length: Int,\n}\n\nservice api from http {\n  on GET(\"/u\") (headers: Up, cookies: Prefs) -> Effect[HttpResult[String]] by v: Visitor { Ok(\"x\") }\n}\n";

    /// The offset just after `needle`'s `@name("` (the cursor inside the string).
    fn inside(needle: &str) -> usize {
        SRC.find(needle).unwrap() + needle.len()
    }

    #[test]
    fn header_names_are_offered_in_a_headers_records_name_annotation() {
        let got = header_name_candidates_at(SRC, inside("@name(\"con"), None).expect("offers");
        assert!(got.iter().any(|c| c.label == "content-length"));
        assert!(got.iter().any(|c| c.label == "idempotency-key"));
    }

    /// Two records whose `@name` lines share the same indented prefix: the cursor's own record
    /// decides, wherever the other one sits.
    #[test]
    fn the_cursors_record_decides_when_two_name_lines_share_a_prefix() {
        // `  @name("` opens both records' lines; only the second record is the `headers` one.
        assert!(header_name_candidates_at(SRC, inside("@name(\"x"), None).is_none());
        assert!(header_name_candidates_at(SRC, inside("@name(\"con"), None).is_some());
    }

    #[test]
    fn a_closed_string_gets_none() {
        let src = SRC.replace("@name(\"con\") length", "@name(\"con\") len");
        let at = src.find("@name(\"con\")").unwrap() + "@name(\"con\") le".len();
        assert!(header_name_candidates_at(&src, at, None).is_none());
    }
}
