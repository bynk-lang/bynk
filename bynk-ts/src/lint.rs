//! The textual lint over [`crate::TsStmt::verbatim`] content (Q2, `design/
//! tracks/the-typescript-tree.md` §3.2's "real gap this settling pass found
//! and closes"). A byte-golden fixture is blind to what's *inside* an
//! opaque `Verbatim` block — this scans the wrapped text directly for the
//! six constructs R7.1 forbids the tree from ever representing (`enum`,
//! `namespace`, a decorator, a constructor parameter property, `: any`/
//! `as any`), so a `Verbatim` block smuggling one of them in stays visible
//! even while every golden fixture stays green.
//!
//! Pattern match over text, not a real TS parser — same posture `xtask`'s
//! own `ts_any` probe (`xtask/src/greenfield_status.rs`) already takes for
//! the identical `any` patterns, reused here rather than re-derived. A `//`
//! or `/* */` comment line, and the contents of a `"..."`/`'...'`/
//! `` `...` `` string literal, are blanked before matching — #1538's own
//! real gap, found wiring this over real compiled output: a message string
//! naming `namespace` in prose (`"...requires a KV namespace binding..."`)
//! is not a `namespace` declaration.
//!
//! #1538 wires this into `bynk-tests/tests/tsc_verify.rs`, over every
//! `Verbatim`/`VerbatimExpr` leaf `TsProgram::verbatim_content` finds by
//! walking a compiled fixture's tree — real, CI-visible `Verbatim` content
//! to check, closing Decision F's own deferral ("wiring it into a real
//! CI-visible check over compiled output is meaningful only once real
//! `Verbatim` content exists to check").

/// One construct [`verbatim_violations`] found, and the line it was on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub construct: &'static str,
    pub line: String,
}

/// Scan `text` (a `Verbatim` statement's own wrapped TypeScript) for every
/// line matching one of the six banned constructs. Order of the checks
/// within a line matters only for which `construct` label a line already
/// matching two patterns gets — real emitted lines don't do that in
/// practice, so the first match wins and the rest of that line isn't
/// checked further.
pub fn verbatim_violations(text: &str) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut in_block_comment = false;
    for line in text.lines() {
        if in_block_comment {
            if let Some(end) = line.find("*/") {
                in_block_comment = false;
                // Only the text after the closing `*/` is real code — but no
                // real emitted line today has code following a block
                // comment's close on the same line, so, matching this
                // module's own "no inline trailing comment" simplicity for
                // `//`, the rest of the line is skipped rather than
                // re-scanned from `end + 2`.
                let _ = end;
            }
            continue;
        }
        if is_line_comment(line) {
            continue;
        }
        if let Some(start) = line.find("/*") {
            if let Some(end) = line[start..].find("*/") {
                // A block comment that opens and closes on the same line —
                // blank it out and scan what's left, the same treatment a
                // string literal gets below.
                let mut blanked = line.to_string();
                blank_range(&mut blanked, start, start + end + 2);
                if let Some(construct) = detect(&blank_strings(&blanked)) {
                    out.push(Violation {
                        construct,
                        line: line.to_string(),
                    });
                }
                continue;
            }
            in_block_comment = true;
            continue;
        }
        let scanned = blank_strings(line);
        if let Some(construct) = detect(&scanned) {
            out.push(Violation {
                construct,
                line: line.to_string(),
            });
        }
    }
    out
}

/// True if `line`, trimmed, is a `//` line comment — the same check `xtask`'s
/// own `ts_any` probe (`is_line_comment`, `xtask/src/greenfield_status.rs`)
/// already applies to the identical `any`/keyword patterns over Rust source;
/// this module's own doc claims parity with that probe's posture, but never
/// actually had this exclusion until the first real invocation over compiled
/// output (#1538) found it missing — a standalone comment line naming
/// `namespace` in prose (`// A minimal structural view of the Cloudflare
/// Durable Object namespace/stub`) is not a `namespace` declaration. Doesn't
/// attempt an inline `//` after real code on the same line — the same
/// simplicity `xtask`'s own version accepts, and no real emitted line does
/// either today.
fn is_line_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// Replace `text[start..end]` with spaces, byte-for-byte (so every later
/// column offset stays valid) — every replaced byte becomes a single-byte
/// ASCII space, so the result is valid UTF-8 regardless of what multi-byte
/// characters `text` held there.
fn blank_range(text: &mut String, start: usize, end: usize) {
    let mut bytes = std::mem::take(text).into_bytes();
    for b in &mut bytes[start..end] {
        *b = b' ';
    }
    *text = String::from_utf8(bytes).expect("blanking ASCII bytes keeps the string valid UTF-8");
}

/// Blank out the contents of every `"..."`/`'...'`/`` `...` `` string literal
/// on `line` (delimiters kept, so column positions and the surrounding
/// `detect` patterns' own delimiter-adjacent matches are unaffected) —
/// #1538's own real gap found wiring this lint over real compiled output:
/// `"bynk.cloudflare.Kv requires a KV namespace binding …"` is a message
/// string, not a `namespace` declaration, and nothing before this scan
/// distinguished the two. A single-pass state machine over one delimiter at
/// a time (not nested — TypeScript string literals can't nest unescaped),
/// tracking `\`-escapes so an escaped quote doesn't end the literal early.
/// Template-literal `${...}` interpolation is not specially handled (a
/// `namespace`/`any` keyword inside one would still be blanked as literal
/// text) — no real emitted `Verbatim`/`VerbatimExpr` content uses one today.
fn blank_strings(line: &str) -> String {
    let mut out: Vec<u8> = line.as_bytes().to_vec();
    let mut i = 0;
    while i < out.len() {
        let c = out[i];
        if c == b'"' || c == b'\'' || c == b'`' {
            let delim = c;
            let mut j = i + 1;
            while j < out.len() {
                if out[j] == b'\\' && j + 1 < out.len() {
                    j += 2;
                    continue;
                }
                if out[j] == delim {
                    break;
                }
                j += 1;
            }
            let end = j.min(out.len());
            for b in &mut out[i + 1..end] {
                *b = b' ';
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    // Safe: every byte written above is ASCII (a space), and the rest of
    // `out` is copied unchanged from `line`'s own valid UTF-8 bytes — a
    // multi-byte character's continuation bytes are never a `\`/quote/
    // backtick (all ASCII-range), so this never splits one.
    String::from_utf8(out).unwrap_or_else(|_| line.to_string())
}

fn detect(line: &str) -> Option<&'static str> {
    if is_any(line) {
        return Some("TsType::Any");
    }
    if contains_keyword(line, "enum") {
        return Some("enum");
    }
    if contains_keyword(line, "namespace") {
        return Some("namespace");
    }
    if is_decorator(line) {
        return Some("decorator");
    }
    if is_constructor_parameter_property(line) {
        return Some("constructor parameter property");
    }
    None
}

/// The same five shapes `xtask`'s own `ts_any` probe (`line_violates_ts_any`)
/// checks against Rust source — `as any`, bare `: any`, generic-position
/// `<any`/`any>`/`any[]` — but *not* that probe's own plain-substring
/// matching (review of #1308, finding 4): that probe scans Bynk's own Rust
/// source, a corpus the team can rename around a false positive; this scans
/// generated TypeScript carrying arbitrary Bynk-author identifiers nobody
/// here controls (`Company[]`, `Record<string, Company>`, `Map<anything,
/// …>` all contain one of the five substrings and none is `Any`), so a
/// false positive here is a user-facing build failure with no user-side
/// fix. `any` is matched as its own word first (reusing [`contains_keyword`]'s
/// boundary rule), then classified by what's immediately around it.
fn is_any(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut start = 0;
    while let Some(rel) = line[start..].find("any") {
        let i = start + rel;
        let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
        let after_ok = i + 3 >= bytes.len() || !is_ident_byte(bytes[i + 3]);
        if before_ok && after_ok && is_any_type_position(&line[..i], &line[i + 3..]) {
            return true;
        }
        start = i + 3;
    }
    false
}

/// Whether a word-bounded `any` sits in a type position: `as any`, `: any`
/// (with or without the space — `const x:any` is real, emitted-output-
/// unlikely but still a live pattern to catch), a generic open (`<any`), a
/// generic close (`any>`), or an array (`any[]`). `before`/`after` are the
/// line's text on each side of the matched word.
fn is_any_type_position(before: &str, after: &str) -> bool {
    let trimmed = before.trim_end();
    if trimmed.ends_with("as") {
        let as_start_ok =
            trimmed.len() == 2 || !is_ident_byte(trimmed.as_bytes()[trimmed.len() - 3]);
        if as_start_ok {
            return true;
        }
    }
    if trimmed.ends_with(':') || trimmed.ends_with('<') {
        return true;
    }
    after.starts_with('>') || after.starts_with("[]")
}

/// Whether `keyword` appears in `line` as a real word — not as a substring
/// of a longer identifier (`enum` inside `enumerate`, `namespace` inside
/// `MyNamespaceThing`).
fn contains_keyword(line: &str, keyword: &str) -> bool {
    let bytes = line.as_bytes();
    let klen = keyword.len();
    let mut start = 0;
    while let Some(rel) = line[start..].find(keyword) {
        let i = start + rel;
        let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
        let after_ok = i + klen >= bytes.len() || !is_ident_byte(bytes[i + klen]);
        if before_ok && after_ok {
            return true;
        }
        start = i + klen;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// A TypeScript decorator: `@Identifier` at the start of a (trimmed) line —
/// `@Injectable()`, `@Component({ ... })`. Emitted output never has a
/// legitimate `@` at line-start otherwise (no JSDoc `@param` lines survive
/// into `Verbatim` text; those live in comments this scan doesn't need to
/// special-case since a `@param` line's next character is a space, not an
/// identifier start).
fn is_decorator(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix('@')
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
}

/// A constructor parameter property: `private`/`public`/`protected`/
/// `readonly` inside a `constructor(...)`'s own parameter list — the one
/// type-directed construct pure strip-only stripping cannot erase (ADR
/// 0136's own strip-only rationale, already the reason `emitter/emit.rs`'s
/// own provider constructor de-sugars away from this shape by hand). Scoped
/// to *only* the text between `constructor(` and its matching close paren
/// (review of #1308, finding 5: the first version scanned to end of line,
/// so `constructor(deps: Deps) { this.mode = "readonly access"; }` — a
/// `readonly`-shaped *string literal* in the constructor's own body —
/// false-positived, contradicting this doc comment's own claim).
/// Paren-depth tracked, not `{}`-depth: a parameter's own object type
/// (`constructor(deps: { Log: unknown })`) carries braces the scan must
/// walk straight through, so only `(`/`)` count.
fn is_constructor_parameter_property(line: &str) -> bool {
    let Some(after) = line
        .find("constructor(")
        .map(|i| &line[i + "constructor(".len()..])
    else {
        return false;
    };
    let mut depth = 1i32;
    let mut end = after.len();
    for (idx, c) in after.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = idx;
                    break;
                }
            }
            _ => {}
        }
    }
    let params = &after[..end];
    ["private ", "public ", "protected ", "readonly "]
        .iter()
        .any(|kw| params.contains(kw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catches_as_any_and_bare_colon_any() {
        assert_eq!(
            verbatim_violations("const x = (value as any).field;"),
            vec![Violation {
                construct: "TsType::Any",
                line: "const x = (value as any).field;".to_string(),
            }]
        );
        assert!(verbatim_violations("const x: unknown = value;").is_empty());
    }

    #[test]
    fn catches_generic_position_any() {
        assert!(!verbatim_violations("const h: Record<string, any[]> = {};").is_empty());
        assert!(!verbatim_violations("type T = Array<any>;").is_empty());
    }

    #[test]
    fn catches_colon_any_with_no_space() {
        assert!(!verbatim_violations("function f(x:any) {}").is_empty());
    }

    /// Review of #1308, finding 4: `is_any`'s original plain-substring match
    /// flagged `any[]`/`any>`/`<any` wherever they appeared, including
    /// inside an unrelated identifier — a real hazard here specifically,
    /// since this scans generated TypeScript carrying Bynk-author schema
    /// names nobody on this team can rename to dodge a false positive.
    #[test]
    fn does_not_false_positive_on_any_as_a_substring_of_a_real_identifier() {
        assert!(verbatim_violations("type Fleet = Company[];").is_empty());
        assert!(verbatim_violations("const x: Record<string, Company> = {};").is_empty());
        assert!(verbatim_violations("type T = Map<anything, string>;").is_empty());
    }

    #[test]
    fn catches_enum_as_a_real_keyword_not_a_substring() {
        assert_eq!(
            verbatim_violations("enum Colour { Red, Green }")[0].construct,
            "enum"
        );
        assert!(verbatim_violations("function enumerate(x: string) {}").is_empty());
        assert!(verbatim_violations("const myEnum = 1;").is_empty());
    }

    #[test]
    fn catches_namespace_as_a_real_keyword_not_a_substring() {
        assert_eq!(
            verbatim_violations("namespace Foo { export const x = 1; }")[0].construct,
            "namespace"
        );
        assert!(verbatim_violations("const namespaced = true;").is_empty());
    }

    #[test]
    fn catches_a_leading_decorator() {
        assert_eq!(
            verbatim_violations("  @Injectable()")[0].construct,
            "decorator"
        );
        assert_eq!(
            verbatim_violations("@Component({ selector: \"x\" })")[0].construct,
            "decorator"
        );
        // A bare `@` with no following identifier (an email-shaped string
        // literal fragment, say) isn't a decorator.
        assert!(verbatim_violations("const s = \"a@b.com\";").is_empty());
    }

    #[test]
    fn catches_constructor_parameter_properties() {
        assert_eq!(
            verbatim_violations("constructor(private deps: Deps) {}")[0].construct,
            "constructor parameter property"
        );
        assert_eq!(
            verbatim_violations("constructor(a: A, readonly b: B) {}")[0].construct,
            "constructor parameter property"
        );
        // The de-sugared shape `bynk-emit` actually emits — a plain param,
        // assigned in the body — is not a parameter property.
        assert!(
            verbatim_violations("constructor(deps: { Log: unknown }) { this.deps = deps; }")
                .is_empty()
        );
    }

    /// Review of #1308, finding 5: the original scan ran to end of line, so
    /// a `readonly`-shaped string *inside the constructor's own body* (not
    /// its parameter list) false-positived — contradicting the function's
    /// own doc comment, which already claimed the scan was parameter-list-
    /// scoped.
    #[test]
    fn does_not_false_positive_on_the_constructor_body() {
        assert!(
            verbatim_violations("constructor(deps: Deps) { this.mode = \"readonly access\"; }")
                .is_empty()
        );
    }

    #[test]
    fn clean_typescript_produces_no_violations() {
        let text = "export function add(a: number, b: number): number {\n  return a + b;\n}\n";
        assert!(verbatim_violations(text).is_empty());
    }

    #[test]
    fn scans_every_offending_line_not_just_the_first() {
        let text = "enum A { X }\nconst y: any = 1;\n";
        let violations = verbatim_violations(text);
        assert_eq!(violations.len(), 2);
        assert_eq!(violations[0].construct, "enum");
        assert_eq!(violations[1].construct, "TsType::Any");
    }

    /// #1538's own real gap: a message string naming a banned construct in
    /// prose is not the construct itself.
    #[test]
    fn does_not_false_positive_on_a_string_literal_naming_a_construct() {
        assert!(
            verbatim_violations(
                "throw new Error(\"bynk.cloudflare.Kv requires a KV namespace binding\");"
            )
            .is_empty()
        );
        assert!(verbatim_violations("const msg = 'cast as any if unsure';").is_empty());
    }

    /// A string literal's own delimiters are blanked-around, not removed —
    /// a real violation immediately after a string on the same line must
    /// still be caught.
    #[test]
    fn still_catches_a_real_violation_after_a_string_literal_on_the_same_line() {
        assert_eq!(
            verbatim_violations("const s: any = \"a namespace-like string\";")[0].construct,
            "TsType::Any"
        );
    }

    /// #1538's own real gap: a `/** ... */` JSDoc block naming a construct
    /// in prose is not the construct itself, whether the block is single- or
    /// multi-line.
    #[test]
    fn does_not_false_positive_on_a_block_comment() {
        assert!(verbatim_violations("/** A Durable Object namespace stub. */").is_empty());
        let multiline =
            "/**\n * Cast through `any` when the shape is unknown.\n */\nconst x = 1;\n";
        assert!(verbatim_violations(multiline).is_empty());
    }

    /// A same-line block comment doesn't swallow a real violation that
    /// follows it on the same line.
    #[test]
    fn still_catches_a_real_violation_after_a_same_line_block_comment() {
        assert_eq!(
            verbatim_violations("/* namespace-like prose */ const x: any = 1;")[0].construct,
            "TsType::Any"
        );
    }
}
