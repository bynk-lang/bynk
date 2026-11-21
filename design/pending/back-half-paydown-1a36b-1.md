---
level: patch
changelog: Generated modules no longer import runtime names they do not use; each module's runtime import now follows from its code
probes: ts_writes, ts_text_idents, import_prediction_fns
---
refactor(emit): the runtime header and the fixed-prefix imports come from the built tree (1A.36b-1)

Part of #3737. Part of #3676.

A unit module's runtime import is no longer predicted before its body exists. `write_header` and `write_header_single` take the statements the module has built and import each runtime name the tree uses and does not declare (`unbound_names_in`), through the existing `split_runtime_import`. The fixed prefix (`Ok`, `Err`, `Some`, `None`, `Result`, `Option`, `ValidationError`) is imported only when used, and a helper group (`INT_RUNTIME_IMPORTS`, `BYTES_RUNTIME_IMPORTS` and the rest) now brings in only the helpers the module calls. That is a golden change: it removes unused names from import lines, and a module whose runtime import empties loses the line and the blank line that followed it. Nothing is added or reordered.

**Deleted.** The header's predicates and the code behind them: `type_ref_mentions`, `commons_mentions_type`, `file_mentions_json_error`, `file_mentions_queue_result`, `file_mentions_http_result`, `file_mentions_connection`, `uses_json_codec`, `commons_need_rehydrate`, `commons_state_runtime_names`, `agent_state_runtime_names`, `commons_have_held_storage`, `inspect_agent_imports`, `trace_step_imports`, `cookie_runtime_imports` and their tests. `agent_needs_rehydrate` stays, renamed `agent_gates_rehydration`, because it decides the `commitState` an agent emits. `import_prediction_fns` reads 0.

**Decision C's `RuntimeUse` audit.** This slice deletes the notes whose only reader was the runtime header's import list, together with their call sites: `note_icu` (4), `note_session_cookie` (2), `note_trace_step` (1) and `note_trace_binding` (1). Every other note stays, because a Worker module (`workers.rs`, `workers_entry.rs`) or the driver's test modules (`system_support.rs`, `integration_module.rs`) still reads it, or because it gates emitting a helper (`plain_json`, `virtual_time`, `json_codec_qual`/`json_codec_roots`). That is the deletion Invariant 4's "fire exactly as today" rule reserves for this slice.

**Carriers.** The cached constant's accessor and the `Codable` base-type witness hid runtime names in text, so `TsExpr::NullishAssign` builds the first from nodes and `TsExpr::RawUsing(text, names)` declares the second's uses, taken from the nodes it was printed from. `name_closure` reads a type written as text (`Option<A>`, `Result<T, __BoundaryError>`), a type parameter's constraint and default, and a name the module imports that shares a global's (`Request`, `Response`).

**Not derived.** The driver's `tests/*.test.ts` modules keep their predicted imports until 1B.2. They never pass through the emitter's header (the driver builds them), and `runtime_header::derives_runtime_imports` gates the derivation off for the unit kinds that are theirs. The Worker `index.ts` and `compose.ts` (`workers_entry.rs`, `workers.rs`) and the type-import collection (`collect_external_references`, `body_words`) are 1A.36b-2: the Worker entry and compose headers move in 36b-2.

**Probes.** `import_prediction_fns` falls 8 to 0; `ts_text_idents` falls 13 to 12 (the cache accessor's `Ident(format!(`); `ts_writes` falls 681 to 680. No gated probe rises.
