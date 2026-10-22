# 0583 — The result stream reports each case as it finishes, with its duration

- **Status:** Accepted (v0.388.1)

**Context.** The supported `--format ndjson` stream (stage 1) was written at suite
granularity: a module's `run` collected every case's result and the entry emitted
`suite-begin` and the cases only once the suite had finished, so one long suite
reported nothing until it ended, and a case carried no timing. A `system` suite run
against a deployment, with `--settle` retries, is where that matters.

**Decision.** Each module's `run` takes an optional second argument, a callback it calls
with each case's result as that case finishes, before the next one runs; it still
returns the collected results. It times each case with the host's clock (`performance.now()`
where there is one, else `Date.now()`, never the test harness's `Clock`), and the result
carries `durationMs`, rounded to a hundredth of a millisecond. The module also exports
the names of its cases, in dispatch order, so the entry can count what a filter selects
(a name once per case that bears it) before anything runs. The entry therefore emits
`suite-begin` first, with `tests` the number of cases it will report, then each `case`
as it finishes, then `suite-end`; the human output prints each case as it finishes too.

The stream's `case` event gains `durationMs` as its last field, after the document's case
fields, so a later optional `Case` field still lands before it; it is omitted for a skipped
case, which did not run. The JSON document does not carry it. The document is the
deterministic surface (two runs print the same bytes), and a measured duration is not, so
`durationMs` is the stream's alone; the driver reads it from the runner's event and
leaves it out of the document. `suite-begin.tests` keeps its meaning, the suite's case
count, and now holds before the first case instead of after the last. The rest of the
event set, its order and the end-of-stream rule are unchanged. Changing the emitted `run`
regenerates every golden that carries it, in a commit of its own.

The VS Code extension applies each case to the Testing view as its event arrives, and
reports its `durationMs` as the item's run duration. A driver that streams at suite
granularity (stage 1) sends a suite's cases together at its end, which the extension
treats the same way.

**Consequences.** A consumer sees a case's result before the next one runs and may size
progress from `suite-begin.tests`. A consumer that wants timing reads the stream; the
JSON document stays byte-stable. The module's `run` signature and the exported case names
are part of the generated test modules, not a public interface.
