# 0939 — The emitter delimits the shared part of `tests/main.ts` for the snapshot harness

- **Status:** Accepted (v0.678.1)

**Context.** #4235 stopped copying shared generated sources into every fixture's snapshots, but the fixed part of the generated `main()` in `tests/main.ts` has no delimiter the emitter writes, so a one-line change to it re-blessed every expected `tests/main.ts` (382 files).

**Decision.** The emitter writes `// bynk:shared-begin test-runner#<variant>` and `// bynk:shared-end` around the statements of `main()` that follow the per-build `modules` list. The variant names the optional parts the runner carries (`detach`, `generative`, `pg`, `remote`), or `base`. A build whose fixed part holds something of its own, the snapshot report, is not delimited. The test harness elides between the delimiters into one shared snapshot per variant and checks the real region against it byte for byte. The comments are inert; the emitted program is otherwise unchanged.

**Consequences.** A change to the `main()` template re-blesses only the `shared/test-runner@*` snapshots. The generated `tests/main.ts` of every project gains two comment lines.
