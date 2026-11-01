//! Bynk source formatter.
//!
//! Re-parses the source into an AST and re-prints it in canonical form per
//! the style rules in `design/bynk-lsp-spec.md` §3.5:
//!
//! - Tabs by default (one tab per nesting level).
//! - K&R brace style: opening brace on the same line as the construct header.
//! - Trailing commas in multi-line record / sum / parameter / argument lists.
//! - One blank line between top-level declarations.
//! - No blank lines between fields within a record or arms within a match.
//! - Doc blocks immediately above their declaration, no blank line between.
//! - One space around binary operators, after commas, no space inside parens.
//! - Soft 100-column line width — see below.
//!
//! Line width (#963). Every fit test measures the *whole* line: the column the
//! construct starts at, its own width, and the width of whatever the caller
//! will still emit after it (a `-> Ret {` signature tail, a closing `)`, a
//! match arm's `,`). A construct that does not fit is re-emitted vertically,
//! breaking only where the grammar tolerates a newline:
//!
//! - Record constructions, list literals, `exports`/`enum` bodies, and agent
//!   scheme configuration take one entry per line, with a trailing comma.
//! - Parameter and argument lists take one entry per line **without** a
//!   trailing comma — the grammar rejects one there.
//! - A `&&` / `||` / `implies` run breaks before each operator. Arithmetic and
//!   comparison operators never break: a continuation line opening with `+`
//!   does not re-attach to the line above on re-parse.
//! - A `.`-chain of two or more calls breaks before each call, unless the
//!   overflow belongs to a trailing argument that can open its body on the
//!   chain's own line (`xs.fold(init, (acc, x) => match acc {`).
//! - An `if` sends both branches vertical; a block sends its statements
//!   vertical.
//!
//! The target is soft: a construct with no break point inside it — a long
//! string literal, a `Matches("…")` regex — is left over-long rather than
//! mangled. Every layout choice is a function of the AST and the current
//! column, so the result is stable under re-formatting.
//!
//! The formatter is idempotent: format → format yields the same text.
//!
//! Comments (v1.1): line comments are preserved through the lexer-to-parser
//! trivia pipeline (lexer emits `Comment` tokens, parser attaches them to
//! AST declarations and statements). The formatter re-emits leading
//! comments above each node and a trailing comment, if any, on the same
//! line as the node's last token. Comments inside expression sub-trees
//! are not yet attached to individual operands; they are folded into the
//! enclosing statement's leading trivia. When even that would lose a
//! comment, [`format_source`] refuses with a `bynk.fmt.comment_loss`
//! diagnostic instead of dropping user text (#523) — the file is left
//! unchanged. See `design/bynk-lsp-spec.md` §3.5 for the canonical
//! comment-placement rules.

use bynk_syntax::ast::*;
use bynk_syntax::error::CompileError;
use bynk_syntax::lexer::{
    Token, TokenKind, comment_is_legacy, doc_fence_len, doc_line_is_legacy, one_line_doc, tokenize,
    tokenize_legacy,
};
use bynk_syntax::parser::parse_units_migrating;
use bynk_syntax::span::Span;

/// Indentation style: tabs or spaces. Mirrors the LSP spec's `[fmt].indent`
/// setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IndentStyle {
    #[default]
    Tab,
    Spaces(u8),
}

/// Formatter options. All fields have spec-defined defaults.
///
/// `Copy` (#972): three scalar fields, and the config layering passes them by
/// value through per-file resolution — a `clone()` at each hop would be noise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatOptions {
    pub indent: IndentStyle,
    pub max_line_width: u32,
    pub trailing_comma: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            indent: IndentStyle::Tab,
            max_line_width: 100,
            trailing_comma: true,
        }
    }
}

/// Error returned when formatting fails. The formatter cannot format code
/// that does not parse, so all failure modes here surface as parse errors.
#[derive(Debug, Clone)]
pub struct FormatError {
    pub errors: Vec<CompileError>,
}

/// Format a Bynk source string. On parse failure, returns the original
/// source unchanged is *not* this function's responsibility — callers (LSP,
/// CLI) decide how to handle parse failure. Here we surface the errors so
/// the caller can do so.
pub fn format_source(source: &str, opts: &FormatOptions) -> Result<String, FormatError> {
    // #1763: line endings are not a formatting difference. A CRLF file formats
    // to the LF canonical form, CRs inside `---` blocks included (their text
    // was kept verbatim, so a CR survived into the output). A caller rendering
    // this function's errors should render them against the normalised text.
    let source = &*normalize_line_endings(source);
    let tokens = migration_tokens(source).map_err(|e| FormatError { errors: vec![e] })?;
    // v0.113: a file may hold more than one top-level unit (an atomic
    // `commons` + `suite` file, DECISION S). Format each and join with a blank
    // line. Each unit's output already ends in exactly one newline, so joining
    // with `"\n"` inserts one blank line between units and leaves a single-unit
    // file byte-identical.
    let (units, _warnings, _) =
        parse_units_migrating(&tokens, source).map_err(|errors| FormatError { errors })?;
    let output = render_units(&units, opts);
    // #523/#66 guard: trivia is only attached at declaration/statement
    // granularity, so a comment inside an expression subtree can be silently
    // dropped. Losing user text is worse than leaving a file unformatted — when
    // the output holds fewer comments than the input, refuse with a diagnostic
    // pointing at the first comment that would vanish. It runs on every format:
    // the parse's own `fully_drained` flag only says each comment was harvested
    // from the trivia table, not that it reached the AST, and #1756 found
    // comments harvested then dropped (at the end of a service, agent or
    // capability body, and inside a `cors`/`security`/`limits` policy) that the
    // formatter deleted with no refusal. Both places keep their comments now
    // (#1756, #1786); the guard stays for whatever the next gap is.
    if let Some(error) = comment_loss(source, &tokens, &output) {
        return Err(FormatError {
            errors: vec![error],
        });
    }
    // #1664: doc blocks never enter the trivia table, so `fully_drained` says
    // nothing about them, and the guard above counts `--` comments only. Check
    // them separately, on every run. Since #1756 an orphan (a block separated
    // from the next declaration by a blank line, or with none to follow) is
    // kept as a `Comment::OrphanDoc` and printed in place; this is the backstop
    // for a block some printer path still loses.
    if let Some(error) = doc_block_loss(source, &tokens, &output) {
        return Err(FormatError {
            errors: vec![error],
        });
    }
    // #735 guard: the printer is hand-written and dodges several parse traps by
    // convention (a tail `()` re-attaching as a call, a trailing comma making a
    // param list unparseable). A shape the corpus misses that the printer
    // mis-renders would otherwise be written straight over the user's file with
    // exit 0. Before returning, re-parse the output and compare its *code*
    // structure — every comment stripped from both sides, so trivia re-flow is
    // ignored — against the input. When the output fails to re-parse, or a
    // shape round-trips to a different AST, refuse rather than corrupt.
    if let Some(error) = roundtrip_divergence(&tokens, source, &output, opts) {
        return Err(FormatError {
            errors: vec![error],
        });
    }
    Ok(output)
}

/// #2203 (DECISION F): the tokens `bynk fmt` formats `source` from. A file
/// that uses a legacy spelling, a `--` comment or a `--|` doc line, was
/// written before `//` comments, so it is read as it was then ([`tokenize_legacy`]),
/// and printing its comments as `//` and its docs as `--- ` is its migration.
/// Any other file is read as it is now.
///
/// A file can be half migrated, though: a stray `-- note` in a file whose
/// `--- text` docs and `a --b` subtractions are new. The old and new rules read
/// those lines differently, and only the author knows which was meant, so a
/// legacy file is formatted only when both reads agree on everything but the
/// legacy tokens themselves; otherwise it is refused
/// (`bynk.fmt.mixed_comment_syntax`), naming the first line they disagree on.
fn migration_tokens(source: &str) -> Result<Vec<Token>, CompileError> {
    let is_legacy = |t: &Token| match t.kind {
        TokenKind::Comment => comment_is_legacy(source, t.span),
        TokenKind::DocLine => doc_line_is_legacy(source, t.span),
        _ => false,
    };
    let current = tokenize(source);
    if current.as_ref().is_ok_and(|t| !t.iter().any(is_legacy)) {
        return current;
    }
    let old = match tokenize_legacy(source) {
        Ok(old) if old.iter().any(is_legacy) => old,
        _ => return current,
    };
    let Ok(new) = &current else {
        return Ok(old);
    };
    let key = |t: &Token| (t.span.start, t.span.end);
    let shared: std::collections::HashSet<_> =
        new.iter().filter(|t| is_legacy(t)).map(key).collect();
    let rest = |tokens: &[Token]| -> Vec<Token> {
        tokens
            .iter()
            .filter(|t| !(is_legacy(t) && shared.contains(&key(t))))
            .copied()
            .collect()
    };
    let (old_rest, new_rest) = (rest(&old), rest(new));
    let differ = old_rest
        .iter()
        .zip(&new_rest)
        .find(|(a, b)| a.kind != b.kind || key(a) != key(b))
        .map(|(a, b)| {
            if a.span.start <= b.span.start {
                a.span
            } else {
                b.span
            }
        })
        .or_else(|| {
            (old_rest.len() != new_rest.len()).then(|| {
                let n = old_rest.len().min(new_rest.len());
                old_rest
                    .get(n)
                    .or(new_rest.get(n))
                    .expect("one is longer")
                    .span
            })
        });
    match differ {
        None => Ok(old),
        Some(span) => Err(mixed_comment_syntax(source, span)),
    }
}

/// #2203: the `bynk.fmt.mixed_comment_syntax` refusal for a legacy file whose
/// line at `span` reads differently under the old and new comment rules.
fn mixed_comment_syntax(source: &str, span: Span) -> CompileError {
    let line_start = source[..span.start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = source[span.start..]
        .find('\n')
        .map_or(source.len(), |i| span.start + i);
    let line = Span::new_in(span.file, line_start, line_end);
    CompileError::new(
        "bynk.fmt.mixed_comment_syntax",
        line,
        "this file uses the retired `--` comment or `--|` doc spelling, and this line \
         reads differently under the old and new comment rules — the file was left unchanged",
    )
    .with_label(
        line,
        "under the old rules a `--` here is a comment, and `--- text` is a comment, not a doc",
    )
    .with_note(
        "change the retired `--` comments and `--|` docs to `//` and `--- ` (VS Code's Fix All \
         does this), or rewrite this line, then format again",
    )
}

/// #1763: `source` with every line ending made LF, borrowed when there is
/// nothing to change. The canonical form uses LF, and comparing a file against
/// it modulo line endings is what keeps a Windows checkout
/// (`core.autocrlf=true`) of a canonical file canonical.
///
/// A run of CRs before an LF (`\r\n`, `\r\r\n`, …) is one line ending, so
/// this is a fixed point: normalising its own output changes nothing (#1775
/// review; a single `replace` turned `\r\r\n` into a `\r\n` a second pass
/// would change again). A CR not before an LF is left alone; the lexer treats
/// it as whitespace. Normalising can't change a program value, since a string
/// literal can't span a line (`bynk.lex.unterminated_string`).
pub fn normalize_line_endings(source: &str) -> std::borrow::Cow<'_, str> {
    if !source.contains("\r\n") {
        return std::borrow::Cow::Borrowed(source);
    }
    let mut out = String::with_capacity(source.len());
    let mut pending_crs = 0usize;
    for c in source.chars() {
        match c {
            '\r' => pending_crs += 1,
            '\n' => {
                pending_crs = 0;
                out.push('\n');
            }
            _ => {
                out.extend(std::iter::repeat_n('\r', pending_crs));
                pending_crs = 0;
                out.push(c);
            }
        }
    }
    out.extend(std::iter::repeat_n('\r', pending_crs));
    std::borrow::Cow::Owned(out)
}

/// Format every top-level unit and join with a blank line. A file may hold more
/// than one top-level unit (v0.113, an atomic `commons` + `suite` file,
/// DECISION S). Each unit's output already ends in exactly one newline, so
/// joining with `"\n"` inserts one blank line between units and leaves a
/// single-unit file byte-identical.
fn render_units(units: &[SourceUnit], opts: &FormatOptions) -> String {
    let parts: Vec<String> = units
        .iter()
        .map(|unit| {
            let mut f = Formatter::new(opts);
            f.format_unit(unit);
            f.finish()
        })
        .collect();
    parts.join("\n")
}

/// #735: the comment-free canonical rendering of `source` — every `Comment`
/// token dropped before parsing, so the result carries no trivia and reflects
/// only the code structure. Because the parser strips comments the same way
/// ([`split_trivia`]), pre-filtering them changes nothing structural; it just
/// leaves the trivia fields empty so the formatter emits pure code. Returns the
/// parse errors when `source` does not tokenize or parse.
fn code_only_canonical(source: &str, opts: &FormatOptions) -> Result<String, Vec<CompileError>> {
    let tokens = tokenize(source).map_err(|e| vec![e])?;
    code_only_canonical_from_tokens(&tokens, source, opts)
}

/// [`code_only_canonical`], given an already-tokenized `source` — finding #66:
/// `format_source` tokenizes `source` once up front for the real render;
/// `roundtrip_divergence` reuses that token list here instead of tokenizing
/// `source` from scratch a second time.
fn code_only_canonical_from_tokens(
    tokens: &[Token],
    source: &str,
    opts: &FormatOptions,
) -> Result<String, Vec<CompileError>> {
    let code: Vec<Token> = tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Comment)
        .cloned()
        .collect();
    let (units, _, _) = parse_units_migrating(&code, source)?;
    Ok(render_units(&units, opts))
}

/// #735: refuse when the formatter's `output` does not round-trip to the same
/// code as `source`. Both sides are reduced to their comment-free canonical
/// form ([`code_only_canonical`]) and compared: the formatter only re-flows
/// whitespace and trivia, so for a faithful render the two canonical strings
/// are byte-identical. A mismatch means either the output no longer parses (the
/// data-loss vector) or the printer altered the AST. `None` when the output is
/// safe to write.
///
/// Note: this guard assumes the formatter is idempotent on comment-free code —
/// i.e. `render(parse(strip(x)))` is a stable canonical form. That invariant is
/// held by the corpus/property idempotency tests. Were a future formatter
/// change to break it on some shape, this guard would *refuse* an otherwise
/// valid file rather than corrupt it — it fails safe (file unchanged + a
/// diagnostic), but the surprise would be a formatter bug to fix upstream.
fn roundtrip_divergence(
    tokens: &[Token],
    source: &str,
    output: &str,
    opts: &FormatOptions,
) -> Option<CompileError> {
    // The output MUST re-parse to the same structure; a failure here is the
    // core corruption vector this guard exists to stop.
    let canon_out = match code_only_canonical(output, opts) {
        Ok(canon) => canon,
        Err(_) => {
            return Some(roundtrip_error(
                "the formatter produced output that no longer parses",
            ));
        }
    };
    // The input already tokenized (and parsed) in `format_source` — `tokens` is
    // that same token list, reused rather than tokenizing `source` a second
    // time (finding #66). If its canonical form unexpectedly fails to compute,
    // do not block a valid format on our own guard failing — leave the file
    // writable.
    let canon_in = code_only_canonical_from_tokens(tokens, source, opts).ok()?;
    (canon_in != canon_out)
        .then(|| roundtrip_error("the formatter's output does not round-trip to the same code"))
}

/// Build the `bynk.fmt.roundtrip` diagnostic shared by both failure modes of
/// [`roundtrip_divergence`]. The span is deliberately `Span::default()` (the
/// start of the file): the message points at neither the source nor the
/// output — it is a generic "this is a formatter bug" — and the failing branch
/// carries an *output*-relative span, which the caller renders against the
/// *source* string. When the mis-rendered output is longer than the source,
/// that span is out of range for the buffer ariadne is given (a misplaced
/// caret, or a byte-index panic in the very formatter-bug path this guard
/// exists to handle gracefully). A zero span is always in range and buys the
/// message nothing to lose.
fn roundtrip_error(what: &str) -> CompileError {
    CompileError {
        category: "bynk.fmt.roundtrip",
        span: Span::default(),
        message: format!("{what} — the file was left unchanged"),
        labels: Vec::new(),
        notes: vec![
            "this is a formatter bug, not a problem with your source; please report it \
             with the file that triggered it"
                .to_string(),
        ],
        suggestions: Vec::new(),
    }
}

/// #523: compare the comment population of `source` (already tokenized as
/// `tokens`) against `output`. Returns a `bynk.fmt.comment_loss` error naming
/// the first lost comment when the output would hold fewer comments, `None`
/// when every comment survives. Comments may legitimately *move* (expression
/// trivia folds into the enclosing statement's leading block), so the
/// comparison is by body multiset, not position.
fn comment_loss(source: &str, tokens: &[Token], output: &str) -> Option<CompileError> {
    use bynk_syntax::lexer::comment_body;
    let in_comments: Vec<Span> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Comment)
        .map(|t| t.span)
        .collect();
    if in_comments.is_empty() {
        return None;
    }
    // The formatter's own output must tokenize; treat a failure as "all
    // comments lost" rather than silently accepting the write.
    let mut out_bodies: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    if let Ok(out_tokens) = tokenize(output) {
        for t in &out_tokens {
            if t.kind == TokenKind::Comment {
                *out_bodies
                    .entry(comment_body(output, t.span).trim().to_string())
                    .or_insert(0) += 1;
            }
        }
    }
    let mut lost = 0usize;
    let mut first_lost: Option<Span> = None;
    for span in &in_comments {
        let body = comment_body(source, *span).trim().to_string();
        match out_bodies.get_mut(&body) {
            Some(n) if *n > 0 => *n -= 1,
            _ => {
                lost += 1;
                first_lost.get_or_insert(*span);
            }
        }
    }
    let span = first_lost?;
    Some(CompileError {
        category: "bynk.fmt.comment_loss",
        span,
        message: format!(
            "formatting would lose {lost} comment{} — the file was left unchanged",
            if lost == 1 { "" } else { "s" }
        ),
        labels: vec![(
            span,
            "this comment sits where the formatter cannot yet re-attach it".to_string(),
        )],
        notes: vec![
            "comments inside expression subtrees are not yet preserved; move the comment onto \
             its own line before the enclosing statement to format this file"
                .to_string(),
        ],
        suggestions: Vec::new(),
    })
}

/// #1888: every doc in `source` (already tokenized as `tokens`), in order: its
/// span, its form, and its text with each line's surrounding whitespace
/// removed, so the formatter's re-indentation of an attached doc is not
/// mistaken for loss.
fn source_docs(source: &str, tokens: &[Token]) -> Vec<(Span, DocForm, String)> {
    use bynk_syntax::lexer::{doc_block_content, doc_line_text};
    let normalise = |content: &str| {
        content
            .lines()
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n")
    };
    tokens
        .iter()
        .filter_map(|t| match t.kind {
            TokenKind::DocLine => Some((
                t.span,
                DocForm::Lines,
                normalise(doc_line_text(source, t.span)),
            )),
            TokenKind::DocBlock => Some((
                t.span,
                DocForm::Block,
                normalise(&doc_block_content(source, t.span)),
            )),
            _ => None,
        })
        .collect()
}

/// #1664: the `---` counterpart of [`comment_loss`]. Returns a
/// `bynk.fmt.comment_loss` error naming the first doc of `source` (already
/// tokenized as `tokens`) that has no counterpart in `output`, compared by
/// content multiset. Attached and orphaned (#1756) docs are both re-rendered
/// with their content intact, so this fires only on a printer gap.
///
/// #1888: both doc forms are compared, each keyed by its form as well as its
/// content, so a printer path that converted a doc from one form to the other
/// (DECISION F says it never does) is refused as well as one that lost it.
///
/// An `output` that does not tokenize returns `None`: that is a formatter bug
/// the round-trip guard reports accurately ("no longer parses"), and counting
/// every block as lost would point the user at an innocent one instead.
fn doc_block_loss(source: &str, tokens: &[Token], output: &str) -> Option<CompileError> {
    let in_docs = source_docs(source, tokens);
    if in_docs.is_empty() {
        return None;
    }
    let out_tokens = tokenize(output).ok()?;
    let mut out_docs: std::collections::HashMap<(DocForm, String), usize> =
        std::collections::HashMap::new();
    for (_, form, content) in source_docs(output, &out_tokens) {
        *out_docs.entry((form, content)).or_insert(0) += 1;
    }
    let mut lost = 0usize;
    let mut first_lost: Option<Span> = None;
    for (span, form, content) in in_docs {
        match out_docs.get_mut(&(form, content)) {
            Some(n) if *n > 0 => *n -= 1,
            _ => {
                lost += 1;
                first_lost.get_or_insert(span);
            }
        }
    }
    let span = first_lost?;
    Some(CompileError {
        category: "bynk.fmt.comment_loss",
        span,
        message: format!(
            "formatting would lose {lost} doc{} — the file was left unchanged",
            if lost == 1 { "" } else { "s" }
        ),
        labels: vec![(
            span,
            "this doc has no counterpart in the formatted output".to_string(),
        )],
        notes: vec![
            "a doc (a `---` block or `--|` lines) attaches to the declaration or member \
             directly below it; one separated from it by a blank line, or with nothing after \
             it, attaches to nothing. Remove the blank line to attach it, or make it a `--` \
             comment if it documents nothing"
                .to_string(),
            "if the block is already directly above a declaration, this is a formatter bug; \
             please report it with the file that triggered it"
                .to_string(),
        ],
        suggestions: Vec::new(),
    })
}

// -- Internal formatter state --

struct Formatter<'a> {
    opts: &'a FormatOptions,
    out: String,
    indent_level: u32,
    /// True when the formatter has just emitted a newline and is at the
    /// start of a fresh line. Used to gate indent emission.
    at_line_start: bool,
}

impl<'a> Formatter<'a> {
    fn new(opts: &'a FormatOptions) -> Self {
        Self {
            opts,
            out: String::new(),
            indent_level: 0,
            at_line_start: true,
        }
    }

    fn finish(mut self) -> String {
        // Single trailing newline.
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out
    }

    fn indent_unit(&self) -> String {
        match self.opts.indent {
            IndentStyle::Tab => "\t".to_string(),
            IndentStyle::Spaces(n) => " ".repeat(n as usize),
        }
    }

    fn emit_indent(&mut self) {
        let unit = self.indent_unit();
        for _ in 0..self.indent_level {
            self.out.push_str(&unit);
        }
    }

    fn push(&mut self, s: &str) {
        if self.at_line_start && !s.starts_with('\n') {
            self.emit_indent();
            self.at_line_start = false;
        }
        if s.contains('\n') {
            self.push_reindented(s);
        } else {
            self.out.push_str(s);
        }
    }

    /// Append a multi-line string, re-applying the current indent to every
    /// continuation line. Multi-line strings come from the single-line
    /// expression renderers (`expr_to_string` and friends), which build their
    /// internal structure assuming column zero — they embed `\n` plus relative
    /// tabs but know nothing about the current nesting depth. Without this an
    /// argument-position `match` (or any embedded multi-line expression) would
    /// print its arms and trailing brace at column one regardless of how deeply
    /// it is nested. The first line is emitted as-is (its indent, if any, was
    /// handled by `push`); blank lines are left empty rather than padded.
    fn push_reindented(&mut self, s: &str) {
        let prefix = self.indent_unit().repeat(self.indent_level as usize);
        for (i, line) in s.split('\n').enumerate() {
            if i > 0 {
                self.out.push('\n');
                if !line.is_empty() {
                    self.out.push_str(&prefix);
                }
            }
            self.out.push_str(line);
        }
    }

    fn newline(&mut self) {
        self.out.push('\n');
        self.at_line_start = true;
    }

    /// End the current line if it is open, then leave a blank line unless the
    /// output already ends with one.
    fn blank_line(&mut self) {
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        if !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
        self.at_line_start = true;
    }

    fn indented<F: FnOnce(&mut Self)>(&mut self, f: F) {
        self.indent_level += 1;
        f(self);
        self.indent_level -= 1;
    }

    /// Render `body` into a detached formatter positioned at this one's exact
    /// column and indent, and commit it only if every line it produces stays
    /// inside the budget (the last line counting `reserve` further columns for
    /// whatever the caller will append). Returns whether it was committed.
    ///
    /// A candidate layout is often only judgeable once rendered: whether a
    /// `.`-chain needs breaking at its dots depends on how its arguments wrap,
    /// which depends on the column they start at. Measuring a real rendering
    /// beats predicting one (#963).
    fn try_layout<F: FnOnce(&mut Self)>(&mut self, reserve: usize, body: F) -> bool {
        self.try_layout_if(reserve, body, || true)
    }

    /// [`Self::try_layout`] with a second condition, evaluated after the
    /// rendering, for a caller that judges the candidate on more than its
    /// width — a chain that fits but only by exploding an argument list.
    fn try_layout_if<F: FnOnce(&mut Self), A: FnOnce() -> bool>(
        &mut self,
        reserve: usize,
        body: F,
        accept: A,
    ) -> bool {
        let line_start = self.out.rfind('\n').map_or(0, |i| i + 1);
        let mut sub = Formatter {
            opts: self.opts,
            out: self.out[line_start..].to_string(),
            indent_level: self.indent_level,
            at_line_start: self.at_line_start,
        };
        let produced_from = sub.out.len();
        body(&mut sub);
        if !sub.every_line_within_budget(reserve) || !accept() {
            return false;
        }
        // Splice the raw text: `sub` already emitted its own indentation, so
        // going through `push` would double it.
        self.out.push_str(&sub.out[produced_from..]);
        self.at_line_start = sub.at_line_start;
        true
    }

    /// Does every line of this (detached) formatter's buffer fit, with
    /// `reserve` columns still free on the last one?
    fn every_line_within_budget(&self, reserve: usize) -> bool {
        let tab = self.indent_width();
        let mut lines = self.out.split('\n').peekable();
        let mut last = "";
        while let Some(line) = lines.next() {
            if lines.peek().is_none() {
                last = line;
                break;
            }
            if display_width(line, tab) > self.opts.max_line_width as usize {
                return false;
            }
        }
        let last_width = if self.at_line_start {
            // The trailing indent is not in the buffer yet; `push` adds it.
            self.indent_level as usize * tab
        } else {
            display_width(last, tab)
        };
        last_width + reserve <= self.opts.max_line_width as usize
    }

    // -- Doc block --

    /// Emit a doc block immediately above a declaration. The content is
    /// already normalised (common leading indent stripped) when stored in
    /// the AST; we re-emit with the current indent applied per line.
    ///
    /// #1885: a doc block closes only on a marker of its opener's length, so
    /// the fence is the shortest (three or more dashes) that is longer than
    /// every marker-shaped line in the content; the output re-lexes to the
    /// same block. A doc with no such line keeps the canonical `---`.
    ///
    /// #1888 (DECISION F): a doc prints in the form it was written in, never
    /// converted. A one-line doc prints as `--- text` (#2203), without the
    /// optional closer, and a legacy `--|` line prints the same way.
    fn emit_doc(&mut self, doc: &Doc) {
        if doc.form == DocForm::Lines {
            self.push(&one_line_doc(doc));
            self.newline();
            return;
        }
        let fence = "-".repeat(doc_fence_len(doc));
        self.push(&fence);
        self.newline();
        for line in doc.lines() {
            if line.is_empty() {
                self.newline();
            } else {
                self.push(line);
                self.newline();
            }
        }
        self.push(&fence);
        self.newline();
    }

    // -- Line-comment trivia (v1.1) --

    /// Emit a sequence of leading comments, each on its own line at the
    /// current indent. `--` lines have no blank lines between them, except
    /// where a unit-level declaration's comments keep one (#1884,
    /// [`Comment::Blank`]); an orphaned doc block (#1756) prints as a doc block
    /// and is followed by a blank line, which is what keeps it from attaching
    /// to the declaration below.
    fn emit_leading_comments(&mut self, comments: &[Comment]) {
        self.emit_comments(comments, true);
    }

    /// Emit the comments that close a body or file. As
    /// [`Self::emit_leading_comments`], except that a blank line after the
    /// last entry is not printed: nothing follows it to keep apart, and the
    /// body's `}` comes straight after its last line.
    fn emit_trailing_comments(&mut self, comments: &[Comment]) {
        self.emit_comments(comments, false);
    }

    fn emit_comments(&mut self, comments: &[Comment], blank_after_last: bool) {
        for (i, comment) in comments.iter().enumerate() {
            let blank_after = i + 1 < comments.len() || blank_after_last;
            match comment {
                Comment::Line(body) => {
                    self.push("//");
                    self.push(body);
                    self.newline();
                }
                Comment::OrphanDoc(doc) => {
                    self.emit_doc(doc);
                    if blank_after {
                        self.newline();
                    }
                }
                Comment::Blank if blank_after => self.blank_line(),
                Comment::Blank => {}
            }
        }
    }

    /// Emit a trailing comment on the same line as the just-emitted token.
    /// The spec uses two spaces between code and comment for readability.
    fn emit_trailing_comment(&mut self, body: Option<&str>) {
        if let Some(body) = body {
            // Ensure we're on the same line as the preceding tokens —
            // strip any newline we just emitted.
            while self.out.ends_with('\n') {
                self.out.pop();
            }
            self.out.push_str("  //");
            self.out.push_str(body);
            self.newline();
        }
    }

    /// End the current line: with the trailing comment when there is one
    /// (which [`Self::emit_trailing_comment`] closes with its own newline),
    /// otherwise with a bare newline. A site that splices the comment onto a
    /// line something else already ended calls `emit_trailing_comment` alone.
    fn emit_trailing_comment_or_newline(&mut self, trailing: Option<&str>) {
        if trailing.is_some() {
            self.emit_trailing_comment(trailing);
        } else {
            self.newline();
        }
    }

    // -- Top level --

    fn format_unit(&mut self, unit: &SourceUnit) {
        match unit {
            SourceUnit::Commons(c) => self.format_commons(c),
            SourceUnit::Context(c) => self.format_context(c),
            SourceUnit::Suite(t) => self.format_test(t),
            SourceUnit::Adapter(a) => self.format_adapter(a),
        }
    }

    fn format_adapter(&mut self, a: &AdapterDecl) {
        self.emit_leading_comments(&a.trivia.leading);
        if let Some(doc) = &a.documentation {
            self.emit_doc(doc);
        }
        let header = format!("adapter {}", a.name.joined());
        match a.form {
            CommonsForm::Brace => {
                self.push(&header);
                self.push(" {");
                self.newline();
                self.indented(|f| {
                    f.format_adapter_body(a);
                });
                self.push("}");
                self.newline();
            }
            CommonsForm::Fragment => {
                self.push(&header);
                self.newline();
                self.newline();
                self.format_adapter_body(a);
            }
        }
    }

    fn format_adapter_body(&mut self, a: &AdapterDecl) {
        let mut any_header = false;
        if let Some(b) = &a.binding {
            self.emit_leading_comments(&b.trivia.leading);
            self.push(&format!("binding {:?}", b.module));
            if !b.includes.is_empty() {
                let entries: Vec<String> =
                    b.includes.iter().map(|i| format!("{:?}", i.path)).collect();
                self.push(&format!(" includes [{}]", entries.join(", ")));
            }
            if !b.requires.is_empty() {
                let entries: Vec<String> = b
                    .requires
                    .iter()
                    .map(|r| format!("{:?}: {:?}", r.package, r.range))
                    .collect();
                self.push(&format!(" requires {{ {} }}", entries.join(", ")));
            }
            self.emit_trailing_comment_or_newline(b.trivia.trailing.as_deref());
            any_header = true;
        }
        for u in &a.uses {
            self.emit_leading_comments(&u.trivia.leading);
            self.push(&format!("uses {}", u.target.joined()));
            self.emit_trailing_comment_or_newline(u.trivia.trailing.as_deref());
            any_header = true;
        }
        for c in &a.consumes {
            self.format_consumes(c);
            any_header = true;
        }
        for e in &a.exports {
            self.emit_leading_comments(&e.trivia.leading);
            self.format_exports(e);
            if e.trivia.trailing.is_some() {
                self.emit_trailing_comment(e.trivia.trailing.as_deref());
            }
            any_header = true;
        }
        if any_header && !a.items.is_empty() {
            self.newline();
        }
        let mut first = true;
        for item in &a.items {
            if !first {
                self.newline();
            }
            self.format_item(item);
            first = false;
        }
        if !a.trailing_comments.is_empty() {
            if !a.items.is_empty() || any_header {
                self.newline();
            }
            self.emit_trailing_comments(&a.trailing_comments);
        }
    }

    fn format_test(&mut self, t: &SuiteDecl) {
        self.emit_leading_comments(&t.trivia.leading);
        if let Some(doc) = &t.documentation {
            self.emit_doc(doc);
        }
        let mut header = format!("suite {}", t.target.joined());
        if let Some(tier) = t.tier {
            header.push_str(&format!(" as {}", tier.as_str()));
        }
        match t.form {
            CommonsForm::Brace => {
                self.push(&header);
                self.push(" {");
                self.newline();
                self.indented(|f| {
                    f.format_test_body(
                        &t.uses,
                        &t.envs,
                        &t.stubs,
                        &t.cases,
                        &t.properties,
                        &t.trailing_comments,
                    );
                });
                self.push("}");
                self.newline();
            }
            CommonsForm::Fragment => {
                self.push(&header);
                self.newline();
                self.newline();
                self.format_test_body(
                    &t.uses,
                    &t.envs,
                    &t.stubs,
                    &t.cases,
                    &t.properties,
                    &t.trailing_comments,
                );
            }
        }
    }

    fn format_test_body(
        &mut self,
        uses: &[UsesDecl],
        envs: &[EnvClause],
        stubs: &[StubClause],
        cases: &[Case],
        properties: &[PropertyDecl],
        trailing_comments: &[Comment],
    ) {
        let mut first = true;
        for u in uses {
            if !first {
                self.newline();
            }
            self.emit_leading_comments(&u.trivia.leading);
            self.push(&format!("uses {}", u.target.joined()));
            self.emit_trailing_comment_or_newline(u.trivia.trailing.as_deref());
            first = false;
        }
        for e in envs {
            if !first {
                self.newline();
            }
            self.format_env_clause(e);
            first = false;
        }
        for pv in stubs {
            if !first {
                self.newline();
            }
            self.format_stub_clause(pv);
            first = false;
        }
        // `cases` and `properties` are separate lists, so merge them back
        // into source order rather than printing every property last.
        let mut members: Vec<(usize, Result<&Case, &PropertyDecl>)> = cases
            .iter()
            .map(|c| (c.name_span.start, Ok(c)))
            .chain(properties.iter().map(|p| (p.name_span.start, Err(p))))
            .collect();
        members.sort_by_key(|(start, _)| *start);
        for (_, member) in members {
            if !first {
                self.newline();
            }
            match member {
                Ok(c) => self.format_case(c),
                Err(p) => self.format_property(p),
            }
            first = false;
        }
        self.emit_trailing_comments(trailing_comments);
    }

    fn format_case(&mut self, c: &Case) {
        self.emit_leading_comments(&c.trivia.leading);
        if let Some(doc) = &c.documentation {
            self.emit_doc(doc);
        }
        let mut ch = format!("case \"{}\"", escape_string(&c.name));
        if let Some(tier) = c.tier {
            ch.push_str(&format!(" as {}", tier.as_str()));
        }
        ch.push(' ');
        self.push(&ch);
        self.format_case_block(&c.body, &c.envs, &c.stubs);
        self.newline();
    }

    fn format_property(&mut self, p: &PropertyDecl) {
        self.emit_leading_comments(&p.trivia.leading);
        if let Some(doc) = &p.documentation {
            self.emit_doc(doc);
        }
        self.push(&format!("property \"{}\" {{", escape_string(&p.name)));
        self.newline();
        self.indented(|f| f.format_for_all(&p.forall));
        self.push("}");
        self.newline();
    }

    /// v0.118: format a `stub` clause as a suite- or case-body line, with
    /// its leading comments / doc and a terminating newline (testing track
    /// slice 6).
    fn format_stub_clause(&mut self, pv: &StubClause) {
        self.emit_leading_comments(&pv.trivia.leading);
        if let Some(doc) = &pv.documentation {
            self.emit_doc(doc);
        }
        self.push(&stub_clause_to_string(pv));
        self.emit_trailing_comment_or_newline(pv.trivia.trailing.as_deref());
    }

    /// #2134: format an `env { … }` clause as a suite- or case-body member,
    /// one `NAME = value` entry per line.
    fn format_env_clause(&mut self, e: &EnvClause) {
        self.emit_leading_comments(&e.trivia.leading);
        if let Some(doc) = &e.documentation {
            self.emit_doc(doc);
        }
        self.push("env {");
        self.newline();
        self.indented(|f| {
            for entry in &e.entries {
                f.push(&format!(
                    "{} = {}",
                    entry.name.name,
                    expr_to_string(&entry.value)
                ));
                f.newline();
            }
        });
        self.push("}");
        self.emit_trailing_comment_or_newline(e.trivia.trailing.as_deref());
    }

    /// v0.118: format a `case` body, emitting its case-scoped `env` (#2134)
    /// and `stub` clauses as the leading lines inside the block, before the
    /// statements and tail. With neither this is exactly [`Self::format_block`].
    fn format_case_block(&mut self, b: &Block, envs: &[EnvClause], stubs: &[StubClause]) {
        if stubs.is_empty() && envs.is_empty() {
            self.format_block(b);
            return;
        }
        self.push("{");
        self.newline();
        self.indented(|f| {
            for e in envs {
                f.format_env_clause(e);
            }
            for pv in stubs {
                f.format_stub_clause(pv);
            }
            // Through `format_block_body`, so a `let … else` prints back as
            // written (#2010).
            f.format_block_body(b);
        });
        self.push("}");
    }

    /// v0.114: format a `for all <bindings> [where <pred>] { … }` binder — the
    /// sole body of a `property`.
    fn format_for_all(&mut self, fa: &ForAll) {
        let bindings = fa
            .bindings
            .iter()
            .map(|b| format!("{}: {}", b.name.name, type_ref_to_string(&b.type_ref)))
            .collect::<Vec<_>>()
            .join(", ");
        let mut header = format!("for all {bindings}");
        if let Some(w) = &fa.where_pred {
            header.push_str(&format!(" where {}", expr_to_string(w)));
        }
        self.push(&format!("{header} "));
        self.format_block(&fa.body);
        self.newline();
    }

    fn format_commons(&mut self, c: &Commons) {
        self.emit_leading_comments(&c.trivia.leading);
        if let Some(doc) = &c.documentation {
            self.emit_doc(doc);
        }
        let header = format!("commons {}", c.name.joined());
        match c.form {
            CommonsForm::Brace => {
                self.push(&header);
                self.push(" {");
                self.newline();
                self.indented(|f| {
                    f.format_commons_body(&c.uses, &c.items, &c.trailing_comments);
                });
                self.push("}");
                self.newline();
            }
            CommonsForm::Fragment => {
                self.push(&header);
                self.newline();
                self.newline();
                self.format_commons_body(&c.uses, &c.items, &c.trailing_comments);
            }
        }
    }

    fn format_commons_body(
        &mut self,
        uses: &[UsesDecl],
        items: &[CommonsItem],
        trailing_comments: &[Comment],
    ) {
        let mut any_uses = false;
        for u in uses {
            self.emit_leading_comments(&u.trivia.leading);
            self.push(&format!("uses {}", u.target.joined()));
            self.emit_trailing_comment_or_newline(u.trivia.trailing.as_deref());
            any_uses = true;
        }
        if any_uses && !items.is_empty() {
            self.newline();
        }
        let mut first = true;
        for item in items {
            if !first {
                self.newline();
            }
            self.format_item(item);
            first = false;
        }
        if !trailing_comments.is_empty() {
            // One blank line before trailing-file comments if anything
            // came before them.
            if !items.is_empty() || any_uses {
                self.newline();
            }
            self.emit_trailing_comments(trailing_comments);
        }
    }

    fn format_context(&mut self, c: &Context) {
        self.emit_leading_comments(&c.trivia.leading);
        if let Some(doc) = &c.documentation {
            self.emit_doc(doc);
        }
        let header = format!("context {}", c.name.joined());
        match c.form {
            CommonsForm::Brace => {
                self.push(&header);
                self.push(" {");
                self.newline();
                self.indented(|f| {
                    f.format_context_body(
                        &c.uses,
                        &c.consumes,
                        &c.exports,
                        &c.items,
                        &c.removed_agents,
                        &c.trailing_comments,
                    );
                });
                self.push("}");
                self.newline();
            }
            CommonsForm::Fragment => {
                self.push(&header);
                self.newline();
                self.newline();
                self.format_context_body(
                    &c.uses,
                    &c.consumes,
                    &c.exports,
                    &c.items,
                    &c.removed_agents,
                    &c.trailing_comments,
                );
            }
        }
    }

    /// Print one `consumes` clause in any of its three forms: whole-unit,
    /// aliased, or braced capability selection (v0.17 §3.3 — previously the
    /// braced form was silently dropped, a semantic-changing format).
    fn format_consumes(&mut self, c: &ConsumesDecl) {
        self.emit_leading_comments(&c.trivia.leading);
        match (&c.alias, &c.selected) {
            (Some(alias), _) => {
                self.push(&format!("consumes {} as {}", c.target.joined(), alias.name))
            }
            (None, Some(selected)) if selected.is_empty() => {
                self.push(&format!("consumes {} {{ }}", c.target.joined()));
            }
            (None, Some(selected)) => {
                let names: Vec<&str> = selected.iter().map(|i| i.name.as_str()).collect();
                self.push(&format!(
                    "consumes {} {{ {} }}",
                    c.target.joined(),
                    names.join(", ")
                ));
            }
            (None, None) => self.push(&format!("consumes {}", c.target.joined())),
        }
        self.emit_trailing_comment_or_newline(c.trivia.trailing.as_deref());
    }

    fn format_context_body(
        &mut self,
        uses: &[UsesDecl],
        consumes: &[ConsumesDecl],
        exports: &[ExportsDecl],
        items: &[CommonsItem],
        removed_agents: &[RemovedAgentDecl],
        trailing_comments: &[Comment],
    ) {
        let mut any_header = false;
        for u in uses {
            self.emit_leading_comments(&u.trivia.leading);
            self.push(&format!("uses {}", u.target.joined()));
            self.emit_trailing_comment_or_newline(u.trivia.trailing.as_deref());
            any_header = true;
        }
        for c in consumes {
            self.format_consumes(c);
            any_header = true;
        }
        for e in exports {
            self.emit_leading_comments(&e.trivia.leading);
            self.format_exports(e);
            // exports may emit multi-line — the trailing comment goes on
            // its last line. Since format_exports already terminates with
            // a newline, splice the comment before it if present.
            if e.trivia.trailing.is_some() {
                self.emit_trailing_comment(e.trivia.trailing.as_deref());
            }
            any_header = true;
        }
        if any_header && (!items.is_empty() || !removed_agents.is_empty()) {
            self.newline();
        }
        let mut first = true;
        for item in items {
            if !first {
                self.newline();
            }
            self.format_item(item);
            first = false;
        }
        // State-migrations S3 (#2248): `removed agent` declarations, after the
        // live items (a canonical place; they carry no order among items).
        for r in removed_agents {
            if !first {
                self.newline();
            }
            self.emit_leading_comments(&r.trivia.leading);
            if let Some(doc) = &r.documentation {
                self.emit_doc(doc);
            }
            self.push(&format!("removed agent {}", r.name.name));
            self.emit_trailing_comment_or_newline(r.trivia.trailing.as_deref());
            first = false;
        }
        if !trailing_comments.is_empty() {
            if !items.is_empty() || !removed_agents.is_empty() || any_header {
                self.newline();
            }
            self.emit_trailing_comments(trailing_comments);
        }
    }

    fn format_exports(&mut self, e: &ExportsDecl) {
        let vis = match e.kind {
            ExportKind::Type(Visibility::Opaque) => "opaque",
            ExportKind::Type(Visibility::Transparent) => "transparent",
            ExportKind::Capability => "capability",
        };
        // #1797: a comment anywhere in the list forces the multi-line form.
        let has_comments = !e.trailing_comments.is_empty()
            || e.names
                .iter()
                .any(|n| !n.trivia.leading.is_empty() || n.trivia.trailing.is_some());
        if e.names.is_empty() && !has_comments {
            self.push(&format!("exports {} {{}}", vis));
            self.newline();
            return;
        }
        // Single-line form if it fits.
        if !has_comments {
            let oneline = format!(
                "exports {} {{ {} }}",
                vis,
                e.names
                    .iter()
                    .map(|n| n.name.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if self.fits(&oneline, 0) {
                self.push(&oneline);
                self.newline();
                return;
            }
        }
        // Multi-line form.
        self.push(&format!("exports {} {{", vis));
        self.newline();
        self.indented(|f| {
            for (i, n) in e.names.iter().enumerate() {
                f.emit_leading_comments(&n.trivia.leading);
                f.push(&n.name.name);
                if i + 1 < e.names.len() || f.opts.trailing_comma {
                    f.push(",");
                }
                f.emit_trailing_comment_or_newline(n.trivia.trailing.as_deref());
            }
            f.emit_trailing_comments(&e.trailing_comments);
        });
        self.push("}");
        self.newline();
    }

    /// The rendered width of one indent level. A tab is counted as four
    /// columns for width estimation (the file stores one byte; editors render
    /// it at the reader's chosen width, so any fixed number is an estimate).
    fn indent_width(&self) -> usize {
        match self.opts.indent {
            IndentStyle::Tab => 4,
            IndentStyle::Spaces(n) => n as usize,
        }
    }

    /// The column the next character pushed would land on. Everything already
    /// emitted on the current line counts — #963: measuring only
    /// `indent_level` (as this did before) made every fit test blind to the
    /// prefix its caller had already printed, so a body measured as "fits"
    /// while sitting behind `fn name(params) -> Ret ` routinely overflowed.
    fn current_column(&self) -> usize {
        if self.at_line_start {
            // `push` has yet to emit this line's indent; account for it here.
            return self.indent_level as usize * self.indent_width();
        }
        let line = match self.out.rfind('\n') {
            Some(i) => &self.out[i + 1..],
            None => self.out.as_str(),
        };
        display_width(line, self.indent_width())
    }

    /// Does `candidate`, emitted at the current column, leave `reserve`
    /// further columns inside the line budget? `reserve` is the width of text
    /// the caller knows will follow on the same line — a closing `)`, a
    /// `-> Ret {` suffix, an arm's `,`. A multi-line candidate never "fits":
    /// its own line breaks are the decision the caller is trying to make.
    fn fits(&self, candidate: &str, reserve: usize) -> bool {
        if candidate.contains('\n') {
            return false;
        }
        let column =
            self.current_column() + display_width(candidate, self.indent_width()) + reserve;
        column <= self.opts.max_line_width as usize
    }

    fn format_item(&mut self, item: &CommonsItem) {
        match item {
            CommonsItem::Type(t) => self.format_type_decl(t),
            CommonsItem::Fn(f) => self.format_fn_decl(f),
            CommonsItem::Capability(c) => self.format_capability(c),
            CommonsItem::Provider(p) => self.format_provider(p),
            CommonsItem::Service(s) => self.format_service(s),
            CommonsItem::Agent(a) => self.format_agent(a),
            CommonsItem::Actor(a) => self.format_actor(a),
            CommonsItem::Messages(m) => self.format_messages(m),
            CommonsItem::Event(e) => self.format_event_decl(e),
            CommonsItem::Asset(a) => self.format_asset_decl(a),
            CommonsItem::Embed(e) => self.format_embed_decl(e),
            CommonsItem::Const(c) => self.format_const_decl(c),
        }
    }

    /// #1883, slice 1a: `asset name = "path"` on one line.
    fn format_asset_decl(&mut self, a: &AssetDecl) {
        self.emit_leading_comments(&a.trivia.leading);
        if let Some(doc) = &a.documentation {
            self.emit_doc(doc);
        }
        // #2618: the unit-private modifier.
        if a.private.is_some() {
            self.push("private ");
        }
        self.push(&format!(
            "asset {} = \"{}\"",
            a.name.name,
            escape_string(&a.path)
        ));
        if let Some((at, _)) = &a.at {
            self.push(&format!(" at \"{}\"", escape_string(at)));
        }
        self.emit_trailing_comment_or_newline(a.trivia.trailing.as_deref());
    }

    /// #2387, slice 1: `embed name = "path"` on one line.
    fn format_embed_decl(&mut self, e: &EmbedDecl) {
        self.emit_leading_comments(&e.trivia.leading);
        if let Some(doc) = &e.documentation {
            self.emit_doc(doc);
        }
        // #2618: the unit-private modifier.
        if e.private.is_some() {
            self.push("private ");
        }
        self.push(&format!(
            "embed {} = \"{}\"",
            e.name.name,
            escape_string(&e.path)
        ));
        self.emit_trailing_comment_or_newline(e.trivia.trailing.as_deref());
    }

    /// #2617: `const Name: Type = <expr>`, the initialiser laid out like a
    /// `let` value (a long record construction breaks across lines).
    fn format_const_decl(&mut self, c: &ConstDecl) {
        self.emit_leading_comments(&c.trivia.leading);
        if let Some(doc) = &c.documentation {
            self.emit_doc(doc);
        }
        self.push("const ");
        self.push(&c.name.name);
        self.push(": ");
        self.format_type_ref(&c.ty);
        self.push(" = ");
        self.format_expr(&c.init);
        self.emit_trailing_comment_or_newline(c.trivia.trailing.as_deref());
    }

    fn format_event_decl(&mut self, e: &EventDecl) {
        self.emit_leading_comments(&e.trivia.leading);
        if let Some(doc) = &e.documentation {
            self.emit_doc(doc);
        }
        self.push(&format!("event {}", e.name.name));
        // Events slice 3b (#978): an optional `@schema(N)` (and any other
        // future event annotation) — same loop as `format_messages`'s.
        for ann in &e.annotations {
            self.push(" ");
            self.push(&annotation_to_string(ann));
        }
        self.push(" = ");
        self.format_record_body(&e.body);
        self.emit_trailing_comment_or_newline(e.trivia.trailing.as_deref());
    }

    fn format_messages(&mut self, m: &MessagesDecl) {
        self.emit_leading_comments(&m.trivia.leading);
        if let Some(doc) = &m.documentation {
            self.emit_doc(doc);
        }
        // #2618: the unit-private modifier.
        if m.private.is_some() {
            self.push("private ");
        }
        self.push(&format!("messages \"{}\"", escape_string(&m.tag)));
        for ann in &m.annotations {
            self.push(" ");
            self.push(&annotation_to_string(ann));
        }
        self.push(" {");
        self.newline();
        self.indented(|f| {
            for entry in &m.entries {
                if let Some(doc) = &entry.documentation {
                    f.emit_doc(doc);
                }
                f.push(&format!(
                    "\"{}\" => \"{}\"",
                    escape_string(&entry.code),
                    escape_string(&entry.template)
                ));
                f.newline();
            }
        });
        self.push("}");
        self.emit_trailing_comment_or_newline(m.trivia.trailing.as_deref());
    }

    // -- Type declarations --

    fn format_type_decl(&mut self, t: &TypeDecl) {
        self.emit_leading_comments(&t.trivia.leading);
        if let Some(doc) = &t.documentation {
            self.emit_doc(doc);
        }
        // #2618: the unit-private modifier.
        if t.private.is_some() {
            self.push("private ");
        }
        // v0.157 (ADR 0183): `[A, B]` type parameters, spelled as on a function.
        let params = if t.type_params.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = t.type_params.iter().map(type_param_text).collect();
            format!("[{}]", names.join(", "))
        };
        self.push(&format!("type {}{} = ", t.name.name, params));
        self.format_type_body(&t.body);
        if !t.derives.is_empty() {
            let names: Vec<&str> = t.derives.iter().map(|d| d.name.as_str()).collect();
            let clause = format!("derives {}", names.join(" + "));
            // A pipe-form sum ends on its last variant's line, which may carry
            // a comment, so the clause takes its own line under the variants.
            if matches!(&t.body, TypeBody::Sum(s) if s.variants.iter().any(|v| !v.payload.is_empty()))
            {
                self.indented(|f| {
                    f.newline();
                    f.push(&clause);
                });
            } else {
                self.push(&format!(" {clause}"));
            }
        }
        self.emit_trailing_comment_or_newline(t.trivia.trailing.as_deref());
    }

    fn format_type_body(&mut self, body: &TypeBody) {
        match body {
            TypeBody::Refined {
                base, refinement, ..
            } => {
                self.push(base.name());
                if let Some(r) = refinement {
                    self.push(" where ");
                    self.format_refinement(r);
                }
            }
            TypeBody::Opaque {
                base, refinement, ..
            } => {
                self.push("opaque ");
                self.push(base.name());
                if let Some(r) = refinement {
                    self.push(" where ");
                    self.format_refinement(r);
                }
            }
            TypeBody::Record(r) => self.format_record_body(r),
            TypeBody::Sum(s) => self.format_sum_body(s),
        }
    }

    fn format_refinement(&mut self, r: &Refinement) {
        for (i, p) in r.predicates.iter().enumerate() {
            if i > 0 {
                self.push(" && ");
            }
            self.format_pred(p);
        }
    }

    fn format_pred(&mut self, p: &RefinementPred) {
        match &p.kind {
            PredKind::Matches(re) => self.push(&format!("Matches(\"{}\")", escape_string(re))),
            PredKind::InRange(a, b) => self.push(&format!("InRange({}, {})", a.value, b.value)),
            PredKind::InRangeF(a, b) => self.push(&format!("InRange({}, {})", a.lexeme, b.lexeme)),
            PredKind::MinLength(n) => self.push(&format!("MinLength({n})")),
            PredKind::MaxLength(n) => self.push(&format!("MaxLength({n})")),
            PredKind::Length(n) => self.push(&format!("Length({n})")),
            PredKind::NonNegative => self.push("NonNegative"),
            PredKind::Positive => self.push("Positive"),
            PredKind::NonEmpty => self.push("NonEmpty"),
        }
    }

    fn format_record_body(&mut self, r: &RecordBody) {
        // A comment, or a field's doc (#1888), forces the multi-line form.
        let has_comments = !r.trailing_comments.is_empty()
            || r.fields.iter().any(|f| {
                !f.trivia.leading.is_empty()
                    || f.trivia.trailing.is_some()
                    || f.documentation.is_some()
            });
        if r.fields.is_empty() && !has_comments {
            self.push("{}");
            return;
        }
        // A braced declaration body always breaks, one field per line.
        self.push("{");
        self.newline();
        self.indented(|f| {
            for (i, field) in r.fields.iter().enumerate() {
                f.emit_leading_comments(&field.trivia.leading);
                if let Some(doc) = &field.documentation {
                    f.emit_doc(doc);
                }
                f.format_record_field(field);
                if i + 1 < r.fields.len() || f.opts.trailing_comma {
                    f.push(",");
                }
                f.emit_trailing_comment_or_newline(field.trivia.trailing.as_deref());
            }
            f.emit_trailing_comments(&r.trailing_comments);
        });
        self.push("}");
    }

    fn format_record_field(&mut self, field: &RecordField) {
        self.push(&format!("{}: ", field.name.name));
        self.format_type_ref(&field.type_ref);
        if let Some(r) = &field.refinement {
            self.push(" where ");
            self.format_refinement(r);
        }
        if let Some(init) = &field.init {
            self.push(" = ");
            self.format_expr(init);
        }
    }

    fn format_sum_body(&mut self, s: &SumBody) {
        // Two surface forms exist; we render the pipe form (clearest for both
        // variants with and without payload). enum form is only meaningful for
        // payloadless variants — round-trip preserves semantics either way.
        let any_payload = s.variants.iter().any(|v| !v.payload.is_empty());
        if !any_payload {
            // Enum-style. #1794: a comment forces the multi-line form, as in
            // a record body.
            let has_comments = !s.trailing_comments.is_empty()
                || s.variants.iter().any(|v| {
                    !v.trivia.leading.is_empty()
                        || v.trivia.trailing.is_some()
                        || v.documentation.is_some()
                });
            if !has_comments {
                let names: Vec<&str> = s.variants.iter().map(|v| v.name.name.as_str()).collect();
                let oneline = format!("enum {{ {} }}", names.join(", "));
                if self.fits(&oneline, 0) {
                    self.push(&oneline);
                    return;
                }
            }
            self.push("enum {");
            self.newline();
            self.indented(|f| {
                for (i, v) in s.variants.iter().enumerate() {
                    f.emit_leading_comments(&v.trivia.leading);
                    if let Some(doc) = &v.documentation {
                        f.emit_doc(doc);
                    }
                    f.push(&v.name.name);
                    if i + 1 < s.variants.len() || f.opts.trailing_comma {
                        f.push(",");
                    }
                    f.emit_trailing_comment_or_newline(v.trivia.trailing.as_deref());
                }
                f.emit_trailing_comments(&s.trailing_comments);
            });
            self.push("}");
            return;
        }
        // Pipe form: break after `=`, one indented `| Variant` per line.
        // #1794: a variant's end-of-line comment ends its line, so the next
        // variant needs no newline of its own.
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        self.indented(|f| {
            for (i, v) in s.variants.iter().enumerate() {
                if i == 0 || s.variants[i - 1].trivia.trailing.is_none() {
                    f.newline();
                }
                f.emit_leading_comments(&v.trivia.leading);
                if let Some(doc) = &v.documentation {
                    f.emit_doc(doc);
                }
                f.push("| ");
                f.push(&v.name.name);
                if v.payload.iter().any(|p| p.documentation.is_some()) {
                    // #1888: a documented payload field puts each field on its
                    // own line, under its doc, as in a record body.
                    f.push("(");
                    f.newline();
                    let last = v.payload.len() - 1;
                    f.indented(|f| {
                        for (j, p) in v.payload.iter().enumerate() {
                            if let Some(doc) = &p.documentation {
                                f.emit_doc(doc);
                            }
                            f.push(&format!(
                                "{}: {}",
                                p.name.name,
                                type_ref_to_string(&p.type_ref)
                            ));
                            if j < last || f.opts.trailing_comma {
                                f.push(",");
                            }
                            f.newline();
                        }
                    });
                    f.push(")");
                } else if !v.payload.is_empty() {
                    f.push("(");
                    let parts: Vec<String> = v
                        .payload
                        .iter()
                        .map(|p| format!("{}: {}", p.name.name, type_ref_to_string(&p.type_ref)))
                        .collect();
                    f.push(&parts.join(", "));
                    f.push(")");
                }
                f.emit_trailing_comment(v.trivia.trailing.as_deref());
            }
            // v0.154 (ADR 0178): the trailing `embeds E as V, …` clause, on its
            // own line under the variants and the comments above it (#1794).
            // Only a variant followed by `embeds` can be last with a comment on
            // its line.
            if !s.embeds.is_empty() {
                if s.variants
                    .last()
                    .is_none_or(|v| v.trivia.trailing.is_none())
                {
                    f.newline();
                }
                f.emit_trailing_comments(&s.trailing_comments);
                let parts: Vec<String> = s
                    .embeds
                    .iter()
                    .map(|e| {
                        format!(
                            "{} as {}",
                            type_ref_to_string(&e.source_type),
                            e.variant.name
                        )
                    })
                    .collect();
                f.push(&format!("embeds {}", parts.join(", ")));
            }
        });
    }

    fn format_type_ref(&mut self, t: &TypeRef) {
        self.push(&type_ref_to_string(t));
    }

    // -- Function declarations --

    fn format_fn_decl(&mut self, f: &FnDecl) {
        self.emit_leading_comments(&f.trivia.leading);
        if let Some(doc) = &f.documentation {
            self.emit_doc(doc);
        }
        // #2618: the unit-private modifier.
        if f.private.is_some() {
            self.push("private ");
        }
        self.push("fn ");
        self.push(&f.name.display());
        // v0.20a: `[A, B]` type parameters.
        if !f.type_params.is_empty() {
            let names: Vec<String> = f.type_params.iter().map(type_param_text).collect();
            self.push(&format!("[{}]", names.join(", ")));
        }
        // A method's `where A: Eq` clause rides after the return type.
        let where_text = if f.where_bounds.is_empty() {
            String::new()
        } else {
            let entries: Vec<String> = f
                .where_bounds
                .iter()
                .map(|wb| format!("{}: {}", wb.param.name, bounds_text(&wb.bounds)))
                .collect();
            format!(" where {}", entries.join(", "))
        };
        // The signature tail that shares the parameter list's line: ` -> Ret`
        // plus the body's ` {` (a contract clause moves the body to its own
        // line, so only the return type counts then).
        let tail = format!(" -> {}{where_text}", type_ref_to_string(&f.return_type));
        let reserve = if f.requires.is_empty() && f.ensures.is_empty() {
            tail.chars().count() + " {".len()
        } else {
            tail.chars().count()
        };
        self.format_params(&f.params, f.has_self, reserve);
        self.push(" -> ");
        self.format_type_ref(&f.return_type);
        self.push(&where_text);
        // v0.115: contract clauses on their own indented lines between the
        // return type and the body (`requires`/`ensures <name>: <pred>`).
        if f.requires.is_empty() && f.ensures.is_empty() {
            self.push(" ");
        } else {
            self.newline();
            self.indented(|f2| {
                for c in &f.requires {
                    f2.push(&format!(
                        "requires {}: {}",
                        c.name.name,
                        expr_to_string(&c.predicate)
                    ));
                    f2.newline();
                }
                for c in &f.ensures {
                    f2.push(&format!(
                        "ensures {}: {}",
                        c.name.name,
                        expr_to_string(&c.predicate)
                    ));
                    f2.newline();
                }
            });
        }
        self.format_block_multiline(&f.body);
        self.emit_trailing_comment_or_newline(f.trivia.trailing.as_deref());
    }

    /// Emit a parameter list. `reserve` is the width of the signature tail that
    /// will follow it on the same line — `-> Ret`, any `by`/`given` clauses,
    /// and the body's opening brace — so a list that only "fits" by ignoring
    /// what comes after it wraps instead (#963).
    fn format_params(&mut self, params: &[Param], has_self: bool, reserve: usize) {
        let mut rendered: Vec<String> = Vec::new();
        if has_self {
            rendered.push("self".to_string());
        }
        // `params` never includes `self` — it is tracked separately via the
        // `has_self` flag (see parser.rs parse_fn_decl).
        for p in params {
            rendered.push(format!(
                "{}: {}",
                p.name.name,
                type_ref_to_string(&p.type_ref)
            ));
        }
        let oneline = format!("({})", rendered.join(", "));
        // An empty list has nothing to wrap onto; `()` always stays put.
        if rendered.is_empty() || self.fits(&oneline, reserve) {
            self.push(&oneline);
            return;
        }
        // Wrapping moves the reserved tail onto the `)` line. When that line
        // would overflow anyway — a `given` list long enough on its own — the
        // wrap costs lines and buys nothing, so keep the single-line form.
        let closing_line = self.indent_level as usize * self.indent_width() + 1 + reserve;
        if closing_line > self.opts.max_line_width as usize {
            self.push(&oneline);
            return;
        }
        // Multi-line params.
        self.push("(");
        self.newline();
        self.indented(|f| {
            for (i, r) in rendered.iter().enumerate() {
                f.push(r);
                // Parameter lists — unlike records, enum/sum variants, agent
                // state fields and exports — do NOT accept a trailing comma in
                // the grammar, so never emit one here regardless of the
                // `trailing_comma` option, or the wrapped output fails to
                // re-parse.
                if i + 1 < rendered.len() {
                    f.push(",");
                }
                f.newline();
            }
        });
        self.push(")");
    }

    // -- Capability / provider / service / agent (v0.5) --

    fn format_capability(&mut self, c: &CapabilityDecl) {
        self.emit_leading_comments(&c.trivia.leading);
        if let Some(doc) = &c.documentation {
            self.emit_doc(doc);
        }
        // S4a (#1966): a derived capability is one line, its options after the
        // base, space-separated as on a `store` field.
        if let Some(d) = &c.derived {
            self.push(&format!("capability {} = {}", c.name.name, d.base.name));
            for ann in &d.annotations {
                self.push(&format!(" {}", annotation_to_string(ann)));
            }
            self.emit_trailing_comment_or_newline(c.trivia.trailing.as_deref());
            return;
        }
        self.push(&format!("capability {} {{", c.name.name));
        self.newline();
        self.indented(|f| {
            for op in &c.ops {
                f.emit_leading_comments(&op.trivia.leading);
                if let Some(doc) = &op.documentation {
                    f.emit_doc(doc);
                }
                f.push("fn ");
                f.push(&op.name.name);
                // #926: `[T, …]` type parameters on the op itself.
                if !op.type_params.is_empty() {
                    let names: Vec<String> = op.type_params.iter().map(type_param_text).collect();
                    f.push(&format!("[{}]", names.join(", ")));
                }
                let reserve = 4 + type_ref_to_string(&op.return_type).chars().count();
                f.format_params(&op.params, false, reserve);
                f.push(" -> ");
                f.format_type_ref(&op.return_type);
                f.emit_trailing_comment_or_newline(op.trivia.trailing.as_deref());
            }
            // #1756: the comments before the closing `}`, after a blank line.
            if !c.trailing_comments.is_empty() {
                if !c.ops.is_empty() {
                    f.newline();
                }
                f.emit_trailing_comments(&c.trailing_comments);
            }
        });
        self.push("}");
        self.emit_trailing_comment_or_newline(c.trivia.trailing.as_deref());
    }

    fn format_provider(&mut self, p: &ProviderDecl) {
        self.emit_leading_comments(&p.trivia.leading);
        if let Some(doc) = &p.documentation {
            self.emit_doc(doc);
        }
        self.push(&format!(
            "provides {} = {}",
            p.capability.name, p.provider_name.name
        ));
        if !p.given.is_empty() {
            self.push(" given ");
            let names: Vec<String> = p.given.iter().map(cap_ref_src).collect();
            self.push(&names.join(", "));
        }
        // v0.17: an external provider (inside an adapter) has no body.
        if p.external {
            self.emit_trailing_comment_or_newline(p.trivia.trailing.as_deref());
            return;
        }
        self.push(" {");
        self.newline();
        self.indented(|f| {
            for (i, op) in p.ops.iter().enumerate() {
                if i > 0 {
                    f.newline();
                }
                f.emit_leading_comments(&op.trivia.leading);
                f.push("fn ");
                f.push(&op.name.name);
                let reserve = 4 + type_ref_to_string(&op.return_type).chars().count() + 2;
                f.format_params(&op.params, false, reserve);
                f.push(" -> ");
                f.format_type_ref(&op.return_type);
                f.push(" ");
                f.format_block_multiline(&op.body);
                f.emit_trailing_comment_or_newline(op.trivia.trailing.as_deref());
            }
        });
        self.push("}");
        self.emit_trailing_comment_or_newline(p.trivia.trailing.as_deref());
    }

    fn format_service(&mut self, s: &ServiceDecl) {
        self.emit_leading_comments(&s.trivia.leading);
        if let Some(doc) = &s.documentation {
            self.emit_doc(doc);
        }
        let from = match &s.protocol {
            ServiceProtocol::Call => String::new(),
            ServiceProtocol::Http => " from http".to_string(),
            ServiceProtocol::Cron => " from cron".to_string(),
            ServiceProtocol::Queue { name } => {
                format!(" from queue(\"{}\")", escape_string(name))
            }
            ServiceProtocol::WebSocket { in_type, out_type } => {
                format!(
                    " from websocket(in: {}, out: {})",
                    type_ref_to_string(in_type),
                    type_ref_to_string(out_type)
                )
            }
            ServiceProtocol::Events {
                event_type,
                pattern,
                schema_dispatch,
            } => {
                let header = match pattern {
                    Some(p) => format!(
                        " from Events({} {})",
                        type_ref_to_string(event_type),
                        event_pattern_src(p)
                    ),
                    None => format!(" from Events({})", type_ref_to_string(event_type)),
                };
                match schema_dispatch {
                    Some(d) => format!("{header} {}", schema_dispatch_src(d)),
                    None => header,
                }
            }
        };
        // v0.155: the optional service-level `by`/`given` defaults follow the
        // protocol on the header, `by` first — the ambient contract every handler
        // inherits unless it declares its own.
        let mut header = format!("service {}{}", s.name.name, from);
        if let Some(by) = &s.default_by {
            header.push_str(&format!(" {}", by_clause_src(by)));
        }
        if !s.default_given.is_empty() {
            let names: Vec<String> = s.default_given.iter().map(cap_ref_src).collect();
            header.push_str(&format!(" given {}", names.join(", ")));
        }
        self.push(&format!("{header} {{"));
        self.newline();
        self.indented(|f| {
            // v0.131/v0.141/v0.142: the CORS, security, and limits policies are
            // header-position sections, before the handlers (mirroring the agent
            // phase order). A canonical order — `cors`, then `security`, then
            // `limits` — with a blank line between each section.
            if let Some(cors) = &s.cors {
                f.format_cors_policy(cors);
                if s.security.is_some() || s.limits.is_some() || !s.handlers.is_empty() {
                    f.newline();
                }
            }
            if let Some(security) = &s.security {
                f.format_security_policy(security);
                if s.limits.is_some() || !s.handlers.is_empty() {
                    f.newline();
                }
            }
            if let Some(limits) = &s.limits {
                f.format_limits_policy(limits);
                if !s.handlers.is_empty() {
                    f.newline();
                }
            }
            for (i, h) in s.handlers.iter().enumerate() {
                if i > 0 {
                    f.newline();
                }
                f.format_handler(h);
            }
            // #1756: the comments before the closing `}`, after a blank line.
            if !s.trailing_comments.is_empty() {
                if !s.handlers.is_empty()
                    || s.cors.is_some()
                    || s.security.is_some()
                    || s.limits.is_some()
                {
                    f.newline();
                }
                f.emit_trailing_comments(&s.trailing_comments);
            }
        });
        self.push("}");
        self.emit_trailing_comment_or_newline(s.trivia.trailing.as_deref());
    }

    /// Format a `cors { }` policy section (v0.131). One `name: value` field per
    /// line, with a trailing comma, mirroring a record construction.
    fn format_cors_policy(&mut self, cors: &CorsPolicy) {
        let fields: Vec<_> = cors
            .fields
            .iter()
            .map(|f| (&f.name, &f.value, &f.trivia))
            .collect();
        self.format_policy("cors", &cors.trivia, &fields, &cors.trailing_comments);
    }

    /// Format a `security { }` policy section (v0.141), as `format_cors_policy`.
    fn format_security_policy(&mut self, security: &SecurityPolicy) {
        let fields: Vec<_> = security
            .fields
            .iter()
            .map(|f| (&f.name, &f.value, &f.trivia))
            .collect();
        self.format_policy(
            "security",
            &security.trivia,
            &fields,
            &security.trailing_comments,
        );
    }

    /// Format a `limits { }` policy section (v0.142), as `format_cors_policy`. A
    /// `maxBody` value keeps its as-written `_` digit separators (the `IntLit`
    /// lexeme).
    fn format_limits_policy(&mut self, limits: &LimitsPolicy) {
        let fields: Vec<_> = limits
            .fields
            .iter()
            .map(|f| (&f.name, &f.value, &f.trivia))
            .collect();
        self.format_policy("limits", &limits.trivia, &fields, &limits.trailing_comments);
    }

    /// A `cors`/`security`/`limits` policy: `keyword {`, one `name: value,` field
    /// per line, `}`. #1786: each field's comments, the comments before `}`, and
    /// the comment after it are printed in place.
    fn format_policy(
        &mut self,
        keyword: &str,
        trivia: &Trivia,
        fields: &[(&Ident, &Expr, &Trivia)],
        trailing_comments: &[Comment],
    ) {
        self.emit_leading_comments(&trivia.leading);
        self.push(&format!("{keyword} {{"));
        self.newline();
        self.indented(|f| {
            for (name, value, field_trivia) in fields {
                f.emit_leading_comments(&field_trivia.leading);
                f.push(&format!("{}: ", name.name));
                f.format_expr_at(value, 0, 1);
                f.push(",");
                f.emit_trailing_comment_or_newline(field_trivia.trailing.as_deref());
            }
            f.emit_trailing_comments(trailing_comments);
        });
        self.push("}");
        self.emit_trailing_comment_or_newline(trivia.trailing.as_deref());
    }

    fn format_agent(&mut self, a: &AgentDecl) {
        self.emit_leading_comments(&a.trivia.leading);
        if let Some(doc) = &a.documentation {
            self.emit_doc(doc);
        }
        match &a.replaces {
            Some(old) => self.push(&format!("agent {} replaces {} {{", a.name.name, old.name)),
            None => self.push(&format!("agent {} {{", a.name.name)),
        }
        self.newline();
        self.indented(|f| {
            // key
            f.emit_leading_comments(&a.key_trivia.leading);
            f.push(&format!(
                "key {}: {}",
                a.key_name.name,
                type_ref_to_string(&a.key_type)
            ));
            f.emit_trailing_comment_or_newline(a.key_trivia.trailing.as_deref());
            f.newline();
            // storage (v0.81, storage track): the agent's `store` fields.
            for sf in &a.store_fields {
                f.format_store_field(sf);
            }
            // v0.80: invariants form a phase between the storage fields and the
            // handlers.
            for inv in &a.invariants {
                f.newline();
                f.format_invariant(inv);
            }
            // v0.116: step invariants form part of the same phase, beside the
            // snapshot invariants.
            for tr in &a.transitions {
                f.newline();
                f.format_transition(tr);
            }
            // State-migrations S2 (#2224): `migrate` steps, in the same phase.
            for m in &a.migrations {
                f.newline();
                f.format_migrate(m);
            }
            // handlers
            for h in &a.handlers {
                f.newline();
                f.format_handler(h);
            }
            // #1756: the comments before the closing `}`, after a blank line.
            if !a.trailing_comments.is_empty() {
                f.newline();
                f.emit_trailing_comments(&a.trailing_comments);
            }
        });
        self.push("}");
        self.emit_trailing_comment_or_newline(a.trivia.trailing.as_deref());
    }

    /// Format a `store` field (v0.81): `store <name>: <Kind> [= <init>]`, with
    /// its leading comments / doc, ending its line with the trailing comment
    /// or a bare newline, never both (#1859).
    fn format_store_field(&mut self, sf: &StoreField) {
        self.emit_leading_comments(&sf.trivia.leading);
        if let Some(doc) = &sf.documentation {
            self.emit_doc(doc);
        }
        self.push(&format!(
            "store {}: {}",
            sf.name.name,
            store_kind_to_string(&sf.kind)
        ));
        // v0.85 (ADR 0111): annotations follow the kind, one space-separated each.
        for ann in &sf.annotations {
            self.push(&format!(" {}", annotation_to_string(ann)));
        }
        if let Some(init) = &sf.init {
            self.push(" = ");
            self.format_expr(init);
        }
        self.emit_trailing_comment_or_newline(sf.trivia.trailing.as_deref());
    }

    /// Format an agent invariant (v0.80): the name on one line, the predicate
    /// indented beneath, matching the §14 worked examples.
    fn format_invariant(&mut self, inv: &Invariant) {
        self.emit_leading_comments(&inv.trivia.leading);
        if let Some(doc) = &inv.documentation {
            self.emit_doc(doc);
        }
        self.push(&format!("invariant {}:", inv.name.name));
        self.newline();
        self.indented(|f| {
            f.format_expr(&inv.predicate);
        });
        self.emit_trailing_comment_or_newline(inv.trivia.trailing.as_deref());
    }

    /// Format an agent step invariant (v0.116): `transition <name>:` with the
    /// `old`/`new` predicate indented beneath, mirroring [`format_invariant`].
    fn format_transition(&mut self, tr: &Transition) {
        self.emit_leading_comments(&tr.trivia.leading);
        if let Some(doc) = &tr.documentation {
            self.emit_doc(doc);
        }
        self.push(&format!("transition {}:", tr.name.name));
        self.newline();
        self.indented(|f| {
            f.format_expr(&tr.predicate);
        });
        self.emit_trailing_comment_or_newline(tr.trivia.trailing.as_deref());
    }

    /// Format a migration step (state-migrations S2, #2224):
    /// `migrate from N (old: T) { … }`, one item per line.
    fn format_migrate(&mut self, m: &MigrateDecl) {
        self.emit_leading_comments(&m.trivia.leading);
        if let Some(doc) = &m.documentation {
            self.emit_doc(doc);
        }
        self.push(&format!("migrate from {}", m.from));
        if let Some(old) = &m.old {
            self.push(&format!(
                " ({}: {})",
                old.name.name,
                type_ref_to_string(&old.type_ref)
            ));
        }
        self.push(" {");
        self.newline();
        self.indented(|f| {
            for item in &m.items {
                let trivia = match item {
                    MigrateItem::Let(l) => &l.trivia,
                    MigrateItem::Assign(a) => &a.trivia,
                    MigrateItem::Drop(d) => &d.trivia,
                };
                f.emit_leading_comments(&trivia.leading);
                match item {
                    MigrateItem::Let(l) => {
                        f.push("let ");
                        f.push(&l.name.name);
                        if let Some(t) = &l.type_annot {
                            f.push(": ");
                            f.format_type_ref(t);
                        }
                        f.push(" = ");
                        f.format_expr(&l.value);
                    }
                    MigrateItem::Assign(a) => {
                        f.push(&a.target.name);
                        f.push(" := ");
                        f.format_expr(&a.value);
                    }
                    MigrateItem::Drop(d) => f.push(&format!("drop {}", d.field.name)),
                }
                f.emit_trailing_comment_or_newline(trivia.trailing.as_deref());
            }
            if !m.trailing_comments.is_empty() {
                f.emit_trailing_comments(&m.trailing_comments);
            }
        });
        self.push("}");
        self.emit_trailing_comment_or_newline(m.trivia.trailing.as_deref());
    }

    fn format_actor(&mut self, a: &ActorDecl) {
        self.emit_leading_comments(&a.trivia.leading);
        if let Some(doc) = &a.documentation {
            self.emit_doc(doc);
        }
        if let Some(r) = &a.refinement {
            // Reserved refinement form: `actor Name = Base where <predicate>`.
            self.push(&format!(
                "actor {} = {} where {}",
                a.name.name,
                r.base.name,
                expr_to_string(&r.predicate)
            ));
        } else {
            // Normal form: `actor Name { auth = Scheme(, identity = Type)? }`.
            let auth = a.auth.as_ref().map(|i| i.name.as_str()).unwrap_or("None");
            let args: Vec<String> = a
                .auth_config
                .iter()
                .map(|arg| match &arg.value {
                    bynk_syntax::ast::SchemeArgValue::Str(s) => {
                        format!("{} = \"{}\"", arg.key.name, escape_string(s))
                    }
                    bynk_syntax::ast::SchemeArgValue::Int(n) => {
                        format!("{} = {n}", arg.key.name)
                    }
                })
                .collect();
            let config = if args.is_empty() {
                String::new()
            } else {
                format!("({})", args.join(", "))
            };
            let identity = a.identity.as_ref().map(type_ref_to_string);
            let oneline = format!(
                "actor {} {{ auth = {auth}{config}{} }}",
                a.name.name,
                identity
                    .as_ref()
                    .map(|id| format!(", identity = {id}"))
                    .unwrap_or_default()
            );
            // An OIDC-style scheme carries issuer / audience / JWKS URLs that
            // blow past any line budget on one line (#963): open the braces and
            // give each scheme argument its own line. The test ignores comments,
            // so commenting an actor never changes how its arguments break.
            let break_args = !args.is_empty() && !self.fits(&oneline, 0);
            // #1797: a comment anywhere in the body forces the multi-line form.
            let has_comments = !a.trailing_comments.is_empty()
                || [&a.auth_trivia, &a.identity_trivia]
                    .iter()
                    .any(|t| !t.leading.is_empty() || t.trailing.is_some());
            if !break_args && !has_comments {
                self.push(&oneline);
            } else {
                self.push(&format!("actor {} {{", a.name.name));
                self.newline();
                self.indented(|f| {
                    f.emit_leading_comments(&a.auth_trivia.leading);
                    if break_args {
                        f.push(&format!("auth = {auth}("));
                        f.newline();
                        f.indented(|f2| {
                            for (i, arg) in args.iter().enumerate() {
                                f2.push(arg);
                                if i + 1 < args.len() {
                                    f2.push(",");
                                }
                                f2.newline();
                            }
                        });
                        f.push(")");
                    } else {
                        f.push(&format!("auth = {auth}{config}"));
                    }
                    if let Some(id) = &identity {
                        // `identity` is a sibling of `auth`, so its comma stays
                        // with `auth`'s line and it starts a new line.
                        f.push(",");
                        f.emit_trailing_comment_or_newline(a.auth_trivia.trailing.as_deref());
                        f.emit_leading_comments(&a.identity_trivia.leading);
                        f.push(&format!("identity = {id}"));
                        f.emit_trailing_comment_or_newline(a.identity_trivia.trailing.as_deref());
                    } else {
                        f.emit_trailing_comment_or_newline(a.auth_trivia.trailing.as_deref());
                    }
                    f.emit_trailing_comments(&a.trailing_comments);
                });
                self.push("}");
            }
        }
        self.emit_trailing_comment_or_newline(a.trivia.trailing.as_deref());
    }

    fn format_handler(&mut self, h: &Handler) {
        self.emit_leading_comments(&h.trivia.leading);
        if let Some(doc) = &h.documentation {
            self.emit_doc(doc);
        }
        // v0.140 (ADR 0163): handler-position annotations (`@cache(…)`) print one
        // per line above the `on`, mirroring how decorators read in source. Each is
        // rendered by the shared `annotation_to_string` used for `store` fields.
        for ann in &h.annotations {
            self.push(&annotation_to_string(ann));
            self.newline();
        }
        // The handler kind prefix: `on call`, `on http METHOD "path"`, or
        // `on cron("expr")`. Agent `on call` handlers carry a method name.
        match &h.kind {
            HandlerKind::Call => {
                self.push("on call");
                if let Some(m) = &h.method_name {
                    self.push(&format!(" {}", m.name));
                }
            }
            HandlerKind::Http { method, path } => {
                // Trailing space: the path string is followed by the param list,
                // which reads better separated (`… "/path" (params)`).
                self.push(&format!(
                    "on {}(\"{}\") ",
                    method.as_str(),
                    escape_string(path)
                ));
            }
            HandlerKind::Cron { expr } => {
                self.push(&format!("on schedule(\"{}\") ", escape_string(expr)));
            }
            HandlerKind::Message => {
                self.push("on message");
            }
            HandlerKind::Open => {
                self.push("on open");
            }
            HandlerKind::Close => {
                self.push("on close");
            }
            HandlerKind::Event => {
                self.push("on event");
            }
            HandlerKind::Rejected => {
                self.push("on rejected");
            }
        }
        // The param list follows the kind prefix directly — `on call(params)`,
        // `on open(params)` — while the Http/Cron prefixes already emit a trailing
        // space (`on GET("/x") (params)`). (v0.155: the `by` clause no longer sits
        // here, so no separating space is needed.)
        // Everything from `-> Ret` to the body's `{` shares the parameter
        // list's line and none of it can wrap (the `given` list in particular
        // is newline-sensitive), so the whole tail is reserved up front and the
        // parameters are what gives (#963).
        let mut tail = format!(" -> {}", type_ref_to_string(&h.return_type));
        if let Some(by) = &h.by_clause {
            tail.push_str(&format!(" {}", by_clause_src(by)));
        }
        if !h.given.is_empty() {
            let names: Vec<String> = h.given.iter().map(cap_ref_src).collect();
            tail.push_str(&format!(" given {}", names.join(", ")));
        }
        self.format_params(&h.params, false, tail.chars().count() + " {".len());
        self.push(&tail);
        self.push(" ");
        self.format_block_multiline(&h.body);
        self.emit_trailing_comment_or_newline(h.trivia.trailing.as_deref());
    }

    // -- Blocks, statements, expressions --

    fn format_block(&mut self, b: &Block) {
        self.format_block_with_reserve(b, 0);
    }

    /// Format a block, knowing that `reserve` columns of text will follow its
    /// closing brace on the same line (` else {` on an `if`'s then-branch, an
    /// arm's `,`). Only a block that fits *including* that tail stays inline.
    fn format_block_with_reserve(&mut self, b: &Block, reserve: usize) {
        // A block with no statements, no trivia, and a simple tail
        // expression can be emitted inline if it fits; otherwise multi-line.
        let tail_oneline = expr_to_string(&b.tail);
        let any_stmt_trivia = b.statements.iter().any(|s| !statement_trivia(s).is_empty());
        if b.statements.is_empty()
            && b.tail_leading_comments.is_empty()
            && !any_stmt_trivia
            && self.fits(&format!("{{ {tail_oneline} }}"), reserve)
        {
            self.push("{ ");
            self.push(&tail_oneline);
            self.push(" }");
            return;
        }
        self.format_block_multiline(b);
    }

    /// The multi-line block form: brace, one statement per indented line, the
    /// tail expression, closing brace. Split out of [`Self::format_block`] so
    /// the wrapped-expression printer can force it (#963) for an `if`/lambda
    /// body whose single-line form would overflow.
    fn format_block_multiline(&mut self, b: &Block) {
        // A body that is only the implicit unit prints as `{}`, not as an
        // open and close brace on separate lines around nothing.
        if b.statements.is_empty() && b.tail_leading_comments.is_empty() && omit_unit_tail(b) {
            self.push("{}");
            return;
        }
        self.push("{");
        self.newline();
        self.indented(|f| f.format_block_body(b));
        self.push("}");
    }

    /// A block's statements and tail, one per line at the current indent.
    /// #2010: a `let … else` reaches the formatter desugared — a hidden
    /// binding, then a `match` tail whose first arm holds the rest of the
    /// block — so it is printed back as the author wrote it, and the rest of
    /// the block continues at this same indent.
    fn format_block_body(&mut self, b: &Block) {
        for stmt in &b.statements {
            let trivia = statement_trivia(stmt);
            self.emit_leading_comments(&trivia.leading);
            if let Some((l, effectful, le)) = let_else_binding(stmt)
                && let ExprKind::Match { arms, .. } = &b.tail.kind
                && let Some(MatchBody::Block(rest)) = arms.first().map(|a| &a.body)
            {
                self.format_let_else(l, effectful, le, arms);
                self.emit_trailing_comment_or_newline(trivia.trailing.as_deref());
                self.format_block_body(rest);
                return;
            }
            self.format_statement(stmt);
            self.emit_trailing_comment_or_newline(trivia.trailing.as_deref());
        }
        self.emit_leading_comments(&b.tail_leading_comments);
        if !omit_unit_tail(b) {
            self.format_expr(&b.tail);
            self.newline();
        }
    }

    /// #2010: `let <pattern> (= | <-) <value> else <block>` or
    /// `… else match { <arms> }`, from the hidden binding `l` and the arms of
    /// the `match` it was desugared into (arm 0 is the pattern's).
    fn format_let_else(&mut self, l: &LetStmt, effectful: bool, le: &LetElse, arms: &[MatchArm]) {
        self.push("let ");
        self.push(&let_pattern_src(&arms[0].pattern));
        self.push(if effectful { " <- " } else { " = " });
        let principal = l
            .principal
            .as_ref()
            .map(|p| format!(" {}", call_site_actor_src(p)));
        let else_width = if le.form == LetElseForm::Plain {
            0
        } else {
            " else {".len()
        };
        let reserve = principal.as_deref().map_or(0, |p| p.chars().count()) + else_width;
        self.format_expr_at(&l.value, 0, reserve);
        if let Some(principal) = principal {
            self.push(&principal);
        }
        // #2654: the `else`-less form of an irrefutable record pattern.
        if le.form == LetElseForm::Plain {
            return;
        }
        self.push(" else ");
        match (le.form, arms.get(1).map(|a| &a.body)) {
            (LetElseForm::Block, Some(MatchBody::Block(otherwise))) => self.format_block(otherwise),
            _ => {
                self.push("match {");
                self.newline();
                self.indented(|f| f.format_arms(&arms[1..]));
                self.push("}");
            }
        }
    }

    fn format_statement(&mut self, s: &Statement) {
        match s {
            Statement::Let(l) => {
                self.push("let ");
                self.push(&l.name.name);
                if let Some(t) = &l.type_annot {
                    self.push(": ");
                    self.format_type_ref(t);
                }
                self.push(" = ");
                self.format_expr(&l.value);
            }
            Statement::EffectLet(l) => {
                self.push("let ");
                self.push(&l.name.name);
                if let Some(t) = &l.type_annot {
                    self.push(": ");
                    self.format_type_ref(t);
                }
                self.push(" <- ");
                // The `by <Actor>` clause trails the value on the same line.
                let principal = l
                    .principal
                    .as_ref()
                    .map(|p| format!(" {}", call_site_actor_src(p)));
                let reserve = principal.as_deref().map_or(0, |p| p.chars().count());
                self.format_expr_at(&l.value, 0, reserve);
                if let Some(principal) = principal {
                    self.push(&principal);
                }
            }
            Statement::Expect(a) => {
                self.push("expect ");
                self.format_expr(&a.value);
            }
            Statement::Send(s) => {
                self.push("~> ");
                self.format_expr(&s.value);
            }
            Statement::Do(d) => {
                self.push("do ");
                self.format_expr(&d.value);
            }
            Statement::Assign(a) => {
                self.push(&a.target.name);
                self.push(" := ");
                self.format_expr(&a.value);
            }
            Statement::Advance(a) => {
                self.push(&format!("advance {} by ", a.cap.name));
                self.format_expr(&a.amount);
            }
            Statement::Par(p) => {
                self.push("par {");
                self.newline();
                self.indented(|f| {
                    for stmt in &p.statements {
                        let trivia = statement_trivia(stmt);
                        f.emit_leading_comments(&trivia.leading);
                        f.format_statement(stmt);
                        f.emit_trailing_comment_or_newline(trivia.trailing.as_deref());
                    }
                    f.emit_leading_comments(&p.closing_comments);
                });
                self.push("}");
            }
        }
    }

    fn format_expr(&mut self, e: &Expr) {
        self.format_expr_at(e, 0, 0);
    }

    /// Emit `e` at the current column, breaking it across lines when its
    /// single-line form would overrun the line budget (#963).
    ///
    /// `parent_prec` is the enclosing operator's precedence, exactly as in
    /// [`expr_with_prec`] — it decides parenthesisation. `reserve` is the width
    /// of text the caller will emit after this expression on the same line (a
    /// closing `)`, an arm's `,`, ` else {`), so a sub-expression is not judged
    /// to fit on the strength of a line it does not in fact end.
    ///
    /// The flat form always wins when it fits: this only ever *adds* line
    /// breaks, and only at points the grammar accepts (verified by the
    /// round-trip guard in [`format_source`]).
    fn format_expr_at(&mut self, e: &Expr, parent_prec: u8, reserve: usize) {
        // `match` renders multi-line unconditionally, so it must go through the
        // indent-aware emitter rather than `expr_to_string` — the latter builds
        // a flat string with hardcoded single-tab arms that ignores the current
        // nesting depth (the closing brace and every arm would land at column
        // one regardless of how deeply the `match` is nested).
        if let ExprKind::Match { discriminant, arms } = &e.kind {
            self.format_match(discriminant, arms);
            return;
        }
        let flat = expr_with_prec(e, parent_prec);
        // A braced record literal with more than one item (several fields,
        // or a spread with overrides) always breaks, one per line, however
        // short.
        let braced_record = match &e.kind {
            ExprKind::RecordConstruction { fields, .. } => fields.len() > 1,
            ExprKind::RecordSpread { overrides, .. } => !overrides.is_empty(),
            _ => false,
        };
        if !braced_record && self.fits(&flat, reserve) {
            self.push(&flat);
            return;
        }
        // Too wide. Re-emit broken across lines — inside the parentheses the
        // flat form would have added, if precedence calls for them.
        if needs_parens(e, parent_prec) {
            self.push("(");
            self.format_expr_broken(e, reserve + 1);
            self.push(")");
        } else {
            self.format_expr_broken(e, reserve);
        }
    }

    /// The multi-line rendering of an expression that does not fit. Each arm
    /// breaks at a point the grammar tolerates a newline; anything with no such
    /// point (a long string literal, an identifier) falls through to the flat
    /// form, which simply overruns — the 100-column target is soft.
    fn format_expr_broken(&mut self, e: &Expr, reserve: usize) {
        match &e.kind {
            // `T { field: value, … }` — one field per line.
            ExprKind::RecordConstruction { type_name, fields } if !fields.is_empty() => {
                self.push(&format!("{} {{", type_name.name));
                self.format_field_inits(fields.iter(), None);
            }
            // `T { ...base, field: value, … }` — the spread first, then the
            // overrides, one per line.
            ExprKind::RecordSpread {
                type_name,
                base,
                overrides,
            } => {
                match type_name {
                    Some(tn) => self.push(&format!("{} {{", tn.name)),
                    None => self.push("{"),
                }
                let spread = format!("...{}", expr_with_prec(base, 0));
                self.format_field_inits(overrides.iter(), Some(&spread));
            }
            // A call's arguments, one per line. Unlike a record body an
            // argument list does NOT accept a trailing comma (the grammar
            // rejects it), so the wrapped form never emits one.
            ExprKind::Call {
                name,
                type_args,
                args,
            } if !args.is_empty() => {
                self.push(&format!("{}{}(", name.name, type_args_src(type_args)));
                self.format_arg_list(args, reserve);
            }
            ExprKind::Val { type_ref, args } if !args.is_empty() => {
                self.push(&format!("Val[{}](", type_ref_to_string(type_ref)));
                self.format_arg_list(args, reserve);
            }
            // A `.`-chain: `receiver.a().b()` — kept on one line where the
            // overflow is an argument's, broken before each call where it is
            // the chain's own (see `format_chain`).
            ExprKind::MethodCall { .. } | ExprKind::FieldAccess { .. } => {
                self.format_chain(e, reserve);
            }
            // `[a, b, c]` — one element per line.
            ExprKind::ListLit(elems) if !elems.is_empty() => {
                self.push("[");
                self.newline();
                self.indented(|f| {
                    for (i, elem) in elems.iter().enumerate() {
                        let last = i + 1 == elems.len();
                        f.format_expr_at(elem, 0, if last { 0 } else { 1 });
                        if !last || f.opts.trailing_comma {
                            f.push(",");
                        }
                        f.newline();
                    }
                });
                self.push("]");
            }
            // A run of `&&` / `||` / `implies` breaks before each operator, per
            // the spec's "wraps at `&&`/`||` boundaries". Only these: a
            // continuation line starting with an arithmetic or comparison
            // operator does not re-attach to the line above on re-parse.
            ExprKind::BinOp(op, ..) if is_logical(*op) => {
                let prec = binop_prec(*op);
                let mut operands = Vec::new();
                flatten_binop(e, *op, &mut operands);
                self.format_expr_at(operands[0], prec, 0);
                self.indented(|f| {
                    for (i, operand) in operands.iter().enumerate().skip(1) {
                        f.newline();
                        f.push(&format!("{} ", op.name()));
                        let last = i + 1 == operands.len();
                        f.format_expr_at(operand, prec + 1, if last { reserve } else { 0 });
                    }
                });
            }
            // Any other binary operator stays on one line, but its operands may
            // still break internally (a record or call on either side).
            ExprKind::BinOp(op, lhs, rhs) => {
                let prec = binop_prec(*op);
                let tail = format!(" {} {}", op.name(), expr_with_prec(rhs, prec + 1));
                // The right-hand side shares the operator's line whenever it is
                // itself unbroken, so charge it to the left-hand side's budget.
                let lhs_reserve = if tail.contains('\n') {
                    0
                } else {
                    tail.chars().count() + reserve
                };
                self.format_expr_at(lhs, prec, lhs_reserve);
                self.push(&format!(" {} ", op.name()));
                self.format_expr_at(rhs, prec + 1, reserve);
            }
            ExprKind::Is { value, pattern } => {
                let pat = format!(" is {}", is_pattern_src(pattern));
                self.format_expr_at(value, 4, pat.chars().count() + reserve);
                self.push(&pat);
            }
            // `if cond { … } else { … }` — both branches go vertical. Once the
            // one-line form is over budget, splitting only one branch leaves a
            // lopsided line that is no easier to read.
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.push("if ");
                self.format_expr_at(cond, 0, 2);
                self.push(" ");
                self.format_block_multiline(then_block);
                // v0.146 (ADR 0170): an `if` with no `else` carries a
                // synthesised unit else-branch — omit it, as the flat form does.
                if !else_block.is_synth_unit() {
                    self.push(" else ");
                    self.format_block_multiline(else_block);
                }
            }
            ExprKind::Block(b) => self.format_block_multiline(b),
            ExprKind::Lambda(lambda) => {
                let params: Vec<String> = lambda
                    .params
                    .iter()
                    .map(|p| match &p.type_ref {
                        Some(tr) => format!("{}: {}", p.name.name, type_ref_to_string(tr)),
                        None => p.name.name.clone(),
                    })
                    .collect();
                self.push(&format!("({}) => ", params.join(", ")));
                self.format_expr_at(&lambda.body, 0, reserve);
            }
            // Single-argument wrappers: nothing to break at the wrapper itself,
            // so recurse and let the payload wrap inside the parentheses.
            ExprKind::Ok(v) => self.wrap_call("Ok(", v, reserve),
            ExprKind::Err(v) => self.wrap_call("Err(", v, reserve),
            ExprKind::Some(v) => self.wrap_call("Some(", v, reserve),
            ExprKind::EffectPure(v) => self.wrap_call("Effect.pure(", v, reserve),
            ExprKind::Wire(v) => self.wrap_call("Wire(", v, reserve),
            ExprKind::Paren(v) => self.wrap_call("(", v, reserve),
            ExprKind::Question(v) => {
                self.format_expr_at(v, 8, reserve + 1);
                self.push("?");
            }
            ExprKind::Expect(v) => {
                self.push("expect ");
                self.format_expr_at(v, 0, reserve);
            }
            // Nothing breakable — a literal, an identifier, an interpolated
            // string. Emit it as-is and overrun.
            _ => self.push(&expr_with_prec(e, 0)),
        }
    }

    /// `<head><inner>)` where `inner` wraps inside the parentheses.
    fn wrap_call(&mut self, head: &str, inner: &Expr, reserve: usize) {
        self.push(head);
        self.format_expr_at(inner, 0, reserve + 1);
        self.push(")");
    }

    /// The body of a wrapped record construction or spread: one `name: value`
    /// per indented line, then the closing brace. `spread` is the leading
    /// `...base` entry, when there is one. The brace and any type name are the
    /// caller's to emit.
    fn format_field_inits<'f, I>(&mut self, fields: I, spread: Option<&str>)
    where
        I: ExactSizeIterator<Item = &'f FieldInit>,
    {
        let total = fields.len() + usize::from(spread.is_some());
        self.newline();
        self.indented(|f| {
            let mut emitted = 0usize;
            if let Some(spread) = spread {
                f.push(spread);
                emitted += 1;
                if emitted < total || f.opts.trailing_comma {
                    f.push(",");
                }
                f.newline();
            }
            for field in fields {
                f.push(&field.name.name);
                if let Some(v) = &field.value {
                    f.push(": ");
                    f.format_expr_at(v, 0, 1);
                }
                emitted += 1;
                if emitted < total || f.opts.trailing_comma {
                    f.push(",");
                }
                f.newline();
            }
        });
        self.push("}");
    }

    /// The arguments of a wrapped call plus its closing `)` — the caller has
    /// already emitted the `name(` head.
    ///
    /// A trailing argument hugs the call — the earlier arguments stay on the
    /// call's line and it opens its own body there, so
    /// `xs.fold(init, (acc, x) => match acc {` reads as one construct instead
    /// of being pushed down a level. Hugging is attempted for a sole argument
    /// (there is no sibling for it to misalign against) and, past that, only
    /// for a trailing lambda / record / block / `match` / `if`, whose opening
    /// line is short. It is taken only if every line it produces fits;
    /// otherwise each argument goes on its own indented line.
    ///
    /// Never a trailing comma — the grammar rejects one in an argument list,
    /// and the wrapped output has to re-parse.
    fn format_arg_list(&mut self, args: &[Expr], reserve: usize) -> bool {
        if let Some((last, leading)) = args.split_last()
            && (leading.is_empty() || is_block_like(last))
            && self.try_layout(reserve, |f| {
                for arg in leading {
                    f.push(&expr_with_prec(arg, 0));
                    f.push(", ");
                }
                f.format_expr_at(last, 0, 1);
                f.push(")");
            })
        {
            return false;
        }
        self.newline();
        self.indented(|f| {
            for (i, arg) in args.iter().enumerate() {
                let last = i + 1 == args.len();
                f.format_expr_at(arg, 0, if last { 0 } else { 1 });
                if !last {
                    f.push(",");
                }
                f.newline();
            }
        });
        self.push(")");
        true
    }

    /// Emit a `.`-chain (`receiver.a(…).b(…).c`) broken across lines.
    ///
    /// A single-call chain never breaks at its `.` — there is no pipeline to
    /// read and the overflow belongs to the argument list. A multi-call chain
    /// prefers to stay on one line too, and breaks before each call only when
    /// staying inline would strand an exploded argument list mid-chain.
    fn format_chain(&mut self, e: &Expr, reserve: usize) {
        let (base, links) = flatten_chain(e);
        let calls = links
            .iter()
            .filter(|l| matches!(l, ChainLink::Method { .. }))
            .count();
        if calls < 2 {
            self.format_chain_inline(base, &links, reserve);
            return;
        }
        // Keeping a multi-call chain intact is preferable when the overflow
        // belongs to an argument rather than to the chain — the
        // `xs.fold(init, (acc, x) => match acc {` shape, where a body opens on
        // the chain's own line. That reading survives only while every step
        // either fits or *hugs*. A step forced to put its arguments one per
        // line strands a bare `)` mid-chain, at which point the chain itself is
        // what is too long, and breaking at its dots reads better.
        let exploded = std::cell::Cell::new(false);
        if self.try_layout_if(
            reserve,
            |f| exploded.set(f.format_chain_inline(base, &links, reserve)),
            || !exploded.get(),
        ) {
            return;
        }
        // Break before each *call*, not before each `.`. A field access is part
        // of whatever it qualifies: a leading `msg.params` belongs to the
        // receiver, and a `.rows.count()` reads as one step, so a line never
        // opens with a bare `.field`.
        let first_call = links
            .iter()
            .position(|l| matches!(l, ChainLink::Method { .. }))
            .expect("a chain with two calls has one");
        self.format_expr_at(base, 8, 0);
        for link in &links[..first_call] {
            self.format_chain_link(link, 0);
        }
        self.indented(|f| {
            let mut i = first_call;
            while i < links.len() {
                let mut end = i;
                while end < links.len() && matches!(links[end], ChainLink::Field(_)) {
                    end += 1;
                }
                // …and the call those field accesses qualify, if any.
                if end < links.len() {
                    end += 1;
                }
                f.newline();
                for (offset, link) in links[i..end].iter().enumerate() {
                    let is_last = end == links.len() && i + offset + 1 == links.len();
                    f.format_chain_link(link, if is_last { reserve } else { 0 });
                }
                i = end;
            }
        });
    }

    /// The chain on one line: the receiver, then every `.`-step in place. Any
    /// step whose arguments do not fit wraps them, but no break is introduced
    /// at a `.`. Reports whether any step had to put its arguments one per
    /// line, which is what tells [`Self::format_chain`] this layout is a poor
    /// fit for the chain.
    fn format_chain_inline(
        &mut self,
        base: &Expr,
        links: &[ChainLink<'_>],
        reserve: usize,
    ) -> bool {
        self.format_expr_at(base, 8, 0);
        let mut exploded = false;
        for (i, link) in links.iter().enumerate() {
            exploded |=
                self.format_chain_link(link, if i + 1 == links.len() { reserve } else { 0 });
        }
        exploded
    }

    /// One `.field` or `.method(args)` step of a chain, wrapping the argument
    /// list when the step does not fit on the current line. Reports whether
    /// that wrapping was the one-argument-per-line form.
    fn format_chain_link(&mut self, link: &ChainLink<'_>, reserve: usize) -> bool {
        match link {
            ChainLink::Field(name) => {
                self.push(&format!(".{name}"));
                false
            }
            ChainLink::Method {
                method,
                type_args,
                args,
            } => {
                let head = format!(".{}{}", method, type_args_src(type_args));
                let flat = format!(
                    "{head}({})",
                    args.iter()
                        .map(|a| expr_with_prec(a, 0))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                if args.is_empty() || self.fits(&flat, reserve) {
                    self.push(&flat);
                    return false;
                }
                self.push(&head);
                self.push("(");
                self.format_arg_list(args, reserve)
            }
        }
    }

    /// Emit a `match` expression at the current indent level. Arms sit one
    /// level deeper than the `match`/`}`; block-bodied arms recurse through
    /// `format_block` so their statements indent correctly in turn.
    fn format_match(&mut self, discriminant: &Expr, arms: &[MatchArm]) {
        self.push("match ");
        self.format_expr_at(discriminant, 0, " {".len());
        self.push(" {");
        self.newline();
        self.indented(|f| f.format_arms(arms));
        self.push("}");
    }

    /// The arms of a `match` (or of a `let … else match`), one per line.
    fn format_arms(&mut self, arms: &[MatchArm]) {
        for arm in arms {
            self.push(&pattern_to_string(&arm.pattern));
            // ADR 0169: render an optional `if <guard>` before `=>`.
            if let Some(guard) = &arm.guard {
                self.push(" if ");
                self.format_expr_at(guard, 0, " => ".len());
            }
            self.push(" => ");
            // Every arm ends in a `,`, which counts against its budget.
            match &arm.body {
                MatchBody::Expr(e) => self.format_expr_at(e, 0, 1),
                MatchBody::Block(b) => self.format_block_with_reserve(b, 1),
            }
            self.push(",");
            self.newline();
        }
    }
}

/// One step of a `.`-chain, as collected by [`flatten_chain`].
enum ChainLink<'e> {
    Field(&'e str),
    Method {
        method: &'e str,
        type_args: &'e [TypeRef],
        args: &'e [Expr],
    },
}

/// Split `receiver.a(…).b.c(…)` into its innermost receiver and the `.`-steps
/// applied to it, outermost last. A non-chain expression yields itself and an
/// empty list.
fn flatten_chain(e: &Expr) -> (&Expr, Vec<ChainLink<'_>>) {
    let mut links = Vec::new();
    let mut cur = e;
    loop {
        match &cur.kind {
            ExprKind::FieldAccess { receiver, field } => {
                links.push(ChainLink::Field(field.name.as_str()));
                cur = receiver;
            }
            ExprKind::MethodCall {
                receiver,
                method,
                type_args,
                args,
            } => {
                links.push(ChainLink::Method {
                    method: method.name.as_str(),
                    type_args,
                    args,
                });
                cur = receiver;
            }
            _ => break,
        }
    }
    links.reverse();
    (cur, links)
}

/// The `[T, U]` type-argument suffix on a call, or the empty string.
fn type_args_src(type_args: &[TypeRef]) -> String {
    if type_args.is_empty() {
        return String::new();
    }
    format!(
        "[{}]",
        type_args
            .iter()
            .map(type_ref_to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The operators a wrapped expression may break *before*. A continuation line
/// opening with `&&`, `||`, or `implies` re-attaches to the line above on
/// re-parse; one opening with `+` or `==` does not.
fn is_logical(op: BinOp) -> bool {
    matches!(op, BinOp::And | BinOp::Or | BinOp::Implies)
}

/// Collect the operands of a left-nested run of the same operator, so
/// `a && b && c` breaks into three lines rather than nesting two levels deep.
fn flatten_binop<'e>(e: &'e Expr, op: BinOp, out: &mut Vec<&'e Expr>) {
    if let ExprKind::BinOp(inner_op, lhs, rhs) = &e.kind
        && *inner_op == op
    {
        flatten_binop(lhs, op, out);
        out.push(rhs);
        return;
    }
    out.push(e);
}

/// An expression whose wrapped form opens with a short header and a brace, so
/// it reads correctly as the sole argument of a call it shares a line with —
/// `xs.forEach((x) => {`, `Fetch.send(Request {`. Excludes anything whose first
/// wrapped line is as long as the construct itself (a call, a `.`-chain), which
/// would just move the overflow rather than remove it.
fn is_block_like(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Lambda(_)
            | ExprKind::Block(_)
            | ExprKind::RecordConstruction { .. }
            | ExprKind::RecordSpread { .. }
            | ExprKind::Match { .. }
            | ExprKind::If { .. }
    )
}

/// Whether [`expr_with_prec`] would parenthesise `e` in a `parent_prec`
/// context. The wrapped printer emits those parentheses itself, since it
/// bypasses the flat renderer that would otherwise add them.
fn needs_parens(e: &Expr, parent_prec: u8) -> bool {
    match &e.kind {
        ExprKind::BinOp(op, ..) => binop_prec(*op) < parent_prec,
        ExprKind::UnaryOp(..) => parent_prec > 7,
        _ => false,
    }
}

/// Borrow the trivia attached to a statement variant.
/// Render a `given`-clause capability reference back to source: a bare name
/// for a local capability, or `prefix.Name` for a cross-context one (v0.15).
fn cap_ref_src(c: &CapRef) -> String {
    match &c.context {
        Some(prefix) => format!("{}.{}", prefix.joined(), c.name.name),
        None => c.name.name.clone(),
    }
}

/// Render a `by` clause back to source: `by <Actor>` (binder-less), `by <b>: <Actor>`
/// (captured identity), or an ordered sum `by <b>: A | B` (v0.52). Shared by handler
/// and service-header (v0.155) formatting.
fn by_clause_src(by: &ByClause) -> String {
    let actors = by
        .actors
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    match &by.binder {
        Some(b) => format!("by {}: {actors}", b.name),
        None => format!("by {actors}"),
    }
}

/// Render an events subscription pattern (Events track slice 1, spine
/// #936): `{ field: value, .. }`. The trailing `..` is mandatory whenever any
/// field is listed, so it always renders — there is no pattern-less `Some`
/// case to omit it for (a pattern-less subscription is `pattern: None` on
/// `ServiceProtocol::Events`, handled by the caller before this is reached).
fn event_pattern_src(p: &EventPattern) -> String {
    let fields: Vec<String> = p
        .fields
        .iter()
        .map(|f| format!("{}: {}", f.name.name, event_pattern_value_src(&f.value)))
        .collect();
    format!("{{ {}, .. }}", fields.join(", "))
}

fn event_pattern_value_src(v: &EventPatternValue) -> String {
    match v {
        EventPatternValue::Literal { value, .. } => match value {
            LiteralValue::Int(n) => n.to_string(),
            LiteralValue::Str(s) => format!("\"{}\"", escape_string(s)),
            LiteralValue::Bool(b) => b.to_string(),
        },
        EventPatternValue::Variant {
            type_name, variant, ..
        } => match type_name {
            Some(t) => format!("{}.{}", t.name, variant.name),
            None => variant.name.clone(),
        },
    }
}

/// Render a `via schema(N)` dispatch clause (Events track slice 4, spine
/// #936), written after the `from Events(...)` header's closing `)`.
fn schema_dispatch_src(d: &SchemaDispatch) -> String {
    match &d.pattern {
        SchemaVersionPattern::Literal(n) => format!("via schema({n})"),
    }
}

/// v0.182 (#664): render a call-site actor clause — `by User("bob")` or the
/// unit-identity `by Visitor`.
fn call_site_actor_src(p: &CallSiteActor) -> String {
    match (&p.identity, &p.claims) {
        // #2380 (L5): an `Oidc` principal may carry the token's claims too.
        (Some(id), Some(claims)) => format!(
            "by {}({}, {})",
            p.actor.name,
            expr_with_prec(id, 0),
            expr_with_prec(claims, 0)
        ),
        (Some(id), None) => format!("by {}({})", p.actor.name, expr_with_prec(id, 0)),
        (None, _) => format!("by {}", p.actor.name),
    }
}

/// Render a storage kind: `Cell[Int]`, `Map[K, V]`, or a bare head (v0.81).
fn store_kind_to_string(k: &StoreKind) -> String {
    if k.args.is_empty() {
        k.head.name.clone()
    } else {
        format!(
            "{}[{}]",
            k.head.name,
            k.args
                .iter()
                .map(type_ref_to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// Render a storage annotation (v0.85; ADR 0111): `@name`, or `@name(arg, …)`
/// where each argument is an optional `label: ` then the value expression.
/// Render a storage annotation as a single source-syntax token: `@indexed(by:
/// id)`, `@bounded(10000)`, `@ttl(5.minutes)`, or a bare `@retain`. Public so
/// the LSP's agent-state hover (ADR 0161) can render a `store` field's
/// annotations without re-deriving them.
pub fn annotation_to_string(ann: &Annotation) -> String {
    if ann.args.is_empty() {
        return format!("@{}", ann.name.name);
    }
    let args = ann
        .args
        .iter()
        .map(|a| match &a.label {
            Some(l) => format!("{}: {}", l.name, expr_with_prec(&a.value, 0)),
            None => expr_with_prec(&a.value, 0),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("@{}({})", ann.name.name, args)
}

/// v0.118: render a `stub` clause head-to-tail as a single source line:
/// `stub <capability>.<method>(<args>) <rhs>` (testing track slice 6).
fn stub_clause_to_string(pv: &StubClause) -> String {
    let args = pv
        .args
        .iter()
        .map(|a| match a {
            ArgPattern::Any(_) => "_".to_string(),
            ArgPattern::Value(e) => expr_to_string(e),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let rhs = match &pv.rhs {
        StubRhs::Returns(e) => format!("returns {}", expr_to_string(e)),
        StubRhs::Fails(_) => "fails".to_string(),
        StubRhs::ReturnsEach(outcomes, _) => {
            let items = outcomes
                .iter()
                .map(|o| match o {
                    SeqOutcome::Value(e) => expr_to_string(e),
                    SeqOutcome::Fails(_) => "fails".to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("returns each [{items}]")
        }
    };
    format!(
        "stub {}.{}({}) {}",
        pv.capability.name, pv.method.name, args, rhs
    )
}

fn statement_trivia(s: &Statement) -> &Trivia {
    match s {
        Statement::Let(l) | Statement::EffectLet(l) => &l.trivia,
        Statement::Expect(a) => &a.trivia,
        Statement::Send(s) => &s.trivia,
        Statement::Do(d) => &d.trivia,
        Statement::Assign(a) => &a.trivia,
        Statement::Advance(a) => &a.trivia,
        Statement::Par(p) => &p.trivia,
    }
}

// -- String-rendering helpers (used by inline single-line emission) --

/// The rendered width of a single line: one column per character, except a
/// tab, which advances to the next multiple of `tab`. Counting `char`s rather
/// than bytes keeps a non-ASCII identifier or string literal from being
/// over-measured and wrapped for no reason.
fn display_width(line: &str, tab: usize) -> usize {
    let mut col = 0usize;
    for ch in line.chars() {
        if ch == '\t' {
            col += tab - (col % tab);
        } else {
            col += 1;
        }
    }
    col
}

fn type_ref_to_string(t: &TypeRef) -> String {
    match t {
        TypeRef::Base(b, _) => b.name().to_string(),
        TypeRef::Named(id) => id.name.clone(),
        TypeRef::Result(a, b, _) => format!(
            "Result[{}, {}]",
            type_ref_to_string(a),
            type_ref_to_string(b)
        ),
        TypeRef::Option(t, _) => format!("Option[{}]", type_ref_to_string(t)),
        TypeRef::Effect(t, _) => format!("Effect[{}]", type_ref_to_string(t)),
        TypeRef::HttpResult(t, _) => format!("HttpResult[{}]", type_ref_to_string(t)),
        TypeRef::QueueResult(_) => "QueueResult".to_string(),
        TypeRef::Asset(_) => "Asset".to_string(),
        TypeRef::RequestHeaders(_) => "RequestHeaders".to_string(),
        TypeRef::TokenClaims(_) => "TokenClaims".to_string(),
        TypeRef::BoundaryRejection(_) => "BoundaryRejection".to_string(),
        TypeRef::List(t, _) => format!("List[{}]", type_ref_to_string(t)),
        TypeRef::Set(t, _) => format!("Set[{}]", type_ref_to_string(t)),
        TypeRef::Query(t, _) => format!("Query[{}]", type_ref_to_string(t)),
        TypeRef::Stream(t, _) => format!("Stream[{}]", type_ref_to_string(t)),
        TypeRef::Connection(t, _) => format!("Connection[{}]", type_ref_to_string(t)),
        TypeRef::History(t, _) => format!("History[{}]", type_ref_to_string(t)),
        TypeRef::Map(k, v, _) => {
            format!("Map[{}, {}]", type_ref_to_string(k), type_ref_to_string(v))
        }
        TypeRef::ValidationError(_) => "ValidationError".to_string(),
        TypeRef::JsonError(_) => "JsonError".to_string(),
        TypeRef::Unit(_) => "()".to_string(),
        // v0.157 (ADR 0183): a user generic-type application, as written.
        TypeRef::App { name, args, .. } => format!(
            "{}[{}]",
            name.name,
            args.iter()
                .map(type_ref_to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeRef::Fn(params, ret, _) => {
            let lhs = match params.len() {
                0 => "()".to_string(),
                1 if !matches!(params[0], TypeRef::Fn(..)) => type_ref_to_string(&params[0]),
                _ => format!(
                    "({})",
                    params
                        .iter()
                        .map(type_ref_to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            format!("{lhs} -> {}", type_ref_to_string(ret))
        }
    }
}

pub fn refinement_to_string(r: &Refinement) -> String {
    let mut s = String::new();
    for (i, p) in r.predicates.iter().enumerate() {
        if i > 0 {
            s.push_str(" && ");
        }
        s.push_str(&pred_to_string(p));
    }
    s
}

fn pred_to_string(p: &RefinementPred) -> String {
    match &p.kind {
        PredKind::Matches(re) => format!("Matches(\"{}\")", escape_string(re)),
        PredKind::InRange(a, b) => format!("InRange({}, {})", a.value, b.value),
        PredKind::InRangeF(a, b) => format!("InRange({}, {})", a.lexeme, b.lexeme),
        PredKind::MinLength(n) => format!("MinLength({n})"),
        PredKind::MaxLength(n) => format!("MaxLength({n})"),
        PredKind::Length(n) => format!("Length({n})"),
        PredKind::NonNegative => "NonNegative".to_string(),
        PredKind::Positive => "Positive".to_string(),
        PredKind::NonEmpty => "NonEmpty".to_string(),
    }
}

pub fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

pub fn expr_to_string(e: &Expr) -> String {
    expr_with_prec(e, 0)
}

// Operator precedences (smaller = binds looser):
//   1: || 2: && 3: == != 4: < <= > >= 5: + - 6: * / 7: unary ! - 8: postfix . () ?
fn binop_prec(op: BinOp) -> u8 {
    match op {
        // v0.80: `implies` is the lowest-precedence binary operator (below `||`).
        BinOp::Implies => 0,
        BinOp::Or => 1,
        BinOp::And => 2,
        BinOp::Eq | BinOp::NotEq => 3,
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => 4,
        BinOp::Add | BinOp::Sub => 5,
        BinOp::Mul | BinOp::Div => 6,
    }
}

fn expr_with_prec(e: &Expr, parent_prec: u8) -> String {
    match &e.kind {
        // v0.142 (ADR 0166): the stored lexeme verbatim — formatting must not
        // normalise away the author's `_` digit separators.
        ExprKind::IntLit { lexeme, .. } => lexeme.clone(),
        // v0.21: the stored lexeme verbatim — formatting must not normalise.
        ExprKind::FloatLit { lexeme, .. } => lexeme.clone(),
        // v0.86 (ADR 0112): a duration literal `<value>.<unit>`.
        ExprKind::DurationLit { value, unit, .. } => format!("{value}.{}", unit.name()),
        ExprKind::StrLit(s) => format!("\"{}\"", escape_string(s)),
        // v0.43: re-emit the interpolated string — chunks re-escaped, each
        // hole as `\(expr)`. Re-escaping a chunk's literal `\` to `\\` keeps a
        // source `\\(` (an escaped `\(`) round-tripping as text, not a hole.
        ExprKind::InterpStr(parts) => {
            let mut out = String::from("\"");
            for part in parts {
                match part {
                    InterpPart::Chunk(text) => out.push_str(&escape_string(text)),
                    InterpPart::Hole(hole) => {
                        out.push_str(&format!("\\({})", expr_with_prec(hole, 0)));
                    }
                }
            }
            out.push('"');
            out
        }
        ExprKind::BoolLit(b) => b.to_string(),
        ExprKind::UnitLit => "()".to_string(),
        ExprKind::Ident(id) => id.name.clone(),
        ExprKind::ListLit(elems) => format!(
            "[{}]",
            elems
                .iter()
                .map(expr_to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ExprKind::Call {
            name,
            type_args,
            args,
        } => {
            let targs = if type_args.is_empty() {
                String::new()
            } else {
                format!(
                    "[{}]",
                    type_args
                        .iter()
                        .map(type_ref_to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let parts: Vec<String> = args.iter().map(|a| expr_with_prec(a, 0)).collect();
            format!("{}{}({})", name.name, targs, parts.join(", "))
        }
        ExprKind::BinOp(op, l, r) => {
            let prec = binop_prec(*op);
            let inner = format!(
                "{} {} {}",
                expr_with_prec(l, prec),
                op.name(),
                expr_with_prec(r, prec + 1)
            );
            if prec < parent_prec {
                format!("({inner})")
            } else {
                inner
            }
        }
        ExprKind::UnaryOp(op, inner) => {
            // Unary binds tightly (prec 7).
            let s = format!("{}{}", op.name(), expr_with_prec(inner, 7));
            if parent_prec > 7 { format!("({s})") } else { s }
        }
        ExprKind::Paren(inner) => format!("({})", expr_with_prec(inner, 0)),
        // v0.20a: a lambda prints as `(params) => body`.
        ExprKind::Lambda(lambda) => {
            let params: Vec<String> = lambda
                .params
                .iter()
                .map(|p| match &p.type_ref {
                    Some(tr) => format!("{}: {}", p.name.name, type_ref_to_string(tr)),
                    None => p.name.name.clone(),
                })
                .collect();
            let body = match &lambda.body.kind {
                ExprKind::Block(b) => format_block_oneline(b),
                _ => expr_with_prec(&lambda.body, 0),
            };
            format!("({}) => {}", params.join(", "), body)
        }
        ExprKind::Block(b) => format_block_oneline(b),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            // v0.146 (ADR 0170): an `if` with no `else` carries a synthesised
            // unit else-branch — omit it so the else-less form round-trips.
            if else_block.is_synth_unit() {
                format!(
                    "if {} {}",
                    expr_with_prec(cond, 0),
                    format_block_oneline(then_block),
                )
            } else {
                format!(
                    "if {} {} else {}",
                    expr_with_prec(cond, 0),
                    format_block_oneline(then_block),
                    format_block_oneline(else_block),
                )
            }
        }
        ExprKind::Ok(v) => format!("Ok({})", expr_with_prec(v, 0)),
        ExprKind::Err(v) => format!("Err({})", expr_with_prec(v, 0)),
        ExprKind::Some(v) => format!("Some({})", expr_with_prec(v, 0)),
        ExprKind::None => "None".to_string(),
        ExprKind::Question(v) => format!("{}?", expr_with_prec(v, 8)),
        ExprKind::RecordConstruction { type_name, fields } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| match &f.value {
                    Some(v) => format!("{}: {}", f.name.name, expr_with_prec(v, 0)),
                    None => f.name.name.clone(),
                })
                .collect();
            if parts.is_empty() {
                format!("{} {{}}", type_name.name)
            } else {
                format!("{} {{ {} }}", type_name.name, parts.join(", "))
            }
        }
        ExprKind::FieldAccess { receiver, field } => {
            format!("{}.{}", expr_with_prec(receiver, 8), field.name)
        }
        ExprKind::MethodCall {
            receiver,
            method,
            type_args,
            args,
        } => {
            let targs = if type_args.is_empty() {
                String::new()
            } else {
                format!(
                    "[{}]",
                    type_args
                        .iter()
                        .map(type_ref_to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let parts: Vec<String> = args.iter().map(|a| expr_with_prec(a, 0)).collect();
            format!(
                "{}.{}{targs}({})",
                expr_with_prec(receiver, 8),
                method.name,
                parts.join(", ")
            )
        }
        ExprKind::Match { discriminant, arms } => {
            let mut out = String::new();
            out.push_str("match ");
            out.push_str(&expr_with_prec(discriminant, 0));
            out.push_str(" {\n");
            for arm in arms {
                out.push('\t');
                out.push_str(&pattern_to_string(&arm.pattern));
                if let Some(guard) = &arm.guard {
                    out.push_str(" if ");
                    out.push_str(&expr_with_prec(guard, 0));
                }
                out.push_str(" => ");
                match &arm.body {
                    MatchBody::Expr(e) => out.push_str(&expr_with_prec(e, 0)),
                    MatchBody::Block(b) => out.push_str(&format_block_oneline(b)),
                }
                out.push_str(",\n");
            }
            out.push('}');
            out
        }
        ExprKind::Is { value, pattern } => {
            format!(
                "{} is {}",
                expr_with_prec(value, 4),
                is_pattern_src(pattern)
            )
        }
        ExprKind::RecordSpread {
            type_name,
            base,
            overrides,
        } => {
            let mut parts = vec![format!("...{}", expr_with_prec(base, 0))];
            for f in overrides {
                if let Some(v) = &f.value {
                    parts.push(format!("{}: {}", f.name.name, expr_with_prec(v, 0)));
                } else {
                    parts.push(f.name.name.clone());
                }
            }
            let body = parts.join(", ");
            match type_name {
                Some(tn) => format!("{} {{ {} }}", tn.name, body),
                None => format!("{{ {} }}", body),
            }
        }
        ExprKind::EffectPure(v) => format!("Effect.pure({})", expr_with_prec(v, 0)),
        ExprKind::Expect(v) => format!("expect {}", expr_with_prec(v, 0)),
        ExprKind::Val { type_ref, args } => {
            let t = type_ref_to_string(type_ref);
            if args.is_empty() {
                format!("Val[{t}]")
            } else {
                let a = args
                    .iter()
                    .map(|x| expr_with_prec(x, 0))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("Val[{t}]({a})")
            }
        }
        ExprKind::Wire(inner) => format!("Wire({})", expr_with_prec(inner, 0)),
        ExprKind::Trace { cap, op } => format!("trace({}.{})", cap.name, op.name),
        // #1706: the fault claim — the call, then the contextual `faults`.
        // #1812: the call-site principal sits between the call and `faults`,
        // in the same position as on an effect-let.
        ExprKind::Faults(f) => match &f.principal {
            Some(p) => format!(
                "{} {} faults",
                expr_with_prec(&f.call, 0),
                call_site_actor_src(p)
            ),
            None => format!("{} faults", expr_with_prec(&f.call, 0)),
        },
        ExprKind::Observation(o) => {
            let subject = format!("{}.{}", o.cap.name, o.op.name);
            match &o.matcher {
                ObservationMatcher::NeverCalled => format!("{subject} never called"),
                ObservationMatcher::Before { cap, op } => {
                    format!("{subject} before {}.{}", cap.name, op.name)
                }
                ObservationMatcher::Called { count, with_pred } => {
                    let mut s = format!("{subject} called");
                    if let Some(c) = count {
                        if matches!(c.kind, ExprKind::IntLit { value: 1, .. }) {
                            s.push_str(" once");
                        } else {
                            s.push_str(&format!(" {} times", expr_with_prec(c, 0)));
                        }
                    }
                    if let Some(p) = with_pred {
                        s.push_str(&format!(" with {}", expr_with_prec(p, 0)));
                    }
                    s
                }
            }
        }
    }
}

fn pattern_to_string(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard(_) => "_".to_string(),
        // ADR 0169: a bare name binding renders as its identifier.
        Pattern::Binding(id) => id.name.clone(),
        // v0.130: literal patterns render as their source literal.
        Pattern::Literal { value, .. } => match value {
            LiteralValue::Int(n) => n.to_string(),
            LiteralValue::Str(s) => format!("\"{}\"", escape_string(s)),
            LiteralValue::Bool(b) => b.to_string(),
        },
        // #472: `p where predicate` — the inner pattern, then the predicate
        // list rendered the same way a `type X = Base where P` refinement is.
        Pattern::Refined {
            inner, predicate, ..
        } => format!(
            "{} where {}",
            pattern_to_string(inner),
            refinement_to_string(predicate)
        ),
        Pattern::Variant {
            type_name,
            variant,
            bindings,
            ..
        } => {
            let name_part = match type_name {
                Some(t) => format!("{}.{}", t.name, variant.name),
                None => variant.name.clone(),
            };
            if bindings.is_empty() {
                name_part
            } else {
                // ADR 0169: each payload binding is a full sub-pattern.
                let parts: Vec<String> = bindings
                    .iter()
                    .map(|b| match &b.kind {
                        PatternBindingKind::Positional { pattern } => pattern_to_string(pattern),
                        PatternBindingKind::Named { field, pattern } => {
                            format!("{}: {}", field.name, pattern_to_string(pattern))
                        }
                    })
                    .collect();
                format!("{}({})", name_part, parts.join(", "))
            }
        }
        // #2641: a record pattern renders like a record literal; the trailing
        // `..` is part of the syntax, not an optional marker.
        Pattern::Record {
            type_name, fields, ..
        } => {
            let mut parts: Vec<String> = fields
                .iter()
                .map(|(name, sub)| match sub {
                    Some(p) => format!("{}: {}", name.name, pattern_to_string(p)),
                    None => name.name.clone(),
                })
                .collect();
            parts.push("..".to_string());
            format!("{} {{ {} }}", type_name.name, parts.join(", "))
        }
        // #2680: a list pattern renders like a list literal, with the rest
        // marker last.
        Pattern::List { elems, rest, .. } => {
            let mut parts: Vec<String> = elems.iter().map(pattern_to_string).collect();
            if let Some(r) = rest {
                parts.push(match &r.binder {
                    Some(b) => format!("..{}", b.name),
                    None => "..".to_string(),
                });
            }
            format!("[{}]", parts.join(", "))
        }
        // #474: an or-pattern renders as its alternatives joined by `|`.
        Pattern::Or(alts, _) => alts
            .iter()
            .map(pattern_to_string)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

/// #2641: a pattern after `is`. A record pattern there must be parenthesised —
/// its `{` would otherwise read as the block that follows the condition — so a
/// record, or an or-pattern with a record alternative, keeps the parentheses
/// the parser dropped as transparent grouping.
fn is_pattern_src(p: &Pattern) -> String {
    let src = pattern_to_string(p);
    let has_record = match p {
        Pattern::Record { .. } => true,
        Pattern::Or(alts, _) => alts.iter().any(|a| matches!(a, Pattern::Record { .. })),
        _ => false,
    };
    if has_record { format!("({src})") } else { src }
}

/// #981: whether a block's `()` tail should be omitted rather than printed.
///
/// A `()` tail — whether the parser synthesised it ([`Block::implicit_tail`])
/// or the user wrote it out explicitly — is exactly the block's default
/// value, so dropping it is loss-free: the parser re-derives the same
/// implicit unit tail either way (v0.7 / v0.146, ADR 0170).
///
/// Omitting it is not just an idempotency nicety, it is required for
/// correctness whenever anything precedes the tail (a statement, a `case`'s
/// `stub` clause): Bynk has no statement terminator, so a printed `()`
/// immediately after a preceding line's last token re-attaches to it as a
/// zero-arg call on re-parse (`x` / `()` → `x()`) rather than staying two
/// separate constructs. #735 only special-cased the *implicit*-tail shape;
/// #981 found the identical corruption for an *explicit* `()` tail (e.g. the
/// last statement of a `match` arm's block), which is exactly as dangerous
/// once anything comes before it. So this covers both, structurally, rather
/// than special-casing another syntactic position.
fn omit_unit_tail(b: &Block) -> bool {
    matches!(b.tail.kind, ExprKind::UnitLit) && b.tail_leading_comments.is_empty()
}

fn format_block_oneline(b: &Block) -> String {
    if b.statements.is_empty() {
        // v0.146 (ADR 0170): an empty block with a synthesised `()` tail prints
        // as `{}` — printing `{ () }` would not round-trip against the parser's
        // implicit-tail synthesis.
        if b.implicit_tail {
            "{}".to_string()
        } else {
            format!("{{ {} }}", expr_with_prec(&b.tail, 0))
        }
    } else {
        // Multi-line block — render with newlines and tab indentation.
        let mut out = String::from("{\n");
        block_body_lines(b, &mut out);
        out.push('}');
        out
    }
}

/// The string form of [`Formatter::format_block_body`]: each statement and
/// the tail on its own tab-indented line, a `let … else` printed back as
/// written (#2010).
fn block_body_lines(b: &Block, out: &mut String) {
    for stmt in &b.statements {
        if let Some((l, effectful, le)) = let_else_binding(stmt)
            && let ExprKind::Match { arms, .. } = &b.tail.kind
            && let Some(MatchBody::Block(rest)) = arms.first().map(|a| &a.body)
        {
            out.push('\t');
            out.push_str(&let_else_to_string(l, effectful, le, arms));
            out.push('\n');
            block_body_lines(rest, out);
            return;
        }
        out.push('\t');
        out.push_str(&stmt_to_string(stmt));
        out.push('\n');
    }
    if !omit_unit_tail(b) {
        out.push('\t');
        out.push_str(&expr_with_prec(&b.tail, 0));
        out.push('\n');
    }
}

/// #2010: the hidden binding a `let … else` desugars into, if `s` is one —
/// with whether it was `<-` and the sugar's own marker.
fn let_else_binding(s: &Statement) -> Option<(&LetStmt, bool, &LetElse)> {
    match s {
        Statement::Let(l) => l.let_else.as_ref().map(|le| (l, false, le)),
        Statement::EffectLet(l) => l.let_else.as_ref().map(|le| (l, true, le)),
        _ => None,
    }
}

/// #2010: a `let … else` pattern as source. After `let`, a bare name is a
/// binding, not a pattern (`let Missing | Late = p` would re-parse as a
/// binding named `Missing`), so a pattern whose text opens with a name not
/// followed by `(`, `.` or `{` keeps the parentheses its author had to write.
/// `None`, `true` and `false` are keywords, not names, so they need none.
fn let_pattern_src(p: &Pattern) -> String {
    let src = pattern_to_string(p);
    let name_len = src
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(src.len());
    let opens_with_name = src.starts_with(|c: char| c.is_ascii_alphabetic())
        && !matches!(&src[..name_len], "None" | "true" | "false");
    let then = src[name_len..].chars().next();
    // #2654: `Name {` opens a record pattern, so it needs no parentheses.
    let opens_record = src[name_len..].trim_start().starts_with('{');
    if opens_with_name && !opens_record && !matches!(then, Some('(' | '.')) {
        format!("({src})")
    } else {
        src
    }
}

/// #2010: the single-line string form of a `let … else` (see
/// [`Formatter::format_let_else`]).
fn let_else_to_string(l: &LetStmt, effectful: bool, le: &LetElse, arms: &[MatchArm]) -> String {
    let mut out = format!(
        "let {} {} {}",
        let_pattern_src(&arms[0].pattern),
        if effectful { "<-" } else { "=" },
        expr_with_prec(&l.value, 0)
    );
    if let Some(p) = &l.principal {
        out.push_str(&format!(" {}", call_site_actor_src(p)));
    }
    match (le.form, arms.get(1).map(|a| &a.body)) {
        (LetElseForm::Plain, _) => {}
        (LetElseForm::Block, Some(MatchBody::Block(otherwise))) => {
            out.push_str(&format!(" else {}", format_block_oneline(otherwise)));
        }
        _ => {
            let rendered: Vec<String> = arms[1..]
                .iter()
                .map(|a| {
                    let body = match &a.body {
                        MatchBody::Expr(e) => expr_with_prec(e, 0),
                        MatchBody::Block(b) => format_block_oneline(b),
                    };
                    let guard = a
                        .guard
                        .as_ref()
                        .map(|g| format!(" if {}", expr_with_prec(g, 0)))
                        .unwrap_or_default();
                    format!("{}{guard} => {body}", pattern_to_string(&a.pattern))
                })
                .collect();
            out.push_str(&format!(" else match {{ {} }}", rendered.join(", ")));
        }
    }
    out
}

fn stmt_to_string(s: &Statement) -> String {
    match s {
        Statement::Let(l) => {
            let mut out = format!("let {}", l.name.name);
            if let Some(t) = &l.type_annot {
                out.push_str(&format!(": {}", type_ref_to_string(t)));
            }
            out.push_str(&format!(" = {}", expr_with_prec(&l.value, 0)));
            out
        }
        Statement::EffectLet(l) => {
            let mut out = format!("let {}", l.name.name);
            if let Some(t) = &l.type_annot {
                out.push_str(&format!(": {}", type_ref_to_string(t)));
            }
            out.push_str(&format!(" <- {}", expr_with_prec(&l.value, 0)));
            if let Some(p) = &l.principal {
                out.push_str(&format!(" {}", call_site_actor_src(p)));
            }
            out
        }
        Statement::Expect(a) => format!("expect {}", expr_with_prec(&a.value, 0)),
        Statement::Send(s) => format!("~> {}", expr_with_prec(&s.value, 0)),
        Statement::Do(d) => format!("do {}", expr_with_prec(&d.value, 0)),
        Statement::Assign(a) => format!("{} := {}", a.target.name, expr_with_prec(&a.value, 0)),
        Statement::Advance(a) => {
            format!("advance {} by {}", a.cap.name, expr_with_prec(&a.amount, 0))
        }
        Statement::Par(p) => {
            let body: Vec<String> = p.statements.iter().map(stmt_to_string).collect();
            format!("par {{ {} }}", body.join(" "))
        }
    }
}

/// `T` or `T: Eq + C`: a type parameter as written, bounds included.
fn type_param_text(tp: &TypeParam) -> String {
    if tp.bounds.is_empty() {
        tp.name.name.clone()
    } else {
        format!("{}: {}", tp.name.name, bounds_text(&tp.bounds))
    }
}

fn bounds_text(bounds: &[Ident]) -> String {
    bounds
        .iter()
        .map(|b| b.name.as_str())
        .collect::<Vec<_>>()
        .join(" + ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Constrained generics slice 0: bounds and a method's `where` clause print as
    /// written, and the output is a fixed point.
    #[test]
    fn bounds_and_where_clauses_round_trip() {
        let src = "commons m\n\ntype Pair[T: Eq] = {\n\tleft: T,\n}\n\nfn same[T: Eq + Ord, U](a: T, b: T, u: U) -> Bool {\n\ta == b\n}\n\nfn Pair.has(self, x: T) -> Bool where T: Eq, T: Ord {\n\tself.left == x\n}\n\nfn Pair.has2(self, x: T) -> Bool where T: Eq\n\trequires p: true\n{\n\tself.left == x\n}\n";
        let out = format_source(src, &FormatOptions::default()).unwrap();
        assert_eq!(out, src);
    }

    /// #2640: a `par` block prints one binding per line, indented, and the
    /// output is a fixed point; comments around and inside it stay put.
    #[test]
    fn par_block_round_trips() {
        let src = "commons x\n\nfn f(id: Int) -> Effect[Int] {\n\t// load both\n\tpar {\n\t\t// the user\n\t\tlet user <- Users.get(id)  // trailing\n\t\tlet orders: Int <- Orders.forUser(id)\n\t\tdo Audit.touch(id)\n\t\t// last word\n\t}\n\tEffect.pure(user + orders)\n}\n";
        let out = format_source(src, &FormatOptions::default()).unwrap();
        assert_eq!(out, src);
        assert_eq!(format_source(&out, &FormatOptions::default()).unwrap(), out);
    }

    #[test]
    fn par_block_is_reindented_and_split_onto_lines() {
        let messy = "commons x\n\nfn f(id: Int) -> Effect[Int] {\n  if id > 0 {   par { let a <- A.get(id)   let b <- B.get(id) }\n  Effect.pure(a + b) } else { Effect.pure(0) }\n}\n";
        let out = format_source(messy, &FormatOptions::default()).unwrap();
        assert!(
            out.contains("\t\tpar {\n\t\t\tlet a <- A.get(id)\n\t\t\tlet b <- B.get(id)\n\t\t}\n"),
            "{out}"
        );
        assert_eq!(format_source(&out, &FormatOptions::default()).unwrap(), out);
    }

    /// #2321: a binding's `includes` list prints between the module and
    /// `requires`, and the output is a fixed point.
    #[test]
    fn binding_includes_round_trips() {
        let src = "adapter x {\n\tbinding \"./x.ts\" includes [\"./a.wasm\", \"b.wasm\"] requires { \"p\": \"^1\" }\n}\n";
        let out = format_source(src, &FormatOptions::default()).unwrap();
        assert_eq!(out, src);
    }

    /// #1885: a doc holding a `---` line prints with a `----` fence, so the
    /// output re-lexes as one block and is a fixed point; a doc with none
    /// prints the canonical `---`, whatever fence it was written with.
    #[test]
    fn doc_fence_outgrows_the_marker_lines_it_holds() {
        let held = "commons m\n\n-------\nAbove.\n---\nInner.\n-----\nBelow.\n-------\nfn f() -> Int { 1 }\n";
        let out = format_source(held, &FormatOptions::default()).unwrap();
        assert_eq!(
            out,
            "commons m\n\n------\nAbove.\n---\nInner.\n-----\nBelow.\n------\nfn f() -> Int {\n\t1\n}\n"
        );
        assert_eq!(format_source(&out, &FormatOptions::default()).unwrap(), out);
        let plain = "commons m\n\n-----\nJust prose.\n-----\nfn f() -> Int { 1 }\n";
        assert_eq!(
            format_source(plain, &FormatOptions::default()).unwrap(),
            "commons m\n\n---\nJust prose.\n---\nfn f() -> Int {\n\t1\n}\n"
        );
        assert_eq!(doc_fence_len("a\n  ---  \nb"), 4);
        assert_eq!(doc_fence_len("a\n--\n- - -"), 3);
    }

    /// #1888 (DECISION F): each doc form prints as written, on declarations
    /// and on every member kind: a `--- text` line stays a line, a one-line
    /// fence stays a fence. Canonical input is a fixed point.
    #[test]
    fn each_doc_form_round_trips_unchanged() {
        let src = "commons m\n\n--- A commit SHA.\ntype Commit = String\n\n---\nOne line, fenced.\n---\ntype Id = Int\n\ntype Point = {\n\t--- Across.\n\tx: Int,\n}\n\n--- One line.\ntype Run = {\n\t--- The repository.\n\trepo: String,\n\t---\n\tThe version.\n\t---\n\tversion: String,\n\t// a note\n\tsha: String,  // trailing\n}\n\ntype Outcome = enum {\n\t--- Passed.\n\tPass,\n\t---\n\tFailed.\n\t---\n\tFail,\n}\n\ntype Shape =\n\t--- Round.\n\t| Circle(\n\t\t--- The radius.\n\t\tr: Int,\n\t\td: Int,\n\t)\n\t--- A dot.\n\t| Dot\n\nmessages \"en\" @reference {\n\t--- Greets.\n\t\"hi\" => \"Hello\"\n}\n";
        let out = format_source(src, &FormatOptions::default()).unwrap();
        assert_eq!(out, src);
        // #2203: the marker is `---` and its separating space is one; the
        // optional closer is not printed, and a text ending in a `---` run
        // takes a longer marker so the run is not read as a closer.
        for (written, printed) in [
            ("--- bare", "--- bare"),
            ("-----\tspaced", "--- spaced"),
            ("--- Closed. ---", "--- Closed."),
            ("---- Ends in a rule ---", "---- Ends in a rule ---"),
        ] {
            let src = format!("commons m\n\n{written}\nfn f() -> Int {{ 1 }}\n");
            assert_eq!(
                format_source(&src, &FormatOptions::default()).unwrap(),
                format!("commons m\n\n{printed}\nfn f() -> Int {{\n\t1\n}}\n"),
                "{written:?}"
            );
        }
    }

    /// #2203 (DECISION F): `bynk fmt` is the migration. A file with a legacy
    /// `--` comment or `--|` doc line is read by the old rules and printed
    /// with `//` comments and `--- ` docs, and `a--b` stays subtraction.
    #[test]
    fn legacy_spellings_format_to_the_new_ones() {
        let src = "-- A header.\ncommons m\n\n--| A run.\ntype Run = {\n\t--| The repo.\n\trepo: String,\n\tn: Int,  -- trailing\n}\n\nfn d(a: Int, b: Int) -> Int { a--b }\n";
        let out = format_source(src, &FormatOptions::default()).unwrap();
        assert_eq!(
            out,
            "// A header.\ncommons m\n\n--- A run.\ntype Run = {\n\t--- The repo.\n\trepo: String,\n\tn: Int,  // trailing\n}\n\nfn d(a: Int, b: Int) -> Int {\n\ta - -b\n}\n"
        );
        assert_eq!(format_source(&out, &FormatOptions::default()).unwrap(), out);
        // A `--|` text ending in a `---` run takes a longer marker.
        let ruled = "commons m\n\n--| Ends in a rule ---\nfn f() -> Int { 1 }\n";
        assert_eq!(
            format_source(ruled, &FormatOptions::default()).unwrap(),
            "commons m\n\n---- Ends in a rule ---\nfn f() -> Int {\n\t1\n}\n"
        );
    }

    /// #2203 review: a half-migrated file, a stray legacy `--` comment among
    /// lines that the old and new rules read differently (a `--- text` doc,
    /// an `a --b` subtraction), is refused, never silently reread; one whose
    /// other lines read the same either way migrates.
    #[test]
    fn a_half_migrated_file_is_refused_where_the_reads_differ() {
        let opts = FormatOptions::default();
        for (src, line) in [
            (
                "commons m\n\n-- note\n--- Adds two ints.\nfn add(a: Int, b: Int) -> Int { a + b }\n",
                "--- Adds two ints.",
            ),
            (
                "commons m\n\n-- note\nfn f(a: Int, b: Int) -> Int { a --b }\n",
                "fn f(a: Int, b: Int) -> Int { a --b }",
            ),
        ] {
            let err = format_source(src, &opts).unwrap_err();
            assert_eq!(
                err.errors[0].category, "bynk.fmt.mixed_comment_syntax",
                "{src}"
            );
            assert_eq!(&src[err.errors[0].span.range()], line, "{src}");
        }
        // An old `--- Helpers ---` divider was a comment but now reads as a
        // doc, so it is refused too, not guessed at.
        let divider = "-- A header.\ncommons m\n\n--- Helpers ---\n\nfn f() -> Int { 1 }\n";
        let err = format_source(divider, &opts).unwrap_err();
        assert_eq!(err.errors[0].category, "bynk.fmt.mixed_comment_syntax");
        let agreeing =
            "commons m\n\n-- note\n// new\nfn f(a: Int, b: Int) -> Int { a--b }  --| old\n";
        assert_eq!(
            format_source(agreeing, &opts).unwrap(),
            "commons m\n\n// note\n// new\nfn f(a: Int, b: Int) -> Int {\n\ta - -b\n}  // old\n"
        );
    }

    /// #1888 review: a `//` comment under a one-line doc is on its own line,
    /// as one under a `---` block is, so the blank line after it (#1884) is
    /// kept in both forms.
    #[test]
    fn a_blank_after_a_comment_under_a_doc_line_is_kept() {
        for (doc, printed) in [
            ("--- Doc.\n", "--- Doc.\n"),
            ("---\nDoc.\n---\n", "---\nDoc.\n---\n"),
        ] {
            let src = format!("commons m\n\n{doc}// note\n\ntype X = Int\n");
            assert_eq!(
                format_source(&src, &FormatOptions::default()).unwrap(),
                format!("commons m\n\n// note\n\n{printed}type X = Int\n"),
                "{doc:?}"
            );
        }
    }

    /// #1888: an orphaned one-line doc (a blank line before the declaration)
    /// stays a line too, where it was.
    #[test]
    fn an_orphaned_doc_line_run_keeps_its_form() {
        let src = "commons m\n\n--- Orphaned.\n\nfn f() -> Int { 1 }\n";
        assert_eq!(
            format_source(src, &FormatOptions::default()).unwrap(),
            "commons m\n\n--- Orphaned.\n\nfn f() -> Int {\n\t1\n}\n"
        );
    }

    /// #1888: the doc-loss guard keys a doc by its form as well as its text,
    /// so a doc printed in the other form counts as lost.
    #[test]
    fn the_doc_loss_guard_sees_a_converted_form() {
        let src = "commons m\n\n--- Text.\nfn f() -> Int { 1 }\n";
        let tokens = tokenize(src).unwrap();
        let converted = "commons m\n\n---\nText.\n---\nfn f() -> Int { 1 }\n";
        assert!(doc_block_loss(src, &tokens, converted).is_some());
        assert!(doc_block_loss(src, &tokens, src).is_none());
    }

    /// #1763: a CRLF copy of a canonical file formats to the LF canonical form,
    /// and a doc block's lines lose their CR too (its text was kept verbatim).
    #[test]
    fn crlf_formats_to_the_lf_canonical_form() {
        let lf = "context greeting\n\n---\nA greeting for a name.\n---\nfn greet(name: String) -> String {\n\tname\n}\n";
        assert_eq!(format_source(lf, &FormatOptions::default()).unwrap(), lf);
        let crlf = lf.replace('\n', "\r\n");
        let formatted = format_source(&crlf, &FormatOptions::default()).unwrap();
        assert_eq!(formatted, lf);
        assert!(!formatted.contains('\r'));
    }

    /// A braced declaration always breaks, however short: a record type one
    /// field per line, an `fn` body on its own lines, an empty body as `{}`.
    #[test]
    fn braced_declarations_always_break() {
        let src = "commons m\n\ntype P = { x: Int, y: Int }\n\nfn id(n: Int) -> Int { n }\n\nfn nop() -> () {}\n";
        let out = format_source(src, &FormatOptions::default()).unwrap();
        assert_eq!(
            out,
            "commons m\n\ntype P = {\n\tx: Int,\n\ty: Int,\n}\n\nfn id(n: Int) -> Int {\n\tn\n}\n\nfn nop() -> () {}\n"
        );
        assert_eq!(format_source(&out, &FormatOptions::default()).unwrap(), out);
    }

    /// A record literal with several fields breaks; a single field stays
    /// inline. `match` arms and `if` branches stay compact when they fit.
    #[test]
    fn record_literals_break_but_match_arms_stay_compact() {
        let src = "commons m\n\nfn a(v: Int) -> P { P { x: v, y: v } }\n\nfn b(v: Int) -> P { P { x: v } }\n\nfn c(v: Int) -> Int { match v { 0 => 1, _ => 2 } }\n\nfn d(v: Int) -> Int { if v > 0 { 1 } else { 0 } }\n";
        let out = format_source(src, &FormatOptions::default()).unwrap();
        assert_eq!(
            out,
            "commons m\n\nfn a(v: Int) -> P {\n\tP {\n\t\tx: v,\n\t\ty: v,\n\t}\n}\n\nfn b(v: Int) -> P {\n\tP { x: v }\n}\n\nfn c(v: Int) -> Int {\n\tmatch v {\n\t\t0 => 1,\n\t\t_ => 2,\n\t}\n}\n\nfn d(v: Int) -> Int {\n\tif v > 0 { 1 } else { 0 }\n}\n"
        );
        assert_eq!(format_source(&out, &FormatOptions::default()).unwrap(), out);
    }

    /// A fragment-form `suite` header is followed by a blank line, and
    /// `case` and `property` blocks keep the order they were written in.
    #[test]
    fn suite_header_is_spaced_and_members_keep_source_order() {
        let src = "suite shop\nuses money\nproperty \"p1\" {\n  for all a: Int { expect a == a }\n}\ncase \"c1\" { expect true }\nproperty \"p2\" {\n  for all a: Int { expect a == a }\n}\n";
        let out = format_source(src, &FormatOptions::default()).unwrap();
        assert_eq!(
            out,
            "suite shop\n\nuses money\n\nproperty \"p1\" {\n\tfor all a: Int {\n\t\texpect a == a\n\t}\n}\n\ncase \"c1\" {\n\texpect true\n}\n\nproperty \"p2\" {\n\tfor all a: Int {\n\t\texpect a == a\n\t}\n}\n"
        );
        assert_eq!(format_source(&out, &FormatOptions::default()).unwrap(), out);
    }

    /// A spread with overrides breaks like a multi-field literal; a bare
    /// spread (one item) stays inline.
    #[test]
    fn spread_literals_break_only_with_overrides() {
        let src =
            "commons m\n\nfn a(b: P) -> P { P { ...b, x: 1 } }\n\nfn c(b: P) -> P { P { ...b } }\n";
        let out = format_source(src, &FormatOptions::default()).unwrap();
        assert_eq!(
            out,
            "commons m\n\nfn a(b: P) -> P {\n\tP {\n\t\t...b,\n\t\tx: 1,\n\t}\n}\n\nfn c(b: P) -> P {\n\tP { ...b }\n}\n"
        );
        assert_eq!(format_source(&out, &FormatOptions::default()).unwrap(), out);
    }

    #[test]
    fn normalize_line_endings_borrows_when_there_is_nothing_to_do() {
        assert!(matches!(
            normalize_line_endings("a\nb"),
            std::borrow::Cow::Borrowed(_)
        ));
        assert_eq!(normalize_line_endings("a\r\nb\r\n"), "a\nb\n");
    }

    /// #1775 review: a run of CRs before an LF is one line ending, so the
    /// normaliser is a fixed point; a CR not before an LF is kept.
    #[test]
    fn normalize_line_endings_is_a_fixed_point() {
        for input in ["a\r\r\nb", "a\r\nb\rc\r\n", "\r\r\r\n", "x\r", "a\r\n\rb"] {
            let once = normalize_line_endings(input).into_owned();
            assert_eq!(normalize_line_endings(&once), once, "{input:?}");
            assert!(!once.contains("\r\n"), "{input:?} -> {once:?}");
        }
        assert_eq!(normalize_line_endings("a\r\r\nb"), "a\nb");
        assert_eq!(normalize_line_endings("a\r\nb\rc"), "a\nb\rc");
    }

    /// #1755 review: output that does not even tokenize is the round-trip
    /// guard's to report ("no longer parses"). The doc-block guard runs first,
    /// so it must stay silent rather than blame every doc block as lost.
    #[test]
    fn doc_block_loss_leaves_an_untokenizable_output_to_the_roundtrip_guard() {
        let source = "commons d\n\n---\nattached\n---\nfn f() -> Int { 1 }\n";
        let tokens = tokenize(source).unwrap();
        assert!(tokenize("commons d\n\"unterminated").is_err());
        assert!(
            doc_block_loss(source, &tokens, "commons d\n\"unterminated").is_none(),
            "an untokenizable output was reported as doc-block loss"
        );
    }

    fn fmt(src: &str) -> String {
        format_source(src, &FormatOptions::default()).expect("format failed")
    }

    #[test]
    fn formats_minimal_commons() {
        let src = "commons fitness.units {}";
        let out = fmt(src);
        assert!(out.starts_with("commons fitness.units"));
        // Idempotency.
        let out2 = fmt(&out);
        assert_eq!(out, out2);
    }

    #[test]
    fn formats_refined_type() {
        let src = "commons x { type Metres = Int where NonNegative }";
        let out = fmt(src);
        assert!(out.contains("type Metres = Int where NonNegative"));
        let out2 = fmt(&out);
        assert_eq!(out, out2);
    }

    #[test]
    fn formats_function_decl() {
        let src = "commons x { fn add(a: Int, b: Int) -> Int { a + b } }";
        let out = fmt(src);
        assert!(out.contains("fn add(a: Int, b: Int) -> Int"));
        let out2 = fmt(&out);
        assert_eq!(out, out2);
    }

    #[test]
    fn formats_record() {
        let src = "commons x { type Pt = { x: Int, y: Int } }";
        let out = fmt(src);
        let out2 = fmt(&out);
        assert_eq!(out, out2, "formatter not idempotent: {out}");
    }

    #[test]
    fn formats_a_fault_claim_with_a_call_site_principal() {
        // #1812: the principal sits between the call and `faults`, and the
        // claim round-trips with it — and without it.
        let src = "suite s {\n  case \"c\" {\n    expect api.POST(\"/o\", x)   by  User( \"bob\" ) faults\n    expect api.GET(\"/a\") by Visitor faults\n    expect box.call() faults\n  }\n}\n";
        let out = fmt(src);
        assert!(
            out.contains("expect api.POST(\"/o\", x) by User(\"bob\") faults"),
            "{out}"
        );
        assert!(
            out.contains("expect api.GET(\"/a\") by Visitor faults"),
            "{out}"
        );
        assert!(out.contains("expect box.call() faults"), "{out}");
        assert_eq!(out, fmt(&out), "formatter not idempotent: {out}");
    }

    #[test]
    fn formats_doc_block() {
        let src = "commons x {\n---\nA descriptive doc.\n---\ntype T = Int where Positive\n}";
        let out = fmt(src);
        assert!(out.contains("A descriptive doc."));
        let out2 = fmt(&out);
        assert_eq!(out, out2);
    }

    // -- v1.1 comment preservation --

    #[test]
    fn preserves_leading_line_comment_on_decl() {
        let src = "commons x {\n// explain T\ntype T = Int where NonNegative\n}";
        let out = fmt(src);
        assert!(out.contains("// explain T"), "comment dropped: {out}");
        // Idempotent.
        assert_eq!(out, fmt(&out));
    }

    #[test]
    fn preserves_trailing_line_comment_on_decl() {
        let src = "commons x {\ntype T = Int where NonNegative  // short\n}";
        let out = fmt(src);
        assert!(out.contains("// short"));
        // The trailing comment must remain on the same line as the decl.
        assert!(
            out.lines()
                .any(|l| l.contains("type T") && l.contains("// short")),
            "trailing comment not on same line: {out}"
        );
        assert_eq!(out, fmt(&out));
    }

    #[test]
    fn preserves_grouped_leading_comments() {
        let src = "commons x {\n// one\n// two\ntype T = Int where Positive\n}";
        let out = fmt(src);
        assert!(out.contains("// one"));
        assert!(out.contains("// two"));
        // Adjacent — no blank line between the comments.
        let i1 = out.find("// one").unwrap();
        let i2 = out.find("// two").unwrap();
        let between = &out[i1..i2];
        assert_eq!(
            between.matches('\n').count(),
            1,
            "blank line inserted: {out}"
        );
        assert_eq!(out, fmt(&out));
    }

    #[test]
    fn preserves_comment_before_block_tail() {
        let src = "commons x {\nfn f(n: Int) -> Int {\nlet y = n + 1\n// result\ny\n}\n}";
        let out = fmt(src);
        assert!(out.contains("// result"), "tail comment dropped: {out}");
        assert_eq!(out, fmt(&out));
    }

    #[test]
    fn preserves_comment_with_doc_block_above_decl() {
        let src = "commons x {\n// TODO: rename\n---\nThe canonical T.\n---\ntype T = Int where Positive\n}";
        let out = fmt(src);
        assert!(out.contains("// TODO: rename"));
        assert!(out.contains("The canonical T."));
        // Spec layout: comment, then doc block, then declaration.
        let ic = out.find("// TODO: rename").unwrap();
        let id = out.find("The canonical T.").unwrap();
        let it = out.find("type T").unwrap();
        assert!(ic < id && id < it, "ordering wrong: {out}");
        assert_eq!(out, fmt(&out));
    }

    #[test]
    fn preserves_trailing_file_comment() {
        let src = "commons x.y\n\ntype T = Int where Positive\n// TODO\n";
        let out = fmt(src);
        assert!(out.contains("// TODO"));
        assert_eq!(out, fmt(&out));
    }

    // -- Finding #66: the drain-check fast path must not weaken the
    // comment-loss guard --

    /// A comment strictly inside an expression subtree (here, a binop chain)
    /// is the one shape `TriviaTable` genuinely cannot drain — `fully_drained`
    /// must be `false` for it, so `format_source`'s fast path does not skip
    /// `comment_loss` and the file is still refused rather than silently
    /// losing the comment.
    #[test]
    fn expression_interior_comment_is_not_fully_drained_and_still_refused() {
        let src = "commons x {\n  fn f() -> Int {\n    1 + // note\n    2\n  }\n}\n";
        let tokens = tokenize(src).unwrap();
        let (_, _, fully_drained) = bynk_syntax::parser::parse_units_with_drain_check(&tokens, src)
            .expect("a comment inside an expression is still a valid parse");
        assert!(
            !fully_drained,
            "an expression-interior comment must leave the trivia table undrained"
        );
        let err = format_source(src, &FormatOptions::default())
            .expect_err("formatting must still refuse rather than lose the comment");
        assert_eq!(err.errors[0].category, "bynk.fmt.comment_loss");
    }

    /// The mirror image: an ordinary file with no expression-interior comment
    /// (every comment sits before a declaration/statement, or trailing one) is
    /// `fully_drained`, so `format_source`'s fast path is what actually runs —
    /// and formatting must still succeed and preserve the comment.
    #[test]
    fn ordinary_comment_is_fully_drained_and_formats_normally() {
        let src = "commons x {\n// note\ntype T = Int where Positive\n}\n";
        let tokens = tokenize(src).unwrap();
        let (_, _, fully_drained) =
            bynk_syntax::parser::parse_units_with_drain_check(&tokens, src).expect("should parse");
        assert!(
            fully_drained,
            "a declaration-leading comment must be fully drained"
        );
        let out = fmt(src);
        assert!(out.contains("// note"));
    }

    // -- #981: a bare-identifier statement + trailing `()` must not merge
    // into a call expression.

    #[test]
    fn match_arm_block_tail_unit_after_assign_does_not_reattach_as_a_call() {
        // The exact shape from #981: an Assign statement whose value is a
        // bare (capitalised, enum-variant-shaped) identifier, immediately
        // followed by the block's own explicit `()` tail. The formatter must
        // not print these adjacently — that reparses as `status := Paid()`,
        // a call, rather than the original two constructs.
        let src = "commons x { fn f(status: T) -> T {\n  match status {\n    Draft => {\n      status := Paid\n      ()\n    }\n    Paid => (),\n  }\n} }";
        let out = fmt(src);
        assert!(
            !out.contains("Paid()"),
            "the `()` tail must not re-attach to `Paid` as a call:\n{out}"
        );
        assert!(
            out.contains("status := Paid"),
            "the assignment must survive unmangled:\n{out}"
        );
        assert_eq!(out, fmt(&out), "must be idempotent");
    }

    #[test]
    fn explicit_unit_tail_after_a_statement_is_omitted_like_an_implicit_one() {
        // Not just the implicit-tail shape #735 special-cased — an
        // *explicit* `()` written by the user right after a statement is
        // exactly as dangerous to print, so it is omitted the same way.
        let src = "commons x { fn f() -> Effect[()] {\n  let a = 1\n  ()\n} }";
        let out = fmt(src);
        let expected = "commons x {\n\tfn f() -> Effect[()] {\n\t\tlet a = 1\n\t}\n}\n";
        assert_eq!(
            out, expected,
            "an explicit unit tail after a statement must be omitted"
        );
        assert_eq!(out, fmt(&out), "must be idempotent");
    }

    // -- #735 round-trip guard --

    #[test]
    fn code_only_canonical_ignores_comments() {
        // Two sources whose only difference is comments must reduce to the same
        // comment-free canonical form — this is what lets the round-trip guard
        // compare structure while the formatter re-flows trivia freely.
        let opts = FormatOptions::default();
        let bare = "commons x { type T = Int where Positive }";
        let commented = "commons x {\n// a note\ntype T = Int where Positive  // trailing\n}";
        assert_eq!(
            code_only_canonical(bare, &opts).unwrap(),
            code_only_canonical(commented, &opts).unwrap(),
        );
    }

    #[test]
    fn roundtrip_guard_accepts_faithful_output() {
        // The formatter's own output over a real source must round-trip.
        let opts = FormatOptions::default();
        let src = "commons x { fn add(a: Int, b: Int) -> Int { a + b } }";
        let out = format_source(src, &opts).unwrap();
        let tokens = tokenize(src).unwrap();
        assert!(roundtrip_divergence(&tokens, src, &out, &opts).is_none());
    }

    #[test]
    fn roundtrip_guard_rejects_non_parsing_output() {
        // Simulate a printer that emitted garbage: the output no longer parses,
        // so the guard must fire rather than let it be written. The output here
        // is *longer* than the source and its parse error lands near its end —
        // the error's span must nonetheless stay within the source, because the
        // caller renders it against `source`, not the output (an out-of-range
        // primary span misplaces the caret or panics ariadne). See
        // `roundtrip_error`.
        let opts = FormatOptions::default();
        let src = "commons x { type T = Int where Positive }";
        let corrupt = "commons x { type T = Int where Positive } fn f(a: Int) -> Int { a +";
        assert!(
            corrupt.len() > src.len(),
            "output must be the longer buffer"
        );
        let tokens = tokenize(src).unwrap();
        let err = roundtrip_divergence(&tokens, src, corrupt, &opts)
            .expect("must reject non-parsing output");
        assert_eq!(err.category, "bynk.fmt.roundtrip");
        assert!(
            err.span.end <= src.len(),
            "roundtrip error span {:?} escapes the source it is rendered against (len {})",
            err.span,
            src.len(),
        );
    }

    #[test]
    fn roundtrip_guard_rejects_structural_divergence() {
        // Simulate a printer that emitted parseable-but-wrong code: the output
        // parses, but to a different AST than the input. The guard must catch it.
        let opts = FormatOptions::default();
        let src = "commons x { type T = Int where Positive }";
        let wrong = "commons x { type T = Bool }";
        let tokens = tokenize(src).unwrap();
        let err = roundtrip_divergence(&tokens, src, wrong, &opts)
            .expect("must reject structural divergence");
        assert_eq!(err.category, "bynk.fmt.roundtrip");
        assert!(err.span.end <= src.len(), "span escapes the source buffer");
    }

    #[test]
    fn roundtrip_error_renders_against_source_without_panicking() {
        // The guard's error is rendered against the *source* (`run_fmt` calls
        // `print_errors(&e.errors, &source, …)`). ariadne uses a primary span
        // unconditionally, so a span outside the source buffer misplaces the
        // caret or panics — exactly in the formatter-bug path this guard must
        // handle gracefully. Render the real diagnostic against a short source
        // and assert it produces output without panicking.
        let err = roundtrip_error("the formatter produced output that no longer parses");
        let source = "commons x {}";
        let rendered = bynk_render::render_errors(std::slice::from_ref(&err), source, "<test>");
        assert!(
            rendered.contains("bynk.fmt.roundtrip"),
            "diagnostic did not render: {rendered}"
        );
    }

    #[test]
    fn unchanged_files_without_comments_format_identically() {
        let src = "commons x { type T = Int where NonNegative }";
        let out = fmt(src);
        // Sanity: the formatter still produces the canonical output for
        // existing fixtures (no spurious comment rendering).
        assert!(!out.contains("//"), "unexpected comment in output: {out}");
    }

    // -- v0.81 storage track: `store` fields and the `:=` write --

    #[test]
    fn formats_store_field_and_cell_write() {
        let src = "context shop {\nagent Counter {\nkey id: String\nstore count: Cell[Int] = 0\non call bump() -> Effect[()] {\ncount := count + 1\n()\n}\n}\n}";
        let out = fmt(src);
        assert!(
            out.contains("store count: Cell[Int] = 0"),
            "store field not formatted: {out}"
        );
        assert!(
            out.contains("count := count + 1"),
            "cell write not formatted: {out}"
        );
        assert_eq!(out, fmt(&out), "formatter not idempotent: {out}");
    }

    #[test]
    fn formats_store_only_agent_without_state_block() {
        let src = "context shop {\nagent Counter {\nkey id: String\nstore count: Cell[Int] = 0\non call get() -> Effect[Int] {\ncount\n}\n}\n}";
        let out = fmt(src);
        // A `store`-only agent emits no empty `state { }` block.
        assert!(!out.contains("state {"), "spurious state block: {out}");
        assert!(out.contains("store count: Cell[Int] = 0"), "{out}");
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    // -- #963: line-width-driven wrapping --

    /// Every line of `out`, measured the way the formatter measures them.
    fn widths(out: &str) -> Vec<usize> {
        out.lines().map(|l| display_width(l, 4)).collect()
    }

    fn assert_within_budget(out: &str) {
        let over: Vec<&str> = out.lines().filter(|l| display_width(l, 4) > 100).collect();
        assert!(over.is_empty(), "lines over 100 columns: {over:?}\n{out}");
    }

    #[test]
    fn fit_test_counts_the_prefix_already_on_the_line() {
        // The body fits on its own but not behind the signature, which is what
        // the pre-#963 fit test measured.
        let src = "commons x {\nfn authorise(amount: Int, ceiling: Int, floor: Int) -> Result[Int, Error] { if amount > ceiling { Err(Declined) } else { Ok(amount) } }\n}";
        let out = fmt(src);
        assert_within_budget(&out);
        assert!(
            out.contains("-> Result[Int, Error] {\n"),
            "body not broken out: {out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    #[test]
    fn wraps_a_long_record_construction_one_field_per_line() {
        let src = "commons x {\nfn make() -> R { R { alpha: \"first value here\", beta: \"second value here\", gamma: \"third value here\", delta: \"fourth value\" } }\n}";
        let out = fmt(src);
        assert_within_budget(&out);
        assert!(
            out.contains("\t\talpha: \"first value here\",\n"),
            "fields not one per line: {out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    #[test]
    fn wraps_a_long_argument_list_without_a_trailing_comma() {
        // A parameter/argument list rejects a trailing comma in the grammar, so
        // the wrapped form must not emit one — `format_source` would refuse the
        // output on the round-trip guard if it did.
        let src = "commons x {\nfn go() -> Int { combine(firstOperandValue, secondOperandValue, thirdOperandValue, fourthOperandValue) }\n}";
        let out = fmt(src);
        assert_within_budget(&out);
        assert!(
            !out.contains(",\n\t\t)"),
            "trailing comma in an argument list: {out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    #[test]
    fn wraps_long_parameter_lists_behind_the_return_type() {
        let src = "context x {\nservice api from http {\non POST(\"/reservations/confirm\") (identifier: String, body: Reservation) -> Effect[HttpResult[Reservation]] by Visitor { Ok(body) }\n}\n}";
        let out = fmt(src);
        assert_within_budget(&out);
        assert!(
            out.contains("\t\t\tidentifier: String,\n"),
            "params not wrapped: {out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    #[test]
    fn breaks_a_long_chain_at_its_dots() {
        let src = "commons x {\nfn go(rows: List[Row]) -> List[Int] { rows.filterOnlyTheInteresting((r) => r.nights > 0).mapEachOntoItsValue((r) => r.nights * r.rate).collect() }\n}";
        let out = fmt(src);
        assert_within_budget(&out);
        assert!(
            out.contains("\n\t\t\t.collect()"),
            "chain not broken at the dots: {out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    #[test]
    fn keeps_a_chain_intact_when_only_an_argument_needs_wrapping() {
        // The chain itself is short; the `match` inside is what spans lines.
        // Breaking at the dots here would be noise, so the chain stays put.
        let src = "commons x {\nfn join(parts: List[String]) -> String {\nlet init: Option[String] = None\nparts.fold(init, (acc, p) => match acc {\nSome(s) => Some(s.concat(p)),\nNone => Some(p),\n}).getOrElse(\"\")\n}\n}";
        let out = fmt(src);
        assert_within_budget(&out);
        assert!(
            out.contains("parts.fold(init, (acc, p) => match acc {"),
            "chain broken needlessly: {out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    #[test]
    fn breaks_a_long_conjunction_before_each_operator() {
        let src = "commons x {\nfn ok(a: Int, b: Int, c: Int, d: Int) -> Bool { aSufficientlyLongPredicateName(a) && anotherRatherLongPredicate(b) && yetAnotherLongishPredicate(c) && theFinalPredicateHere(d) }\n}";
        let out = fmt(src);
        assert_within_budget(&out);
        assert!(
            out.lines().any(|l| l.trim_start().starts_with("&& ")),
            "conjunction not broken at the operators: {out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    #[test]
    fn never_breaks_before_an_arithmetic_operator() {
        // A continuation line opening with `+` does not re-attach on re-parse,
        // so an arithmetic run stays on one line however long it gets.
        let src = "commons x {\nfn total(a: Int, b: Int, c: Int, d: Int) -> Int { someLongFunctionName(a) + anotherLongFunction(b) + aThirdLongFunction(c) + lastOne(d) }\n}";
        let out = fmt(src);
        assert!(
            !out.lines().any(|l| l.trim_start().starts_with("+ ")),
            "broke before `+`, which does not re-parse: {out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    #[test]
    fn wraps_an_over_long_actor_auth_config() {
        let src = "context x {\nactor Partner { auth = Oidc(issuer = \"https://issuer.example.test\", audience = \"reservations-api\", jwks = \"https://issuer.example.test/jwks.json\"), identity = PartnerId }\n}";
        let out = fmt(src);
        assert_within_budget(&out);
        assert!(
            out.contains("\t\tissuer = "),
            "scheme args not wrapped: {out}"
        );
        assert!(out.contains("identity = PartnerId"), "identity lost: {out}");
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    #[test]
    fn a_lone_block_like_argument_hugs_its_call() {
        let src = "commons x {\nfn go(items: List[Item]) -> Effect[()] { items.forEachInTurnAndOrder((item: Item) => { let _ <- store.put(item.identifier, item) }) }\n}";
        let out = fmt(src);
        assert_within_budget(&out);
        assert!(
            out.contains("((item: Item) => {"),
            "sole lambda argument did not hug its call: {out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    #[test]
    fn an_unbreakable_line_is_left_long_rather_than_mangled() {
        // No break point exists inside a string literal; the 100-column target
        // is soft, so the line simply overruns.
        let long = "x".repeat(140);
        let src = format!("commons x {{\nfn go() -> String {{ \"{long}\" }}\n}}");
        let out = fmt(&src);
        assert!(
            widths(&out).iter().any(|w| *w > 100),
            "expected an over-long line: {out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    #[test]
    fn wrapping_is_idempotent_and_structure_preserving_across_widths() {
        // The round-trip guard inside `format_source` already refuses output
        // that re-parses differently, so a successful format at each width is
        // itself the structural assertion.
        let src = "context x {\ntype R = { id: String, name: String, size: Int }\nfn build(id: String, name: String, size: Int) -> R { R { id: id, name: name, size: size } }\nfn pick(rows: List[R]) -> List[String] { rows.filter((r) => r.size > 0).map((r) => r.name).collect() }\n}";
        for width in [40u32, 60, 80, 100, 120] {
            let opts = FormatOptions {
                max_line_width: width,
                ..FormatOptions::default()
            };
            let out = format_source(src, &opts).unwrap_or_else(|e| {
                panic!("width {width}: format refused ({} errors)", e.errors.len())
            });
            let again = format_source(&out, &opts).unwrap_or_else(|e| {
                panic!(
                    "width {width}: reformat refused ({} errors)",
                    e.errors.len()
                )
            });
            assert_eq!(out, again, "width {width}: not idempotent:\n{out}");
        }
    }

    // Events slice 3b (#978): `@schema(N)` on an event round-trips the same
    // way `messages "en" @reference { ... }`'s annotation already does.
    #[test]
    fn event_schema_annotation_formats_and_is_idempotent() {
        let src = "context commerce.order {\nevent PaymentConfirmed @schema(2) = {\norderId: String,\n}\n}";
        let out = fmt(src);
        assert!(
            out.contains("event PaymentConfirmed @schema(2) = {"),
            "{out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    // An event with no annotation formats exactly as it did before this
    // slice — no stray space before `=`.
    #[test]
    fn event_with_no_annotation_formats_unchanged() {
        let src = "context commerce.order {\nevent PaymentConfirmed = {\norderId: String,\n}\n}";
        let out = fmt(src);
        assert!(out.contains("event PaymentConfirmed = {"), "{out}");
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    // Events slice 4 (#985): `via schema(N)` on a subscription header
    // round-trips after the `from Events(...)` header's closing `)`.
    #[test]
    fn via_schema_dispatch_formats_and_is_idempotent() {
        let src = "context commerce.order {\nservice OnPayment from Events(PaymentConfirmed) via schema(2) {\non event(e: PaymentConfirmed) -> Effect[()] {\nEffect.pure(())\n}\n}\n}";
        let out = fmt(src);
        assert!(
            out.contains("from Events(PaymentConfirmed) via schema(2)"),
            "{out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }

    // A payload pattern and a `via schema(...)` clause are independent and
    // combine on one header — neither one's presence should perturb the
    // other's rendering.
    #[test]
    fn via_schema_dispatch_combines_with_a_payload_pattern() {
        let src = "context commerce.order {\nservice OnPayment from Events(PaymentConfirmed { region: Domestic, .. }) via schema(2) {\non event(e: PaymentConfirmed) -> Effect[()] {\nEffect.pure(())\n}\n}\n}";
        let out = fmt(src);
        assert!(
            out.contains("from Events(PaymentConfirmed { region: Domestic, .. }) via schema(2)"),
            "{out}"
        );
        assert_eq!(out, fmt(&out), "not idempotent: {out}");
    }
}
