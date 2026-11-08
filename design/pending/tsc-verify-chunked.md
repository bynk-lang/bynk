---
level: patch
changelog: The emitted-TypeScript type-check test now runs `tsc` over the fixtures in chunks of 200 instead of one pass, so it no longer runs out of memory on the macOS release runner
---
