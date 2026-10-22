---
level: patch
changelog: "`bynk test` takes a repeatable `--case` and a new `--suite`, so one process runs a chosen set of tests, and a `--case` or `--suite` value that selects nothing now fails the run instead of passing with nothing run"
---

## ADR: test-case-suite-filter
title: bynk test --case and --suite select a set of tests in one process
summary: The repeatable --case and --suite flags, how they combine, and the per-value error for one that selects nothing

**Context.** `--case` took one name and there was no way to select a suite, so no
consumer could run a chosen set in one process: the editor ran a multi-item
selection as one process per case, which repeats the compile, gives every case its
own root seed and cannot produce one coverage report. A filter that matched
nothing was a silent success (`0 passed, 0 failed`, exit 0), so a typo in a CI
command passed vacuously.

**Decision.** `--case` is repeatable and `--suite` is new and repeatable. Values of
one flag combine as "or" and the two flags as "and": a case runs when its suite is
among the `--suite` values (if any) and its name is among the `--case` values (if
any). `--suite` matches the name the JSON document reports (`suites[].name`, an
integration suite without its prefix) whatever the suite's kind. Because the pair
is a product, the flags cannot name an arbitrary set of `suite`/`case` pairs; a
consumer that needs one runs the product and filters the results. A value is the
whole argument, never split on a comma, since a case name can contain one. An empty
value is dropped and a repeated one runs once, which extends the old rule that an
empty `--case` is unset. A single `--case` behaves as before.

The set reaches the emitted test entry as one JSON value in `BYNK_TEST_FILTER`,
replacing `BYNK_TEST_CASE`. The entry, and only the entry, changes: it skips the
modules whose suite is not selected, calls each remaining module's unchanged
`run(name)` once per selected case name, and concatenates the results, so a module
is still one suite record holding its cases in the order the flags gave them, and
`suite-begin.tests` is unchanged. A module's `run` holds no setup, so calling it
once per name repeats nothing. A suite that `--suite` leaves out is omitted
entirely, with no `suite-begin`; `--case` still keeps an empty record for a suite
none of whose cases matched, as before.

Each value is judged on its own: a `--case` value that names no case in the
suite-filtered selection, or a `--suite` value that names no suite, is unmatched,
so `--case a --case typo` fails although `a` matches. The entry reports the
unmatched values in an optional `unmatched` object (`suites` and `cases`, each
present only when non-empty) on `run-end`, present only when a filter was given and
a value went unmatched, and exits non-zero. After the usual output the driver
prints `<program> test: no case matched --case "x"` or `no suite named --suite "x"`
to stderr for each; a rich run's entry prints the same itself. The document's shape
is unchanged. `--inspect`, `--coverage`, `--against` and `--seed` honour the set,
and `--no-run` discovery ignores both flags. `--against dev` runs one process per
`env` group, each seeing only its own cases, so the parent judges every value
against the `system` cases of all the groups before starting any, and the group
runs, told so by `BYNK_TEST_PARTIAL_SELECTION`, do not judge. An older `bynkc`
rejects a repeated `--case`, which a consumer reads as "sets are not supported".

**Consequences.** A script that relied on a filter matching nothing and passing now
fails, and the changelog says so. Results follow the order of the flags, not
declaration order, so a consumer keys them by `suite::case`. `run-end` gains an
optional field that a later result stream documents with the rest of the event
set; a consumer ignores fields it does not know. The test entry's text changes, so
every `tests/main.ts` golden is regenerated; no module golden changes.
