---
title: "`bynk-fmt`"
---
Bynk's formatter. There is one implementation, in the `bynk-fmt` crate — a leaf
over `bynk-syntax` that never links the compiler; the CLI (via `bynk-driver`)
and the language server both drive that one copy. You invoke it as `bynk fmt`
(or `bynk fmt`) — see the how-to
[Format your code with `bynk-fmt`](/docs/editor-and-tooling/format/) for usage.

## What it does

`format_source(source, &FormatOptions)` tokenises and parses the source, then
re-prints the AST in canonical form. It is **idempotent** — formatting formatted
code is a no-op — and it returns a `FormatError` (carrying the parse diagnostics)
if the source does not parse.

## Options

`FormatOptions` controls the output:

| Field | Type | Default |
|---|---|---|
| `indent` | `IndentStyle` (`Tab` or `Spaces(n)`) | `Tab` |
| `max_line_width` | `u32` | `100` |
| `trailing_comma` | `bool` | `true` |

Three sources feed them, each overriding the one before:

1. the defaults above;
2. the project's `[fmt]` section in [`bynk.toml`](/docs/manifest/) (`indent`,
   `indent_width`, `max_line_width`, `trailing_comma`), resolved from the
   nearest manifest at or above the file being formatted;
3. the flags a CLI run passes — `--indent tab|spaces`, `--indent-width N`,
   `--max-line-width COLUMNS`, `--trailing-comma` / `--no-trailing-comma`.

Step 2 goes through `bynk_fmt::FmtConfig`, which both the CLI and the language
server call, so a manifest they can both read is interpreted identically.
`--no-config` drops step 2 for a run.

A manifest that does *not* read is where they differ: the CLI reports the error
and formats nothing; the language server falls back to the canonical style,
since it cannot refuse to serve. A typo therefore shows up as an editor that
quietly stops honouring the section and a `bynk fmt` that fails loudly.

## Canonical style

- Tab indentation, one tab per nesting level.
- K&R braces — the opening brace stays on the construct's line.
- Trailing commas in multi-line records, sums, list literals and `exports`
  clauses. Parameter and argument lists never carry one — the grammar rejects it.
- One blank line between top-level declarations; none inside record/sum/parameter
  lists or between match arms.
- A doc block sits directly above its declaration, with no blank line between.
- One space around binary operators and after commas; no padding inside
  parentheses.
- A soft 100-column width. A construct that would overrun it wraps vertically —
  one entry per line for records, lists and argument lists; a break before each
  `&&`/`||`; a break before each call of a long `.`-chain. A line whose overflow
  sits inside a single token (a long string literal) is left long rather than
  mangled.

## Programmatic use

```rust
use bynk_fmt::{format_source, FormatOptions};

let formatted = format_source(source, &FormatOptions::default())?;
```

This is exactly what `bynk fmt` and the language server's formatting requests
call, and both resolve their `FormatOptions` through the same `[fmt]` reader, so
editor format-on-save and CLI formatting agree on any project whose manifest
parses.
