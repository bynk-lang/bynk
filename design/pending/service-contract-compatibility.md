---
level: patch
changelog: "`bynk deploy` and the editor's contract diff classify a service-contract change as compatible or breaking, with reasons"
---

## ADR: service-contract-compatibility-classifier
title: A service-contract change is classified compatible or breaking from a stored structured form
summary: The rules, the ledger and manifest fields that carry the structure, and a report-only first slice

**Context.** A service contract was reduced to a normal form and an FNV-1a hash
(ADR 0200), and the deploy gate (ADR 0491) refused any difference. Adding an
optional response field blocked a deploy exactly as removing a required request
field did. Events and agent state already had an additive classifier
(`reconcile`, `reconcile_agents`); services were the one boundary without one,
so the editor's contract diff (ADR 0640) could only call a service change
`changed`. The normal form cannot be diffed after the fact: it cannot be parsed
back (`AgeInt where Positive` does not say where a type's name ends and its base
begins), and the ledger kept only the hash.

**Decision.**

- One classifier, `check_call(caller, callee)` in `bynk-check`
  (`contract_compat.rs`), answers "can a caller compiled against this contract
  call a callee providing that one". Both deploy skews are that question: a
  pushed callee asks it of the callers' recorded contract and the new build's,
  a pushed caller of the new build's expectation and the live callee's. The
  editor's old-to-new is the first.
- The rules are the wire format's. A request flows caller to callee and a
  response callee to caller; in both the receiver must decode everything the
  sender can produce. An unknown key is ignored, a missing `Option` is `None`,
  a bare value is accepted for an `Option`, one parameter travels bare and
  several as an object keyed by name. So an optional request field added is
  compatible and a required one breaking; a response field added is compatible
  and a response variant added breaking; a refinement the receiver accepts
  everything the sender sends under is compatible, and otherwise breaking.
  Predicate entailment covers numeric ranges, `Positive`/`NonNegative` and the
  length predicates; `Matches` only entails an identical pattern.
- The classifier errs towards breaking. A field's default is not part of the
  contract, so an added field is compatible only when it is an `Option`.
- DECISION A: the structure is stored beside the hash. `ServiceShape` is the
  contract as a tree, built by the same walk as `service_normal_form` and
  rendering back to exactly that text, so a stored shape is trusted only when it
  hashes to the hash recorded with it. Its text form (`Svc("charge",P(…),Ret(…))`)
  is a separate, unambiguous grammar. `bynk-contracts.json` gains
  `provides_shapes` and `expects_shapes`, and `bynk.deploy.lock` gains `shapes`
  and `expects_shapes` on a Worker record. All are additive, defaulted and
  omitted when empty. A ledger with hashes only keeps today's behaviour for that
  entry until the next whole-project deploy records its shape.
- DECISION C: this slice is report-only. The gate refuses every skew as before
  (`bynk.deploy.contract_skew`), and now prints each skew's verdict and reasons,
  `--dry-run` included. A compatible skew is still refused because the runtime
  check compares hashes and the pair would answer `409 ContractMismatch`.
  Letting compatible skew through needs the runtime to accept an older hash and
  the gate to change what it allows, which is a deploy-semantics change for its
  own proposal.
- `bynk/contractDiff` service rows keep their `changed`/`added`/`removed`
  status and gain `compat` (`verdict`, `reasons`) on a `changed` row whose live
  and current shapes are both on record.

**Consequences.** The gate, the dry run and the editor share one definition of
"breaking" for services, as they already do for events and agent state. A wrong
"compatible" cannot loosen anything yet, because the verdict changes no
decision; the paired fixtures in `contract_compat.rs` (field added, removed, made
optional or required, refinement widened or narrowed, variant added or removed,
each for requests and responses) pin the rules before a later increment lets a
verdict decide. The ledger and the manifest grow by one structured string per
service.

**Tooling delta.** Hover, completion, semantic tokens and signature help are
unchanged: no language surface is added. The extension reads `compat` from
`bynk/contractDiff` (vscode-bynk#15).
