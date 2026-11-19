---
level: patch
probes: ts_writes, lowering_text_bridge
changelog: Intrinsic and type statics (`List.empty()`, `Int.parse`, `Bytes.*`, a static on a declared type and others) are built as TypeScript nodes rather than text, so a match expression passed to one is indented by the printer; no change to what the emitted code does
---
refactor(emit): intrinsic and type statics lower to nodes; `bynk_ts` gains type arguments on calls (1A.20)

Closes #3721. Part of #3676.

**Goldens, as ruled.** 3 `.ts` files, whitespace only, group (a): a `match` IIFE that is the argument of a converted static is now a node, so the printer indents it relative to the call (`1427_property_where_matches_local_sum` `demo_gen.test.ts`, `3277_http_cookies_raw` `auth-api/handlers.ts`, `917_bytes_in_test_case` `blobs.test.ts`; 31 lines). `git diff -w` is empty and the whitespace-stripped text of each file is identical. Group (c) is empty (`Int.parse` prints on one line as before) and group (d) is empty.

**Probes.** `ts_writes` 1395 to 1377 and `lowering_text_bridge` 86 to 68 fall. `ts_any` does not move (UFCS reuses the list kernel's `any_ty()`), and `ast_importers` stays at 14.

**Departures.** None from the accepted decisions. Named for the record: `bynk_ts` printer contract: a `new` callee that reaches a call through its members is now bracketed (`new (f().C)()`), which no golden shows; and `TsLit::Raw` holds the two fixed `Int.parse`/`Float.parse` grammar regexes, the one place a 1A slice puts literal text in a node (Decision C).
