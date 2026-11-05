# 0739 — A multi-line string literal may carry a language tag for editors

- **Status:** Accepted (v0.510)

**Context.** Multi-line strings ([ADR 0694](0694-string-literal-forms.md)) are
mostly used for inline SQL, because D1 statements must be literals
(`bynk.types.sql_not_literal`). In an editor that SQL is one block of string
colour. A TextMate grammar can embed another language only where the source
marks the region: guessing from context (the first argument of `.run(` is SQL)
is beyond a regex grammar, and semantic tokens cannot apply another language's
grammar. So the marker has to be lexical syntax, which the compiler owns.

**Decision.** This amends ADR 0694's multi-line form.

- *Tag on `"""`, not backtick fences.* The opening `"""` (or `r"""`) may be
  followed directly by a language tag: `"""sql`. Markdown-style fences were
  rejected because Bynk source constantly sits inside Markdown code fences (doc
  blocks, the book, READMEs, issues), where a line starting with three
  backticks would close the surrounding fence. They would also be a second
  multi-line syntax.
- *Tag syntax.* `[a-z][a-z0-9_-]*`, with no space before it, then optional
  spaces or tabs, then the newline. Anything else after the delimiter stays
  `bynk.lex.multiline_open`, whose note now mentions the tag. An untagged `"""`
  still takes nothing before its newline, not even spaces.
- *Open set.* The lexer accepts any tag. Editors map the tags they know: `sql`,
  `json`, `html`, `css` and `js` in the site's TextMate grammar (and in
  vscode-bynk's, through its own PR), and whatever tree-sitter grammar an
  editor has, through `injections.scm`. An unknown tag is a
  plain string. New editor mappings never need a compiler release.
- *Value-neutral, no checking.* The tag is not part of the value: the token,
  decoded value and type are those of the untagged literal, so the checker,
  emitter and runtime are unchanged. Checking a tagged body (a `"""json` that
  doesn't parse, a `"""sql` outside a statement position) would be a separate
  increment.
- *Multi-line only.* A single-line string takes no tag: `"sql…"` has no
  unambiguous spelling, and the value is in multi-line SQL.

Tooling: semantic tokens mark the tag as `decorator` and still give the body no
whole-string token, so an editor's embedded highlighting shows through. Hover,
completion and signature help are unchanged. The formatter keeps the tag as
written, dropping trailing spaces after it. tree-sitter-bynk gains `string_tag`,
`string_body` and `raw_string_body` nodes, and `injections.scm` injects the
body as the tag's language. The raw multi-line string is now an external
opener, the opening line's rest, and an external body, so its tag and body are
nodes too, and its opener is now validated like the compiler's.

**Consequences.** `"""sql` highlights as SQL in the book and in tree-sitter
editors. VS Code follows when vscode-bynk takes the same TextMate rules in its
own PR. The site's grammar is a copy of vscode-bynk's, and now carries the tag
rules first, with a comment saying to change both together. It also regained the
two keyword rules it had drifted without. A `regex` tag has no TextMate mapping,
because neither VS Code nor Shiki ships a standalone regex grammar, but
tree-sitter editors with a `regex` grammar inject it.
