# 0516 — The web extension analyses each open file on its own, hides the cross-unit cascade, and says so

- **Status:** Accepted (v0.359.1)

**Context.** The wasm entry points analyse one in-memory source. On the repository's own examples, a context that `uses` a sibling commons gets `bynk.uses.unknown_commons` and then a `bynk.resolve.unknown_type`/`unknown_function`/`unknown_name` for every name the import would have provided. A test file gets `bynk.suite.unknown_target`. None of these are errors in the project.

**Decision.** Slice 1 analyses each open file on its own. The filter's rules come from analysing every `bynkc/tests/fixtures/positive` source file alone. Those files all check clean as projects, so whatever the single-file analysis reports for them is an artefact. Before publishing, the worker drops:

- **The cross-unit roots, always:** `bynk.uses.unknown_commons`, `bynk.consumes.unknown_context` and `bynk.suite.unknown_target`. Each names a unit in another file.
- **The single-file artefacts, always.** `bynk.tier.system_needs_wire` needs the suite's target context. `bynk.stability.experimental` needs the `bynk.toml` `[unstable] allow` list. `bynk.project.inconsistent_commons_name` needs the real path, and the in-memory path is synthesised.
- **An unreadable binding module (`bynk.adapter.no_binding`, "could not be read"), always.** The worker reads no files. An adapter missing its `binding` clause is still reported.
- **The cascade, in a file with a root, or in one file of a unit split across several** (its `context`/`commons`/`adapter` name doesn't end in its file name). The cascade is `bynk.resolve.*`, `bynk.consumes.*`, `bynk.given.unknown_capability`, `bynk.event.unknown_subscription`, `bynk.generics.type_arg_mismatch`, `bynk.types.cannot_infer_*` and `bynk.types.uninferable_element_type`.

In a self-contained file these are real errors and are kept. With these rules no positive fixture shows an error on its own. A status bar item reads **Bynk (web, single-file)**, and its tooltip says that names from other units aren't resolved and their errors are hidden. Slice 2 loads the project through `vscode.workspace.fs`, and the filter goes away there.

**Consequences.** A file that uses or consumes another unit shows no false errors on the web. A genuine misspelled name or inference failure in that same file is hidden too, until project loading lands. Other type errors and parse errors are always reported.
