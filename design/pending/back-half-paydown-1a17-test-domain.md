---
level: patch
probes: ts_writes, lowering_text_bridge, stmt_text_lines, ts_any
raises: ts_any — the node form of the test-scaffold refined brand (`unchecked_construct_test_node`, `TsType::named("any")`) sits beside the text `unchecked_construct_test`, which the driver's `property_generators.rs` still calls; the cast itself is unchanged (P7.2) and the count falls when those callers convert
changelog: The test-domain constructs (`expect`, `advance`, `Val`, observations, `trace` and fault claims) are built as TypeScript nodes rather than text; no change to emitted code beyond layout in test modules
---
refactor(emit): test-domain constructs lower to nodes (1A.17)

Closes #3718. Part of #3676.

Built as nodes whenever their parts are nodes, otherwise today's text byte for byte:
- `expect <pred>`: `If { Unary(Not, Paren(cond)), InlineBlock([Throw(__bynkExpectFailure(…))]) }`. A comparison's detail is a left-nested `Add` of `Str` and `show(Paren(..))` calls. The location and source are the unescaped strings (`expect_location_raw`, `expect_source_text_raw`) in `Str` nodes, so the printer escapes them once; the text form escapes them with the same function.
- `advance`: `ExprStmt(Call(__bynkAdvanceClock, [amount]))`.
- Snapshot `expect` with an unknown type: a `Throw` node.
- `Val`, `mock_value` and the default helpers return nodes; `unchecked_construct_test_node` keeps `as any`.
- `Observation`, `Trace`, and the `with` predicate (an arrow with a `Multiline` body).
- `Faults`: `Paren(Await(Call(Paren(async Arrow))))` over a `TryCatch`, as the 1A.15 claim-scope wrapper.

**Departure from the proposal (accepted).** #3718 writes the `Faults` and `with` arrow bodies as `Block`, and mentions 1A.14's `InlineBlock`. `TsArrowBody::Block` is the compact one-line form and cannot hold a `TryCatch`; 1A.14 added no `InlineBlock`. Both use `TsArrowBody::Multiline`, the existing multi-line form.

**Still text (`// 1A:`).** The snapshot `expect`'s known-type arm (its `Json.encode` text comes from the serialisers, which take `&str`; recorded on the track as the "serialisers take `&TsExpr`" prerequisite) and the `undefined /* mock: unresolved type */` branch of `lower_val`. A claim or observation whose call or predicate is still text (a service call, say) keeps its text form.

**Goldens**, as ruled: 13 test-module files, nothing else.
- (a) depth, 4 files, whitespace-only: a match IIFE holding `expect` statements moves 2 columns left.
- (b) 1 file: the empty record `({  })` prints `({})`.
- (c) 8 files, 14 hunks: each `with` predicate arrow and each fault claim goes from one line to multi-line; the `if (!(…)) { throw …; }` stays on one line.
- (d) none. `expect`, `advance`, `Val` and snapshot lines are byte-identical.

**Gated probes** (against the state after #3761):

| Probe | Before | After |
|---|---|---|
| `ts_writes` | 1419 | 1407 |
| `stmt_text_lines` | 31 | 30 |
| `lowering_text_bridge` | 88 | 88 |
| `ts_any` | 38 | 39 (raised, see `raises:`) |
