//! Completion for the cursor, keyed off the line up to it.
//!
//! The surface is the canonical *cursor context × candidate-kind* matrix fixed
//! by ADR 0093 (`design/decisions/0093-completion-surface-contract.md`), spec'd
//! at `design/bynk-lsp-spec.md` §3.15. [`complete`] dispatches the six contexts
//! it can serve purely (no analysis cache):
//!
//! - `consumes <prefix>` / `consumes U { … }` / `given …` — consumable units and
//!   in-scope capabilities (v0.17);
//! - **type position** (`: T`, `-> T`, inside `[ … ]` type args) — built-in
//!   types, the `bynk`-surface transparent types, and project `type` decls;
//! - **keyword position** (a bare word at a declaration/statement start) — the
//!   reserved keywords (with registry docs) and declaration snippets;
//! - **name-receiver `UpperIdent.`** — sum variants (project + built-in
//!   `HttpResult`/`QueueResult`), refined/opaque `of`/`unsafe`, capability ops,
//!   and built-in type statics (`Int.parse`/`List.empty`/`Effect.pure`/…);
//! - **expression position** (after `=`/`(`/`,`/`=>`/an operator) — the value
//!   constructors (`Ok`/`Some`/`true`/…), in-scope type names, and in-scope free
//!   functions (the current unit's own `fn`s + `uses`-imported stdlib/project
//!   combinators, gated on the `uses` set) (ADR 0093 D3).
//!
//! Two further contexts need the analysis overlay and so live handler-side
//! (`main.rs`): **value-receiver `lower.`** members (kernel methods + record
//! fields) and **in-scope locals/params**. They depend on the analysis overlay
//! (the boundary is ADR 0093 D4), but since slice 4 (ADR 0094) it is
//! error-tolerant: best-effort partial types are recorded even on a broken
//! buffer, so they no longer go silent on an unrelated error. Items also carry a
//! one-line `detail` eagerly; the richer `documentation` is filled in lazily by
//! `completionItem/resolve`, handler-side (slice 5).
//!
//! Context detection is lexical (it must work mid-edit, when the buffer rarely
//! parses); candidates are semantic. Unit/type/capability/member enumeration
//! parses the project's `.bynk` files (and the embedded `bynk` surface) with
//! recovery, so it works even while the file the cursor sits in is mid-edit.
//! Built-ins, keywords, and constructors come from the static `bynkc` registries
//! (`keywords`/`builtin_names`/`firstparty`/`ast`), never the index — first-party
//! symbols aren't indexed (the v0.28 finding); the project parse supplies only
//! *project* symbols.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use bynk_check::checker::{NamedKind, Ty, TyId, Types};
use bynk_check::kernel_methods;
use bynk_check::locals::LocalBinding;
use bynk_check::store_ops;
use bynk_syntax::ast::{CommonsItem, ExportKind, FnName, SourceUnit, TypeBody, TypeRef, UsesDecl};
use bynk_syntax::{keywords, lexer, parser};

use crate::symbols::type_ref_str;

/// What a candidate refers to — maps to an LSP `CompletionItemKind`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Unit,
    Capability,
    Type,
    Keyword,
    Snippet,
    /// A sum-type variant (`Color.Red`).
    Variant,
    /// A name-receiver member: a refined/opaque `of`/`unsafe` constructor, a
    /// capability operation, or a built-in type static (`Int.parse`).
    Member,
    /// A record field on a value receiver (`order.total`).
    Field,
    /// A value constructor at expression position (`Ok`/`Some`/`true`).
    Constructor,
    /// A free function in scope at expression position — the current unit's own
    /// top-level `fn`s and the `uses`-imported stdlib/project combinators.
    Function,
    /// A named constant value at expression position — an `asset` (#1883),
    /// in scope by the same rule as a free function.
    Constant,
    /// A constraint name after `T:` or `where A:` (constrained generics
    /// slice 0).
    Constraint,
}

pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    /// LSP snippet text (with `${n:…}`/`$0` tab stops) for `Snippet` items;
    /// `None` means insert the label verbatim.
    pub insert_text: Option<String>,
    /// #1888: the candidate's doc, as Markdown: a record field's or a
    /// variant's own doc, in either form.
    pub documentation: Option<String>,
}

impl Completion {
    pub fn item(label: impl Into<String>, kind: CompletionKind, detail: Option<String>) -> Self {
        Completion {
            label: label.into(),
            kind,
            detail,
            insert_text: None,
            documentation: None,
        }
    }

    /// #1888: this candidate with `doc` as its documentation.
    fn with_doc(mut self, doc: Option<&str>) -> Self {
        self.documentation = doc.map(str::to_owned);
        self
    }

    fn snippet(label: &str, body: &str) -> Self {
        Completion {
            label: label.to_string(),
            kind: CompletionKind::Snippet,
            detail: Some(format!("{label} scaffold")),
            insert_text: Some(body.to_string()),
            documentation: None,
        }
    }
}

/// Produce completions for the cursor, given the text of the line up to the
/// cursor, the current document text, and the project source root (if any).
/// Without a project root, a project's dependencies are found from the
/// manifest in `files` ([`crate::packages::DependencyView::from_files`]);
/// the language server calls [`complete_in`] with the root it knows.
pub fn complete(
    line_prefix: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    complete_in(None, line_prefix, doc_text, files)
}

/// [`complete`] for the project rooted at `project_root`, whose path
/// dependencies decide which unit names are offered (#2223).
pub fn complete_in(
    project_root: Option<&Path>,
    line_prefix: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    // #2223: the project's own files and its dependencies' reachable units,
    // resolved once per request and only by the branches that name units.
    let split = || split_dependency_files(files, project_root);
    // 1. Inside `consumes U { … <cursor>` — the capabilities U exports.
    if let Some(unit) = consumes_brace_unit(line_prefix) {
        return capabilities_of_unit(&unit, doc_text, &split())
            .into_iter()
            .map(|c| {
                let detail = format!(
                    "capability exported by `{unit}`{}",
                    stability_suffix(experimental(&format!("{unit}.{c}")))
                );
                Completion::item(c, CompletionKind::Capability, Some(detail))
            })
            .collect();
    }
    // 2. After `consumes <prefix>` — consumable unit names.
    if is_consumes_target(line_prefix) {
        return consumable_units(doc_text, &split());
    }
    // 2a. #2223: after `uses <prefix>` — the commons a `uses` can name: the
    //     project's own, and each dependency's exported ones as
    //     `<alias>.<unit>`.
    if is_uses_target(line_prefix) {
        return usable_commons(doc_text, &split());
    }
    // 3. After `given …` — in-scope capabilities.
    if is_given_position(line_prefix) {
        return in_scope_capabilities(doc_text, &split());
    }
    // 4. `UpperIdent.<cursor>` — name-receiver members: sum variants, refined/
    //    opaque `of`/`unsafe`, capability ops, or built-in type statics.
    if let Some(receiver) = member_receiver(line_prefix) {
        return member_candidates(&receiver, doc_text, files);
    }
    // v0.124 (slice 3): the non-keyword clause/construction contexts, before the
    // generic type/keyword/expression cells they would otherwise fall into.
    // 4a-pre. #2641: `Type { <cursor>` in a *pattern* — the fields not yet
    //     listed, then `..` (a record pattern always ends with it).
    if let Some((recv, listed)) = record_pattern_receiver(line_prefix) {
        let mut fields = record_field_names(&recv, doc_text, files);
        if !fields.is_empty() {
            fields.retain(|c| !listed.contains(&c.label));
            fields.push(Completion::item(
                "..",
                CompletionKind::Keyword,
                Some("the rest of the fields — a record pattern is always open".to_string()),
            ));
            return fields;
        }
    }
    // 4a. `Type { <cursor>` — record field names on construction.
    if let Some(recv) = record_construction_receiver(line_prefix) {
        let fields = record_field_names(&recv, doc_text, files);
        if !fields.is_empty() {
            return fields;
        }
    }
    // 4b. `from <cursor>` — the service protocols.
    if after_clause_keyword(line_prefix, "from") {
        return protocol_candidates();
    }
    // 4c. `on <cursor>` — the handler kinds.
    if after_clause_keyword(line_prefix, "on") {
        return handler_kind_candidates();
    }
    // 4d. `by <cursor>` — the project's actor names.
    if after_clause_keyword(line_prefix, "by") {
        return actor_candidates(doc_text, files);
    }
    // 4e. `exports <cursor>` — the export kinds (adapter).
    if after_clause_keyword(line_prefix, "exports") {
        return export_kind_candidates();
    }
    // 4f. `provides <cursor>` — the in-scope capabilities to implement.
    if after_clause_keyword(line_prefix, "provides") {
        return in_scope_capabilities(doc_text, &split());
    }
    // 4f'. Constrained generics slice 0: `fn f[T: <cursor>` / `where A: <cursor>`
    //      — the constraint names.
    if is_constraint_position(line_prefix) {
        return constraint_candidates();
    }
    // 4g. `where <cursor>` — the closed refinement-predicate vocabulary.
    // Shared by a type declaration's `type X = Base where <cursor>` and
    // (#472) a match arm's `_ where <cursor>`; both reuse the same closed
    // predicate catalogue, so one branch serves both surfaces. Excludes a
    // `for all x: T, … where <cursor>` binder's clause, which takes an
    // arbitrary `Bool` expression, not the predicate catalogue — that case
    // falls through to expression-position candidates below.
    if after_clause_keyword(line_prefix, "where") && !is_for_all_where(line_prefix) {
        return predicate_name_candidates();
    }
    // 5. Type position (`: T`, `-> T`, `[ … ]` type args) — built-ins, the
    //    `bynk`-surface transparent types, and project type declarations.
    if is_type_position(line_prefix) {
        return type_candidates(doc_text, files);
    }
    // 6. Keyword position (a bare word at a declaration/statement start) — the
    //    reserved keywords plus declaration snippets.
    if is_keyword_position(line_prefix) {
        return keyword_and_snippet_candidates();
    }
    // 7. Expression position (after `=`/`(`/`,`/`=>`/a binary operator) — a value
    //    starts here: the constructor keywords + in-scope type names. In-scope
    //    locals/params (and, from slice 3, free functions) are appended
    //    handler-side, where the analysis cache lives (ADR 0093 D3).
    if is_expression_position(line_prefix) {
        return expression_candidates(doc_text, files);
    }
    Vec::new()
}

// -- Cursor-context detection (line-prefix scanning) --

/// `consumes U { … ` with the brace still open at the cursor → `Some(U)`.
fn consumes_brace_unit(line: &str) -> Option<String> {
    let idx = line.rfind("consumes")?;
    let after = &line[idx + "consumes".len()..];
    let open = after.find('{')?;
    // The brace must still be open up to the cursor (no closing brace after it).
    if after[open + 1..].contains('}') {
        return None;
    }
    let unit = after[..open].trim();
    if unit.is_empty() || !is_qualified_name(unit) {
        return None;
    }
    Some(unit.to_string())
}

/// `consumes <partial>` with no brace or `as` yet → completing the target name.
fn is_consumes_target(line: &str) -> bool {
    let Some(idx) = line.rfind("consumes") else {
        return false;
    };
    // `consumes` must be a standalone keyword (preceded by start/whitespace).
    if !line[..idx]
        .chars()
        .last()
        .map(|c| c.is_whitespace())
        .unwrap_or(true)
    {
        return false;
    }
    let after = &line[idx + "consumes".len()..];
    // Need at least one separating space, and no `{`, `}`, or `as` yet.
    after.starts_with(char::is_whitespace)
        && !after.contains('{')
        && !after.contains('}')
        && !after.split_whitespace().any(|w| w == "as")
}

/// #2223: `uses <partial>` as a unit header clause → completing the target.
fn is_uses_target(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(after) = trimmed.strip_prefix("uses") else {
        return false;
    };
    after.starts_with(char::is_whitespace) && after.split_whitespace().count() <= 1
}

/// Constrained generics slice 0: the cursor follows the `:` (or a `+`) of a
/// bound — inside a `fn`/`type` type-parameter list (`fn f[T: <cursor>`), or in
/// a method's `where` clause (`-> Bool where A: <cursor>`). The partial
/// constraint name being typed is stripped first.
fn is_constraint_position(line: &str) -> bool {
    let head = line
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .trim_end();
    if !(head.ends_with(':') && !head.ends_with("::") || head.ends_with('+')) {
        return false;
    }
    // Inside a still-open `[ … ]` that a `fn name` / `type Name` opened.
    if let Some(open) = head.rfind('[')
        && !head[open..].contains(']')
    {
        let before = head[..open].trim_end();
        let name_start = before
            .rfind(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.')))
            .map_or(0, |i| i + 1);
        let decl = before[..name_start].trim_end();
        return decl.ends_with("fn") || decl.ends_with("type");
    }
    // A `where` clause after a return type: only names, `:`, `,`, `+` since it.
    if let Some(w) = head.rfind("where")
        && line[..w].contains("->")
        && head[..w].chars().last().is_none_or(char::is_whitespace)
    {
        return head[w + "where".len()..]
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | ',' | '+' | ' ' | '\t'));
    }
    false
}

/// The constraint names (slice 0: `Eq` only).
fn constraint_candidates() -> Vec<Completion> {
    vec![Completion::item(
        "Eq",
        CompletionKind::Constraint,
        Some("constraint: values of the type compare with `==`".to_string()),
    )]
}

/// The cursor is inside a `given` list (after `given`, before the `{` body).
fn is_given_position(line: &str) -> bool {
    let Some(idx) = line.rfind("given") else {
        return false;
    };
    if !line[..idx]
        .chars()
        .last()
        .map(|c| c.is_whitespace())
        .unwrap_or(true)
    {
        return false;
    }
    let after = &line[idx + "given".len()..];
    if !after.starts_with(char::is_whitespace) {
        return false;
    }
    // Still in the given list while only capability names, dots, commas and
    // whitespace follow — a `{` opens the handler body.
    after
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | ',' | ' ' | '\t'))
}

fn is_qualified_name(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|seg| {
            !seg.is_empty()
                && seg.chars().all(|c| c.is_alphanumeric() || c == '_')
                && !seg.chars().next().unwrap().is_ascii_digit()
        })
}

/// The cursor sits in a type position: a return type (`-> T`), a type
/// annotation/field type (`: T`), or inside a `[ … ]` type-argument list. The
/// partial type name being typed is stripped before inspecting the preceding
/// token, so `: Optio` and `-> Eff` both qualify.
///
/// Conservative by construction: a list literal `[1, 2` is excluded (its `[` is
/// not preceded by a type constructor). The one accepted false positive is a
/// record *construction* value (`Order { id: <cursor>`), lexically identical to
/// a record field-type declaration — offering type names there is mild noise.
fn is_type_position(line: &str) -> bool {
    let head = line
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .trim_end();
    head.ends_with("->") || (head.ends_with(':') && !head.ends_with("::")) || in_type_arg_list(head)
}

/// `head` ends inside an unclosed `[ … ` whose opening bracket immediately
/// follows an identifier (a type constructor, e.g. `Option[`, `Result[Int, `) —
/// as opposed to a bare list-literal `[`.
fn in_type_arg_list(head: &str) -> bool {
    let chars: Vec<char> = head.chars().collect();
    let mut depth = 0i32;
    let mut opener_after_ident = false;
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '[' => {
                depth += 1;
                if depth == 1 {
                    opener_after_ident =
                        i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
                }
            }
            ']' => depth -= 1,
            _ => {}
        }
    }
    depth > 0 && opener_after_ident
}

/// A bare word at a declaration/statement start: the line up to the cursor is
/// only leading whitespace plus an optional partial identifier (no operators,
/// colons, or brackets). Fires on an empty line too. Disjoint from
/// `is_type_position` (an internal helper), whose triggers (`:`/`->`/`[`) make
/// this false.
pub fn is_keyword_position(line: &str) -> bool {
    line.trim().chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// The cursor sits where a **value** expression is expected — after `=`/`(`/`,`,
/// a `=>` lambda arrow, or a binary operator — so in-scope locals are offered
/// (v0.31, ADR 0064). Conservative: covers the common positions, excludes the
/// type arrow `->`. (The handler also offers locals at keyword position.)
pub fn is_expression_position(line: &str) -> bool {
    let head = line
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .trim_end();
    if head.ends_with("->") {
        return false; // a return/param type, not a value
    }
    if head.ends_with("=>") {
        return true; // a lambda body
    }
    matches!(
        head.chars().last(),
        Some('=' | '(' | ',' | '[' | '+' | '-' | '*' | '/' | '<' | '>' | '&' | '|')
    )
}

/// `UpperIdent.<partial>` at the cursor → `Some("UpperIdent")` — a name
/// receiver whose members are statically enumerable (a sum/refined/opaque
/// type or a capability). Conservative: the receiver is a **single**
/// uppercase-initial identifier, not itself a `.`-qualified segment (so
/// `bynk.cloudflare.` and `a.B.` are excluded) and not a number (so the
/// decimal `1.` is excluded). A lowercase `x.` is a *value* receiver — deferred
/// to slice 3 — and yields `None`.
fn member_receiver(line: &str) -> Option<String> {
    // Drop the partial member name being typed, then require a trailing dot.
    let head = line
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .strip_suffix('.')?;
    // The receiver is the identifier immediately before that dot. Advance past
    // the matched char by its UTF-8 length: a multi-byte non-identifier char
    // (`"`, `€`, `—`, …) would make `i + 1` land mid-codepoint and panic.
    let start = head
        .char_indices()
        .rfind(|&(_, c)| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let recv = &head[start..];
    let first = recv.chars().next()?;
    if !first.is_ascii_uppercase() {
        return None;
    }
    // Reject a `.`-qualified receiver (`a.B.`): the char before it is a dot.
    if head[..start].ends_with('.') {
        return None;
    }
    Some(recv.to_string())
}

/// v0.124 (slice 3): the cursor is at a field-*name* position of a record
/// construction — inside an unclosed `{` opened immediately after an
/// uppercase-initial type name (`Order { <cursor>` / `Order { id: 1, <cursor>`),
/// with the current field segment not yet past its `:` (a field *type*
/// position, left to [`is_type_position`]). Returns the record type name.
fn record_construction_receiver(line: &str) -> Option<String> {
    // The innermost `{` still open at the cursor.
    let bytes = line.as_bytes();
    let mut depth = 0i32;
    let mut open = None;
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b'}' => depth += 1,
            b'{' => {
                if depth == 0 {
                    open = Some(i);
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    let open = open?;
    // Only at a name position: the current field (since the last comma) has no
    // `:` yet — else the cursor is in that field's type.
    let current = line[open + 1..].rsplit(',').next().unwrap_or("");
    if current.contains(':') {
        return None;
    }
    // The receiver is the uppercase-initial identifier immediately before `{`.
    let head = line[..open].trim_end();
    let start = head
        .char_indices()
        .rfind(|&(_, c)| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let recv = &head[start..];
    if recv.chars().next()?.is_ascii_uppercase() {
        Some(recv.to_string())
    } else {
        None
    }
}

/// #2641: the cursor is at a field-name position of a record *pattern* —
/// `Type { <cursor>` where the text before `Type` is only pattern syntax (a
/// match arm's start, `Some(`, `is (`, `field: `, `|`), never an expression
/// (`let o = Type {`). Returns the type name and the fields already listed.
fn record_pattern_receiver(line: &str) -> Option<(String, Vec<String>)> {
    let recv = record_construction_receiver(line)?;
    let open = line.rfind('{')?;
    // Only the segment after `is` (the pattern) or the whole line (a match arm).
    let head = line[..open].trim_end();
    let head = head.strip_suffix(recv.as_str())?;
    let after_is = head
        .rsplit_once(" is ")
        .map_or(head, |(_, pattern)| pattern);
    // What precedes the type name must be pattern syntax only: separators, an
    // uppercase constructor before `(`, or a field name before `:`.
    let mut rest = after_is.trim_start();
    loop {
        let before = rest;
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '(' || c == '|');
        let ident_end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let (ident, tail) = rest.split_at(ident_end);
        let tail_trim = tail.trim_start();
        if !ident.is_empty()
            && ((tail_trim.starts_with('(') && ident.starts_with(|c: char| c.is_ascii_uppercase()))
                || (tail_trim.starts_with(':') && !tail_trim.starts_with("::")))
        {
            rest = &tail_trim[1..];
        }
        if rest == before {
            break;
        }
    }
    if !rest.trim().is_empty() {
        return None;
    }
    // The fields already listed at this brace's top level.
    let mut listed = Vec::new();
    let mut depth = 0i32;
    let mut segment = String::new();
    for c in line[open + 1..].chars() {
        match c {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            ',' if depth == 0 => {
                let name = segment.split(':').next().unwrap_or("").trim();
                if !name.is_empty() {
                    listed.push(name.to_string());
                }
                segment.clear();
                continue;
            }
            _ => {}
        }
        segment.push(c);
    }
    Some((recv, listed))
}

/// v0.131: the CORS policy fields, offered at a field-name position inside a
/// `cors { }` block. `Allow-Methods` is deliberately absent (derived from the
/// routes), so the closed set is these four.
pub const CORS_FIELDS: &[(&str, &str)] = &[
    (
        "origins",
        "the allowed origins — an exact allowlist, or `[\"*\"]`",
    ),
    (
        "headers",
        "the `Access-Control-Allow-Headers` a preflight advertises",
    ),
    (
        "credentials",
        "whether credentialed requests are allowed (`true`/`false`)",
    ),
    (
        "maxAge",
        "how long a browser may cache the preflight (a `Duration`)",
    ),
];

/// v0.141 (ADR 0164): the security-headers policy fields, offered at a field-name
/// position inside a `security { }` block. The closed set is these two.
pub const SECURITY_FIELDS: &[(&str, &str)] = &[
    (
        "nosniff",
        "stamp `X-Content-Type-Options: nosniff` (`true`/`false`, default `true`)",
    ),
    (
        "hsts",
        "opt in to `Strict-Transport-Security` — the `max-age` as a `Duration`",
    ),
];

/// v0.142 (ADR 0165): the request-limits policy fields, offered at a field-name
/// position inside a `limits { }` block. The closed set is this one.
pub const LIMITS_FIELDS: &[(&str, &str)] = &[(
    "maxBody",
    "the maximum request body size in bytes (a positive `Int`)",
)];

/// v0.140 (ADR 0163): the `@cache` handler-annotation arguments, offered at an
/// argument-name position inside `@cache( … )`. `maxAge` is required (the freshness
/// window); `scope` is optional (`public`/`private`, default `private`), as is
/// `immutable` (caching raw responses; default `false`). The conditional
/// `ETag`/`304` half is automatic and has no surface.
pub const CACHE_ARGS: &[(&str, &str)] = &[
    (
        "maxAge",
        "the freshness window — a whole-second `Duration` (e.g. `5.minutes`) lowered to `Cache-Control: max-age`",
    ),
    (
        "scope",
        "`public` or `private` (default `private` — a shared cache stores only on `public`)",
    ),
    (
        "immutable",
        "`true` promises the response never changes (a content-addressed path) — appends `immutable` to `Cache-Control`; purge it through `EdgeCache` if it can change; default `false`",
    ),
];

/// v0.142 (ADR 0165): the `@limit` handler-annotation arguments, offered at an
/// argument-name position inside `@limit( … )`. Its one arg is `maxBody`, the
/// request-body byte ceiling above which a `413` is synthesised before the body
/// is read.
pub const LIMIT_ARGS: &[(&str, &str)] = &[(
    "maxBody",
    "the maximum request body size in bytes (a positive `Int`) — a `413` is synthesised past it",
)];

/// The `@raw` handler-annotation arguments, offered at an argument-name position
/// inside `@raw( … )`. Its one arg is `contentType`, the media type the route accepts
/// (`"*/*"` for any).
pub const RAW_ARGS: &[(&str, &str)] = &[(
    "contentType",
    "the media type the route accepts, a string literal (`\"*/*\"` for any) — another `Content-Type` is a `415`",
)];

/// The byte index of the innermost `{` still open at `offset` (a naive scan that,
/// like [`record_construction_receiver`], does not skip braces inside strings or
/// comments — acceptable for a completion heuristic).
fn innermost_open_brace(text: &str, offset: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let end = offset.min(bytes.len());
    let mut depth = 0i32;
    for i in (0..end).rev() {
        match bytes[i] {
            b'}' => depth += 1,
            b'{' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// The bare word immediately before byte index `open` (skipping trailing
/// whitespace), e.g. the `cors` before a `cors {`.
fn word_before_brace(text: &str, open: usize) -> &str {
    let head = text[..open].trim_end();
    let start = head
        .char_indices()
        .rfind(|&(_, c)| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    &head[start..]
}

/// #2640: the cursor is directly inside a `par { … }` block — the innermost open
/// brace is opened by the word `par`.
pub fn in_par_block_position(text: &str, offset: usize) -> bool {
    innermost_open_brace(text, offset).is_some_and(|open| word_before_brace(text, open) == "par")
}

/// #2640: what a `par { … }` block can hold, so nothing else is offered there:
/// the effectful binding and the unit effect (DECISION B).
pub fn par_block_candidates() -> Vec<Completion> {
    vec![
        Completion::snippet("let x <-", "let ${1:name} <- ${2:effect}"),
        Completion::item(
            "do",
            CompletionKind::Keyword,
            Some("run a unit effect alongside the others in this `par` block".to_string()),
        ),
    ]
}

/// #2640: whether `offset` sits in an effectful body, the only place a `par`
/// block is legal. Lexical, like the other position cells: walk the enclosing
/// braces outwards; the first one whose header declares a return type decides —
/// effectful when that type is `Effect[…]`. A brace with no `->` (an `if`, a
/// `match` arm, a bare block) defers to the one around it.
pub fn in_effectful_body_position(text: &str, offset: usize) -> bool {
    let mut at = offset.min(text.len());
    while let Some(open) = innermost_open_brace(text, at) {
        let before = &text[..open];
        let header_start = before.rfind(['{', '}']).map_or(0, |i| i + 1);
        let header = &before[header_start..];
        if let Some(arrow) = header.rfind("->") {
            return header[arrow + 2..].trim_start().starts_with("Effect[");
        }
        at = open;
    }
    false
}

/// #2640: the `par` block snippet, offered at statement position in an
/// effectful body.
pub fn par_snippet() -> Completion {
    Completion::snippet(
        "par",
        "par {\n\tlet ${1:first} <- ${2:effect}\n\tlet ${3:second} <- ${4:effect}\n}",
    )
}

/// v0.131: the cursor is at a field-*name* position inside a `cors { … }` block —
/// the innermost open brace is opened by `cors`, and the current field segment
/// (since the last `,` or newline) has no `:` yet (a value position).
pub fn in_cors_field_position(text: &str, offset: usize) -> bool {
    let Some(open) = innermost_open_brace(text, offset) else {
        return false;
    };
    if word_before_brace(text, open) != "cors" {
        return false;
    }
    // `offset` arrives on a char boundary from the position converter, but
    // slice defensively — a mid-codepoint offset must degrade, not panic.
    let Some(seg) = text.get(open + 1..offset.min(text.len())) else {
        return false;
    };
    let current = seg.rsplit(['\n', ',']).next().unwrap_or("");
    !current.contains(':')
}

/// v0.141: the cursor is at a field-*name* position inside a `security { … }`
/// block — the innermost open brace is opened by `security`, and the current
/// field segment has no `:` yet. Mirrors [`in_cors_field_position`].
pub fn in_security_field_position(text: &str, offset: usize) -> bool {
    let Some(open) = innermost_open_brace(text, offset) else {
        return false;
    };
    if word_before_brace(text, open) != "security" {
        return false;
    }
    // `offset` arrives on a char boundary from the position converter, but
    // slice defensively — a mid-codepoint offset must degrade, not panic.
    let Some(seg) = text.get(open + 1..offset.min(text.len())) else {
        return false;
    };
    let current = seg.rsplit(['\n', ',']).next().unwrap_or("");
    !current.contains(':')
}

/// v0.142 (ADR 0165): the cursor is at a field-*name* position inside a
/// `limits { … }` block — the innermost open brace is opened by `limits`, and the
/// current field segment has no `:` yet. Mirrors [`in_security_field_position`].
pub fn in_limits_field_position(text: &str, offset: usize) -> bool {
    let Some(open) = innermost_open_brace(text, offset) else {
        return false;
    };
    if word_before_brace(text, open) != "limits" {
        return false;
    }
    // `offset` arrives on a char boundary from the position converter, but
    // slice defensively — a mid-codepoint offset must degrade, not panic.
    let Some(seg) = text.get(open + 1..offset.min(text.len())) else {
        return false;
    };
    let current = seg.rsplit(['\n', ',']).next().unwrap_or("");
    !current.contains(':')
}

/// #2134: the cursor is at an entry-*name* position inside a suite's or case's
/// `env { … }` block — the innermost open brace is opened by `env`, and the
/// current entry (since the last newline or `,`) has no `=` yet.
pub fn in_env_name_position(text: &str, offset: usize) -> bool {
    let Some(open) = innermost_open_brace(text, offset) else {
        return false;
    };
    if word_before_brace(text, open) != "env" {
        return false;
    }
    let Some(seg) = text.get(open + 1..offset.min(text.len())) else {
        return false;
    };
    let current = seg.rsplit(['\n', ',']).next().unwrap_or("");
    !current.contains('=')
}

/// #2134: the names an `env` block can usefully give — each secret an actor
/// declares (`secret = "…"`, a Bearer, Signature or Foreign scheme) and each
/// literal name a `Secrets.get("…")` reads, across the buffer and the project's
/// files. A lexical scan, so it works mid-edit; sorted and deduplicated.
pub fn env_name_candidates(
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    let mut names: BTreeMap<String, &'static str> = BTreeMap::new();
    let mut scan = |src: &str| {
        for (marker, detail) in [
            ("secret = \"", "a secret an actor declares"),
            ("Secrets.get(\"", "a name `Secrets.get` reads"),
        ] {
            let mut rest = src;
            while let Some(i) = rest.find(marker) {
                rest = &rest[i + marker.len()..];
                if let Some(end) = rest.find('"') {
                    let name = &rest[..end];
                    if !name.is_empty() && !name.contains(char::is_whitespace) {
                        names.entry(name.to_string()).or_insert(detail);
                    }
                }
            }
        }
    };
    scan(doc_text);
    if let Some(files) = files {
        for src in files.values() {
            scan(src);
        }
    }
    names
        .into_iter()
        .map(|(name, detail)| {
            let mut c = Completion::item(name.clone(), CompletionKind::Field, Some(detail.into()));
            c.insert_text = Some(format!("{name} = \"\""));
            c
        })
        .collect()
}

/// v0.131: the cursor is at a service-body item start — a bare word inside a
/// `service … {` block (not a nested block) — where `cors` may begin, alongside
/// the `on` handler kinds. Gated on `is_keyword_position` so it only fires at a
/// fresh item start, and on the enclosing brace's header line naming `service`.
pub fn in_service_body_item_position(text: &str, offset: usize, line_prefix: &str) -> bool {
    if !is_keyword_position(line_prefix) {
        return false;
    }
    let Some(open) = innermost_open_brace(text, offset) else {
        return false;
    };
    let header_start = text[..open].rfind('\n').map_or(0, |i| i + 1);
    text[header_start..open].contains("service ")
}

/// The byte index of the innermost `(` still open at `offset` — the paren analogue
/// of [`innermost_open_brace`], for `@cache( … )` argument-position detection.
fn innermost_open_paren(text: &str, offset: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let end = offset.min(bytes.len());
    let mut depth = 0i32;
    for i in (0..end).rev() {
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// v0.140 (ADR 0163): the cursor is at an argument-*name* position inside a
/// `@cache( … )` — the innermost open paren is opened by the `cache` annotation
/// (`@cache`, not a bare `cache(` call), and the current argument segment (since
/// the last `,`) has no `:` yet (a name position, not a value one).
pub fn in_cache_arg_position(text: &str, offset: usize) -> bool {
    let Some(open) = innermost_open_paren(text, offset) else {
        return false;
    };
    if word_before_brace(text, open) != "cache" {
        return false;
    }
    // Distinguish the `@cache` annotation from any ordinary `cache(...)` call: the
    // word must be immediately preceded by `@`.
    let head = text[..open].trim_end();
    let before_cache = head[..head.len() - "cache".len()].trim_end();
    if !before_cache.ends_with('@') {
        return false;
    }
    // `offset` arrives on a char boundary from the position converter, but
    // slice defensively — a mid-codepoint offset must degrade, not panic.
    let Some(seg) = text.get(open + 1..offset.min(text.len())) else {
        return false;
    };
    let current = seg.rsplit(['\n', ',']).next().unwrap_or("");
    !current.contains(':')
}

/// v0.142 (ADR 0165): the cursor is at an argument-*name* position inside a
/// `@limit( … )` — the innermost open paren is opened by the `limit` annotation
/// (`@limit`, not a bare `limit(` call), and the current argument segment has no
/// `:` yet. Mirrors [`in_cache_arg_position`].
pub fn in_limit_arg_position(text: &str, offset: usize) -> bool {
    let Some(open) = innermost_open_paren(text, offset) else {
        return false;
    };
    if word_before_brace(text, open) != "limit" {
        return false;
    }
    // Distinguish the `@limit` annotation from any ordinary `limit(...)` call: the
    // word must be immediately preceded by `@`.
    let head = text[..open].trim_end();
    let before_limit = head[..head.len() - "limit".len()].trim_end();
    if !before_limit.ends_with('@') {
        return false;
    }
    // P3 (#2342): `capability X = RateLimit @limit(…)` is an instance option,
    // not the handler annotation, so `maxBody` doesn't belong there.
    if on_capability_declaration(text, open) {
        return false;
    }
    // `offset` arrives on a char boundary from the position converter, but
    // slice defensively — a mid-codepoint offset must degrade, not panic.
    let Some(seg) = text.get(open + 1..offset.min(text.len())) else {
        return false;
    };
    let current = seg.rsplit(['\n', ',']).next().unwrap_or("");
    !current.contains(':')
}

/// Whether the `(` at `open` is on a `capability X = Base @opt(…)` line: an
/// instance option, not a handler annotation.
fn on_capability_declaration(text: &str, open: usize) -> bool {
    let line_start = text[..open].rfind('\n').map_or(0, |i| i + 1);
    let line = text[line_start..open].trim_start();
    line.starts_with("capability ") && line.contains('=')
}

/// P3 (#2342): the values offered for an instance option at its value
/// position, `capability X = RateLimit @period(▮`: the rate limiting windows.
/// Empty anywhere else.
pub fn instance_option_values(
    text: &str,
    offset: usize,
) -> &'static [(&'static str, &'static str)] {
    let Some(open) = innermost_open_paren(text, offset) else {
        return &[];
    };
    if !on_capability_declaration(text, open) {
        return &[];
    }
    let Some(seg) = text.get(open + 1..offset.min(text.len())) else {
        return &[];
    };
    if !seg.trim().is_empty() {
        return &[];
    }
    let head = text[..open].trim_end();
    let before = head[..head.len() - word_before_brace(text, open).len()].trim_end();
    if !before.ends_with('@') {
        return &[];
    }
    match word_before_brace(text, open) {
        "period" => RATELIMIT_PERIODS,
        _ => &[],
    }
}

/// P3 (#2342): the instance completions on a `capability X = …` line, as
/// `(label, detail, insert text)`: after the `=`, each instantiable platform
/// capability; after `= <Base> `, a snippet of the base's required options
/// (`@limit(${1:100}) @period(${2:60.seconds})`), when it has any. Empty
/// anywhere else.
pub fn instance_declaration_completions(line: &str) -> Vec<(String, String, String)> {
    let line = line.trim_start();
    let Some(rest) = line.strip_prefix("capability ") else {
        return Vec::new();
    };
    let Some((_, after)) = rest.split_once('=') else {
        return Vec::new();
    };
    let kinds = || {
        bynk_check::firstparty::UNITS
            .iter()
            .flat_map(|u| u.instantiable.iter().map(move |i| (u.name, i)))
    };
    let typed = after.trim_start();
    if !typed.contains(char::is_whitespace) {
        // The base being typed (or not yet begun).
        return kinds()
            .filter(|(_, i)| i.capability.starts_with(typed))
            .map(|(unit, i)| {
                (
                    i.capability.to_string(),
                    format!("an instance of `{unit}`'s `{}`", i.capability),
                    i.capability.to_string(),
                )
            })
            .collect();
    }
    let (base, tail) = typed.split_once(char::is_whitespace).unwrap_or((typed, ""));
    if !tail.trim().is_empty() {
        return Vec::new();
    }
    let Some((_, kind)) = kinds().find(|(_, i)| i.capability == base) else {
        return Vec::new();
    };
    if kind.required.is_empty() {
        return Vec::new();
    }
    let snippet: Vec<String> = kind
        .required
        .iter()
        .enumerate()
        .map(|(n, o)| format!("@{o}(${{{}:{}}})", n + 1, option_sample(o)))
        .collect();
    let label: Vec<String> = kind.required.iter().map(|o| format!("@{o}(…)")).collect();
    vec![(
        label.join(" "),
        format!("the options every `{base}` instance needs"),
        snippet.join(" "),
    )]
}

/// A sample value of instance option `option`, as a snippet offers it.
fn option_sample(option: &str) -> &'static str {
    match option {
        "limit" => "100",
        "period" => "60.seconds",
        "jurisdiction" => "\"eu\"",
        "name" => "\"order-events\"",
        _ => "\"legacy-billing\"",
    }
}

/// The windows a `RateLimit` instance's `@period` takes.
pub const RATELIMIT_PERIODS: &[(&str, &str)] = &[
    (
        "10.seconds",
        "a 10-second window — one of the two the platform offers",
    ),
    (
        "60.seconds",
        "a 60-second window — one of the two the platform offers",
    ),
];

/// The cursor is at an argument-*name* position inside a `@raw( … )` annotation.
/// Mirrors [`in_limit_arg_position`].
pub fn in_raw_arg_position(text: &str, offset: usize) -> bool {
    let Some(open) = innermost_open_paren(text, offset) else {
        return false;
    };
    if word_before_brace(text, open) != "raw" {
        return false;
    }
    let head = text[..open].trim_end();
    let before_raw = head[..head.len() - "raw".len()].trim_end();
    if !before_raw.ends_with('@') {
        return false;
    }
    let Some(seg) = text.get(open + 1..offset.min(text.len())) else {
        return false;
    };
    let current = seg.rsplit(['\n', ',']).next().unwrap_or("");
    !current.contains(':')
}

/// v0.124 (slice 3): the cursor is completing the argument to a leading clause
/// keyword — `from`/`on`/`by`/`exports`/`provides <cursor>` — with only a
/// partial identifier typed after the keyword. `kw` must be a standalone word
/// (line start or whitespace before it), so a field named `from` or the `on`
/// inside `session` does not trigger it.
fn after_clause_keyword(line: &str, kw: &str) -> bool {
    let Some(idx) = line.rfind(kw) else {
        return false;
    };
    if !line[..idx]
        .chars()
        .last()
        .map(char::is_whitespace)
        .unwrap_or(true)
    {
        return false;
    }
    let after = &line[idx + kw.len()..];
    after.starts_with(char::is_whitespace)
        && after
            .trim_start()
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_')
}

/// #472 (finding 2, PR #827 review): `for all x: T, y: U where <cursor>` — a
/// generative-test binder's clause. Unlike a type declaration's or a refined
/// pattern's `where`, this one takes an arbitrary `Bool` expression over the
/// bound names, not the closed predicate catalogue; `after_clause_keyword`
/// only matches the keyword text, so this excludes that one `where` position
/// from the predicate-vocabulary branch. `line`'s last `where` is checked
/// against the text immediately before it, ignoring the binding list, since
/// `for`/`all` are contextual (not lexer) keywords only meaningful in this
/// exact `for all` sequence.
fn is_for_all_where(line: &str) -> bool {
    let Some(idx) = line.rfind("where") else {
        return false;
    };
    let head = line[..idx].trim_start();
    let Some(after_for) = head.strip_prefix("for") else {
        return false;
    };
    // `for` must be a standalone word (a whitespace boundary), not a prefix
    // of a longer identifier (`format(...)`).
    if !after_for.starts_with(char::is_whitespace) {
        return false;
    }
    let Some(after_all) = after_for.trim_start().strip_prefix("all") else {
        return false;
    };
    // Likewise `all` — `allocate` is not the `for all` keyword.
    after_all.is_empty() || after_all.starts_with(char::is_whitespace)
}

/// v0.124 (slice 3): the cursor sits in a contract-clause predicate —
/// `requires <name>: <cursor>` or `ensures <name>: <cursor>` — where the
/// enclosing function's parameters (and, for an `ensures`, `result`) are in
/// scope. Returns `Some(is_ensures)`; the parameters themselves are resolved
/// handler-side from the enclosing `fn` (needs the cursor offset).
pub fn contract_clause_kind(line: &str) -> Option<bool> {
    let colon = line.rfind(':')?;
    let clause = line[..colon].trim();
    for (kw, is_ensures) in [("requires", false), ("ensures", true)] {
        if let Some(rest) = clause.strip_prefix(kw) {
            let rest = rest.trim();
            if !rest.is_empty() && rest.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return Some(is_ensures);
            }
        }
    }
    None
}

/// The record fields of a project (or embedded-surface) type named `name`, as
/// field-name completions — the construction-position half of what
/// [`value_member_candidates`] offers on a value receiver.
///
/// Finding #62: stops at the first unit declaring `name` rather than unioning
/// fields across every matching unit — `for_each_unit` can otherwise yield
/// the live buffer *and* that same file's stale on-disk copy, and a name
/// removed from the buffer's declaration must not resurface from the disk
/// copy's fields.
fn record_field_names(
    name: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    let mut out: Vec<Completion> = Vec::new();
    let mut found = false;
    for_each_unit(doc_text, files, |unit| {
        if found {
            return;
        }
        let items = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            _ => return,
        };
        for item in items {
            if let CommonsItem::Type(t) = item
                && t.name.name == name
                && let TypeBody::Record(r) = &t.body
            {
                found = true;
                for f in &r.fields {
                    out.push(
                        Completion::item(
                            f.name.name.clone(),
                            CompletionKind::Field,
                            Some(format!("field of `{name}`")),
                        )
                        .with_doc(f.documentation.as_deref()),
                    );
                }
                return;
            }
        }
    });
    out
}

/// v0.124 (slice 3): the variants of a project (or embedded-surface) sum type
/// named `name`, as pattern completions — the `is`/`match` candidate set once
/// the scrutinee's type is known (resolved handler-side from `expr_types`).
/// `pub` so the completion handler can offer them at an `is` position.
///
/// Finding #62: stops at the first unit declaring `name`, for the same
/// buffer-vs-stale-disk-copy reason as `record_field_names` above.
pub fn sum_type_variants(
    name: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    let mut out: Vec<Completion> = Vec::new();
    let mut found = false;
    for_each_unit(doc_text, files, |unit| {
        if found {
            return;
        }
        let items = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            _ => return,
        };
        for item in items {
            if let CommonsItem::Type(t) = item
                && t.name.name == name
                && let TypeBody::Sum(s) = &t.body
            {
                found = true;
                for v in &s.variants {
                    out.push(
                        Completion::item(
                            v.name.name.clone(),
                            CompletionKind::Variant,
                            Some(format!("variant of `{name}`")),
                        )
                        .with_doc(v.documentation.as_deref()),
                    );
                }
                return;
            }
        }
    });
    out
}

/// v0.145 (ADR 0169, base gap for #565): the variants offerable for a scrutinee
/// `Ty` at a pattern position. A user-declared sum's variants come from source
/// (`sum_type_variants`); the built-in `Result`/`Option` variants do not (they
/// are not declared types, so `sum_type_variants` can't see them) and are
/// intrinsic here. This is why match-arm / `is` completion now fires for a
/// `Result`/`Option` scrutinee, not only a user sum.
pub fn variants_for_ty(
    ty: TyId,
    tys: &Types,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    match &*tys.get(ty) {
        Ty::Named { name, .. } => sum_type_variants(name, doc_text, files),
        Ty::Result(..) => built_in_variants(&["Ok", "Err"], "Result"),
        Ty::Option(..) => built_in_variants(&["Some", "None"], "Option"),
        _ => Vec::new(),
    }
}

/// v0.145 (ADR 0169, nested-variant completion for #565): the variants offerable
/// inside `OuterVariant(‸` within a match arm — the payload field type's
/// variants. Resolves the single-field payload type of `outer_variant` on the
/// scrutinee `ty` (the same shape as `bynk-emit`'s `payload_field_ty`):
/// `Result`/`Option`/`HttpResult` generic args come straight off the `Ty`, and a
/// user-declared sum's field type is walked from source. `Ok`/`Err` inside
/// `Some(‸)` on an `Option[Result[…]]` is the headline case.
pub fn nested_variant_completions(
    ty: TyId,
    tys: &Types,
    outer_variant: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    match &*tys.get(ty) {
        Ty::Result(t, e) => match outer_variant {
            "Ok" => variants_for_ty(*t, tys, doc_text, files),
            "Err" => variants_for_ty(*e, tys, doc_text, files),
            _ => Vec::new(),
        },
        Ty::HttpResult(t) if outer_variant == "Ok" => variants_for_ty(*t, tys, doc_text, files),
        Ty::Option(t) if outer_variant == "Some" => variants_for_ty(*t, tys, doc_text, files),
        Ty::Named {
            kind: NamedKind::Sum,
            name,
            ..
        } => payload_type_ref_variants(name, outer_variant, doc_text, files),
        _ => Vec::new(),
    }
}

/// The built-in variant names of `Result`/`Option` as `Variant` completions.
fn built_in_variants(names: &[&str], of: &str) -> Vec<Completion> {
    names
        .iter()
        .map(|v| {
            Completion::item(
                (*v).to_string(),
                CompletionKind::Variant,
                Some(format!("variant of `{of}`")),
            )
        })
        .collect()
}

/// The variants of the payload field type of `variant` on the user-declared sum
/// `sum_name`, walked from source (the LSP holds no resolved type map, so it
/// reads the field's `TypeRef` off the parsed decl — the source-side analogue of
/// `payload_field_ty`'s `commons.types` lookup).
fn payload_type_ref_variants(
    sum_name: &str,
    variant: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    let mut field_ty: Option<TypeRef> = None;
    for_each_unit(doc_text, files, |unit| {
        let items = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            _ => return,
        };
        for item in items {
            if let CommonsItem::Type(t) = item
                && t.name.name == sum_name
                && let TypeBody::Sum(s) = &t.body
                && let Some(v) = s.variants.iter().find(|v| v.name.name == variant)
                && let Some(f) = v.payload.first()
            {
                field_ty = Some(f.type_ref.clone());
            }
        }
    });
    match field_ty {
        Some(tr) => variants_for_type_ref(&tr, doc_text, files),
        None => Vec::new(),
    }
}

/// `variants_for_ty` over an unresolved `TypeRef` (a user-sum payload field). A
/// named type's variants come from source; `Result`/`Option` are intrinsic.
fn variants_for_type_ref(
    tr: &TypeRef,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    match tr {
        TypeRef::Named(id) => sum_type_variants(&id.name, doc_text, files),
        TypeRef::Result(..) => built_in_variants(&["Ok", "Err"], "Result"),
        TypeRef::Option(..) => built_in_variants(&["Some", "None"], "Option"),
        _ => Vec::new(),
    }
}

/// The service protocols offerable after `from`.
fn protocol_candidates() -> Vec<Completion> {
    ["http", "cron", "queue", "websocket"]
        .into_iter()
        .map(|p| Completion::item(p, CompletionKind::Keyword, Some("service protocol".into())))
        .collect()
}

/// The handler kinds offerable after `on`.
fn handler_kind_candidates() -> Vec<Completion> {
    [
        "call", "GET", "POST", "PUT", "PATCH", "DELETE", "schedule", "message", "open", "close",
        "rejected",
    ]
    .into_iter()
    .map(|k| Completion::item(k, CompletionKind::Keyword, Some("handler kind".into())))
    .collect()
}

/// The export kinds offerable after `exports` (adapter).
fn export_kind_candidates() -> Vec<Completion> {
    ["capability", "transparent", "opaque"]
        .into_iter()
        .map(|k| Completion::item(k, CompletionKind::Keyword, Some("export kind".into())))
        .collect()
}

/// The closed refinement-predicate vocabulary offerable after `where` —
/// shared by a type declaration's refinement and (#472) a match arm's
/// `_ where <predicate>`. Mirrors `PredKind::name()`.
fn predicate_name_candidates() -> Vec<Completion> {
    [
        "Matches",
        "InRange",
        "MinLength",
        "MaxLength",
        "Length",
        "NonNegative",
        "Positive",
        "NonEmpty",
    ]
    .into_iter()
    .map(|k| {
        Completion::item(
            k,
            CompletionKind::Keyword,
            Some("refinement predicate".into()),
        )
    })
    .collect()
}

/// The project's `actor` names, offerable after `by`.
fn actor_candidates(doc_text: &str, files: Option<&HashMap<PathBuf, String>>) -> Vec<Completion> {
    let mut out: Vec<Completion> = Vec::new();
    // #2135: in a suite, a call-site `by` may also name a reserved principal
    // (`Nobody`, `Forged`, `Stale`), which drives the seam to a `401`.
    // A lexical test, so it holds mid-edit, when the suite does not parse.
    let in_suite = lexer::tokenize(doc_text).ok().is_some_and(|toks| {
        toks.iter()
            .find(|t| !matches!(t.kind, lexer::TokenKind::Comment))
            .is_some_and(|t| t.kind == lexer::TokenKind::Suite)
    });
    if in_suite {
        for (name, doc) in crate::symbols::RESERVED_PRINCIPALS {
            out.push(
                Completion::item(
                    *name,
                    CompletionKind::Type,
                    Some("reserved test principal".into()),
                )
                .with_doc(Some(doc)),
            );
        }
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for_each_unit(doc_text, files, |unit| {
        let items = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            _ => return,
        };
        for item in items {
            if let CommonsItem::Actor(a) = item
                && seen.insert(a.name.name.clone())
            {
                out.push(Completion::item(
                    a.name.name.clone(),
                    CompletionKind::Type,
                    Some("actor".into()),
                ));
            }
        }
    });
    out
}

/// Built-in type statics — real language statics that are not user-declared, so
/// they come from this small table rather than the project parse. Covers the
/// numeric parse statics and the JSON codec (v0.22, ADRs 0048/0049), the
/// collection `empty` constructors (v0.20b), and `Effect.pure` (v0.5). The full
/// real set per ADR 0093 D2 — kept complete and drift-tested
/// (`builtin_statics_are_reachable`).
pub const BUILTIN_STATICS: &[(&str, &[(&str, &str)])] = &[
    ("Int", &[("parse", "parse(s: String) -> Option[Int]")]),
    ("Float", &[("parse", "parse(s: String) -> Option[Float]")]),
    (
        "Json",
        &[
            ("encode", "encode(value) -> String"),
            ("decode", "decode[T](s: String) -> Result[T, JsonError]"),
            (
                "decodeSql",
                "decodeSql[T](s: String) -> Result[T, JsonError]",
            ),
        ],
    ),
    ("List", &[("empty", "empty() -> List[T]")]),
    ("Map", &[("empty", "empty() -> Map[K, V]")]),
    (
        "Set",
        &[
            ("empty", "empty() -> Set[T]"),
            ("of", "of(xs: List[T]) -> Set[T]"),
        ],
    ),
    ("Effect", &[("pure", "pure(value) -> Effect[T]")]),
    (
        "Bytes",
        &[
            ("fromUtf8", "fromUtf8(s: String) -> Bytes"),
            ("fromBase64", "fromBase64(s: String) -> Option[Bytes]"),
            ("fromHex", "fromHex(s: String) -> Option[Bytes]"),
            ("fromBase64Url", "fromBase64Url(s: String) -> Option[Bytes]"),
            ("empty", "empty() -> Bytes"),
        ],
    ),
];

/// Variants of a built-in sum type (`HttpResult`/`QueueResult`), sourced from
/// the AST variant registries so a new variant surfaces in completion for free
/// (ADR 0093 D2/G3). Empty for any other receiver.
fn builtin_sum_variants(receiver: &str) -> Vec<(String, String)> {
    match receiver {
        "HttpResult" => bynk_syntax::ast::HTTP_VARIANTS
            .iter()
            .map(|v| {
                (
                    v.name.to_string(),
                    if v.status == bynk_syntax::ast::AUTHOR_CHOSEN_STATUS {
                        "variant of `HttpResult` (4xx/5xx, author-chosen)".to_string()
                    } else {
                        format!("variant of `HttpResult` ({})", v.status)
                    },
                )
            })
            .collect(),
        "QueueResult" => bynk_syntax::ast::QUEUE_VARIANTS
            .iter()
            .map(|v| (v.name.to_string(), "variant of `QueueResult`".to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Members of a name receiver: built-in type statics, then built-in sum-type
/// variants, then — from the project and embedded-surface parse — project sum
/// variants, refined/opaque `of`/`unsafe`, or capability operations. Yields `[]`
/// when the receiver resolves to none of these (e.g. a plain `type X = Int`
/// alias or a record).
fn member_candidates(
    receiver: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    member_candidates_from(receiver, doc_text, files, &mut BTreeSet::new())
}

/// [`member_candidates`], with the derived capabilities already followed
/// (S4a, #1966). Completion reads unchecked source, so a derivation cycle
/// (`capability A = B`, `capability B = A`) can be on screen: each receiver
/// is followed once.
fn member_candidates_from(
    receiver: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
    followed: &mut BTreeSet<String>,
) -> Vec<Completion> {
    followed.insert(receiver.to_string());
    if let Some((_, statics)) = BUILTIN_STATICS.iter().find(|(name, _)| *name == receiver) {
        return statics
            .iter()
            .map(|(label, sig)| {
                Completion::item(*label, CompletionKind::Member, Some(sig.to_string()))
            })
            .collect();
    }
    let mut out: Vec<Completion> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    // Built-in sum types (`HttpResult`/`QueueResult`) — variants from the AST
    // registry, on the same name-receiver path as project sums (ADR 0093 G3).
    // S4a (#1966): a derived capability's (`capability Sessions = Kv`) base,
    // whose operations it has.
    let mut derived_base: Option<String> = None;
    for (label, detail) in builtin_sum_variants(receiver) {
        if seen.insert(label.clone()) {
            out.push(Completion::item(
                label,
                CompletionKind::Variant,
                Some(detail),
            ));
        }
    }
    for_each_unit(doc_text, files, |unit| {
        let items = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            _ => return,
        };
        for item in items {
            match item {
                CommonsItem::Type(t) if t.name.name == receiver => match &t.body {
                    bynk_syntax::ast::TypeBody::Sum(s) => {
                        for v in &s.variants {
                            if seen.insert(v.name.name.clone()) {
                                out.push(
                                    Completion::item(
                                        v.name.name.clone(),
                                        CompletionKind::Variant,
                                        Some(format!("variant of `{receiver}`")),
                                    )
                                    .with_doc(v.documentation.as_deref()),
                                );
                            }
                        }
                    }
                    bynk_syntax::ast::TypeBody::Refined { .. }
                    | bynk_syntax::ast::TypeBody::Opaque { .. } => {
                        for (label, sig) in [
                            (
                                "of",
                                format!("of(value) -> Result[{receiver}, ValidationError]"),
                            ),
                            ("unsafe", format!("unsafe(value) -> {receiver}")),
                        ] {
                            if seen.insert(label.to_string()) {
                                out.push(Completion::item(
                                    label,
                                    CompletionKind::Member,
                                    Some(sig),
                                ));
                            }
                        }
                    }
                    // A plain alias (`type X = Int`) or a record has no
                    // name-receiver members — record fields are value-receiver
                    // (slice 3).
                    _ => {}
                },
                CommonsItem::Capability(c) if c.name.name == receiver && c.derived.is_some() => {
                    derived_base = c.derived.as_ref().map(|d| d.base.name.clone());
                }
                CommonsItem::Capability(c) if c.name.name == receiver => {
                    for op in &c.ops {
                        if seen.insert(op.name.name.clone()) {
                            // Typed signature (params + return), the same
                            // `type_ref_str` rendering hover/signature help use —
                            // not bare param names (slice 5 detail polish).
                            let params = op
                                .params
                                .iter()
                                .map(|p| format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)))
                                .collect::<Vec<_>>()
                                .join(", ");
                            // #926: `[T, …]` type parameters on the op itself.
                            let type_params = if op.type_params.is_empty() {
                                String::new()
                            } else {
                                let names: Vec<&str> = op
                                    .type_params
                                    .iter()
                                    .map(|tp| tp.name.name.as_str())
                                    .collect();
                                format!("[{}]", names.join(", "))
                            };
                            out.push(Completion::item(
                                op.name.name.clone(),
                                CompletionKind::Member,
                                Some(format!(
                                    "{}{type_params}({params}) -> {} — operation of `{receiver}`",
                                    op.name.name,
                                    type_ref_str(&op.return_type)
                                )),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    });
    if let Some(base) = derived_base.filter(|b| !followed.contains(b)) {
        for c in member_candidates_from(&base, doc_text, files, followed) {
            if seen.insert(c.label.clone()) {
                out.push(c);
            }
        }
    }
    out
}

// -- Positional candidate sources (static registries + project parse) --

/// Built-in type names not declared in any parseable source. Base and generic
/// types from the language core; collection types from `builtin_names`. Docs
/// are drawn from the `keywords` registry where present (one source of truth).
const BUILTIN_TYPES: &[&str] = &[
    bynk_check::builtin_names::types::INT,
    "Bool",
    bynk_check::builtin_names::types::FLOAT,
    "String",
    "Option",
    "Result",
    "Effect",
    bynk_check::builtin_names::types::LIST,
    bynk_check::builtin_names::types::MAP,
    bynk_check::builtin_names::types::SET,
];

/// Declaration snippets (`CompletionItemKind::SNIPPET`), as LSP snippet bodies.
/// `pub` so `tests/scaffolds_compile.rs` (ADR 0157) can enumerate them.
pub const SNIPPETS: &[(&str, &str)] = &[
    // -- Units --
    ("context", "context ${1:name} {\n\t$0\n}"),
    ("commons", "commons ${1:my.lib}\n\n$0"),
    (
        "adapter",
        "adapter ${1:name} {\n\tbinding \"${2:./module}\"\n\t$0\n}",
    ),
    // -- Unit-header clauses --
    ("uses", "uses ${1:module}"),
    ("consumes", "consumes ${1:bynk} { ${2:Random} }"),
    // -- Types --
    (
        "type record",
        "type ${1:Name} = {\n\t${2:field}: ${3:Int},\n}",
    ),
    ("type enum", "type ${1:Name} = enum {\n\t${2:Variant},\n}"),
    (
        "type refined",
        "type ${1:Name} = ${2:String} where ${3:MinLength(1)}",
    ),
    (
        "type opaque",
        "type ${1:Name} = opaque ${2:Int} where ${3:NonNegative}",
    ),
    // -- Functions --
    (
        "fn",
        "fn ${1:name}(${2:x}: ${3:Int}) -> ${4:Int} {\n\t$0\n}",
    ),
    (
        "fn contract",
        "fn ${1:name}(${2:x}: ${3:Int}) -> ${4:Int}\n\trequires ${5:in_range}: ${6:x >= 0}\n\tensures ${7:non_negative}: ${8:result >= 0}\n{\n\t$0\n}",
    ),
    // -- Capabilities & providers --
    (
        "capability",
        "capability ${1:Name} {\n\tfn ${2:op}() -> Effect[${3:Unit}]\n}",
    ),
    (
        "provides",
        "provides ${1:Cap} = ${2:Impl} {\n\tfn ${3:op}(${4}) -> Effect[${5:()}] {\n\t\tEffect.pure(${6:()})\n\t}\n}",
    ),
    // -- Actors & agents --
    (
        "actor",
        "actor ${1:Name} { auth = ${2:Bearer(secret = \"AUTH_JWT_SECRET\")}, identity = ${3:UserId} }",
    ),
    (
        "agent",
        "agent ${1:Name} {\n\tkey ${2:id}: ${3:String}\n\n\tstore ${4:status}: Cell[${5:Int}] = ${6:0}\n\n\tinvariant ${7:non_negative}: ${8:status >= 0}\n\n\ttransition ${9:monotonic}: ${10:new.status >= old.status}\n\n\ton call ${11:op}(${12}) -> Effect[Result[${13:()}, String]] {\n\t\tOk(${14:()})\n\t}\n}",
    ),
    // -- Services & handlers --
    (
        "service",
        "service ${1:name} {\n\ton call(${2}) -> Effect[${3:Unit}] {\n\t\t$0\n\t}\n}",
    ),
    ("on call", "on call(${1}) -> Effect[${2:Unit}] {\n\t$0\n}"),
    (
        "on http",
        "on ${1|GET,POST,PUT,DELETE,PATCH|}(\"${2:/path}\") (${3:body}: ${4:Req}) -> Effect[HttpResult[${5:Res}]] given ${6:Cap} {\n\t$0\n}",
    ),
    (
        "on cron",
        "on schedule(\"${1:0 * * * *}\") () -> Effect[Result[(), String]] {\n\t$0\n\tOk(())\n}",
    ),
    // -- Tests --
    (
        "suite",
        "suite ${1:target}\n\ncase \"${2:it works}\" {\n\tlet ${3:actual} = ${4:0}\n\texpect ${5:actual == 0}\n}",
    ),
    (
        "property",
        "property \"${1:invariant holds}\" {\n\tfor all ${2:x}: ${3:Int} {\n\t\texpect ${4:x == x}\n\t}\n}",
    ),
    // #2137: move the test harness's clock (a `case` body statement).
    ("advance Clock by", "advance Clock by ${1:15.minutes}"),
];

/// The value constructors offered at expression position (ADR 0093 D3) — the
/// closed set of `Result`/`Option` variant constructors and the boolean
/// literals. A value expression can begin with any of these; their docs reuse
/// the `keywords` registry (one source of truth).
const CONSTRUCTORS: &[&str] = &["Ok", "Err", "Some", "None", "true", "false"];

/// Expression-position candidates: the value constructors plus in-scope type
/// names (the entry to a static call like `Int.parse` or a record construction
/// like `Order { … }`). In-scope values — locals/params, and from slice 3 free
/// functions — are appended by the handler, which owns the analysis cache, so
/// they are not produced here (ADR 0093 D3).
fn expression_candidates(
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    let mut out: Vec<Completion> = CONSTRUCTORS
        .iter()
        .map(|&name| {
            Completion::item(
                name,
                CompletionKind::Constructor,
                keyword_doc(name).map(str::to_string),
            )
        })
        .collect();
    // Type names are valid here too (static receiver / record construction); the
    // `Type.` member context (slice 1) takes over once the user types the dot.
    out.extend(type_candidates(doc_text, files));
    // In-scope free functions — the current unit's own `fn`s and the combinators
    // of every `uses`-imported module (project + stdlib) — ADR 0093 D3 / G5 —
    // plus their assets (#1883), in scope by the same rule.
    out.extend(free_function_candidates(doc_text, files));
    out
}

/// A unit's top-level items and its `uses` clauses, for the kinds that carry
/// free functions. Service/other units contribute neither.
fn unit_items_and_uses(unit: &SourceUnit) -> (&[CommonsItem], &[UsesDecl]) {
    match unit {
        SourceUnit::Commons(c) => (&c.items, &c.uses),
        SourceUnit::Context(c) => (&c.items, &c.uses),
        SourceUnit::Adapter(a) => (&a.items, &a.uses),
        _ => (&[], &[]),
    }
}

/// The qualified name of the unit the cursor's document declares, via a recovery
/// parse (the header survives a mid-edit body). `None` for a headerless fragment
/// that names no unit.
fn current_unit_name(doc_text: &str) -> Option<String> {
    let tokens = lexer::tokenize(doc_text).ok()?;
    let (unit, _errs) = parser::parse_unit_with_recovery(&tokens, doc_text);
    Some(unit?.name().joined())
}

/// Standard-library S3 (#1963): whether a first-party item path is experimental.
fn experimental(path: &str) -> bool {
    bynk_check::firstparty::stability_of(path)
        == Some(bynk_check::firstparty::Stability::Experimental)
}

/// The completion-detail suffix that marks an experimental item, so it is
/// visible before it is used.
fn stability_suffix(experimental: bool) -> &'static str {
    if experimental {
        " — experimental"
    } else {
        ""
    }
}

/// Render a free function's signature for the completion detail, the same way
/// hover and signature help do (`symbols::type_ref_str`) — one format, never
/// divergent. Mirrors signature help, bounds included.
fn free_fn_signature(name: &str, f: &bynk_syntax::ast::FnDecl) -> String {
    let params = f
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{name}{}({params}) -> {}{}",
        crate::symbols::fn_type_params_str(f),
        type_ref_str(&f.return_type),
        crate::symbols::fn_where_str(f)
    )
}

/// Free-function candidates at expression position: the current unit's own
/// top-level `fn`s plus the free `fn`s of every `uses`-imported module (project
/// commons and the embedded stdlib). Gated on the `uses` set so a combinator is
/// offered only where it is actually in scope (ADR 0093 D3 / G5). The same
/// units' `asset` declarations (#1883) ride the same pass and the same gate —
/// an asset name resolves exactly where a free function's would.
fn free_function_candidates(
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    let Some(current) = current_unit_name(doc_text) else {
        return Vec::new();
    };
    // One parse pass: collect each unit's name, its free `fn`s (name + signature),
    // its assets (name + declaration), and its `uses` targets.
    struct UnitFns {
        name: String,
        fns: Vec<(String, String)>,
        assets: Vec<(String, String, Option<String>)>,
        embeds: Vec<(String, String, Option<String>)>,
        uses: Vec<String>,
    }
    let mut units: Vec<UnitFns> = Vec::new();
    for_each_unit(doc_text, files, |unit| {
        let (items, uses) = unit_items_and_uses(unit);
        let fns = items
            .iter()
            .filter_map(|it| match it {
                CommonsItem::Fn(f) => match &f.name {
                    FnName::Free(id) => Some((id.name.clone(), free_fn_signature(&id.name, f))),
                    FnName::Method { .. } => None,
                },
                _ => None,
            })
            .collect();
        let assets = items
            .iter()
            .filter_map(|it| match it {
                CommonsItem::Asset(a) => Some((
                    a.name.name.clone(),
                    format!(
                        "asset {} = \"{}\"{}",
                        a.name.name,
                        bynk_fmt::escape_string(&a.path),
                        a.at.as_ref()
                            .map(|(p, _)| format!(" at \"{}\"", bynk_fmt::escape_string(p)))
                            .unwrap_or_default()
                    ),
                    a.documentation.as_deref().map(str::to_string),
                )),
                _ => None,
            })
            .collect();
        // #2387: an embed rides the same pass and gate as an asset.
        let embeds = items
            .iter()
            .filter_map(|it| match it {
                CommonsItem::Embed(e) => Some((
                    e.name.name.clone(),
                    format!(
                        "embed {} = \"{}\"",
                        e.name.name,
                        bynk_fmt::escape_string(&e.path)
                    ),
                    e.documentation.as_deref().map(str::to_string),
                )),
                // #2617: a constant rides the same pass and gate as an embed.
                CommonsItem::Const(c) => Some((
                    c.name.name.clone(),
                    format!(
                        "const {}: {}",
                        c.name.name,
                        crate::symbols::type_ref_str(&c.ty)
                    ),
                    c.documentation.as_deref().map(str::to_string),
                )),
                _ => None,
            })
            .collect();
        units.push(UnitFns {
            name: unit.name().joined(),
            fns,
            assets,
            embeds,
            uses: uses.iter().map(|u| u.target.joined()).collect(),
        });
    });
    // The import scope: the `uses` targets of every unit sharing the current name
    // (a unit may span files, so union them).
    let mut imported: BTreeSet<String> = BTreeSet::new();
    for u in &units {
        if u.name == current {
            imported.extend(u.uses.iter().cloned());
        }
    }
    // Offer the current unit's own fns and the fns of each imported module.
    let mut out: Vec<Completion> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for u in &units {
        let own = u.name == current;
        if !own && !imported.contains(&u.name) {
            continue;
        }
        let origin = if own { "this unit" } else { u.name.as_str() };
        for (name, sig) in &u.fns {
            if seen.insert(name.clone()) {
                let marked = !own && experimental(&format!("{}.{name}", u.name));
                out.push(Completion::item(
                    name.clone(),
                    CompletionKind::Function,
                    Some(format!("{sig} — `{origin}`{}", stability_suffix(marked))),
                ));
            }
        }
        for (name, decl, doc) in u.assets.iter().chain(&u.embeds) {
            if seen.insert(name.clone()) {
                out.push(
                    Completion::item(
                        name.clone(),
                        CompletionKind::Constant,
                        Some(format!("{decl} — `{origin}`")),
                    )
                    .with_doc(doc.as_deref()),
                );
            }
        }
    }
    out
}

/// The one-line doc for a name in the `keywords` registry, if present.
/// `pub` so hover's bare-keyword fallback (ADR 0156) can reuse it —
/// completion and hover render the same doc, never a parallel copy.
pub fn keyword_doc(word: &str) -> Option<&'static str> {
    keywords::KEYWORDS
        .iter()
        .find(|k| k.word == word)
        .map(|k| k.meaning)
}

/// Type-position candidates: built-in types (with registry docs), then every
/// `type` declaration found in the project sources and the embedded `bynk`
/// surface (so the transparent surface types `Uuid`/`Method`/… come for free).
fn type_candidates(doc_text: &str, files: Option<&HashMap<PathBuf, String>>) -> Vec<Completion> {
    let mut out: Vec<Completion> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for &name in BUILTIN_TYPES {
        if seen.insert(name.to_string()) {
            let detail = keyword_doc(name)
                .map(str::to_string)
                .or_else(|| match name {
                    "List" => Some("The built-in list type, `List[T]`.".to_string()),
                    "Map" => Some("The built-in map type, `Map[K, V]`.".to_string()),
                    "Set" => Some("The built-in set type, `Set[T]`.".to_string()),
                    _ => Some("built-in type".to_string()),
                });
            out.push(Completion::item(name, CompletionKind::Type, detail));
        }
    }
    for_each_unit(doc_text, files, |unit| {
        let items = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            _ => return,
        };
        for item in items {
            if let CommonsItem::Type(t) = item
                && seen.insert(t.name.name.clone())
            {
                out.push(Completion::item(
                    t.name.name.clone(),
                    CompletionKind::Type,
                    Some("type".to_string()),
                ));
            }
        }
    });
    out
}

/// Keyword-position candidates: the lowercase-initial reserved keywords (the
/// declaration/statement words — uppercase type/value names like `Int`/`Some`
/// belong to type/expression position) with their registry docs, plus the
/// declaration snippets.
fn keyword_and_snippet_candidates() -> Vec<Completion> {
    let mut out: Vec<Completion> = keywords::KEYWORDS
        .iter()
        .filter(|k| k.word.chars().next().is_some_and(char::is_lowercase))
        .map(|k| Completion::item(k.word, CompletionKind::Keyword, Some(k.meaning.to_string())))
        .collect();
    for &(label, body) in SNIPPETS {
        out.push(Completion::snippet(label, body));
    }
    out
}

// -- Enumeration (parse project sources + the embedded `bynk` surface) --

/// Lex + recovery-parse one source, yielding its primary [`SourceUnit`]
/// (`None` when nothing parses — an empty or header-broken file).
fn parse_source_unit(src: &str) -> Option<SourceUnit> {
    let tokens = lexer::tokenize(src).ok()?;
    parser::parse_unit_with_recovery(&tokens, src).0
}

/// The embedded first-party surface, parsed **once** for the whole process.
/// These are compile-time `include_str!` constants, so their parse is fixed;
/// re-lexing and re-parsing all five on every keystroke-driven request was
/// pure waste (#733). The `bynk` surface, the `bynk.cloudflare` platform
/// adapter, and the stdlib commons (`bynk.list`/`bynk.map`/`bynk.string`) whose
/// free fns are enumerable for `uses`-imported completion (G5) and signature
/// help. Harmless to the other contexts — the commons declare only `fn`s (no
/// types/capabilities) and are never a `consumes` target.
static EMBEDDED_UNITS: LazyLock<Vec<Arc<SourceUnit>>> = LazyLock::new(|| {
    // The single first-party source list (`bynk-check::firstparty`), so a new
    // first-party commons is completion-visible without a second edit here
    // (#901 — `bynk.locale`/`bynk.locale.types` were missing from this copy).
    bynk_check::firstparty::FIRSTPARTY_SOURCES
        .iter()
        .filter_map(|(_, src)| parse_source_unit(src).map(Arc::new))
        .collect()
});

/// S4a (#1966): the first-party unit `unit` (`bynk.cloudflare`), from the
/// embedded surface, when it declares capability `cap`: what a derived
/// capability's hover renders its base's operations from.
pub(crate) fn embedded_capability(
    unit: &str,
    cap: &str,
) -> Option<bynk_syntax::ast::CapabilityDecl> {
    EMBEDDED_UNITS.iter().find_map(|u| {
        if u.name().joined() != unit {
            return None;
        }
        let SourceUnit::Adapter(a) = u.as_ref() else {
            return None;
        };
        a.items.iter().find_map(|item| match item {
            CommonsItem::Capability(c) if c.name.name == cap => Some(c.clone()),
            _ => None,
        })
    })
}

/// The parsed unit for a project file's already-read `content`. P8.4
/// (#1515): served from `bynk_project::parse_cache`'s own shared, durable
/// cache (replacing this module's former `PROJECT_UNIT_CACHE` — one cache
/// of the same fact, not two independently-invalidated ones, per ADR
/// 0413/[DECISION C]) when the cached/fresh **strict** parse is clean; a
/// syntax error elsewhere in the project falls back to a local, uncached
/// recovery-parse for that one file ([DECISION E], `bynk-project::
/// parse_cache`'s own doc comment) — for clean source (the overwhelming
/// common case) the strict and recovery parsers produce the identical AST,
/// since there is nothing to recover from.
///
/// PR #1520's own bot review (finding #4): `parser::
/// parse_units_with_warnings_from` (what the shared cache stores) always
/// converts an empty unit list into `Err("bynk.parse.unexpected_eof")`
/// (`bynk-syntax/src/parser.rs`'s own `parse_units_with_drain_check_from`)
/// — `Ok` with zero units is not a real case this parser produces, so
/// `units.first()` alone says what is meant without an `is_empty()` guard
/// that would otherwise route a (nonexistent) clean-but-empty parse down
/// the same uncached recovery-parse fallback as a genuine syntax error,
/// permanently, on every request.
fn cached_project_unit(path: &Path, content: &str) -> Option<Arc<SourceUnit>> {
    let (_file_id, result) = bynk_project::parse_cache::cached_parse(path, content);
    match result {
        Ok((units, _warnings)) => units.first().cloned().map(Arc::new),
        Err(_) => parse_source_unit(content).map(Arc::new),
    }
}

/// Parse every project unit, plus the embedded first-party adapters (the
/// `bynk` surface and the `bynk.cloudflare` platform adapter), and call `f`
/// for each. Recovery parsing tolerates the in-progress edit at the cursor.
///
/// The embedded surface is parsed once (`EMBEDDED_UNITS`) and the project's
/// other files are served from a per-file parse cache (`cached_project_unit`),
/// keyed on content rather than disk metadata (content-ownership track,
/// #1086, slice 0+1) — `files` is a pre-read `(path, content)` map the caller
/// (`bynk-lsp`) built by overlaying every open buffer over a disk sweep, so
/// an unsaved edit to file A is visible here exactly like a saved one. Only
/// `doc_text` — the buffer under the cursor, which changes per keystroke — is
/// parsed fresh each call (#733), never looked up in `files` (the caller
/// excludes the cursor's own file from `files` for exactly this reason).
/// Ordering is preserved (embedded, then the buffer, then the other files) so
/// a callback's first-name-wins dedup still prefers the live buffer.
pub fn for_each_unit(
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
    mut f: impl FnMut(&SourceUnit),
) {
    for unit in EMBEDDED_UNITS.iter() {
        f(unit);
    }
    if let Some(unit) = parse_source_unit(doc_text) {
        f(&unit);
    }
    // Slice A: the project's files come from the compiler's own discovery
    // (`bynk_ide::discover_files`) — every `include` root, `exclude` honoured.
    // This used to walk a single directory by hand, so completion could not see
    // a second root and swept `out`/`node_modules` if they sat beneath it.
    if let Some(content) = files {
        for (path, text) in content {
            if let Some(unit) = cached_project_unit(path, text) {
                f(&unit);
            }
        }
    }
}

/// #2223: `files` split into the project's own files and its dependencies'
/// files (each with its package), when the project has dependencies. A
/// dependency's units are never reachable by their bare names, so completion
/// that offers unit names reads only the project's own files, and offers a
/// dependency's units as `<alias>.<unit>`.
fn split_dependency_files<'f>(
    files: Option<&'f HashMap<PathBuf, String>>,
    project_root: Option<&Path>,
) -> SplitFiles<'f> {
    let Some(content) = files else {
        return SplitFiles::default();
    };
    let view = match project_root {
        Some(root) => crate::packages::DependencyView::for_root(root, content),
        None => crate::packages::DependencyView::from_files(content),
    };
    let Some(view) = view else {
        return SplitFiles {
            own: Some(Cow::Borrowed(content)),
            deps: Vec::new(),
        };
    };
    let mut own = HashMap::new();
    let mut deps = Vec::new();
    let mut keys: Vec<&PathBuf> = content.keys().collect();
    keys.sort();
    for path in keys {
        let text = &content[path];
        match view.package_of(path) {
            None => {
                own.insert(path.clone(), text.clone());
            }
            Some(id) => {
                if let Some(unit) = cached_project_unit(path, text) {
                    for alias in view.aliases_of(id) {
                        let declared = unit.name().joined();
                        if view.exports(id, &declared) {
                            deps.push((format!("{alias}.{declared}"), (*unit).clone()));
                        }
                    }
                }
            }
        }
    }
    SplitFiles {
        own: Some(Cow::Owned(own)),
        deps,
    }
}

/// [`split_dependency_files`]'s result: the project's own files (borrowed
/// unchanged when the project has no dependencies), and each reachable
/// dependency unit with the `<alias>.<unit>` label it's written as.
#[derive(Default)]
struct SplitFiles<'f> {
    own: Option<Cow<'f, HashMap<PathBuf, String>>>,
    deps: Vec<(String, SourceUnit)>,
}

impl SplitFiles<'_> {
    fn own(&self) -> Option<&HashMap<PathBuf, String>> {
        self.own.as_deref()
    }
}

/// #2223: commons a `uses` can name — the project's own, then each
/// dependency's exported commons as `<alias>.<unit>`.
fn usable_commons(doc_text: &str, split: &SplitFiles<'_>) -> Vec<Completion> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<Completion> = Vec::new();
    for_each_unit(doc_text, split.own(), |unit| {
        if let SourceUnit::Commons(c) = unit {
            let name = c.name.joined();
            if seen.insert(name.clone()) {
                out.push(Completion::item(
                    name,
                    CompletionKind::Unit,
                    Some("commons".to_string()),
                ));
            }
        }
    });
    for (label, unit) in &split.deps {
        if matches!(unit, SourceUnit::Commons(_)) && seen.insert(label.clone()) {
            out.push(Completion::item(
                label,
                CompletionKind::Unit,
                Some("commons from a dependency".to_string()),
            ));
        }
    }
    out
}

/// Consumable unit names: contexts and adapters (plus `bynk`), deduplicated.
/// #2223: a dependency's adapters are offered as `<alias>.<unit>`, its
/// contexts never (a context is never shared).
fn consumable_units(doc_text: &str, split: &SplitFiles<'_>) -> Vec<Completion> {
    let files = split.own();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<Completion> = Vec::new();
    for (label, unit) in &split.deps {
        if matches!(unit, SourceUnit::Adapter(_)) && seen.insert(label.clone()) {
            out.push(Completion::item(
                label.clone(),
                CompletionKind::Unit,
                Some("adapter from a dependency".to_string()),
            ));
        }
    }
    for_each_unit(doc_text, files, |unit| {
        let (name, kind) = match unit {
            SourceUnit::Context(c) => (c.name.joined(), "context"),
            SourceUnit::Adapter(a) => (a.name.joined(), "adapter"),
            _ => return,
        };
        if seen.insert(name.clone()) {
            out.push(Completion::item(
                name,
                CompletionKind::Unit,
                Some(kind.to_string()),
            ));
        }
    });
    out
}

/// The capability names a unit `exports capability`.
///
/// Finding #62: a first-wins guard, for the same buffer-vs-stale-disk-copy
/// reason as [`record_field_names`] — the `BTreeSet` still dedups a single
/// matching unit's own (possibly repeated) export clauses.
fn capabilities_of_unit(unit: &str, doc_text: &str, split: &SplitFiles<'_>) -> Vec<String> {
    // #2223: `<alias>.<unit>` names a dependency's adapter, declared there
    // under its own name.
    if let Some((_, dep)) = split.deps.iter().find(|(label, _)| label == unit) {
        let mut out: BTreeSet<String> = BTreeSet::new();
        if let SourceUnit::Adapter(a) = dep {
            for clause in &a.exports {
                if clause.kind == ExportKind::Capability {
                    for n in clause.names.iter().map(|e| &e.name) {
                        out.insert(n.name.clone());
                    }
                }
            }
        }
        return out.into_iter().collect();
    }
    let files = split.own();
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut found = false;
    for_each_unit(doc_text, files, |u| {
        if found {
            return;
        }
        let (name, exports) = match u {
            SourceUnit::Context(c) => (c.name.joined(), &c.exports),
            SourceUnit::Adapter(a) => (a.name.joined(), &a.exports),
            _ => return,
        };
        if name != unit {
            return;
        }
        found = true;
        for clause in exports {
            if clause.kind == ExportKind::Capability {
                for n in clause.names.iter().map(|e| &e.name) {
                    out.insert(n.name.clone());
                }
            }
        }
    });
    out.into_iter().collect()
}

/// Capabilities in scope for a `given` clause in the current document: locally
/// declared capabilities, bare names flattened by a braced `consumes`, and
/// `U.Cap` for each whole-unit `consumes U`.
fn in_scope_capabilities(doc_text: &str, split: &SplitFiles<'_>) -> Vec<Completion> {
    let mut labels: BTreeSet<String> = BTreeSet::new();
    let Ok(tokens) = lexer::tokenize(doc_text) else {
        return Vec::new();
    };
    let (Some(unit), _errs) = parser::parse_unit_with_recovery(&tokens, doc_text) else {
        return Vec::new();
    };
    let (items, consumes) = match &unit {
        SourceUnit::Context(c) => (&c.items, &c.consumes),
        SourceUnit::Adapter(a) => (&a.items, &EMPTY_CONSUMES),
        _ => return Vec::new(),
    };
    // Locally declared capabilities.
    for item in items {
        if let bynk_syntax::ast::CommonsItem::Capability(c) = item {
            labels.insert(c.name.name.clone());
        }
    }
    // Consumed capabilities: flattened bare names, or qualified `U.Cap`.
    for c in consumes {
        let unit_name = c.target.joined();
        match &c.selected {
            Some(names) => {
                for n in names {
                    labels.insert(n.name.clone());
                }
            }
            None => {
                let prefix = c
                    .alias
                    .as_ref()
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| unit_name.clone());
                for cap in capabilities_of_unit(&unit_name, doc_text, split) {
                    labels.insert(format!("{prefix}.{cap}"));
                }
            }
        }
    }
    labels
        .into_iter()
        .map(|label| {
            Completion::item(
                label,
                CompletionKind::Capability,
                Some("capability in scope".to_string()),
            )
        })
        .collect()
}

// -- Value-receiver `.method`/`.field` (slice 3, ADR 0063) --

/// If the cursor (byte `offset` into `text`) sits just after a **lowercase**
/// `receiver.`(`partial`) — a *value* receiver — return the buffer **rewritten**
/// so the receiver is a complete expression (the trailing `.partial` dropped,
/// so the file parses) and the byte offset of the receiver to type. Returns
/// `None` for an uppercase name receiver (slice 2), a decimal `1.`, or a
/// `.`-qualified segment.
///
/// The rewrite is the spike's fix for the mid-edit parse: a bare `email.`
/// cascades and loses the receiver, but `email` (dot dropped) types cleanly.
pub fn value_receiver_rewrite(text: &str, offset: usize) -> Option<(String, usize)> {
    let prefix = text.get(..offset)?;
    let head = prefix
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .strip_suffix('.')?;
    let (recv, start) = ident_ending_at(head, head.len())?;
    let first = recv.chars().next()?;
    if !(first.is_ascii_lowercase() || first == '_') {
        return None; // uppercase = name receiver (slice 2); a digit = a decimal
    }
    if head[..start].ends_with('.') {
        return None; // a `.`-qualified segment, not a bare value receiver
    }
    let dot = head.len(); // the receiver ends here; the dot was the next byte
    let rewritten = format!("{}{}", &text[..dot], &text[offset..]);
    Some((rewritten, dot.saturating_sub(1)))
}

/// The identifier run ending at byte `end` in `text` — `(name, start)` — the
/// trim-back-to-a-non-identifier-boundary scan shared by every receiver/
/// identifier extraction in the LSP: [`value_receiver_rewrite`] above,
/// `symbols::receiver_segment_at` (a member's receiver, dot-preceded), and
/// `symbols::store_field_kind_at` (a bare receiver's own end offset, no dot).
/// Each has different preconditions on what precedes `end`, but the boundary
/// scan itself — walk back to the nearest non-identifier char, respecting
/// UTF-8 boundaries — is one definition, so it can't drift between them (a
/// review flagged the previous three near-identical copies). `None` if
/// nothing identifier-shaped precedes `end`.
pub fn ident_ending_at(text: &str, end: usize) -> Option<(&str, usize)> {
    let before = text.get(..end)?;
    let start = before
        .char_indices()
        .rfind(|&(_, c)| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let name = &before[start..];
    (!name.is_empty()).then_some((name, start))
}

/// The members of a typed value receiver: the built-in kernel methods of its
/// type (from the enumerable registry) plus, for a record, its fields.
pub fn value_member_candidates(
    ty: TyId,
    tys: &Types,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    let mut out: Vec<Completion> = kernel_methods::methods_for(ty, tys)
        .iter()
        .map(|km| {
            Completion::item(
                km.name,
                CompletionKind::Member,
                Some(km.signature.to_string()),
            )
        })
        .collect();
    // An asset's one field (#1883).
    if matches!(&*tys.get(ty), Ty::Asset) {
        out.push(Completion::item(
            "url",
            CompletionKind::Field,
            Some("String — the URL the platform serves the asset at".to_string()),
        ));
    }
    // Record fields — resolve the receiver's named type to its declaration.
    if let Ty::Named { name, .. } = &*tys.get(ty) {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for_each_unit(doc_text, files, |unit| {
            let items = match unit {
                SourceUnit::Commons(c) => &c.items,
                SourceUnit::Context(c) => &c.items,
                SourceUnit::Adapter(a) => &a.items,
                _ => return,
            };
            for item in items {
                if let CommonsItem::Type(t) = item
                    && &t.name.name == name
                    && let TypeBody::Record(r) = &t.body
                {
                    for f in &r.fields {
                        if seen.insert(f.name.name.clone()) {
                            out.push(
                                Completion::item(
                                    f.name.name.clone(),
                                    CompletionKind::Field,
                                    Some(format!("field of `{name}`")),
                                )
                                .with_doc(f.documentation.as_deref()),
                            );
                        }
                    }
                }
            }
        });
    }
    out
}

/// #2138: `id` after `run.` in a `case` body that hasn't bound its own
/// `run` — the test run's identity. `rewritten`/`recv_offset` are
/// [`value_receiver_rewrite`]'s output (`recv_offset` is the receiver's last
/// byte). Empty anywhere else.
pub fn run_id_member_candidates(rewritten: &str, recv_offset: usize) -> Vec<Completion> {
    let Some((name, start)) = ident_ending_at(rewritten, recv_offset + 1) else {
        return Vec::new();
    };
    if name != "run" || !crate::symbols::is_test_run_receiver(rewritten, start) {
        return Vec::new();
    }
    vec![
        Completion::item("id", CompletionKind::Field, Some("String".to_string()))
            .with_doc(Some(crate::symbols::RUN_ID_DOC)),
    ]
}

/// #596: the entry ops (and, for a `Map`, the `.entries`/`.keys`/`.values`
/// query accessors) of a bare `store` field receiver — merged onto
/// [`value_member_candidates`] so a store field offers its whole vocabulary,
/// not just the `Query` half `kernel_methods::methods_for` covers. A bare
/// store `Map` field types (via the checker's ADR 0120 "whole map as a value"
/// reading) to plain `Ty::Query`, indistinguishable from an ordinary
/// `Query`-typed local — so this reads the receiver's *provenance* instead,
/// the same way hover's `describe_store_op_at` does. Empty when the receiver
/// isn't a store field of an enclosing agent, or is shadowed by a local.
///
/// `rewritten`/`recv_offset` are [`value_receiver_rewrite`]'s output; `locals`
/// is the current round's locals for the file, best-effort (empty when the
/// analysed round doesn't match `rewritten`, in which case the shadowing
/// check simply sees no local in scope).
pub fn store_field_member_candidates(
    rewritten: &str,
    recv_offset: usize,
    locals: &[LocalBinding],
) -> Vec<Completion> {
    let Some((kind_head, held)) =
        crate::symbols::store_field_kind_at(rewritten, recv_offset + 1, locals)
    else {
        return Vec::new();
    };
    let mut out: Vec<Completion> = store_ops::ops_for(&kind_head)
        .iter()
        .map(|o| {
            Completion::item(
                o.name,
                CompletionKind::Member,
                Some(o.signature.to_string()),
            )
        })
        .collect();
    if kind_head == "Map" && !held {
        out.extend(store_ops::MAP_QUERY_ACCESSORS.iter().map(|a| {
            Completion::item(a.name, CompletionKind::Field, Some(a.signature.to_string()))
        }));
    }
    out
}

static EMPTY_CONSUMES: Vec<bynk_syntax::ast::ConsumesDecl> = Vec::new();

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_check::firstparty::BYNK_LIST_SRC;

    /// S3 (#1963): an experimental first-party item is marked in its completion
    /// detail; nothing ships experimental, so a stable one is unmarked.
    #[test]
    fn completion_marks_experimental_items() {
        assert_eq!(stability_suffix(true), " — experimental");
        assert_eq!(stability_suffix(false), "");
        assert!(!experimental("bynk.cloudflare.Kv"));
        let caps = complete("consumes bynk.cloudflare { ", "context app.probe\n", None);
        let kv = caps
            .iter()
            .find(|c| c.label == "Kv")
            .expect("Kv is offered");
        assert_eq!(
            kv.detail.as_deref(),
            Some("capability exported by `bynk.cloudflare`")
        );
    }

    fn labels(line: &str, doc: &str) -> Vec<String> {
        complete(line, doc, None)
            .into_iter()
            .map(|c| c.label)
            .collect()
    }

    /// Constrained generics slice 0: the constraint names are offered after the
    /// `:` / `+` of a bound, in a type-parameter list and in a `where` clause,
    /// and only there.
    #[test]
    fn constraint_names_follow_a_bound_colon() {
        let doc = "commons m\n";
        let eq = |line: &str| {
            complete(line, doc, None)
                .into_iter()
                .filter(|c| c.kind == CompletionKind::Constraint)
                .map(|c| c.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(eq("fn same[T: "), ["Eq"]);
        assert_eq!(eq("fn same[T: E"), ["Eq"]);
        assert_eq!(eq("fn same[T: Eq + "), ["Eq"]);
        assert_eq!(eq("fn same[A, T: "), ["Eq"]);
        assert_eq!(eq("type Pair[T: "), ["Eq"]);
        assert_eq!(eq("fn Box.has(self, x: A) -> Bool where A: "), ["Eq"]);
        assert_eq!(eq("fn f(self) -> Int where A: Eq, B: "), ["Eq"]);
        // A parameter's type annotation and a refinement `where` are not bounds.
        assert!(eq("fn f(x: ").is_empty());
        assert!(eq("fn f(x: Int) -> ").is_empty());
        assert!(eq("type Age = Int where ").is_empty());
        assert!(eq("fn f[T](x: ").is_empty());
    }

    #[test]
    fn consumes_target_suggests_units_including_bynk() {
        // An adapter in the open doc plus the always-available `bynk` surface.
        let doc = "adapter tokens {\n  binding \"./b.ts\"\n  capability Jwt { fn f() -> Effect[Int] }\n  provides Jwt = X\n}\n";
        let got = labels("  consumes ", doc);
        assert!(got.contains(&"bynk".to_string()), "{got:?}");
        assert!(got.contains(&"tokens".to_string()), "{got:?}");
    }

    #[test]
    fn consumes_brace_suggests_that_units_capabilities() {
        let got = labels("  consumes bynk { ", "context a.b\n");
        // The embedded `bynk` surface exports these.
        assert!(got.contains(&"Clock".to_string()), "{got:?}");
        assert!(got.contains(&"Random".to_string()), "{got:?}");
        assert!(got.contains(&"Logger".to_string()), "{got:?}");
    }

    #[test]
    fn given_suggests_local_and_flattened_capabilities() {
        let doc = "context a.b\n\
                   consumes bynk { Clock }\n\
                   capability Local { fn f() -> Effect[Int] }\n\
                   service s {\n\
                   on call() -> Effect[Int] given Clock {\n\
                   1\n\
                   }\n\
                   }\n";
        let got = labels("    on call() -> Effect[Int] given ", doc);
        assert!(got.contains(&"Clock".to_string()), "flattened: {got:?}");
        assert!(got.contains(&"Local".to_string()), "local: {got:?}");
    }

    #[test]
    fn expression_position_offers_constructors_and_types() {
        // ADR 0093 D3/D5: a value position (after `=`) yields every constructor
        // keyword and in-scope type names — the entry to a static call or a
        // record construction. (Locals/params are appended handler-side, not by
        // `complete()`.) Registry-driven over CONSTRUCTORS.
        let doc = "commons m {\n  type Order = { id: Int }\n}\n";
        let items = complete("  let x = ", doc, None);
        for &c in CONSTRUCTORS {
            assert!(
                find(&items, c, CompletionKind::Constructor).is_some(),
                "constructor {c}: {:?}",
                items.iter().map(|i| &i.label).collect::<Vec<_>>()
            );
        }
        assert!(
            find(&items, "Int", CompletionKind::Type).is_some(),
            "builtin type"
        );
        assert!(
            find(&items, "Order", CompletionKind::Type).is_some(),
            "project type"
        );
    }

    #[test]
    fn value_receiver_and_decimal_are_not_expression_positions() {
        // A trailing `x.`/`1.` is a member/decimal context, not an expression
        // start — `complete()` yields nothing (the value-receiver path is
        // handler-side; see `record_value_and_decimal_receivers_yield_nothing`).
        assert!(complete("  let p = q.", "context a.b\n", None).is_empty());
        assert!(complete("  let n = 1.", "context a.b\n", None).is_empty());
    }

    /// Free `fn` names declared in a unit source (registry-driven test helper).
    fn free_fn_names(src: &str) -> Vec<String> {
        let tokens = lexer::tokenize(src).unwrap();
        let (unit, _) = parser::parse_unit_with_recovery(&tokens, src);
        let unit = unit.unwrap();
        let (items, _) = unit_items_and_uses(&unit);
        items
            .iter()
            .filter_map(|it| match it {
                CommonsItem::Fn(f) => match &f.name {
                    FnName::Free(id) => Some(id.name.clone()),
                    FnName::Method { .. } => None,
                },
                _ => None,
            })
            .collect()
    }

    #[test]
    fn free_functions_offered_for_own_unit_and_used_modules() {
        // ADR 0093 D3/G5: expression position offers the current unit's own
        // free `fn`s and the combinators of every `uses`-imported module.
        let doc = "commons app {\n  uses bynk.list\n  fn helper(x: Int) -> Int { x }\n}\n";
        let items = complete("  let y = ", doc, None);
        // The current unit's own function.
        assert!(
            find(&items, "helper", CompletionKind::Function).is_some(),
            "own fn: {:?}",
            items.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
        // Every combinator of the imported `bynk.list` — registry-driven over the
        // embedded source, so a new stdlib combinator must surface or this fails.
        for name in free_fn_names(BYNK_LIST_SRC) {
            assert!(
                find(&items, &name, CompletionKind::Function).is_some(),
                "bynk.list.{name}: {:?}",
                items.iter().map(|i| &i.label).collect::<Vec<_>>()
            );
        }
        // A module that is not imported does not leak its fns.
        assert!(
            find(&items, "values", CompletionKind::Function).is_none(),
            "bynk.map.values leaked without `uses bynk.map`"
        );
    }

    #[test]
    fn locale_functions_offered_when_bynk_locale_is_used() {
        // #901: `bynk.locale` was absent from the embedded-source list, so its
        // builders/render never surfaced in completion even under `uses
        // bynk.locale`. Exercised through the real `complete()` path, not the
        // renderer alone.
        let doc = "commons app {\n  uses bynk.locale\n}\n";
        let items = complete("  let y = ", doc, None);
        for name in [
            "render",
            "message",
            "withText",
            "withWhole",
            "withNum",
            "withMoment",
        ] {
            assert!(
                find(&items, name, CompletionKind::Function).is_some(),
                "bynk.locale.{name} missing from completion: {:?}",
                items.iter().map(|i| &i.label).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn free_functions_require_a_uses_import() {
        // Own fns are always in scope; stdlib combinators only with their `uses`.
        let doc = "commons app {\n  fn helper(x: Int) -> Int { x }\n}\n";
        let items = complete("  let y = ", doc, None);
        assert!(find(&items, "helper", CompletionKind::Function).is_some());
        for name in ["map", "filter", "reverse"] {
            assert!(
                find(&items, name, CompletionKind::Function).is_none(),
                "bynk.list.{name} offered without `uses bynk.list`"
            );
        }
    }

    #[test]
    fn assets_offered_by_the_free_function_scope_rule() {
        // #1883: an asset name resolves where a free function's would — the
        // unit's own assets, and those of a `uses`-imported unit; never one
        // the unit does not import.
        let doc = "context app.page {\n  uses app.shared\n  asset logo = \"logo.svg\"\n  asset icon = \"favicon.ico\" at \"/favicon.ico\"\n}\n";
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("/p/shared.bynk"),
            "context app.shared {\n  --- The site stylesheet.\n  asset style = \"site.css\"\n}\n"
                .to_string(),
        );
        files.insert(
            PathBuf::from("/p/other.bynk"),
            "context app.other {\n  asset banner = \"banner.png\"\n}\n".to_string(),
        );
        let items = complete("  let y = ", doc, Some(&files));
        let labels = || items.iter().map(|i| &i.label).collect::<Vec<_>>();
        let logo = find(&items, "logo", CompletionKind::Constant);
        assert!(logo.is_some(), "own asset: {:?}", labels());
        assert_eq!(
            logo.unwrap().detail.as_deref(),
            Some("asset logo = \"logo.svg\" — `this unit`")
        );
        assert_eq!(
            find(&items, "icon", CompletionKind::Constant).and_then(|c| c.detail.as_deref()),
            Some("asset icon = \"favicon.ico\" at \"/favicon.ico\" — `this unit`")
        );
        let style = find(&items, "style", CompletionKind::Constant);
        assert!(style.is_some(), "used unit's asset: {:?}", labels());
        assert_eq!(
            style.unwrap().documentation.as_deref(),
            Some("The site stylesheet.")
        );
        assert!(
            find(&items, "banner", CompletionKind::Constant).is_none(),
            "an asset of a unit not in `uses` leaked: {:?}",
            labels()
        );
    }

    #[test]
    fn embeds_offered_by_the_free_function_scope_rule() {
        // #2387: an embed name resolves where a free function's would — the
        // unit's own embeds, and those of a `uses`-imported unit; never one
        // the unit does not import.
        let doc =
            "context app.page {\n  uses app.shared\n  embed shell = \"embeds/shell.html\"\n}\n";
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("/p/shared.bynk"),
            "commons app.shared {\n  --| The mail body.\n  embed mail = \"embeds/mail.txt\"\n}\n"
                .to_string(),
        );
        files.insert(
            PathBuf::from("/p/other.bynk"),
            "context app.other {\n  embed hidden = \"embeds/hidden.txt\"\n}\n".to_string(),
        );
        let items = complete("  let y = ", doc, Some(&files));
        let labels = || items.iter().map(|i| &i.label).collect::<Vec<_>>();
        assert_eq!(
            find(&items, "shell", CompletionKind::Constant).and_then(|c| c.detail.as_deref()),
            Some("embed shell = \"embeds/shell.html\" — `this unit`"),
            "own embed: {:?}",
            labels()
        );
        let mail = find(&items, "mail", CompletionKind::Constant);
        assert!(mail.is_some(), "used unit's embed: {:?}", labels());
        assert_eq!(
            mail.unwrap().documentation.as_deref(),
            Some("The mail body.")
        );
        assert!(
            find(&items, "hidden", CompletionKind::Constant).is_none(),
            "an embed of a unit not in `uses` leaked: {:?}",
            labels()
        );
    }

    #[test]
    fn asset_member_completion_offers_url() {
        // #1883: an asset value's one field.
        let asset = TYS.intern(Ty::Asset);
        let items = value_member_candidates(asset, &TYS, "context a.b\n", None);
        assert!(find(&items, "url", CompletionKind::Field).is_some());
    }

    #[test]
    fn member_completion_reaches_inside_an_interpolation_hole() {
        // v0.43: a `Type.`/`Cap.` receiver inside a `\(…)` hole completes just
        // as it does in bare expression position — context detection is purely
        // lexical, so the surrounding string and `\(` do not interfere.
        let doc = "context a.b\n  capability Timer { fn now() -> Effect[Int] }\n";
        let in_hole = complete("    \"the time is \\(Timer.", doc, None);
        assert!(
            find(&in_hole, "now", CompletionKind::Member).is_some(),
            "capability op not offered inside a hole: {:?}",
            in_hole.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
        // A built-in static receiver works inside a hole too.
        let statics = complete("  \"n=\\(Int.", "context a.b\n", None);
        assert!(find(&statics, "parse", CompletionKind::Member).is_some());
    }

    #[test]
    fn consumes_with_as_is_not_a_target_completion() {
        // `consumes X as ` is aliasing, not target-name completion.
        assert!(!is_consumes_target("consumes platform.time as "));
        assert!(is_consumes_target("consumes platform"));
    }

    fn find<'a>(
        items: &'a [Completion],
        label: &str,
        kind: CompletionKind,
    ) -> Option<&'a Completion> {
        items.iter().find(|c| c.label == label && c.kind == kind)
    }

    #[test]
    fn type_annotation_suggests_builtins_surface_and_project_types() {
        let doc = "commons m {\n  type Order = { id: Int }\n}\n";
        let got = labels("  let x: ", doc);
        // Built-ins (with registry docs), the `bynk`-surface transparent types,
        // and the project's own type declaration.
        for want in ["Int", "Option", "Result", "Effect", "List", "Map"] {
            assert!(got.contains(&want.to_string()), "built-in {want}: {got:?}");
        }
        assert!(got.contains(&"Uuid".to_string()), "surface: {got:?}");
        assert!(got.contains(&"Order".to_string()), "project: {got:?}");
    }

    #[test]
    fn return_type_and_type_args_are_type_positions() {
        assert!(is_type_position("  on call() -> "));
        assert!(is_type_position("  let x: Option["));
        assert!(is_type_position("  let x: Result[Int, "));
        // A partial type name being typed still counts.
        assert!(is_type_position("  -> Eff"));
    }

    #[test]
    fn list_literal_is_not_a_type_position() {
        // A bare `[` opening a list literal is an expression, not type args…
        assert!(!is_type_position("  let xs = ["));
        // …so it is an expression position: a list element is a value, and the
        // constructor keywords are offered there (ADR 0093 D3) — not a
        // type-argument completion.
        let items = complete("  let xs = [", "context a.b\n", None);
        assert!(
            find(&items, "Some", CompletionKind::Constructor).is_some(),
            "{:?}",
            items.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn builtin_type_carries_its_registry_doc() {
        let items = complete("  let x: ", "context a.b\n", None);
        let int = find(&items, "Int", CompletionKind::Type).expect("Int present");
        assert_eq!(int.detail.as_deref(), keyword_doc("Int"));
        assert!(int.detail.is_some(), "Int should have a doc");
    }

    #[test]
    fn keyword_position_suggests_keywords_and_snippets() {
        let items = complete("  ", "context a.b\n", None);
        // Declaration/statement keywords, with docs.
        assert!(find(&items, "capability", CompletionKind::Keyword).is_some());
        assert!(find(&items, "fn", CompletionKind::Keyword).is_some());
        assert!(find(&items, "let", CompletionKind::Keyword).is_some());
        // Uppercase type/value names are *not* keyword-position candidates.
        assert!(find(&items, "Int", CompletionKind::Keyword).is_none());
        assert!(find(&items, "Some", CompletionKind::Keyword).is_none());
        // Snippets are offered alongside.
        let snip = find(&items, "service", CompletionKind::Snippet).expect("service snippet");
        let body = snip.insert_text.as_deref().unwrap_or("");
        assert!(body.contains("on call"), "snippet body: {body:?}");
        assert!(body.contains("${1"), "snippet tab stop: {body:?}");
    }

    #[test]
    fn keyword_position_fires_on_an_empty_line() {
        assert!(is_keyword_position(""));
        assert!(is_keyword_position("  cap"));
        assert!(!is_keyword_position("  let x ="));
        assert!(!is_keyword_position("  x: "));
        assert!(!complete("", "context a.b\n", None).is_empty());
    }

    #[test]
    fn member_receiver_is_a_single_upper_ident_before_a_dot() {
        assert_eq!(member_receiver("  Color."), Some("Color".to_string()));
        assert_eq!(
            member_receiver("  let e = Email.o"),
            Some("Email".to_string())
        );
        assert_eq!(member_receiver("  x."), None); // lowercase = value receiver (slice 3)
        assert_eq!(member_receiver("  1."), None); // decimal literal, not a member access
        assert_eq!(member_receiver("  a.B."), None); // `.`-qualified segment
        assert_eq!(member_receiver("  Color"), None); // no dot yet
    }

    #[test]
    fn receiver_extraction_survives_a_multibyte_char_before_the_receiver() {
        // A multi-byte non-identifier char immediately before the receiver used
        // to make the `i + 1` byte offset land mid-codepoint → panic on slice
        // (#715, hit on every completion/hover keystroke inside a string).
        assert_eq!(member_receiver("\"Foo."), Some("Foo".to_string()));
        assert_eq!(member_receiver("€Color."), Some("Color".to_string()));
        assert_eq!(member_receiver("—Bar."), Some("Bar".to_string()));
        assert_eq!(word_before_brace("\"cors {", 6), "cors");
        assert_eq!(
            record_construction_receiver("\"€Order {"),
            Some("Order".to_string())
        );
        // The reproduction from the issue: a completion request against a buffer
        // where the line prefix opens a string literal must not panic.
        let _ = complete("let x = \"Foo.", "commons m {}\n", None);
        let _ = complete("  \"€42.", "commons m {}\n", None);
        assert_eq!(
            value_receiver_rewrite("\"email.", 7).map(|(_, r)| r),
            Some(5),
        );
    }

    #[test]
    fn sum_member_suggests_variants() {
        let doc = "commons m {\n  type Color = enum { Red, Green, Blue }\n}\n";
        let items = complete("  let c = Color.", doc, None);
        for v in ["Red", "Green", "Blue"] {
            assert!(
                find(&items, v, CompletionKind::Variant).is_some(),
                "variant {v}: {:?}",
                items.iter().map(|c| &c.label).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn refined_and_plain_alias_members_are_of_and_unsafe() {
        // A refinement-bearing type…
        let doc = "commons m {\n  type Email = String where NonEmpty\n}\n";
        let items = complete("  Email.", doc, None);
        assert!(find(&items, "of", CompletionKind::Member).is_some());
        assert!(find(&items, "unsafe", CompletionKind::Member).is_some());
        // …and a plain alias `type Id = Int` is *also* branded (the emitter
        // emits Id.of/Id.unsafe for every Refined body, refinement or not).
        let doc = "commons m {\n  type Id = Int\n}\n";
        assert!(find(&complete("  Id.", doc, None), "of", CompletionKind::Member).is_some());
    }

    #[test]
    fn capability_member_suggests_ops() {
        let doc = "context a.b\n  capability Timer { fn now() -> Effect[Int]\n  fn at(t: Int) -> Effect[()] }\n";
        let items = complete("    Timer.", doc, None);
        let now = find(&items, "now", CompletionKind::Member).expect("`now` op offered");
        // Slice 5 detail polish: a typed signature (params + return), not bare
        // param names.
        assert_eq!(
            now.detail.as_deref(),
            Some("now() -> Effect[Int] — operation of `Timer`")
        );
        let at = find(&items, "at", CompletionKind::Member).expect("`at` op offered");
        assert_eq!(
            at.detail.as_deref(),
            Some("at(t: Int) -> Effect[()] — operation of `Timer`")
        );
    }

    #[test]
    fn builtin_type_statics_are_offered() {
        assert!(
            find(
                &complete("  Int.", "context a.b\n", None),
                "parse",
                CompletionKind::Member
            )
            .is_some()
        );
        let j = complete("  Json.", "context a.b\n", None);
        assert!(find(&j, "encode", CompletionKind::Member).is_some());
        assert!(find(&j, "decode", CompletionKind::Member).is_some());
    }

    #[test]
    fn builtin_sum_variants_are_complete() {
        // ADR 0093 D5/G3: every built-in sum variant in the AST registry must
        // surface on its name receiver. Registry-driven — adding an
        // `HttpResult`/`QueueResult` variant must appear in completion or this
        // fails (the standing drift guard, mirroring `kernel_registry`).
        let http: Vec<&str> = bynk_syntax::ast::HTTP_VARIANTS
            .iter()
            .map(|v| v.name)
            .collect();
        let queue: Vec<&str> = bynk_syntax::ast::QUEUE_VARIANTS
            .iter()
            .map(|v| v.name)
            .collect();
        for (recv, names) in [("HttpResult", http), ("QueueResult", queue)] {
            let items = complete(&format!("  {recv}."), "context a.b\n", None);
            for name in names {
                assert!(
                    find(&items, name, CompletionKind::Variant).is_some(),
                    "{recv}.{name} missing: {:?}",
                    items.iter().map(|c| &c.label).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn builtin_statics_are_reachable() {
        // ADR 0093 D5/G2: every BUILTIN_STATICS entry is reachable through the
        // name-receiver context — exercises the member_receiver→member_candidates
        // wiring for each receiver (e.g. that `Effect.`/`List.` are recognised).
        for &(recv, members) in BUILTIN_STATICS {
            let items = complete(&format!("  {recv}."), "context a.b\n", None);
            for &(member, _) in members {
                assert!(
                    find(&items, member, CompletionKind::Member).is_some(),
                    "{recv}.{member} unreachable: {:?}",
                    items.iter().map(|c| &c.label).collect::<Vec<_>>()
                );
            }
        }
        // The slice-1 additions specifically — guards against a table regression
        // (the loop above can't catch an entry being deleted).
        for (recv, member) in [("List", "empty"), ("Map", "empty"), ("Effect", "pure")] {
            let items = complete(&format!("  {recv}."), "context a.b\n", None);
            assert!(
                find(&items, member, CompletionKind::Member).is_some(),
                "{recv}.{member} missing from the statics table"
            );
        }
    }

    #[test]
    fn record_value_and_decimal_receivers_yield_nothing() {
        // A record type has no name-receiver members (fields are value-receiver).
        let doc = "commons m {\n  type Point = { x: Int }\n}\n";
        assert!(complete("  Point.", doc, None).is_empty(), "record");
        // A lowercase value receiver is deferred to slice 3.
        assert!(complete("  let p = q.", doc, None).is_empty(), "value");
        // A decimal literal is not a member access.
        assert!(complete("  let n = 1.", doc, None).is_empty(), "decimal");
    }

    #[test]
    fn value_receiver_rewrite_drops_the_dot_for_lowercase_receivers() {
        let text = "  let x = email.\n";
        let offset = text.find('.').unwrap() + 1; // just after the dot
        let (rewritten, recv) = value_receiver_rewrite(text, offset).expect("value receiver");
        assert_eq!(
            rewritten, "  let x = email\n",
            "the trailing dot is dropped"
        );
        assert!(
            text.get(recv..=recv).is_some_and(|c| c == "l"),
            "the receiver offset lands inside `email`"
        );
        // A partial member is dropped too.
        let text2 = "  let x = email.ma\n";
        let off2 = text2.find(".ma").unwrap() + 3;
        assert_eq!(
            value_receiver_rewrite(text2, off2).map(|(r, _)| r),
            Some("  let x = email\n".to_string())
        );
        // Uppercase (name receiver, slice 2), decimal, and no-dot yield None.
        assert!(value_receiver_rewrite("  Email.", 8).is_none());
        assert!(value_receiver_rewrite("  let n = 1.", 12).is_none());
        assert!(value_receiver_rewrite("  email", 7).is_none());
    }

    /// One table for every fixture in this module — `Types` is `Send + Sync`
    /// (T3.6b), so a `LazyLock` static gives the `TyId`s below something to
    /// resolve against without threading a value through each helper.
    static TYS: std::sync::LazyLock<Types> = std::sync::LazyLock::new(Types::new);

    /// #2138: `run.` offers `id` in a case body, not once the case binds its
    /// own `run`.
    #[test]
    fn run_member_offers_id_in_a_case_body() {
        let labels = |text: &str| {
            let offset = text.find("run.").expect("run.") + "run.".len();
            let (rewritten, recv) = value_receiver_rewrite(text, offset).expect("a value receiver");
            run_id_member_candidates(&rewritten, recv)
                .into_iter()
                .map(|c| c.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels("suite rid.orders {\n  case \"a\" {\n    expect run. == \"local\"\n  }\n}\n"),
            ["id"]
        );
        assert!(
            labels("suite rid.orders {\n  case \"a\" {\n    let run = 1\n    expect run. == 1\n  }\n}\n")
                .is_empty()
        );
    }

    #[test]
    fn value_member_candidates_lists_kernel_methods() {
        use bynk_syntax::ast::BaseType;
        let list = TYS.intern(Ty::List(TYS.intern(Ty::Base(BaseType::Int))));
        let items = value_member_candidates(list, &TYS, "context a.b\n", None);
        assert!(find(&items, "fold", CompletionKind::Member).is_some());
        assert!(find(&items, "get", CompletionKind::Member).is_some());

        let string = TYS.intern(Ty::Base(BaseType::String));
        let items = value_member_candidates(string, &TYS, "context a.b\n", None);
        assert!(find(&items, "split", CompletionKind::Member).is_some());
        assert!(find(&items, "trim", CompletionKind::Member).is_some());
    }

    #[test]
    fn value_member_candidates_lists_refined_inherited_kernel_methods() {
        // #561: a refined receiver offers its base type's read-only kernel
        // methods in `.`-member completion.
        use bynk_check::checker::NamedKind;
        use bynk_syntax::ast::BaseType;
        let name = TYS.intern(Ty::Named {
            name: "Name".to_string(),
            kind: NamedKind::Refined(BaseType::String),
            args: Vec::new(),
        });
        let items = value_member_candidates(
            name,
            &TYS,
            "commons m {\n  type Name = String where NonEmpty\n}\n",
            None,
        );
        assert!(find(&items, "toUpper", CompletionKind::Member).is_some());
        assert!(find(&items, "length", CompletionKind::Member).is_some());
    }

    #[test]
    fn expression_position_offers_locals() {
        // Value-expecting positions (locals offered).
        assert!(is_expression_position("  let y = "));
        assert!(is_expression_position("  let y = a + lo")); // after a binary op
        assert!(is_expression_position("  f("));
        assert!(is_expression_position("  g(a, "));
        assert!(is_expression_position("  xs.fold(0, (acc, x) => ac")); // lambda body
        // `let y = foo` is still a value position (you're typing the value).
        assert!(is_expression_position("  let y = foo"));
        // Not value positions.
        assert!(!is_expression_position("  let y: ")); // type annotation
        assert!(!is_expression_position("  on call() -> ")); // return type
        assert!(!is_expression_position("  tot")); // bare line start (keyword position covers it)
    }

    #[test]
    fn value_member_candidates_lists_record_fields() {
        use bynk_check::checker::NamedKind;
        let order = TYS.intern(Ty::Named {
            name: "Order".to_string(),
            kind: NamedKind::Record,
            args: Vec::new(),
        });
        let doc = "commons m {\n  type Order = { id: Int, total: Int }\n}\n";
        let items = value_member_candidates(order, &TYS, doc, None);
        assert!(
            find(&items, "id", CompletionKind::Field).is_some(),
            "{items:?}",
            items = items.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
        assert!(find(&items, "total", CompletionKind::Field).is_some());
    }

    #[test]
    fn store_field_member_candidates_offers_entry_ops_and_query_accessors() {
        // #596: a bare `store Map` field receiver offers its entry ops
        // (put/get/…) AND the `.entries`/`.keys`/`.values` accessors — the
        // vocabulary `value_member_candidates` alone can't see, since the
        // checker types a bare store map as plain `Ty::Query` (ADR 0120).
        let doc = "context shop\n\nagent Inventory {\n  key id: String\n  store items: Map[String, Int]\n\n  on call f() -> Effect[()] {\n    items.\n  }\n}\n";
        let offset = doc.find("items.").unwrap() + "items.".len();
        let (rewritten, recv_offset) = value_receiver_rewrite(doc, offset).expect("bare receiver");
        let items = store_field_member_candidates(&rewritten, recv_offset, &[]);
        assert!(
            find(&items, "put", CompletionKind::Member).is_some(),
            "{items:?}",
            items = items.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
        assert!(find(&items, "get", CompletionKind::Member).is_some());
        assert!(find(&items, "update", CompletionKind::Member).is_some());
        assert!(find(&items, "entries", CompletionKind::Field).is_some());
        assert!(find(&items, "keys", CompletionKind::Field).is_some());
        assert!(find(&items, "values", CompletionKind::Field).is_some());
    }

    #[test]
    fn store_field_member_candidates_empty_for_ordinary_local() {
        // An ordinary local named the same as no store field yields nothing —
        // the receiver's provenance, not its name alone, drives this path.
        let doc = "context shop\n\nagent Inventory {\n  key id: String\n\n  on call f() -> Effect[()] {\n    let items = 1\n    items.\n  }\n}\n";
        let offset = doc.rfind("items.").unwrap() + "items.".len();
        let (rewritten, recv_offset) = value_receiver_rewrite(doc, offset).expect("bare receiver");
        let items = store_field_member_candidates(&rewritten, recv_offset, &[]);
        assert!(items.is_empty());
    }

    #[test]
    fn store_field_member_candidates_skips_query_accessors_on_held_map() {
        // ADR 0184: a held `Map[K, Connection]` never offers the key-query
        // accessors, only its entry ops (`put`/`get`/…).
        let doc = "context shop\n\nagent Room {\n  key id: String\n  store conns: Map[String, Connection[String]]\n\n  on call f() -> Effect[()] {\n    conns.\n  }\n}\n";
        let offset = doc.find("conns.").unwrap() + "conns.".len();
        let (rewritten, recv_offset) = value_receiver_rewrite(doc, offset).expect("bare receiver");
        let items = store_field_member_candidates(&rewritten, recv_offset, &[]);
        assert!(find(&items, "put", CompletionKind::Member).is_some());
        assert!(find(&items, "entries", CompletionKind::Field).is_none());
        assert!(find(&items, "keys", CompletionKind::Field).is_none());
        assert!(find(&items, "values", CompletionKind::Field).is_none());
    }

    #[test]
    fn store_field_member_candidates_offers_set_and_cache_vocabularies() {
        // Not every store kind gets the Map-only accessors, but every kind
        // gets its own entry ops.
        let doc = "context shop\n\nagent Inventory {\n  key id: String\n  store tags: Set[String]\n\n  on call f() -> Effect[()] {\n    tags.\n  }\n}\n";
        let offset = doc.find("tags.").unwrap() + "tags.".len();
        let (rewritten, recv_offset) = value_receiver_rewrite(doc, offset).expect("bare receiver");
        let items = store_field_member_candidates(&rewritten, recv_offset, &[]);
        assert!(find(&items, "add", CompletionKind::Member).is_some());
        assert!(find(&items, "entries", CompletionKind::Field).is_none());
    }

    // -- v0.124 (slice 3): the non-keyword completion contexts --

    #[test]
    fn record_construction_offers_field_names() {
        let doc = "commons m {\n  type Order = { id: Int, total: Int }\n}\n";
        let got = labels("  let o = Order { ", doc);
        assert!(got.contains(&"id".to_string()), "{got:?}");
        assert!(got.contains(&"total".to_string()), "{got:?}");
        // After a comma, still field-name position.
        let got2 = labels("  let o = Order { id: 1, ", doc);
        assert!(got2.contains(&"total".to_string()), "{got2:?}");
        // After a `:`, it is a field *type* position, not a field name.
        assert!(record_construction_receiver("  let o = Order { id: ").is_none());
        // A lowercase brace context (a block) is not a construction.
        assert!(record_construction_receiver("  if x { ").is_none());
    }

    #[test]
    fn record_pattern_offers_the_unlisted_fields_then_rest() {
        let doc = "commons m {\n  type Order = { id: Int, total: Int, note: String }\n}\n";
        for prefix in [
            "    Order { ",
            "    Some(Order { ",
            "  if o is (Order { ",
            "    Ok(Order { id: 1, ",
        ] {
            let got = labels(prefix, doc);
            assert!(got.contains(&"..".to_string()), "{prefix}: {got:?}");
            assert!(got.contains(&"total".to_string()), "{prefix}: {got:?}");
        }
        // Already-listed fields are not offered again; a nested comma is not a split.
        let got = labels("    Order { id: Some(1, 2), total, ", doc);
        assert!(!got.contains(&"id".to_string()), "{got:?}");
        assert!(!got.contains(&"total".to_string()), "{got:?}");
        assert!(got.contains(&"note".to_string()), "{got:?}");
        // A construction keeps offering every field, and no `..`.
        let got = labels("  let o = Order { ", doc);
        assert!(
            got.contains(&"id".to_string()) && !got.contains(&"..".to_string()),
            "{got:?}"
        );
        let got = labels("  => Order { id: 1, ", doc);
        assert!(!got.contains(&"..".to_string()), "{got:?}");
    }

    #[test]
    fn from_offers_protocols() {
        let got = labels("  service s from ", "context a.b\n");
        assert!(got.contains(&"http".to_string()), "{got:?}");
        assert!(got.contains(&"cron".to_string()) && got.contains(&"queue".to_string()));
    }

    #[test]
    fn on_offers_handler_kinds() {
        let got = labels("  on ", "context a.b\n");
        assert!(got.contains(&"call".to_string()), "{got:?}");
        assert!(got.contains(&"GET".to_string()) && got.contains(&"schedule".to_string()));
    }

    #[test]
    fn by_offers_project_actors() {
        let doc = "context a.b\n\nactor Caller { auth = Bearer }\n";
        let got = labels("    by ", doc);
        assert!(got.contains(&"Caller".to_string()), "{got:?}");
    }

    #[test]
    fn exports_offers_export_kinds() {
        let got = labels("  exports ", "adapter t {\n  binding \"./b.ts\"\n}\n");
        assert!(got.contains(&"capability".to_string()), "{got:?}");
        assert!(got.contains(&"transparent".to_string()));
    }

    #[test]
    fn provides_offers_in_scope_capabilities() {
        let doc = "context a.b\n\ncapability Store { fn get() -> Effect[Int] }\n";
        let got = labels("  provides ", doc);
        assert!(got.contains(&"Store".to_string()), "{got:?}");
    }

    #[test]
    fn where_offers_predicate_names() {
        // Type-decl refinement.
        let got = labels("  type Code = Int where ", "context a.b\n");
        assert!(got.contains(&"InRange".to_string()), "{got:?}");
        assert!(got.contains(&"NonNegative".to_string()));
        // #472: a match arm's `_ where <predicate>` shares the same catalogue.
        let got = labels("      _ where ", "context a.b\n");
        assert!(got.contains(&"Matches".to_string()), "{got:?}");
        assert!(got.contains(&"NonEmpty".to_string()));
    }

    #[test]
    fn is_for_all_where_matches_only_the_for_all_binder() {
        assert!(is_for_all_where("\tfor all x: Int, y: Int where "));
        assert!(is_for_all_where("for all x: Int where "));
        // `for`/`all` must be standalone words, not prefixes of longer
        // identifiers.
        assert!(!is_for_all_where("format(x) where "));
        assert!(!is_for_all_where("for allocate x where "));
        // A type-decl or match-arm `where` isn't a `for all` binder.
        assert!(!is_for_all_where("  type Code = Int where "));
        assert!(!is_for_all_where("      _ where "));
        assert!(!is_for_all_where("no where here"));
    }

    #[test]
    fn for_all_where_falls_through_to_expression_position_not_predicates() {
        // #472 (finding 2, PR #827 review): a `for all` binder's `where`
        // clause takes an arbitrary `Bool`, not the predicate catalogue —
        // it must not offer `InRange`/`NonNegative`/etc.
        let got = labels("\tfor all x: Int where ", "context a.b\n");
        assert!(!got.contains(&"InRange".to_string()), "{got:?}");
        assert!(!got.contains(&"NonNegative".to_string()), "{got:?}");
    }

    #[test]
    fn clause_detectors_do_not_over_fire() {
        // `on` inside a larger word, and a field named `from`, must not trigger.
        assert!(!after_clause_keyword("  session ", "on"));
        assert!(!after_clause_keyword("  let from = ", "from"));
        // A standalone keyword does.
        assert!(after_clause_keyword("  service s from ", "from"));
        assert!(after_clause_keyword("    by ", "by"));
    }

    #[test]
    fn contract_clause_kind_detects_requires_and_ensures() {
        assert_eq!(contract_clause_kind("  requires positive: "), Some(false));
        assert_eq!(contract_clause_kind("  ensures never_neg: "), Some(true));
        // Not a contract clause.
        assert_eq!(contract_clause_kind("  id: Int"), None);
        assert_eq!(contract_clause_kind("  let x = 1"), None);
    }

    #[test]
    fn cors_field_position_inside_cors_block() {
        // Cursor at a fresh field-name line inside a `cors { }` block.
        let doc = "service api from http {\n  cors {\n    ";
        assert!(in_cors_field_position(doc, doc.len()));
        // After a `:` it is a value position, not a field-name position.
        let doc2 = "service api from http {\n  cors {\n    origins: ";
        assert!(!in_cors_field_position(doc2, doc2.len()));
        // Not inside a `cors` block (an ordinary record construction) — no.
        let doc3 = "let x = Order {\n    ";
        assert!(!in_cors_field_position(doc3, doc3.len()));
    }

    /// #2640: inside `par { }` only the two statement forms are offered, and the
    /// `par` snippet appears only where a body is effectful.
    #[test]
    fn par_block_positions_and_candidates() {
        let inside = "service s {\n  on call() -> Effect[Int] {\n    par {\n      ";
        assert!(in_par_block_position(inside, inside.len()));
        let labels: Vec<String> = par_block_candidates()
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(labels, ["let x <-", "do"]);
        // After the closing brace the statement is an ordinary one again.
        let after =
            "service s {\n  on call() -> Effect[Int] {\n    par {\n      let a <- f()\n    }\n    ";
        assert!(!in_par_block_position(after, after.len()));
        assert!(in_effectful_body_position(after, after.len()));
    }

    #[test]
    fn par_snippet_is_for_effectful_bodies_only() {
        let effectful = "fn f() -> Effect[Int] {\n  if true {\n    ";
        assert!(in_effectful_body_position(effectful, effectful.len()));
        let pure = "fn f() -> Int {\n  ";
        assert!(!in_effectful_body_position(pure, pure.len()));
        let top = "context c\n\n";
        assert!(!in_effectful_body_position(top, top.len()));
        assert!(par_snippet().insert_text.unwrap().starts_with("par {"));
    }

    #[test]
    fn security_field_position_inside_security_block() {
        // Cursor at a fresh field-name line inside a `security { }` block.
        let doc = "service api from http {\n  security {\n    ";
        assert!(in_security_field_position(doc, doc.len()));
        // After a `:` it is a value position, not a field-name position.
        let doc2 = "service api from http {\n  security {\n    hsts: ";
        assert!(!in_security_field_position(doc2, doc2.len()));
        // A `cors { }` block is not a `security` block.
        let doc3 = "service api from http {\n  cors {\n    ";
        assert!(!in_security_field_position(doc3, doc3.len()));
    }

    /// Non-ASCII text inside the block (a `-- café` comment) must not break
    /// the field-position probes; a mid-codepoint offset degrades to `false`
    /// instead of panicking the request handler.
    #[test]
    fn field_position_probes_survive_non_ascii() {
        let doc = "service api from http {\n  cors { // café\n    ";
        assert!(in_cors_field_position(doc, doc.len()));
        // A raw byte offset that lands inside the `é` must not panic.
        let mid = doc.find('é').unwrap() + 1;
        assert!(!doc.is_char_boundary(mid));
        let _ = in_cors_field_position(doc, mid);
        let _ = in_security_field_position(doc, mid);
        let _ = in_limits_field_position(doc, mid);
    }

    #[test]
    fn by_in_a_suite_offers_the_reserved_principals() {
        // #2135: `Nobody`, `Forged` and `Stale` after `by` in a suite only.
        let suite = "suite pay.hooks as system {\n  case \"c\" {\n    let r <- webhooks.POST(\"/h\", \"x\") by ";
        let labels: Vec<String> =
            complete("    let r <- webhooks.POST(\"/h\", \"x\") by ", suite, None)
                .into_iter()
                .map(|c| c.label)
                .collect();
        for name in ["Nobody", "Forged", "Stale"] {
            assert!(labels.iter().any(|l| l == name), "{name}: {labels:?}");
        }
        let context = "context pay.hooks\n\nservice api from http {\n  on GET(\"/a\") () -> Effect[HttpResult[String]] by ";
        let labels: Vec<String> = complete(
            "  on GET(\"/a\") () -> Effect[HttpResult[String]] by ",
            context,
            None,
        )
        .into_iter()
        .map(|c| c.label)
        .collect();
        assert!(!labels.iter().any(|l| l == "Forged"), "{labels:?}");
    }

    #[test]
    fn env_name_position_and_candidates() {
        // #2134: a fresh entry line inside a suite's `env { }` block.
        let doc = "suite shop.pay {\n  env {\n    ";
        assert!(in_env_name_position(doc, doc.len()));
        // After `=` it is a value position.
        let doc2 = "suite shop.pay {\n  env {\n    A = ";
        assert!(!in_env_name_position(doc2, doc2.len()));
        // A case body is not an `env` block.
        let doc3 = "suite shop.pay {\n  case \"c\" {\n    ";
        assert!(!in_env_name_position(doc3, doc3.len()));
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("src/pay.bynk"),
            "actor User { auth = Bearer(secret = \"AUTH_JWT_SECRET\"), identity = String }\n\
             let p <- Secrets.get(\"PAYMENTS_PROCESSOR\")\n"
                .to_string(),
        );
        let labels: Vec<String> = env_name_candidates(doc, Some(&files))
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(labels, ["AUTH_JWT_SECRET", "PAYMENTS_PROCESSOR"]);
    }

    #[test]
    fn limits_field_position_inside_limits_block() {
        // Cursor at a fresh field-name line inside a `limits { }` block.
        let doc = "service api from http {\n  limits {\n    ";
        assert!(in_limits_field_position(doc, doc.len()));
        // After a `:` it is a value position, not a field-name position.
        let doc2 = "service api from http {\n  limits {\n    maxBody: ";
        assert!(!in_limits_field_position(doc2, doc2.len()));
        // A `security { }` block is not a `limits` block.
        let doc3 = "service api from http {\n  security {\n    ";
        assert!(!in_limits_field_position(doc3, doc3.len()));
    }

    #[test]
    fn service_body_item_position_offers_cors() {
        // A bare-word item start inside a `service … {` block.
        let doc = "service api from http {\n  ";
        assert!(in_service_body_item_position(doc, doc.len(), "  "));
        // Inside a nested `cors { }` block the innermost brace is not the
        // service brace, so the service-item cell does not fire.
        let doc2 = "service api from http {\n  cors {\n    ";
        assert!(!in_service_body_item_position(doc2, doc2.len(), "    "));
    }

    #[test]
    fn cache_arg_position_inside_cache_annotation() {
        // Cursor at a fresh argument-name position inside `@cache( … )`.
        let doc = "service api from http {\n  @cache(";
        assert!(in_cache_arg_position(doc, doc.len()));
        // After a first arg + comma, still an argument-name position.
        let doc2 = "service api from http {\n  @cache(maxAge: 5.minutes, ";
        assert!(in_cache_arg_position(doc2, doc2.len()));
        // After a `:` it is a value position, not an argument-name position.
        let doc3 = "service api from http {\n  @cache(maxAge: ";
        assert!(!in_cache_arg_position(doc3, doc3.len()));
        // A bare `cache(` call (no `@`) is not the annotation.
        let doc4 = "let x = cache(";
        assert!(!in_cache_arg_position(doc4, doc4.len()));
        // An ordinary handler param list is not a `@cache` position.
        let doc5 = "on GET(\"/x\") by v: Visitor (";
        assert!(!in_cache_arg_position(doc5, doc5.len()));
    }

    #[test]
    fn a_capability_declaration_completes_its_base_then_its_options() {
        // P3 (#2342): the instantiable kinds after `=`, narrowed as typed.
        let bases: Vec<String> = instance_declaration_completions("capability LoginLimit = ")
            .into_iter()
            .map(|(label, ..)| label)
            .collect();
        assert_eq!(
            bases,
            [
                "Kv",
                "Service",
                "R2",
                "RateLimit",
                "Queue",
                "Analytics",
                "D1",
                "EdgeCache"
            ]
        );
        let narrowed: Vec<String> = instance_declaration_completions("capability L = Ra")
            .into_iter()
            .map(|(label, ..)| label)
            .collect();
        assert_eq!(narrowed, ["RateLimit"]);
        // Then the required options, as one snippet.
        let options = instance_declaration_completions("capability LoginLimit = RateLimit ");
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].0, "@limit(…) @period(…)");
        assert_eq!(options[0].2, "@limit(${1:100}) @period(${2:60.seconds})");
        // P2 (#2578): a `Queue` names its queue.
        let options = instance_declaration_completions("capability OrderEvents = Queue ");
        assert_eq!(options[0].0, "@name(…)");
        assert_eq!(options[0].2, "@name(${1:\"order-events\"})");
        // A kind with no required option, or a line already carrying them,
        // gets nothing; nor does any other line.
        assert!(instance_declaration_completions("capability A = R2 ").is_empty());
        assert!(instance_declaration_completions("capability Edge = EdgeCache ").is_empty());
        assert!(instance_declaration_completions("capability L = RateLimit @limit(1) ").is_empty());
        assert!(instance_declaration_completions("let x = R").is_empty());
    }

    #[test]
    fn a_rate_limit_instance_option_is_not_the_limit_annotation() {
        // P3 (#2342): `@limit(` on a capability declaration is an instance option.
        let doc = "context a\ncapability LoginLimit = RateLimit @limit(";
        assert!(!in_limit_arg_position(doc, doc.len()));
        // `@period(` offers the platform's two windows, and only at its value.
        let doc = "context a\ncapability LoginLimit = RateLimit @limit(100) @period(";
        let offered: Vec<&str> = instance_option_values(doc, doc.len())
            .iter()
            .map(|(v, _)| *v)
            .collect();
        assert_eq!(offered, ["10.seconds", "60.seconds"]);
        assert!(instance_option_values("let x = period(", 15).is_empty());
        let doc = "service api from http {\n  @period(";
        assert!(instance_option_values(doc, doc.len()).is_empty());
    }

    #[test]
    fn limit_arg_position_inside_limit_annotation() {
        // Cursor at a fresh argument-name position inside `@limit( … )`.
        let doc = "service api from http {\n  @limit(";
        assert!(in_limit_arg_position(doc, doc.len()));
        // After a first arg + comma, still an argument-name position.
        let doc2 = "service api from http {\n  @limit(maxBody: 1048576, ";
        assert!(in_limit_arg_position(doc2, doc2.len()));
        // After a `:` it is a value position, not an argument-name position.
        let doc3 = "service api from http {\n  @limit(maxBody: ";
        assert!(!in_limit_arg_position(doc3, doc3.len()));
        // A bare `limit(` call (no `@`) is not the annotation.
        let doc4 = "let x = limit(";
        assert!(!in_limit_arg_position(doc4, doc4.len()));
        // An ordinary handler param list is not a `@limit` position.
        let doc5 = "on GET(\"/x\") by v: Visitor (";
        assert!(!in_limit_arg_position(doc5, doc5.len()));
    }

    #[test]
    fn raw_arg_position_inside_raw_annotation() {
        let doc = "service api from http {\n  @raw(";
        assert!(in_raw_arg_position(doc, doc.len()));
        let doc2 = "service api from http {\n  @raw(contentType: ";
        assert!(!in_raw_arg_position(doc2, doc2.len()));
        let doc3 = "let x = raw(";
        assert!(!in_raw_arg_position(doc3, doc3.len()));
        let doc4 = "service api from http {\n  @limit(";
        assert!(!in_raw_arg_position(doc4, doc4.len()));
    }

    #[test]
    fn sum_type_variants_lists_variants() {
        let doc = "commons m {\n  type Status = enum { Pending, Shipped }\n}\n";
        let got: Vec<String> = sum_type_variants("Status", doc, None)
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert!(got.contains(&"Pending".to_string()), "{got:?}");
        assert!(got.contains(&"Shipped".to_string()), "{got:?}");
    }

    /// #1888: a field's and a variant's doc ride on their completion items;
    /// an undocumented one has none.
    /// Review of #1968: completion reads unchecked source, so a derivation
    /// cycle completes nothing rather than recursing without end.
    #[test]
    fn a_derivation_cycle_completes_nothing() {
        let doc = "context a\n\ncapability A = B\ncapability B = C\ncapability C = A\n";
        assert!(member_candidates("A", doc, None).is_empty());
    }

    /// S4a (#1966): a derived capability completes its base's operations.
    #[test]
    fn a_derived_capability_completes_its_bases_operations() {
        const DERIVED_DOC: &str =
            "context a\n\nconsumes bynk.cloudflare { Kv }\n\ncapability Sessions = Kv\n";
        let labels: Vec<String> = member_candidates("Sessions", DERIVED_DOC, None)
            .into_iter()
            .map(|c| c.label)
            .collect();
        for op in ["get", "put", "putTtl", "delete", "list"] {
            assert!(labels.iter().any(|l| l == op), "{op} missing: {labels:?}");
        }
    }

    #[test]
    fn member_completions_carry_their_docs() {
        let doc = "commons m\n\ntype Rec = {\n  --- The id.\n  id: Int,\n  n: Int,\n}\n\ntype Status = enum {\n  ---\n  Waiting.\n  ---\n  Pending,\n  Shipped,\n}\n";
        let docs = |items: Vec<Completion>| -> Vec<(String, Option<String>)> {
            items
                .into_iter()
                .map(|c| (c.label, c.documentation.map(|d| d.trim_end().to_string())))
                .collect()
        };
        assert_eq!(
            docs(record_field_names("Rec", doc, None)),
            [("id".into(), Some("The id.".into())), ("n".into(), None)]
        );
        assert_eq!(
            docs(sum_type_variants("Status", doc, None)),
            [
                ("Pending".into(), Some("Waiting.".into())),
                ("Shipped".into(), None)
            ]
        );
        assert_eq!(
            docs(member_candidates("Status", doc, None))
                .into_iter()
                .filter(|(l, _)| l == "Pending")
                .collect::<Vec<_>>(),
            [("Pending".into(), Some("Waiting.".into()))]
        );
    }

    #[test]
    fn variants_for_ty_offers_built_in_result_and_option() {
        // v0.145 (ADR 0169, base gap): a `Result`/`Option` scrutinee offers its
        // built-in variants, which are not declared types (`sum_type_variants`
        // can't see them). This is what makes match-arm completion fire for a
        // Result/Option scrutinee at all.
        use bynk_syntax::ast::BaseType;
        let result = TYS.intern(Ty::Result(
            TYS.intern(Ty::Base(BaseType::Int)),
            TYS.intern(Ty::Base(BaseType::String)),
        ));
        let got: Vec<String> = variants_for_ty(result, &TYS, "", None)
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(got, vec!["Ok".to_string(), "Err".to_string()]);

        let option = TYS.intern(Ty::Option(TYS.intern(Ty::Base(BaseType::Int))));
        let got: Vec<String> = variants_for_ty(option, &TYS, "", None)
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(got, vec!["Some".to_string(), "None".to_string()]);
    }

    #[test]
    fn nested_variant_completions_resolves_the_payload_type() {
        use bynk_check::checker::NamedKind;
        use bynk_syntax::ast::BaseType;
        // Built-in outer: `Some(‸)` on `Option[Result[Int, E]]` offers the
        // payload `Result`'s variants.
        let payload = TYS.intern(Ty::Result(
            TYS.intern(Ty::Base(BaseType::Int)),
            TYS.intern(Ty::Named {
                name: "E".to_string(),
                kind: NamedKind::Sum,
                args: Vec::new(),
            }),
        ));
        let scrut = TYS.intern(Ty::Option(payload));
        let got: Vec<String> = nested_variant_completions(scrut, &TYS, "Some", "", None)
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(got, vec!["Ok".to_string(), "Err".to_string()]);

        // The other outer variant of a Result payload resolves the error arm.
        let opt_int = TYS.intern(Ty::Option(TYS.intern(Ty::Base(BaseType::Int))));
        let inner = TYS.intern(Ty::Result(TYS.intern(Ty::Base(BaseType::Int)), opt_int));
        let got: Vec<String> = nested_variant_completions(inner, &TYS, "Err", "", None)
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(got, vec!["Some".to_string(), "None".to_string()]);

        // User-sum outer: `Wrap(‸)` on a declared sum offers the payload field
        // type's variants, walked from source.
        let doc = "commons m {\n  type Inner = enum { A, B }\n  \
                   type Outer = | Wrap(inner: Inner) | Bare\n}\n";
        let outer = TYS.intern(Ty::Named {
            name: "Outer".to_string(),
            kind: NamedKind::Sum,
            args: Vec::new(),
        });
        let got: Vec<String> = nested_variant_completions(outer, &TYS, "Wrap", doc, None)
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert!(got.contains(&"A".to_string()), "{got:?}");
        assert!(got.contains(&"B".to_string()), "{got:?}");
    }

    #[test]
    fn literal_kind_scrutinee_suggests_no_variants() {
        // v0.130: a literal-kind `match` (a primitive `Int`/`String`/`Bool`
        // scrutinee, or a refinement over one) has no variant names to offer.
        // A primitive types to `Ty::Base`, which never reaches this lookup; a
        // refined scrutinee types to `Ty::Named` but has no `enum` body, so the
        // variant candidate set is empty — no bogus completions.
        let doc = "commons m {\n  type Quantity = Int where InRange(1, 99)\n}\n";
        assert!(sum_type_variants("Quantity", doc, None).is_empty());
        assert!(sum_type_variants("Int", doc, None).is_empty());
    }

    // -- parse cache (#733) --

    /// Collect the enumerated unit names for a buffer + project file set.
    fn enumerated_units(doc_text: &str, files: Option<&HashMap<PathBuf, String>>) -> Vec<String> {
        let mut names = Vec::new();
        for_each_unit(doc_text, files, |u| names.push(u.name().joined()));
        names
    }

    /// Content-ownership track (#1086) slice 0+1: `files` is now a pre-read
    /// `(path, content)` map — the caller's job (`bynk-lsp`'s overlay-then-disk
    /// sweep) — rather than a bare path list `cached_project_unit` used to
    /// read from disk itself. These tests build that map directly; none of
    /// them touch the real filesystem any more.
    fn synthetic_path(name: &str) -> PathBuf {
        PathBuf::from(format!("/synthetic/{name}.bynk"))
    }

    #[test]
    fn for_each_unit_yields_embedded_buffer_and_project_files() {
        let sibling = synthetic_path("sibling");
        let files = HashMap::from([(
            sibling,
            "commons proj.sibling {\n  fn s() -> Int { 1 }\n}\n".to_string(),
        )]);

        let names = enumerated_units(
            "commons proj.buffer {\n  fn b() -> Int { 1 }\n}\n",
            Some(&files),
        );
        // The embedded `bynk` surface, the live buffer, and the project file all show.
        assert!(names.iter().any(|n| n == "bynk"), "embedded: {names:?}");
        assert!(
            names.iter().any(|n| n == "proj.buffer"),
            "buffer: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "proj.sibling"),
            "project file: {names:?}"
        );
    }

    /// Finding #62: when the buffer and a project file both declare a type
    /// under the same name — the buffer's stale copy, or two distinct files
    /// that happen to collide — `record_field_names` and `sum_type_variants`
    /// must take the first match (the buffer, since `for_each_unit` yields it
    /// before `files`) rather than unioning both, so a field/variant removed
    /// from the live buffer does not resurface from the stale copy.
    #[test]
    fn same_named_declarations_do_not_union_across_units() {
        let stale = HashMap::from([(
            synthetic_path("stale"),
            "commons m {\n  \
             type Rec = { total: Int, old_field: Int }\n  \
             type Status = enum { Pending, Retired }\n\
             }\n"
            .to_string(),
        )]);

        let buffer = "commons m {\n  \
                       type Rec = { total: Int }\n  \
                       type Status = enum { Pending, Shipped }\n\
                       }\n";
        let files = Some(&stale);

        let fields: Vec<String> = record_field_names("Rec", buffer, files)
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(fields, vec!["total".to_string()], "{fields:?}");

        let variants: Vec<String> = sum_type_variants("Status", buffer, files)
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(
            variants,
            vec!["Pending".to_string(), "Shipped".to_string()],
            "{variants:?}"
        );
    }

    /// Finding #62: `capabilities_of_unit` takes the first unit named `unit`
    /// rather than unioning exports across every matching unit — the same
    /// buffer-vs-stale-copy reasoning as
    /// `same_named_declarations_do_not_union_across_units`.
    #[test]
    fn capabilities_of_unit_does_not_union_across_units() {
        let stale = HashMap::from([(
            synthetic_path("stale"),
            "adapter tokens {\n  \
             exports capability { Jwt, Retired }\n\
             }\n"
            .to_string(),
        )]);

        let buffer = "adapter tokens {\n  exports capability { Jwt }\n}\n";
        let files = Some(&stale);

        let caps = capabilities_of_unit("tokens", buffer, &split_dependency_files(files, None));
        assert_eq!(caps, vec!["Jwt".to_string()], "{caps:?}");
    }

    #[test]
    fn project_unit_cache_invalidates_on_change() {
        let path = synthetic_path("unit");

        let first_files = HashMap::from([(
            path.clone(),
            "commons proj.first {\n  fn a() -> Int { 1 }\n}\n".to_string(),
        )]);
        let first = enumerated_units("context a.b\n", Some(&first_files));
        assert!(
            first.iter().any(|n| n == "proj.first"),
            "first read: {first:?}"
        );

        // Same path, different content — the cache is keyed on content
        // equality (not disk metadata, since content is supplied directly
        // and an open-buffer overlay has no meaningful mtime at all), so it
        // must serve the new parse, not the stale one.
        let second_files = HashMap::from([(
            path,
            "commons proj.second.longer {\n  fn a() -> Int { 1 }\n  fn c() -> Int { 3 }\n}\n"
                .to_string(),
        )]);
        let second = enumerated_units("context a.b\n", Some(&second_files));
        assert!(
            second.iter().any(|n| n == "proj.second.longer"),
            "after change: {second:?}"
        );
        assert!(
            !second.iter().any(|n| n == "proj.first"),
            "stale served: {second:?}"
        );
    }

    #[test]
    fn unparseable_project_file_is_skipped() {
        // Content-ownership track (#1086) slice 0+1: `cached_project_unit` no
        // longer reads disk itself, so there is no more "path doesn't exist on
        // disk" case to skip — the equivalent today is content that fails to
        // parse (the caller's sweep omits an unreadable file from the map
        // entirely, so this covers the other unusable case: present but not
        // parseable).
        let files = HashMap::from([(synthetic_path("broken"), "not bynk source {{{".to_string())]);
        let names = enumerated_units("context a.b\n", Some(&files));
        let baseline = enumerated_units("context a.b\n", None);
        // No panic, and the broken file contributes nothing beyond the same
        // embedded surface + buffer a project with no files at all yields —
        // not just "the embedded surface is present somewhere", which would
        // pass even if a future recovery-parse change made the broken
        // content yield a spurious unit alongside it.
        assert_eq!(names, baseline, "{names:?}");
    }
}
