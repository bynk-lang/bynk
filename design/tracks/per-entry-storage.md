# Per-entry agent storage — collection entries and index postings under their own keys

- **Status:** Settling. Direction accepted in
  [#3189](https://github.com/bynk-lang/bynk/issues/3189) (label `accepted`);
  originating issue [#3159](https://github.com/bynk-lang/bynk/issues/3159). The
  spike (§5, Q1–Q3) has run and is recorded below. Spine
  [#3195](https://github.com/bynk-lang/bynk/issues/3195).
- **Realises:** the follow-on named by ADR 0110 D3, ADR 0118 D6, ADR 0119 and
  ADR 0121, and the Gate 3 stored-state freeze of
  [`../bynk-1.0-definition.md`](../bynk-1.0-definition.md) (ADR 0538).
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md),
  run GitHub-native per [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md).
  The decisions themselves (A–H) are argued in #3189; this doc is the map the
  slices are cut from, and carries the spike evidence.

## 1. The theme

An agent's state is one stored value under the Durable Object key `"state"`.
`loadState` reads it, merges zero-then-stored, checks `__v`, and rehydrates;
`commitState` encodes the whole record and writes it back. A one-entry `put`
therefore costs time proportional to the whole state, and one stored value bounds
every collection together.

The track moves each `Map`/`Set`/`Cache`/`Log` entry and each index posting to its
own key on the Durable Object KV API, keeps the `Cell` record in `"state"`, and
adds a `__layout` marker beside `__v`. The surface (grammar, AST, checker) is
untouched. It must land before 1.0 freezes the stored-state encoding.

## 2. Why a track (the ADR 0076 trigger)

- **Multi-increment:** yes. Six slices, with a release boundary between slice 0
  and the first writer change.
- **Surface not yet settled:** no. The surface is unchanged. What is open is the
  layout, the commit semantics and the upgrade.
- **Security/safety boundary:** yes. This is a stored-data layout change with a
  rollback hazard (§8).

## 3. What's already true

- One key, `"state"`; `__v` is the lock-derived schema version (ADR 0538).
- A handler binds a working copy and calls `commitState` once at the end; a fault
  before the flush persists nothing (ADR 0109 D1). ADR 0109 D2 already describes a
  write-set overlay keyed by entry.
- Query lowering is "source → pipeline" so a per-entry store replaces only the
  source (ADR 0119). Invariants mention only keyed `map.get(k)` (ADR 0110 D4).
- `bynk migrate sweep` wakes every instance through `/_bynk/migrate`, which runs
  `loadState` then `commitState`.
- Every deployed class is SQLite-backed (ADR 0438).

## 4. Constraints on any answer

- Behaviour behind the surface does not change, except the intended I/O and size
  properties.
- The encoding of each storable type (ADR 0538) is untouched; only placement moves.
- No build that can write the new layout is released before a build that refuses
  to read it wrongly (slice 0 first).
- A faulted or invariant-refused handler persists nothing, index and expiry keys
  included.

## 5. Open design questions and the spike

Decisions A–H are in #3189. The proposal quoted platform figures from memory and
left three questions to a spike. The spike ran on 2026-11-10 against the current
Cloudflare documentation and a local `workerd 2026-11-10` (npm `workerd`) with a
SQLite-backed Durable Object. Findings follow; the scripts are described, not
committed.

### Q1 — Platform limits

| Quantity | Proposal | Cloudflare docs (limits page, "Last updated Nov 7, 2026") | Local workerd, SQLite-backed |
|---|---|---|---|
| Value size | about 2 MB | **"Key and value combined cannot exceed 8 MB"** (SQLite-backed); 128 KiB for the KV-backed backend | 1, 3 and 7 MB values accepted; 9 MB fails with `SQLITE_TOOBIG` |
| Key size | about 2 KiB | SQLite-backed: the same combined 8 MB cell; **2 KiB applies to the KV-backed backend only** | 3 KiB and 100 KiB keys accepted |
| Keys per `get`/`put`/`delete` | 128 | "Supports up to 128 keys at a time" (storage API page) | 129 and 200-key batches accepted; the limit is not enforced locally |
| Storage per object | not stated | 10 GB (SQLite-backed) | not tested |

Consequences:

- **The 2 MB figure is not supported.** The repo states no number; the proposal
  brought it from memory. The SQLite-backed ceiling is 8 MB for key and value
  combined. The ceiling still exists and per-entry keys still remove it, so the
  motivation survives, but the docs must say 8 MB and say the figure is
  Cloudflare's to change.
- **The 2 KiB key limit does not apply** to SQLite-backed classes, which are the
  only ones `bynk deploy` emits. `StorageKeyTooLong` (decision B) is therefore not
  needed for the platform; the remaining reason for a key-length cap is our own
  key-plus-value budget and listing cost. Slice 1 decides whether to keep a cap.
- **Treat 128 as binding.** Production documents it; local workerd is more
  permissive, so a test passing locally with a larger batch proves nothing. The
  in-memory test storage must enforce 128.
- Classes that an older Bynk created KV-backed (128 KiB values) are already
  refused by deploy (ADR 0438), so they do not constrain the layout.

### Q2 — Transactional semantics of a multi-key flush

Documented: each `get`/`put`/`delete` is "implicitly wrapped inside a
transaction", atomic "even when accessing multiple key-value pairs"; consecutive
`put`/`delete` calls with no `await` between them are "automatically combined and
submitted atomically"; "Using `await` will disable automatic write coalescing".
The SQLite-backed page says "Explicit transactions are no longer necessary" and
calls the `txn` object "obsolete", but does not remove `transaction()`.
`transactionSync(callback)` rolls back if the synchronous callback throws.

Local workerd:

- `transaction()` with a throw, or with `txn.rollback()`, leaves nothing written.
- Three `put` calls of 100 keys issued with no `await` between them and joined
  afterwards all land (300 keys).
- Three awaited `put` calls of 128 keys inside `transaction()` all land (384).
- `transactionSync` with the synchronous `storage.kv` API rolls back on a throw.
- `list({ prefix, limit, reverse })` returns UTF-8 byte order, so `m/a/1` sorts
  before `m/a/10` before `m/a/2`: zero-padding in `Log` keys is required, as
  proposed.

Consequences for decision E:

- A 128-key chunk is atomic by itself. A flush larger than 128 keys is atomic
  only if the chunks are issued with no `await` between them (coalescing) or
  inside `transaction()`/`transactionSync`. The documentation does not promise
  that coalescing has no size ceiling, so **do not rely on it for the upgrade**.
- `transaction()` still works but the platform calls explicit transactions
  unnecessary. The two honest options for slice 1 are (a) keep an `async`
  `transaction()` and rely on its documented commit-or-abort, or (b) build the
  flush as a synchronous closure over `storage.kv` inside `transactionSync`,
  which is explicit, rolls back on a throw and has no 128-key batching. (b) needs
  every flush input already in memory, which the overlay guarantees, but it is a
  larger runtime shape. Slice 1 must pick one and test it against the refused-commit
  regression; this is the one finding that **revises, but does not undermine**,
  decision E.
- Decision A is unaffected: the KV API is documented and supported on SQLite-backed
  classes, with ordered range `list`.

### Q3 — Interleaving today (decision E's open question)

Documented (the input/output-gate rules are in the 2021 blog post, which
Cloudflare still links; the API pages do not restate them): while a storage
operation runs, no other event is delivered; an `await` on non-storage I/O does
not close the input gate.

Local workerd, a handler that loads, awaits, then commits `n + 1` under `"state"`,
with five concurrent requests:

| Await between load and commit | Expected increments | Observed |
|---|---|---|
| none | 6 | 6 |
| a 200 ms timer | 6 | 2 (four updates lost) |
| a `fetch()` | 6 | 5 (one lost) |

So **a lost update on the single `"state"` record is possible today** whenever a
handler awaits a non-storage effect between `loadState` and `commitState`, which
a handler calling another agent or a capability does. Under per-entry keys,
writes to disjoint entries would both survive, while `Cell` writes and writes to
the same entry stay last-writer-wins. The proposal asked the ADR to state which
model is intended. This track records the evidence and leaves the choice to the
slice that carries the ADR (slice 2): either keep today's semantics and document
them, or serialise store handlers with `blockConcurrencyWhile` (whose semantics the
fetched pages do not document in detail, so it needs its own test). Because the
choice changes observable behaviour, it is a maintainer question, not a worker
default.

### Q4 — Upgrade size and resumability (not yet closed)

The lazy upgrade reads one value (at most the ceiling, now 8 MB) and writes its
entries. A single value of that size is within a request's CPU budget on the
documented limits (30 s default), but the spike did not measure a real upgrade.
Slice 2 measures it; if it does not fit, the upgrade becomes resumable
(`__layout: "upgrading"`). Open.

### Q5 — Small-agent round-trip cost (not yet closed)

Per-entry reads are more round trips than one wholesale `get` for a small agent.
Not measured here; slice 3's fixture records the number and decides on an inline
small-collection threshold only if it regresses measurably. Open.

## 6. Candidate slice decomposition

0. **Forward-compatibility guard.** `loadState` knows `__layout`; an unknown value
   faults `RehydrationViolation` with cause `NewerLayout`, never decoding or
   writing back. No writer changes. Ships on its own, and is released before any
   later slice.
1. **Test storage and runtime primitives** (batched ops enforcing 128, ranges,
   the chosen atomic flush, op counting) and the `Cell`-record split.
2. **`Map`/`Set` entries** per key, with the overlay flush and the lazy upgrade.
   Carries the main ADR, including the interleaving decision (Q3).
3. **Indexes** as posting keys and the index-routed read.
4. **`Cache` and `Log`** (expiry index, time-ordered keys, retention ranges).
5. **Sweep reporting, docs, the ceiling note retired, live check on Cloudflare.**

Slices 1–4 touch `emit_agent`'s `loadState`/`commitState`; they are sequenced after
the in-flight split of `emit_agent` to avoid a conflicting rewrite.

## 7. Front-loaded ADR candidates

- Slice 0: the `__layout` marker, its reserved name, "absent means wholesale", and
  the `NewerLayout` fault, amending ADR 0538 to name `__layout` as part of the
  frozen layout.
- Slice 2: one ADR recording decisions A–H (backing store, key layout, `Cell`
  placement, upgrade, commit semantics, reads, housekeeping), marking ADR 0110 D3
  and ADR 0118 D6 settled and amending ADRs 0119 and 0121; it states the Q1
  figures and the Q3 interleaving model.

Numbers are assigned at merge.

## 8. Threat model

- **Asset:** a deployed agent's stored state.
- **Hazard, not adversary:** a rollback or a mixed-version rollout. A build that
  predates the layout, reading the new layout, would see a record whose collection
  fields are absent, zero-merge them (ADR 0124 D4) and write wholesale, orphaning
  every per-entry key. The existing `NewerSchema` guard covers only a `__v` bump,
  and only for builds with a lock.
- **Mitigation:** slice 0 ships a build that faults on an unknown `__layout` first;
  the writer flips only in a later release; downgrade across the flip is explicitly
  unsupported and the changelog says so. The upgrade is one atomic flush, so a
  crash mid-upgrade leaves the old record intact.
- **Verification:** a fixture that presents an unknown layout and asserts a fault
  with nothing written; the refused-commit regression against the new flush; a live
  check on a real Worker before the flip.

## 9. Slice status

Live status is on the spine. Mirror:

- [ ] Slice 0 — forward-compatibility guard
- [ ] Slice 1 — runtime primitives and `Cell`-record split
- [ ] Slice 2 — `Map`/`Set` entries and the lazy upgrade
- [ ] Slice 3 — index postings
- [ ] Slice 4 — `Cache` and `Log`
- [ ] Slice 5 — sweep reporting, docs, live check

## 10. Non-goals

- SQL tables as the backing store (decision A); a later move would be a second
  layout change.
- Alarm-based reaping of `Cache`/`Log` (still deferred by ADRs 0113 and 0121).
- Any change to the author-facing surface.

## 11. Done when

As in #3189 "Done when": a `Map` larger than the old single-value ceiling works; a
one-entry `put` and an indexed one-entry read perform a bounded, size-independent
number of operations on the instrumented test storage; a wholesale instance
upgrades on first wake and via `bynk migrate sweep`, crash-safe; a faulted handler
persists nothing; docs and tooling surfaces are current; the track is retired.
