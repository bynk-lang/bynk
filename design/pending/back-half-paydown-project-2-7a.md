---
level: patch
changelog: Internal restructuring: the driver's `project.rs` loses its options, entry points, deps plan and tests to child modules; no output changes
probes: ts_writes, ts_any
---
refactor: split `bynk-driver/src/project.rs`, outer layer (2.7a)

Part of #3676. Slice 2.7a of #4108.

No output changes. `options.rs`, `entry.rs`, `in_memory.rs` and `agent_deps.rs` take the build options, the on-disk and in-memory entry points and the Durable Object deps plan; both test modules move whole to `project/tests.rs` and `project/entry_scope_tests.rs`. The parent re-exports every moved public item, so no caller changes. `project.rs` falls from 4,312 to 2,875 production lines, under the 3,000 exit target, and its ceiling is lowered to match. `options_sources` is re-pointed at `project/options.rs` and still reads `present`. A correction to measurement, not paydown: the family probes now skip whole test files in both crates, because a module's tests moving to `foo/tests.rs` would otherwise raise `ts_writes` with no production line changed. `ts_writes` reads 462 (was 464) and `ts_any` 21 (was 24); both fall because `bynk-emit` already had whole test files counted as production.
