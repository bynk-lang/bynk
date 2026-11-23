---
level: patch
changelog: Internal restructuring: a generated test module takes its imports from the tree it builds, as every other generated module does, so it imports what its code uses and no more; the identifier audit and the import-shadow equality now cover those modules too
probes: ast_importers, ts_writes, emit_print_calls, emit_silent_fallbacks, import_prediction_fns
---
refactor: test modules are trees (1B.5e)

Closes #4054. Part of #3676.

The last part of 1B.5 (#4054, Decisions C, E, F, G and H). 1B.5a to 1B.5d turned
the test harness's text into `bynk_ts` nodes; this derives the generated test
modules' imports from those trees, widens both corpus audits to cover them, and
deletes the prediction machinery that is then orphaned.

**Imports (Decision E).** `emit_test_module` and `emit_integration_module` build
their runtime import with `derived_test_runtime_names` — `unbound_names_in` over
the finished tree, filtered by `is_runtime_export` and ranked by `TEST_ORDER` —
and then prune every unused import, as every other generated module has since
1A.36b-2. The participant namespace imports are kept, as ruled, until it is
shown that dropping one cannot change evaluation order.

**The splice (Decision C).** `bundle.mjs` records the runtime names each
`runtime/src/testing/` module imports as a fourth table column, and
`TestRuntimeModule::splice` carries them as `raw_using` names, so a fragment's
text cannot leave a module under-importing what it spells. A unit test holds each
module's column against the runtime exports its artifact spells.

**The audits (Decision F).** `import_shadow.rs` and `ident_audit.rs` cover every
module the emitter or the harness builds, with no test-module predicate.
`TEXT_CLASSES` gained no class: the two text identifiers the widened audit found
(a zero-entry object type and the event sequence's pre-increment) are nodes now.
With no text `Ident` left anywhere, 1A.40's Decision F lands: `expr_contains_await`
is structural again and the printer debug-asserts that an `Ident` is an
identifier path. `bynk_ts::raw_statement_texts` reads opaque statements at any
depth, so `destructure_named_participant_types`' assertion no longer has a
nested-raw blind spot.

**Decision G.** The six prediction-shaped predicates are renamed for what they
decide (`case_observes`, `case_reads_run_id`, `case_advances_clock`,
`target_hands_off_to_exec`, `target_has_timers`, `target_emits_events`); each
decides what the harness emits, never an import. The property draw's fallbacks
are missing checker facts and are reported as
`bynk.internal.missing_checker_fact`; a new negative fixture pins the checker's
own refusal of a binding it cannot generate (`for all xs: List[Count]` is
`bynk.val.needs_pin`), which is why the draw's remaining arms are faults rather
than a silent `undefined`. The two real absent values are a named `absent()`.

**Deleted.** `testgen/imports.rs` with its five unit tests, 28 `RuntimeUse`
queries with their notes, fields and 67 call sites, the 24 `*_RUNTIME_IMPORTS`
constants, `effect_runtime_imports`, `time_runtime_imports`, `note_predicate`,
`note_par_fan_out` and `derives_runtime_imports` with its test: about 790 lines.

**Probes (Decision H).** `emit_print_calls`, `import_prediction_fns` and
`emit_silent_fallbacks` scan all of `bynk-emit/src` again and read 0, 0 and 0;
`testgen_print_calls` is retired at 0, as `lowering_text_bridge` was.
`ts_raw_sites` stays split, because §8 has a row for each half, and
`ast_importers` keeps reading production emission alone, because the harness
reads the AST by design (it falls to 13: `runtime_use.rs` no longer names a
`PredKind`). `ts_writes` falls to 467. No ceiling rises.

Goldens: the ruled import-only class in all 371 `tests/*.test.ts`, and the ruled
1A.40 class `{  }` to `{}`. See the PR.

## ADR: generated-test-modules-are-trees
title: A generated test module is a tree, and its imports come from it
summary: Test modules derive their imports, and both corpus audits cover them

**Context.** A generated test module (`tests/<unit>.test.ts`) used to be a
`TsProgram` whose bodies were mostly opaque text: every case runner, every stub,
every system driver. That text hid the names it used from
`bynk_ts::unbound_names_in`, so the module's import list could not be read off
its own tree. It was predicted instead, from a fixed prefix and 31 `RuntimeUse`
flags the producers set as they emitted, and the prediction had to agree exactly
with what the text later spelled. ADR 0885 recorded that consequence when
test-harness generation moved into `bynk-emit::testgen`: the test modules keep
predicted imports, and their text stays outside the identifier audit, until the
bodies are nodes.

Prediction is the failure mode 1A.36 removed everywhere else. A flag set on one
path and not another under-imports and fails `tsc`; a flag set for a feature a
body never reaches over-imports. Both are invisible to the producer, because the
fact and its reader sit in different files.

**Decision.** A generated test module is a tree, and its imports come from that
tree. 1B.5a to 1B.5d converted the harness's text to `bynk_ts` nodes, leaving two
foreign-text sites, each a `TsStmt::raw_using` that declares the names it spells:
the test-runtime splice, whose names come from the generated module table, and an
authored test double's fixed body. The runtime import is then
`unbound_names_in` over the finished module, filtered by `is_runtime_export` and
ordered by `TEST_ORDER`, and an unused import is pruned — the same machinery the
emitter's own modules use. A participant namespace import is kept even when the
tree does not use it, because a participant module may run a side effect when it
loads and dropping the import would change evaluation order; that is a separate
question, not a paydown one.

Both corpus audits cover every module the emitter or the harness builds:
`ident_audit.rs` (no `TsExpr::Ident` holds expression text) and
`import_shadow.rs` (the tree's import set is the module's, and no runtime export
is spelled in code without being imported). With no text identifier left, the
printer debug-asserts that an `Ident` is an identifier path, and
`expr_contains_await` answers from the node tree alone.

**Consequences.** This supersedes ADR 0885's consequence that test modules keep
predicted imports and stay outside the identifier audit. A test module imports
what its code uses: 2,586 runtime-name occurrences that no tree used are gone
from the 371 corpus modules, and so are 88 unused namespace or default imports
the prune reaches. `testgen/imports.rs`, 28 orphaned `RuntimeUse` queries with
their notes and fields, the 24 orphaned `*_RUNTIME_IMPORTS` constants, both
import-group functions and the `derives_runtime_imports` gate are deleted:
nothing in the emitter decides an import from a flag any more.

The harness reads like production emission, so the probes that measured the two
apart stop needing to: `emit_print_calls`, `import_prediction_fns` and
`emit_silent_fallbacks` scan the whole crate, and `testgen_print_calls` is
retired. `ts_raw_sites` stays split because the track's exit table has a row for
each half, and `ast_importers` still reads production alone because the harness
reads the AST by design.

What the tree cannot see is now one rule with one net: a producer of foreign text
declares the names its text spells, and the widened `import_shadow.rs` fails at
the module and the name if it does not.
