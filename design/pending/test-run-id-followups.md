---
level: patch
changelog: "`run.id` is fresh for every `bynk test --against` run, even with `BYNK_TEST_RUN_ID` left in the environment. `bynk.test.remote_clock_dependent` now follows run-derived instants through calls and checks every `expect` in a case, including those in `if` and `match` arms and after a `let … else`. Hover and completion on `run.id` respect every binder that shadows `run` (#2175)"
---
