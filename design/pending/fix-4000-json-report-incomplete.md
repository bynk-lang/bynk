---
level: patch
changelog: `bynk test --format json` and `--coverage` now exit non-zero when the document carries an `error`, so a runner that ends before `run-end` can no longer leave an incomplete report (a suite with no `cases`) behind an exit status of 0
---
fix: an incomplete `bynk test --format json` report no longer exits 0

Closes #4000.
