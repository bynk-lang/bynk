---
level: patch
changelog: Internal restructuring: `bynk-ts`'s printer (`printer.rs`) is split into modules by concern, with its tests in their own file; no output changes
---
refactor: split bynk-ts's printer into modules by concern (2.6b)

Closes #4091. Part of #3676. The optional 2.6c (splitting `printer/tests.rs` by concern) is deferred to Phase 3.

No output changes. `bynk-ts/src/printer.rs` (3,629 production lines, 7,000 test lines) becomes a 454-line parent over `printer/{stmt,compact,expr,precedence,multiline,types,decl,fragment}.rs`, and its test module moves whole to `printer/tests.rs`. No gated probe moves. `lib.rs` is unchanged.
