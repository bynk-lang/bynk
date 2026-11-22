---
level: patch
changelog: Internal hardening in the code generator; a new enum variant now fails the build at each emitter match outside lowering that has to handle it
---
refactor(emit): remove the wildcard enum arms outside lowering (1A.37a)

Part of #3738. Part of #3676.

Every `match` over an enum in the emitter files that lowering does not own now names its variants, so adding a variant to `Ty`, `TypeRef`, `ExprKind`, `CommonsItem`, `BindingSource`, `StoreKindIr` and the rest fails the build where the emitter has to decide what to do with it. This is the first half of 1A.lints; 1A.37b does `lower.rs`, `lower_ctx.rs`, `lower_stmt.rs`, `lower_list.rs`, `lower_call/` and the crate's opt-in.

**How.** An arm that did different work per variant lists the variants. A match with one meaningful arm and a default (`X(a) => Some(a), _ => None`) became a `let ... else` or an `if let`, which asks the same question without a list. Where several sites ask the same question of an enum, it is one exhaustive method: `CommonsItem::{as_service, as_agent, as_capability, as_messages, handlers}` in `bynk-syntax`, `StoreKindIr::{map_value, log_element}` in `bynk-model`, and two emitter helpers, `peel_list_option` (a field's `List`/`Option` layer) and `type_base_is` (a type's base, directly or through a refined or opaque alias). Each has a unit test. Arms that were impossible for a checked program keep their `unreachable!` or `panic!` with the same message.

**Guard.** Seventeen of the cleaned files carry a file-level `deny` of the lint until the crate opts in (1A.37b removes them). `emitter.rs` is cleaned but has none, because an inner attribute there would cover the lowering modules it declares.

**Carried from #3864.** A test pins `name_closure`'s reading of a capitalised tuple label (`[First: A]`) and a function-type parameter (`(Req: X) => Y`) as uses of `First` and `Req`, which is conservative. `typeof import("./x")` no longer records the keyword `import` as a used name, with a test.

**Output.** Byte-identical: every file the positive fixtures emit, in project and single-file form, source maps included, compared before and after (13,494 files).

**Probes.** `wildcard_arms` (a trend probe, so not declarable here) falls 730 to 643. `verbatim_sites` and every other gated probe is unchanged. `workspace_lints` is untouched; it changes in 1A.37b.
