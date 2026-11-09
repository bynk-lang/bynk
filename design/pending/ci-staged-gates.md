---
level: patch
changelog: CI runs in stages matched to a PR's state — fast checks on drafts, the full three-OS gate once a PR is ready for review, and a thin integration check in the merge queue
---

## ADR: ci-staged-gates
title: CI gates by stage — the full check runs when a PR is ready, and the merge queue checks integration only
summary: Draft PRs get fast feedback, ready PRs get the full three-OS gate during review, and the queue re-checks only what can break when PRs combine

**Context.** `ci.yml` runs the same Linux-only PR stage on every push, draft
or ready. Windows, macOS, the Linux system tier, TypeScript 7, the Node floor
and MSRV run first in the merge queue. That split came from runner
saturation: with a PR stage of ~20 jobs per push and the queue building up to
five entries at once, the concurrency-capped hosted pool reached p95 runner
waits of ~8 min (November 2026). Moving the expensive legs to the queue fixed
the saturation but moved discovery of platform-specific and system-tier
defects to the most expensive place to find them.

The queue's 100 runs from 7 to 9 November 2026 show the cost:

- **A clean run is not the problem.** It takes a median 12.8 min, its
  critical path being the Linux system tier (~11 min) and the Windows leg
  (~10 min). Jobs started within seconds of being queued, so runner wait was
  not the problem either. A PR that passed first time landed a median 13 min
  after entering the queue.
- **Ejections are the problem.** 34 of 100 runs failed, and 22 of 50 PRs
  needed more than one attempt. Those 22 took a median 36 min from first
  entering the queue to landing, and up to 174 min (#3058).
- **Most failures were real defects that reached the queue late.**
  - 8 were defects a PR could have shown alone, in jobs that never run on a
    PR. #3058 broke on Windows, macOS and the Linux system tier (3 runs),
    #3080 and #3082 on Windows only (4 runs), and #3043 in the Node floor
    (1 run).
  - 6 were integration failures, the queue's proper job. #3074 (a pinned
    formatter snapshot, 4 runs) and #3031 (a generated diagnostics page out
    of date, 2 runs) failed in the Linux unit tier, which a PR does run. So
    each passed alone and broke only on top of the changes ahead of it.
  - 6 were cascades. An entry fails because a broken PR is queued ahead of it.
    5 of those came from #3058 and #3082.
  - 11 were infrastructure failures, and 4 were a flaky tsc heap exhaustion
    in the Node floor job. Those need fixing whichever stages CI runs, and
    are out of scope here.

So the queue does two jobs at once: it is the first full check of each PR,
and the check that PRs still work when combined. Doing the first job in the
queue makes every defect it finds cost an ejection, a re-queue, and a re-run
of each entry behind it. The second job is the only one a queue is needed
for.

The PR stage also does not tell a draft from a ready PR. Both get the same
Linux-only check, so nothing marks the point where a PR is worth the full
one. And `promote` marks a draft ready once `ci-fast` passes (~1–6 min), so
in practice the draft phase barely exists.

**Decision.** CI runs in five stages. Each check runs at the earliest stage
where a failure is cheap, and no check runs in two stages without a reason.

| Stage | Trigger | Runs | Purpose |
|---|---|---|---|
| Draft | `pull_request` on a draft | `ci-fast`'s gates (fmt, clippy, API docs, the cheap path-gated jobs) and the Linux unit and integration tiers | Feedback while the PR is still changing |
| Ready | `pull_request` on a ready PR, including the transition to ready | Everything the queue runs today: the draft set, the Windows and macOS legs (`ci-os`), the Linux system tier, TypeScript 7, the Node floor, and MSRV on a global change | The full check, run while the human review happens |
| Merge queue | `merge_group` | Build, clippy and the Linux unit and integration tiers, path-filtered as now | That the PR still works on top of the entries ahead of it |
| `main` | `push` | Unchanged: every job the queue did not pass on this commit (`queue_passed`), plus cache saves | The backstop |
| Nightly | `schedule` | Unchanged: everything, unfiltered, on all three OSes | Drift and the full `ci` profile |

The rules that make the stages hold together:

- **`CI green` stays the only required check, and every stage reports it.**
  On a ready PR it covers the ready set. In the queue it covers the queue set.
  The queue can never wedge on a check its stage does not run. This is the
  same invariant the current workflow relies on.
- **A draft-only run never reports `CI green` as passing.** Required checks
  match on name and head SHA, so a green `CI green` from the draft set would
  let a PR be approved and queued on the draft set alone. Since the queue no
  longer runs the platform legs, that defect would reach `main`. When a run
  ends with the PR still a draft and the ready set not run, `CI green` fails,
  with a message saying the ready-stage check has not run. It cannot simply be
  skipped, because GitHub counts a skipped required check as passing. Drafts
  cannot merge anyway, so the red mark costs nothing. The ready-set run that
  follows reports `CI green` again on the same SHA, and the latest result is
  the one that counts.
- **Promotion runs the full check in the same run, not a dispatched one.**
  `promote` flips a draft with the `GITHUB_TOKEN`, and events that token causes
  do not start workflow runs. So the `ready_for_review` it fires starts
  nothing. Rather than dispatch a second run, the ready-set jobs take `needs:
  promote` and run in the same `pull_request` run when `promote` flipped the
  PR or the PR was already ready. The run keeps its concurrency group, so a
  newer push still cancels it, its `pull_request` context for the `changes`
  path filter, and its reporting on the head SHA. It also needs no write
  token beyond what `promote` has today. The draft and ready stages simply run
  one after the other on that push.
- **A human marking a PR ready starts the full check** through the
  `ready_for_review` activity type. That is a normal event, so it covers fork
  PRs too, which `promote` never touches.
- **`promote` waits for the draft stage, not just `ci-fast`.** A draft is
  promoted once the Linux unit and integration tiers pass too. Promotion then
  means "worth a full run and a review", not just "it lints".
- **There is no label to ask for the full check early.** To get it, mark the
  PR ready: that is what "ready" now means. A `ci:full` label was considered
  and rejected. It needs the `labeled` activity type, and that starts a run on
  *every* label change. Such a run shares the PR's concurrency group, so it
  cancels an in-progress ready run. It then reports `CI green` without having
  run the check, which an unrelated label edit must never be able to do.
- **The trigger lists its activity types explicitly**: `opened`,
  `synchronize`, `reopened` and `ready_for_review`.
- **Superseded runs are cancelled** on drafts and ready PRs alike, as
  `pull_request` runs are today. Every draft-stage and ready-stage run is a
  `pull_request` run in the PR's own concurrency group.
- **The queue does not repeat the ready check.** A queued PR has already
  passed the full set at its own head. The queue re-checks only what combining
  it with the entries ahead can break, and on the platform where that is
  cheapest to find.

**Consequences.**

- **Platform and system-tier defects surface before the queue.** On the days
  measured, that is the 8 runs from #3058, #3080, #3082 and #3043, plus the 5
  cascades they caused: 13 of the 34 failures. The 6 integration failures
  still eject from the queue, as they should, but each ejection costs minutes
  rather than a full three-OS run.
- **A queue run drops from ~13 min to the length of its slowest Linux job.**
  On run 42580968208 that was ~2 min of tests on the self-hosted runner. An
  ejection now costs minutes, not a quarter of an hour.
- **The ~13 min full check moves earlier but mostly stops costing wall
  clock.** It runs during review, not after approval. The author now waits on
  it before merging rather than after queueing, which is the same wait, paid
  once.
- **Runner load moves; it does not grow without bound.** The three-OS legs
  run on a ready PR's pushes instead of on each queue entry. A ready PR gets
  far fewer pushes than a draft, and superseded runs are cancelled. The queue
  also stops making re-runs: ~35 extra full three-OS runs in the two days
  measured. Draft pushes cost what PR pushes cost today, because the draft set
  is today's PR stage. This is a prediction, and the first thing to measure after the
  change: if p95 runner wait rises back towards the ~8 min that caused the
  original split, revisit the ready set, not the staging.
- **What we give up.** Two PRs that each pass the full check alone could break
  only in combination, and only on Windows or macOS, or only in the system
  tier. The queue would no longer catch that. The `main` backstop runs every
  job the queue did not pass, so such a break shows on `main` within one run
  of landing, and before any release, because `release.yml` runs the full
  suite. It never reaches a user. This is accepted as rare. If it recurs, add
  that leg to the queue set behind a path filter rather than restoring the
  whole matrix.
- **A ready PR's full check can go stale as `main` moves.** The queue's
  integration check covers this on Linux. Off Linux, the `main` backstop
  covers it, as above.
- **`ci-fast` keeps its meaning** as the fast subset the Claude review opens
  on. The draft stage is `ci-fast` plus the Linux test tiers.
- **Not covered here.** The queue's infrastructure failures need separate
  fixes, whatever the staging: self-hosted runner health (`r4` with no
  `git`), retries on 5xx release downloads, and the Node floor heap
  exhaustion. They are not part of this decision. They are also why the
  queue's failure rate will not drop to zero.
