---
level: patch
probes: ts_writes, ts_any
changelog: Test-case and system-tier service calls in generated test harnesses are built as TypeScript nodes rather than text, so a few expressions print with different layout and six redundant brackets (`(x).url`) are dropped; no change to what the emitted code does
---
refactor(emit): test-case and system-tier service calls lower to nodes (1A.26)

Closes #3727. Part of #3676.

**What changed.** The HTTP, cron, queue, event, plain `call`, asset and system arms of the test-case and system-tier service-call dispatchers build nodes through `node_or_text`. They live in `lower_call/test_calls.rs`, moved first and mechanically (Decision D). `call_site_identity` and `call_site_claims` are `Option<LoweredExpr>`, set at the two `by` sites with `pre.lower_node`; `system_http_claims_arg` and `system_http_sum_actor_arg` return the same, and `call_site_claim_lines` is converted too. Driver names come from naming functions as `Ident(name)`, never `Ident(format!(..))`. `settle_with(helper, built)` is new and `settle_built` now calls it. No new `fn-length-ceilings.txt` entry.

**Goldens, as ruled: 24 `.test.ts` files blessed.** (b1) faults IIFEs go multi-line; (b2) match IIFEs are one step shallower; (c) six redundant bracket pairs `(x).url` become `x.url` (the 1883 asset suites, three files, four pairs; the 2024 let-shadow fixtures, two files, one pair each). Whitespace-stripped text is identical in 19 files; the other 5 differ only by those six bracket pairs. No driver name, `claims` or `identity` slot, slot order, `as any`, string content or source map changed.

**Foreign text, named.** The actor-claims table is built in the driver (`system_support.rs`), so that arm stays text, marked. The three websocket-lifecycle IIFEs stay text (`// 1A: needs typeof and indexed-access TsTypes`). `by User("ada")` stays text, because the brand-cast construct belongs to another family.

**Probes.** `ts_writes` 1329 to 1323 and `ts_any` 40 to 38. `lowering_text_bridge` stays at 58, as ruled: no counted bridge site was retired, because the remaining arms (the claims table, the websocket lifecycle IIFEs, the brand-cast `by`) are the ones that stay text. `ast_importers` stays 14 and `ts_text_idents` 14.
