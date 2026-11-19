---
level: patch
probes: ts_writes, lowering_text_bridge
changelog: The built-in `Int`, `Float`, `Decimal`, `Duration`, `Instant`, `Bytes` and `String` kernel methods in generated code are built as TypeScript nodes rather than text, so redundant brackets round receivers are gone and several generated files, among them the standard-library `string.ts`, print an enclosing closure with different layout; no change to what the emitted code does
---
refactor(emit): the scalar kernels lower to nodes (1A.29)

Closes #3730. Part of #3676. Mentions #3770 (not fixed here).

**What changed.** `lower_int_kernel`, `lower_float_kernel`, `lower_duration_kernel`, `lower_instant_kernel`, `lower_bytes_kernel`, `lower_string_kernel`, `regex_construction` and `lower_decimal_kernel` move, byte for byte, to `lower_call/kernel_scalar.rs` (Decision D), then build nodes through `node_or_text`. A text operand falls back to today's text byte for byte. Receivers go through 1A.28's `receiver_object`, so a receiver is bracketed only where precedence needs it. Every `note_*` call, including the per-op `Decimal` helper, fires as before, and every kernel exits through `Pre::finish_built`.

**Regex.** The pattern is a `Str` node in `new globalThis.RegExp(..)`. The printer's escape and `escape_ts_string` are one function; a test compares the node and the text over a corpus of hard patterns and every pattern literal in the fixtures, so no regex arm is held back. `findRegex`, `findAllRegex` and `replaceRegex` build their typed arrows as nodes.

**Arms that stay text, named.**
- `Duration.toMillis`, `Instant.toEpochMillis` and `Int.toFloat` are identity arms. The first two are the kernels behind #3770, and converting any of them would make the printer bracket the operand, which is a behaviour fix and not a paydown change. They stay text, marked `// 1A: #3770, ...`, and a test pins today's (wrong) output for the #3770 case; the fix PR flips it deliberately. `Int.toFloat` was kept text by the same instruction without first measuring whether converting it would gain a bracket.
- `String.splitRegex` stays text, marked `// 1A:`: its body is a statement block with a `while` loop that `bynk_ts` cannot print in the one-line layout this arm has.

**Goldens, as ruled: 75 files blessed, all `.ts`.** 132 redundant receiver bracket pairs dropped in 47 files (100 `.length`, 13 Bytes `.slice`, 4 Decimal `.scale`, 2 `.replace`, 1 `.matchAll`, 12 unattributed in the `expect` text of the same ops), and the enclosing IIFE re-laid out in 36 files (24 are the standard-library `string.ts` `join`). Whitespace-stripped text is identical apart from the dropped brackets. No bracket was gained, and no regex source or flags, `Decimal` helper, import or string literal changed.

**Test changes.** Six existing unit tests used a `String` kernel as their still-text example; they now use `Set.contains`, `Set.length` or `Option.getOrElse`, which are still text. `delegated_kernels_print_as_before` now expects `b.length` and `s.length`.

**Nits from the 1A.28 review.** `receiver_object`'s doc now says it also brackets `Spread` (defensively) and `TsLit::Raw`. No output change.
