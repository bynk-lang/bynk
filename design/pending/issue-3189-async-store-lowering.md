---
level: minor
probes: ts_writes
changelog: Store collection operations and store queries in emitted agents are written so a handle backed by storage can serve them, with no change to an agent's behaviour (per-entry storage, lazy-read slice, part 2a)
---

## ADR: async-ready-store-lowering
title: Store collection calls are awaited, and store queries chain through __bynkThen
summary: Every call the emitted code makes on a store handle is awaited and every store query runs its stages through a helper that serves a value or a promise, so a storage-backed handle can replace the in-memory one without a second change to the lowering

**Context.** The in-memory handles (#3254) return their values at once; the storage-backed ones (#3295) return promises. The lowering written for the first assumed the first: a keyed op called `has`/`get`/`put` and used the results synchronously, and a store `Query[T]` was a thunk producing an array that each builder and terminal indexed directly. Decision F of #3189 needs the second, and the `Awaitable` return types of the handle interface were designed for it. Doing this as its own increment keeps the lazy wiring (part 2b) small and lets this change be checked against the whole existing suite, since it must change no behaviour.

**Decision.** (1) Every call the emitted code makes on a `store Map`, `Set`, `Cache` or `Log` handle (`get`, `put`, `remove`, `add`, `append`, `prune`, `liveSize`, and the `has` and `get` of `update` and `upsert`) is awaited inside the async function that already wraps the operation; `contains` and `size` return the handle's value, which the effect that runs them already awaits. (2) A store `Query[T]` is a thunk producing its elements **or a promise of them**; its TypeScript type is `() => readonly T[] | Promise<readonly T[]>`. Each query builder and terminal runs on its source through a runtime helper, `then(value, f)` (emitted as `__bynkThen`), which applies `f` at once to a value and after a promise settles: an in-memory source still runs synchronously, a promise-returning one runs when it arrives. Operations that read another query (`flatMap`, the joins) await it.

**Consequences.** No agent's behaviour changes: the in-memory handles return values, so each operation runs in the order it did before; the whole existing suite passes, including the TypeScript strict verification of every emitted fixture. The emitted code changes shape for every agent with a store collection (golden churn), and a store `Query` parameter's TypeScript type gains the promise alternative. The storage-backed handles are wired in by the next increment. Hover, completion, semantic tokens and signature help are unchanged, because no syntax, name or signature changes.
