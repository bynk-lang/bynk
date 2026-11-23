---
level: patch
changelog: Internal restructuring: the per-Worker emission moves from `bynk-driver` into `bynk-emit`, rendered from the binding model; no output changes
---
refactor: the per-Worker emission moves into `bynk-emit` (1B.2d)

Follows #4028. Part of #3676.

No output changes. `WorkersEmit` and its parts (`emit_member`, `finish_worker`, the group entry and manifests, `stage_worker_extras`), `MemberParts`, `stage_binding_files` and `migratable_agents` move from `bynk-driver/src/project.rs` into `bynk-emit/src/emitter/workers_emit.rs`, and `emit_worker_handlers_barrel` joins its helpers in `emitter/commons_barrel.rs`. The driver builds a `WorkersEmit` from the checked project and its options and calls `emit_all`; it passes the `WorkerBindings` that `run_checks` derived once (with each container's build path located), so the emission decides nothing about which bindings a Worker has. `project.rs` falls by about 1,000 lines. `bynk-emit` takes no JSON dependency: the group's inspect listing and agents manifest, which used `serde_json::to_string_pretty`, are written by hand in the same layout, and a test holds each to `serde_json`'s output (`serde_json` is a dev-dependency of `bynk-emit`, as the secrets and contracts tests parse their manifests). The moved code's wildcard arm over `ContextMessageBundle` is exhaustive, as `bynk-emit`'s lints require. No gated probe moves.
