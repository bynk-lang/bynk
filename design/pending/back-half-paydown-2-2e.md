---
level: patch
changelog: Internal restructuring: the call dispatchers move out of `lower.rs` into their own module, which completes the `lower.rs` split
---
refactor: split lower.rs, part five (2.2e)

Closes #4087. Part of #3676.

The last part of the `lower.rs` split. A pure move: the call dispatchers
(`lower/calls.rs`: the method-call dispatcher, UFCS and cross-context calls,
function calls and the `settle` helpers) leave `lower.rs`, which re-exports them,
so no caller changes. `list_kernel_tests` moves into `lower_list.rs`, whose
kernel it tests. `lower.rs`' module doc is rewritten to describe the core and list
the families, and `lower_call/mod.rs`' doc now points at `lower/calls.rs`.
Function bodies are byte-identical; output is unchanged.

No gated probe moves: `ast_importers` is unchanged because the new file is
entered in `AST_IMPORTER_ORIGINS` (origin `emitter/lower.rs`), so no `probes:`
line. No ceiling is touched.
