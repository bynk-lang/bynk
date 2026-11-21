---
level: patch
changelog: The generated TypeScript client's conversions, route constants, contract table and `createClient` are built as trees, not text, so the client's layout is regular (one blank line between declarations, each route function on one line after its parameters); its tokens and source maps are unchanged apart from the one-line doc comment on `Client` becoming a block, and the contract table of a foreign-caller client (`bynk-client.ts`) gaining its trailing comma
probes: ts_writes, ts_raw_sites_emit, emit_print_calls
---
refactor(emit): the http_client raw sites become nodes (1A.38b)

Part of #3745. Part of #3676.

Four `TsStmt::raw(` sites in `bynk-emit/src/emitter/http_client.rs` become nodes: the `__toPublic_X` / `__toWire_X` conversions, each `__route_*` constant, `API_CONTRACTS`, and `createClient` with the `Client` type. `convs` is `BTreeMap<String, Option<TsDecl>>`, `convert` takes and returns a `TsExpr`, and `route`, `encoding`, `record_arg` and `multipart_arg` return nodes. The route and argument builders move first, unchanged, to `http_client/route.rs` so no ceiling rises. `FILE_TYPE` and `FILE_CODEC` stay as text (fixed authored snippet, no interpolation), as does the post-print `field: "__Wire_` replacement, recorded as the next text escape.

`bynk_ts` gains `TsParam.default` (`TsParam::with_default`), and a multiline object or array under `as` or `as const` now keeps its layout (a printer change the proposal did not name; it also gives the foreign-caller client's `CONTRACTS` table its multi-line form).

`ts_raw_sites_emit` falls by 4 (9 to 5: the four survivors and the migration step function, which is 1A.38c's). `ts_writes` falls from 771 to 715 and `emit_print_calls` from 55 to 44.
