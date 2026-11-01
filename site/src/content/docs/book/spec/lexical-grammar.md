---
title: "§3 Lexical grammar"
---
The lexical grammar defines Bynk's terminals: the tokens a source text is divided
into, and the trivia discarded between them. Each production below is generated
from the grammar ([§2.1](/book/spec/conventions/)); this chapter states only the
**syntactic** facts. Constraints beyond lexing (for example, the admissible range
of a literal) are well-formedness rules and are deferred to §5.

## §3.1 Identifiers and names

### §3.1.1 identifier

{{#grammar identifier}}

A letter followed by letters, digits, or underscores. Identifiers name
declarations, parameters, fields, and bindings. A source word that matches a
[keyword](/book/reference/keywords/) is lexed as that keyword, not as an
identifier.

Some words are **contextual keywords**: they always lex as identifiers, and
the syntactic grammar gives them a keyword's meaning only in one position. For
example, `private` is the visibility modifier only immediately before a
`type`, `fn`, `messages`, `asset`, `embed` or `const` declaration
([§4.1.12d](/book/spec/syntactic-grammar/#4112d-visibility_modifier)); a
binding named `private`, or the `private` in `@cache(scope: private)`, is an
ordinary identifier.

### §3.1.2 constant_name

{{#grammar constant_name}}

An upper-case-initial name. Constant names denote sum-type variants, enum
constants and named constants (`const MaxRetries: Int = 3`,
[§5.12b](/book/spec/static-semantics/#constants)), whose names MUST have this
shape.

## §3.2 Literals

### §3.2.1 number_literal

{{#grammar number_literal}}

A run of decimal digits. A number literal is unsigned; a leading `-` is the unary
negation operator ([§4.6](/book/spec/syntactic-grammar/)), not part of the token. An
integer literal whose magnitude exceeds 2^53 − 1 (9007199254740991), the largest
safe integer and so the `Int` range (#1657), is `bynk.lex.integer_overflow`.

### §3.2.1a float_literal

{{#grammar float_literal}}

A `Float` literal (v0.21): a fraction with a **digit required on both sides**
of the `.` (`1.0`, `0.5`), an exponent (`1e10`, `1.5e-3`), or both. `1.` and
`.5` are rejected as `bynk.parse.malformed_float_literal`. Like
`number_literal` the token is unsigned. A literal that does not fit a finite
IEEE 754 double (`1e999`) is `bynk.lex.float_literal_overflow` — there is no
way to write a non-finite `Float` literal.

`1` is an `Int`; `1.0` (or any exponent form) is a `Float`. The
digit-both-sides rule keeps method calls on numeric literals unambiguous
under maximal munch: `2.5.round()` lexes as `2.5` `.` `round`, and
`1.toFloat()` as `1` `.` `toFloat`. The compiler preserves the literal's
**lexeme** through emission and formatting — `1e10` does not normalise to
`10000000000`.

### §3.2.2 string_literal

{{#grammar string_literal}}

A string literal has three **forms** (#2619). All three denote a `String`; they
differ in how the source text maps to the value.

**Ordinary** — `"…"`, on one line. The escape sequences are:

| Escape | Value |
|---|---|
| `\n` | line feed (U+000A) |
| `\r` | carriage return (U+000D) |
| `\t` | tab (U+0009) |
| `\0` | NUL (U+0000) |
| `\"` | a double quote |
| `\\` | a backslash |
| `\u{H…}` | the Unicode scalar value with 1 to 6 hexadecimal digits |

A `\u{…}` naming a surrogate code point (U+D800 to U+DFFF) or a value above
U+10FFFF is `bynk.lex.bad_unicode_escape`, so every literal is valid Unicode
text; a malformed one (no braces, no digits, more than six, a non-hex digit) and
any other backslash sequence is `bynk.lex.bad_escape`. An unescaped newline does
not appear within the token (`bynk.lex.unterminated_string`).

**Raw** — `r"…"`, or `r#"…"#` with any number of `#` (the closing `"` is followed
by the same number). A raw literal has **no escapes and no interpolation**: every
character, backslashes included, stands for itself, so `r"\d+\.\d+"` is the
eight characters `\d+\.\d+`. The `#` form lets the text hold a `"`
(`r#"say "hi""#`). The `r` is a raw opener only when it is immediately followed by
the `#`s and the `"`; `r "x"` is an identifier `r` and a string. A raw literal is
on one line; use the raw multi-line form for several.

**Multi-line** — `"""…"""` and the raw form `r"""…"""`. The opening delimiter is
followed by a newline (`bynk.lex.multiline_open` otherwise) and the closing
delimiter has a line of its own (`bynk.lex.multiline_close_not_own_line` when a
`"""` ends a line of text). The text is the lines between them, joined by `\n`:

- the **closing delimiter's indentation** (its leading spaces and tabs) is
  removed from the start of every line; a line that is not blank and does not
  start with that indentation is `bynk.lex.multiline_indent`, and a blank line
  shorter than the indentation is empty;
- the newline before the closing delimiter is not part of the value, so a body
  of one line `SELECT 1` has the value `SELECT 1`; a blank last line adds a
  trailing `\n`;
- **line endings are always `\n`**: a source file's `\r\n` does not reach the value,
  and a `\r\n` in a literal is written explicitly;
- the ordinary form takes the same escapes and interpolation holes as `"…"` (an
  interpolation hole closes on the line it opens on); the raw form takes neither.
  A `"` or `""` may appear in the text; in the ordinary form a `"""` is written
  `\"""`, and a raw multi-line literal cannot hold a `"""` at all.
- the indentation must match the closing delimiter's **character for
  character**: a tab is not a space, and a line that mixes them differently is
  `bynk.lex.multiline_indent`, whose note says so;
- indentation is removed from each physical line *before* interpolation holes
  are found, and never inside a hole; the newline right after the opening
  delimiter is not part of the value; a lone `\r` is not a line break.

A multi-line string cannot appear inside an interpolation hole
(`bynk.lex.multiline_in_hole`): a hole closes on the line it opens on. A line
whose text starts with `"""` closes the literal unless a letter, digit, `_` or
`"` follows it, in which case the `"""` is a stray delimiter
(`bynk.lex.multiline_close_not_own_line`).

Design choices (#2619): the closing delimiter has a line to itself, a
single-line raw literal cannot span lines, and `#` delimiters belong to the
single-line raw form only (there is no `r#"""`).

Indentation is removed *before* escapes are read, so `\t` is a tab in the value
whatever the margin.

A string may also contain **interpolation holes** of the form `\(expr)`
(v0.43): the text `\(` opens a hole whose body runs to its matching `)`
(parentheses balance, and a nested `"…"` inside a hole is skipped so its
parens do not close the hole), and the body is an ordinary
[expression](/book/spec/syntactic-grammar/). `\\(` is the escape for a literal `\(`,
so existing literals are unaffected (a bare `\(` was previously an invalid
escape). A string containing one or more holes is an *interpolated string*; one
with none is the plain string literal above. The hole rule and emission are
specified in [§5.2 well-typedness](/book/spec/static-semantics/#52-well-typedness) and
[§7 emission](/book/spec/emission/).

### §3.2.2b escape_sequence

{{#grammar escape_sequence}}

One escape sequence inside an ordinary or multi-line string literal (#2619); the
table in §3.2.2 gives each value.

### §3.2.2a string_interpolation

{{#grammar string_interpolation}}

One interpolation hole, `\(expr)`, inside a string literal (v0.43).

### §3.2.3 boolean_literal

{{#grammar boolean_literal}}

The two `Bool` values, `true` and `false`.

### §3.2.4 unit_literal

{{#grammar unit_literal}}

The unit value `()` — the single value of the unit type. It is lexically the
empty parenthesis pair.

## §3.3 Comments and doc-blocks

### §3.3.1 line_comment

{{#grammar line_comment}}

A comment runs from `//` to the end of the line, anywhere on a line (#2203).
Line comments are trivia ([§3.4](#34-trivia)). Bynk has no prefix `/`, so `//`
cannot mean anything else and needs no whitespace rule, and `--` is two `-`
operators: `a--b` lexes as `a - -b` (a subtraction of a negation), and `x--` as
`x` followed by two `-` operators. There is no block comment.

### §3.3.2 doc-blocks

A **doc-block** is a `--- … ---` documentation block. It is an *external token*:
not a grammar rule but a terminal recognised by the lexer and attached to the
declaration that follows it. Like comments and whitespace, a doc-block is trivia
([§3.4](#34-trivia)).

The **opening and closing markers** are each a line whose only content is **three
or more consecutive hyphens** (`---`, `----`, …), preceded only by optional
horizontal whitespace and followed only by optional horizontal whitespace to the
end of the line. The **closing marker has exactly as many hyphens as the opening
one**, as with a Markdown code fence; a marker line of any other length inside the
block is content. So a block opened with `----` can hold `---` lines: front
matter, or a Bynk example with its own doc-block. A Markdown thematic break
inside a doc is best written `***` or `___`, which need no longer fence.

```bynk
---
The answer, with a Markdown rule in its doc:

***

Below the rule.
---
fn answer() -> Int { 42 }
```

There is **no standalone `---` divider**: a marker line always *opens* (or
*closes*) a doc-block, so a marker with no closing marker of its length is the
error `bynk.lex.unclosed_doc_block`, not a horizontal rule. The content between
the markers is arbitrary text and may contain `//` or `--` fragments without
closing the block.

A doc-block's content must not be code. If the content, with its Markdown code
fences (```` ``` ```` or `~~~`) removed, parses as one or more declarations, the
block is the error `bynk.parse.doc_block_contains_code`. This catches a pair of
dash dividers around code, which would otherwise turn that code into
documentation silently, whether or not a declaration follows. For a divider,
write a line comment (`// ---------`); to show code in a doc, put it in a code
fence.

```bynk,fail=bynk.parse.doc_block_contains_code
--------------------------------
fn helper() -> Int { 1 }
--------------------------------
fn f() -> Int { 2 }
```

**The one-line form.** A **one-line doc** is a marker of three or more hyphens,
**at least one space or tab**, then text, all on one line (#2203). It is an
external token too, and trivia in the same way. It is a doc only when it is the
**first thing on its line**, after optional horizontal whitespace; after code
on a line, `---` is three `-` operators, so there is no end-of-line doc. A
marker with nothing else on its line is a block marker, as above. The marker's
length matters only for an optional **closer**: a final run of exactly the
opener's hyphens, preceded by whitespace, which is not part of the doc. Any
other run of trailing hyphens is text. The doc's text is what follows the
marker and one separating space, less the closer.

```bynk
--- A full commit SHA.
type Commit = String

type Run = {
	--- The repository, as `owner/name`. ---
	repo: String,
	// A comment, not a doc.
	passed: Int,  // an end-of-line comment
}
```

A one-line doc is not probed for code (`bynk.parse.doc_block_contains_code`):
it is marked as a doc on purpose, so no pair of dividers can turn code into one
by accident.

**Where a doc may go.** A doc of either shape documents the declaration that
follows it and, since #1888, each of these members: a record field, a sum
variant of either form (`| V` or an `enum { V }` tag), a variant's payload
field, and a `messages` entry. A function's or handler's parameters are
documented in the function's own doc. A doc separated from what follows by a
blank line, or with nothing after it in its body, attaches to nothing
(`bynk.parse.orphan_doc_block`, a warning).

**One doc per target.** A doc directly followed by another, of either shape,
with no blank line between, is the error `bynk.parse.multiple_docs`: a
declaration or member has one doc, so a doc of more than one line is a block.
When both are one-line docs, the error's fix joins them into one block. `bynk
fmt` prints each doc in the shape it was written in and never converts between
them.

```bynk,fail=bynk.parse.multiple_docs
--- The answer.
--- And a second line.
fn answer() -> Int { 42 }
```

### §3.3.3 Retired spellings

Before #2203 a comment was `--` and a one-line doc was `--|`. Both are
**errors**, each with a machine-applicable fix, so an old file never silently
changes meaning (an old `x = 1 -- note` is not read as `1 - -note`):

- `bynk.lex.legacy_comment`: a `--` at the start of a line or after whitespace,
  followed by a space or the end of the line. The fix is `//`. For a real
  subtraction of a negation, write `a - -b`.
- `bynk.lex.legacy_doc_line`: a `--|` first on its line. The fix is `--- `.
- `bynk.lex.slash_doc`: a `///` first on its line (a Rust or C# doc habit). The
  fix is `---`. A run of four or more slashes is a comment.

`bynk fmt` rewrites the first two over a whole project, and the VS Code
extension's **Fix All** (`source.fixAll.bynk`) applies the same fixes.

```bynk,fail=bynk.lex.legacy_comment
fn answer() -> Int { 42 }  -- the answer
```

## §3.4 Trivia

Between tokens the lexer discards **trivia**: whitespace (`/\s+/`), line comments
([§3.3.1](#331-line_comment)), and doc-blocks and one-line docs
([§3.3.2](#332-doc-blocks)). Trivia
does not appear in the productions of §4 — no production has a newline
terminal — but it is not wholly insignificant: the parser consults whether a
newline separates two tokens at three sites, each a narrow, documented
carve-out rather than a general rule. A `+`/`-` beginning a new line does not
continue an additive chain ([§4.6.6](/book/spec/syntactic-grammar/#467-binary_expr));
a `[` opening a new line is a list literal rather than explicit type
application on the preceding name, both for a call
([§4.6.21a](/book/spec/syntactic-grammar/#4621a-list_literal)) and for a
method call ([§4.6.8](/book/spec/syntactic-grammar/#468-method_call)). No
other construct is newline-sensitive — in particular, `match`-arm separation
([§4.7.1](/book/spec/syntactic-grammar/#471-match_arm)) is not: arms are
terminated by their own greedy parse and an optional trailing comma, with no
newline check. The complete token-and-trivia summary is part of the grammar
appendix ([§11](/book/spec/grammar-appendix/)).
