---
level: patch
changelog: The VS Code Test Explorer runs and debugs a multi-item selection as one `bynk test` process with the repeatable `--case` and `--suite`, instead of one process per case or the whole project (Test Explorer depth track, Slice A follow-up)
---

## ADR: test-explorer-selection-as-a-set
title: The Test Explorer runs a selection as one process, narrowed by --suite and --case
summary: How a selection of cases and suites maps onto the set filter, and what happens against a driver that lacks it

**Context.** `--case` took one name, so Slice A ran a selection of several items as
one process per case for a handful of cases and as the whole project, filtered, for
more, and Debug could narrow only a single case. Each process repeats the compile,
gives its case its own root seed and cannot produce one coverage report. The driver
now takes a repeatable `--case` and a `--suite`, which combine as "and" (ADR on
`--case`/`--suite`).

**Decision.** A selection of several cases is one `bynk test` process, and Debug takes
the same set. The extension names suites by the driver's name (`suites[].name`, the
last part of the tree item's id) and plans the set from what is selected:

- every touched suite fully selected: `--suite` for each, which is exact;
- no touched suite fully selected: `--suite` for each and `--case` for the case
  names, exact for one suite, and for several it also runs a name another selected
  suite has, bounded by the selection;
- a mix of fully and partly selected suites: `--suite` alone, because `--case` would
  also narrow the fully selected suites, so a partly selected suite then runs whole.

Only the requested items are reported in each case, as before, so what a set runs
beyond the selection is never shown. One selected case stays `--case <name>`, so a
pinned seed still goes to a run of exactly one case, and a coverage run keeps its
single `--case` or whole-project process. A run against a target uses the set too
instead of the whole project. The per-case limit stays only for a driver without the
set filter.

A driver that predates the filter rejects a repeated `--case` or a `--suite` on stderr
before it runs anything. The extension remembers that until the binary setting
(`bynk.bynkPath` or `bynk.compilerPath`) changes, says so in the run output, and plans
the selection as the interim did.

**Consequences.** A multi-item selection is one compile and one root seed. A
selection that mixes whole and partial suites can run cases the user did not pick; they
are not reported. Coverage over a selection is still the single-case or whole-project
process, and waits for the coverage work of the same track.
