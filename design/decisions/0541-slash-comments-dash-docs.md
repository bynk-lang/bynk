# 0541 — A comment is `//`; a one-line doc is `--- text`

- **Status:** Accepted (v0.373)

**Context.** A `--` comment and a `---` doc share a glyph, and keeping them
apart cost three rules and one silent-swallow incident. `--` opened a comment
only after whitespace, so `a--b` was subtraction and `a --b` a comment (ADR
0188 D1). A dash ruler, the natural divider in a `--` language, is a doc-block
marker, and a pair of them around code turned it into documentation (ADR
0449). And the one-line doc could not be `--- text`, because `--- Helpers ---`
dividers would have become docs, so it became a third dash spelling, `--|`
(ADR 0454). Proposal #2203 settled the forks below.

**Decision.**

- **A. The comment is `//`**, to the end of the line, anywhere. There is no
  prefix `/`, so `a//b` cannot mean anything else and needs no whitespace rule;
  this supersedes ADR 0188 D1. `--` is two `-` operators, so `a--b` is
  `a - -b`. `#` was rejected (it is a Markdown heading inside a doc), `;` reads
  as a statement separator and `%` would spend modulo.
- **B. The doc marker stays `---`.** `///` was rejected: a `//////` ruler would
  open a doc (ADR 0449's accident with slashes), Rust habits expect consecutive
  `///` lines to join, `//` and `///` would be one character apart again, and
  it loses the thematic-break look. A thematic break inside a doc is `***` or
  `___`, and ADR 0449's `----` fence holds `---` lines.
- **C. The one-line doc is `--- text`**: a marker of three or more dashes, at
  least one space or tab, then text, first on its line. A marker alone on its
  line is still a block fence. An optional closer, a final run of exactly the
  opener's dashes preceded by whitespace, is not part of the doc; any other
  trailing dashes are text. After code on a line, `---` is operators, so an
  end-of-line doc stays out of scope (#1888 DECISION E). This amends ADR 0454:
  the one-line spelling is `--- text`, not `--|`.
- **D. A target has one doc.** Two docs directly above one target, of either
  form, are `bynk.parse.multiple_docs`; when both are one-line docs the fix
  joins them into a block. This replaces ADR 0454 DECISION B (consecutive `--|`
  lines join) and generalises its mixed-forms error, `bynk.parse.doc_forms_mixed`,
  which is retired.
- **E. The old spellings are permanent errors, each with a machine-applicable
  fix.** `bynk.lex.legacy_comment` is a `--` at the start of a line or after
  whitespace, followed by a space or the end of the line (fix: `//`);
  `bynk.lex.legacy_doc_line` is `--|` first on its line (fix: `--- `);
  `bynk.lex.slash_doc` is `///` first on its line (fix: `---`; `////` and
  longer are comments, as in Rust). A legacy comment or doc line still lexes as
  a comment or doc, so an old `x = 1 -- note` is never read as `1 - -note`, and
  the parser reports every one at once. `a -- b` with spaces, as real
  subtraction of a negation, is the price: the message says to write `a - -b`.
- **F. `bynk fmt` is the migration.** A file with any legacy spelling is read by
  the pre-#2203 rules and printed with `//` comments and `--- ` docs; the LSP's
  `source.fixAll.bynk` applies the E fixes. Where a line reads differently under
  the old and new rules (an old `--- Helpers ---` divider, which now reads as a
  doc; a new `--- text` doc or `a --b` subtraction in a half-migrated file),
  only the author knows which was meant, so the formatter refuses the file
  (`bynk.fmt.mixed_comment_syntax`) and names the line rather than guess. The formatter never converts
  between a one-line doc and a block, and it does not print a one-line doc's
  optional closer: the AST records a doc's text and form, not its closer.
  A `--|` run of several lines has no one-line spelling and is migrated by
  hand, to a block.
- **G. No block comment.** ADR 0449 removed the `---` block-comment mapping on
  purpose, and Toggle Line Comment covers the case.

**Consequences.** Every `.bynk` file changes, and the repository migrates in
the implementing change. Dashes only ever mean documentation and `//` only a
comment. The tooling follows: tree-sitter scans `doc_line` externally (only the
scanner can tell a marker with text from a fence, and see that it is first on
its line), and its string text is one token so a `//` in a URL stays text; the
VS Code grammar and language configuration use `//` and `--- text`; the LSP no
longer emits a semantic token for one-line docs, leaving both doc shapes to the
editor grammars so their Markdown highlighting shows, while its legend keeps
the `comment` type and `documentation` modifier (append-only).
