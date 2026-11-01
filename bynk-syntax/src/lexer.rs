//! Lexer for Bynk v0.
//!
//! Token kinds correspond to the terminals defined in the grammar (spec §3
//! and §4). Whitespace is skipped; `//` line comments are emitted as `Comment`
//! tokens so the formatter can preserve them through round-trips (v1.1 LSP
//! spec §3.5). Doc blocks (`---`) are emitted as `DocBlock` tokens, and
//! one-line docs (`--- text`, #2203) as `DocLine` tokens, both lexed outside
//! of logos (see [`tokenize`]).
//!
//! #2203: the legacy spellings, a `--` comment and a `--|` doc line, still lex
//! as `Comment` and `DocLine` tokens, so every tool keeps working on an
//! unmigrated file and the formatter can rewrite it. The parser reports each
//! as an error with a machine-applicable fix ([`legacy_comment_error`],
//! [`legacy_doc_line_error`]); [`comment_is_legacy`] and
//! [`doc_line_is_legacy`] tell the spellings apart.

use logos::Logos;

use crate::error::{Applicability, CompileError};
use crate::span::{FileId, Span};

/// #1657: the largest `Int`, `Number.MAX_SAFE_INTEGER` (2^53 − 1). An `Int`
/// is a JS safe integer; the range is symmetric.
pub const MAX_SAFE_INT: i64 = 9_007_199_254_740_991;

/// v0.142 (ADR 0166): strip `_` digit separators from a numeric literal's lexeme
/// before it is parsed into a value. The lexer's `IntLit`/`FloatLit` regexes only
/// admit an `_` between two digit groups, so removing every `_` yields a plain
/// digit string; the separators are purely visual. Allocates only when the
/// literal actually carries a separator (the common case does not).
pub(crate) fn strip_digit_separators(lexeme: &str) -> std::borrow::Cow<'_, str> {
    if lexeme.as_bytes().contains(&b'_') {
        std::borrow::Cow::Owned(lexeme.replace('_', ""))
    } else {
        std::borrow::Cow::Borrowed(lexeme)
    }
}

/// Token kinds. Discriminants without payload data; the lexeme is recovered
/// from the source string via the token's [`Span`].
///
/// Note: `--` line comments and `---` doc block markers are handled outside
/// logos (see [`tokenize`]), because doc blocks are delimited by `---` lines
/// containing only the marker and may span multiple source lines.
#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
#[logos(skip r"[ \t\r\n]+")]
pub enum TokenKind {
    // Keywords
    #[token("commons")]
    Commons,
    #[token("type")]
    Type,
    // Events track, slice 0 (spine #936): `event Name = { fields }` — a
    // context-only item declaring a typed fact. RESERVED_CONTEXTUAL, not
    // hard, per ADR 0272's `messages` postmortem (a hard keyword broke the
    // dotted-name case its own worked example used); `event` is at least as
    // likely a parameter/local name (`on event(event: T)`).
    #[token("event")]
    Event,
    // #1883 (static assets), slice 1a: `asset name = "path"` — a context or
    // commons item declaring one static file the platform serves.
    // RESERVED_CONTEXTUAL, not hard, for the same reason as `event` (ADR
    // 0272's `messages` postmortem): `asset` is a likely local or segment name.
    #[token("asset")]
    Asset,
    // #2387 (embedded text), slice 1: `embed name = "path"` — a context or
    // commons item whose value is a project file's text. RESERVED_CONTEXTUAL,
    // for the same reason as `asset`: `embed` is a likely local name.
    #[token("embed")]
    Embed,
    // #2617 (named constants): `const Name: Type = <static expression>` — a
    // context or commons item naming a value. RESERVED_CONTEXTUAL, for the
    // same reason as `embed`: `const` is a plausible local or field name, and
    // it was a free identifier before this keyword existed.
    #[token("const")]
    Const,
    #[token("fn")]
    Fn,
    #[token("where")]
    Where,
    // #548: the `and` keyword was retired — refinement predicates now join with
    // `&&` (the one conjunction spelling). `and` is a free identifier again.
    #[token("true")]
    True,
    #[token("false")]
    False,
    #[token("Int")]
    Int,
    #[token("String")]
    String,
    #[token("Bool")]
    Bool,
    // v0.21 keyword
    #[token("Float")]
    Float,
    // v0.86 keyword (ADR 0112): the `Duration` base type.
    #[token("Duration")]
    Duration,
    // v0.90 keyword (ADR 0114): the `Instant` base type.
    #[token("Instant")]
    Instant,
    // v0.110 keyword (ADR 0142): the `Bytes` base type.
    #[token("Bytes")]
    Bytes,
    // ADR (#2622): the `Decimal` base type.
    #[token("Decimal")]
    Decimal,
    // v0.1 keywords
    #[token("let")]
    Let,
    #[token("if")]
    If,
    #[token("else")]
    Else,
    #[token("Ok")]
    Ok,
    #[token("Err")]
    Err,
    #[token("Result")]
    Result,
    #[token("ValidationError")]
    ValidationError,
    // v0.22b keyword
    #[token("JsonError")]
    JsonError,
    // v0.2 keywords
    #[token("enum")]
    Enum,
    #[token("match")]
    Match,
    #[token("Option")]
    Option,
    #[token("record")]
    Record,
    #[token("self")]
    Self_,
    #[token("Some")]
    Some,
    #[token("None")]
    None,
    #[token("is")]
    Is,
    // v0.3 keywords
    #[token("opaque")]
    Opaque,
    #[token("uses")]
    Uses,
    // v0.4 keywords
    #[token("context")]
    Context,
    #[token("consumes")]
    Consumes,
    #[token("exports")]
    Exports,
    #[token("transparent")]
    Transparent,
    // v0.6 keywords
    #[token("as")]
    As,
    // v0.7 keywords (v0.112: `assert`→`expect`, `test`→`suite`/`case`;
    // v0.118: `mocks` retired — test doubles are stubs at a seam; the stub form
    // moved off the punned `provides` keyword to its own `stub` keyword in the
    // keyword-hygiene batch, #548)
    #[token("expect")]
    Expect,
    #[token("suite")]
    Suite,
    #[token("case")]
    Case,
    // Keyword-hygiene batch (#548): the test-scope stub `stub Cap.op(…) <rhs>`,
    // formerly the third pun on `provides`. `provides` now heads only a provider
    // declaration / external provider.
    #[token("stub")]
    Stub,
    // v0.114 keyword — generative tests (testing track slice 2). `for` and `all`
    // are deliberately *not* keywords: `all` is a list combinator (`all(xs, p)`)
    // and must stay a usable identifier. The `for all` binder is parsed
    // contextually (two identifiers) inside a `property` body instead.
    #[token("property")]
    Property,
    // v0.17 keywords
    #[token("adapter")]
    Adapter,
    #[token("binding")]
    Binding,
    // v0.5 keywords
    #[token("agent")]
    Agent,
    #[token("capability")]
    Capability,
    #[token("Effect")]
    Effect,
    // v0.146 keyword (ADR 0170): `do e` — an effect-performing expression
    // statement (the binder-free `let _ <- e` for a unit effect).
    #[token("do")]
    Do,
    #[token("given")]
    Given,
    #[token("on")]
    On,
    // v0.9 keyword
    #[token("http")]
    Http,
    // v0.10a keyword
    #[token("cron")]
    Cron,
    // v0.10b keyword
    #[token("queue")]
    Queue,
    // v0.44 keywords: `from` heads a service's protocol clause; `protocol` is
    // reserved (protocols are a closed, compiler-known set — no declaration kind).
    #[token("from")]
    From,
    #[token("protocol")]
    Protocol,
    #[token("provides")]
    Provides,
    #[token("service")]
    Service,
    // v0.45 keywords: `actor` heads a boundary-contract declaration; `by`
    // heads a handler's actor clause.
    #[token("actor")]
    Actor,
    #[token("by")]
    By,
    // v0.80 keywords: `invariant` heads an agent invariant declaration; `implies`
    // is the directional logical-implication operator (`P implies Q` ≡ `!P || Q`).
    #[token("invariant")]
    Invariant,
    #[token("implies")]
    Implies,
    // v0.115 keywords — function contracts (testing track slice 3). `requires`
    // and `ensures` head a contract clause on a `fn` signature (between the
    // return type and the body). `result` is deliberately *not* a keyword: it is
    // the ordinary value name outside a contract, so it stays a usable
    // identifier; inside an `ensures` predicate it is bound contextually as the
    // function's return value (parsed by scope, like `for`/`all` in slice 2).
    // Distinct from ADR 0127's capability `@requires` annotation.
    #[token("requires")]
    Requires,
    #[token("ensures")]
    Ensures,
    // v0.116 keyword — step invariants (testing track slice 4). `transition` heads
    // an agent step-invariant declaration (beside `invariant`), a predicate over
    // the pre- and post-commit state pair. `old` and `new` are deliberately *not*
    // keywords: they stay ordinary value names outside a `transition`, and inside a
    // `transition` predicate they are bound contextually to the old/new state
    // records (parsed by scope, like `result` in an `ensures`).
    #[token("transition")]
    Transition,
    // message-bundles track, slice 1: `messages <tag> { "code" => "template" }`
    // — a commons item declaring one locale's message bundle.
    #[token("messages")]
    Messages,
    /// `...` — used in record-spread expressions (v0.5).
    #[token("...")]
    DotDotDot,
    /// `..` — the "rest of the fields" marker on an events subscription
    /// pattern (Events track slice 1, spine #936): `from Events(E { region:
    /// Region.Domestic, .. })`. A genuine token, not two adjacent `Dot`s —
    /// logos maximal-munches `...`/`..`/`.` correctly once all three are
    /// registered, and a real token keeps this in agreement with
    /// tree-sitter's grammar (which declares `".."` as one literal), so the
    /// two parsers cannot diverge on a whitespace-split `. .` the way ADR
    /// 0253 D4 found a leaking `where`-check divergence once before.
    #[token("..")]
    DotDot,
    /// `<-` — Effect bind operator (v0.5).
    #[token("<-")]
    LArrow,
    /// `~>` — asynchronous fire-and-forget send marker (v0.79). A leading
    /// statement marker, never on the RHS of a `let`; distinct from `<-` so the
    /// call site shows whether the caller waits.
    #[token("~>")]
    TildeArrow,
    /// `:=` — Cell write (v0.81, storage track). A handler statement
    /// `cell := expr`; distinct from `=` (binding) and `:` (annotation). Longer
    /// than `:`/`=` so logos matches it as one token.
    #[token(":=")]
    ColonEq,

    /// A documentation block: `---` line ... `---` line. The token's span
    /// covers the full block including both `---` markers. The body content
    /// is recovered from the source via the span (see [`doc_block_content`]).
    /// Inserted by [`tokenize`]; not lexed by logos directly.
    DocBlock,

    /// #2203: the one-line doc form, `--- text`: a marker of three or more
    /// dashes, at least one space or tab, then text, first on its line
    /// (indentation aside). An optional closer, a final run of exactly the
    /// opener's dashes preceded by whitespace, is not part of the doc. A
    /// marker alone on its line is a [`TokenKind::DocBlock`] fence instead.
    /// Like [`TokenKind::DocBlock`], the span runs from the marker through the
    /// terminating newline (or EOF), so the parser's blank-line check treats
    /// both forms alike ([`doc_line_text`] recovers the text). The legacy
    /// `--| text` spelling (#1888) lexes as this token too, and the parser
    /// reports it ([`doc_line_is_legacy`]). Inserted by [`tokenize`].
    DocLine,

    /// A line comment: `// ...` running to end of line, anywhere (#2203). The
    /// span starts at the `//` marker and runs through the last character
    /// before the terminating newline (exclusive). The trivia body (the text
    /// after the marker) is recovered from the source via the span
    /// ([`comment_body`]). The legacy `-- ...` spelling lexes as this token
    /// too, and the parser reports it ([`comment_is_legacy`]). Inserted by
    /// [`tokenize`]; not lexed by logos directly.
    Comment,

    // Identifier
    #[regex(r"[A-Za-z][A-Za-z0-9_]*")]
    Ident,

    // Literals. v0.142 (ADR 0166): an `_` digit separator may appear between
    // digits (`1_048_576`) — never leading, trailing, or doubled (each `_` must
    // sit between two digit groups). The separators are stripped before the value
    // is parsed; they are purely visual.
    #[regex(r"[0-9]+(_[0-9]+)*")]
    IntLit,
    // A float literal: fraction with a digit on both sides of the `.`, an
    // exponent, or both (v0.21 §3). `1.` and `.5` are NOT float literals —
    // the digit-both-sides rule keeps `2.5.round()` / `1.toFloat()` lexing
    // as method calls on numeric literals. Digit separators (v0.142) may appear
    // in any digit group, including the exponent.
    #[regex(
        r"[0-9]+(_[0-9]+)*\.[0-9]+(_[0-9]+)*([eE][+-]?[0-9]+(_[0-9]+)*)?|[0-9]+(_[0-9]+)*[eE][+-]?[0-9]+(_[0-9]+)*"
    )]
    FloatLit,
    // A decimal literal (#2622): an integer or fraction with a trailing `d`
    // (`1.20d`, `5d`). The as-written scale is the number of fraction digits.
    // No exponent form. Digit separators may appear in either group.
    #[regex(r"[0-9]+(_[0-9]+)*(\.[0-9]+(_[0-9]+)*)?d")]
    DecimalLit,
    // A string literal with no interpolation hole: `"…"`, the raw forms
    // `r"…"` / `r#"…"#`, and the multi-line forms `"""…"""` / `r"""…"""`
    // (#2619). Hand-scanned in `tokenize` (never produced by logos) so one
    // scanner owns every form; the span covers the whole lexeme and
    // [`decode_string_literal`] recovers the value.
    StrLit,
    // An interpolated string `"… \(expr) …"` (v0.43). Hand-scanned in
    // `tokenize` (logos cannot balance the holes' parens), never produced by
    // the logos lexer — like [`TokenKind::DocBlock`]/[`TokenKind::Comment`].
    // The span covers the whole `"…"`; the parser splits chunks from holes.
    InterpStr,

    // Multi-char operators
    #[token("->")]
    Arrow,
    #[token("==")]
    EqEq,
    #[token("!=")]
    BangEq,
    #[token("<=")]
    LtEq,
    #[token(">=")]
    GtEq,
    #[token("&&")]
    AmpAmp,
    #[token("||")]
    PipePipe,

    // Single-char operators
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("!")]
    Bang,
    #[token("=")]
    Eq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    // v0.1 postfix operator
    #[token("?")]
    Question,
    // v0.2 match-arm arrow
    #[token("=>")]
    FatArrow,
    // v0.2 wildcard pattern. An identifier must start with a letter, so
    // `_foo` lexes as `_` then `foo`, never as one Ident.
    #[token("_")]
    Underscore,
    // v0.2 sum-type variant separator (also used as future bitwise OR);
    // single `|` distinct from `||`.
    #[token("|")]
    Pipe,
    /// `@` — storage-annotation marker (v0.85, storage track; ADR 0111). Leads a
    /// `@name(args)` annotation on a `store` field (`@ttl(…)`/`@indexed(…)`); it
    /// appears only in store-field-declaration position, never as an expression
    /// operator.
    #[token("@")]
    At,

    // Punctuation
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token(".")]
    Dot,
}

impl TokenKind {
    /// Human-readable display name for diagnostics.
    pub fn describe(self) -> &'static str {
        use TokenKind::*;
        match self {
            Commons => "`commons`",
            Type => "`type`",
            Event => "`event`",
            Asset => "`asset`",
            Embed => "`embed`",
            Const => "`const`",
            Fn => "`fn`",
            Where => "`where`",
            True => "`true`",
            False => "`false`",
            Int => "`Int`",
            String => "`String`",
            Bool => "`Bool`",
            Float => "`Float`",
            Duration => "`Duration`",
            Instant => "`Instant`",
            Bytes => "`Bytes`",
            Decimal => "`Decimal`",
            Let => "`let`",
            If => "`if`",
            Else => "`else`",
            Ok => "`Ok`",
            Err => "`Err`",
            Result => "`Result`",
            ValidationError => "`ValidationError`",
            JsonError => "`JsonError`",
            Enum => "`enum`",
            Match => "`match`",
            Option => "`Option`",
            Record => "`record`",
            Self_ => "`self`",
            Some => "`Some`",
            None => "`None`",
            Is => "`is`",
            Opaque => "`opaque`",
            Uses => "`uses`",
            Context => "`context`",
            Consumes => "`consumes`",
            Exports => "`exports`",
            Transparent => "`transparent`",
            As => "`as`",
            Expect => "`expect`",
            Suite => "`suite`",
            Case => "`case`",
            Property => "`property`",
            Adapter => "`adapter`",
            Binding => "`binding`",
            Agent => "`agent`",
            Capability => "`capability`",
            Effect => "`Effect`",
            Do => "`do`",
            Given => "`given`",
            On => "`on`",
            Http => "`http`",
            Cron => "`cron`",
            Queue => "`queue`",
            From => "`from`",
            Protocol => "`protocol`",
            Provides => "`provides`",
            Stub => "`stub`",
            Service => "`service`",
            Actor => "`actor`",
            By => "`by`",
            Invariant => "`invariant`",
            Implies => "`implies`",
            Requires => "`requires`",
            Ensures => "`ensures`",
            Transition => "`transition`",
            Messages => "`messages`",
            ColonEq => "`:=`",
            DotDotDot => "`...`",
            DotDot => "`..`",
            LArrow => "`<-`",
            TildeArrow => "`~>`",
            DocBlock => "documentation block",
            DocLine => "one-line documentation (`--- text`)",
            Comment => "line comment",
            Ident => "identifier",
            IntLit => "integer literal",
            FloatLit => "float literal",
            DecimalLit => "decimal literal",
            StrLit => "string literal",
            InterpStr => "interpolated string",
            Arrow => "`->`",
            EqEq => "`==`",
            BangEq => "`!=`",
            LtEq => "`<=`",
            GtEq => "`>=`",
            AmpAmp => "`&&`",
            PipePipe => "`||`",
            Plus => "`+`",
            Minus => "`-`",
            Star => "`*`",
            Slash => "`/`",
            Bang => "`!`",
            Eq => "`=`",
            Lt => "`<`",
            Gt => "`>`",
            Question => "`?`",
            FatArrow => "`=>`",
            Underscore => "`_`",
            Pipe => "`|`",
            At => "`@`",
            LParen => "`(`",
            RParen => "`)`",
            LBrace => "`{`",
            RBrace => "`}`",
            LBracket => "`[`",
            RBracket => "`]`",
            Comma => "`,`",
            Colon => "`:`",
            Dot => "`.`",
        }
    }
}

/// A token plus its source span.
#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Tokenise a source string with no real file identity — every span's
/// [`FileId`] defaults to [`FileId::UNKNOWN`]. See [`tokenize_in`] for the
/// real-identity entry point production callers use.
pub fn tokenize(source: &str) -> Result<Vec<Token>, CompileError> {
    tokenize_in(source, FileId::UNKNOWN)
}

/// Tokenise a source string, stamping every token's span with `file`.
/// Returns the full token vector or the first lexical error.
///
/// Doc blocks (`---` ... `---`), one-line docs (`--- text`) and line comments
/// (`// ...`) are recognised outside the logos-generated lexer: we scan the
/// source one segment at a time, dispatching to logos for ordinary tokens
/// between non-token spans.
pub fn tokenize_in(source: &str, file: FileId) -> Result<Vec<Token>, CompileError> {
    tokenize_mode(source, file, false)
}

/// #2203: tokenise `source` as the lexer read it before `//` comments, for
/// the formatter's migration of a file that still uses the legacy spellings.
/// A `--` opens a comment wherever it starts the input or follows whitespace
/// (ADR 0188 D1), so `--note` and a `--- Helpers ---` divider are comments, as
/// they were; `--|` first on its line is a doc line. `//` comments and `---`
/// blocks lex as in [`tokenize`], so a half-migrated file reads too. Every
/// legacy token keeps its legacy spelling, so [`comment_body`] and
/// [`doc_line_text`] read it and the formatter prints the new one.
pub fn tokenize_legacy(source: &str) -> Result<Vec<Token>, CompileError> {
    tokenize_mode(source, FileId::UNKNOWN, true)
}

fn tokenize_mode(source: &str, file: FileId, legacy: bool) -> Result<Vec<Token>, CompileError> {
    let mut tokens = Vec::new();
    let bytes = source.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        // Detect a `---` doc-block marker at the start of a line (the line may
        // begin with leading whitespace; the marker itself must be alone on
        // its line).
        if let Some((open_end, dashes)) = doc_block_open_at(source, pos) {
            // Find the matching closing marker: a line of exactly as many
            // dashes as the opener (#1885), so a `----` block can hold `---`.
            match doc_block_close(source, open_end, dashes) {
                Some((close_start, close_end)) => {
                    let span = Span::new_in(file, pos, close_end);
                    tokens.push(Token {
                        kind: TokenKind::DocBlock,
                        span,
                    });
                    let _ = close_start;
                    pos = close_end;
                    continue;
                }
                None => {
                    return Err(CompileError::new(
                        "bynk.lex.unclosed_doc_block",
                        Span::new_in(file, pos, open_end),
                        "documentation block opened but never closed",
                    )
                    .with_note(format!(
                        "a doc block is closed by a line of exactly {dashes} dashes on its own; \
                         a line of three or more dashes is a doc-block marker, not a divider, so \
                         for a divider use a line comment, such as `// ---------`"
                    )));
                }
            }
        }
        let line_first = only_indent_before(bytes, pos);
        // #2203: a `--- text` line is a one-line doc, its span running through
        // the newline as a doc block's does. Before #2203 it was a `--`
        // comment, and a legacy read keeps it one.
        if line_first && doc_line_marker(bytes, pos) {
            if legacy {
                pos = push_comment(&mut tokens, bytes, pos, file);
            } else {
                pos = push_doc_line(&mut tokens, bytes, pos, file);
            }
            continue;
        }
        // #2203 (DECISION E): `///` first on its line is a Rust or C# doc
        // habit. It never lexed (Bynk has no prefix `/`), so it is a plain
        // lexical error pointing at `---`. A longer run of slashes is a `//`
        // comment, as `////` is in Rust.
        if line_first && bytes[pos..].starts_with(b"///") && bytes.get(pos + 3) != Some(&b'/') {
            return Err(slash_doc_error(bytes, pos, file));
        }
        // #1888: the legacy `--|` doc line, first on its line. It still lexes
        // as a doc line; the parser reports its spelling.
        if line_first && bytes[pos..].starts_with(b"--|") {
            pos = push_doc_line(&mut tokens, bytes, pos, file);
            continue;
        }
        // #2203: a `//` line comment, anywhere: emit a `Comment` token covering
        // everything up to (but not including) the terminating newline.
        // Preserving comments as trivia tokens lets the parser attach them to
        // declarations so the formatter can emit them in place (v1.1 LSP spec
        // §3.5). There is no prefix `/`, so `a//b` cannot mean anything else
        // and needs no whitespace rule.
        if bytes[pos..].starts_with(b"//") {
            pos = push_comment(&mut tokens, bytes, pos, file);
            continue;
        }
        // The legacy `--` comment (DECISION E): at the start of the input or
        // after whitespace, followed by a space or the end of the line, or by
        // `|` (a `--|` after code was a comment, #1888 DECISION E). It
        // still lexes as a comment, so an old `x = 1 -- note` is never read as
        // `1 - -note`; the parser reports its spelling. Any other `--` is two
        // `-` operators, so `a--b` and `x--` are subtraction. A legacy read
        // takes ADR 0188 D1's rule instead: any whitespace-preceded `--`.
        let after_space = pos == 0 || matches!(bytes[pos - 1], b' ' | b'\t' | b'\r' | b'\n');
        if after_space
            && bytes[pos..].starts_with(b"--")
            && (legacy
                || matches!(
                    bytes.get(pos + 2),
                    None | Some(b' ' | b'\t' | b'\r' | b'\n' | b'|')
                ))
        {
            pos = push_comment(&mut tokens, bytes, pos, file);
            continue;
        }
        // Skip ordinary whitespace inline (logos handles it too, but we may
        // be in the middle of the source between specials).
        if matches!(bytes[pos], b' ' | b'\t' | b'\r' | b'\n') {
            pos += 1;
            continue;
        }
        // A string literal in any of its forms (#2619): plain, raw, or
        // multi-line. One hand-scanner validates and decodes the literal; a
        // literal holding a `\(` hole lexes as `InterpStr`, any other as
        // `StrLit`.
        if string_open(bytes, pos).is_some() {
            let (end, segments) = lex_string(source, pos, 0, file)?;
            let kind = if segments.iter().any(|s| matches!(s, InterpSegment::Hole(_))) {
                TokenKind::InterpStr
            } else {
                TokenKind::StrLit
            };
            tokens.push(Token {
                kind,
                span: Span::new_in(file, pos, end),
            });
            pos = end;
            continue;
        }
        // Otherwise dispatch a single logos token starting at `pos`.
        let mut lex = TokenKind::lexer(&source[pos..]);
        let Some(result) = lex.next() else {
            // No token at this position; treat as unexpected character so
            // the user sees something useful.
            let ch = source[pos..].chars().next().unwrap_or('\0');
            let span = Span::new_in(file, pos, pos + ch.len_utf8());
            return Err(CompileError::new(
                "bynk.lex.unexpected_character",
                span,
                format!("unexpected character `{ch}`"),
            ));
        };
        let local = lex.span();
        let span: Span = Span::new_in(file, pos + local.start, pos + local.end);
        match result {
            Ok(kind) => {
                // #1657 (runtime-semantics track §3.4): an `Int` is a JS safe
                // integer, ±(2^53 − 1), the range the runtime represents
                // exactly. A larger literal would round silently
                // (`9007199254740993 == 9007199254740992`). The sign is a
                // separate operator, and the bound is symmetric, so the
                // magnitude decides.
                if kind == TokenKind::IntLit {
                    let slice = &source[span.range()];
                    if !strip_digit_separators(slice)
                        .parse::<i64>()
                        .is_ok_and(|n| n <= MAX_SAFE_INT)
                    {
                        return Err(CompileError::new(
                            "bynk.lex.integer_overflow",
                            span,
                            format!("integer literal `{slice}` is out of range for an `Int`"),
                        )
                        .with_note(
                            "an `Int` is a safe integer, -(2^53 - 1) to 2^53 - 1 \
                             (±9007199254740991); use a `Float` or a `String` for a larger value",
                        ));
                    }
                }
                if kind == TokenKind::DecimalLit {
                    // #2622: the limits every `Decimal` obeys, so a literal
                    // always encodes to text the wire decoder accepts.
                    let slice = &source[span.range()];
                    let body = strip_digit_separators(&slice[..slice.len() - 1]).into_owned();
                    let frac = body.split_once('.').map_or(0, |(_, f)| f.len());
                    if frac > 100 || body.len() > 1000 {
                        return Err(CompileError::new(
                            "bynk.lex.decimal_literal_too_large",
                            span,
                            "decimal literal is too large",
                        )
                        .with_note(
                            "a `Decimal` has at most 100 fraction digits and 1000 characters",
                        ));
                    }
                }
                if kind == TokenKind::FloatLit {
                    let slice = &source[span.range()];
                    match strip_digit_separators(slice).parse::<f64>() {
                        Ok(v) if v.is_finite() => {}
                        _ => {
                            return Err(CompileError::new(
                                "bynk.lex.float_literal_overflow",
                                span,
                                format!(
                                    "float literal `{slice}` is out of range for a 64-bit float"
                                ),
                            )
                            .with_note(
                                "the literal does not fit a finite IEEE 754 double; \
                                 the largest finite value is ~1.8e308",
                            ));
                        }
                    }
                }
                tokens.push(Token { kind, span });
                pos = span.end;
            }
            Err(()) => {
                let slice = &source[span.range()];
                let ch = slice.chars().next().unwrap_or('\0');
                let err = if ch == '"' {
                    CompileError::new(
                        "bynk.lex.unterminated_string",
                        span,
                        "unterminated string literal",
                    )
                    .with_note(
                        "string literals must close with `\"` on the same line; \
                         supported escapes are `\\n`, `\\t`, `\\\"`, `\\\\`",
                    )
                } else {
                    CompileError::new(
                        "bynk.lex.unexpected_character",
                        span,
                        format!("unexpected character `{ch}`"),
                    )
                };
                return Err(err);
            }
        }
    }
    Ok(tokens)
}

/// Like [`tokenize`], but with every interpolated-string token replaced by the
/// tokens of its holes — each hole's bytes re-lexed and its token spans rebased
/// to absolute source positions (the same rebase [`crate::parser`] applies when
/// parsing a hole), recursing through nested interpolation. Chunk (literal) text
/// between holes yields no tokens.
///
/// An interpolated string lexes to a single opaque `InterpStr` token, so the
/// LSP's token-based cursor resolution (hover, go-to-definition, references,
/// semantic tokens) is otherwise blind to identifiers inside `"… \(name) …"`.
/// Expanding the holes makes those identifiers visible as ordinary `Ident`
/// tokens with their real spans. (Issue #473.)
///
/// On a malformed interpolation (an `InterpStr` whose holes don't split, or a
/// hole whose bytes don't re-lex) the offending token is kept opaque rather than
/// dropped, so resolution degrades to the pre-fix behaviour instead of losing
/// tokens.
pub fn tokenize_expanding_holes(source: &str) -> Result<Vec<Token>, CompileError> {
    tokenize_expanding_holes_in(source, FileId::UNKNOWN)
}

/// Like [`tokenize_expanding_holes`], but stamping every token's span
/// (including rebased hole tokens) with `file`.
pub fn tokenize_expanding_holes_in(source: &str, file: FileId) -> Result<Vec<Token>, CompileError> {
    let mut out = Vec::new();
    for tok in tokenize_in(source, file)? {
        expand_hole_token(source, file, tok, &mut out);
    }
    Ok(out)
}

/// Push `tok` onto `out`, expanding it into its holes' tokens if it is an
/// `InterpStr` (see [`tokenize_expanding_holes`]); otherwise push it as-is.
fn expand_hole_token(source: &str, file: FileId, tok: Token, out: &mut Vec<Token>) {
    if tok.kind != TokenKind::InterpStr {
        out.push(tok);
        return;
    }
    let Ok(segments) = split_interp(source, tok.span) else {
        out.push(tok); // malformed interpolation — keep the opaque token
        return;
    };
    for segment in segments {
        let InterpSegment::Hole(hole) = segment else {
            continue; // chunk text carries no tokens
        };
        let Ok(hole_tokens) = tokenize_in(&source[hole.range()], file) else {
            continue;
        };
        for mut t in hole_tokens {
            // Rebase the hole's local spans to absolute source positions.
            t.span = Span::new_in(file, t.span.start + hole.start, t.span.end + hole.start);
            expand_hole_token(source, file, t, out); // recurse for nested interpolation
        }
    }
}

// ---- String literals (v0.43 interpolation; #2619 raw / multi-line / more escapes) ----

/// The shape of a string literal opening at some offset.
#[derive(Clone, Copy)]
struct StringOpen {
    /// `r"…"`, `r#"…"#`, `r"""…"""`: no escapes, no interpolation.
    raw: bool,
    /// `"""…"""` / `r"""…"""`: the body spans lines.
    multi: bool,
    /// Number of `#` between the `r` and the opening quote (single-line raw only).
    hashes: usize,
    /// Offset just past the opening delimiter (`"`, `"""`, `r#"`, …).
    body: usize,
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Does a string literal open at `start`? `r` opens a raw string only when it
/// is *immediately* followed by `#`s and a `"`, so an identifier `r` followed by
/// a (spaced) string is unaffected.
fn string_open(bytes: &[u8], start: usize) -> Option<StringOpen> {
    let raw = bytes.get(start) == Some(&b'r');
    let mut p = start + usize::from(raw);
    let mut hashes = 0;
    if raw {
        while bytes.get(p) == Some(&b'#') {
            hashes += 1;
            p += 1;
        }
    }
    if bytes.get(p) != Some(&b'"') {
        return None;
    }
    let multi = hashes == 0 && bytes[p..].starts_with(b"\"\"\"");
    Some(StringOpen {
        raw,
        multi,
        hashes,
        body: if multi { p + 3 } else { p + 1 },
    })
}

fn unterminated_string(file: FileId, start: usize, end: usize, multi: bool) -> CompileError {
    let note = if multi {
        "a multi-line string closes with `\"\"\"` on a line of its own"
    } else {
        "string literals must close with `\"` on the same line (use `\"\"\"` for a multi-line string); \
         supported escapes: \\n \\r \\t \\0 \\\" \\\\ \\u{…} and `\\(…)` interpolation"
    };
    CompileError::new(
        "bynk.lex.unterminated_string",
        Span::new_in(file, start, end),
        "unterminated string literal",
    )
    .with_note(note)
}

/// Span of the byte range `from..to`, advanced to a char boundary so it never
/// splits a multibyte codepoint (a fuzz invariant).
fn boundary_span(source: &str, file: FileId, from: usize, to: usize) -> Span {
    let mut end = to.min(source.len());
    while end < source.len() && !source.is_char_boundary(end) {
        end += 1;
    }
    Span::new_in(file, from, end)
}

const ESCAPE_NOTE: &str = "supported escapes: \\n \\r \\t \\0 \\\" \\\\ \\u{…} \\(…)";

/// Scan and decode the string literal opening at `start`, returning the offset
/// just past it and its segments (literal chunks with escapes resolved, and the
/// spans of any `\(…)` holes). This is the single scanner for every form.
fn lex_string(
    source: &str,
    start: usize,
    depth: usize,
    file: FileId,
) -> Result<(usize, Vec<InterpSegment>), CompileError> {
    let bytes = source.as_bytes();
    let open = string_open(bytes, start).expect("caller checked string_open");
    if depth > crate::MAX_NESTING_DEPTH {
        // Anchor on the opening `"` of the string that tipped over the limit.
        return Err(too_deeply_nested_interpolation(Span::new_in(
            file,
            start,
            start + 1,
        )));
    }
    let mut segments = Vec::new();
    let mut chunk = String::new();
    if open.multi {
        let end = lex_multiline(source, start, open, depth, file, &mut segments, &mut chunk)?;
        if !chunk.is_empty() {
            segments.push(InterpSegment::Chunk(chunk));
        }
        return Ok((end, segments));
    }
    if open.raw {
        // `r#"…"#`: the body runs to the first `"` followed by `hashes` `#`s.
        let mut i = open.body;
        loop {
            if i >= bytes.len() || bytes[i] == b'\n' {
                return Err(unterminated_string(file, start, i.min(bytes.len()), false));
            }
            if bytes[i] == b'"' && (1..=open.hashes).all(|k| bytes.get(i + k) == Some(&b'#')) {
                break;
            }
            i += 1;
        }
        chunk.push_str(&source[open.body..i]);
        if !chunk.is_empty() {
            segments.push(InterpSegment::Chunk(chunk));
        }
        return Ok((i + 1 + open.hashes, segments));
    }
    let end = decode_run(
        source,
        open.body,
        Stop::Quote(start),
        depth,
        file,
        &mut segments,
        &mut chunk,
    )?;
    if !chunk.is_empty() {
        segments.push(InterpSegment::Chunk(chunk));
    }
    Ok((end + 1, segments))
}

/// Where a [`decode_run`] stops.
#[derive(Clone, Copy)]
enum Stop {
    /// At the closing `"` of a single-line string (the payload is the literal's
    /// start, for the unterminated error); a newline or end of input first is an
    /// error.
    Quote(usize),
    /// At a fixed offset: one line of a multi-line string, where a bare `"""`
    /// inside the line is an error (the closing delimiter has a line to itself).
    End(usize),
}

/// Decode non-raw string text from `from`, appending to `chunk` / `segments`.
/// Returns the offset where it stopped (the closing `"` for [`Stop::Quote`]).
fn decode_run(
    source: &str,
    from: usize,
    stop: Stop,
    depth: usize,
    file: FileId,
    segments: &mut Vec<InterpSegment>,
    chunk: &mut String,
) -> Result<usize, CompileError> {
    let bytes = source.as_bytes();
    let limit = match stop {
        Stop::Quote(_) => bytes.len(),
        Stop::End(e) => e,
    };
    let mut i = from;
    loop {
        if i >= limit {
            match stop {
                Stop::End(_) => return Ok(i),
                Stop::Quote(start) => {
                    return Err(unterminated_string(file, start, i.min(bytes.len()), false));
                }
            }
        }
        match bytes[i] {
            b'\n' if matches!(stop, Stop::Quote(_)) => {
                let Stop::Quote(start) = stop else {
                    unreachable!()
                };
                return Err(unterminated_string(file, start, i, false));
            }
            b'"' => match stop {
                Stop::Quote(_) => return Ok(i),
                Stop::End(e) => {
                    if bytes[i..e].starts_with(b"\"\"\"") {
                        return Err(CompileError::new(
                            "bynk.lex.multiline_close_not_own_line",
                            Span::new_in(file, i, i + 3),
                            "a `\"\"\"` inside a multi-line string closes it, but it is not on a line of its own",
                        )
                        .with_note(
                            "put the closing `\"\"\"` on its own line, or escape a quote as `\\\"`",
                        ));
                    }
                    chunk.push('"');
                    i += 1;
                }
            },
            b'\\' => {
                let next = if i + 1 < limit {
                    Some(bytes[i + 1])
                } else {
                    None
                };
                match next {
                    Some(b'n') => {
                        chunk.push('\n');
                        i += 2;
                    }
                    Some(b't') => {
                        chunk.push('\t');
                        i += 2;
                    }
                    Some(b'r') => {
                        chunk.push('\r');
                        i += 2;
                    }
                    Some(b'0') => {
                        chunk.push('\0');
                        i += 2;
                    }
                    Some(b'"') => {
                        chunk.push('"');
                        i += 2;
                    }
                    Some(b'\\') => {
                        chunk.push('\\');
                        i += 2;
                    }
                    Some(b'u') => {
                        let (ch, after) = decode_unicode_escape(source, i, limit, file)?;
                        chunk.push(ch);
                        i = after;
                    }
                    Some(b'(') => {
                        if !chunk.is_empty() {
                            segments.push(InterpSegment::Chunk(std::mem::take(chunk)));
                        }
                        let hole_start = i + 2;
                        let after = scan_hole(bytes, source, hole_start, depth + 1, file)?;
                        // `after` is one past the matching `)`; the hole body is
                        // everything up to that `)`.
                        segments.push(InterpSegment::Hole(Span::new_in(
                            file,
                            hole_start,
                            after - 1,
                        )));
                        i = after;
                    }
                    other => {
                        let shown = other.map(|b| (b as char).to_string()).unwrap_or_default();
                        // Cover `\` plus the whole offending char, advanced to a
                        // char boundary.
                        return Err(CompileError::new(
                            "bynk.lex.bad_escape",
                            boundary_span(source, file, i, i + 2),
                            format!("invalid escape sequence `\\{shown}` in string literal"),
                        )
                        .with_note(ESCAPE_NOTE));
                    }
                }
            }
            // Any other byte: copy the whole char. UTF-8 continuation bytes are
            // all >= 0x80, so they never collide with the ASCII specials.
            _ => {
                let ch = source[i..].chars().next().unwrap();
                chunk.push(ch);
                i += ch.len_utf8();
            }
        }
    }
}

/// Decode `\u{…}` at `i` (the `\`): 1–6 hex digits naming a Unicode scalar
/// value. Returns the char and the offset past the `}`. A lone surrogate
/// (D800–DFFF) or a value above 10FFFF is a lexical error, so every literal is
/// valid UTF-8 text.
fn decode_unicode_escape(
    source: &str,
    i: usize,
    limit: usize,
    file: FileId,
) -> Result<(char, usize), CompileError> {
    let bytes = source.as_bytes();
    let malformed = |end: usize| {
        CompileError::new(
            "bynk.lex.bad_escape",
            Span::new_in(file, i, end),
            "invalid `\\u{…}` escape in string literal",
        )
        .with_note(
            "a Unicode escape is `\\u{` followed by 1 to 6 hex digits and `}`, e.g. `\\u{1F600}`",
        )
    };
    let mut p = i + 2; // past `\u`
    if p >= limit || bytes[p] != b'{' {
        return Err(malformed(p));
    }
    p += 1;
    let digits_start = p;
    while p < limit && bytes[p].is_ascii_hexdigit() {
        p += 1;
    }
    let digits = p - digits_start;
    if digits == 0 || digits > 6 || p >= limit || bytes[p] != b'}' {
        return Err(malformed(p.min(limit)));
    }
    let value = u32::from_str_radix(&source[digits_start..p], 16).expect("hex digits");
    p += 1; // past `}`
    match char::from_u32(value) {
        Some(ch) => Ok((ch, p)),
        None => Err(CompileError::new(
            "bynk.lex.bad_unicode_escape",
            Span::new_in(file, i, p),
            format!("`\\u{{{value:X}}}` is not a Unicode scalar value"),
        )
        .with_note(
            "surrogate code points (D800 to DFFF) and values above 10FFFF cannot appear in a string",
        )),
    }
}

/// Scan and decode a multi-line literal (`"""` / `r"""`) opening at `start`.
/// Returns the offset just past the closing `"""`. The closing delimiter sits on
/// a line of its own; its indentation is stripped from every body line, and a
/// non-blank line indented less is an error. Newlines in the value are always
/// `\n`, whatever the file's line endings.
fn lex_multiline(
    source: &str,
    start: usize,
    open: StringOpen,
    depth: usize,
    file: FileId,
    segments: &mut Vec<InterpSegment>,
    chunk: &mut String,
) -> Result<usize, CompileError> {
    let bytes = source.as_bytes();
    // The opening `"""` is followed by a newline (`\n` or `\r\n`).
    let mut pos = match (bytes.get(open.body), bytes.get(open.body + 1)) {
        (Some(b'\n'), _) => open.body + 1,
        (Some(b'\r'), Some(b'\n')) => open.body + 2,
        _ => {
            return Err(CompileError::new(
                "bynk.lex.multiline_open",
                Span::new_in(file, start, open.body),
                "the opening `\"\"\"` of a multi-line string must be followed by a newline",
            )
            .with_note("the text starts on the next line; write `\"\"\"` then press enter"));
        }
    };
    // Pass 1: split into lines up to the closing line.
    let mut lines: Vec<(usize, usize)> = Vec::new(); // (start, end) excluding the `\n` / `\r\n`
    let (indent_len, end) = loop {
        if pos >= bytes.len() {
            return Err(unterminated_string(file, start, bytes.len(), true));
        }
        let line_end = bytes[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |n| pos + n);
        let mut ws = pos;
        while ws < line_end && matches!(bytes[ws], b' ' | b'\t') {
            ws += 1;
        }
        // A closing delimiter ends its line's text: `"""quoted"""` is body text
        // holding a stray `"""`, which `decode_run` reports.
        if bytes[ws..line_end].starts_with(b"\"\"\"")
            && !bytes
                .get(ws + 3)
                .is_some_and(|&b| is_ident_byte(b) || b == b'"')
        {
            break (ws - pos, ws + 3);
        }
        let trimmed_end = if line_end > pos && bytes[line_end - 1] == b'\r' {
            line_end - 1
        } else {
            line_end
        };
        lines.push((pos, trimmed_end));
        if line_end >= bytes.len() {
            return Err(unterminated_string(file, start, bytes.len(), true));
        }
        pos = line_end + 1;
    };
    let closing_line_start = end - 3 - indent_len;
    let indent = &bytes[closing_line_start..closing_line_start + indent_len];
    // Pass 2: strip the indentation and decode each line.
    for (n, &(ls, le)) in lines.iter().enumerate() {
        if n > 0 {
            chunk.push('\n');
        }
        let line = &bytes[ls..le];
        let blank = line.iter().all(|b| matches!(b, b' ' | b'\t'));
        let content = if line.starts_with(indent) {
            ls + indent_len
        } else if blank {
            le // a blank line shorter than the indentation is empty
        } else {
            return Err(CompileError::new(
                "bynk.lex.multiline_indent",
                boundary_span(source, file, ls, le),
                "a line of this multi-line string is indented less than its closing `\"\"\"`",
            )
            .with_note(if line.len() >= indent.len() {
                "the indentation must match the closing delimiter's character for character: \
                 a tab does not match a space, so mixed tabs and spaces fail here"
            } else {
                "every non-blank line must start with the closing delimiter's indentation, \
                 which is stripped from the value"
            }));
        };
        if open.raw {
            let text = &source[content..le];
            if let Some(at) = text.find("\"\"\"") {
                return Err(CompileError::new(
                    "bynk.lex.multiline_close_not_own_line",
                    Span::new_in(file, content + at, content + at + 3),
                    "a `\"\"\"` inside a multi-line string closes it, but it is not on a line of its own",
                )
                .with_note("put the closing `\"\"\"` on its own line"));
            }
            chunk.push_str(text);
        } else {
            decode_run(source, content, Stop::End(le), depth, file, segments, chunk)?;
        }
    }
    Ok(end)
}

/// Scan an interpolation hole body. `start` points just past the `\(`; returns
/// the offset just past the matching `)`. Tracks paren depth and skips nested
/// strings (whose own parens must not close the hole), recursing through
/// [`lex_string`] so nested interpolation nests correctly. (v0.43.)
fn scan_hole(
    bytes: &[u8],
    source: &str,
    start: usize,
    nesting: usize,
    file: FileId,
) -> Result<usize, CompileError> {
    if nesting > crate::MAX_NESTING_DEPTH {
        // Anchor on the `\(` opener that tipped over the limit; it sits two
        // bytes before `start` and is pure ASCII, so the span stays on char
        // boundaries (a fuzz invariant).
        return Err(too_deeply_nested_interpolation(Span::new_in(
            file,
            start.saturating_sub(2),
            start,
        )));
    }
    let mut i = start;
    let mut depth = 1usize;
    loop {
        if i >= bytes.len() || bytes[i] == b'\n' {
            return Err(CompileError::new(
                "bynk.lex.unterminated_interpolation",
                Span::new_in(file, start.saturating_sub(2), i.min(bytes.len())),
                "unterminated interpolation hole",
            )
            .with_note(
                "an interpolation hole `\\(…)` must close with a matching `)` on the same line",
            ));
        }
        match bytes[i] {
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            b'"' => i = nested_string(source, i, nesting, file)?,
            // A nested raw string: `r"…"` (not the tail of an identifier).
            b'r' if (i == start || !is_ident_byte(bytes[i - 1]))
                && string_open(bytes, i).is_some() =>
            {
                i = nested_string(source, i, nesting, file)?
            }
            _ => i += 1,
        }
    }
}

/// Skip a string literal nested in an interpolation hole. A hole closes on the
/// line it opens on, so a multi-line string cannot sit inside one (#2619).
fn nested_string(
    source: &str,
    at: usize,
    nesting: usize,
    file: FileId,
) -> Result<usize, CompileError> {
    if string_open(source.as_bytes(), at).is_some_and(|o| o.multi) {
        return Err(CompileError::new(
            "bynk.lex.multiline_in_hole",
            Span::new_in(file, at, at + 3),
            "a multi-line string cannot appear inside an interpolation hole",
        )
        .with_note(
            "an interpolation hole `\\(…)` closes on the line it opens on; \
             bind the multi-line text to a name and interpolate that",
        ));
    }
    Ok(lex_string(source, at, nesting + 1, file)?.0)
}

/// The bounded-depth diagnostic for interpolation that nests past
/// [`crate::MAX_NESTING_DEPTH`]. `\("\("\(…` mutually recurses
/// [`lex_string`] ↔ [`scan_hole`], one stack frame per level, so an unbounded
/// scanner overflows and aborts `tokenize` (#713). `span` anchors the report on
/// the opener that tipped over the limit (the `"` or the `\(`).
fn too_deeply_nested_interpolation(span: Span) -> CompileError {
    CompileError::new(
        "bynk.lex.interpolation_too_deep",
        span,
        format!(
            "string interpolation nests more than {} levels deep",
            crate::MAX_NESTING_DEPTH
        ),
    )
    .with_note(
        "deeply nested `\\(…)` interpolation is rejected to keep the lexer from \
         overflowing its stack and aborting; flatten or split the string",
    )
}

/// One segment of a split interpolated string (v0.43): literal text (escapes
/// resolved) or the absolute source span of a hole's expression (the bytes
/// between `\(` and its matching `)`). The parser turns the latter into a real
/// `Expr`; the lexer owns only the scanning.
pub(crate) enum InterpSegment {
    Chunk(String),
    Hole(Span),
}

/// Split a string token (its `span` covers the whole lexeme) into chunks and
/// hole spans. Escapes in the chunks are resolved by [`lex_string`], the one
/// decoder every form shares; holes are returned as spans for the parser to
/// re-lex and parse as expressions. (v0.43, #2619.)
pub(crate) fn split_interp(source: &str, span: Span) -> Result<Vec<InterpSegment>, CompileError> {
    lex_string(source, span.start, 0, span.file).map(|(_, segments)| segments)
}

/// A highlighted part of a string literal (#2619), for the semantic tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringPart {
    /// An escape sequence (`\n`, `\u{1F600}`, …).
    Escape,
    /// A raw literal's delimiter (`r#"` or `"#`), which the TextMate grammar
    /// cannot tell from an identifier `r` and a quote.
    RawDelimiter,
}

/// The escape sequences and raw delimiters of every string literal in `source`
/// (including those inside interpolation holes), for the editor's semantic
/// tokens. Empty when the file does not lex.
pub fn string_parts(source: &str) -> Vec<(Span, StringPart)> {
    let mut out = Vec::new();
    if let Ok(tokens) = tokenize(source) {
        for tok in tokens {
            string_parts_of(source, tok, &mut out);
        }
    }
    out
}

/// [`string_parts`] for one token, recursing into the holes of an
/// interpolated string (each hole re-lexed and rebased to absolute offsets).
fn string_parts_of(source: &str, tok: Token, out: &mut Vec<(Span, StringPart)>) {
    if !matches!(tok.kind, TokenKind::StrLit | TokenKind::InterpStr) {
        return;
    }
    let bytes = source.as_bytes();
    let file = tok.span.file;
    let Some(open) = string_open(bytes, tok.span.start) else {
        return;
    };
    if open.raw {
        // `r#"` and the closing `"#`; a multi-line raw literal's `r"""` /
        // `"""` likewise.
        let close = if open.multi { 3 } else { 1 + open.hashes };
        out.push((
            Span::new_in(file, tok.span.start, open.body),
            StringPart::RawDelimiter,
        ));
        out.push((
            Span::new_in(file, tok.span.end - close, tok.span.end),
            StringPart::RawDelimiter,
        ));
        return;
    }
    let Ok(segments) = split_interp(source, tok.span) else {
        return;
    };
    let holes: Vec<Span> = segments
        .iter()
        .filter_map(|s| match s {
            InterpSegment::Hole(h) => Some(*h),
            InterpSegment::Chunk(_) => None,
        })
        .collect();
    let mut i = open.body;
    let end = tok.span.end - if open.multi { 3 } else { 1 };
    while i < end {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        if let Some(hole) = holes.iter().find(|h| h.start == i + 2) {
            i = hole.end + 1; // past the hole's `)`
            continue;
        }
        let len = if bytes.get(i + 1) == Some(&b'u') {
            bytes[i..end]
                .iter()
                .position(|&b| b == b'}')
                .map_or(2, |n| n + 1)
        } else {
            2
        };
        out.push((Span::new_in(file, i, i + len), StringPart::Escape));
        i += len;
    }
    for hole in holes {
        let Ok(hole_tokens) = tokenize_in(&source[hole.range()], file) else {
            continue;
        };
        for mut t in hole_tokens {
            t.span = Span::new_in(file, t.span.start + hole.start, t.span.end + hole.start);
            string_parts_of(source, t, out);
        }
    }
}

/// The value of a hole-free string lexeme (any form), for callers that hold the
/// lexeme rather than the surrounding source. `span` locates the lexeme in the
/// file so a (normally unreachable — the lexer already validated it) error
/// points at the right place.
pub fn decode_string_literal(lexeme: &str, span: Span) -> Result<String, CompileError> {
    let (_, segments) =
        lex_string(lexeme, 0, 0, span.file).map_err(|e| e.offset_spans(span.start))?;
    let mut out = String::with_capacity(lexeme.len());
    for segment in segments {
        if let InterpSegment::Chunk(text) = segment {
            out.push_str(&text);
        }
    }
    Ok(out)
}

/// If a `---` doc-block marker line starts at or shortly after `pos` (which
/// must be at a line boundary), return the byte offset just past the marker
/// line (after the terminating newline, or at EOF), and the marker's dash
/// count (three or more). The doc-block grammar requires the marker to be
/// alone on its line; leading horizontal whitespace is allowed and ignored.
fn doc_block_open_at(source: &str, pos: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    if !at_line_start(source, pos) {
        return None;
    }
    // Skip leading horizontal whitespace.
    let mut i = pos;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i + 3 > bytes.len() {
        return None;
    }
    if &bytes[i..i + 3] != b"---" {
        return None;
    }
    let dash_start = i;
    i += 3;
    // The marker may have additional trailing dashes (per spec "three or more
    // consecutive hyphens"). Consume them.
    while i < bytes.len() && bytes[i] == b'-' {
        i += 1;
    }
    let dashes = i - dash_start;
    // After the dashes, allow only horizontal whitespace then newline/EOF.
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\r') {
        i += 1;
    }
    if i == bytes.len() {
        return Some((i, dashes));
    }
    if bytes[i] == b'\n' {
        return Some((i + 1, dashes));
    }
    None
}

/// Find the next closing marker line at or after `pos`: a marker of exactly
/// `dashes` dashes, the opener's count (#1885), as with a Markdown code fence.
/// A marker line of any other length is content, so a longer fence can hold a
/// shorter one. Returns `(start_of_line, end_of_line)` (`end_of_line` is just
/// past the terminating newline, or at EOF).
fn doc_block_close(source: &str, mut pos: usize, dashes: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    while pos < bytes.len() {
        // Advance pos to the start of a line.
        let line_start = pos;
        // Find the end of this line.
        let mut line_end = line_start;
        while line_end < bytes.len() && bytes[line_end] != b'\n' {
            line_end += 1;
        }
        // Check this line.
        if let Some((end, n)) = doc_block_open_at(source, line_start)
            && n == dashes
        {
            return Some((line_start, end));
        }
        // Move to the next line.
        pos = if line_end < bytes.len() {
            line_end + 1
        } else {
            line_end
        };
    }
    None
}

/// Returns true if byte offset `pos` is at a line start (column 0).
/// #1888: true when only horizontal whitespace precedes `pos` on its line.
fn only_indent_before(bytes: &[u8], pos: usize) -> bool {
    bytes[..pos]
        .iter()
        .rev()
        .take_while(|&&b| b != b'\n')
        .all(|&b| matches!(b, b' ' | b'\t' | b'\r'))
}

/// #2203: a `--- text` one-line doc starts at `pos`: three or more dashes, a
/// space or tab, then something other than whitespace on the line. A marker
/// with only whitespace after it is a doc-block fence, which
/// [`doc_block_open_at`] has already taken.
fn doc_line_marker(bytes: &[u8], pos: usize) -> bool {
    let dashes = bytes[pos..].iter().take_while(|&&b| b == b'-').count();
    if dashes < 3 || !matches!(bytes.get(pos + dashes), Some(b' ' | b'\t')) {
        return false;
    }
    bytes[pos + dashes..]
        .iter()
        .take_while(|&&b| b != b'\n')
        .any(|b| !b.is_ascii_whitespace())
}

/// Push a [`TokenKind::DocLine`] from `start` through the end of its line,
/// newline included, and return the position after it.
fn push_doc_line(tokens: &mut Vec<Token>, bytes: &[u8], start: usize, file: FileId) -> usize {
    let mut pos = start;
    while pos < bytes.len() && bytes[pos] != b'\n' {
        pos += 1;
    }
    if pos < bytes.len() {
        pos += 1;
    }
    tokens.push(Token {
        kind: TokenKind::DocLine,
        span: Span::new_in(file, start, pos),
    });
    pos
}

/// Push a [`TokenKind::Comment`] from `start` up to (not including) the end of
/// its line, and return the position after it.
fn push_comment(tokens: &mut Vec<Token>, bytes: &[u8], start: usize, file: FileId) -> usize {
    let mut pos = start;
    while pos < bytes.len() && bytes[pos] != b'\n' {
        pos += 1;
    }
    tokens.push(Token {
        kind: TokenKind::Comment,
        span: Span::new_in(file, start, pos),
    });
    pos
}

/// #2203: the `bynk.lex.slash_doc` error for a `///` at `pos`, with the fix to
/// `---` (and a separating space when text follows straight on).
fn slash_doc_error(bytes: &[u8], pos: usize, file: FileId) -> CompileError {
    let marker = Span::new_in(file, pos, pos + 3);
    let fix = match bytes.get(pos + 3) {
        None | Some(b' ' | b'\t' | b'\r' | b'\n') => "---",
        Some(_) => "--- ",
    };
    CompileError::new(
        "bynk.lex.slash_doc",
        marker,
        "`///` is not a doc comment in Bynk; a doc is written with dashes",
    )
    .with_note(
        "a one-line doc is `--- text`, and a longer doc is a block between two `---` lines; \
         a comment is `//`",
    )
    .with_suggestion(
        "write `---`",
        vec![(marker, fix.to_string())],
        Applicability::MachineApplicable,
    )
}

/// The text of a [`TokenKind::DocLine`] token: everything after the marker,
/// less one separating space or tab, less a closer (#2203: a final run of
/// exactly the opener's dashes, preceded by whitespace, with that whitespace),
/// and less the line ending. Further indentation is kept, so a doc line can
/// indent a Markdown code block. A legacy `--|` line (#1888) has no closer.
pub fn doc_line_text(source: &str, span: Span) -> &str {
    let slice = source[span.range()].trim_end_matches(['\n', '\r']);
    if let Some(rest) = slice.strip_prefix("--|") {
        return rest.strip_prefix(' ').unwrap_or(rest);
    }
    let dashes = slice.bytes().take_while(|&b| b == b'-').count();
    let rest = &slice[dashes..];
    let rest = rest.strip_prefix([' ', '\t']).unwrap_or(rest);
    let body = rest.trim_end_matches([' ', '\t']);
    let run = body.len() - body.trim_end_matches('-').len();
    if run == dashes {
        let before = &body[..body.len() - run];
        if before.is_empty() || before.ends_with([' ', '\t']) {
            return before.trim_end_matches([' ', '\t']);
        }
    }
    rest
}

/// #2203: the source spelling of a one-line doc whose text is `text`: `--- `
/// and the text, or a `----` marker when the text ends in a whitespace-preceded
/// run of three dashes (which a `---` marker would read as its closer). Empty
/// text is spelled with a closer, `--- ---`, since a bare `---` opens a block.
pub fn one_line_doc(text: &str) -> String {
    let body = text.trim_end_matches([' ', '\t']);
    if body.trim().is_empty() {
        return "--- ---".to_string();
    }
    let run = body.len() - body.trim_end_matches('-').len();
    let before = &body[..body.len() - run];
    let closer_shaped = run == 3 && (before.is_empty() || before.ends_with([' ', '\t']));
    let marker = if closer_shaped { "----" } else { "---" };
    format!("{marker} {text}")
}

/// #1885: the dash count of the fence a doc block holding `doc` is printed
/// with: one more than the longest marker-shaped line in it (a line of three
/// or more dashes alone, surrounding whitespace aside), and never fewer than
/// three. A shorter fence would close at that line.
pub fn doc_fence_len(doc: &str) -> usize {
    doc.lines()
        .map(str::trim)
        .filter(|l| l.len() >= 3 && l.bytes().all(|b| b == b'-'))
        .map(|l| l.len() + 1)
        .fold(3, usize::max)
}

/// #2203: whether a [`TokenKind::DocLine`] token is spelled the legacy way,
/// `--|` (#1888), rather than `--- text`.
pub fn doc_line_is_legacy(source: &str, span: Span) -> bool {
    source[span.range()].starts_with("--|")
}

/// #2203: whether a [`TokenKind::Comment`] token is spelled the legacy way,
/// `--`, rather than `//`.
pub fn comment_is_legacy(source: &str, span: Span) -> bool {
    source[span.range()].starts_with("--")
}

/// #2203 (DECISION E): the `bynk.lex.legacy_comment` error for the legacy `--`
/// comment at `span` (a [`TokenKind::Comment`] token), with the fix to `//`.
pub fn legacy_comment_error(span: Span) -> CompileError {
    let marker = Span::new_in(span.file, span.start, span.start + 2);
    CompileError::new(
        "bynk.lex.legacy_comment",
        marker,
        "`--` no longer starts a comment; a line comment is `//`",
    )
    .with_note(
        "`bynk fmt` rewrites every legacy comment; for subtraction of a negation, write `a - -b`",
    )
    .with_suggestion(
        "write `//`",
        vec![(marker, "//".to_string())],
        Applicability::MachineApplicable,
    )
}

/// #2203 (DECISION E): the `bynk.lex.legacy_doc_line` error for the legacy
/// `--|` doc line at `span` (a [`TokenKind::DocLine`] token), with the fix to
/// `--- text`. A `--|` with no text has no one-line spelling (a bare `---`
/// opens a block), so it gets no fix.
pub fn legacy_doc_line_error(source: &str, span: Span) -> CompileError {
    let marker = Span::new_in(span.file, span.start, span.start + 3);
    let err = CompileError::new(
        "bynk.lex.legacy_doc_line",
        marker,
        "`--|` no longer starts a doc; a one-line doc is `--- text`",
    );
    let text = doc_line_text(source, span);
    if text.trim().is_empty() {
        return err.with_note(
            "a one-line doc holds text; a doc of more than one paragraph is a block between \
             two `---` lines",
        );
    }
    let line = Span::new_in(span.file, span.start, doc_line_text_range(source, span).end);
    err.with_note("`bynk fmt` rewrites every legacy doc line")
        .with_suggestion(
            "write `--- `",
            vec![(line, one_line_doc(text))],
            Applicability::MachineApplicable,
        )
}

/// #1888: the byte range of a doc line's text within `source`, as
/// [`doc_line_text`] returns it, for callers that map positions in a doc
/// back to the source (document links).
pub fn doc_line_text_range(source: &str, span: Span) -> std::ops::Range<usize> {
    let text = doc_line_text(source, span);
    let start = text.as_ptr() as usize - source.as_ptr() as usize;
    start..start + text.len()
}

fn at_line_start(source: &str, pos: usize) -> bool {
    if pos == 0 {
        return true;
    }
    let bytes = source.as_bytes();
    bytes[pos - 1] == b'\n'
}

/// The doc-block body as a byte range into `source` — leading/trailing `---`
/// marker lines stripped, no further processing (unlike [`doc_block_content`],
/// which additionally strips a common per-line indent — not offset-preserving).
/// Callers that need to map a position in the body back to `source` (e.g.
/// document-link spans) use this instead of re-deriving it from the string
/// `doc_block_content` returns.
pub fn doc_block_body_range(source: &str, span: Span) -> Option<std::ops::Range<usize>> {
    let slice = &source[span.range()];
    // Drop the first line (opening marker).
    let after_open_rel = slice.find('\n')? + 1;
    let after_open = &slice[after_open_rel..];
    let bytes = after_open.as_bytes();
    // Trim the trailing closing-marker line.
    let mut i = bytes.len();
    if i > 0 && bytes[i - 1] == b'\n' {
        i -= 1;
    }
    while i > 0 && matches!(bytes[i - 1], b' ' | b'\t' | b'\r') {
        i -= 1;
    }
    while i > 0 && bytes[i - 1] == b'-' {
        i -= 1;
    }
    if i > 0 && bytes[i - 1] == b'\n' {
        i -= 1;
    }
    let start = span.range().start + after_open_rel;
    Some(start..start + i)
}

/// Extract the body content of a doc-block token from its source span.
/// Strips the leading and trailing `---` marker lines and returns the body
/// verbatim. If every non-empty content line begins with the same horizontal
/// whitespace prefix (e.g., because the doc block sits inside a brace-form
/// commons body), that common prefix is removed so the body reads naturally
/// when emitted as JSDoc.
pub fn doc_block_content(source: &str, span: Span) -> String {
    let Some(range) = doc_block_body_range(source, span) else {
        return String::new();
    };
    let body = &source[range];

    // Compute the common leading-whitespace prefix across all non-empty lines
    // and strip it. This lets writers indent the doc block alongside the
    // declaration it documents without bleeding the indent into the JSDoc.
    let common: Option<usize> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.bytes().take_while(|&b| b == b' ' || b == b'\t').count())
        .min();
    let strip = common.unwrap_or(0);
    if strip == 0 {
        return body.to_string();
    }
    let mut out = String::with_capacity(body.len());
    let mut first = true;
    for line in body.lines() {
        if !first {
            out.push('\n');
        }
        first = false;
        if line.trim().is_empty() {
            // Preserve blank lines.
            continue;
        }
        let leading: usize = line
            .bytes()
            .take_while(|&b| b == b' ' || b == b'\t')
            .count();
        let drop = strip.min(leading);
        out.push_str(&line[drop..]);
    }
    out
}

/// Extract the body of a `Comment` trivia token: everything after the
/// leading `//` marker, preserving its inline whitespace verbatim. Used by
/// the parser when attaching comments to declarations.
///
/// #2203: a legacy `--` comment's body is what follows its `--`, so the
/// formatter prints it after `//`. A legacy body that starts with a dash (a
/// `--- Helpers ---` divider, read by [`tokenize_legacy`]) keeps all of its
/// dashes behind a space instead, as `// --- Helpers ---`, and a legacy
/// `--|` after code (#1888) drops its `|`.
pub fn comment_body(source: &str, span: Span) -> std::borrow::Cow<'_, str> {
    let slice = &source[span.range()];
    if let Some(rest) = slice.strip_prefix("//") {
        return rest.into();
    }
    match slice.strip_prefix("--") {
        Some(rest) if rest.starts_with('-') => format!(" {slice}").into(),
        // A `--|` after code was a comment (#1888); its `|` was not text.
        Some(rest) => rest.strip_prefix('|').unwrap_or(rest).into(),
        None => slice.into(),
    }
}

/// Returns true if there is a blank line (a line containing only whitespace)
/// in `source` strictly between byte offsets `from` (inclusive) and `to`
/// (exclusive). Used by the parser to detect orphan doc blocks.
///
/// A doc-block token's span ends just past the closing-marker line's
/// terminating newline. So if the next declaration begins on the immediately
/// following line, the substring between contains no newline (only optional
/// indentation). Any newline in the substring therefore implies at least one
/// entirely-blank line separating the doc from the declaration.
pub fn has_blank_line_between(source: &str, from: usize, to: usize) -> bool {
    if to <= from {
        return false;
    }
    let bytes = source.as_bytes();
    let mut i = from;
    while i < to {
        if bytes[i] == b'\n' {
            return true;
        }
        if !matches!(bytes[i], b' ' | b'\t' | b'\r') {
            return false;
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        tokenize(source)
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn keywords_and_idents() {
        use TokenKind::*;
        assert_eq!(
            kinds("commons type fn where true false Int String Bool foo bar"),
            vec![
                Commons, Type, Fn, Where, True, False, Int, String, Bool, Ident, Ident
            ],
        );
        // #548: `and` is no longer a keyword — it lexes as an ordinary identifier.
        assert_eq!(kinds("and"), vec![Ident]);
    }

    #[test]
    fn deeply_nested_interpolation_is_bounded_not_overflowed() {
        // `"\("\("\(…` mutually recurses lex_string <-> scan_hole, one frame per
        // level, and an unbounded scanner overflows `tokenize` and aborts the
        // process (#713). Well past the limit it must return a bounded-depth
        // diagnostic instead. The holes are left open so the depth guard, not a
        // later `)`, stops the scan.
        let depth = crate::MAX_NESTING_DEPTH + 8;
        let src = format!("\"{}", "\\(\"".repeat(depth));
        let err = tokenize(&src).unwrap_err();
        assert_eq!(err.category, "bynk.lex.interpolation_too_deep");
    }

    #[test]
    fn integer_and_string_literals() {
        use TokenKind::*;
        assert_eq!(
            kinds(r#"0 42 "hello" "with\nescape""#),
            vec![IntLit, IntLit, StrLit, StrLit]
        );
    }

    #[test]
    fn operators() {
        use TokenKind::*;
        assert_eq!(
            kinds("-> == != <= >= && || + - * / ! = < > ( ) { } [ ] , : . @"),
            vec![
                Arrow, EqEq, BangEq, LtEq, GtEq, AmpAmp, PipePipe, Plus, Minus, Star, Slash, Bang,
                Eq, Lt, Gt, LParen, RParen, LBrace, RBrace, LBracket, RBracket, Comma, Colon, Dot,
                At,
            ],
        );
    }

    #[test]
    fn dot_family_maximal_munch() {
        // Events track slice 1 (spine #936): `..` must lex as one `DotDot`
        // token, not two `Dot`s — a real token keeps agreement with
        // tree-sitter (which declares `".."` as one literal), so a
        // whitespace-split `. .` cannot silently parse where a real `..`
        // is required. Also confirms `...`/`..`/`.` don't shadow each other
        // regardless of declaration order (logos maximal-munch).
        use TokenKind::*;
        assert_eq!(
            kinds("a .. b ... c . d . ."),
            vec![Ident, DotDot, Ident, DotDotDot, Ident, Dot, Ident, Dot, Dot,],
        );
    }

    #[test]
    fn line_comments_emitted_as_trivia() {
        // v1.1: line comments are preserved as Comment tokens so the
        // formatter can attach and re-emit them.
        use TokenKind::*;
        let src = "// a comment\ntype X = Int // trailing\n";
        assert_eq!(kinds(src), vec![Comment, Type, Ident, Eq, Int, Comment],);
    }

    #[test]
    fn comment_body_extracts_text_after_marker() {
        let toks = tokenize("// hello world\n").unwrap();
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].kind, TokenKind::Comment);
        assert_eq!(
            comment_body("// hello world\n", toks[0].span),
            " hello world"
        );
    }

    #[test]
    fn comment_does_not_consume_newline() {
        // Two adjacent comment lines should produce two distinct tokens
        // — the newline between them is not part of either comment's span.
        let toks = tokenize("// one\n// two\n").unwrap();
        assert_eq!(toks.len(), 2);
        assert!(toks.iter().all(|t| t.kind == TokenKind::Comment));
    }

    /// #2203: `//` is a comment anywhere, with no whitespace rule, and `--` is
    /// two `-` operators unless it is spelled like a legacy comment.
    #[test]
    fn slash_slash_is_a_comment_and_dash_dash_is_subtraction() {
        use TokenKind::*;
        assert_eq!(kinds("a//b"), vec![Ident, Comment]);
        assert_eq!(kinds("a--b"), vec![Ident, Minus, Minus, Ident]);
        assert_eq!(kinds("x--\ny"), vec![Ident, Minus, Minus, Ident]);
        assert_eq!(kinds("x--"), vec![Ident, Minus, Minus]);
        assert_eq!(kinds("a - -b"), vec![Ident, Minus, Minus, Ident]);
        assert_eq!(kinds("a --b"), vec![Ident, Minus, Minus, Ident]);
        // A `////` ruler is a comment, as in Rust.
        assert_eq!(kinds("////////\n"), vec![Comment]);
        assert_eq!(kinds("// -------\n"), vec![Comment]);
    }

    /// #2203 (DECISION E): a `--` at the start of a line or after whitespace,
    /// followed by a space or the end of the line, lexes as a comment still,
    /// spelled the legacy way, so it is never read as `- -`.
    #[test]
    fn a_legacy_dash_dash_comment_still_lexes_as_a_comment() {
        use TokenKind::*;
        assert_eq!(kinds("a -- c"), vec![Ident, Comment]);
        assert_eq!(kinds("-- c"), vec![Comment]);
        assert_eq!(kinds("a\n--\nb"), vec![Ident, Comment, Ident]);
        assert_eq!(kinds("x --"), vec![Ident, Comment]);
        assert_eq!(
            kinds("a: Int, --| note"),
            vec![Ident, Colon, Int, Comma, Comment]
        );
        let src = "x = 1 -- note\n";
        let toks = tokenize(src).unwrap();
        let c = toks.last().unwrap().span;
        assert!(comment_is_legacy(src, c));
        assert_eq!(comment_body(src, c), " note");
        let err = legacy_comment_error(c);
        assert_eq!(err.category, "bynk.lex.legacy_comment");
        assert_eq!(err.suggestions[0].edits[0].1, "//");
        assert_eq!(&src[err.suggestions[0].edits[0].0.range()], "--");
    }

    /// #2203 (DECISION C): `--- text` first on its line is a one-line doc; its
    /// closer is a final run of exactly the opener's dashes after whitespace.
    #[test]
    fn a_dash_marker_with_text_is_a_doc_line() {
        use TokenKind::*;
        assert_eq!(kinds("--- doc\ntype"), vec![DocLine, Type]);
        assert_eq!(kinds("  ---- doc\n\t--- x"), vec![DocLine, DocLine]);
        let text = |src: &str| {
            let toks = tokenize(src).unwrap();
            assert_eq!(toks[0].kind, DocLine, "{src}");
            doc_line_text(src, toks[0].span).to_string()
        };
        assert_eq!(text("--- The repo.\n"), "The repo.");
        assert_eq!(text("--- The repo. ---\n"), "The repo.");
        assert_eq!(text("---- The repo. ----\r\n"), "The repo.");
        // A trailing run of another length is text.
        assert_eq!(text("--- a -- b ----\n"), "a -- b ----");
        assert_eq!(text("---- a ---\n"), "a ---");
        assert_eq!(text("--- a---\n"), "a---");
        // Further indentation is kept.
        assert_eq!(text("---   indented\n"), "  indented");
        // A marker alone is a block fence, and a `---` with no space after it
        // is not a doc.
        assert_eq!(kinds("---\nx\n---\n"), vec![DocBlock]);
        assert_eq!(kinds("---x\n"), vec![Minus, Minus, Minus, Ident]);
        // After code, `---` is operators (the parser rejects it).
        assert_eq!(kinds("a --- b"), vec![Ident, Minus, Minus, Minus, Ident]);
        let src = "  --- A doc.  \nx";
        let toks = tokenize(src).unwrap();
        assert_eq!(&src[doc_line_text_range(src, toks[0].span)], "A doc.  ");
    }

    /// #2203: the one-line spelling of a doc's text never reads its own end as
    /// a closer.
    #[test]
    fn one_line_doc_spelling_round_trips() {
        for text in [
            "The repo.",
            "a ---",
            "---",
            "----",
            "a -- b",
            "  indented",
            "",
        ] {
            let src = format!("{}\n", one_line_doc(text));
            let toks = tokenize(&src).unwrap();
            assert_eq!(toks[0].kind, TokenKind::DocLine, "{src}");
            assert_eq!(doc_line_text(&src, toks[0].span), text, "{src}");
        }
    }

    /// #1888's `--|` first on its line is a doc line still, spelled the legacy
    /// way; its fix is `--- text`.
    #[test]
    fn a_legacy_bar_doc_line_still_lexes_as_a_doc_line() {
        use TokenKind::{DocLine, Type};
        assert_eq!(kinds("--| doc\ntype"), vec![DocLine, Type]);
        assert_eq!(kinds("  --| doc\n\t--|\n"), vec![DocLine, DocLine]);
        let src = "  --|  indented  \r\n--|bare\n--|";
        let toks = tokenize(src).unwrap();
        assert!(doc_line_is_legacy(src, toks[0].span));
        assert_eq!(&src[toks[0].span.range()], "--|  indented  \r\n");
        assert_eq!(doc_line_text(src, toks[0].span), " indented  ");
        assert_eq!(doc_line_text(src, toks[1].span), "bare");
        assert_eq!(doc_line_text(src, toks[2].span), "");
        let fix = |i: usize| {
            let err = legacy_doc_line_error(src, toks[i].span);
            assert_eq!(err.category, "bynk.lex.legacy_doc_line");
            err.suggestions
                .first()
                .map(|s| (src[s.edits[0].0.range()].to_string(), s.edits[0].1.clone()))
        };
        assert_eq!(
            fix(1),
            Some(("--|bare".to_string(), "--- bare".to_string()))
        );
        assert_eq!(fix(2), None, "an empty `--|` has no one-line spelling");
    }

    /// #2203: `///` first on its line is `bynk.lex.slash_doc`, fixed to `---`.
    #[test]
    fn a_slash_doc_is_an_error_with_a_fix() {
        for (src, fix) in [
            ("/// A doc.\n", "---"),
            ("///A doc.\n", "--- "),
            ("  ///", "---"),
        ] {
            let err = tokenize(src).unwrap_err();
            assert_eq!(err.category, "bynk.lex.slash_doc");
            let (span, text) = &err.suggestions[0].edits[0];
            assert_eq!(&src[span.range()], "///");
            assert_eq!(text, fix);
        }
    }

    /// #2203: a legacy read takes ADR 0188 D1's rule, so a `--- Helpers ---`
    /// divider and a `--note` are comments, as they were before `//`.
    #[test]
    fn a_legacy_read_keeps_the_old_comment_rule() {
        let src = "--- Helpers ---\nx --note\na--b\n--| doc\n// new\n";
        let toks = tokenize_legacy(src).unwrap();
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        use TokenKind::*;
        assert_eq!(
            kinds,
            vec![
                Comment, Ident, Comment, Ident, Minus, Minus, Ident, DocLine, Comment
            ]
        );
        assert_eq!(comment_body(src, toks[0].span), " --- Helpers ---");
        assert_eq!(comment_body(src, toks[2].span), "note");
    }

    #[test]
    fn unterminated_string_is_error() {
        let err = tokenize("\"oops\n").unwrap_err();
        assert_eq!(err.category, "bynk.lex.unterminated_string");
    }

    #[test]
    fn integer_overflow_is_error() {
        let err = tokenize("99999999999999999999").unwrap_err();
        assert_eq!(err.category, "bynk.lex.integer_overflow");
    }

    #[test]
    fn an_int_literal_must_be_a_safe_integer() {
        // #1657: the `Int` range is ±(2^53 − 1). The largest safe integer lexes;
        // one more, and i64-range values above it, do not.
        assert!(tokenize("9007199254740991").is_ok());
        assert!(tokenize("9_007_199_254_740_991").is_ok());
        for over in [
            "9007199254740992",
            "9007199254740993",
            "9223372036854775807",
        ] {
            let err = tokenize(over).unwrap_err();
            assert_eq!(err.category, "bynk.lex.integer_overflow", "{over}");
        }
    }

    #[test]
    fn range_bounds_never_lex_as_a_float() {
        use TokenKind::*;
        // #2699: a float needs a digit on both sides of its `.`, so every range
        // spelling is `IntLit DotDot IntLit`, with the sign a separate `Minus`.
        assert_eq!(kinds("0..9"), vec![IntLit, DotDot, IntLit]);
        assert_eq!(kinds("18.."), vec![IntLit, DotDot]);
        assert_eq!(kinds("..-1"), vec![DotDot, Minus, IntLit]);
        assert_eq!(kinds("-5..-1"), vec![Minus, IntLit, DotDot, Minus, IntLit]);
        assert_eq!(kinds("1_000..2_000"), vec![IntLit, DotDot, IntLit]);
    }

    #[test]
    fn a_decimal_literal_is_one_token_with_a_d_suffix() {
        use TokenKind::*;
        // #2622: digits, an optional fraction, a trailing `d`.
        assert_eq!(kinds("1.20d"), vec![DecimalLit]);
        assert_eq!(kinds("5d"), vec![DecimalLit]);
        assert_eq!(kinds("1_000.50d"), vec![DecimalLit]);
        // No `d` is still an `Int` or a `Float`, and the other numeric forms are
        // unchanged (a duration unit, a method on a literal, a range).
        assert_eq!(kinds("1.20"), vec![FloatLit]);
        assert_eq!(kinds("5.days"), vec![IntLit, Dot, Ident]);
        assert_eq!(
            kinds("2.5.round()"),
            vec![FloatLit, Dot, Ident, LParen, RParen]
        );
        assert_eq!(kinds("0..9"), vec![IntLit, DotDot, IntLit]);
        // The sign stays a separate operator; there is no exponent form, so an
        // exponent is a float followed by an identifier.
        assert_eq!(kinds("-1.5d"), vec![Minus, DecimalLit]);
        assert_eq!(kinds("1e5d"), vec![FloatLit, Ident]);
        // `Decimal` is a keyword.
        assert_eq!(kinds("Decimal"), vec![Decimal]);
    }

    #[test]
    fn a_decimal_literal_obeys_the_decimal_limits() {
        let frac = |n: usize| format!("0.{}d", "1".repeat(n));
        assert!(tokenize(&frac(100)).is_ok());
        assert!(tokenize(&frac(101)).is_err());
        assert!(tokenize(&format!("{}d", "9".repeat(1000))).is_ok());
        assert!(tokenize(&format!("{}d", "9".repeat(1001))).is_err());
    }

    #[test]
    fn digit_separators_lex_as_one_number() {
        use TokenKind::*;
        // v0.142 (ADR 0166): `_` between digit groups keeps the literal a single
        // token for both Int and Float.
        assert_eq!(kinds("1_048_576"), vec![IntLit]);
        assert_eq!(kinds("1_000.500_5"), vec![FloatLit]);
        assert_eq!(kinds("1_000e1_0"), vec![FloatLit]);
        // A separator-carrying literal that is in range still lexes (the value is
        // validated after stripping the separators).
        assert!(tokenize("9_007_199_254_740_991").is_ok());
        // Overflow is still caught on the separator-free value.
        let err = tokenize("9_999_999_999_999_999_999_9").unwrap_err();
        assert_eq!(err.category, "bynk.lex.integer_overflow");
    }

    #[test]
    fn strip_digit_separators_removes_underscores() {
        assert_eq!(strip_digit_separators("1_048_576"), "1048576");
        assert_eq!(strip_digit_separators("42"), "42");
    }

    #[test]
    fn unexpected_character_is_error() {
        let err = tokenize("type X = Int $").unwrap_err();
        assert_eq!(err.category, "bynk.lex.unexpected_character");
    }

    #[test]
    fn v0_1_keywords() {
        use TokenKind::*;
        assert_eq!(
            kinds("let if else Ok Err Result ValidationError"),
            vec![Let, If, Else, Ok, Err, Result, ValidationError],
        );
    }

    #[test]
    fn question_token() {
        use TokenKind::*;
        assert_eq!(kinds("x?"), vec![Ident, Question]);
    }

    #[test]
    fn v0_2_keywords() {
        use TokenKind::*;
        assert_eq!(
            kinds("enum match Option record self Some None is"),
            vec![Enum, Match, Option, Record, Self_, Some, None, Is],
        );
    }

    #[test]
    fn pipe_and_pipe_pipe_disambiguated() {
        use TokenKind::*;
        assert_eq!(kinds("| || |"), vec![Pipe, PipePipe, Pipe]);
    }

    #[test]
    fn v0_7_keywords() {
        use TokenKind::*;
        assert_eq!(kinds("expect suite case"), vec![Expect, Suite, Case],);
        // v0.118: `mocks` and `wires` are retired — plain identifiers now.
        assert_eq!(kinds("mocks wires"), vec![Ident, Ident]);
    }

    #[test]
    fn fat_arrow_and_underscore() {
        use TokenKind::*;
        assert_eq!(kinds("_ =>"), vec![Underscore, FatArrow]);
    }

    // -- v0.43 string interpolation --

    #[test]
    fn interp_string_is_one_token() {
        use TokenKind::*;
        assert_eq!(kinds(r#""Hello, \(name)!""#), vec![InterpStr]);
        // A plain string (no hole) stays a `StrLit`, via the logos path.
        assert_eq!(kinds(r#""Hello, world""#), vec![StrLit]);
    }

    #[test]
    fn interp_balances_nested_parens_and_strings() {
        use TokenKind::*;
        // The `)` inside `f(x)` must not close the hole early.
        assert_eq!(kinds(r#""= \(f(x))""#), vec![InterpStr]);
        // A `)` inside a nested string inside the hole is also ignored.
        assert_eq!(kinds(r#""= \(label(")"))""#), vec![InterpStr]);
        // A nested interpolated string inside a hole.
        assert_eq!(kinds(r#""out \("in \(x)")""#), vec![InterpStr]);
    }

    // Issue #473: hole-expanding tokenisation makes identifiers inside `\(…)`
    // visible to the LSP's token-based cursor resolution.
    #[test]
    fn expanding_holes_exposes_hole_identifiers() {
        use TokenKind::*;
        let expand = |src: &str| {
            tokenize_expanding_holes(src)
                .unwrap()
                .into_iter()
                .map(|t| t.kind)
                .collect::<Vec<_>>()
        };
        // The opaque `InterpStr` is replaced by its hole's tokens; the chunk
        // text (`Hello, ` / `!`) carries none.
        assert_eq!(expand(r#""Hello, \(name)!""#), vec![Ident]);
        // A call hole exposes every token of the call expression.
        assert_eq!(expand(r#""= \(f(x))""#), vec![Ident, LParen, Ident, RParen]);
        // Nested interpolation recurses to the innermost hole's identifier.
        assert_eq!(expand(r#""out \("in \(x)")""#), vec![Ident]);
        // A plain (hole-free) string is untouched.
        assert_eq!(expand(r#""Hello, world""#), vec![StrLit]);
    }

    #[test]
    fn expanding_holes_rebases_spans_to_absolute() {
        let src = r#""Hello, \(name)!""#;
        let toks = tokenize_expanding_holes(src).unwrap();
        let ident = toks
            .iter()
            .find(|t| t.kind == TokenKind::Ident)
            .expect("the hole identifier is exposed");
        // The span points at `name` in the original source, not a hole-local 0.
        assert_eq!(&src[ident.span.range()], "name");
        assert_eq!(ident.span.start, src.find("name").unwrap());
    }

    #[test]
    fn escaped_open_paren_is_not_a_hole() {
        use TokenKind::*;
        // `\\(` is a literal backslash followed by `(` — no hole, so the
        // string lexes as a plain `StrLit` on the logos path.
        assert_eq!(kinds(r#""a \\(b) c""#), vec![StrLit]);
    }

    #[test]
    fn unterminated_hole_is_an_error() {
        // The hole runs to end of line without its closing `)`.
        let err = tokenize("\"value \\(x + 1\n\"").unwrap_err();
        assert_eq!(err.category, "bynk.lex.unterminated_interpolation");
    }

    #[test]
    fn unterminated_interp_string_is_an_error() {
        // A hole closes but the string never does (newline before the `"`).
        let err = tokenize("\"value \\(x) more\n").unwrap_err();
        assert_eq!(err.category, "bynk.lex.unterminated_string");
    }

    #[test]
    fn bad_escape_in_interp_string_is_an_error() {
        let err = tokenize(r#""a \q \(x)""#).unwrap_err();
        assert_eq!(err.category, "bynk.lex.bad_escape");
    }

    /// The decoded value of the one string token in `src`.
    fn string_value(src: &str) -> String {
        let toks = tokenize(src).unwrap();
        assert_eq!(toks.len(), 1, "{src:?}");
        assert_eq!(toks[0].kind, TokenKind::StrLit, "{src:?}");
        decode_string_literal(&src[toks[0].span.range()], toks[0].span).unwrap()
    }

    /// #2619: `\r`, `\0` and `\u{…}` join the escape set.
    #[test]
    fn new_escapes_decode() {
        assert_eq!(string_value(r#""a\r\nb""#), "a\r\nb");
        assert_eq!(string_value(r#""\0""#), "\0");
        assert_eq!(string_value(r#""\u{1F600}""#), "\u{1F600}");
        assert_eq!(string_value(r#""a\u{00A0}b""#), "a\u{A0}b");
        assert_eq!(string_value(r#""\u{41}\u{000041}""#), "AA");
        assert_eq!(string_value(r#""\u{10FFFF}""#), "\u{10FFFF}");
    }

    #[test]
    fn bad_unicode_escapes_are_lexical_errors() {
        for src in [r#""\u{D800}""#, r#""\u{DFFF}""#, r#""\u{110000}""#] {
            assert_eq!(
                tokenize(src).unwrap_err().category,
                "bynk.lex.bad_unicode_escape",
                "{src}"
            );
        }
        for src in [
            r#""\u""#,
            r#""\u{}""#,
            r#""\u{1234567}""#,
            r#""\u{12""#,
            r#""\u{zz}""#,
        ] {
            assert_eq!(
                tokenize(src).unwrap_err().category,
                "bynk.lex.bad_escape",
                "{src}"
            );
        }
    }

    #[test]
    fn raw_strings_keep_backslashes_and_do_not_interpolate() {
        assert_eq!(string_value(r#"r"\d+\.\d+""#), "\\d+\\.\\d+");
        assert_eq!(string_value(r#"r"a\(b)""#), "a\\(b)");
        assert_eq!(string_value(r###"r#"say "hi""#"###), "say \"hi\"");
        assert_eq!(string_value(r#"r"""#), "");
        // `r` followed by a space is an identifier then a string.
        use TokenKind::*;
        assert_eq!(kinds(r#"r "x""#), vec![Ident, StrLit]);
        assert_eq!(kinds(r#"bar"x""#), vec![Ident, StrLit]);
        // A raw string is not terminated by a newline.
        assert_eq!(
            tokenize("r\"abc\n\"").unwrap_err().category,
            "bynk.lex.unterminated_string"
        );
    }

    #[test]
    fn multiline_strings_strip_the_closing_indentation() {
        let src = "\"\"\"\n    SELECT id\n      FROM t\n\n    \"\"\"";
        assert_eq!(string_value(src), "SELECT id\n  FROM t\n");
        // No indentation, and escapes still apply.
        assert_eq!(string_value("\"\"\"\na\\tb\n\"\"\""), "a\tb");
        // An empty body.
        assert_eq!(string_value("\"\"\"\n\"\"\""), "");
        // Quotes inside are fine; the raw form keeps backslashes.
        assert_eq!(
            string_value("\"\"\"\n  a \"q\" \"\"b\n  \"\"\""),
            "a \"q\" \"\"b"
        );
        assert_eq!(string_value("r\"\"\"\n  \\d+\n  \"\"\""), "\\d+");
    }

    /// Decision D: newlines are `\n` whatever the source's line endings.
    #[test]
    fn multiline_newlines_are_normalised() {
        assert_eq!(string_value("\"\"\"\r\n  a\r\n  b\r\n  \"\"\""), "a\nb");
    }

    #[test]
    fn multiline_with_a_hole_is_an_interpolated_string() {
        use TokenKind::*;
        assert_eq!(kinds("\"\"\"\n  hi \\(name)\n  \"\"\""), vec![InterpStr]);
    }

    /// Pinned by review of #2619: edge cases of the multi-line rules.
    #[test]
    fn multiline_edge_cases() {
        // A hole at the start of an indented line: the margin is stripped
        // first, never inside the hole.
        let src = "\"\"\"\n    \\(a)\n      \\(b) x\n    \"\"\"";
        let toks = tokenize(src).unwrap();
        assert_eq!(toks[0].kind, TokenKind::InterpStr);
        let segs = split_interp(src, toks[0].span).unwrap();
        let shape: Vec<String> = segs
            .iter()
            .map(|s| match s {
                InterpSegment::Chunk(c) => format!("{c:?}"),
                InterpSegment::Hole(h) => src[h.range()].to_string(),
            })
            .collect();
        assert_eq!(shape, ["a", "\"\\n  \"", "b", "\" x\""]);
        // A whitespace-only line shallower than the margin is empty.
        assert_eq!(
            string_value("\"\"\"\n    a\n  \n    b\n    \"\"\""),
            "a\n\nb"
        );
        // Mixed tabs and spaces that do not match the margin exactly.
        let err = tokenize("\"\"\"\n\t  a\n    \"\"\"").unwrap_err();
        assert_eq!(err.category, "bynk.lex.multiline_indent");
        assert!(err.notes[0].contains("character for character"));
        // CRLF inside a raw multi-line string; a lone CR is not a line break.
        assert_eq!(
            string_value("r\"\"\"\r\n  a\\\r\n  b\r\n  \"\"\""),
            "a\\\nb"
        );
        assert_eq!(string_value("\"\"\"\n  a\rb\n  \"\"\""), "a\rb");
        // The raw form holds `"` and `""` but not `"""`.
        assert_eq!(string_value("r\"\"\"\n  \"a\"\" b\n  \"\"\""), "\"a\"\" b");
        // Empty `\u{}` and an eight-digit escape.
        assert!(tokenize(r#""\u{}""#).is_err());
        assert!(tokenize(r#""\u{00000041}""#).is_err());
    }

    /// Review of #2619: a multi-line string cannot nest in a hole, and a
    /// `"""` followed by text is not a closing delimiter.
    #[test]
    fn multiline_in_hole_and_stray_delimiters() {
        let cat = |src: &str| tokenize(src).unwrap_err().category;
        assert_eq!(
            cat("\"a \\(f(\"\"\"\n  x\n  \"\"\")) b\""),
            "bynk.lex.multiline_in_hole"
        );
        assert_eq!(
            cat("\"\"\"\n  \\(g(\"\"\"\n  x\n  \"\"\"))\n  \"\"\""),
            "bynk.lex.multiline_in_hole"
        );
        // `"""quoted"""` on a body line is a stray delimiter, not a close.
        assert_eq!(
            cat("\"\"\"\n  \"\"\"quoted\"\"\"\n  \"\"\""),
            "bynk.lex.multiline_close_not_own_line"
        );
        // A closing delimiter may be followed by punctuation.
        let toks = tokenize("\"\"\"\n  a\n  \"\"\", x)").unwrap();
        assert_eq!(toks.len(), 4);
    }

    #[test]
    fn multiline_errors() {
        let cat = |src: &str| tokenize(src).unwrap_err().category;
        // Under-indented line.
        assert_eq!(
            cat("\"\"\"\n    a\n  b\n    \"\"\""),
            "bynk.lex.multiline_indent"
        );
        // Text after the opening delimiter.
        assert_eq!(cat("\"\"\"abc\n\"\"\""), "bynk.lex.multiline_open");
        // Never closed.
        assert_eq!(cat("\"\"\"\n  a\n"), "bynk.lex.unterminated_string");
        // A closing delimiter that shares a line with text.
        assert_eq!(
            cat("\"\"\"\n  a \"\"\" b\n  \"\"\""),
            "bynk.lex.multiline_close_not_own_line"
        );
    }

    /// #2619: the semantic-token parts of the three string forms.
    #[test]
    fn string_parts_cover_escapes_and_raw_delimiters() {
        let src = "\"a\\n\\u{41}\" r#\"x\\n\"# \"\"\"\n  \\t \\(\"\\r\")\n  \"\"\"";
        let texts: Vec<(&str, StringPart)> = string_parts(src)
            .into_iter()
            .map(|(s, p)| (&src[s.range()], p))
            .collect();
        use StringPart::*;
        assert_eq!(
            texts,
            vec![
                ("\\n", Escape),
                ("\\u{41}", Escape),
                ("r#\"", RawDelimiter),
                ("\"#", RawDelimiter),
                ("\\t", Escape),
                ("\\r", Escape),
            ]
        );
    }

    fn doc_block_span(source: &str) -> Span {
        tokenize(source)
            .unwrap()
            .into_iter()
            .find(|t| t.kind == TokenKind::DocBlock)
            .expect("a DocBlock token")
            .span
    }

    #[test]
    fn doc_block_body_range_slices_to_the_same_bytes_doc_block_content_would_strip() {
        let src = "---\nHello there.\n---\nfn f() -> Int = 1\n";
        let span = doc_block_span(src);
        let range = doc_block_body_range(src, span).unwrap();
        assert_eq!(&src[range], "Hello there.");
    }

    #[test]
    fn doc_block_body_range_is_offset_preserving_unlike_doc_block_content() {
        // A content line indented relative to its (unindented) markers:
        // doc_block_content strips the common indent (not offset-preserving),
        // doc_block_body_range does not — its slice still contains the raw
        // indentation, so span-based callers can map a position in the raw
        // body straight back to `src`.
        let src = "---\n  See [Foo].\n---\nfn f() -> Int = 1\n";
        let span = doc_block_span(src);
        let range = doc_block_body_range(src, span).unwrap();
        assert_eq!(&src[range.clone()], "  See [Foo].");
        assert_eq!(doc_block_content(src, span), "See [Foo].");
        // The raw range still locates `[Foo]` correctly within `src`.
        let bracket_rel = src[range.clone()].find('[').unwrap();
        assert_eq!(
            &src[range.start + bracket_rel..range.start + bracket_rel + 5],
            "[Foo]"
        );
    }

    #[test]
    fn doc_block_content_and_body_range_agree_on_empty_body() {
        let src = "---\n---\nfn f() -> Int = 1\n";
        let span = doc_block_span(src);
        let range = doc_block_body_range(src, span).unwrap();
        assert_eq!(&src[range], "");
        assert_eq!(doc_block_content(src, span), "");
    }

    /// #1885 (DECISION A): a `----` block closes only on `----`, so the `---`
    /// lines inside it are content, a nested doc example among them.
    #[test]
    fn a_longer_fence_holds_shorter_marker_lines() {
        let src = "----\nIntro.\n---\nInner doc.\n---\nTail.\n----\nfn f() -> Int = 1\n";
        let tokens = tokenize(src).unwrap();
        let docs: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::DocBlock)
            .collect();
        assert_eq!(docs.len(), 1, "one block, not three");
        assert_eq!(
            doc_block_content(src, docs[0].span),
            "Intro.\n---\nInner doc.\n---\nTail."
        );
    }

    /// The trailing-dash trim in `doc_block_body_range` stops at the closer's
    /// line, so a body that ends in a shorter marker line keeps it.
    #[test]
    fn a_body_ending_in_a_shorter_marker_line_keeps_it() {
        let src = "----\nAbove.\n---\n----\nfn f() -> Int = 1\n";
        let span = doc_block_span(src);
        assert_eq!(doc_block_content(src, span), "Above.\n---");
    }

    /// #1885: a marker of another length does not close the block, so a
    /// `----` opened and only `---` seen after is unclosed at EOF, and the note
    /// names the length that would close it.
    #[test]
    fn a_marker_of_another_length_does_not_close_the_block() {
        let err = tokenize("----\nText.\n---\nfn f() -> Int = 1\n").unwrap_err();
        assert_eq!(err.category, "bynk.lex.unclosed_doc_block");
        let note = err.notes.join(" ");
        assert!(note.contains("exactly 4 dashes"), "{note}");
        assert!(note.contains("not a divider"), "{note}");
        // The other way round: a `---` block does not close on `----`.
        let err = tokenize("---\nText.\n----\n").unwrap_err();
        assert_eq!(err.category, "bynk.lex.unclosed_doc_block");
    }
}
