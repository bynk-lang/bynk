---
level: patch
changelog: Internal restructuring: `emit.rs` splits further by declaration kind (services and contexts move out); no output changes
---
refactor: `emit.rs` splits by declaration kind, part three: services and contexts (2.1c)

Part of #4086 (slice 2.1; the last part, 2.1g, closes it). Part of #3676.

No output changes. `emit_service`, the context `deps` interface, the `make` surface, the cross-context capability namespaces, `schema_version_test` and the actor-sum types a service's `deps.who` uses move into `emitter/emit/service.rs`, function bodies unchanged. `emit.rs` re-exports it with `pub use service::*` (the file has `pub` items that `testgen` and `workers` use), so every `emitter::emit::X` and `emitter::X` path resolves as before; the child imports explicitly. Five functions become `pub(super)` because the parent or a sibling calls them: `cap_ref_ty`, `workers_env_ty`, `actor_sum_ts_union`, `build_deps_object_ty_with_surface` and `effective_given`. `emit_service`'s function-length ceiling is re-keyed to `emit/service.rs` at its current value (461). `AST_IMPORTER_ORIGINS` gains `service.rs` in the `emitter/emit.rs` group (which also gathers the earlier emit rows, in path order), so `ast_importers` is unchanged. No gated probe moves, and the module-line ceilings and the track doc are not touched.
