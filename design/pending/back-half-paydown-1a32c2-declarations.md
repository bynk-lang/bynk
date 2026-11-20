---
level: patch
changelog: The agent state-persistence functions (`__hydrate<A>State`, `__rehydrate<A>State`, `__encode<A>State`) are built as statement nodes rather than text, so their checks are laid out one statement per line (a ruled layout change in 149 generated files) and an `if` over a `for` gains braces (ruled, 69 files, 150 sites, semantically identical TypeScript); source maps are unchanged apart from the extra lines; what the emitted code does is otherwise unchanged
probes: ts_raw_sites_emit, ts_writes
---
refactor(emit): the agent state-persistence sites lower to nodes (1A.32c-2)

Closes #3733. Part of #3676.

**What changed.** `hydrate_fn_decl`, the rehydration checks and the encode function are nodes. `bynk_ts` gains `TsType::Mapped` (optional `+`/`-` readonly modifier, key, `keyof` operand, value), `TsType::TypeOf`, `TsType::Indexed` and `TsType::StringLiteral`, each with printer tests; `name_closure` treats `typeof s` as a use of `s` and binds a mapped type's key.

**Rulings.**
1. Goldens: 149 `.ts` files blessed. All are layout only (each check prints one statement per line); 69 files also gain braces around a `for` under an `if` (150 sites), the only token change.
2. Source maps: `carrier_source_map`'s two snapshots are re-pinned. The extra lines repeat existing anchors, the anchor sequence is identical across all 3162 corpus maps, and no mapping moved.
3. `deserialise_expr` keeps its text input for now, printed from a node; it moves onto `TsExpr` in 1A.38 (#3745).
4. Object keys stay quoted by the caller (`"n:order"`), as the messages table does.

**Moved on.** 6396 (the migration step function, whose body is `Pre::lower` text) to 1A.38 (#3745).

**Probes.** `ts_raw_sites_emit` falls by 3; `ts_writes` falls from 1200 to 1176 (live count after a trial merge of `origin/main`).
