---
level: patch
probes: emit_diagnostics, diagnostic_coverage
changelog: "`bynk deploy --context --accept-compatible-skew` deploys a callee ahead of live callers whose contract it stays compatible with, recorded in `bynk.contracts.lock`"
---

## ADR: callee-accepts-older-compatible-contracts
title: A callee accepts the contract hashes of older callers it stays compatible with, from a committed contracts lock
summary: Callee-ahead only, a committed lock re-checked against the current contract, and an opt-in flag

**Context.** #2553 classified a service-contract skew as compatible or breaking (ADR 0491's gate, the editor's contract diff) but changed nothing about what a deploy allows. A compatible change was still refused twice: the gate refused any skew, and the callee compared the caller's `X-Bynk-Contract` to its own hash with strict equality, so it would have answered `409 ContractMismatch` anyway. Adding an optional response field therefore forced a lock-step deploy of every caller. The two skews are not symmetric. A callee pushed over live callers is a new build and can be made to carry the old callers' hashes. A caller pushed over a live callee meets a Worker that is already built and cannot learn a hash that did not exist when it was built.

**Decision.** #2554, accepted as proposed.

- DECISION A, callee ahead only. `callee_contract_skews` may pass a compatible skew; `contract_skews` (caller ahead) keeps refusing, and for a compatible one says so and names the callee to deploy first.
- DECISION B, a committed `bynk.contracts.lock` at the project root. It maps each context's service to the older shapes it still accepts (`ServiceShape::to_text`, which keeps the structure the hash throws away). The build reads it through `CompileOptions::contracts_lock`, which `bynk-driver`'s `try_project_options` fills, so every CLI build honours it. The callee entry admits its own hash or any accepted one, and `bynk-contracts.json` records them under `accepts`. The refusal's `expected` still names the callee's own hash. Alternatives rejected: injecting the set from the ledger at upload (builds stop being reproducible from source and differ by environment) and deriving it from git history (fragile, unavailable to a build from a tarball).
- DECISION C, never assume transitivity. Every build re-checks each entry against the contract the service has now with `check_call`. An entry that is no longer compatible, unreadable, for another service or for a context that is gone is left out with a `bynk.project.contracts_lock_stale` warning. The lock is not rewritten by a build. A service accepting more than eight entries gets `bynk.project.contracts_lock_oversized`. `deploy` reports entries no live caller was compiled against.
- DECISION D, opt-in. `bynk deploy --context X --accept-compatible-skew` is the only thing that records an entry and lets a compatible skew through; without it the gate refuses as before, with a line saying the flag would accept it. With the flag, `deploy` takes the live callers' recorded shapes from `bynk.deploy.lock`, writes the lock, rebuilds the callee so it carries them, and proceeds only if the rebuilt callee's `accepts` covers every compatible skew. A breaking or unclassified skew is refused whatever the flag. `--dry-run` reports what it would record and writes and builds nothing.
- DECISION E, Bynk callers only. A foreign caller's pinned hash (`bynk.foreign.toml`, ADR 0486) is held exactly.
- A corrupt lock fails the build (`bynk.project.contracts_lock_corrupt`), as an unreadable `bynk.schema.lock` does.
- The wire format's rule for `X-Bynk-Contract` loosens from "must equal the callee's" to "must equal the callee's or one it declares it accepts". The 409 body and the wire version are unchanged.

**Consequences.** A compatible change ships callee first and callers at their own pace. A wrong "compatible" verdict now ships a real break, so the classifier is held to a model of the wire format's decoder: a property test checks, over a pool of shapes and both directions, that every value a sender can produce decodes at the receiver whenever the verdict is not breaking. Behavioural tests drive a compiled callee with the old hash, the new hash, an unrecorded hash and a made-up one. The response direction is not checked at runtime; a caller decodes leniently, and the classifier's response rules are the same decode rules. Not done: making it the default (the proposal recommends opt-in for the first release), and rejecting a stale lock entry from `bynk check`, which does not read the lock yet.

**Tooling delta.** Hover, completion, semantic tokens and signature help are unchanged: no language surface is added. The contract-diff view could later show the contracts a callee still accepts.
