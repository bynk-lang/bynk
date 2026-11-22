---
level: patch
changelog: Internal hardening in the code generator; bynk-emit now denies wildcard enum arms, so a new enum variant fails the build at each emitter match that has to handle it
probes: workspace_lints
---
refactor(emit): remove the wildcard enum arms in lowering and opt bynk-emit into the workspace lints (1A.37b)

Closes #3738. Part of #3676.

Every `match` over an enum in `lower.rs`, `lower_ctx.rs`, `lower_stmt.rs`, `lower_list.rs` and `lower_call/` now names its variants, so adding a variant to `Ty`, `TypeRef`, `ExprKind`, `BodyMode`, `Statement`, `PredKind` and the rest fails the build where lowering has to decide what to do with it. With the arms gone, `bynk-emit` opts into `[lints] workspace = true`: the lint is effectively deny for the whole crate, test code included, and the 17 file-level `deny` attributes 1A.37a added are removed. No `allow`, `expect` or `deny` of the lint remains in the crate.

**How.** A match that decides per construct lists its variants (the `BodyMode` accessors in `lower_ctx.rs` all do). A question several sites ask is one exhaustive method: `Ty::{as_option, as_list, as_effect, as_result, scalar_base}` in `bynk-check`; `ExprKind::{chain_link, branching, refinable_literal}` (with `ChainLink`, `Branching` and `RefinableLiteral`) and `CommonsItem::as_actor` in `bynk-syntax`; `TsExpr::needs_receiver_brackets` in `bynk-ts`. Each has a unit test. A single-shape extraction is a `let ... else`, an `if let` or `matches!`. Arms that were impossible for a checked program keep their `unreachable!` or `panic!` and message; no new panic and no suppression.

**Carried from #3867.** The final arm of `deserialise_expr_via` is written as `other @ (A | B | ..)`. rustfmt cannot format that function's main match (a comment would be lost), so the arm and one other overlong line in the function are wrapped by hand in rustfmt's own layout.

**Output.** Byte-identical: every file the positive fixtures emit, in project and single-file form, source maps included, compared before and after. No golden, pinned client or `examples/storefront/web/src/api.ts` changed.

**Probes.** `workspace_lints` changes to `opted in: bynk-emit, bynk-model, bynk-ts`. `wildcard_arms` (a trend probe, so not declarable here) falls 643 to 544. `verbatim_sites` and every other gated probe is unchanged.
