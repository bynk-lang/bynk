---
level: patch
changelog: Internal restructuring: `bynk deploy` and `bynk dev` read each Worker's bindings from the build's binding model instead of parsing them back out of `wrangler.toml` and its sidecars; no output changes
probes: binding_kind_homes
---
refactor: the CLI's parse of the binding tables and sidecars is deleted (1C.3b)

Closes #4040. Part of #3676.

No output changes: `wrangler.toml` and every sidecar are byte-identical, and the three binding sidecars (`bynk-vectorize.json`, `bynk-containers.json`, `bynk-hyperdrive.json`) are still written for users and scripts; the deploy just stops reading them. `deploy/config.rs` loses the 14 binding stanza structs, the three sidecar readers with their six structs, the binding half of `read_resources` (now `read_worker`, which reads the settings the model does not hold: `exports`, queue consumers and `addresses`, and the secrets manifest) and `project_resources_for`'s second `WorkerLayout`, and falls from 1,853 to 999 production lines. `project_resources` takes the build's `WorkersBuild`, so the service graph and a group's members come from the layout the build used. Five error paths that guarded a damaged build tree are gone (a Vectorize or container missing from its sidecar, an unknown sidecar version three times over, a Hyperdrive kind other than `mysql`). `dev_containers` reads the model. The bridge of 1C.3a (the debug cross-check and its corpus test) is replaced by a permanent names-only guard in `bynk-tests`: for every Workers fixture, the `binding` or `name` of each array-of-tables entry of the rendered config, read as a plain `toml::Table`, is the set of the model's binding names less the singletons. The config tests about bindings now run over a model with their asserted values unchanged. `binding_kind_homes` 2 to 1, which meets the "places a binding kind is defined" row.
