---
level: patch
probes: ts_writes
changelog: The built-in kernel methods on connections, request views, cookies, `within`, `race`, `detach` and `Effect[Result]` combinators in generated code are built as TypeScript nodes rather than text, so redundant brackets round receivers are gone and twelve generated files print an enclosing closure with different layout; no change to what the emitted code does
---
refactor(emit): the inline and Effect kernel arms lower to nodes (1A.28)

Closes #3729. Part of #3676.

**What changed.** `lower_kernel_method_call`, the `Effect` kernels and the `EffectResult` combinators move, byte for byte, to `lower_call/kernel.rs` (Decision D); the sibling scalar and container kernels stay in `lower.rs` for 1A.29 and 1A.30. The arms written inline in the dispatcher now build nodes through `node_or_text`: `TestConnection` (`sent`, `closed`, `connection`), `Connection` (`send`, `close`), the `HttpResult` cookie ops, the request and claims views, `within`, `race`, `detach` and the four `EffectResult` ops. A text operand falls back to today's text byte for byte. `race` flattens its chain in source order as before, and every `note_*` call and the `exec` argument fire as before. The mismatch panics (#3164, finding #28) are unchanged, and tests assert their messages.

**Brackets.** A receiver is bracketed only where precedence needs it. The printer brackets a binary, a cast, an arrow, a conditional and an assignment as a member's object but not an `await`, a unary operator or a number literal, so `receiver_object` adds the brackets for those three. `(a + b).op()`, `(await f()).op()` and `(c ? x : y).op()` keep theirs.

**`detach` site.** The location and the error type name are `Str` nodes only where the printer's escape prints the bytes the text path does; a runtime check per site keeps the text path where they differ. The text path escapes an assertion location twice, so the two agree there only where escaping is the identity. A test covers every location shape the fixtures produce, non-ASCII included.

**Departures from the #3729 table, accepted.** `HttpResult.cookies` keeps the outer `Paren` round the `??`, and the cookie argument keeps its explicit `Paren` (in `withCookie` and as `Spread(Paren(arg))` in `clearCookie`). The only token difference in the goldens is therefore the dropped receiver brackets.

**Goldens, as ruled: 34 files blessed.** 83 redundant receiver bracket pairs dropped in 32 files (28 `Connection` calls, 19 `TestConnection` ops, 22 request views, 14 `(r) as any` inside `cookies`), and 12 enclosing IIFEs re-laid out (1769_send_in_match_guard, 1818_ws_agent_reads_its_key, 236_websocket_chatroom, 238_websocket_inbound_workers, 3041_ws_open_actor_sum, both 3051 `chat.ts`, 338_ws_broadcast_collect_all, 340_ws_broadcast_try, 3437_detach_shapes, 3277_http_cookies_write and 3446_typed_cookie_write). Whitespace-stripped text is identical apart from the dropped brackets. No bracket was gained, and the `withCookie`, `within`, `race`, `detach` and `Effect[Result]` lines are byte-identical in every fixture.

**Source maps.** Source maps are line-granular. In ten of the twelve re-layout files the enclosing closure now prints over more lines, so the map gains rows and shifts. The ten are 1769_send_in_match_guard, 1818_ws_agent_reads_its_key, 236_websocket_chatroom, 238_websocket_inbound_workers, 3041_ws_open_actor_sum, 3051_test_websocket_address, 3051_test_websocket_param_order, 338_ws_broadcast_collect_all, 340_ws_broadcast_try and 3437_detach_shapes. The mappings become more precise: in 1769 the nine lines of the closure mapped to source 12:6 each and now map to 13:8, 14:8 and 16:8. The Q2 rule is read as "no map change beyond what the ruled re-layout implies". Earlier re-layout slices most likely shifted maps the same way without detection, because the repo has no `.map` goldens.

**Probes.** `ts_writes` 1323 to 1322. `lowering_text_bridge` stays at 58, against the proposal's expectation that it would fall: none of these arms was a counted `into_text` bridge site, since each built its text with `format!`. `ts_any` stays at 38 (the cast and the `__r` parameter are named through `any_ty()`), `ast_importers` at 14 and `ts_text_idents` at 14.

**From the 1A.27 review (#3783).** The three stale doc comments in `lower_call/agents.rs` describe the node form, and `agent_construct_node` builds its deps argument only on Workers. No output change.
