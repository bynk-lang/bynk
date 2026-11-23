---
level: patch
changelog: Internal restructuring: the test harness decides which participant types a case names from the statements it built, and builds a `SignedCookie` actor's `Cookie` header as a value, instead of lexing and patching emitted TypeScript; no emitted output changes
probes: ts_writes
---
refactor: no TypeScript lexing or patching in the harness (1B.3c)

Part of #3676. Closes #4047.

`names_ident`, the harness's hand-written TypeScript lexer, is replaced by `bynk_ts::unbound_names_in` over the statements the case body and the suite's helpers were printed from (`lower_integration_case_body` returns them beside its text, as `suite_fn_decls` does). `merge_cookie` is replaced by a `CookieHeader` built from nodes and rendered once, with `SessionDriver::pair` building a template-literal node. The `replacen` that rewrote `const deps = __makeHarness();` is gone (the body is typed before the declaration is written), and so is the `insert_str(0, …)` that prepended the event counter (declared first when needed).

Output is byte-identical. `ts_writes` falls by 7.
