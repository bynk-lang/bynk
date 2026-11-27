# 1118 — A resource step that fails part-way records what it did before its failure is returned

- **Status:** Accepted (v0.831.1)

**Context.** #3999. A Worker's queue step makes every queue it has not yet attempted, then records them all. A create that failed returned its error from inside the loop, and the apply writes no commit for a failed step (1D.2b, #4001 Decision A). The queues made earlier in the step were on the account but not in the ledger. The next run found them and recorded them as reused, which `--prune` never deletes, so they were orphaned for good. The other kinds make one thing a step, so their failed step had nothing to lose.

**Decision.**

- `Applied` gains `stopped: Option<DeployFailure>`, set with `Applied::stopping`. `apply_kind` writes a stopped step's commits in order, then returns its failure. A step that fails before changing the account still returns `Err` with nothing to write.
- The queue step, on a failed create, records each queue it made before it as Bynk's own (`created`), then stops with the create's failure. Nothing more is asked of the account. The queues it found but did not make are not recorded by the failed run; the next run records them, as before.
- The success path is unchanged: the same calls, the same writes, in the same order.

**Consequences.** A failed queue deploy leaves a ledger that matches the account for what Bynk made, so `--prune` can clean up. `every_kind.apply.failures.txt` is unchanged, because its failing queue create is the first its step makes; `kind/queue.rs::the_queues_made_before_a_failed_create_are_recorded_as_created` pins the case, and fails without the fix. A kind that later makes several things in one step can stop the same way.
