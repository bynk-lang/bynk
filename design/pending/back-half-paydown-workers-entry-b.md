---
level: patch
changelog: Internal restructuring: the Worker entry generator's HTTP family (route dispatch, request field specs and CORS and security policy) moves into modules of its own; no output changes
---
refactor: split the Worker entry's HTTP family out of workers_entry.rs (2.4b)

Part of #4089. Part of #3676.

No output changes. `field_specs`, `http_policy` and `http` move out of `bynk-emit/src/emitter/workers_entry.rs`, which falls from 2,937 to 1,165 lines and leaves the module-size ceilings. `emit_http_route_dispatch`'s function-length ceiling is re-keyed to `workers_entry/http.rs`. `ast_importers` gains three origin rows and reads the same.
