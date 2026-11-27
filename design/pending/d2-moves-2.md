---
level: patch
probes: ts_writes
raises: ts_writes — the agent-move variants name each forward's entry and miss test (`__fwdN`, `__fwdMissN`, as the member table already names `__mN`) and head the receive and stub entries with a comment naming the moving context and its scripts; `TsStmt::comment` takes text and identifiers are strings, so there is no typed node to build them from (#3565)
changelog: A Workers build can write the phase variants of a supported agent move beside a Worker's own files (a receiving Worker's `wrangler.receive.toml` and `index.receive.ts`, which forward the moving context to its old script, a source's `wrangler.commit.toml` with `transferred` tombstones, and a stub for a dissolved source); only `bynk deploy` will ask for them, so every build today writes exactly what it did (#3565)
---
