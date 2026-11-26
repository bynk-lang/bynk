---
level: minor
probes: diagnostic_coverage
changelog: A Cache, a Log or an indexed Map store field can be renamed with `replaces` too, so every storage kind keeps its entries, expiries and index postings across a rename
---

## ADR: collection-field-rename-all-kinds
title: The resumable key move covers Cache expiries, Log entries and index postings, and `bynk.store.replaces_collection` is retired
summary: `replaces` renames every storage kind; the move generalises to `x/` expiry keys and `i/` postings, and the refusal that said some kinds could not be renamed goes
**Context.** #4482. Part A (the resumable, batched key move for a `Map` and a `Set`) left a `Cache`, a `Log` and an indexed `Map` refused with `bynk.store.replaces_collection`, because they keep keys the move did not yet cover.

**Decision.** `moveRenamedEntries` takes the field's kind and whether it keeps postings, and moves each key family under the old name with the same batched, transactional, new-key-wins move: a `Map`, `Cache` and indexed `Map` under `m/` (a `Cache` also `x/`, its expiry entries ordered by time, and an indexed field `i/`, its postings `i/<field>/<index>/<by>/<pk>`), a `Set` under `s/`, a `Log` under `l/` (its keys carry a time and a sequence, which the rename leaves alone). The emitter says which families a renamed field has from its declaration: its kind, and an `@indexed` or `@unique` annotation (`@unique` keeps the same postings). The metadata (`c/`, which holds the count, the next insertion sequence and an index signature) moves after the last family, and when both names hold metadata only the entries moved are counted (an expiry or a posting is not an entry). `bynk.store.replaces_collection` has nothing left to say and is removed. It was never in a release (`bynk-syntax/shipped-codes.txt` does not list it), so it needs no tombstone. The non-additive refusal's rename hint now offers `replaces` for every storage kind.

**Consequences.** Every storage kind is renamed with `replaces`, with no `migrate from`. The same cost note applies: the first load after the deploy does the whole move, and its cost grows with the collection. The workerd test covers a `Map`, a `Set`, an indexed `Map` (read back through an equality filter on the index) and a `Log`, each with more entries than a batch; a `Cache`'s expiry keys are covered by the runtime unit test, since a workerd `Cache` test would need a clock to expire against.
