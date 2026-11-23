---
level: patch
changelog: Internal restructuring: the driver's `project.rs` loses its pipeline, per-unit work and output assembly to child modules; no output changes
---
refactor: split `bynk-driver/src/project.rs`, the pipeline (2.7b)

Closes #4108. Part of #3676.

No output changes. `pipeline.rs` takes `run_checks` and its phases, `unit.rs` the per-unit check and emission, and `output.rs` the build's output assembly. `project.rs` becomes a 92-line facade over seven children and no `project/` file is over 1,350 production lines, so `project.rs` leaves 2.0's ceilings file. `run_checks` and `build_output` keep their function-length ceilings (934 and 332) under their new keys. No gated probe moves.
