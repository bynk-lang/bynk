---
level: patch
probes: ts_writes, ts_raw_sites_emit, stmt_text_lines
changelog: Service and agent handler bodies reach the TypeScript printer as statements, and IIFE and lambda bodies are now source-mapped, so `bynk test --inspect` can step into them; output differs only in indentation
---
refactor(emit): handler bodies reach the tree as statements; IIFE and lambda bodies are source-mapped (1A.15, second half)

Closes #3716. This is the second half of the Decision F split. The first half, #3756, moved function, method, provider-op and WebSocket Durable Object bodies.

**Handler bodies.** Service and agent handler bodies now go through `Body::into_stmts`.
- They drop `body_out`, the per-body `SourceMapBuilder`, `shift_checkpoints` and the events-IIFE raw blob.
- `RawBody` and `emit_class_method_and_merge_source_map` are deleted.
- The `__events` declaration, the events flush, the `return __result`, the agent `currentState`/`__state` declarations and `commitState` are now nodes.

**Claim-scope wrapper.** It is now nodes: `let __r;`, a `try` over an async closure that assigns the body's result, a `catch` that releases the claims and rethrows, then the confirmation and the return.
- The `__claims` initialiser stays one text statement (`CLAIM_SCOPE_DECLARATION`). Its type assertion carries a generic method signature (`note<C>(...)`), which `bynk_ts` type members cannot express.

**Source maps inside arrow bodies.** `bynk_ts` threads the source map explicitly through the expression renderers, so statements inside a multi-line arrow body record spans and merge their `nested_map`.
- `without_source_map`, `Body::to_nodes_unmapped` and the `mapped` parameter of the `match` emitters are deleted.
- IIFE, `match`-IIFE and lambda bodies are mapped for the first time, which ends the `bynk test --inspect` degradation inside them.
- Two new inspector-style tests step into a lambda body and into a value-position `if`. Both fail before this change.
- A lambda reached through the text method-call dispatchers (for example a list `.map`) is still text and stays unmapped.

**Value-position `match` (carried over from 1A.14).** `match_body` writes each arm tail through `tail_item`, and `lower_expr`'s `Match` arm keeps the node.

**Left as text on purpose.** The events-release closure lines and the prologue guards stay `Item::line` text, because converting them would reindent 126 golden files. Track row 1A.ws covers that whitespace-only follow-up.

**Goldens.** 84 files change, all whitespace only (`git diff -w` is empty), as ruled:
- 6 for the claim wrapper;
- 78 for the value-position `match` layout.

**Gated probes** (against the state after #3756):

| Probe | Before | After |
|---|---|---|
| `ts_writes` | 1447 | 1428 |
| `ts_raw_sites_emit` | 37 | 35 |
| `stmt_text_lines` | 32 | 31 |
