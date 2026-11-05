---
title: "`tree-sitter-bynk`"
---
The [tree-sitter](https://tree-sitter.github.io/tree-sitter/) grammar for Bynk.
It is the source of truth for structural tooling — syntax highlighting (the
playground's live editor, and any editor that speaks tree-sitter). The static
documentation site highlights its code blocks separately, via Expressive Code /
Shiki from the editor's TextMate grammar.

## Layout

| Path | What it is |
|---|---|
| `grammar.js` | The grammar definition — the file you edit. |
| `src/parser.c`, `src/scanner.c` | Generated parser plus a hand-written external scanner (for `--- … ---` doc blocks and raw strings). |
| `src/grammar.json`, `src/node-types.json` | Generated grammar metadata. |
| `queries/highlights.scm` | Highlight queries (the highlight groups). |
| `queries/injections.scm` | Language-injection queries: a tagged multi-line string's body (`"""sql`) as the tag's language. |
| `test/corpus/` | Versioned parse-tree test cases. |

The generated files in `src/` are committed. The book's
[grammar reference](/book/reference/grammar/) is generated from `src/grammar.json`.

## Build and test

With the [tree-sitter CLI](https://tree-sitter.github.io/tree-sitter/cli):

```sh
npm install          # once
npm run build        # tree-sitter generate  → regenerates src/
npm test             # tree-sitter test      → runs the corpus
```

After editing `grammar.js`, run `npm run build` to regenerate the parser, then
`npm test` to check it against the corpus. A corpus case is a named section: the
source, a `---` separator, and the expected S-expression parse tree.

## Highlight groups

`highlights.scm` maps grammar nodes to standard highlight groups — `@keyword`
(and `@keyword.declaration`, `.import`, `.modifier`, `.operator`), `@type`,
`@type.builtin`, `@string`, `@number`, `@comment`, `@function`, `@variable`,
`@operator`, `@punctuation.*`, and more. These groups drive the editor (VS Code
via `vscode-bynk`) and the playground's wasm tree-sitter highlighter, so
interactive highlighting stays correct as the grammar evolves. The static
documentation site highlights ```` ```bynk ```` blocks separately, via Expressive
Code / Shiki from the editor's TextMate grammar, so it is not driven by these
tree-sitter highlight groups.

## Language injection

A multi-line string may carry a language tag, `"""sql` or `r"""regex`. The tag
is a `string_tag` node, and the body is a `string_body` (or, raw,
`raw_string_body`) node; `injections.scm` injects the body as the language the
tag names. Interpolation holes and escapes are children of the body, so they are
left out of the injected text. An editor with no grammar for the tag shows a
plain string.

## Keeping it in sync

`grammar.js` is regenerated to `src/` via `tree-sitter generate`; commit the
generated files together with the grammar change so downstream consumers (the
editors and the playground's highlighter) stay consistent.
