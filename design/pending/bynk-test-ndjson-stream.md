---
level: patch
changelog: "`bynk test --format ndjson` streams the run as one pinned JSON event per line, each suite as it finishes, and the VS Code Test Explorer shows results suite by suite as they arrive (Test Explorer depth track, Slice D stage 1)"
---

## ADR: test-ndjson-result-stream
title: bynk test --format ndjson is a supported result stream
summary: The event set, its field order, the end-of-stream rule, and the stability promise; amends ADR 0098 D2
status: Accepted

**Context.** ADR 0098 D2 made the runner's NDJSON an internal protocol between the
generated entry and the driver, and left "a clean path to live streaming later".
The driver collected all of the runner's output before printing the one document,
so the Testing view showed nothing until `bynk test` exited. A consumer could not
get results as they were produced without scraping the runner's internal lines,
which would freeze their wording as a contract.

**Decision.** `--format` gains `ndjson`, a supported stream the driver writes as the
run goes. The events are the driver's own, not the runner's lines passed through, so
the runner's wording stays free to change. They are `run-begin {suites}`,
`suite-begin {name, kind, tests}`, `case {suite, …}`, `suite-end {name}`,
`run-end {passed, failed, skipped?, transport?}` and `error {kind, message?,
diagnostics?, stderr?}`. A `case` event is the document's case object, in the
document's field order, behind `type` and `suite`, so it carries `tier` and the
generative fields and gains any later optional `Case` field without a second
contract. An `error` event is the document's `error` object behind `type`. The
stream ends with exactly one of `run-end` or `error`: a project that does not
compile is a single `error` of kind `compile`; a runner that dies before `run-end`
ends the stream with an `error` of kind `runtime` carrying its stderr, after the
events it had produced; the driver's refusal of a run (an unknown target, no
runner) is an `error` too. A case a remote run skipped is not in the build, so it
is a `case` event with outcome `skipped` after the last `suite-end` and before
`run-end`, which also carries `skipped` and `transport`. `suite-begin.tests` is the
number of cases the suite will report. The runner's `unmatched` values (ADR on
`--case`/`--suite`) are not part of the stream: the driver prints them to stderr
and exits non-zero. A line of the runner's that is not a recognised event is
dropped, so stdout stays machine-clean.

Field order is part of the contract, and a consumer must ignore event types and
fields it does not know, so the set grows additively. `ndjson` is refused with
`--coverage`, whose block is a whole-run aggregate, and with `--no-run`, whose
discovery is a document, each as a single `error` event; `bynk test` also refuses
it across a workspace, as it does `json`. `--format json` is unchanged. The stream
is flushed event by event, and the driver kills the runner if its own stdout stops
accepting output. Under `--against dev`, which runs one process per `env` group, the
groups run as `json` children and the merged result is written as a stream when they
have finished. A suite is the unit of streaming in this stage: the runner reports a
suite's cases when it ends. Reporting each case as it finishes, with its duration,
changes the emitted `run` and regenerates every golden that carries it, and is the
second stage of the same proposal.

The VS Code extension runs with `--format ndjson`, applies each suite to the Testing
view as it ends, and keeps the one document for coverage. A driver that predates the
value rejects it, and the extension runs `json` instead for the rest of the session.

**Consequences.** `ndjson` is a public contract: the event set, the field order and
the end-of-stream rule are stable, and a consumer reads `error` as terminal. A
consumer that needs one result keeps using `json`. Results stream at suite
granularity until stage 2, so a single long suite still reports when it finishes.
