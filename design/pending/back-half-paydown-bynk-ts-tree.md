---
level: patch
changelog: Internal restructuring: `bynk-ts`'s tree (`program.rs`) is split into modules by node family; no output changes
---
refactor: split bynk-ts's tree into modules by node family (2.6a)

Part of #3676. Part of #4091.

No output changes. `bynk-ts/src/program.rs` (3,682 production lines) becomes a 441-line parent over `program/{stmt,expr,types,decl,verbatim,spans}.rs`; no probe that is gated moves. `lib.rs` is unchanged, because the parent re-exports every moved item.
