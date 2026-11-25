---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: An agent handler deletes its own instance's stored state with `self.erase()`: when it commits, every field, entry, index posting and marker for the key goes, the next call starts from the initial state, a faulting handler erases nothing, and a `:=` after it is `bynk.agent.write_after_erase`
---

## ADR: agent-erase
title: `self.erase()` deletes an agent instance's stored state as its commit
summary: What `self.erase()` deletes, when, and what it leaves to events, invariants and the tests

**Context.** An agent's state could be removed only for a whole class (`removed agent Old` deletes
every instance). A handler had no way to delete its own instance: a closed account's data could not be
erased (the right to erasure), a finished order or an expired session lived in storage for ever, and a
cleanup job had no operation to call. `Map.remove` and `Cell := zero` clear values but leave the
instance's record, its schema markers and any entry the handler does not know to remove. Only the
runtime knows everything that belongs to one instance (#3931).

**Decision.** A. `self.erase()` resets the instance to its initial state: after the commit it is
indistinguishable from a key never seen, and each field reads its initialiser or zero. The alternative,
making the key unreachable, needs a stored tombstone, which is itself data kept for ever. B. The erasure
is part of the atomic commit. The call marks the handler; where the handler would call `commitState` it
calls `eraseState` instead when marked, which runs `storage.deleteAll()` (the in-memory double does the
same on the bundle target) and drops what the instance remembers of its last read. The handler's other
writes, before or after the call, are discarded, and a `:=` after a `do self.erase()` in the same block
is the warning `bynk.agent.write_after_erase`. A handler that faults never reaches the commit, so it
erases nothing. C. No invariant or `transition` is checked against the initial state: every new key
starts in it, so checking it again would add nothing. D. Events emitted in the handler are released as at
any commit, so a subscriber learns of the erasure and it can be audited. (Pending alarms are cancelled
once the alarms track ships them.) E. Tests need no new observation: a case reads the instance through a
read handler. The checker types `self.erase()` as `Effect[()]` (a new `Callee::AgentErase`), legal only in
an effectful agent `on call` handler, and the editor offers `erase` after `self.` and says what it does
on hover.

**Consequences.** Erasure is an explicit call in a handler the author wrote, and the reference advises
emitting an event first. `deleteAll` on a very large instance deletes the object's storage, which
Cloudflare bounds per object. The erasure is not undone by anything the handler does afterwards, and a
write the handler makes before it is lost, which the warning covers only for the write after it. The
runtime's `StorageLike` gains `deleteAll`, so a test double of it must too. Nothing changes for an agent
that never calls `self.erase()`: the emitted class has no `eraseState`, and the handler's commit is the
call it was.
