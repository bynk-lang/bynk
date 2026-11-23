---
level: patch
changelog: Internal restructuring: the driver's `project.rs` loses its options, entry points, deps plan and tests to child modules; no output changes
---
refactor: split `bynk-driver/src/project.rs`, outer layer (2.7a)

Part of #3676. Slice 2.7a of #4108.

No output changes. `options.rs`, `entry.rs`, `in_memory.rs` and `agent_deps.rs` take the build options, the on-disk and in-memory entry points and the Durable Object deps plan; both test modules move whole to `project/tests.rs` and `project/entry_scope_tests.rs`. The parent re-exports every moved public item, so no caller changes. `project.rs` falls from 4,312 to 2,875 production lines, under the 3,000 exit target, and its ceiling is lowered to match. No gated probe moves: `options_sources` is re-pointed at `project/options.rs` and still reads `present`.
