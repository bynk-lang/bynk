---
level: patch
changelog: Five fixes. `Log.since`, `before` and `between` no longer throw once a window holds more than 512 entries (#3747). `bynk test` no longer fails with `bynk.codegen.tsc_failed` after `bynk test --against` or a Workers build left files in `out/workers/` (#3748). `bynk deploy --dry-run` no longer rewrites `bynk.schema.lock` or writes `.bynk/deploy` (#3796). A build that records stored positions in an older `bynk.schema.lock` now says so and asks for the lock to be committed (#3749). Bundle builds emit the events-dispatch cases in the same order every time, so the output is reproducible (#3815)
---
