---
level: patch
changelog: Internal restructuring: `bynk dev`'s `dev.rs` splits into its run, events, stand-ins, bindings, serving, supervision and watch helpers; no output changes
---
refactor: split `bynk/src/dev.rs` (2.9)

Closes #4110. Part of #3676.

No output changes. `dev/run.rs` takes the serve loop, `DevOptions`, the selection and the pre-flight; `dev/events.rs` what `bynk dev` tells the user; `dev/stand_ins.rs` the stand-in Workers for foreign services; `dev/bindings.rs` each binding wired locally or proxied and the dev configs; `dev/serving.rs` the ports and serve arguments; `dev/supervise.rs` running, reading and stopping the children; and `dev/watch.rs` the watch helpers. The 52 unit tests move beside the code they test, the six `watch_tests` move whole into `dev/watch.rs`, and the shared helpers go to `dev/test_support.rs`. `dev.rs` becomes a 64-line facade, and 2.0's ceiling entry for it is deleted. `run` moves whole, and its function-length ceiling (420) is re-keyed to `dev/run.rs`. No gated probe moves. The probes that skip whole test files now also skip a module declared `#[cfg(test)] mod x;`, as `module_lines` does, so the shared test helpers in `dev/test_support.rs` are not counted as production. This is a measurement correction, and no gated probe's reading changes.
