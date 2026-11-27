---
level: patch
changelog: A Workers build can write the phase variants of a supported agent move beside a Worker's own files (a receiving Worker's `wrangler.receive.toml` and `index.receive.ts`, which forward the moving context to its old script, a source's `wrangler.commit.toml` with `transferred` tombstones, and a stub for a dissolved source); only `bynk deploy` will ask for them, so every build today writes exactly what it did (#3565)
---
