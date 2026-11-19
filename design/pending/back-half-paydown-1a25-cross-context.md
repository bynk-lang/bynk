---
level: patch
probes: verbatim_sites, lowering_text_bridge
changelog: Cross-context service calls, on the bundle and the Workers target, are built as TypeScript nodes rather than text, so a call with no arguments sends `{}` rather than `{  }`; no change to what the emitted code does
---
refactor(emit): cross-context service calls lower to nodes (1A.25)

Closes #3726. Part of #3676.

**What changed.** The bundle call `deps.surface.<key>.<method>(args)` and the Workers `__callService(…)` build nodes when every argument is one, and fall back to the same text otherwise. They live in `lower_call/cross_context.rs`, with the boundary casts (`param_cast`, `return_cast`, `lower_workers_cross_context_call` and their helpers), moved first and mechanically from `emit.rs` and `lower.rs` (Decision E). Both casts share one `Paren(As(As(value, unknown), ty))` builder. `lower_cross_context_service_call` takes the caller's `Pre` and returns `Option<Built>`, so the dispatcher passes it straight through; the traced build uses 1A.24's `trace_step_node`.

**Decision C: the printer needed no change.** The proposal's premise was stale. `bynk_ts` already printed a left-nested `As` without brackets (`a as unknown as T`): the `As` arm brackets only a `Binary`, `Arrow` or `Conditional` operand, and `serialisation.rs` already builds such chains. There is no printer contract change. Two printer tests now pin the behaviour: a left-nested `As` prints bracket-free, and an `As` in a member, callee or `await` position, or an explicit `Paren` under an `As`, keeps its brackets. The stale note was removed from the `VerbatimExpr` doc.

**Decision D, a layout change.** A zero-argument Workers service call sent the opaque `{  }`; it is an empty `Object` and prints `{}`. `workers.rs` and `project.rs` keep their own text.

**Goldens, as ruled.** 21 Workers `.ts` files (14 test harnesses, 7 `handlers.ts`), 40 lines, all `{  }` to `{}`; whitespace-stripped text identical in every file. No bundle file, bracket, contract hash, argument order, `trace_binding` or source map changed.

**Foreign text, named.** The renamed-literal cast `globalThis.Parameters<typeof codec>[0]` stays a text type name (`bynk_ts` has no `typeof` or indexed-access type, and none was added). A still-text argument goes through `text_value`, marked.

**Probes.** `verbatim_sites` 15 to 14; `lowering_text_bridge` 60 to 58; `ts_writes`, `ts_text_idents`, `ast_importers` and `ts_any` do not move.
