---
level: patch
changelog: Internal restructuring: `emit.rs` is now a thin facade over one file per declaration kind (agent handlers and dispatch, the agent class and the suite declarations move out); no output changes
---
refactor: `emit.rs` splits by declaration kind, part seven: agent dispatch, the agent class and the suite declarations (2.1g)

Closes #4086 (slice 2.1, the last part). Part of #3676.

No output changes. Three files come out of `emit.rs`, function bodies unchanged:

- `emitter/emit/agent_dispatch.rs` holds an agent class's handler methods (`emit_agent_handlers`), the Workers `fetch` dispatch (`emit_agent_dispatch`) and the agent factory (`emit_agent_factory`).
- `emitter/emit/agent.rs` holds `emit_agent`, the class constructor, the state record and the zero entries that seed it, with `state_field_zero` and `zero_value_ts`.
- `emitter/emit/suite.rs` holds the suite declarations `testgen` calls (`emit_suite_fn`, `emit_suite_decls`, `suite_base_codec_roots`, `emit_suite_late_codecs`).

`emit.rs` is now its `//!` doc, one `use super::*;`, the 14 `mod` lines and the re-exports. It has no function, struct or test module. The glob import is not for the facade: moved function bodies spell `super::tco::plan` and `super::commons_alias`, which resolve through it, so removing it would mean editing bodies. The re-exports are `pub use suite::*` (its items are `pub`), `pub(crate) use agent::*` (`emit_agent` is `pub(crate)`) and a plain `use agent_dispatch::*`. All three children import explicitly.

Five items become `pub(super)` because the parent or a sibling uses them: `AgentDispatchFacts`, `emit_agent_handlers`, `emit_agent_dispatch`, `emit_agent_factory` and `entry_fields_const`. So do the six fields of `AgentDispatchFacts`. `emit_agent_dispatch`'s function-length ceiling is re-keyed to `emit/agent_dispatch.rs` at its current value (315). `AST_IMPORTER_ORIGINS` gains the three files in the `emitter/emit.rs` group, in path order, so `ast_importers` is unchanged. The one comment line the move leaves behind, the `// -- v0.5 emission --` banner above `emit_agent_handlers`, is dropped. No gated probe moves, and the module-line ceilings and the track doc are not touched.

A separate, labelled doc commit follows the move. It changes comments only: the stale `ast_importers` note on `emit_type`, the over-long doc line on `AST_IMPORTER_ORIGINS`, the `emit_agent` doc block that sat above `agent_reads_key`, and "elsewhere in this file" wording in moved comments that now points to the wrong file.
