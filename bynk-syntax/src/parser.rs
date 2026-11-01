//! Hand-written recursive-descent parser for Bynk v0.
//!
//! Token grammar in spec §4. The expression parser uses one function per
//! precedence level (§4.4). Errors carry spans and short fix-oriented
//! messages; the parser does not currently attempt synchronisation, which
//! means at most one parse error is reported per compilation.

use crate::ast::*;
use crate::error::CompileError;
use crate::lexer::{
    Token, TokenKind, comment_body, comment_is_legacy, doc_block_content, doc_fence_len,
    doc_line_is_legacy, doc_line_text, has_blank_line_between, legacy_comment_error,
    legacy_doc_line_error,
};
use crate::span::Span;
mod declarations;
mod expressions;
mod statements;
mod types;

/// Side-channel store for line-comment trivia (v1.1 LSP spec §3.5).
///
/// Built once up-front by [`split_trivia`] from the raw lexer token stream.
/// Comments are removed from the token stream the parser walks; their text
/// is filed into `leading` (comments on lines preceding a content token)
/// and `trailing` (a single comment on the same line as a content token).
/// The parser consumes entries through [`TriviaTable::take_leading`] and
/// [`TriviaTable::take_trailing`] as it recognises declarations.
#[derive(Debug, Default)]
struct TriviaTable {
    /// `leading[i]` holds the comments that appear immediately before content
    /// token `i` (zero or more `--` lines, in source order, not separated from
    /// the token by another content token). A comment on its own line is
    /// followed by a [`Comment::Blank`] when a blank line comes after it
    /// (#1884).
    leading: Vec<Vec<Comment>>,
    /// `trailing[i]` holds an optional comment on the same source line as
    /// content token `i`. Only one trailing comment is recorded per token
    /// because a single `--` consumes the rest of the line. A doc block or an
    /// opening `{` never has one: see [`split_trivia`].
    trailing: Vec<Option<String>>,
    /// Any pending leading comments at end-of-file (no content token
    /// followed), shaped as a `leading` entry. Used to preserve file-trailing
    /// comments.
    epilogue: Vec<Comment>,
    /// #2203: a `bynk.lex.legacy_comment` or `bynk.lex.legacy_doc_line` error
    /// for each comment or doc line spelled the legacy way. The tokens still
    /// stand as trivia and docs, so the parse goes on and reports them all;
    /// [`Parser::new`] files them with the doc errors, and the formatter's
    /// migrating parse ([`parse_units_migrating`]) drops them.
    legacy: Vec<CompileError>,
}

impl TriviaTable {
    fn take_leading(&mut self, index: usize) -> Vec<Comment> {
        match self.leading.get_mut(index) {
            Some(v) => std::mem::take(v),
            None => Vec::new(),
        }
    }

    fn take_trailing(&mut self, index: usize) -> Option<String> {
        self.trailing.get_mut(index).and_then(|s| s.take())
    }

    fn take_epilogue(&mut self) -> Vec<Comment> {
        std::mem::take(&mut self.epilogue)
    }

    /// True when every entry has been drained via `take_leading`/
    /// `take_trailing`/`take_epilogue` — i.e. no comment was silently
    /// dropped. Each harvest is already a `mem::take`, so anything still
    /// present here is exactly the set of comments that never reached an
    /// AST `Trivia` field.
    ///
    /// Deliberately **not** wired into a `debug_assert!` in the general parse
    /// path: expressions carry no per-node trivia (§ "Comment trivia" in the
    /// 2026-07-27 pipeline review), so an ordinary, valid program with a
    /// comment inside a `match`/list/record/binop — a common, accepted
    /// pattern `bynk-fmt`'s own comment-loss guard already handles
    /// gracefully — would leave `leading`/`trailing` non-empty and trip it on
    /// every compile, not just on formatting. Instead surfaced through
    /// [`parse_units_with_drain_check`] (finding #66), whose one caller
    /// (`bynk-fmt`) *does* care about exactly this signal.
    /// [`Self::epilogue_is_empty`] is the narrower, safe-to-assert check.
    fn is_fully_drained(&self) -> bool {
        self.leading.iter().all(Vec::is_empty)
            && self.trailing.iter().all(Option::is_none)
            && self.epilogue.is_empty()
    }

    /// True when no file-trailing comment was left stranded. Unlike
    /// [`Self::is_fully_drained`], this is safe to assert unconditionally: a
    /// clean file's epilogue is empty by construction (nothing pending at
    /// EOF), and the one shape that legitimately populates it — a top-level
    /// trailing comment — is drained by every parse path that calls
    /// `take_epilogue`. A brace-form declaration that forgets to is exactly
    /// the bug this catches.
    fn epilogue_is_empty(&self) -> bool {
        self.epilogue.is_empty()
    }
}

/// Remove `Comment` trivia tokens from `tokens` and bin them into a
/// [`TriviaTable`] keyed against the surviving content tokens. A comment
/// on the same source line as the preceding content token is recorded as
/// that token's *trailing* trivia, unless that token is a doc block or an
/// opening `{`; everything else is *leading* for the next content token.
///
/// A blank line after a leading comment on its own line, before the next
/// comment or content token, is recorded as a [`Comment::Blank`] (#1884). Only
/// a declaration that canonical style sets apart with blank lines keeps it: see
/// [`Parser::take_leading_trivia`].
fn split_trivia(tokens: &[Token], source: &str) -> (Vec<Token>, TriviaTable) {
    let mut filtered: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut table = TriviaTable::default();
    let mut pending_leading: Vec<Comment> = Vec::new();
    let mut last_content_end: Option<usize> = None;
    // Where the last comment in `pending_leading` ends, if it has a line to
    // itself: the gap from there to the next token says whether a blank line
    // follows it. A comment's span stops before its newline, so a blank line
    // is two newlines. A comment that leads only because it sits on a `{` line
    // has no blank line of its own: the one below it was between the `{` and
    // the first item.
    let mut last_leading_end: Option<usize> = None;
    for tok in tokens {
        if !pending_leading.is_empty()
            && let Some(end) = last_leading_end
            && source[end..tok.span.start].matches('\n').count() >= 2
        {
            pending_leading.push(Comment::Blank);
        }
        if tok.kind == TokenKind::DocLine && doc_line_is_legacy(source, tok.span) {
            table.legacy.push(legacy_doc_line_error(source, tok.span));
        }
        if tok.kind == TokenKind::Comment {
            if comment_is_legacy(source, tok.span) {
                table.legacy.push(legacy_comment_error(tok.span));
            }
            let body = comment_body(source, tok.span).into_owned();
            // If nothing has been buffered as leading for the next token and
            // there is no newline between the previous content token and
            // this comment, it trails that token. A doc block never has a
            // trailing comment: its token ends past the newline after the
            // closing `---`, so a `--` line directly under it would otherwise
            // look same-line and be binned where nothing collects it (#1756).
            // A `--|` doc line (#1888) ends past its newline the same way.
            // Nor does an opening `{`: no parser collects trivia right after
            // one, so its comment leads whatever follows instead, the first
            // item or the `}` of an empty body (#1788).
            if pending_leading.is_empty()
                && filtered.last().is_some_and(|t| {
                    !matches!(
                        t.kind,
                        TokenKind::DocBlock | TokenKind::DocLine | TokenKind::LBrace
                    )
                })
                && let Some(prev_end) = last_content_end
                && !source[prev_end..tok.span.start].contains('\n')
            {
                let last_idx = filtered.len() - 1;
                // Only attach if no trailing already recorded (shouldn't
                // happen because `--` consumes through end-of-line).
                if table.trailing[last_idx].is_none() {
                    table.trailing[last_idx] = Some(body);
                    continue;
                }
            }
            // A doc block's token ends past its closing line's newline, so a
            // comment under it is on its own line with no newline between. A
            // `--|` doc line's token does too (#1888).
            let own_line = !pending_leading.is_empty()
                || filtered
                    .last()
                    .is_none_or(|t| matches!(t.kind, TokenKind::DocBlock | TokenKind::DocLine))
                || last_content_end.is_some_and(|end| source[end..tok.span.start].contains('\n'));
            pending_leading.push(Comment::Line(body));
            last_leading_end = own_line.then_some(tok.span.end);
            continue;
        }
        filtered.push(*tok);
        table.leading.push(std::mem::take(&mut pending_leading));
        table.trailing.push(None);
        last_content_end = Some(tok.span.end);
        last_leading_end = None;
    }
    table.epilogue = pending_leading;
    (filtered, table)
}

/// Parse a token slice into a [`Commons`] AST.
///
/// Accepts either form of v0.3 commons file:
/// - Brace form: `commons name { items... }` (v0–v0.2 compatible).
/// - Fragment form: `commons name uses... items...` to EOF (v0.3).
pub fn parse(tokens: &[Token], source: &str) -> Result<Commons, Vec<CompileError>> {
    parse_with_warnings(tokens, source).map(|(c, _warnings)| c)
}

/// [`parse`] with the non-fatal diagnostics threaded out alongside the AST
/// (ADR 0117) — see [`parse_units_with_warnings`].
pub fn parse_with_warnings(
    tokens: &[Token],
    source: &str,
) -> Result<(Commons, Vec<CompileError>), Vec<CompileError>> {
    let (unit, warnings) = parse_unit_with_warnings(tokens, source)?;
    match unit {
        SourceUnit::Commons(c) => Ok((c, warnings)),
        SourceUnit::Context(ctx) => Err(vec![
            CompileError::new(
                "bynk.parse.unexpected_context",
                ctx.span,
                "expected a `commons` declaration but found a `context` declaration",
            )
            .with_note(
                "contexts must be compiled as part of a project — pass the source directory, e.g. `bynkc compile --target bundle --output out src`",
            ),
        ]),
        SourceUnit::Suite(t) => Err(vec![
            CompileError::new(
                "bynk.parse.unexpected_suite",
                t.span,
                "expected a `commons` declaration but found a `suite` declaration",
            )
            .with_note(
                "tests must be compiled as part of a project — pass the source directory, e.g. `bynkc compile --target bundle --output out src`",
            ),
        ]),
        SourceUnit::Adapter(a) => Err(vec![
            CompileError::new(
                "bynk.parse.unexpected_adapter",
                a.span,
                "expected a `commons` declaration but found an `adapter` declaration",
            )
            .with_note(
                "adapters must be compiled as part of a project — pass the source directory, e.g. `bynkc compile --target bundle --output out src`",
            ),
        ]),
    }
}

/// Parse a token slice into a [`SourceUnit`] with error recovery, returning a
/// best-effort partial AST plus the full list of parse errors and warnings.
///
/// Used by the LSP: item-level recovery skips past a malformed declaration to
/// the next top-level item, so multiple errors are reported per compilation
/// rather than just the first. Compared to [`parse_unit`], this never bails;
/// if no SourceUnit could be parsed at all (e.g. the file is empty or the
/// header itself fails) the returned `Option` is `None`.
///
/// Keeps only the *first* unit — v0.113 allows more than one top-level unit
/// per file (an atomic `commons` + `suite`, DECISION S), and every existing
/// caller here is keyed on the primary declaration. [`parse_units_with_recovery`]
/// is the same recovery parse without that narrowing, for the one caller
/// (finding #29/#30) that needs every unit a file declares.
pub fn parse_unit_with_recovery(
    tokens: &[Token],
    source: &str,
) -> (Option<SourceUnit>, Vec<CompileError>) {
    let (units, errors) = parse_units_with_recovery(tokens, source);
    (units.into_iter().next(), errors)
}

/// [`parse_unit_with_recovery`], keeping **every** top-level unit instead of
/// discarding all but the first (finding #29/#30). Used by the IDE's own parse
/// entry point, which needs to see a trailing `suite` in an atomic
/// `commons`+`suite` file, not just the primary declaration.
pub fn parse_units_with_recovery(
    tokens: &[Token],
    source: &str,
) -> (Vec<SourceUnit>, Vec<CompileError>) {
    let recovered = parse_units_recovering(tokens, source);
    (recovered.units, recovered.errors)
}

/// #1663: everything a recovering parse learns — the units it built, every
/// syntax error, and the names of the top-level declarations it had to skip.
#[derive(Debug)]
pub struct Recovered {
    pub units: Vec<SourceUnit>,
    pub errors: Vec<CompileError>,
    /// Declarations recovery dropped, by name (see `Parser::broken_decl_names`).
    /// A caller keeps references to them from echoing as unknown names.
    pub broken_decl_names: Vec<String>,
}

/// #1663: a strict parse's error(s) together with a recovering parse's, each
/// reported once. The strict errors come first and always survive: some rules
/// hold only for the strict single-unit parse (`extra_tokens`, a `commons`
/// expected but a `context` found), which the multi-unit recovering parse
/// accepts. A recovered error at the same position as one already listed is
/// the same fault (the two parsers may name it differently) and is dropped.
pub fn merge_syntax_errors(
    strict: Vec<CompileError>,
    recovered: Vec<CompileError>,
) -> Vec<CompileError> {
    let mut out = strict;
    for e in recovered {
        if !out.iter().any(|o| o.span.start == e.span.start) {
            out.push(e);
        }
    }
    out
}

/// [`parse_units_with_recovery`], also returning the names of the declarations
/// recovery skipped (#1663).
pub fn parse_units_recovering(tokens: &[Token], source: &str) -> Recovered {
    parse_units_recovering_from(tokens, source, &mut 0)
}

/// [`parse_units_recovering`], continuing [`ExprId`] allocation from `next_id`
/// rather than starting at 0 — see [`parse_unit_with_warnings_from`]. #1710:
/// the project path checks a recovered file's surviving declarations
/// alongside every other file's, so its ids must come from the same durable
/// counter (`bynk_project::parse_cache`), or they would collide.
pub fn parse_units_recovering_from(tokens: &[Token], source: &str, next_id: &mut u32) -> Recovered {
    let (filtered, trivia) = split_trivia(tokens, source);
    let mut warnings = Vec::new();
    let mut p = Parser::new(&filtered, source, trivia, &mut warnings);
    p.recover_mode = true;
    p.next_expr_id = *next_id;
    let mut units = Vec::new();
    loop {
        match p.parse_unit() {
            Ok(u) => units.push(u),
            Err(e) => {
                p.recovered_errors.push(e);
                break;
            }
        }
        // A genuinely malformed trailing declaration is still surfaced via
        // recovery — checked *after* each successful parse, matching
        // `parse_unit`'s own "at least once" attempt on the first unit (an
        // empty file must still produce its usual unexpected-EOF diagnostic,
        // not silently yield an empty `units` with no error at all).
        if p.peek().is_none() {
            break;
        }
    }
    let broken_decl_names = std::mem::take(&mut p.broken_decl_names);
    *next_id = p.next_expr_id;
    let mut all_errors = p.recovered_errors;
    all_errors.append(&mut p.doc_errors);
    all_errors.append(&mut warnings);
    Recovered {
        units,
        errors: all_errors,
        broken_decl_names,
    }
}

/// Parse a token slice into a [`SourceUnit`] — either a commons or a context.
///
/// Each `.bynk` file is exactly one declaration of one kind.
pub fn parse_unit(tokens: &[Token], source: &str) -> Result<SourceUnit, Vec<CompileError>> {
    parse_unit_with_warnings(tokens, source).map(|(unit, _warnings)| unit)
}

/// [`parse_unit`] with the non-fatal diagnostics threaded out alongside the
/// AST (ADR 0117) — see [`parse_units_with_warnings`].
pub fn parse_unit_with_warnings(
    tokens: &[Token],
    source: &str,
) -> Result<(SourceUnit, Vec<CompileError>), Vec<CompileError>> {
    parse_unit_with_warnings_from(tokens, source, &mut 0)
}

/// [`parse_unit_with_warnings`], continuing [`ExprId`] allocation from
/// `next_id` instead of starting at 0, and writing the id one past the last
/// one this parse handed out back into it. T3.4 (R2.4): every top-level parse
/// entry point in this file constructs its own `Parser` and therefore its own
/// zero-based id space; a caller that will check two files' output together
/// in one pass (a multi-file commons — `bynk-emit`'s `collect_unit_methods`
/// merges a type's methods from sibling files into the file that declares the
/// type, before one `check_record` call) must thread one counter across every
/// file it parses, or two independently-numbered files collide on the same
/// id in the same `expr_types` map. Every *other* caller (a single buffer, an
/// LSP hover/completion query, a fixture test) never merges its output with
/// another file's before checking, so starting at 0 every time is correct —
/// [`parse_unit_with_warnings`] above is that default, unchanged.
pub fn parse_unit_with_warnings_from(
    tokens: &[Token],
    source: &str,
    next_id: &mut u32,
) -> Result<(SourceUnit, Vec<CompileError>), Vec<CompileError>> {
    let (filtered, trivia) = split_trivia(tokens, source);
    let mut warnings = Vec::new();
    let mut p = Parser::new(&filtered, source, trivia, &mut warnings);
    p.next_expr_id = *next_id;
    let result = match p.parse_unit() {
        Ok(u) => {
            if let Some(extra) = p.peek() {
                Err(vec![
                    CompileError::new(
                        "bynk.parse.extra_tokens",
                        extra.span,
                        "unexpected token after top-level declaration",
                    )
                    .with_note(
                        "a `.bynk` file contains exactly one `commons` or `context` declaration",
                    ),
                ])
            } else {
                Ok(u)
            }
        }
        Err(e) => Err(vec![e]),
    };
    // #1885: a doc block that holds code fails the parse, after any error the
    // parse itself hit.
    let result = match (result, std::mem::take(&mut p.doc_errors)) {
        (r, docs) if docs.is_empty() => r,
        (Ok(_), docs) => Err(docs),
        (Err(mut errs), mut docs) => {
            errs.append(&mut docs);
            Err(errs)
        }
    };
    *next_id = p.next_expr_id;
    // ADR 0117: warnings (e.g. orphan doc blocks) ride alongside a successful
    // parse — severity governs gating at the caller, not here.
    match result {
        Ok(u) => {
            // See `parse_units_with_warnings`: a file-trailing comment must
            // have been drained by `take_epilogue`.
            debug_assert!(
                p.trivia.epilogue_is_empty(),
                "a file-trailing comment was left undrained after a successful parse"
            );
            Ok((u, warnings))
        }
        Err(mut errs) => {
            errs.append(&mut warnings);
            Err(errs)
        }
    }
}

/// Parse a token slice into **all** the top-level [`SourceUnit`]s in one file
/// (v0.113, testing track slice 1b). A `.bynk` file may hold more than one
/// top-level declaration — an *atomic* file with `commons`/`context` **and** a
/// `suite` together (DECISION S) — so the compiler parses a `Vec`, not a single
/// unit. Test-ness is a property of each declaration, not of the file.
///
/// Bails on the first malformed declaration (like [`parse_unit`], not the
/// recovering LSP path). An empty file is an error.
pub fn parse_units(tokens: &[Token], source: &str) -> Result<Vec<SourceUnit>, Vec<CompileError>> {
    parse_units_with_warnings(tokens, source).map(|(units, _warnings)| units)
}

/// [`parse_units`] with the non-fatal diagnostics threaded out alongside the
/// AST (ADR 0117): a successful parse returns `Ok((units, warnings))` instead
/// of hard-failing on a warning-severity diagnostic (an orphan doc block used
/// to abort file discovery and throw the good AST away). A failed parse still
/// returns every diagnostic — errors then warnings — in the `Err`.
pub fn parse_units_with_warnings(
    tokens: &[Token],
    source: &str,
) -> Result<(Vec<SourceUnit>, Vec<CompileError>), Vec<CompileError>> {
    parse_units_with_drain_check(tokens, source)
        .map(|(units, warnings, _drained)| (units, warnings))
}

/// [`parse_units_with_warnings`], continuing [`ExprId`] allocation from
/// `next_id` rather than starting at 0 — see [`parse_unit_with_warnings_from`]
/// for why this exists. The one production caller is `bynk-emit`'s per-file
/// parse loop (`phase_parse`), which owns one counter across every file in a
/// single project parse so two files whose methods later get merged into one
/// `check_record` call (a multi-file commons) never collide.
pub fn parse_units_with_warnings_from(
    tokens: &[Token],
    source: &str,
    next_id: &mut u32,
) -> Result<(Vec<SourceUnit>, Vec<CompileError>), Vec<CompileError>> {
    parse_units_with_drain_check_from(tokens, source, next_id)
        .map(|(units, warnings, _drained)| (units, warnings))
}

/// [`parse_units_with_warnings`] plus whether every comment's trivia was
/// drained into the AST (`TriviaTable::is_fully_drained`). Finding #66:
/// `bynk-fmt`'s comment-preservation guard re-tokenized its own rendered
/// output just to diff comment bodies against the input — wasted work in the
/// overwhelming common case where nothing was left behind. That guard uses
/// this drain signal, computed from the same parse it already needs for
/// rendering, as a fast-path: `true` means every comment landed in the AST,
/// so re-checking the output can be skipped outright. No other caller needs
/// the signal, so it rides its own entry point rather than widening
/// [`parse_units_with_warnings`].
pub fn parse_units_with_drain_check(
    tokens: &[Token],
    source: &str,
) -> Result<(Vec<SourceUnit>, Vec<CompileError>, bool), Vec<CompileError>> {
    parse_units_with_drain_check_from(tokens, source, &mut 0)
}

/// [`parse_units_with_drain_check`], continuing [`ExprId`] allocation from
/// `next_id` — see [`parse_unit_with_warnings_from`].
pub fn parse_units_with_drain_check_from(
    tokens: &[Token],
    source: &str,
    next_id: &mut u32,
) -> Result<(Vec<SourceUnit>, Vec<CompileError>, bool), Vec<CompileError>> {
    parse_units_drain(tokens, source, next_id, false)
}

/// #2203 (DECISION F): [`parse_units_with_drain_check`] for the formatter,
/// which accepts the legacy `--` comment and `--|` doc line spellings instead
/// of reporting them, since printing them as `//` and `--- ` is the
/// migration. Every other error is reported as usual.
pub fn parse_units_migrating(
    tokens: &[Token],
    source: &str,
) -> Result<(Vec<SourceUnit>, Vec<CompileError>, bool), Vec<CompileError>> {
    parse_units_drain(tokens, source, &mut 0, true)
}

fn parse_units_drain(
    tokens: &[Token],
    source: &str,
    next_id: &mut u32,
    migrate: bool,
) -> Result<(Vec<SourceUnit>, Vec<CompileError>, bool), Vec<CompileError>> {
    let (filtered, trivia) = split_trivia(tokens, source);
    let mut warnings = Vec::new();
    let mut p = Parser::new(&filtered, source, trivia, &mut warnings);
    if migrate {
        p.doc_errors.clear();
    }
    p.next_expr_id = *next_id;
    let mut units = Vec::new();
    let mut errors: Vec<CompileError> = Vec::new();
    while p.peek().is_some() {
        match p.parse_unit() {
            Ok(u) => units.push(u),
            Err(e) => {
                errors.push(e);
                break;
            }
        }
    }
    // #1885: a doc block that holds code fails the parse.
    errors.append(&mut p.doc_errors);
    *next_id = p.next_expr_id;
    let eof = p.eof_span();
    let fully_drained = p.trivia.is_fully_drained();
    // `p` (and thus its `&mut warnings` borrow) is no longer used past here, so
    // the local `warnings` are readable again.
    if !errors.is_empty() {
        errors.append(&mut warnings);
        return Err(errors);
    }
    if units.is_empty() {
        return Err(vec![CompileError::new(
            "bynk.parse.unexpected_eof",
            eof,
            "expected `commons`, `context`, or `suite` to start the file, found end of file",
        )]);
    }
    // A file-trailing comment must have been drained by `take_epilogue` — the
    // one brace-form declarations forgot to call (a live comment-loss bug,
    // not the fundamentally-unfixed expression-interior case: expressions
    // carry no trivia at all, so asserting full drainage here would fire on
    // any ordinary program with a comment inside a `match`/list/record, which
    // `bynk-fmt`'s own comment-loss guard already handles gracefully rather
    // than as a hard failure).
    debug_assert!(
        p.trivia.epilogue_is_empty(),
        "a file-trailing comment was left undrained after a successful parse"
    );
    Ok((units, warnings, fully_drained))
}

/// A signed numeric literal in refinement-bound position (v0.21): `InRange`
/// bounds are either both `Int` or both `Float`.
enum SignedNumLit {
    Int(IntBound),
    Float(FloatBound),
}

struct Parser<'a> {
    tokens: &'a [Token],
    source: &'a str,
    pos: usize,
    /// Accumulated non-fatal diagnostics. v0.3 uses this for orphan-doc
    /// warnings, which are emitted as errors with a distinguishable category.
    warnings: &'a mut Vec<CompileError>,
    /// When true, the item-level loops catch errors from individual item
    /// parses, push them into `recovered_errors`, and skip forward to the
    /// next top-level item boundary instead of bailing. Used by the LSP via
    /// [`parse_unit_with_recovery`]; disabled in the normal `parse` path so
    /// existing single-error behaviour is preserved.
    recover_mode: bool,
    /// Errors collected during recovery-mode parsing. Only populated when
    /// `recover_mode` is true.
    recovered_errors: Vec<CompileError>,
    /// #1663: the token index where the current top-level item starts, set by
    /// each top-level item loop. Read once by [`Self::handle_item_err`] to name
    /// the declaration a recovery skips.
    pub(crate) item_start: Option<usize>,
    /// #1663: the names of top-level declarations recovery skipped (a `type`,
    /// `fn`, method, `event`, `capability`, `service`, `agent` or `actor`).
    /// They are known names whose declaration is broken, so a caller can keep
    /// references to them from echoing as unknown. Recovery mode only.
    broken_decl_names: Vec<String>,
    /// Line-comment trivia separated from the token stream. See
    /// [`TriviaTable`].
    trivia: TriviaTable,
    /// Live recursion depth of the three self-recursive parse entry points
    /// (`parse_expr`, `parse_type_ref`, `parse_pattern`). Incremented on entry
    /// and decremented on exit by [`Parser::enter_recursion`] so it tracks the
    /// current stack depth; when it exceeds [`crate::MAX_NESTING_DEPTH`] the
    /// parser reports a bounded-depth diagnostic instead of overflowing its
    /// stack (#713).
    depth: usize,
    /// When true, a bare `ident {` on the *spine* of the current expression is
    /// an identifier followed by an unrelated block, never a record
    /// construction — so an `if`/`match` condition that ends in a bare
    /// identifier does not swallow the branch/arm block as `Ident { field }`
    /// (#636). Set only around the condition parse (see [`parse_cond_expr`]);
    /// `parse_expr` clears it, so the restriction is lifted inside any
    /// delimited sub-expression (parentheses, call arguments, list, record
    /// field). Mirrors Rust's `NO_STRUCT_LITERAL` restriction.
    no_record_literal: bool,
    /// True while parsing the tail of an `is` pattern that sits outside every
    /// delimiter — `x is P`, `x is A | B`. A record pattern there would be
    /// ambiguous with the block that follows an `if` condition, so it must be
    /// parenthesised (`x is (T { .. })`, patterns track Q2, #2641). Cleared by
    /// [`Parser::parse_pattern`], the entry every delimited (nested) pattern
    /// goes through.
    is_pattern_tail: bool,
    /// Running count of unclosed `{` seen so far — maintained solely by
    /// [`Self::bump`] (the one primitive that advances `self.pos`), so it
    /// always reflects the true nesting depth no matter which parse function
    /// is on the call stack. Finding #27/#30: `recover_to_top_item` reads it
    /// against [`Self::item_loop_baseline`] to tell "the enclosing item
    /// loop's own closing brace" apart from a still-unclosed nested
    /// construct's — without it, a sync scan that started partway through
    /// such a construct (an error deep inside a function body) stopped at the
    /// first `}` it saw, however deeply nested, and the enclosing item loop
    /// mistook that for its own body's end.
    brace_depth: usize,
    /// Stack of `brace_depth` snapshots, one per active item-loop body
    /// (commons/context/adapter/suite) — pushed right after that body's own
    /// `{` is consumed (or at loop entry, for a brace-free fragment form),
    /// popped at the loop's normal exit. `recover_to_top_item` treats its top
    /// entry as the depth an `}` must return to before it counts as the
    /// enclosing body's own closing brace rather than a nested construct's.
    item_loop_baseline: Vec<usize>,
    /// T3.4 (R2.4): next [`ExprId`] to hand out — monotonic, incremented by
    /// [`Self::alloc_expr_id`], the sole allocation point every `Expr`
    /// construction site in this parser calls.
    next_expr_id: u32,
    /// #1885: `bynk.parse.doc_block_contains_code` errors found by
    /// [`Self::take_doc_block`], with its `bynk.parse.multiple_docs` (#2203)
    /// and the legacy-spelling errors [`split_trivia`] found. Kept apart from the item `Result` path so a
    /// doc block, which every item loop reads before its recovery point, never
    /// aborts the body it sits in; each entry point turns them into errors.
    doc_errors: Vec<CompileError>,
    /// #1885: true for the parser [`doc_content_is_code`] runs over a doc
    /// block's content. It skips the code check on doc blocks inside that
    /// content, so the probe stays a plain parse.
    doc_probe: bool,
}

impl<'a> Parser<'a> {
    fn new(
        tokens: &'a [Token],
        source: &'a str,
        mut trivia: TriviaTable,
        warnings: &'a mut Vec<CompileError>,
    ) -> Self {
        let doc_errors = std::mem::take(&mut trivia.legacy);
        Self {
            tokens,
            source,
            pos: 0,
            warnings,
            recover_mode: false,
            recovered_errors: Vec::new(),
            item_start: None,
            broken_decl_names: Vec::new(),
            trivia,
            depth: 0,
            no_record_literal: false,
            is_pattern_tail: false,
            brace_depth: 0,
            item_loop_baseline: Vec::new(),
            next_expr_id: 0,
            doc_errors,
            doc_probe: false,
        }
    }

    /// T3.4 (R2.4): allocate the next [`ExprId`]. The sole allocation point —
    /// every `Expr { id: self.alloc_expr_id(), .. }` construction in this
    /// parser calls it exactly once, so two nodes never share an id and every
    /// id a caller holds was actually handed out here.
    fn alloc_expr_id(&mut self) -> ExprId {
        let id = ExprId(self.next_expr_id);
        self.next_expr_id += 1;
        id
    }

    /// Enter a self-recursive parse step, bumping the live recursion depth and
    /// failing with a bounded-depth diagnostic if it would exceed
    /// [`crate::MAX_NESTING_DEPTH`]. The caller pairs a successful entry with a
    /// matching `self.depth -= 1` on the way out (see `parse_expr` /
    /// `parse_type_ref`); on the error path the depth is restored here so a
    /// recovering caller is not left mis-counted. `what` names the construct
    /// for the message (e.g. "this expression", "this type"). See #713.
    fn enter_recursion(&mut self, what: &str) -> Result<(), CompileError> {
        self.depth += 1;
        if self.depth > crate::MAX_NESTING_DEPTH {
            self.depth -= 1;
            let span = self
                .peek()
                .map(|t| t.span)
                .unwrap_or_else(|| self.eof_span());
            return Err(self.nesting_too_deep(span, what));
        }
        Ok(())
    }

    /// The bounded-depth diagnostic shared by [`enter_recursion`] and
    /// [`enter_chain_fold`].
    fn nesting_too_deep(&self, span: Span, what: &str) -> CompileError {
        CompileError::new(
            "bynk.parse.nesting_too_deep",
            span,
            format!(
                "{what} nests more than {} levels deep",
                crate::MAX_NESTING_DEPTH
            ),
        )
        .with_note(
            "deeply nested source is rejected to keep the parser from overflowing its \
             stack and aborting; flatten or split the construct",
        )
    }

    /// The bounded-depth diagnostic for the *iteratively*-built spines —
    /// associative operator chains ([`enter_chain_fold`]) and postfix receiver
    /// chains ([`deepen_spine`]). Same code as [`nesting_too_deep`] (one budget,
    /// one diagnostic) but phrased for a flat chain, which is long rather than
    /// *nested*, and points at the idiomatic fix.
    fn expression_too_long(&self, span: Span) -> CompileError {
        CompileError::new(
            "bynk.parse.nesting_too_deep",
            span,
            format!(
                "this expression is more than {} levels deep",
                crate::MAX_NESTING_DEPTH
            ),
        )
        .with_note(
            "a long operator or member chain is rejected to keep the compiler from overflowing \
             its stack; split it across `let` bindings, or reduce a sequence with \
             `.sum()`/`.fold(...)`",
        )
    }

    /// Count one more operand folded onto an associative operator chain against
    /// the same recursion budget as [`enter_recursion`] (#714).
    ///
    /// Associative chains (`+`, `*`, `&&`, `||`) are built *iteratively* in the
    /// precedence ladder, so — unlike parentheses, calls, or `implies` — they
    /// never re-enter `parse_expr` and thus slip past the `enter_recursion`
    /// guard. Yet each fold deepens the left-nested `Expr` tree by one level,
    /// and a long flat chain (`1 + 1 + … + 1`) overflows every *recursive*
    /// consumer of that tree downstream — the checker's `type_of`, the
    /// formatter, the emitter, and the AST's own recursive `Drop` — exactly as
    /// deeply nested source overflows the parser. Counting each fold on the
    /// shared `depth` budget bounds the whole expression's height, and because
    /// it is the *same* budget it composes with the ambient nesting depth, so a
    /// chain buried inside deeply nested source cannot exceed the bound either.
    ///
    /// The caller accumulates `folds` and subtracts them from `depth` before it
    /// returns, so the live count unwinds as a recursive descent would; on the
    /// overflow path the whole chain's contribution is restored here so a
    /// recovering caller is not left mis-counted.
    fn enter_chain_fold(&mut self, folds: &mut usize, span: Span) -> Result<(), CompileError> {
        self.depth += 1;
        *folds += 1;
        if self.depth > crate::MAX_NESTING_DEPTH {
            self.depth -= *folds;
            *folds = 0;
            return Err(self.expression_too_long(span));
        }
        Ok(())
    }

    /// Count one more level of an iteratively-built postfix receiver spine
    /// (`a.b.c…`, `f()?.g()…`) against the shared budget (#714). Like
    /// [`enter_chain_fold`], postfix loops rather than recurses, so a long spine
    /// escapes [`enter_recursion`] yet grows an arbitrarily deep receiver tree
    /// that the downstream walks recurse through. `parse_postfix` restores
    /// `depth` wholesale on the way out (its many error paths make a
    /// save/restore wrapper cleaner than per-fold unwinding), so this only bumps
    /// and checks.
    fn deepen_spine(&mut self, span: Span) -> Result<(), CompileError> {
        self.depth += 1;
        if self.depth > crate::MAX_NESTING_DEPTH {
            return Err(self.expression_too_long(span));
        }
        Ok(())
    }

    /// Comments immediately preceding the current peek position. Consumed
    /// (the table entry is cleared) so the same comments are not attached
    /// to two nodes.
    ///
    /// For a slot in a body: canonical style has no blank lines there, so a
    /// [`Comment::Blank`] is dropped and the comments stay attached to what
    /// follows. A declaration set apart by blank lines (one at unit level, or a
    /// service's or agent's section or handler) keeps it, through
    /// [`Self::take_unit_leading_trivia`].
    fn take_leading_trivia(&mut self) -> Vec<Comment> {
        let mut comments = self.take_unit_leading_trivia();
        comments.retain(|c| *c != Comment::Blank);
        comments
    }

    /// [`Self::take_leading_trivia`] for a slot set apart by blank lines, keeping each
    /// [`Comment::Blank`] (#1884).
    fn take_unit_leading_trivia(&mut self) -> Vec<Comment> {
        self.trivia.take_leading(self.pos)
    }

    /// The comments after the last token, which close the file at unit level,
    /// so a [`Comment::Blank`] between them is kept.
    fn take_epilogue_trivia(&mut self) -> Vec<Comment> {
        self.trivia.take_epilogue()
    }

    /// Trailing comment, if any, on the same source line as the most
    /// recently consumed content token. Call AFTER finishing a declaration
    /// or statement, while `self.pos` points one past its last token.
    fn take_trailing_trivia(&mut self) -> Option<String> {
        if self.pos == 0 {
            return None;
        }
        self.trivia.take_trailing(self.pos - 1)
    }

    /// Handle a per-item parse error. In recovery mode, record the error and
    /// advance to the next sync point so the item loop can continue; otherwise
    /// propagate as a hard failure.
    fn handle_item_err(&mut self, e: CompileError) -> Result<(), CompileError> {
        if self.recover_mode {
            self.recovered_errors.push(e);
            if let Some(start) = self.item_start.take() {
                self.broken_decl_names.extend(self.decl_names_at(start));
            }
            let before = self.pos;
            self.recover_to_top_item();
            // The sync target may be the very token that produced the error —
            // a context-only keyword (`capability`, `service`, …) at item
            // position in a commons errors *without consuming it*, and it is
            // itself a sync point. Recovery must always make progress, or the
            // item loop re-reports the same error until memory runs out
            // (found by the `parse` fuzz target on a seed input).
            // #1663: unless the cursor is on the `}` that closes this item
            // loop's own body (an error raised at the end of the last item):
            // consuming it would make the body look unclosed, a follow-on
            // `unexpected_eof`. The loop ends on that `}` itself. A fragment-
            // form loop has no closing brace (baseline 0), so a stray `}`
            // there is still consumed.
            let baseline = self.item_loop_baseline.last().copied().unwrap_or(0);
            let closes_body = baseline > 0
                && self.brace_depth == baseline
                && self.peek_kind() == Some(TokenKind::RBrace);
            if self.pos == before && !closes_body {
                self.bump();
                // #1663: and skip the rest of the rejected item too (`agent`
                // in a commons: its name and `{ … }` body). Otherwise the next
                // loop iteration misreads its name as a malformed item and
                // reports a second, follow-on syntax error.
                self.recover_to_top_item();
            }
            Ok(())
        } else {
            Err(e)
        }
    }

    /// #1663: the name(s) a declaration starting at token `start` declares:
    /// `type T`, `fn f`, `fn T.m` (recorded as `T.m`), `event E`,
    /// `capability C`, `service S`, `agent A`, `actor A`. Empty when the item
    /// is not one of those or its name never parsed (`fn ( -> Int`).
    fn decl_names_at(&self, start: usize) -> Vec<String> {
        let kind_at = |i: usize| self.tokens.get(i).map(|t| t.kind);
        let ident_at = |i: usize| {
            self.tokens
                .get(i)
                .filter(|t| t.kind == TokenKind::Ident)
                .map(|t| self.slice(t.span).to_string())
        };
        match kind_at(start) {
            Some(
                TokenKind::Type
                | TokenKind::Event
                | TokenKind::Capability
                | TokenKind::Service
                | TokenKind::Agent
                | TokenKind::Actor,
            ) => ident_at(start + 1).into_iter().collect(),
            Some(TokenKind::Fn) => match (ident_at(start + 1), kind_at(start + 2)) {
                // A method is recorded qualified, `T.m`, so a broken method
                // is never mistaken for a free `fn m` of the same name.
                (Some(owner), Some(TokenKind::Dot)) => match ident_at(start + 3) {
                    Some(method) => vec![format!("{owner}.{method}")],
                    None => Vec::new(),
                },
                (Some(name), _) => vec![name],
                (None, _) => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Skip forward to the next top-level item boundary: either an
    /// [`is_item_start`] keyword at the enclosing item loop's own nesting
    /// depth, a closing brace that returns to that depth, or end-of-input.
    /// Used only in recovery mode.
    ///
    /// Finding #27/#30: brace-depth-gated against
    /// [`Self::item_loop_baseline`], so a `}` deep inside a still-unclosed
    /// nested construct (an error partway through a function body, itself
    /// inside a `match` arm) is skipped over rather than mistaken for the
    /// enclosing body's own closing brace — the old flat scan stopped at
    /// literally the first `}` it saw, however deep, handing the item loop a
    /// brace that did not belong to it and making it return with zero items.
    fn recover_to_top_item(&mut self) {
        let baseline = self.item_loop_baseline.last().copied().unwrap_or(0);
        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::RBrace if self.brace_depth == baseline => return,
                _ if self.brace_depth == baseline && is_item_start(t.kind) => return,
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Mark the start of a top-level item loop (`declarations.rs`'s
    /// `parse_commons_brace`/`_fragment`, `parse_context_brace`/`_fragment`,
    /// `parse_test_brace`/`_fragment`, `parse_adapter_body`) — called right
    /// after that body's own `{` is consumed (brace form) or at the loop's
    /// own entry (fragment form, which has no enclosing brace of its own).
    /// Paired with [`Self::exit_item_loop`] at the loop's normal exit.
    fn enter_item_loop(&mut self) {
        self.item_loop_baseline.push(self.brace_depth);
    }

    /// Pair of [`Self::enter_item_loop`].
    fn exit_item_loop(&mut self) {
        self.item_loop_baseline.pop();
    }

    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.pos).copied()
    }

    fn peek_kind(&self) -> Option<TokenKind> {
        self.peek().map(|t| t.kind)
    }

    /// #2618: consume a leading `private` modifier at item position and return
    /// its span. `private` is contextual, not reserved: it is a modifier only
    /// when it is an identifier spelled `private` immediately followed by a
    /// declaration keyword, so `@cache(scope: private)` and a binding named
    /// `private` keep their meaning.
    ///
    /// The modifier applies to `fn`, `type`, `messages`, `asset`, `embed` and
    /// `const` in a commons or context (`allowed`). On any other declaration, or in an
    /// adapter, it is `bynk.visibility.private_not_allowed`: an error in
    /// recovery mode that leaves the declaration to parse as if unmodified,
    /// and the parse's error otherwise.
    fn take_private_modifier(&mut self, allowed: bool) -> Result<Option<Span>, CompileError> {
        let Some(t) = self.peek() else {
            return Ok(None);
        };
        if t.kind != TokenKind::Ident || self.slice(t.span) != "private" {
            return Ok(None);
        }
        let Some(next) = self.nth(1) else {
            return Ok(None);
        };
        let modifiable = matches!(
            next.kind,
            TokenKind::Fn
                | TokenKind::Type
                | TokenKind::Messages
                | TokenKind::Asset
                | TokenKind::Embed
                | TokenKind::Const
        );
        let declaration = modifiable
            || matches!(
                next.kind,
                TokenKind::Service
                    | TokenKind::Agent
                    | TokenKind::Capability
                    | TokenKind::Provides
                    | TokenKind::Actor
                    | TokenKind::Event
            );
        if !declaration {
            return Ok(None);
        }
        self.bump();
        if modifiable && allowed {
            return Ok(Some(t.span));
        }
        let what = self.slice(next.span);
        let err = if allowed {
            CompileError::new(
                "bynk.visibility.private_not_allowed",
                t.span,
                format!("`private` does not apply to `{what}` declarations"),
            )
            .with_note(
                "`private` applies to `fn`, `type`, `messages`, `asset`, `embed` and `const`; a context's `exports` and `consumes` govern how far its services, agents, capabilities, providers, actors and events reach",
            )
        } else {
            CompileError::new(
                "bynk.visibility.private_not_allowed",
                t.span,
                "`private` applies only to declarations in a commons or context, not in an adapter",
            )
            .with_note("an adapter's `exports` governs what its consumers see")
        };
        if self.recover_mode {
            self.recovered_errors.push(err);
            Ok(None)
        } else {
            Err(err)
        }
    }

    /// The token `n` positions ahead of the cursor (`nth(0)` == `peek()`).
    fn nth(&self, n: usize) -> Option<Token> {
        self.tokens.get(self.pos + n).copied()
    }

    fn nth_kind(&self, n: usize) -> Option<TokenKind> {
        self.nth(n).map(|t| t.kind)
    }

    /// The source text of the token `n` positions ahead, or `""` if none.
    fn nth_text(&self, n: usize) -> &'a str {
        self.nth(n).map(|t| self.slice(t.span)).unwrap_or("")
    }

    /// The span of the most recently consumed token (`self.pos - 1`). Falls back
    /// to the current token's span when nothing has been consumed yet.
    fn prev_span(&self) -> Span {
        self.tokens
            .get(self.pos.wrapping_sub(1))
            .or_else(|| self.peek_ref())
            .map(|t| t.span)
            .unwrap_or_default()
    }

    fn peek_ref(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        let t = self.peek();
        if let Some(t) = t {
            match t.kind {
                TokenKind::LBrace => self.brace_depth += 1,
                TokenKind::RBrace => self.brace_depth = self.brace_depth.saturating_sub(1),
                _ => {}
            }
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, kind: TokenKind) -> Option<Token> {
        if self.peek_kind() == Some(kind) {
            self.bump()
        } else {
            None
        }
    }

    fn slice(&self, span: Span) -> &'a str {
        &self.source[span.range()]
    }

    /// True when the next token sits on a later line than `prev`. Used to
    /// keep a `[` that opens a new line out of the postfix type-application
    /// form: `f` followed by `[1, 2]` on the next line is an identifier and
    /// a list literal, not `f[…]` (v0.20b).
    fn next_token_on_new_line(&self, prev: Span) -> bool {
        match self.peek() {
            Some(t) if prev.end <= t.span.start => {
                self.source[prev.end..t.span.start].contains('\n')
            }
            _ => false,
        }
    }

    /// Span pointing at the end of input — used for "unexpected EOF" reports.
    /// The start backs up to the **start of the final char**, not `len - 1`, so
    /// the span never splits a multibyte codepoint (an unterminated construct
    /// whose last line ends in non-ASCII — e.g. a `--` comment ending in `→`).
    fn eof_span(&self) -> Span {
        let end = self.source.len();
        let start = (0..end)
            .rev()
            .find(|&i| self.source.is_char_boundary(i))
            .unwrap_or(0);
        Span::new(start, end)
    }

    fn expect(&mut self, kind: TokenKind, ctx: &str) -> Result<Token, CompileError> {
        match self.peek() {
            Some(t) if t.kind == kind => {
                self.bump();
                Ok(t)
            }
            Some(t) => Err(CompileError::new(
                "bynk.parse.expected_token",
                t.span,
                format!(
                    "expected {} {ctx}, found {}",
                    kind.describe(),
                    t.kind.describe()
                ),
            )),
            None => Err(CompileError::new(
                "bynk.parse.unexpected_eof",
                self.eof_span(),
                format!("expected {} {ctx}, found end of file", kind.describe()),
            )),
        }
    }

    fn expect_ident(&mut self, ctx: &str) -> Result<Ident, CompileError> {
        match self.peek() {
            Some(t) if t.kind == TokenKind::Ident => {
                self.bump();
                Ok(Ident {
                    name: self.slice(t.span).to_string(),
                    span: t.span,
                })
            }
            // v0.5 contextual keyword `on` doubles as an identifier in
            // expression / field-access positions so users can name fields and
            // parameters using it. It retains its keyword meaning only at
            // handler-decl-level (`on call(...)`).
            //
            // v0.7 / v0.112: `suite` and `case` are contextual too — they
            // introduce the suite declaration and its cases, but are perfectly
            // valid commons/context/field names otherwise.
            //
            // The tier is single-sourced in `keywords::RESERVED_CONTEXTUAL`:
            // this arm defers to it rather than hardcoding the token kinds, so
            // extending that list is enough to admit a new contextual keyword
            // here. Each of these words lexes only to its own token, so matching
            // the source text is equivalent to matching the kind.
            Some(t) if crate::keywords::is_reserved_contextual(self.slice(t.span)) => {
                self.bump();
                Ok(Ident {
                    name: self.slice(t.span).to_string(),
                    span: t.span,
                })
            }
            Some(t) if is_reserved_keyword(t.kind) => Err(CompileError::new(
                "bynk.parse.reserved_keyword",
                t.span,
                format!(
                    "expected identifier {ctx}, but `{}` is a reserved keyword",
                    self.slice(t.span)
                ),
            )
            .with_note("rename the identifier to something that is not a keyword")),
            Some(t) => Err(CompileError::new(
                "bynk.parse.expected_token",
                t.span,
                format!("expected identifier {ctx}, found {}", t.kind.describe()),
            )),
            None => Err(CompileError::new(
                "bynk.parse.unexpected_eof",
                self.eof_span(),
                format!("expected identifier {ctx}, found end of file"),
            )),
        }
    }

    // -- top level --

    /// Consume an optional doc at the current position, in either form,
    /// returning the doc and its span (through the end of its last line).
    /// Returns None if the next token is not a doc.
    ///
    /// #1885: a doc block whose content is code (see [`doc_content_is_code`])
    /// records `bynk.parse.doc_block_contains_code` in [`Self::doc_errors`];
    /// the block is still returned, so parsing carries on around it. A run of
    /// `--|` lines is not probed: each line is marked as a doc on purpose, so
    /// it cannot swallow code by accident the way a pair of dividers can.
    ///
    /// #2203 (DECISION D): a target has one doc. A doc directly followed by
    /// another (no blank line between), of either form, records
    /// `bynk.parse.multiple_docs` the same way; all are consumed and the first
    /// is returned. This replaces #1888's joined `--|` lines and its
    /// mixed-forms error.
    fn take_doc_block(&mut self) -> Option<(Doc, Span)> {
        let first = self.take_one_doc()?;
        let mut docs = vec![first.clone()];
        while let Some(next) = self.peek()
            && matches!(next.kind, TokenKind::DocBlock | TokenKind::DocLine)
            && !has_blank_line_between(self.source, docs[docs.len() - 1].1.end, next.span.start)
        {
            docs.push(self.take_one_doc().expect("a doc token was peeked"));
        }
        if docs.len() > 1 && !self.doc_probe {
            self.doc_errors.push(multiple_docs(self.source, &docs));
        }
        Some(first)
    }

    /// One doc: a `---` block or a one-line doc.
    fn take_one_doc(&mut self) -> Option<(Doc, Span)> {
        match self.peek_kind()? {
            TokenKind::DocBlock => {
                let t = self.bump().unwrap();
                let body = doc_block_content(self.source, t.span);
                if !self.doc_probe && doc_content_is_code(&body) {
                    self.doc_errors.push(doc_block_contains_code(t.span));
                }
                Some((Doc::new(body, DocForm::Block), t.span))
            }
            TokenKind::DocLine => {
                let t = self.bump().unwrap();
                let text = doc_line_text(self.source, t.span);
                Some((Doc::new(text, DocForm::Lines), t.span))
            }
            _ => None,
        }
    }

    /// Collect all line-comment trivia leading the next declaration plus
    /// the optional doc block. Comments may appear both *before* and
    /// *between* the doc and the declaration; the spec canonicalises both
    /// groups above the doc, so we concatenate them.
    ///
    /// The doc comes back as a [`DocLead`], which records where it sat among
    /// the comments, so an orphan can be kept in place ([`keep_orphan`]).
    ///
    /// For a declaration set apart by blank lines (a unit-level one, or a
    /// service's or agent's section or handler), which keeps a blank line after
    /// a comment (#1884); [`Self::collect_body_item_lead`] is the form for a
    /// body whose items have no blank lines between them.
    fn collect_item_lead(&mut self) -> (Vec<Comment>, Option<DocLead>) {
        self.collect_lead(Self::take_unit_leading_trivia)
    }

    /// [`Self::collect_item_lead`] for a declaration in a body, which drops a
    /// [`Comment::Blank`] as [`Self::take_leading_trivia`] does.
    fn collect_body_item_lead(&mut self) -> (Vec<Comment>, Option<DocLead>) {
        self.collect_lead(Self::take_leading_trivia)
    }

    fn collect_lead(
        &mut self,
        take: fn(&mut Self) -> Vec<Comment>,
    ) -> (Vec<Comment>, Option<DocLead>) {
        let mut leading = take(self);
        let doc = self.take_doc_block().map(|(content, span)| DocLead {
            content,
            span,
            at: leading.len(),
        });
        if doc.is_some() {
            leading.extend(take(self));
        }
        (leading, doc)
    }

    /// #1888: the comments and doc above a member: a record field, a variant,
    /// a variant's payload field or a message entry. As
    /// [`Self::collect_body_item_lead`] and [`Self::finalize_doc`] together, so
    /// a blank line orphans a member's doc as it does a declaration's, and a
    /// [`Comment::Blank`] is dropped, as in any body (#1884). A doc with
    /// `close` (the body's closing token) next documents nothing: it is warned
    /// about and kept among the comments, which the caller then takes as the
    /// body's closing comments.
    fn collect_member_lead(&mut self, close: TokenKind) -> (Vec<Comment>, Option<Doc>) {
        let (mut leading, doc) = self.collect_body_item_lead();
        let Some(doc) = doc else {
            return (leading, None);
        };
        match self.peek() {
            Some(t) if t.kind != close => {
                let doc = self.finalize_doc(Some(doc), t.span, &mut leading);
                (leading, doc)
            }
            _ => {
                self.warnings.push(CompileError::new(
                    "bynk.parse.orphan_doc_block",
                    doc.span,
                    "documentation has no following member to attach to",
                ));
                keep_orphan(&mut leading, doc);
                (leading, None)
            }
        }
    }

    /// #1888: the kind of the first token after any doc tokens at the cursor,
    /// so a member loop can tell a doc on its next member from a doc on the
    /// declaration after the body (a pipe-form sum has no closing token).
    fn peek_past_docs(&self) -> Option<TokenKind> {
        self.tokens[self.pos.min(self.tokens.len())..]
            .iter()
            .map(|t| t.kind)
            .find(|k| !matches!(k, TokenKind::DocBlock | TokenKind::DocLine))
    }

    /// Attach a parsed doc block to a following declaration unless a blank
    /// line separates them, in which case the doc is orphaned: a warning, and
    /// the block is kept in `leading` where it sat (#1756).
    fn finalize_doc(
        &mut self,
        doc: Option<DocLead>,
        next_span: Span,
        leading: &mut Vec<Comment>,
    ) -> Option<Doc> {
        let doc = doc?;
        let doc_span = doc.span;
        // A blank line between the doc and the next decl orphans the doc.
        if has_blank_line_between(self.source, doc_span.end, next_span.start) {
            self.warnings.push(
                CompileError::new(
                    "bynk.parse.orphan_doc_block",
                    doc_span,
                    "documentation block is separated from the following declaration by a blank line; it will not be attached",
                )
                .with_note(
                    "remove the blank line to attach the doc to the next declaration, \
                     or remove the doc block if it is not meant to document anything",
                ),
            );
            keep_orphan(leading, doc);
            return None;
        }
        Some(doc.content)
    }
}

/// A doc block read ahead of a declaration, before it is known to attach: its
/// normalised content, its span, and its index among the leading comments
/// collected with it.
pub(crate) struct DocLead {
    pub(crate) content: Doc,
    pub(crate) span: Span,
    pub(crate) at: usize,
}

/// #1756: keep an orphaned doc block in `leading`, at the place it was written
/// among those comments, so the formatter prints it where it was rather than
/// dropping it.
fn keep_orphan(leading: &mut Vec<Comment>, doc: DocLead) {
    let at = doc.at.min(leading.len());
    leading.insert(at, Comment::OrphanDoc(doc.content));
}

/// #2203 (DECISION D): the `bynk.parse.multiple_docs` error for `docs`, two
/// or more docs directly above one target. When every one is a `--- text`
/// line, the fix joins them into one block, which is what a doc of more than
/// one line is.
fn multiple_docs(source: &str, docs: &[(Doc, Span)]) -> CompileError {
    let (_, first) = docs[0];
    let (_, last) = docs[docs.len() - 1];
    let mut err = CompileError::new(
        "bynk.parse.multiple_docs",
        docs[1].1,
        "this declaration or member already has a doc; a target has one doc",
    )
    .with_label(first, "the first doc is here")
    .with_note(
        "a doc of more than one line is a block: a `---` line, the text, and another `---` line",
    );
    let one_liners = docs
        .iter()
        .all(|(d, span)| d.form == DocForm::Lines && !doc_line_is_legacy(source, *span));
    if one_liners {
        let line_start = source[..first.start].rfind('\n').map_or(0, |i| i + 1);
        let indent = &source[line_start..first.start];
        let text = docs
            .iter()
            .map(|(d, _)| d.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let fence = "-".repeat(doc_fence_len(&text));
        let mut block = fence.clone();
        for line in text.lines() {
            block.push('\n');
            if !line.is_empty() {
                block.push_str(indent);
                block.push_str(line);
            }
        }
        block.push('\n');
        block.push_str(indent);
        block.push_str(&fence);
        let end = source[..last.end].trim_end_matches(['\n', '\r']).len();
        err = err.with_suggestion(
            "join the docs into one `---` block",
            vec![(Span::new_in(first.file, first.start, end), block)],
            crate::error::Applicability::MachineApplicable,
        );
    }
    err
}

/// #1885: the `bynk.parse.doc_block_contains_code` error for the doc block at
/// `span`.
fn doc_block_contains_code(span: Span) -> CompileError {
    CompileError::new(
        "bynk.parse.doc_block_contains_code",
        span,
        "this documentation block contains declarations, so they are not compiled",
    )
    .with_note(
        "a line of three or more dashes opens or closes a doc block, so two dash \
         dividers turn the code between them into documentation; for a divider use a \
         line comment with text, such as `-- Helpers --`, and to show code in a doc \
         block put it in a Markdown code fence (```)",
    )
}

/// The keywords that open a declaration in a unit body. A doc block's content
/// is probed as code only when it lexes and holds one of these, so prose (which
/// usually fails to lex, or has none) costs one tokenize at most.
const DECLARATION_KEYWORDS: &[TokenKind] = &[
    TokenKind::Type,
    TokenKind::Fn,
    TokenKind::Capability,
    TokenKind::Provides,
    TokenKind::Service,
    TokenKind::Agent,
    TokenKind::Actor,
    TokenKind::Messages,
    TokenKind::Event,
    TokenKind::Asset,
    TokenKind::Embed,
    TokenKind::Const,
    TokenKind::Uses,
    TokenKind::Consumes,
    TokenKind::Exports,
    TokenKind::Binding,
];

/// #1885 (ADR: doc-fence-matched-length): true when a doc block's `content`,
/// with its Markdown code fences removed, parses as one or more declarations.
///
/// The probe is an `adapter` body in fragment form: an adapter accepts every
/// item a context or commons does, plus `binding`, and leaves placement to the
/// checker. Success means the whole content parsed and declared something; a
/// content that only holds another doc block (front matter, say) is prose.
///
/// Nesting is legal since #1885, so code can hide one level down: a doc block
/// inside the content lexes to one opaque token, and its content is probed in
/// turn (up to [`DOC_PROBE_MAX_DEPTH`] levels), the outer block reporting what
/// any level finds. The probe parser itself does not report on those blocks
/// (`doc_probe`). Content that does not lex because it holds a lone `---` (a
/// Markdown rule) is probed again with its marker lines blanked out.
fn doc_content_is_code(content: &str) -> bool {
    doc_content_is_code_at(content, 0)
}

/// How many levels of nested doc blocks [`doc_content_is_code`] looks into.
/// Doc blocks nested this deep are written on purpose; the bound only keeps
/// adversarial input from costing a parse per level without end.
const DOC_PROBE_MAX_DEPTH: usize = 4;

fn doc_content_is_code_at(content: &str, depth: usize) -> bool {
    let stripped = strip_code_fences(content);
    let tokens = match crate::lexer::tokenize(&stripped) {
        Ok(tokens) => tokens,
        Err(_) => {
            // A lone `---` rule leaves the content unlexable; without the
            // marker lines it may still be nothing but declarations.
            let unmarked = blank_marker_lines(&stripped);
            if unmarked == stripped {
                return false;
            }
            return doc_content_is_code_at(&unmarked, depth);
        }
    };
    if depth < DOC_PROBE_MAX_DEPTH
        && tokens.iter().any(|t| {
            t.kind == TokenKind::DocBlock
                && doc_content_is_code_at(&doc_block_content(&stripped, t.span), depth + 1)
        })
    {
        return true;
    }
    if !tokens
        .iter()
        .any(|t| DECLARATION_KEYWORDS.contains(&t.kind))
    {
        return false;
    }
    let (filtered, trivia) = split_trivia(&tokens, &stripped);
    let mut warnings = Vec::new();
    let mut p = Parser::new(&filtered, &stripped, trivia, &mut warnings);
    p.doc_probe = true;
    let at = Span::new(0, 0);
    let name = QualifiedName {
        parts: Vec::new(),
        span: at,
    };
    match p.parse_adapter_body(at, name, None, false) {
        Ok(a) => {
            p.peek().is_none()
                && (!a.items.is_empty()
                    || !a.uses.is_empty()
                    || !a.consumes.is_empty()
                    || !a.exports.is_empty()
                    || a.binding.is_some())
        }
        Err(_) => false,
    }
}

/// `content` with every doc-block marker line (three or more dashes alone on
/// the line) blanked out, line for line.
fn blank_marker_lines(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    for line in content.lines() {
        let trimmed = line.trim();
        if !(trimmed.len() >= 3 && trimmed.bytes().all(|b| b == b'-')) {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// `content` with every Markdown fenced code block (```` ``` ```` or `~~~`,
/// three or more, closed by a fence of the same character at least as long)
/// blanked out, line for line. An unclosed fence runs to the end, as in
/// Markdown. Code shown in a fence is an example, not swallowed code.
fn strip_code_fences(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut open: Option<(char, usize)> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        let fence = ['`', '~'].into_iter().find_map(|c| {
            let n = trimmed.chars().take_while(|&x| x == c).count();
            (n >= 3).then_some((c, n))
        });
        match (open, fence) {
            (None, Some(f)) => open = Some(f),
            (Some((c, n)), Some((fc, fnum)))
                if fc == c && fnum >= n && trimmed.len() == fnum * c.len_utf8() =>
            {
                open = None;
            }
            (None, None) => out.push_str(line),
            (Some(_), _) => {}
        }
        out.push('\n');
    }
    out
}

/// Parse the body of a lexed double-quoted string literal (the lexeme,
/// including surrounding quotes), applying the v0 escape rules.
fn parse_string_literal(lexeme: &str, span: Span) -> Result<String, CompileError> {
    let bytes = lexeme.as_bytes();
    debug_assert!(bytes.first() == Some(&b'"') && bytes.last() == Some(&b'"'));
    let inner = &lexeme[1..lexeme.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                other => {
                    return Err(CompileError::new(
                        "bynk.lex.bad_escape",
                        span,
                        format!(
                            "invalid escape sequence `\\{}` in string literal",
                            other.map(|c| c.to_string()).unwrap_or_default()
                        ),
                    )
                    .with_note("supported escapes: \\n \\t \\\" \\\\"));
                }
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn is_reserved_keyword(kind: TokenKind) -> bool {
    use TokenKind::*;
    matches!(
        kind,
        Commons
            | Type
            | Fn
            | Where
            | True
            | False
            | Int
            | String
            | Bool
            | Let
            | If
            | Else
            | Ok
            | Err
            | Result
            | ValidationError
            | Enum
            | Match
            | Option
            | Record
            | Self_
            | Some
            | None
            | Is
            | Opaque
            | Uses
            | Context
            | Consumes
            | Exports
            | Transparent
            | Agent
            | As
            | Capability
            | Effect
            | Do
            | Given
            | On
            | Http
            | Provides
            | Stub
            | Service
            | Actor
            | By
            | Expect
            | Suite
            | Case
            | Float
            | Duration
            | Instant
            | Bytes
            | JsonError
            | Property
            | Adapter
            | Binding
            | Cron
            | Queue
            | From
            | Protocol
            | Invariant
            | Implies
            | Requires
            | Ensures
            | Transition
    )
}

/// True when `kind` starts a top-level unit (`commons`/`context`/`adapter`/
/// `suite`) or an item within one of their bodies — every keyword any of
/// `parse_commons_brace`/`_fragment`, `parse_context_brace`/`_fragment`,
/// `parse_test_brace`/`_fragment`, or `parse_adapter_body` dispatches on
/// (`declarations.rs`). The single set [`Parser::recover_to_top_item`]'s sync
/// scan checks against — finding #27/#30: that scan had drifted from what the
/// item loops actually recognise (`Property`, `Actor`, `Event`, `Binding`,
/// and the `adapter` unit keyword itself were all missing), so an error
/// recovery sync could walk past a real item/unit boundary instead of
/// stopping there.
fn is_item_start(kind: TokenKind) -> bool {
    use TokenKind::*;
    matches!(
        kind,
        // Top-level unit keywords.
        Commons | Context | Adapter | Suite
        // Body items shared across commons/context/adapter.
        | Type | Fn | Messages | Event | Asset | Embed | Const | Uses
        // Context/adapter-only body items.
        | Consumes | Exports | Capability | Provides | Service | Agent | Actor
        // Adapter-only.
        | Binding
        // Suite/test-only body items.
        | Stub | Case | Property
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn parse_str(src: &str) -> Result<Commons, Vec<CompileError>> {
        let toks = tokenize(src).map_err(|e| vec![e])?;
        parse(&toks, src)
    }

    fn parse_recover_str(src: &str) -> (Option<SourceUnit>, Vec<CompileError>) {
        let toks = match tokenize(src) {
            Ok(t) => t,
            Err(e) => return (None, vec![e]),
        };
        parse_unit_with_recovery(&toks, src)
    }

    /// Finding #29/#30: `parse_units_with_recovery` keeps every top-level unit
    /// an atomic `commons`+`suite` file declares (v0.113, DECISION S), where
    /// `parse_unit_with_recovery` keeps only the first — the defect the IDE's
    /// old single-unit parse entry point had (it silently discarded the
    /// trailing `suite`).
    #[test]
    fn parse_units_with_recovery_keeps_every_top_level_unit() {
        let src =
            "commons m {\n  fn f() -> Int { 1 }\n}\n\nsuite m\n\ncase \"c\" {\n  expect true\n}\n";
        let toks = tokenize(src).unwrap();
        let (units, errors) = parse_units_with_recovery(&toks, src);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(units.len(), 2, "expected both units, got {units:?}");
        assert!(matches!(units[0], SourceUnit::Commons(_)));
        assert!(matches!(units[1], SourceUnit::Suite(_)));

        // The singular wrapper still narrows to just the first, unchanged.
        let (unit, errors) = parse_unit_with_recovery(&toks, src);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(matches!(unit, Some(SourceUnit::Commons(_))));
    }

    /// `parse_unit_with_recovery` always attempts at least one parse, even on
    /// empty input — `parse_units_with_recovery`'s loop must preserve that
    /// (its `while`-style peek check alone would skip the body entirely and
    /// silently return no error), since 16+ existing callers rely on an empty
    /// file still producing its usual diagnostic rather than a silent `None`
    /// with no error at all.
    #[test]
    fn empty_input_still_reports_an_error_through_the_plural_entry_point() {
        let toks = tokenize("").unwrap();
        let (units, errors) = parse_units_with_recovery(&toks, "");
        assert!(units.is_empty());
        assert!(
            !errors.is_empty(),
            "an empty file must still produce a diagnostic, not silently no units and no error"
        );

        let (unit, unit_errors) = parse_unit_with_recovery(&toks, "");
        assert!(unit.is_none());
        assert_eq!(
            errors.len(),
            unit_errors.len(),
            "the singular wrapper must see the same error(s) as the plural entry point"
        );
    }

    /// Finding #66: `parse_units_with_drain_check`'s `fully_drained` flag is
    /// `bynk-fmt`'s signal for whether its comment-loss guard can skip a
    /// re-tokenize-and-diff of its own output. It must be `true` for an
    /// ordinary file (every comment sits before a declaration/statement or
    /// trails one) and `false` the moment a comment sits inside an expression
    /// subtree, where `TriviaTable` has no field to attach it to.
    #[test]
    fn drain_check_reports_expression_interior_comments_as_undrained() {
        let ordinary = "commons x {\n// note\ntype T = Int where Positive\n}\n";
        let toks = tokenize(ordinary).unwrap();
        let (_, _, drained) = parse_units_with_drain_check(&toks, ordinary).unwrap();
        assert!(
            drained,
            "a declaration-leading comment must be fully drained"
        );

        let lossy = "commons x {\n  fn f() -> Int {\n    1 + // note\n    2\n  }\n}\n";
        let toks = tokenize(lossy).unwrap();
        let (_, _, drained) = parse_units_with_drain_check(&toks, lossy).unwrap();
        assert!(
            !drained,
            "a comment inside a binop expression must be reported as undrained"
        );
    }

    #[test]
    fn eof_span_never_splits_a_multibyte_codepoint() {
        // An unterminated construct whose final line ends in a non-ASCII char
        // (here a `--` comment ending in `→`) once produced an `unexpected_eof`
        // span of `len - 1 .. len`, landing on the arrow's last continuation
        // byte. Every reported span must sit on char boundaries.
        for src in [
            "commons x {\n  // ends with an arrow →",
            "agent A {\n  key k: String\n  // note 🦀",
            "commons y {\n  type T = é",
        ] {
            let (_unit, errors) = parse_recover_str(src);
            for e in &errors {
                assert!(
                    src.is_char_boundary(e.span.start) && src.is_char_boundary(e.span.end),
                    "span {:?} splits a codepoint in {src:?}",
                    e.span,
                );
            }
        }
    }

    #[test]
    fn reserved_contextual_keywords_readable_in_expression_position() {
        // Events track, slice 0 (#939): `expect_ident`'s `RESERVED_CONTEXTUAL`
        // exemption (`keywords::RESERVED_CONTEXTUAL`: case/event/messages/on/
        // suite) covers *declaring* a binding with one of these names — a
        // parameter, a `let` — but the primary-expression parser previously
        // only admitted plain `TokenKind::Ident` when *reading one back*.
        // Latent since messages/on/case/suite shipped (no fixture happened to
        // name a binding after one of them and read it back inside the body);
        // surfaced concretely when `event` joined the tier and collided with
        // `examples/event-log`'s pre-existing `add(event: Event)` handler.
        for kw in [
            "asset", "case", "const", "embed", "event", "messages", "on", "suite",
        ] {
            let src = format!("commons x\n\nfn f({kw}: Int) -> Int {{\n  {kw}\n}}\n");
            let result = parse_str(&src);
            assert!(
                result.is_ok(),
                "a parameter named `{kw}` must be readable in expression position: {:?}",
                result.err()
            );
        }
    }

    #[test]
    fn recovery_skips_garbage_between_decls() {
        // Two `type` declarations separated by garbage. Recovery should
        // accept both and report one error for the garbage between them.
        let src = "commons x {\n\
                   type A = Int where NonNegative\n\
                   ??? !!!\n\
                   type B = String where NonEmpty\n\
                   }";
        let (unit, errors) = parse_recover_str(src);
        let unit = unit.expect("recovery should produce a partial AST");
        let SourceUnit::Commons(c) = unit else {
            panic!("expected commons")
        };
        // Both type decls should have been collected despite the garbage.
        let names: Vec<_> = c
            .items
            .iter()
            .map(|i| match i {
                CommonsItem::Type(t) => t.name.name.clone(),
                _ => panic!("expected only types"),
            })
            .collect();
        assert!(
            names.contains(&"A".to_string()) && names.contains(&"B".to_string()),
            "expected both A and B; got {names:?}",
        );
        assert!(!errors.is_empty(), "expected at least one parse error");
    }

    #[test]
    fn recovery_handles_bad_first_decl_then_good_second() {
        // First decl is malformed (missing `=`); second is well-formed.
        let src = "commons x {\n\
                   type A Int where NonNegative\n\
                   type B = String where NonEmpty\n\
                   }";
        let (unit, errors) = parse_recover_str(src);
        let unit = unit.expect("recovery should produce a partial AST");
        let SourceUnit::Commons(c) = unit else {
            panic!("expected commons")
        };
        let names: Vec<_> = c
            .items
            .iter()
            .filter_map(|i| match i {
                CommonsItem::Type(t) => Some(t.name.name.clone()),
                _ => None,
            })
            .collect();
        assert!(
            names.contains(&"B".to_string()),
            "B should be parsed after A's failure; got {names:?}"
        );
        assert!(!errors.is_empty(), "expected at least one parse error");
    }

    /// Finding #27/#30: an error two levels deep inside `f`'s body (a
    /// `match` arm's own block) used to make `recover_to_top_item`'s flat,
    /// depth-blind scan stop at the *first* `}` it saw — the arm block's own,
    /// not `f`'s. Two more `}` (the match's, then `f`'s) then got consumed one
    /// at a time across repeated recovery re-entries, and the outer item loop
    /// eventually mistook the commons's *own* closing `}` for having arrived
    /// early, returning zero items and a spurious second
    /// `bynk.parse.expected_unit_header` error. With brace-depth tracking, `g`
    /// is recovered as the sole item and only `f`'s own error is reported.
    /// #1663: an item that ends without its body (`fn f() -> Int` then the
    /// commons' own `}`) raises its error *on* that `}`, so recovery makes no
    /// progress. The no-progress step must not consume the brace that closes
    /// the body (brace form): doing so made the body look unclosed, a
    /// follow-on `unexpected_eof`.
    #[test]
    fn recovery_keeps_the_bodys_closing_brace_after_a_bodiless_item() {
        let src = "commons m {\n  fn f() -> Int\n}\n";
        let (unit, errors) = parse_recover_str(src);
        assert!(unit.is_some(), "recovery should produce a partial AST");
        let categories: Vec<_> = errors.iter().map(|e| e.category).collect();
        assert_eq!(
            categories.len(),
            1,
            "one syntax error, no follow-on: {categories:?}"
        );
        assert!(
            !categories.contains(&"bynk.parse.unexpected_eof"),
            "the commons' closing brace must survive: {categories:?}"
        );
    }

    /// #1663: an item keyword illegal at this position (`agent` in a commons)
    /// is skipped *with* its name and `{ … }` body, not one token at a time —
    /// otherwise the item loop misreads the agent's name as a malformed item,
    /// a second, follow-on `expected_item`. The next item still parses.
    #[test]
    fn recovery_skips_a_rejected_item_whole() {
        let src = "commons m\n\nagent Counter {\n  key id: String\n}\n\nfn g() -> Int { 2 }\n";
        let recovered = parse_units_recovering(&crate::lexer::tokenize(src).unwrap(), src);
        assert_eq!(
            recovered.errors.len(),
            1,
            "one syntax error, no follow-on: {:?}",
            recovered
                .errors
                .iter()
                .map(|e| e.category)
                .collect::<Vec<_>>()
        );
        let Some(SourceUnit::Commons(c)) = recovered.units.first() else {
            panic!("expected commons")
        };
        assert!(
            c.items.iter().any(|i| matches!(i, CommonsItem::Fn(_))),
            "the `fn g` after the skipped agent must still parse"
        );
        assert_eq!(recovered.broken_decl_names, ["Counter"]);
    }

    #[test]
    fn recovery_skips_a_nested_blocks_own_closing_brace() {
        let src = "commons m {\n  \
                   fn f() -> Int {\n    \
                   match 1 {\n      \
                   is 1 -> { let z = }\n      \
                   is _ -> 2\n    \
                   }\n  \
                   }\n  \
                   fn g() -> Int { 2 }\n\
                   }\n";
        let (unit, errors) = parse_recover_str(src);
        let unit = unit.expect("recovery should produce a partial AST");
        let SourceUnit::Commons(c) = unit else {
            panic!("expected commons")
        };
        let names: Vec<_> = c
            .items
            .iter()
            .filter_map(|i| match i {
                CommonsItem::Fn(f) => match &f.name {
                    FnName::Free(id) => Some(id.name.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            vec!["g".to_string()],
            "g must still be recovered as an item; got {names:?}"
        );
        assert!(
            !errors
                .iter()
                .any(|e| e.category == "bynk.parse.expected_unit_header"),
            "the outer body's own closing brace must not be mistaken for \
             end-of-file: {errors:?}"
        );
    }

    #[test]
    fn doc_block_attaches_to_type() {
        let c =
            parse_str("commons x {\n---\nA descriptive doc.\n---\ntype T = Int where Positive\n}")
                .unwrap();
        let CommonsItem::Type(t) = &c.items[0] else {
            panic!()
        };
        assert!(t.documentation.is_some());
        assert!(
            t.documentation
                .as_ref()
                .unwrap()
                .contains("A descriptive doc.")
        );
    }

    #[test]
    fn interpolated_string_parses_into_parts() {
        // v0.43: `"Hi, \(name)!"` splits into chunk / hole / chunk.
        let c = parse_str("commons x\n\nfn f(name: String) -> String {\n  \"Hi, \\(name)!\"\n}\n")
            .unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!("expected fn")
        };
        let ExprKind::InterpStr(parts) = &f.body.tail.kind else {
            panic!("expected InterpStr, got {:?}", f.body.tail.kind)
        };
        assert_eq!(parts.len(), 3);
        assert!(matches!(&parts[0], InterpPart::Chunk(s) if s == "Hi, "));
        assert!(
            matches!(&parts[1], InterpPart::Hole(h) if matches!(&h.kind, ExprKind::Ident(id) if id.name == "name"))
        );
        assert!(matches!(&parts[2], InterpPart::Chunk(s) if s == "!"));
    }

    #[test]
    fn interpolated_hole_parses_a_full_expression() {
        // A hole holds an arbitrary expression, not just an identifier.
        let c =
            parse_str("commons x\n\nfn f(a: Int, b: Int) -> String {\n  \"sum = \\(a + b)\"\n}\n")
                .unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!("expected fn")
        };
        let ExprKind::InterpStr(parts) = &f.body.tail.kind else {
            panic!("expected InterpStr")
        };
        assert!(matches!(&parts[1], InterpPart::Hole(h) if matches!(&h.kind, ExprKind::BinOp(..))));
    }

    #[test]
    fn empty_interpolation_hole_is_rejected() {
        let errs = parse_str("commons x\n\nfn f() -> String {\n  \"\\()\"\n}\n").unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.category == "bynk.parse.empty_interpolation"),
            "expected empty_interpolation; got {errs:?}"
        );
    }

    #[test]
    fn interpolation_hole_lex_error_span_is_rebased() {
        // #716: a lex error inside a `\(…)` hole once carried a span relative to
        // the hole substring — never rebased by `hole.start` — so it pointed at
        // the file's opening bytes and could split a multibyte char, tripping
        // the char-boundary invariant. The error must land on the offending
        // bytes within the hole and stay on char boundaries.
        let cases = [
            // `$` is not a valid token; the error should point at it, not byte 0.
            "commons x\n\nfn f() -> String {\n  \"a \\($)\"\n}\n",
            // Integer overflow — the reported span must cover the literal itself.
            "commons x\n\nfn f() -> String {\n  \"n = \\(99999999999999999999)\"\n}\n",
            // A multibyte char before the hole means an un-rebased span could
            // land inside the `é`; the rebased span must not.
            "commons x\n\nfn f() -> String {\n  \"é \\($)\"\n}\n",
        ];
        for src in cases {
            let errs = parse_str(src).unwrap_err();
            assert!(!errs.is_empty(), "expected a lex error for {src:?}");
            for e in &errs {
                assert!(
                    src.is_char_boundary(e.span.start) && src.is_char_boundary(e.span.end),
                    "span {:?} splits a codepoint in {src:?}",
                    e.span,
                );
                // The error must point inside the interpolation hole, not at the
                // header text that precedes it.
                let hole_start = src.find("\\(").expect("case has a hole") + 2;
                assert!(
                    e.span.start >= hole_start,
                    "span {:?} precedes the hole (starts at {hole_start}) in {src:?}",
                    e.span,
                );
            }
        }
    }

    #[test]
    fn fragment_form_parses() {
        let c = parse_str("commons x.y\n\ntype T = Int where NonNegative\n").unwrap();
        assert_eq!(c.form, CommonsForm::Fragment);
        assert_eq!(c.items.len(), 1);
    }

    #[test]
    fn uses_parses() {
        let c = parse_str("commons x\n\nuses other.lib\n").unwrap();
        assert_eq!(c.uses.len(), 1);
        assert_eq!(c.uses[0].target.joined(), "other.lib");
    }

    fn parse_unit_str(src: &str) -> Result<SourceUnit, Vec<CompileError>> {
        let toks = tokenize(src).map_err(|e| vec![e])?;
        parse_unit(&toks, src)
    }

    #[test]
    fn minimal_context_parses() {
        let u = parse_unit_str("context commerce.orders {}").unwrap();
        let SourceUnit::Context(c) = u else {
            panic!("expected context");
        };
        assert_eq!(c.name.joined(), "commerce.orders");
        assert!(c.items.is_empty());
    }

    #[test]
    fn context_consumes_and_exports_parse() {
        let src = "context commerce.orders {\n  uses commerce.money\n  consumes commerce.payment\n  exports opaque { OrderId }\n  exports transparent { OrderError }\n  type OrderId = String where Matches(\"ORD-[0-9]+\")\n  type OrderError = enum { CartEmpty, BadInput }\n}";
        let u = parse_unit_str(src).unwrap();
        let SourceUnit::Context(c) = u else { panic!() };
        assert_eq!(c.uses.len(), 1);
        assert_eq!(c.consumes.len(), 1);
        assert_eq!(c.exports.len(), 2);
        assert_eq!(c.exports[0].kind, ExportKind::Type(Visibility::Opaque));
        assert_eq!(c.exports[1].kind, ExportKind::Type(Visibility::Transparent));
    }

    #[test]
    fn context_fragment_form_parses() {
        let src = "context x.y\n\nuses other.lib\nconsumes other.ctx\nexports opaque { T }\n\ntype T = Int where NonNegative\n";
        let u = parse_unit_str(src).unwrap();
        let SourceUnit::Context(c) = u else { panic!() };
        assert_eq!(c.form, CommonsForm::Fragment);
        assert_eq!(c.uses.len(), 1);
        assert_eq!(c.consumes.len(), 1);
        assert_eq!(c.exports.len(), 1);
    }

    #[test]
    fn opaque_type_parses() {
        let c = parse_str("commons x { type T = opaque Int where NonNegative }").unwrap();
        let CommonsItem::Type(t) = &c.items[0] else {
            panic!()
        };
        assert!(matches!(t.body, TypeBody::Opaque { .. }));
    }

    #[test]
    fn empty_commons() {
        let c = parse_str("commons fitness.units {}").unwrap();
        assert_eq!(c.name.joined(), "fitness.units");
        assert!(c.items.is_empty());
    }

    #[test]
    fn one_type_decl() {
        let c = parse_str("commons x { type Metres = Int where NonNegative }").unwrap();
        assert_eq!(c.items.len(), 1);
        let CommonsItem::Type(t) = &c.items[0] else {
            panic!()
        };
        assert_eq!(t.name.name, "Metres");
        match &t.body {
            TypeBody::Refined {
                base, refinement, ..
            } => {
                assert_eq!(*base, BaseType::Int);
                assert!(refinement.is_some());
            }
            _ => panic!("expected refined body"),
        }
    }

    #[test]
    fn function_decl() {
        let c = parse_str("commons x { fn add(a: Int, b: Int) -> Int { a + b } }").unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!()
        };
        assert_eq!(f.name.ident().name, "add");
        assert_eq!(f.params.len(), 2);
    }

    #[test]
    fn chained_comparison_is_error() {
        let errs = parse_str("commons x { fn f(a: Int, b: Int, c: Int) -> Bool { a < b < c } }")
            .unwrap_err();
        assert_eq!(errs[0].category, "bynk.parse.non_associative");
    }

    #[test]
    fn chained_equality_is_error() {
        let errs = parse_str("commons x { fn f(a: Int, b: Int, c: Int) -> Bool { a == b == c } }")
            .unwrap_err();
        assert_eq!(errs[0].category, "bynk.parse.non_associative");
    }

    /// Run `f` on a thread with a generous stack. The depth-guard tests build
    /// source that, *without* the guard, overflows — so if the guard ever
    /// regressed we want a clean assertion failure, not a `SIGABRT` that takes
    /// the whole test binary down. A large stack also absorbs the fat frames a
    /// debug build spends per recursion level (production release frames are
    /// ~9 KB/level, so `MAX_NESTING_DEPTH = 64` sits well inside a 1 MB stack;
    /// a debug frame is several times larger and would overflow libtest's
    /// default 2 MB test thread near the limit even though the guard fires).
    fn on_big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn deeply_nested_parens_are_bounded_not_overflowed() {
        // Without a depth guard the parenthesised-expression recursion
        // (`parse_primary` -> `parse_expr` -> …) overflows the stack and aborts
        // the process (#713). Well past the limit it must instead report a
        // bounded-depth diagnostic. The nesting is left open so the guard, not
        // a later `)`, is what stops the descent.
        let errs = on_big_stack(|| {
            let depth = crate::MAX_NESTING_DEPTH + 8;
            let src = format!(
                "commons x {{ fn f() -> Int {{ {}0{} }} }}",
                "(".repeat(depth),
                ")".repeat(depth),
            );
            parse_str(&src).unwrap_err()
        });
        assert_eq!(errs[0].category, "bynk.parse.nesting_too_deep");
    }

    #[test]
    fn deeply_nested_types_are_bounded_not_overflowed() {
        // The type parser self-recurses through generic type arguments
        // (`parse_type_ref` -> `parse_type_atom` -> `parse_type_ref`); the same
        // guard bounds it (#713). A right-nested `Result[Int, …]` in parameter
        // position drives that recursion.
        let errs = on_big_stack(|| {
            let depth = crate::MAX_NESTING_DEPTH + 8;
            let src = format!(
                "commons x {{ fn f(x: {}Int{}) -> Int {{ 0 }} }}",
                "Result[Int, ".repeat(depth),
                "]".repeat(depth),
            );
            parse_str(&src).unwrap_err()
        });
        assert_eq!(errs[0].category, "bynk.parse.nesting_too_deep");
    }

    #[test]
    fn deeply_nested_patterns_are_bounded_not_overflowed() {
        // Variant patterns are a third self-recursive descent (`parse_pattern`
        // -> `parse_pattern_binding` -> `parse_pattern`) that routes through
        // neither `parse_expr` nor `parse_type_ref`; without its own guard a
        // nested `Ok(Ok(…))` match arm reproduces the #713 crash.
        let errs = on_big_stack(|| {
            let depth = crate::MAX_NESTING_DEPTH + 8;
            let src = format!(
                "commons x {{ fn f(n: Int) -> Int {{ match n {{ {}n{} => 0 }} }} }}",
                "Ok(".repeat(depth),
                ")".repeat(depth),
            );
            parse_str(&src).unwrap_err()
        });
        assert_eq!(errs[0].category, "bynk.parse.nesting_too_deep");
    }

    #[test]
    fn nesting_below_the_limit_still_parses() {
        // The guard must not reject ordinary well-nested source: a paren-nested
        // expression comfortably under the limit still parses cleanly.
        let ok = on_big_stack(|| {
            let depth = crate::MAX_NESTING_DEPTH - 8;
            let src = format!(
                "commons x {{ fn f() -> Int {{ {}0{} }} }}",
                "(".repeat(depth),
                ")".repeat(depth),
            );
            parse_str(&src).is_ok()
        });
        assert!(ok, "well-nested source under the limit should parse");
    }

    #[test]
    fn let_statement_parses() {
        let c = parse_str("commons x { fn f(n: Int) -> Int { let y = n + 1\n y } }").unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!()
        };
        assert_eq!(f.body.statements.len(), 1);
        match &f.body.statements[0] {
            Statement::Let(l) => {
                assert_eq!(l.name.name, "y");
                assert!(l.type_annot.is_none());
            }
            _ => panic!("expected a pure `let` statement"),
        }
    }

    #[test]
    fn let_with_annotation() {
        let c = parse_str("commons x { fn f(n: Int) -> Int { let y: Int = n\n y } }").unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!()
        };
        match &f.body.statements[0] {
            Statement::Let(l) => assert!(l.type_annot.is_some()),
            _ => panic!("expected a pure `let` statement"),
        }
    }

    #[test]
    fn if_else_parses_as_expression() {
        let c = parse_str("commons x { fn f(b: Bool) -> Int { if b { 1 } else { 0 } } }").unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!()
        };
        assert!(matches!(f.body.tail.kind, ExprKind::If { .. }));
    }

    #[test]
    fn else_if_chain_parses() {
        let c = parse_str(
            "commons x { fn f(n: Int) -> Int { if n < 0 { -1 } else if n == 0 { 0 } else { 1 } } }",
        )
        .unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!()
        };
        let ExprKind::If { else_block, .. } = &f.body.tail.kind else {
            panic!()
        };
        // The else-branch is a block whose tail is another `If`.
        assert!(else_block.statements.is_empty());
        assert!(matches!(else_block.tail.kind, ExprKind::If { .. }));
    }

    #[test]
    fn ok_and_err_parse_as_expressions() {
        let c = parse_str("commons x { fn f(n: Int) -> Result[Int, String] { Ok(n) } }").unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!()
        };
        assert!(matches!(f.body.tail.kind, ExprKind::Ok(_)));

        let c =
            parse_str("commons x { fn f(n: Int) -> Result[Int, String] { Err(\"x\") } }").unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!()
        };
        assert!(matches!(f.body.tail.kind, ExprKind::Err(_)));
    }

    #[test]
    fn question_postfix_parses() {
        let c = parse_str(
            "commons x { type T = Int where Positive\n fn f(n: Int) -> Result[T, ValidationError] { let x = T.of(n)?\n Ok(x) } }",
        )
        .unwrap();
        let CommonsItem::Fn(f) = &c.items[1] else {
            panic!()
        };
        let Statement::Let(l) = &f.body.statements[0] else {
            panic!("expected a pure `let` statement");
        };
        assert!(matches!(l.value.kind, ExprKind::Question(_)));
    }

    #[test]
    fn constructor_call_parses() {
        let c = parse_str(
            "commons x { type T = Int where Positive\n fn f(n: Int) -> Result[T, ValidationError] { T.of(n) } }",
        )
        .unwrap();
        let CommonsItem::Fn(f) = &c.items[1] else {
            panic!()
        };
        // v0.2: T.of(n) parses as a MethodCall with receiver Ident("T"); the
        // checker reinterprets it as a static call by noticing T is a type.
        let ExprKind::MethodCall {
            receiver, method, ..
        } = &f.body.tail.kind
        else {
            panic!("expected MethodCall, got {:?}", f.body.tail.kind)
        };
        let ExprKind::Ident(id) = &receiver.kind else {
            panic!("expected receiver Ident");
        };
        assert_eq!(id.name, "T");
        assert_eq!(method.name, "of");
    }

    #[test]
    fn result_type_ref_parses() {
        let c = parse_str("commons x { fn f(n: Int) -> Result[Int, String] { Ok(n) } }").unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!()
        };
        assert!(matches!(f.return_type, TypeRef::Result(_, _, _)));
    }

    #[test]
    fn result_missing_arg_count_errors() {
        let errs = parse_str("commons x { fn f(n: Int) -> Result[Int] { Ok(n) } }").unwrap_err();
        assert_eq!(errs[0].category, "bynk.parse.generic_arg_count");
    }

    #[test]
    fn field_access_parses_in_v0_2() {
        // v0.2: field access is supported (the type checker validates the
        // field exists on the receiver's type). Parser-level acceptance:
        let c =
            parse_str("commons x { type R = { foo: Int }\n fn f(r: R) -> Int { r.foo } }").unwrap();
        let CommonsItem::Fn(f) = &c.items[1] else {
            panic!()
        };
        assert!(matches!(f.body.tail.kind, ExprKind::FieldAccess { .. }));
    }

    // -- v1.1 trivia attachment --

    #[test]
    fn leading_line_comment_attaches_to_next_decl() {
        let src = "commons x {\n// explain the type\ntype T = Int where NonNegative\n}";
        let c = parse_str(src).unwrap();
        let CommonsItem::Type(t) = &c.items[0] else {
            panic!()
        };
        assert_eq!(
            t.trivia.leading,
            vec![Comment::Line(" explain the type".to_string())]
        );
        assert!(t.trivia.trailing.is_none());
    }

    #[test]
    fn trailing_line_comment_attaches_to_prev_decl() {
        let src = "commons x {\ntype T = Int where NonNegative  // trailing note\n}";
        let c = parse_str(src).unwrap();
        let CommonsItem::Type(t) = &c.items[0] else {
            panic!()
        };
        assert!(t.trivia.leading.is_empty());
        assert_eq!(t.trivia.trailing.as_deref(), Some(" trailing note"));
    }

    #[test]
    fn grouped_leading_comments_attach_together() {
        let src = "commons x {\n// one\n// two\n// three\ntype T = Int where Positive\n}";
        let c = parse_str(src).unwrap();
        let CommonsItem::Type(t) = &c.items[0] else {
            panic!()
        };
        assert_eq!(
            t.trivia.leading,
            vec![
                Comment::Line(" one".to_string()),
                Comment::Line(" two".to_string()),
                Comment::Line(" three".to_string())
            ],
        );
    }

    #[test]
    fn comment_with_doc_block_keeps_both() {
        // Both `-- intro` and the doc block should attach to the type decl.
        let src = "commons x {\n// intro\n---\ndocs\n---\ntype T = Int where Positive\n}";
        let c = parse_str(src).unwrap();
        let CommonsItem::Type(t) = &c.items[0] else {
            panic!()
        };
        assert_eq!(t.trivia.leading, vec![Comment::Line(" intro".to_string())]);
        assert_eq!(t.documentation.as_deref(), Some("docs"));
    }

    #[test]
    fn messages_keyword_does_not_collide_with_a_commons_name_segment() {
        // `messages` is RESERVED_CONTEXTUAL (like `case`/`on`/`suite`), not a
        // hard keyword: `commons app.messages { ... }` — the design's own
        // natural naming choice for a bundle commons — must still parse.
        // (Caught during slice-1 implementation: a first pass made `messages`
        // a plain hard keyword and this exact name broke.)
        let src = "commons app.messages {\ntype T = Int where Positive\n}";
        let c = parse_str(src).unwrap();
        assert_eq!(c.name.joined(), "app.messages");
    }

    #[test]
    fn messages_decl_parses_tag_annotation_and_entries() {
        // message-bundles slice 1 (#859): the construct + doc/trivia wiring.
        let src = "commons app.messages {\n\
                   // intro\n\
                   ---\n\
                   docs\n\
                   ---\n\
                   messages \"en\" @reference {\n\
                   \"greeting\" => \"Hello, {name}!\"\n\
                   \"farewell\" => \"Bye\"\n\
                   } // trailing\n\
                   }";
        let c = parse_str(src).unwrap();
        let CommonsItem::Messages(m) = &c.items[0] else {
            panic!("expected a messages item, got {:?}", c.items[0]);
        };
        assert_eq!(m.tag, "en");
        assert_eq!(m.annotations.len(), 1);
        assert_eq!(m.annotations[0].name.name, "reference");
        assert!(m.annotations[0].args.is_empty());
        assert_eq!(m.entries.len(), 2);
        assert_eq!(m.entries[0].code, "greeting");
        assert_eq!(m.entries[0].template, "Hello, {name}!");
        assert_eq!(m.entries[1].code, "farewell");
        assert_eq!(m.entries[1].template, "Bye");
        assert_eq!(m.trivia.leading, vec![Comment::Line(" intro".to_string())]);
        assert_eq!(m.documentation.as_deref(), Some("docs"));
        assert_eq!(m.trivia.trailing.as_deref(), Some(" trailing"));
    }

    #[test]
    fn messages_decl_parses_with_no_annotation_and_no_entries() {
        // The parser stays permissive on annotation cardinality (zero-or-more)
        // — "exactly one `@reference`" is a checker concern (validate.rs), not
        // a parse error.
        let src = "commons app.messages {\nmessages \"en\" {\n}\n}";
        let c = parse_str(src).unwrap();
        let CommonsItem::Messages(m) = &c.items[0] else {
            panic!("expected a messages item, got {:?}", c.items[0]);
        };
        assert_eq!(m.tag, "en");
        assert!(m.annotations.is_empty());
        assert!(m.entries.is_empty());
    }

    #[test]
    fn messages_decl_parses_syntactically_inside_a_context_too() {
        // Commons-only legality is a checker concern (bynk.messages.outside_commons
        // in bynk-emit's project validation), not a parser rejection — mirrors
        // how `service`/`agent` already parse syntactically inside `adapter`
        // bodies for the same reason.
        let src = "context app.svc {\nmessages \"en\" @reference {\n\"a\" => \"b\"\n}\n}";
        let toks = tokenize(src).unwrap();
        let (unit, errors) = parse_unit_with_recovery(&toks, src);
        assert!(errors.is_empty(), "unexpected parse errors: {errors:?}");
        let Some(SourceUnit::Context(ctx)) = unit else {
            panic!("expected a context")
        };
        let CommonsItem::Messages(m) = &ctx.items[0] else {
            panic!("expected a messages item, got {:?}", ctx.items[0]);
        };
        assert_eq!(m.tag, "en");
    }

    #[test]
    fn comment_before_let_statement_attaches() {
        let src = "commons x {\nfn f(n: Int) -> Int {\n// pick a value\nlet y = n + 1\ny\n}\n}";
        let c = parse_str(src).unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!()
        };
        let Statement::Let(l) = &f.body.statements[0] else {
            panic!()
        };
        assert_eq!(
            l.trivia.leading,
            vec![Comment::Line(" pick a value".to_string())]
        );
    }

    #[test]
    fn comment_before_tail_attaches_to_block_tail() {
        let src = "commons x {\nfn f(n: Int) -> Int {\nlet y = n + 1\n// result\ny\n}\n}";
        let c = parse_str(src).unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!()
        };
        assert_eq!(
            f.body.tail_leading_comments,
            vec![Comment::Line(" result".to_string())],
        );
    }

    /// #637 Gap A: the contextual keywords `on` / `suite` / `case` are lexer
    /// tokens but `expect_ident` admits them as identifiers outside their one
    /// keyword position, so they are valid record-field and parameter names.
    /// The keyword reference now renders them as a distinct "contextual" tier
    /// rather than claiming (falsely) that they cannot be used as identifiers.
    #[test]
    fn contextual_keywords_are_valid_identifiers() {
        // Record field names.
        let c = parse_str("commons demo {\n  type R = { on: Int, suite: String, case: Bool }\n}")
            .expect("`on`/`suite`/`case` are valid field names");
        let CommonsItem::Type(_) = &c.items[0] else {
            panic!("expected a type decl")
        };

        // Function parameter names (the other `expect_ident` position).
        parse_str("commons demo {\n  fn f(on: Int, case: Int) -> Int { 0 }\n}")
            .expect("`on`/`case` are valid parameter names");

        // `suite` too, as a field name.
        parse_str("commons demo {\n  type R = { suite: Int }\n}")
            .expect("`suite` is a valid field name");
    }

    /// Drift guard: every alphabetic keyword the lexer declares must be
    /// classified by `is_reserved_keyword`, or be one of the *contextual*
    /// keywords `expect_ident` deliberately admits as identifiers
    /// (`on`/`suite`/`case`). Everything else in this codebase that can
    /// drift has a guard; this predicate had silently fallen 17 keywords
    /// behind, degrading the reserved-keyword diagnostic to the generic
    /// expected-token one.
    #[test]
    fn is_reserved_keyword_covers_every_lexer_keyword() {
        let lexer_src = include_str!("lexer.rs");
        let mut words = Vec::new();
        for line in lexer_src.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("#[token(\"")
                && let Some(word) = rest.split('"').next()
                && word.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                words.push(word.to_string());
            }
        }
        assert!(
            words.len() > 30,
            "keyword extraction looks broken: only {} words",
            words.len()
        );
        // Contextual keywords double as identifiers (see `expect_ident`); the
        // tier is single-sourced in `keywords::RESERVED_CONTEXTUAL`.
        use crate::keywords::RESERVED_CONTEXTUAL;
        let mut unclassified = Vec::new();
        for word in &words {
            let tokens = crate::lexer::tokenize(word).expect("keyword lexes");
            let kind = tokens.first().expect("keyword yields a token").kind;
            if !is_reserved_keyword(kind) && !RESERVED_CONTEXTUAL.contains(&word.as_str()) {
                unclassified.push(word.clone());
            }
        }
        assert!(
            unclassified.is_empty(),
            "keywords missing from is_reserved_keyword (add them, or document \
             them as contextual): {unclassified:?}"
        );
    }

    /// Finding #27/#30: pins `is_item_start` to exactly the keyword set
    /// `declarations.rs`'s six item loops (`parse_commons_brace`/`_fragment`,
    /// `parse_context_brace`/`_fragment`, `parse_test_brace`/`_fragment`) plus
    /// `parse_adapter_body` dispatch on, as of this writing — the drift this
    /// finding fixed (`Property`, `Actor`, `Event`, `Binding`, and the
    /// `adapter` unit keyword itself were all missing from
    /// `recover_to_top_item`'s old hand-written sync list, even though every
    /// one of them is a real item/unit start). Adding a new item keyword to
    /// any of those loops should mean deliberately updating this list too,
    /// not silently leaving recovery unable to resync at it.
    #[test]
    fn is_item_start_matches_the_pinned_keyword_set() {
        use TokenKind::*;
        let expected_true = [
            Commons, Context, Adapter, Suite, Type, Fn, Messages, Event, Asset, Embed, Const, Uses,
            Consumes, Exports, Capability, Provides, Service, Agent, Actor, Binding, Stub, Case,
            Property,
        ];
        for kind in expected_true {
            assert!(is_item_start(kind), "{kind:?} must be an item start");
        }
        let expected_false = [
            Ident, Plus, Minus, Colon, Dot, Eq, LBrace, RBrace, LParen, RParen, If, Else, Let,
            Where, True, False, Match, Is, On, Given,
        ];
        for kind in expected_false {
            assert!(!is_item_start(kind), "{kind:?} must not be an item start");
        }
    }

    /// Fuzz-found (#516): a context-only keyword at item position in a
    /// commons errors without consuming the token, and the recovery sync
    /// stops at exactly that keyword — without a progress guard the item
    /// loop re-reported the same error until memory ran out.
    #[test]
    fn recovery_makes_progress_on_context_only_keyword_in_commons() {
        let src = "commons demo\n\ncapability Logger {\n  fn log(m: String) -> Effect[()]\n}\n";
        let tokens = crate::lexer::tokenize(src).unwrap();
        let (unit, errors) = parse_unit_with_recovery(&tokens, src);
        assert!(unit.is_some(), "the commons header still parses");
        assert!(
            errors
                .iter()
                .any(|e| e.category == "bynk.capability.outside_context"),
            "the misplaced capability is reported: {errors:?}"
        );
        // Termination is the real assertion (this used to OOM); a bounded,
        // non-repeating error list is the observable proxy.
        assert!(errors.len() < 10, "recovery repeated itself: {errors:?}");
    }

    #[test]
    fn trailing_file_comment_becomes_unit_trailing() {
        // A comment after the last item but before EOF (fragment form)
        // becomes the commons body's trailing comments so the formatter
        // can preserve it.
        let src = "commons x\n\ntype T = Int where Positive\n// afterword\n";
        let c = parse_str(src).unwrap();
        assert_eq!(
            c.trailing_comments,
            vec![Comment::Line(" afterword".to_string())]
        );
    }

    #[test]
    fn trailing_file_comment_after_a_brace_form_commons_is_not_dropped() {
        // Regression: the brace form's item loop exits on `RBrace`, never
        // reaching the fragment form's end-of-input case that drains the
        // trivia table's epilogue — so a comment after the closing `}` was
        // silently discarded (and, per `epilogue_is_empty`'s debug_assert,
        // would panic a debug build instead of round-tripping through
        // `bynk-fmt`).
        let src = "commons x {\n  type T = Int where Positive\n}\n// afterword\n";
        let c = parse_str(src).unwrap();
        assert_eq!(
            c.trailing_comments,
            vec![Comment::Line(" afterword".to_string())]
        );
    }

    #[test]
    fn trailing_file_comment_after_a_brace_form_context_is_not_dropped() {
        // Same regression as the commons case, for `parse_context_brace`.
        let src = "context x {\n  type T = Int where Positive\n}\n// afterword\n";
        let SourceUnit::Context(c) = parse_unit_str(src).unwrap() else {
            panic!("expected context");
        };
        assert_eq!(
            c.trailing_comments,
            vec![Comment::Line(" afterword".to_string())]
        );
    }

    #[test]
    fn trailing_file_comment_after_a_brace_form_suite_is_not_dropped() {
        // Same regression as the commons case, for `parse_test_brace`.
        let src = "suite x {\n  case \"c\" {\n    expect 1 == 1\n  }\n}\n// afterword\n";
        let SourceUnit::Suite(s) = parse_unit_str(src).unwrap() else {
            panic!("expected suite");
        };
        assert_eq!(
            s.trailing_comments,
            vec![Comment::Line(" afterword".to_string())]
        );
    }

    /// Finding #30: unlike the three regressions just above,
    /// `parse_adapter_body`'s brace-closing path never called
    /// `take_epilogue` at all (not a regression from a shared pattern — it
    /// simply never had the call), so a comment after a brace-form adapter's
    /// closing `}` was silently dropped. The fragment form (no braces) was
    /// already correct.
    #[test]
    fn trailing_file_comment_after_a_brace_form_adapter_is_not_dropped() {
        let src = "adapter x {\n  binding \"./x.ts\"\n}\n// afterword\n";
        let SourceUnit::Adapter(a) = parse_unit_str(src).unwrap() else {
            panic!("expected adapter");
        };
        assert_eq!(
            a.trailing_comments,
            vec![Comment::Line(" afterword".to_string())]
        );
    }

    /// #2321: `includes [ … ]` follows the module, before `requires`; it is a
    /// contextual word, so a malformed or unclosed list is an error, not a panic.
    #[test]
    fn binding_includes_list_parses_before_requires() {
        let src = "adapter x {\n  binding \"./x.ts\" includes [\"./a.wasm\", \"b/c.wasm\"] requires { \"p\": \"^1\" }\n}\n";
        let SourceUnit::Adapter(a) = parse_unit_str(src).unwrap() else {
            panic!("expected adapter");
        };
        let b = a.binding.unwrap();
        let paths: Vec<_> = b.includes.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["./a.wasm", "b/c.wasm"]);
        assert_eq!(b.requires.len(), 1);
        assert!(parse_unit_str("adapter x {\n  binding \"./x.ts\" includes [\"a.wasm\"").is_err());
        assert!(parse_unit_str("adapter x {\n  binding \"./x.ts\" includes [a]\n}\n").is_err());
    }

    // -- Six-fold unification (review Part 3): the fragment-only ordering
    // restrictions declarations.rs's brace/fragment pairs preserve, now that
    // they share one function each behind `brace: bool`. None of these had
    // any prior test coverage at all. --

    /// Fragment-form commons: `uses` must precede every `type`/`fn`.
    #[test]
    fn commons_fragment_rejects_uses_after_a_decl() {
        let src = "commons x\n\ntype T = Int where Positive\nuses bynk.list\n";
        let errs = parse_str(src).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.category == "bynk.parse.uses_after_decls"),
            "{errs:?}"
        );
    }

    /// The same ordering is NOT enforced in brace form — `uses` may appear
    /// anywhere in the body.
    #[test]
    fn commons_brace_allows_uses_after_a_decl() {
        let src = "commons x {\n  type T = Int where Positive\n  uses bynk.list\n}\n";
        parse_str(src).expect("brace form must not enforce fragment's uses-ordering rule");
    }

    /// Fragment-form context: `consumes` must precede every `type`/`fn`/etc.
    #[test]
    fn context_fragment_rejects_consumes_after_a_decl() {
        let src = "context x\n\ntype T = Int where Positive\nconsumes bynk\n";
        let errs = parse_unit_str(src).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.category == "bynk.parse.consumes_after_decls"),
            "{errs:?}"
        );
    }

    /// Fragment-form context: `exports` must precede every `type`/`fn`/etc.
    #[test]
    fn context_fragment_rejects_exports_after_a_decl() {
        let src = "context x\n\ntype T = Int where Positive\nexports opaque { T }\n";
        let errs = parse_unit_str(src).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.category == "bynk.parse.exports_after_decls"),
            "{errs:?}"
        );
    }

    /// Brace-form context enforces none of the three orderings.
    #[test]
    fn context_brace_allows_consumes_and_exports_after_a_decl() {
        let src = "context x {\n  type T = Int where Positive\n  consumes bynk\n  exports opaque { T }\n}\n";
        parse_unit_str(src)
            .expect("brace form must not enforce fragment's consumes/exports-ordering rules");
    }

    /// Fragment-form suite/test: `uses` must precede every `stub`/`case`/`property`.
    #[test]
    fn test_fragment_rejects_uses_after_a_decl() {
        let src = "suite m\n\ncase \"c\" {\n  expect true\n}\nuses bynk.list\n";
        let errs = parse_unit_str(src).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.category == "bynk.parse.uses_after_decls"),
            "{errs:?}"
        );
    }

    /// Brace-form suite/test allows `uses` anywhere.
    #[test]
    fn test_brace_allows_uses_after_a_decl() {
        let src = "suite m {\n  case \"c\" {\n    expect true\n  }\n  uses bynk.list\n}\n";
        parse_unit_str(src).expect("brace form must not enforce fragment's uses-ordering rule");
    }

    // ---- #636: `if`/`match` condition vs record construction ----

    /// Parse `body` as the tail expression of a fn and return its kind.
    fn body_tail(body: &str) -> ExprKind {
        let src = format!("commons x\n\nfn f() -> Int {{\n  {body}\n}}\n");
        let c = parse_str(&src).unwrap_or_else(|e| panic!("parse failed for {body:?}: {e:?}"));
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!("expected fn, got {:?}", c.items[0]);
        };
        f.body.tail.kind.clone()
    }

    fn body_err(body: &str) -> Vec<CompileError> {
        let src = format!("commons x\n\nfn f() -> Int {{\n  {body}\n}}\n");
        parse_str(&src).expect_err(&format!("expected a parse error for {body:?}"))
    }

    // ---- #2641: record patterns ----

    /// Apply a parse error's first suggestion to `src`.
    fn apply_fix(src: &str, err: &CompileError) -> String {
        let s = &err.suggestions[0];
        assert_eq!(
            s.applicability,
            crate::error::Applicability::MachineApplicable
        );
        let mut edits = s.edits.clone();
        edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
        let mut out = src.to_string();
        for (span, text) in edits {
            out.replace_range(span.start..span.end, &text);
        }
        out
    }

    #[test]
    fn record_pattern_parses_in_a_match_arm_and_nested() {
        let ExprKind::Match { arms, .. } = body_tail(
            "match o { Order { status: Paid, total, .. } => 1 Some(Order { .. }) => 2 _ => 3 }",
        ) else {
            panic!("expected Match");
        };
        let Pattern::Record {
            type_name, fields, ..
        } = &arms[0].pattern
        else {
            panic!("expected a record pattern, got {:?}", arms[0].pattern);
        };
        assert_eq!(type_name.name, "Order");
        assert_eq!(fields.len(), 2);
        assert!(fields[0].1.is_some(), "`status: Paid` has a sub-pattern");
        assert!(fields[1].1.is_none(), "`total` is shorthand");
        let Pattern::Variant { bindings, .. } = &arms[1].pattern else {
            panic!("expected Some(..)");
        };
        assert!(matches!(bindings[0].pattern(), Pattern::Record { .. }));
    }

    #[test]
    fn record_pattern_without_rest_is_an_error_with_a_fix() {
        for body in [
            "match o { Order { total } => 1 }",
            "match o { Order { total, } => 1 }",
            "match o { Order { status: Paid, total } => 1 }",
            "match o { Order { } => 1 }",
        ] {
            let errs = body_err(body);
            assert_eq!(errs[0].category, "bynk.parse.record_pattern_without_rest");
            let src = format!("commons x\n\nfn f() -> Int {{\n  {body}\n}}\n");
            let fixed = apply_fix(&src, &errs[0]);
            parse_str(&fixed).unwrap_or_else(|e| panic!("{body:?} fixed to {fixed:?}: {e:?}"));
        }
    }

    #[test]
    fn record_pattern_after_is_must_be_parenthesised() {
        for body in [
            "if o is Order { status: Paid, .. } { 1 } else { 2 }",
            "if o is None | Order { .. } { 1 } else { 2 }",
        ] {
            let errs = body_err(body);
            assert_eq!(
                errs[0].category,
                "bynk.parse.record_pattern_in_is_unparenthesised"
            );
            let src = format!("commons x\n\nfn f() -> Int {{\n  {body}\n}}\n");
            if body.contains("None |") {
                continue; // the fix wraps only the record alternative
            }
            let fixed = apply_fix(&src, &errs[0]);
            parse_str(&fixed).unwrap_or_else(|e| panic!("{body:?} fixed to {fixed:?}: {e:?}"));
        }
        // Parenthesised, nested, and a unit variant before the block all parse.
        for body in [
            "if o is (Order { status: Paid, .. }) { 1 } else { 2 }",
            "if o is Some(Order { .. }) { 1 } else { 2 }",
            "if o is (None | Order { .. }) { 1 } else { 2 }",
            "if o is Paid { 1 } else { 2 }",
        ] {
            body_tail(body);
        }
    }

    #[test]
    fn if_condition_ending_in_ident_does_not_swallow_a_single_ident_branch() {
        // #636: `ready { result }` shares its shape with a shorthand-field
        // record construction. In condition position the branch must win.
        for src in [
            "if ready { result } else { fallback }",
            "if ready { fallback } else { result }",
            "if !ready { result } else { fallback }",
            "if a == b { result } else { fallback }",
            "if a && b { result } else { fallback }",
        ] {
            let ExprKind::If {
                then_block,
                else_block,
                ..
            } = body_tail(src)
            else {
                panic!("expected If for {src:?}, got {:?}", body_tail(src));
            };
            // Both branches carry a bare-identifier tail — proof the `{ … }`
            // was read as a block, not consumed as a record by the condition.
            assert!(
                matches!(&then_block.tail.kind, ExprKind::Ident(_)),
                "then-branch tail not an ident for {src:?}: {:?}",
                then_block.tail.kind,
            );
            assert!(
                matches!(&else_block.tail.kind, ExprKind::Ident(_)),
                "else-branch tail not an ident for {src:?}: {:?}",
                else_block.tail.kind,
            );
        }
    }

    #[test]
    fn else_less_if_with_single_ident_branch_parses() {
        // The no-`else` reproduction: previously errored `found `}``.
        let ExprKind::If { then_block, .. } = body_tail("if ready { result }") else {
            panic!("expected If");
        };
        assert!(matches!(&then_block.tail.kind, ExprKind::Ident(_)));
    }

    #[test]
    fn record_construction_still_parses_in_value_position() {
        // The restriction is confined to condition spines — an ordinary value
        // position still constructs records, including the shorthand tail form.
        assert!(matches!(
            body_tail("Point { x }"),
            ExprKind::RecordConstruction { .. }
        ));
        assert!(matches!(
            body_tail("Point { x: 1, y: 2 }"),
            ExprKind::RecordConstruction { .. }
        ));
        assert!(matches!(
            body_tail("Empty {}"),
            ExprKind::RecordConstruction { .. }
        ));
    }

    #[test]
    fn parenthesised_record_is_allowed_in_condition_head() {
        // A delimiter lifts the restriction: `(ready { result })` constructs a
        // record even in condition position (mirrors Rust's paren escape).
        let ExprKind::If { cond, .. } =
            body_tail("if (ready { result }) { branch } else { other }")
        else {
            panic!("expected If");
        };
        let ExprKind::Paren(inner) = &cond.kind else {
            panic!("expected a parenthesised condition, got {:?}", cond.kind);
        };
        assert!(
            matches!(&inner.kind, ExprKind::RecordConstruction { .. }),
            "parenthesised record in condition head should still construct: {:?}",
            inner.kind,
        );
    }

    #[test]
    fn record_in_call_arg_within_condition_still_constructs() {
        // The restriction is lifted through a call-argument delimiter, so a
        // record literal passed to a predicate in the condition still parses.
        let ExprKind::If { cond, .. } = body_tail("if check(Point { x: 1 }) { a } else { b }")
        else {
            panic!("expected If");
        };
        let ExprKind::Call { args, .. } = &cond.kind else {
            panic!("expected Call in condition, got {:?}", cond.kind);
        };
        assert!(matches!(&args[0].kind, ExprKind::RecordConstruction { .. }));
    }

    #[test]
    fn safe_condition_shapes_are_unaffected() {
        // Cases the issue lists as already-safe must stay safe.
        assert!(matches!(
            body_tail("if ready == true { result } else { fallback }"),
            ExprKind::If { .. }
        ));
        assert!(matches!(
            body_tail("if (ready) { result } else { fallback }"),
            ExprKind::If { .. }
        ));
        assert!(matches!(
            body_tail("if ready { \"a\" } else { \"b\" }"),
            ExprKind::If { .. }
        ));
    }

    fn body_block(body: &str) -> Block {
        let src = format!("commons x\n\nfn f() -> Int {{\n  {body}\n}}\n");
        let c = parse_str(&src).unwrap_or_else(|e| panic!("parse failed for {body:?}: {e:?}"));
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!("expected fn, got {:?}", c.items[0]);
        };
        f.body.clone()
    }

    fn has_error(errs: &[CompileError], code: &str) -> bool {
        errs.iter().any(|e| e.category == code)
    }

    #[test]
    fn let_else_desugars_to_a_hidden_binding_and_a_match() {
        // #2010: `let P = e else E; rest` is `let __v = e` then
        // `match __v { P => { rest }, _ => E }`.
        let b = body_block("let x = 1\n  let Some(n) = find(x) else { 0 }\n  n + x");
        assert_eq!(
            b.statements.len(),
            2,
            "the plain let and the hidden binding"
        );
        let Statement::Let(hidden) = &b.statements[1] else {
            panic!("expected a pure hidden binding, got {:?}", b.statements[1]);
        };
        assert!(
            is_generated_binding(&hidden.name.name),
            "{}",
            hidden.name.name
        );
        assert_eq!(
            hidden.name.span.start, hidden.name.span.end,
            "the binder has no extent"
        );
        let le = hidden
            .let_else
            .as_ref()
            .expect("the binding records the sugar");
        assert_eq!(le.form, LetElseForm::Block);
        let ExprKind::Match { discriminant, arms } = &b.tail.kind else {
            panic!("expected a match tail, got {:?}", b.tail.kind);
        };
        assert!(matches!(&discriminant.kind, ExprKind::Ident(i) if i.name == hidden.name.name));
        assert_eq!(arms.len(), 2);
        assert!(matches!(arms[0].pattern, Pattern::Variant { .. }));
        let MatchBody::Block(rest) = &arms[0].body else {
            panic!("the pattern's arm runs the rest of the block");
        };
        assert!(rest.statements.is_empty());
        assert!(
            !matches!(rest.tail.kind, ExprKind::UnitLit),
            "{:?}",
            rest.tail.kind
        );
        assert!(matches!(arms[1].pattern, Pattern::Wildcard(_)));
    }

    /// #2640: `par { … }` parses to a `Par` holding its statements in order.
    #[test]
    fn par_block_holds_its_statements_in_source_order() {
        let b = body_block(
            "par {\n    let a <- one()\n    do two()\n    let b: Int <- three()\n  }\n  a",
        );
        let Statement::Par(p) = &b.statements[0] else {
            panic!("expected a par block, got {:?}", b.statements[0]);
        };
        assert_eq!(p.statements.len(), 3);
        assert!(matches!(&p.statements[0], Statement::EffectLet(l) if l.name.name == "a"));
        assert!(matches!(&p.statements[1], Statement::Do(_)));
        assert!(
            matches!(&p.statements[2], Statement::EffectLet(l) if l.name.name == "b" && l.type_annot.is_some())
        );
    }

    /// #2640: `par` is contextual — an identifier everywhere but before `{`.
    #[test]
    fn par_is_an_identifier_outside_its_statement_shape() {
        let b = body_block("let par = 1\n  let x = par + 1\n  x");
        assert_eq!(b.statements.len(), 2);
        assert!(matches!(&b.statements[0], Statement::Let(l) if l.name.name == "par"));
    }

    /// #2640: the parser admits any statement in a block (the checker names the
    /// forbidden ones), except those it cannot hold at all.
    #[test]
    fn par_block_admits_statements_for_the_checker_to_refuse() {
        let b = body_block(
            "par {\n    let a = 1\n    ~> notify()\n    par {\n      do x()\n    }\n  }\n  1",
        );
        let Statement::Par(p) = &b.statements[0] else {
            panic!("expected a par block");
        };
        assert!(matches!(&p.statements[0], Statement::Let(_)));
        assert!(matches!(&p.statements[1], Statement::Send(_)));
        assert!(matches!(&p.statements[2], Statement::Par(_)));
    }

    #[test]
    fn par_block_refuses_a_refutable_let_and_a_stray_expression() {
        assert!(has_error(
            &body_err("par {\n    let Ok(v) <- load() else { 0 }\n  }\n  1"),
            "bynk.par.invalid_statement"
        ));
        assert!(has_error(
            &body_err("par {\n    load()\n  }\n  1"),
            "bynk.par.invalid_statement"
        ));
    }

    #[test]
    fn effectful_let_else_binds_with_an_arrow() {
        let b = body_block("let Ok(v) <- load() else { Effect.pure(0) }\n  v");
        assert!(matches!(&b.statements[0], Statement::EffectLet(l) if l.let_else.is_some()));
    }

    #[test]
    fn let_else_chain_nests_each_step_in_the_previous_arm() {
        // Three steps in one flat block: each later step lives in the earlier
        // one's pattern arm, and the plain `let` between them stays in order.
        let b = body_block(
            "let Ok(a) = one() else { 1 }\n  let y = a\n  let Ok(b) = two(y) else { 2 }\n  let Ok(c) = three(b) else { 3 }\n  c",
        );
        let ExprKind::Match { arms, .. } = &b.tail.kind else {
            panic!("expected a match tail");
        };
        let MatchBody::Block(second) = &arms[0].body else {
            panic!("expected a block arm");
        };
        assert_eq!(
            second.statements.len(),
            2,
            "`let y` and the second hidden binding"
        );
        assert!(matches!(&second.statements[0], Statement::Let(l) if l.name.name == "y"));
        let ExprKind::Match { arms, .. } = &second.tail.kind else {
            panic!("expected the second step's match");
        };
        let MatchBody::Block(third) = &arms[0].body else {
            panic!("expected a block arm");
        };
        let ExprKind::Match { arms, .. } = &third.tail.kind else {
            panic!("expected the third step's match");
        };
        let MatchBody::Block(last) = &arms[0].body else {
            panic!("expected a block arm");
        };
        assert!(matches!(&last.tail.kind, ExprKind::Ident(i) if i.name == "c"));
    }

    #[test]
    fn else_match_arms_follow_the_patterns_arm() {
        let b = body_block("let Ok(v) = r else match { Err(e) => 0 }\n  v");
        let Statement::Let(l) = &b.statements[0] else {
            panic!("expected the hidden binding");
        };
        assert_eq!(l.let_else.as_ref().unwrap().form, LetElseForm::Match);
        let ExprKind::Match { arms, .. } = &b.tail.kind else {
            panic!("expected a match tail");
        };
        assert_eq!(arms.len(), 2);
        assert!(
            matches!(&arms[1].pattern, Pattern::Variant { variant, .. } if variant.name == "Err")
        );
    }

    #[test]
    fn an_irrefutable_record_let_needs_no_else() {
        // #2654: `let T { .. } = e` is the one-arm `match` of a `let … else`
        // with no `else` arm, marked `LetElseForm::Plain`.
        for body in [
            "let Item { sku, qty: n, .. } = item\n  n",
            "let (Item { sku, .. }) = item\n  sku",
            "let Item { sku: _, inner: Inner { x, .. }, .. } <- load(1)\n  x",
            "let Item { sku, .. } <- load(1) by Admin(\"a\")\n  sku",
        ] {
            let b = body_block(body);
            let (Statement::Let(l) | Statement::EffectLet(l)) = &b.statements[0] else {
                panic!("{body:?}: expected the hidden binding");
            };
            let le = l.let_else.as_ref().expect("the binding records the sugar");
            assert_eq!(le.form, LetElseForm::Plain, "{body:?}");
            assert_eq!(le.else_span.start, le.else_span.end, "no `else` extent");
            let ExprKind::Match { arms, .. } = &b.tail.kind else {
                panic!("{body:?}: expected a match tail");
            };
            assert_eq!(arms.len(), 1, "{body:?}: only the pattern's arm");
            assert!(matches!(arms[0].pattern, Pattern::Record { .. }));
        }
    }

    #[test]
    fn a_refutable_record_let_without_else_names_the_refutable_field() {
        for (body, field) in [
            (
                "let Order { status: Paid, total, .. } = o\n  total",
                "status: Paid",
            ),
            (
                "let Order { total, c: Cust { kind: Gold, .. }, .. } = o\n  total",
                "kind: Gold",
            ),
            ("let Order { id: 1, .. } <- load()\n  1", "id: 1"),
        ] {
            let errs = body_err(body);
            assert_eq!(errs[0].category, "bynk.parse.let_pattern_without_else");
            assert!(
                errs[0]
                    .notes
                    .iter()
                    .any(|n| n.contains(&format!("`{field}` can fail to match"))),
                "{body:?}: {:?}",
                errs[0].notes
            );
        }
        // Any other pattern without an `else` is refused as before, with no field named.
        for body in [
            "let Ok(n) = r\n  n",
            "let (Ok(n)) = r\n  n",
            "let Item { sku, .. } | Other = r\n  1",
        ] {
            let errs = body_err(body);
            assert_eq!(
                errs[0].category, "bynk.parse.let_pattern_without_else",
                "{body:?}"
            );
        }
    }

    #[test]
    fn a_record_let_keeps_an_else_when_one_is_written() {
        let b = body_block("let Item { sku, .. } = item else { 0 }\n  1");
        let Statement::Let(l) = &b.statements[0] else {
            panic!("expected the hidden binding");
        };
        assert_eq!(l.let_else.as_ref().unwrap().form, LetElseForm::Block);
    }

    #[test]
    fn a_capitalised_binding_is_not_a_record_pattern() {
        // `let Name = e` and `let Name: T = e` read `Name` as a binding, as before.
        for body in ["let Total = 1\n  Total", "let Total: Int = 1\n  Total"] {
            let b = body_block(body);
            let Statement::Let(l) = &b.statements[0] else {
                panic!("{body:?}: expected a plain let");
            };
            assert!(l.let_else.is_none(), "{body:?}");
        }
        // And `let Name {` is no longer an error at the brace: it opens a pattern.
        let errs = body_err("let Item { sku } = item\n  1");
        assert_eq!(errs[0].category, "bynk.parse.record_pattern_without_rest");
    }

    #[test]
    fn let_else_as_the_last_statement_leaves_a_unit_rest() {
        let b = body_block("let Some(n) = find(1) else { () }");
        let ExprKind::Match { arms, .. } = &b.tail.kind else {
            panic!("expected a match tail");
        };
        let MatchBody::Block(rest) = &arms[0].body else {
            panic!("expected a block arm");
        };
        assert!(rest.implicit_tail && rest.statements.is_empty());
    }

    #[test]
    fn let_pattern_forms_are_recognised() {
        for src in [
            "let None = o else { 0 }\n  1",
            "let Status.Done = s else { 0 }\n  1",
            "let Shipped(at) = s else { 0 }\n  at",
            "let 0 = n else { 1 }\n  0",
            "let -1 = n else { 1 }\n  0",
            "let \"a\" = s else { 1 }\n  0",
            "let (Ok(x) | Err(x)) = r else { 1 }\n  x",
        ] {
            let b = body_block(src);
            assert!(
                matches!(&b.statements[0], Statement::Let(l) if l.let_else.is_some()),
                "{src:?} should parse as a let-else"
            );
        }
        // A bare name, even capitalised, is still a binding.
        let b = body_block("let Total = 1\n  Total");
        assert!(
            matches!(&b.statements[0], Statement::Let(l) if l.let_else.is_none() && l.name.name == "Total")
        );
    }

    #[test]
    fn let_pattern_without_else_is_refused() {
        assert!(has_error(
            &body_err("let Ok(v) = r\n  v"),
            "bynk.parse.let_pattern_without_else"
        ));
        assert!(has_error(
            &body_err("let Ok(v) <- r()\n  v"),
            "bynk.parse.let_pattern_without_else"
        ));
    }

    #[test]
    fn else_on_a_plain_binding_is_refused() {
        assert!(has_error(
            &body_err("let v = r else { 0 }\n  v"),
            "bynk.parse.let_else_on_binding"
        ));
        assert!(has_error(
            &body_err("let v <- r() else { 0 }\n  v"),
            "bynk.parse.let_else_on_binding"
        ));
    }

    #[test]
    fn annotated_let_pattern_is_refused() {
        assert!(has_error(
            &body_err("let Ok(v): Int = r else { 0 }\n  v"),
            "bynk.parse.let_pattern_annotation"
        ));
    }

    #[test]
    fn empty_else_match_is_refused() {
        assert!(has_error(
            &body_err("let Ok(v) = r else match { }\n  v"),
            "bynk.parse.empty_match"
        ));
    }

    #[test]
    fn let_else_steps_count_against_the_nesting_budget() {
        // Each step nests the rest of the block one level deeper, exactly as
        // the hand-written `match` it stands for, so a very long chain is
        // refused with the nesting diagnostic rather than overflowing later.
        let chain = |n: usize| -> String {
            let steps: String = (0..n)
                .map(|i| format!("let Ok(v{i}) = step() else {{ {i} }}\n  "))
                .collect();
            format!("{steps}0")
        };
        body_block(&chain(40));
        let errs = body_err(&chain(400));
        assert!(
            errs.iter()
                .any(|e| e.category == "bynk.parse.nesting_too_deep"),
            "{errs:?}"
        );
    }

    #[test]
    fn empty_match_reports_its_own_diagnostic() {
        // #636: `match result {}` once parsed `result {}` as an empty record,
        // masking `bynk.parse.empty_match`. The intended diagnostic is now
        // reachable.
        let errs = body_err("match result {}");
        assert!(
            errs.iter().any(|e| e.category == "bynk.parse.empty_match"),
            "expected empty_match; got {errs:?}",
        );
    }

    #[test]
    fn match_discriminant_ending_in_ident_parses() {
        // A `match` over a bare-identifier discriminant reaches its arm list.
        assert!(matches!(
            body_tail("match ready { x => x }"),
            ExprKind::Match { .. }
        ));
    }

    /// #981: an identifier statement immediately followed, on its own line, by
    /// a standalone `()` must stay two separate constructs — not merge into a
    /// zero-arg call. `status := Paid` / `()` is an Assign statement whose
    /// value is the bare identifier `Paid`, then a unit tail; it must never
    /// parse as a single `status := Paid()` (a call). The call-parens rule
    /// mirrors the v0.20b same-line `[` rule already applied to type
    /// arguments: a postfix opener that begins a new line does not continue
    /// the previous token.
    #[test]
    fn identifier_statement_followed_by_unit_tail_does_not_merge_into_a_call() {
        let src = "commons c\n\nfn f() -> Int {\n  status := Paid\n  ()\n}\n";
        let c = parse_str(src).unwrap_or_else(|e| panic!("parse failed: {e:?}"));
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!("expected fn, got {:?}", c.items[0]);
        };
        assert_eq!(
            f.body.statements.len(),
            1,
            "expected exactly one Assign statement, got {:?}",
            f.body.statements
        );
        let Statement::Assign(a) = &f.body.statements[0] else {
            panic!(
                "expected an Assign statement, got {:?}",
                f.body.statements[0]
            );
        };
        assert!(
            matches!(a.value.kind, ExprKind::Ident(_)),
            "assign value must stay the bare identifier `Paid`, got {:?}",
            a.value.kind
        );
        assert!(
            matches!(f.body.tail.kind, ExprKind::UnitLit),
            "the `()` must remain the block's own tail, got {:?}",
            f.body.tail.kind
        );
    }

    /// #1858: a variant pattern's `(` must sit on the variant's line too. An
    /// `is` test ending in a nullary variant, above a `()` tail on its own
    /// line, is a pattern and a unit, not `Paid()`/`None()`: `fmt` writes that
    /// tail after a comment, and its reparse must not absorb it.
    #[test]
    fn variant_pattern_followed_by_unit_tail_does_not_take_it_as_bindings() {
        for (name, pat) in [("user variant", "Paid"), ("built-in variant", "None")] {
            let src = format!("commons c\n\nfn f() -> () {{\n  let b = r is {pat}\n  ()\n}}\n");
            let c = parse_str(&src).unwrap_or_else(|e| panic!("{name}: parse failed: {e:?}"));
            let CommonsItem::Fn(f) = &c.items[0] else {
                panic!("expected fn, got {:?}", c.items[0]);
            };
            let Statement::Let(l) = &f.body.statements[0] else {
                panic!("{name}: expected a Let, got {:?}", f.body.statements[0]);
            };
            let ExprKind::Is { pattern, .. } = &l.value.kind else {
                panic!("{name}: expected an `is` test, got {:?}", l.value.kind);
            };
            let Pattern::Variant { variant, span, .. } = pattern.as_ref() else {
                panic!("{name}: expected a variant pattern, got {pattern:?}");
            };
            assert_eq!(
                span.end, variant.span.end,
                "{name}: the pattern must end at `{pat}`, not take the `()` below it"
            );
            assert!(
                matches!(f.body.tail.kind, ExprKind::UnitLit),
                "{name}: the `()` must remain the block's own tail, got {:?}",
                f.body.tail.kind
            );
        }
    }

    /// #1858's boundary: a `(` on the variant's line still opens its payload
    /// list, and the list itself may span lines.
    #[test]
    fn variant_pattern_payload_on_the_same_line_still_binds() {
        for (name, pat) in [
            ("user variant", "Paid(\n    x\n  )"),
            ("built-in variant", "Some(x)"),
        ] {
            let src = format!("commons c\n\nfn f() -> Bool {{\n  r is {pat}\n}}\n");
            let c = parse_str(&src).unwrap_or_else(|e| panic!("{name}: parse failed: {e:?}"));
            let CommonsItem::Fn(f) = &c.items[0] else {
                panic!("expected fn, got {:?}", c.items[0]);
            };
            let ExprKind::Is { pattern, .. } = &f.body.tail.kind else {
                panic!("{name}: expected an `is` tail, got {:?}", f.body.tail.kind);
            };
            assert!(
                matches!(pattern.as_ref(), Pattern::Variant { bindings, .. } if bindings.len() == 1),
                "{name}: expected one binding, got {pattern:?}"
            );
        }
    }

    /// #1858, chosen uniformly: a match arm's pattern is followed only by `if`
    /// or `=>`, so a next-line `(` there could not be the next statement, but
    /// the same-line rule holds in every pattern position anyway. One lexical
    /// rule, wherever a pattern appears, is the one a reader can apply without
    /// knowing the position. The arm is rejected, not misparsed.
    #[test]
    fn match_arm_pattern_payload_on_a_later_line_is_rejected() {
        let src = "commons c\n\nfn f() -> Int {\n  match m {\n    Settled\n      (amount) => 1,\n    _ => 0,\n  }\n}\n";
        assert!(
            parse_str(src).is_err(),
            "a next-line `(` after an arm's variant must not open its payload list"
        );
        let same_line = src.replace("Settled\n      (amount)", "Settled(amount)");
        parse_str(&same_line).unwrap_or_else(|e| panic!("same-line form must parse: {e:?}"));
    }

    /// #981: the same same-line rule extends to a method call's parens — a
    /// `.method` immediately followed, on its own line, by a standalone `()`
    /// must not merge into `.method()`.
    #[test]
    fn method_reference_followed_by_unit_tail_does_not_merge_into_a_call() {
        let src = "commons c\n\nfn f() -> Int {\n  let y = x.field\n  ()\n}\n";
        let c = parse_str(src).unwrap_or_else(|e| panic!("parse failed: {e:?}"));
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!("expected fn, got {:?}", c.items[0]);
        };
        let Statement::Let(l) = &f.body.statements[0] else {
            panic!("expected a Let statement, got {:?}", f.body.statements[0]);
        };
        assert!(
            matches!(l.value.kind, ExprKind::FieldAccess { .. }),
            "let value must stay a field access, got {:?}",
            l.value.kind
        );
        assert!(
            matches!(f.body.tail.kind, ExprKind::UnitLit),
            "the `()` must remain the block's own tail, got {:?}",
            f.body.tail.kind
        );
    }

    #[test]
    fn unparenthesised_record_in_condition_head_now_errors() {
        // #636 narrowing (matches Rust): a record literal in condition *head*
        // position must be parenthesised. Unparenthesised, `Point` reads as the
        // discriminant and `{ x: 1 }` as the arm list, whose first "arm" `x: 1`
        // is not an arm — so the parse fails. Pinned so the divergence from the
        // (still-accepting) tree-sitter grammar is deliberate, not a bug.
        assert!(
            !body_err("match Point { x: 1 } { p => p }").is_empty(),
            "unparenthesised record discriminant should not parse",
        );
        // Parenthesised, the record is the discriminant and the match parses.
        let ExprKind::Match { discriminant, .. } = body_tail("match (Point { x: 1 }) { p => p }")
        else {
            panic!("expected Match for the parenthesised form");
        };
        let ExprKind::Paren(inner) = &discriminant.kind else {
            panic!(
                "expected a parenthesised discriminant, got {:?}",
                discriminant.kind
            );
        };
        assert!(matches!(&inner.kind, ExprKind::RecordConstruction { .. }));
    }

    /// The codes a strict multi-unit parse (the build's path) reports.
    fn strict_codes(src: &str) -> Vec<&'static str> {
        let toks = tokenize(src).unwrap();
        match parse_units_with_warnings(&toks, src) {
            Ok((_, warnings)) => warnings.iter().map(|e| e.category).collect(),
            Err(errs) => errs.iter().map(|e| e.category).collect(),
        }
    }

    /// #1885 (DECISION B): a divider pair around a declaration, attached to
    /// the next one, swallowed `helper` silently. Every parse entry point now
    /// reports it.
    #[test]
    fn a_divider_pair_around_code_is_doc_block_contains_code() {
        let src =
            "commons m\n\n--------\nfn helper() -> Int { 1 }\n--------\nfn f() -> Int { 2 }\n";
        assert_eq!(strict_codes(src), ["bynk.parse.doc_block_contains_code"]);
        let errs = parse_str(src).unwrap_err();
        assert_eq!(errs[0].category, "bynk.parse.doc_block_contains_code");
        // The span is the whole doc block.
        assert_eq!(
            &src[errs[0].span.range()],
            "--------\nfn helper() -> Int { 1 }\n--------\n"
        );
        let (unit, errs) = parse_recover_str(src);
        assert!(unit.is_some(), "recovery keeps the unit");
        assert!(
            errs.iter()
                .any(|e| e.category == "bynk.parse.doc_block_contains_code"),
            "{errs:?}"
        );
    }

    /// The orphan shape (a blank line before `fn f`) is the same error, beside
    /// the orphan warning; and so is a pair with nothing after it.
    #[test]
    fn an_orphaned_or_trailing_divider_pair_is_doc_block_contains_code() {
        let orphan = "commons m\n\n---\nfn helper() -> Int { 1 }\n---\n\nfn f() -> Int { 2 }\n";
        let codes = strict_codes(orphan);
        assert!(
            codes.contains(&"bynk.parse.doc_block_contains_code"),
            "{codes:?}"
        );
        let trailing = "commons m\n\nfn f() -> Int { 2 }\n\n---\ntype Id = Int\n---\n";
        let codes = strict_codes(trailing);
        assert!(
            codes.contains(&"bynk.parse.doc_block_contains_code"),
            "{codes:?}"
        );
    }

    /// Prose that only starts like a declaration, a doc whose code sits in a
    /// Markdown fence, and a doc that holds only a nested doc example are all
    /// documentation.
    #[test]
    fn near_miss_prose_and_fenced_code_are_not_code() {
        for doc in [
            "type of the thing",
            "fn is used here",
            "The type Id is an Int.",
            "uses the clock",
            "Event handler.\nfn helper() -> Int { 1 }",
            "Example:\n```bynk\nfn helper() -> Int { 1 }\n```",
            "~~~~\ntype Id = Int\n~~~~",
            "---\nA nested doc.\n---",
            "",
        ] {
            assert!(!doc_content_is_code(doc), "{doc:?} read as code");
        }
        for doc in [
            "fn helper() -> Int { 1 }",
            "type Id = Int\nfn f() -> Int { 1 }",
            "```\nan example\n```\nfn helper() -> Int { 1 }",
            "---\nA nested doc.\n---\nfn helper() -> Int { 1 }",
        ] {
            assert!(doc_content_is_code(doc), "{doc:?} not read as code");
        }
    }

    /// #1894 review: code one level down, inside a nested doc block, or
    /// beside a lone `---` rule that keeps the content from lexing, is still
    /// code; prose at either level is not.
    #[test]
    fn code_inside_a_nested_doc_or_beside_a_rule_is_code() {
        let nested =
            "commons m\n\n----\n---\nfn helper() -> Int { 1 }\n---\n----\nfn f() -> Int { 2 }\n";
        assert_eq!(strict_codes(nested), ["bynk.parse.doc_block_contains_code"]);
        let ruled = "commons m\n\n----\n---\nfn helper() -> Int { 1 }\n----\nfn f() -> Int { 2 }\n";
        assert_eq!(strict_codes(ruled), ["bynk.parse.doc_block_contains_code"]);
        // Two levels down.
        assert!(doc_content_is_code(
            "----\n---\nfn helper() -> Int { 1 }\n---\n----"
        ));
        for doc in [
            "---\nAn inner doc.\n---",
            "Intro.\n\n---\n\nfn is used here",
            "---\n```\nfn helper() -> Int { 1 }\n```\n---",
        ] {
            assert!(!doc_content_is_code(doc), "{doc:?} read as code");
        }
        // Deeper than the bound, the probe stops looking.
        let mut deep = "fn helper() -> Int { 1 }".to_string();
        for n in (3..3 + DOC_PROBE_MAX_DEPTH + 1).rev() {
            let fence = "-".repeat(n);
            deep = format!("{fence}\n{deep}\n{fence}");
        }
        assert!(!doc_content_is_code(&deep));
    }

    /// A `----` doc holding a nested doc example in a code fence attaches to
    /// its declaration with no diagnostic.
    #[test]
    fn a_fenced_nested_doc_example_parses_clean() {
        let src = "commons m\n\n----\nShows a doc.\n\n```bynk\n---\nInner.\n---\nfn example() -> Int { 1 }\n```\n----\nfn f() -> Int { 2 }\n";
        assert!(strict_codes(src).is_empty(), "{:?}", strict_codes(src));
        let c = parse_str(src).unwrap();
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!("expected fn")
        };
        assert!(
            f.documentation
                .as_deref()
                .is_some_and(|d| d.contains("---\nInner.\n---")),
            "{:?}",
            f.documentation
        );
    }

    /// #2618: `private` before a declaration keyword is the modifier, its span
    /// recorded and folded into the declaration's; a doc above it still
    /// attaches. Elsewhere `private` is an ordinary identifier.
    #[test]
    fn private_modifier_is_contextual() {
        let src = "commons m\n\n--- Hidden.\nprivate type T = Int\n\nprivate fn T.get(self) -> Int { 1 }\n\nfn f(private: Int) -> Int { private }\n";
        let c = parse_str(src).unwrap();
        let t = type_at(&c, 0);
        let at = src.find("private type").unwrap();
        assert_eq!(t.private.map(|s| s.start), Some(at));
        assert_eq!(t.span.start, at);
        assert_eq!(doc_of(&t.documentation), Some(("Hidden.", DocForm::Lines)));
        assert!(c.items[1].is_private());
        assert!(!c.items[2].is_private());
    }

    /// `private` before `const` is the modifier too, its span folded into the
    /// constant's.
    #[test]
    fn private_modifier_on_a_constant() {
        let src = "commons m\n\nprivate const Max: Int = 3\n\nconst Min: Int = 1\n";
        let c = parse_str(src).unwrap();
        let CommonsItem::Const(k) = &c.items[0] else {
            panic!("a constant");
        };
        assert_eq!(
            k.private.map(|s| s.start),
            Some(src.find("private").unwrap())
        );
        assert_eq!(k.span.start, src.find("private").unwrap());
        assert!(c.items[0].is_private());
        assert!(!c.items[1].is_private());
    }

    /// #2618: `private` on a declaration it does not apply to is rejected.
    #[test]
    fn private_on_a_service_is_rejected() {
        let src = "context m\n\nprivate service s {\n  on call() -> Effect[Int] { 1 }\n}\n";
        let tokens = crate::lexer::tokenize(src).unwrap();
        let errs = parse_units(&tokens, src).unwrap_err();
        assert_eq!(errs[0].category, "bynk.visibility.private_not_allowed");
    }

    /// The `TypeDecl` at `items[i]`.
    fn type_at(c: &Commons, i: usize) -> &TypeDecl {
        let CommonsItem::Type(t) = &c.items[i] else {
            panic!("expected a type at {i}")
        };
        t
    }

    fn doc_of(d: &Option<Doc>) -> Option<(&str, DocForm)> {
        // An indented `---` block keeps a trailing newline (its closing
        // marker's indent survives the marker trim); it prints the same.
        d.as_ref().map(|d| (d.as_str().trim_end(), d.form))
    }

    /// #2203 (DECISION C): a `--- text` line documents a declaration, with or
    /// without its closer.
    #[test]
    fn doc_lines_document_a_declaration() {
        let src = "commons m\n\n--- A commit SHA.\ntype Commit = String\n\n---- First. ----\nfn f() -> Int { 1 }\n";
        let c = parse_str(src).unwrap();
        assert_eq!(
            doc_of(&type_at(&c, 0).documentation),
            Some(("A commit SHA.", DocForm::Lines))
        );
        let CommonsItem::Fn(f) = &c.items[1] else {
            panic!("expected fn")
        };
        assert_eq!(doc_of(&f.documentation), Some(("First.", DocForm::Lines)));
        assert!(strict_codes(src).is_empty(), "{:?}", strict_codes(src));
    }

    /// #2203 (DECISION E): a legacy `--` comment and `--|` doc line still
    /// parse as a comment and a doc, and each is reported, all at once, with
    /// its fix; the formatter's migrating parse accepts them.
    #[test]
    fn legacy_spellings_parse_and_are_each_reported() {
        let src =
            "-- a header\ncommons m\n\n--| A run.\ntype Run = {\n  repo: String, -- trailing\n}\n";
        assert_eq!(
            strict_codes(src),
            [
                "bynk.lex.legacy_comment",
                "bynk.lex.legacy_doc_line",
                "bynk.lex.legacy_comment"
            ]
        );
        let tokens = tokenize(src).unwrap();
        let (units, _, _) = parse_units_migrating(&tokens, src).unwrap();
        let SourceUnit::Commons(c) = &units[0] else {
            panic!("commons")
        };
        assert_eq!(
            doc_of(&type_at(c, 0).documentation),
            Some(("A run.", DocForm::Lines))
        );
    }

    /// #2203 (DECISION D): two one-line docs on one target is
    /// `bynk.parse.multiple_docs`, whose fix joins them into a block.
    #[test]
    fn consecutive_doc_lines_are_multiple_docs_with_a_block_fix() {
        let src = "commons m\n\ntype R = {\n  --- First.\n  --- Second.\n  a: Int,\n}\n";
        let tokens = tokenize(src).unwrap();
        let errs = parse_units(&tokens, src).unwrap_err();
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(errs[0].category, "bynk.parse.multiple_docs");
        let (span, block) = &errs[0].suggestions[0].edits[0];
        assert_eq!(&src[span.range()], "--- First.\n  --- Second.");
        assert_eq!(block, "---\n  First.\n  Second.\n  ---");
    }

    /// #1888 (DECISION D): record fields, both sum forms' variants, payload
    /// fields and message entries take a doc in either form; a `//` comment
    /// above a field stays a comment.
    #[test]
    fn members_take_docs_in_either_form() {
        let src = "commons m\n\ntype Run = {\n  --- The repository.\n  repo: String,\n  ---\n  The version.\n  ---\n  version: String,\n  // a note\n  sha: String, // trailing\n}\n\ntype Outcome = enum {\n  --- Passed.\n  Pass,\n  Fail,\n}\n\ntype Shape =\n  --- Round.\n  | Circle(\n    --- The radius.\n    r: Int,\n  )\n  | Dot\n\n--- After the sum.\nfn f() -> Int { 1 }\n\nmessages \"en\" @reference {\n  --- Greets.\n  \"hi\" => \"Hello\"\n}\n";
        assert!(strict_codes(src).is_empty(), "{:?}", strict_codes(src));
        let c = parse_str(src).unwrap();
        let TypeBody::Record(r) = &type_at(&c, 0).body else {
            panic!("record")
        };
        assert_eq!(
            doc_of(&r.fields[0].documentation),
            Some(("The repository.", DocForm::Lines))
        );
        assert_eq!(
            doc_of(&r.fields[1].documentation),
            Some(("The version.", DocForm::Block))
        );
        assert_eq!(r.fields[2].documentation, None);
        assert_eq!(
            r.fields[2].trivia.leading,
            vec![Comment::Line(" a note".into())]
        );
        assert_eq!(r.fields[2].trivia.trailing.as_deref(), Some(" trailing"));
        let TypeBody::Sum(e) = &type_at(&c, 1).body else {
            panic!("enum")
        };
        assert_eq!(
            doc_of(&e.variants[0].documentation),
            Some(("Passed.", DocForm::Lines))
        );
        assert_eq!(e.variants[1].documentation, None);
        let TypeBody::Sum(p) = &type_at(&c, 2).body else {
            panic!("sum")
        };
        assert_eq!(
            doc_of(&p.variants[0].documentation),
            Some(("Round.", DocForm::Lines))
        );
        assert_eq!(
            doc_of(&p.variants[0].payload[0].documentation),
            Some(("The radius.", DocForm::Lines))
        );
        assert_eq!(p.variants[1].documentation, None);
        // The doc after the last variant is the next declaration's.
        let CommonsItem::Fn(f) = &c.items[3] else {
            panic!("expected fn")
        };
        assert_eq!(
            doc_of(&f.documentation),
            Some(("After the sum.", DocForm::Lines))
        );
        let CommonsItem::Messages(m) = &c.items[4] else {
            panic!("expected messages")
        };
        assert_eq!(
            doc_of(&m.entries[0].documentation),
            Some(("Greets.", DocForm::Lines))
        );
    }

    /// #2203 (DECISION D): two docs directly above one target, of either
    /// form or both, is `bynk.parse.multiple_docs`, on a declaration and on a
    /// member; the recovering parse keeps the unit.
    #[test]
    fn two_docs_on_one_target_is_multiple_docs() {
        for src in [
            "commons m\n\n---\nBlock.\n---\n--- Lines.\nfn f() -> Int { 1 }\n",
            "commons m\n\n--- Lines.\n---\nBlock.\n---\nfn f() -> Int { 1 }\n",
            "commons m\n\n---\nOne.\n---\n---\nTwo.\n---\nfn f() -> Int { 1 }\n",
            "commons m\n\n--- One.\n--- Two.\nfn f() -> Int { 1 }\n",
            "commons m\n\ntype R = {\n  --- Lines.\n  ---\n  Block.\n  ---\n  a: Int,\n}\n",
        ] {
            assert_eq!(strict_codes(src), ["bynk.parse.multiple_docs"], "{src}");
            let (unit, errs) = parse_recover_str(src);
            assert!(unit.is_some());
            assert!(
                errs.iter()
                    .any(|e| e.category == "bynk.parse.multiple_docs")
            );
        }
    }

    /// #1888 review: a doc with only a payload's `)` after it is an orphan
    /// warning, and the variant still parses with its field.
    #[test]
    fn a_dangling_doc_before_a_payload_close_is_only_an_orphan() {
        let src =
            "commons m\n\ntype S =\n  | Circle(\n    r: Int,\n    --- dangling\n  )\n  | Dot\n";
        assert_eq!(strict_codes(src), ["bynk.parse.orphan_doc_block"]);
        let c = parse_str(src).unwrap();
        let TypeBody::Sum(s) = &type_at(&c, 0).body else {
            panic!("sum")
        };
        assert_eq!(s.variants[0].payload.len(), 1);
        assert_eq!(s.variants[0].payload[0].documentation, None);
    }

    /// #1888: a member doc follows the declaration rules: a blank line
    /// orphans it (a warning, kept in place), as does the body's end.
    #[test]
    fn an_unattached_member_doc_is_an_orphan() {
        let src = "commons m\n\ntype R = {\n  --- Spaced.\n\n  a: Int,\n  --- Dangling.\n}\n";
        assert_eq!(
            strict_codes(src),
            ["bynk.parse.orphan_doc_block", "bynk.parse.orphan_doc_block"]
        );
        let c = parse_str(src).unwrap();
        let TypeBody::Record(r) = &type_at(&c, 0).body else {
            panic!("record")
        };
        assert_eq!(r.fields[0].documentation, None);
        assert!(matches!(
            &r.fields[0].trivia.leading[..],
            [Comment::OrphanDoc(d)] if d.as_str() == "Spaced." && d.form == DocForm::Lines
        ));
        assert!(matches!(
            &r.trailing_comments[..],
            [Comment::OrphanDoc(d)] if d.as_str() == "Dangling."
        ));
    }
}
