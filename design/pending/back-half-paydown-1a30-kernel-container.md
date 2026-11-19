---
level: patch
probes: ts_writes, lowering_text_bridge
changelog: The built-in `Option`, `Result`, `Set` and `Map` kernel methods in generated code are built as TypeScript nodes rather than text, so redundant brackets round receivers are gone and several generated files, among them the standard-library `string.ts` and `locale.ts`, print an enclosing closure with different layout; no change to what the emitted code does
---
refactor(emit): the container kernels lower to nodes (1A.30)

Closes #3731. Part of #3676.

**What changed.** `lower_option_kernel`, `lower_result_kernel`, `lower_set_kernel` and `lower_map_kernel` move, byte for byte, to `lower_call/kernel_container.rs` (Decision C), then build nodes through `node_or_text`. A text operand falls back to today's text byte for byte. Receivers go through `receiver_object`, so a receiver is bracketed only where precedence needs it. The `Option` and `Result` combinators are typed-arrow IIFEs (`Call` over an `Arrow` with typed parameters and a `Conditional`); the printer already brackets an arrow callee, so no `Paren` is added. `Set` and `Map` arms use `TypeApp` for `new globalThis.Set<T>(..)` and the keyed helpers. Every `note_key` fires before the receiver is lowered, as before, and the finding #28 panics are unchanged, with the same messages. `Set.toList` is a node too (`Paren(As(Array[Spread(recv)], readonly T[]))`).

**Arm that stays text, named.** The plain `Set.remove` is a comma expression, which `bynk_ts` has no node for. It stays text, marked `// 1A: comma expression has no bynk_ts node`. `Map` has no `remove`.

**Type spelling.** `ts_ty` is `print_type(ts_ty_to_ts_type(..))`, so the node and text type arguments are identical by construction; the goldens show no type token change.

**Goldens, as ruled: 77 files blessed, all `.ts`.** 134 redundant receiver bracket pairs dropped (63 `(recv).size`, 29 `[...(recv).keys()]`, 3 `[...(recv).values()]`, 7 `(recv).has(`, 32 `[...(recv)]` in `toList`), and the enclosing IIFE re-laid out in 33 files (27 runtime, among them `string.ts` in 24 fixtures and `locale.ts` in 23, where the plain-key sort's `const __ka = ..., __kb = ...;` becomes two statements). Whitespace-stripped text is identical apart from the dropped brackets and that split. No bracket was gained, and no tag test, constructor, type argument, `ReadonlySet` or keyed-helper choice changed. No source map fixture changed.

**Test changes.** Tests that used a container kernel as their still-text example now use a construct that stays text: a `contains` or `length` over a plain `Set.remove`, or a `String.splitRegex` list mapped and read with `firstOrElse`. They are in `lower.rs` (bridge tests, list kernel tests), `lower_stmt.rs`, `lower_call/statics.rs` and `lower_call/kernel_scalar.rs`. `delegated_kernels_print_as_before` now expects `m.size` and `s.has(1)`. New tests are in `lower_call/kernel_container.rs` and `bynk-tests/tests/integration/kernel_container_nodes.rs`.
