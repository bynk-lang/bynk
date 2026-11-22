---
level: patch
changelog: Generated modules and Worker entry files no longer import names they do not use; each import list now follows from the module's code
probes: emit_print_calls
---
refactor(emit): the type imports and the Worker entry headers come from the built tree (1A.36b-2)

Closes #3737. Part of #3676.

The second half of #3737 Decision A. A unit module's sibling, used-commons and consumed-context imports are no longer collected by walking the AST ahead of the body: they are the names the built body uses and does not declare (`unbound_names_in`), each resolved to its home by the existing `record_name_ref` and fed to `emit_project_imports` and `emit_cross_context_namespace_imports`. A consumed context's namespace is imported iff the body uses it. The Worker `index.ts` and `compose.ts` take the runtime names their finished tree uses, in the order those files have always had (`WORKER_ORDER`).

**Deleted.** `collect_external_references`, the `collect_refs_in_*` walkers, `ExternalReferences::{imply_names, settle_implied}`, `record_ty_refs`, `body_words` (which takes `emit_print_calls` from 28 to 27), and in the Worker modules `platform_binding_imports`, the signed-cookie, email-identity and signature-type predicates, `append_missing_bindings` and the `push_*_imports` helpers.

**`TsProgram::prune_unused_imports`.** Imports the emitter or the driver adds for a body that may never reach them (a fixed `handlers`, a provider namespace, an agent's binding module, a boundary codec, the locale `render`) are kept only if the tree uses them. It closes the Worker entry and compose, the unit modules' extra imports and the bundle composition root, so `import_shadow.rs` can assert tree == imported for every module bar the driver's `tests/*.test.ts` (1B.2). The 137 names the printed body spelled that the tree could not see (36b-1's residue) are 0: the queue entry's `msg.body as __JsonValue` is a node, an agent's DO-side deps expression declares the namespaces it spells (`AgentDepsPlan::names`), and the barrel's merged helpers name their file aliases.

**Decision C's `RuntimeUse` audit.** No note is deleted. The only readers this slice removed were the Worker headers', and the driver's test modules (`system_support.rs`, `integration_module.rs`, `test_module.rs`, `test_deps.rs`) still read every note that remains, or it gates emission (`plain_json`, `virtual_time`, `json_codec_qual`, `json_codec_roots`). A Worker module's own `RuntimeUse` is now a sink the shared codec producers write into and nothing reads. The per-note disposition is in the PR.

**Review carry-overs from 36b-1.** `name_closure` treats a capitalised word before `:` as an object key only inside `{ }`, so the true branch of `A extends B ? C : D` is a use; it reads `typeof ns.member` in type text (a namespace spelled only there). `import_shadow.rs` asserts that no module the emitter builds spells a runtime export it does not import, the net under the derived headers. `bundle.mjs` reads `export { type Foo, Bar }` as a type-only `Foo`.

**#3807.** A boundary cast's `lib` on the bundle target is bound because the body uses it; the `KNOWN` entries are gone and a test pins it. A namespace import of a split context still names one file (the smallest path), so `2704_recursive_cycle_across_files` is masked, not fixed.

**Not derived.** The driver's `tests/*.test.ts` modules keep their predicted imports until 1B.2.

**Probes.** `import_prediction_fns` stays 0; `emit_print_calls` falls 28 to 27; `ts_raw_sites_emit` stays 4. No gated probe rises.
