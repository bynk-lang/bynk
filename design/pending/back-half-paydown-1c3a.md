---
level: patch
changelog: Internal restructuring: `bynk deploy` and `bynk dev` take each Worker's binding model from the compile they run, and check it against the configuration it wrote; no output changes
---
refactor: the CLI declares each Worker's bindings from the build's `WorkerBindings` (1C.3a)

Part of #4040. Part of #3676.

No output changes. `ProjectOutput` gains `workers: Option<WorkersBuild>`, the `WorkerBindings` and `WorkerLayout` a Workers build wrote its configuration from, and `workers::compile_built` returns it (`compile_once_reporting` is a wrapper). `bynk deploy` and `bynk dev` hold it, and pass it to `project_resources_for`, `local_edge_keys`, `local_session_keys` and `bind_for_dev`; the watch loop replaces it only when a rebuild succeeds, as it replaces the build. `deploy/declared.rs::declared_bindings` fills the sixteen binding fields of `Resources` from one Worker's bindings through one exhaustive `match` on `BindingSource`, keeping today's rules: a `ForeignService` whose script is one of the build's own is still a project edge, `MySql` without `tls` declares `required`, and a database's migrations are its staged `.sql` files. Today's parse of `wrangler.toml` and the sidecars stays as the bridge: debug builds compare the two for every Worker, and a `bynk-tests` test compares them over every Workers fixture (456 configurations). Nothing reads the projection yet; 1C.3b deletes the parse. No gated probe moves.
