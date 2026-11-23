# 0889 — A generated test module is a tree, and its imports come from it

- **Status:** Accepted (v0.640.71)

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
ordered by `TEST_ORDER` — the same machinery the emitter's own modules use, so an
unused runtime name is never imported. A participant namespace import is kept even when the
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
from the 371 corpus modules. The 88 unused participant namespace imports are
kept (`import_shadow.rs` allows them by name, `is_kept_participant_namespace`).
`testgen/imports.rs`, 28 orphaned `RuntimeUse` queries with
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
