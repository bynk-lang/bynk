---
level: patch
probes: ts_writes
changelog: Capability calls, `Events.emit`, the session and typed cookies and the derived-`D1` `select` are built as TypeScript nodes rather than text, so an event emission and a `D1` read are laid out as indented blocks; no change to what the emitted code does
---
refactor(emit): capability and platform calls lower to nodes (1A.24)

Closes #3725. Part of #3676.

**What changed.** The capability call (in a handler, a provider, a test case and through a `D1.peek` handle), the cross-context capability call, `Events.emit`, the signed and typed cookies, `D1.peek` and the derived-`D1` `select`/`firstAs` return `Option<Built>` and build nodes, in `lower_call/platform.rs` (moved first, mechanically; Decision F). `lower_cross_context_call` stays in `lower.rs` for its service-call half (1A.25) and calls the capability half first. `LowerCtx::trace_step_node` is the node form of `trace_step` (Decision C), and `encode_call` is node-only, so the `// 1A: witness printed for a text caller` markers are gone (Decision E). The first-party `Idempotency` key is a `TemplateLit` when its scope is template-safe, else it stays text. The node and text type-argument forms share one `wire` filter. `D1.peek` is a `Paren(Assign)`. Also, from the 1A.23 review (#3779): the printer brackets an `await` or unary operand under a postfix `!`.

**Correction to #3725 Decision B.** The tracked claim is `Paren(Await(Call(Paren(Arrow), args)))`, not the table's `Await(Paren(Call(…)))`, which printed `await ((…)(a, b))` and moved brackets in nine fixtures.

**Goldens, as ruled.** 28 `.ts` files, layout only, all blessed: 17 `Events.emit` IIFEs as blocks, the 7 `D1` `select` files re-indented from column 0 to their real depth, and 4 parents (a lambda, a `forEach` and `par` thunks) that now print as nodes. The whitespace-stripped text of every file is identical; no bracket, string or `as const` change, no source-map change. Cookies, claims and `Idempotency` are byte-identical.

**Probes.** `ts_writes` 1333 to 1329; `lowering_text_bridge` 60 (no change); `ast_importers`, `ts_any`, `verbatim_sites`, `ts_text_idents` do not move.

**Witness API.** No caller of a string witness form remains: `encode_call` returns a node and `witness_args`, `codec` and `comparator` already did. The remaining `witness_args` callers (`statics.rs`, `lower.rs`) take nodes.
