# 0532 — A case reads the test run's identity as `run.id`

- **Status:** Accepted (v0.371)

**Context.** End-to-end testing track §3.8 (#2138). A real deployment keeps its state between runs, so a case that creates `order-1` finds it there the second time, and two runs against one preview deployment collide. The in-process harness starts every case fresh, so it never needed a name for the run.

**Decision.**

- **`run.id`**, a `String`, read in a `case` body. In process it is always `"local"`, so the in-process run stays deterministic. Each `bynk test --against` run sets a fresh value (the time and the runner's process id); every `bynk dev` session of one `--against dev` run shares it. The runner passes it to the harness as `BYNK_TEST_RUN_ID`.
- **`run` is contextual, not a keyword.** It names the test run only where nothing else binds `run`: a `let run = …` shadows it, and a history property's `for all run: History[…]` is an ordinary binding. So no existing program changes meaning, and `run` stays usable as a name.
- **Tooling.** Hover on `run.id` in a case body describes it, and completion after `run.` offers `id` there (ADR 0156).

**Alternatives.** A `Val[RunId]` fabrication was rejected: a fabricated value is random per case, but a case needs one value shared by the run (and by its `bynk dev` groups) so a later run can tell its data apart. A reserved keyword was rejected because `run` is already a common binding name.

**Consequences.** A case that keys its data by `run.id` reads the same in process every time, and on a deployment leaves one set of data per run behind; the runner doesn't clean it up.
