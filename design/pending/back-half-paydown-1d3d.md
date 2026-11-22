---
level: patch
changelog: Internal test-harness change; the parallel test processes that run `wrangler` through `npx` provision it once per machine, under a lock, instead of racing on a cold npx cache
---
