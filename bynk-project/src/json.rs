//! The one JSON string escaper for every manifest `bynk-emit` hand-renders.
//!
//! Several build outputs are JSON that `bynk-emit` writes by hand rather than
//! via serde (`bynk-contracts.json`, `bynk-secrets.json`, `package.json`, the
//! source map): the shapes are a handful of fields each, and staying
//! serde-free keeps the dependency footprint honest. What they must share is
//! the escaping — every one of them interpolates text that originated in
//! Bynk source (a secret name from a `StrLit`, an adapter's npm package
//! name, a file path), so as far as the renderer is concerned the input is
//! arbitrary.
//!
//! This lived as four near-copies, one per rendering module. Three agreed;
//! the `package.json` one escaped only `"` and `\`, so an adapter-declared
//! package name or version range carrying a control character emitted a
//! literal control character inside a JSON string — a parse error, i.e. an
//! invalid `package.json` in the build output. One definition means a fix
//! like that cannot land in three places and miss the fourth.
//!
//! P4.0 (#1113, [DECISION C]): moved here from `bynk-emit/src/json.rs` so
//! `bynk-project`'s own `paths::render_package_json` (its sole in-crate use)
//! doesn't need a dependency back on `bynk-emit`; `bynk-emit`'s four other
//! call sites (`secrets.rs`, `contracts.rs`, `source_map.rs`, `emit.rs`) now
//! depend on this copy instead of a crate-local one, one definition either
//! way.

use std::fmt::Write as _;

/// Render `s` as a double-quoted JSON string literal, escaping the two
/// structural characters (`"` and `\`) and the whole C0 control range.
///
/// The C0 range matters and is easy to forget: RFC 8259 forbids an unescaped
/// character below U+0020 inside a string, so a stray newline or NUL is the
/// difference between a manifest that parses and one that merely usually
/// parses. Characters at or above U+0020 pass through as-is — the output is
/// UTF-8 JSON, so there is no reason to `\u`-escape non-ASCII.
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // JSON also has short escapes for U+0008 (`\b`) and U+000C (`\f`),
            // but they are written `\u0008` and `\u000c` here, which decode to the
            // same value; the rest of C0 has only the `\u00xx` form.
            // `write!` into the buffer rather than `push_str(&format!(…))` (what
            // two of the four copies this replaces did) — no `String` allocated
            // per control character.
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_merely_quoted() {
        assert_eq!(json_string("AUTH_JWT_SECRET"), "\"AUTH_JWT_SECRET\"");
        assert_eq!(json_string(""), "\"\"");
    }

    #[test]
    fn structural_characters_are_escaped() {
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
    }

    #[test]
    fn the_control_range_is_escaped() {
        assert_eq!(json_string("a\nb"), "\"a\\nb\"");
        assert_eq!(json_string("a\rb"), "\"a\\rb\"");
        assert_eq!(json_string("a\tb"), "\"a\\tb\"");
        assert_eq!(json_string("a\u{1}b"), "\"a\\u0001b\"");
        assert_eq!(json_string("a\u{0}b"), "\"a\\u0000b\"");
        assert_eq!(json_string("a\u{1f}b"), "\"a\\u001fb\"");
    }

    /// U+0020 is the first character JSON allows unescaped; the boundary is
    /// exactly where the C0 arm stops.
    #[test]
    fn space_and_above_pass_through() {
        assert_eq!(json_string("a b"), "\"a b\"");
        assert_eq!(json_string("a\u{7f}b"), "\"a\u{7f}b\"");
        assert_eq!(json_string("héllo ☃"), "\"héllo ☃\"");
    }
}
