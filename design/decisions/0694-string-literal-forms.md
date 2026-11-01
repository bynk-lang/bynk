# 0694 — String literals have raw and multi-line forms and a wider escape set

- **Status:** Accepted (v0.476)

**Context.** A string literal recognised only `\n`, `\t`, `\"`, `\\` and `\(…)`.
There was no `\r` (HTTP header lines, CSV `\r\n`), no way to write a
non-printing or confusable character, and no raw or multi-line form. The
regex methods and `Matches` require a string *literal* (`bynk.types.regex_not_literal`)
and D1 statements must be literals too (`bynk.types.sql_not_literal`), so every
regex backslash was doubled and every SQL statement sat on one line. There is no
library workaround for a missing escape.

**Decision.** (proposed in #2619)

- **Escapes.** Add `\r`, `\0` and `\u{1–6 hex digits}`. A `\u{…}` naming a
  surrogate (D800–DFFF) or a value above 10FFFF is a lexical error
  (`bynk.lex.bad_unicode_escape`), keeping every literal valid Unicode text; a
  malformed one is `bynk.lex.bad_escape`.
- **Raw strings.** `r"…"`, and `r#"…"#` (any number of `#`) for text holding
  quotes: no escapes, no interpolation. `r` opens a raw string only when it is
  immediately followed by the `#`s and the quote, so an identifier `r` followed
  by a (spaced) string is unaffected and the formatter never inserts a space.
- **Multi-line strings.** `"""` followed by a newline, text, and a closing `"""`
  on a line of its own; `r"""…"""` is the raw variant. The closing delimiter's
  indentation is stripped from every line, and a non-blank line indented less is
  a lexical error (`bynk.lex.multiline_indent`). Escapes and interpolation work
  as in an ordinary string. The newline before the closing delimiter is not part
  of the value.
- **Line endings.** A multi-line literal's newlines are always `\n`, whatever the
  source file's line endings; `\r\n` is written explicitly.
- **One token, one value.** The lexer decodes every form to the same
  `StrLit`/`InterpStr` token and AST value, so the checker, the regex and `Matches`
  checks, D1 and message bundles consume the decoded value unchanged. The emitter
  lowers the decoded value to an escaped TypeScript string (`\0` and other control
  characters, U+2028 and U+2029 as `\u{…}`). The formatter keeps a raw, multi-line
  or `\u{…}` literal as written (a multi-line one re-indented to the surrounding
  code, which its stripping makes value-neutral).

**Consequences.** No existing program changes meaning: each new form was a lexical
error. `"""` could not previously start a valid token. Semantic tokens mark every
escape sequence and a raw literal's delimiters; hover, completion and signature
help are unchanged, because a literal has no hover beyond its type, nothing
completes inside it, and no signature changes. The tree-sitter grammar recognises
raw strings with an external-scanner token (the `#` count is not regular) and
leaves the surrogate, indentation and own-line-delimiter rules to the compiler.
