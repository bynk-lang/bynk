# 0758 — A tsc failure on generated TypeScript is a Bynk diagnostic at the source line it came from

- **Status:** Accepted (v0.521)

**Context.** A program that passes `bynk check` can still produce TypeScript
that `tsc` rejects (#2905, #2959, #3041, #3042). The error named a generated
line, and finding the Bynk construct meant reading the Worker. It also appeared
only in `bynk test`, the one command that runs `tsc` (`bynk dev` and
`bynk compile` strip types and never type-check).

**Decision.** (#3068) One code, `bynk.codegen.tsc_failed`, Error severity,
worded as a compiler bug with a please-report note; the TS code, message and
generated line are attached as notes. Each `error TS` record is mapped by line
through the emitted `.ts.map` (ADR 0103: line-level, nearest enclosing statement,
read backwards to the nearest mapped line at or before the error), to the span of
that Bynk line; the TS column is kept only in the note. Errors on one Bynk line
are one diagnostic, at most ten locations are listed. An error that cannot be
mapped (no map, runtime or prelude file) is still reported, at the project level,
never dropped. A failed `tsc` with no `error TS` record (a crash, out of memory)
is a toolchain failure reported as raw output, not a compiler bug. `bynk test`
keeps running `tsc` and now reports through this; under `--format json` the
failure is a `compile` error with the diagnostic lines and the raw output in
`stderr`. `bynk check --emit` is opt-in: after a passing check it generates the
Workers output into a temporary directory and runs one `tsc --noEmit`, with the
same runner ladder as `test` minus `tsx`, which does not type-check. `check`
without it, `compile`, `dev` and the editor never run `tsc`.

**Consequences.** No emitter, checker, grammar or runtime change, and the emitted
output is byte-identical. The `.ts.map` reader moves out of coverage's private use
into a shared one. Spans are statement-line granularity; sub-line maps remain
ADR 0103's escape hatch. The unmapped fallback is project-level, not the owning
module's first line.
