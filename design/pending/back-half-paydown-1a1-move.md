---
level: patch
probes: ast_importers
changelog: The lowering context and TypeScript type rendering move out of `emitter.rs` into their own modules, with no change to emitted output
---

## ADR: tree-native-lowering
title: Tree-native lowering is the route Part 15.1 left open
summary: Phase 1A lowers into the existing bynk_ts tree through a counted bridge, and no expression IR is built

**Context.** `design/bynk-greenfield-compiler.md` Part 15.1 refuses a second expression code generator (the expression IR), at a measured cost of about 4,400 lines for three of seven entry points. Its trigger is an emit-side change that needs expression-level facts the checker already resolved, and which "the tree-native route (lowering to `bynk_ts` nodes, retiring `ts_writes`)" would not serve better. So the refusal leaves the tree-native route open rather than closing it. The back-half debt review (§2.1) found the lowering still builds text: `Lowered { pre: Vec<String>, expr: String }`, with `ts_writes`, `ts_raw_sites` and `ts_text_idents` counting the string-built sites. The back-half paydown track's Phase 1A is that route.

**Decision.** Phase 1A lowers into the existing `bynk_ts` tree. It does so through a counted bridge, `LoweredExpr { Node(TsExpr), Text(String) }` (introduced in 1A.2), so that each slice converts one expression family and the count of `Text` producers falls monotonically. The bridge is deleted when 1A exits. No expression IR is built. Slice 1A.1, which only moves `LowerCtx`, `ModuleCtx` and type rendering out of `emitter.rs`, is byte-identical.

**Consequences.**

- The expression-IR refusal's trigger has not fired: the tree-native route is the better-serving alternative the trigger names, so Part 15.1's refusal stands.
- `ts_writes`, `ts_raw_sites` and `ts_text_idents` fall as 1A proceeds.
- The track's Q2 policy applies to later 1A slices (not to 1A.1): layout-only golden diffs are accepted, each PR stating how many goldens change and that behaviour is unchanged.
- The bridge is temporary scaffolding with a stated exit, not a second lowering path.
