# 0514 — The JSON coverage block lists covered lines as well as uncovered ones

- **Status:** Accepted (v0.358)

**Context.** #2104 surfaces `bynk test --coverage` in VS Code's native coverage
UI. A file's entry in the `--format json` `coverage` block (#854) named its
uncovered lines but not its covered ones, so an editor could mark only the red
lines, while VS Code's detailed coverage expects every executable line.
Separately, under a relative input (`bynk test .`, which is how the editor runs
tests), each `path` collapsed to the bare file name (`window.bynk` instead of
`src/window.bynk`). The map sources are absolute, and a relative root never
prefixes them.

**Decision.** Each `files[]` entry gains `covered_lines`, the covered
executable lines, 1-based and ascending. With `uncovered` it lists every
executable line. The field is named `covered_lines` rather than the proposal's
`covered`, because `covered` is already the entry's count. It is the entry's
last field and always present on a coverage run, so the block's existing field
order is unchanged and a consumer that predates it is unaffected (DECISION A).
Coverage stays whole-run: per-test attribution would need one run per case and
is out of scope (DECISION B).

Paths are relativised against the project root's canonical form as well as the
form given, so `bynk test .` and `bynk test /abs/project` report the same
project-relative paths.

**Consequences.** The editor's coverage profile maps each entry to a
`FileCoverage`, with statement totals from `covered`/`lines` and per-line
`StatementCoverage` from the two lists. With a driver that predates
`covered_lines`, it marks only the uncovered lines. The editor also names the
project by its absolute path on a coverage run, so an older driver's paths
resolve too.
