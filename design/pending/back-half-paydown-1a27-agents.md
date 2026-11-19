---
level: patch
probes: lowering_text_bridge
changelog: Agent construction and agent calls in generated code are built as TypeScript nodes rather than text, so four generated test and demo files print an enclosing closure with different layout; no change to what the emitted code does
---
refactor(emit): agent construction and agent calls lower to nodes (1A.27)

Closes #3728. Part of #3676.

**What changed.** The inline call `Agent(key).m(args)`, the let-bound call, the self-agent WebSocket transfer, the `AgentInit` branch of `lower_call` and agent construction build nodes through `node_or_text`. They live in `lower_call/agents.rs`, moved first and mechanically (Decision D). `LowerCtx::agent_construct_node` replaces the string `agent_construct`: it sets `agents_instantiated` as before and builds `Call(Ident(agent_factory_name), [key])`, with `Member(<deps node>, "env")` appended on Workers, so a provider body keeps `this.deps.env`. The deps argument is the existing `cap_deps_node`. The key is lowered first, and in the self-agent case not at all. `record_agent_call` and the trace note fire as before. A text key or argument falls back to today's text byte for byte. No new `fn-length-ceilings.txt` entry.

**Goldens, as ruled: 4 files blessed.** `139_agent_state_zero_option` (`demo_slot.test.ts`), `2050_agent_emit_under_test` (`app.test.ts`), `2654_record_let_destructuring` (`shop_orders.test.ts`) and `3298_par_closure_rewrite` (`demo.ts`). An enclosing faults IIFE, match IIFE or `par` closure now prints through the node path, so its layout changes. Whitespace-stripped text is identical in all four, token counts are equal, and no file gained trailing blank lines. No bracket, argument order, `deps` or `env` argument, `this`/`deps` choice or source map changed.

**`trace_step` stays.** The string wrapper still has callers: the text fallbacks in `cross_context.rs` and `platform.rs`, and a unit test in `platform.rs`. The agent calls use it only for a call whose key or argument is still text.

**Probes.** `lowering_text_bridge` 58 to 57: `AgentInit` was a counted `into_text` bridge site and is now a node. `ts_writes` stays at 1323, against the proposal's expectation that it would fall: the agent paths built their text with `format!`, which the probe does not count, so nothing it counts was retired. `ast_importers` stays 14 and `ts_text_idents` 14.
