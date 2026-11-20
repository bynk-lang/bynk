---
level: patch
changelog: The agent class and its history-property driver are built as TypeScript declaration nodes rather than text, so a generated agent class no longer ends with a blank line before its closing brace (a ruled layout change in 149 generated files); the four generated files that carry a history driver also lay it out one statement per line and separate the members of its object types with `;` instead of `,` (ruled, semantically identical TypeScript); source maps are unchanged apart from the driver's extra lines; what the emitted code does is otherwise unchanged
probes: ts_writes, ts_any, ts_raw_sites_emit, ts_text_idents
---
refactor(emit): the agent class and history driver lower to declaration nodes (1A.32b)

Part of #3733. Part of #3676. #3733 closes with 1A.32c.

**What changed.** `emit_agent` returns a doc comment and an exported `Class` over typed fields (`state`, `__key!`, `private __env`), a node constructor and every method (`loadState`, `commitState`, the handlers, the WebSocket methods, `fetch` and the WebSocket dispatch). Member spans and the class's closing span replace the carrier's `SourceMapBuilder`, its `record` sites and its `nested_map`. `emit_agent_history_driver` returns an exported `async` `Function` with a `try`/`finally` body, built from nodes, with the key's zero value a node too. `bynk_ts` gains `TsClassField.definite`, `TsStmtKind::Break`, `TsParam.rest` and `TsExpr::Spread` as a call argument (as well as an array item), each with printer tests; `TsClassField::new` and `TsParam::new`/`optional` let the existing literals stay within their length ceilings.

**Rulings.**
1. Goldens: all 149 blessed. 145 lose the blank line before an agent class's closing `}` and are otherwise byte-identical. The 4 with a history driver also take the printer's layout and `;` instead of `,` between type-literal members; with `;` and `,` equated they are token-identical.
2. Source maps: `carrier_source_map.rs`'s wallet snapshot tail grows from 39 to 57 entries. The carrier part is unchanged; the growth is the driver's lines inheriting the agent span, from the ruled re-layout. A corpus comparison of every positive fixture's maps against `main` found no file with identical TypeScript and different mappings.
3. `TsExpr::Spread` is legal as a call argument, guarded elsewhere. The dispatch's `...__decodeAgentArgs(..)` moved onto it with byte-identical output.

**Probes.** `ts_writes` falls, `ts_any` 38 to 35 (the driver's four `any` lines are one helper), `ts_raw_sites_emit` falls by 2 (the agent carrier and the history driver), `ts_text_idents` falls by 1.
