# 1033 — A PR's gated-probe moves are named by the PR's own pending files

- **Status:** Accepted (v0.769.1)

**Context.** The table gate (`greenfield_status_table_is_current`) and the growth gate excuse a probe when any pending file in the tree names it, and the tree holds every increment merged since the last stamp. A PR that moved a probe without naming it was excused by another PR's file, passed CI and the merge queue, and went red only on the stamp that consumed that other file (#4415 for three diagnostics probes, #4421 for `ts_writes`).

**Decision.** The existing `--check-growth` step, which already has the base revision, also runs the gated probes on a temporary checkout of the base (`git worktree add --detach`, so it needs only git and the commit) and on the PR's tree. Every probe whose reading differs must be named in `probes:`, and every growth probe whose count rose in `raises:`, by a pending file the PR adds or changes against the base (`xtask/src/pr_attribution.rs`). A probe only another file names is not excused, and the failure says so. A move the PR records itself, by rewriting the probe's row in the committed table to the new reading (`cargo xtask greenfield-status --apply`, the route for a PR with no increment), is the PR's own too. A stamp PR deletes pending files and rewrites the table but changes no code, so it moves no probe and owes nothing: the stamp path is unchanged. The table gate itself, which has no base revision, is unchanged.

**Consequences.** A PR that moves a probe fails its own CI unless its own pending file names it, so a stamp never inherits an undeclared movement. The gated probes run twice in that CI step, once on a checkout of the base. A probe with no reading at the base is newly introduced and has not moved.
