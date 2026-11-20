---
level: patch
probes: emit_diagnostics, lowering_text_bridge, emit_silent_fallbacks, diagnostic_coverage, explained_codes
changelog: A checker fact the code generator needs and does not find is reported as an internal-error diagnostic, `bynk.internal.missing_checker_fact`, at the source span instead of being written as `any`, `undefined` or a panic; no program the checker accepts is affected and the generated TypeScript and source maps are unchanged
---
refactor(emit): one policy for a missing checker fact (1A.34)

Closes #3735. Part of #3676.

**The policy.** When the checker has not recorded a fact lowering needs (a type, a callee, an argument, a field default), the emitter reports an internal error at the `.bynk` span. It no longer writes `any` in a type position or `undefined` in a value position, and it no longer panics.

- `RuntimeUse` gains an internal-fault sink with its own interior mutability (`RefCell<Vec<InternalFault>>`), the one per-module accumulator every lowering context already shares. `LowerCtx::missing_fact(span, what)` records a fault and returns the `undefined` identifier node (`missing_fact_ty` returns `never` in a type position), so lowering continues and every fault of the unit is reported, not only the first. A `FaultSite` carries the same two things to a helper that holds no context (the `Query` arms, `decode_map_key`).
- `emit_project` returns `Result<_, Vec<InternalFault>>` (and the single-file `emit` likewise). The driver turns each fault into a `bynk.internal.missing_checker_fact` diagnostic at its span, attributed to the file, and drops that unit's output. The test-module and integration-module emitters read the same sink after their case bodies lower.
- The new code has a `bynk explain` entry: it says this is a compiler fault, points at the construct, and says where to report it. The explain gate lists it among the codes whose example is not Bynk source (no program provokes it).
- The one deliberate widening (#2795: a generic method's receiver type argument that is not in scope in every module) is `unnameable_receiver_type_arg()`, documented as a widening. It is not a fallback.

**The sweep.** Every fallback in `bynk-emit` is one of: a missing fact (reported), the named widening, or a legitimate default (an `Option` in the Rust, rendered once as `undefined_node()`). The four missing-fact `any` sites (`list_ok_elem`, `sequence`, `traverseAll`, the receiver type argument) are reported; `mock_value`, the test-service argument defaults, the state-field zero value, and the `checker should already have typed it` panics and expects in the lowering are converted; the full classification is in the PR. `CallSitePrincipal::of`'s fallback is a classification and not a missing fact, so it keeps its `debug_assert`.

**New growth probe `emit_silent_fallbacks`.** Counts a fallback position (`unwrap_or_else`, `unwrap_or`, `map_or`, `_ =>`, `None =>`, `Err(_) =>`) followed on the same line by an `any` or `undefined` construction, in `bynk-emit` production source; the pattern is defined in the probe's doc. It read 15 on `main` and reads 0. A new fallback of that shape needs a `raises:` with a reason.

**Output.** Byte-identical, including source maps: every positive fixture's `.ts` and `.map` (19,392 files, 3,162 maps) matches `main`, and no fixture reports a fault.

**Probes.** `emit_silent_fallbacks` is new (0). `lowering_text_bridge` falls 44 to 43 (the `Val[T]` unresolved-type text fallback is now a reported fault). `emit_diagnostics` reads `bynk-emit=4/5` (the new code's definition lives in `bynk-emit`, which emits it). `explained_codes` rises by the one new entry, and `diagnostic_coverage` reads `asserted 866/896` with `unasserted` unchanged at 30: unit tests in `bynk-emit` and the driver name the code, so it needs no exemption. `ts_any` stays 35: the four missing-fact sites shared the single `any_ty()` definition the probe counts, so removing their calls does not move it. `ast_importers` and `ts_text_idents` do not move.
