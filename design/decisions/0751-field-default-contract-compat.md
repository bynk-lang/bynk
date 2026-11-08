# 0751 — A field with a default is compatible in the service-contract classifier, and the hash is unchanged

- **Status:** Accepted (v0.515)

**Context.** ADR 0676 E left a follow-on: adding a field with a default still
changes a service's contract hash, so a callee that adds one needed every caller
rebuilt, though the new callee decodes an old body (the missing key takes the
default, ADR 0676 C) and an old callee ignores the extra key. ADRs 0647 and 0648
already classify a skew compatible or breaking and let a callee keep answering
older hashes, but the classifier "errs towards breaking" and credited only an
`Option` for an absent field, because a default was not in the stored shape.
Proposed in #2689.

**Decision.**

- **A. The hash is unchanged.** Dropping defaulted fields from the normal form
  would change the hash of every service that already has one (a new wire
  version, every embedded hash invalid) and would hide removing or retyping such
  a field from the hash, turning a `409 ContractMismatch` into a `400` at decode.
  The hash detects skew; the classifier judges it, as in ADR 0647.
- **B. The classifier credits a default.** A receiver's field missing from the
  sender is compatible when it is an `Option` or declares a default. One rule in
  `flows`, so it holds for a request (callee receives) and a response (caller
  receives) alike. A retyped or narrowed defaulted field, or a required field
  beside it, is still breaking. The default's value never matters: it is read
  only when the key is absent.
- **C. The stored shape records it.** `Field` gains `defaulted`, written as a
  trailing `D()` in the shape text and ignored by `normal_form()` and the hash,
  so a stored shape still hashes to the hash recorded with it. Text recorded
  before the marker reads as having no default and classifies as before. For a
  request the receiver is the freshly built callee, so its shape carries the
  marker; a response's receiver is the caller's recorded expectation, which
  carries it after the caller's next whole-project deploy.
- **D. Clients that embed a hash are unchanged.** A Bynk caller gets the benefit
  through ADR 0648's accepted-older-hashes lock. A foreign caller's pinned hash
  and the TypeScript client are held exactly, so they regenerate and re-pin as
  before. Nothing about which hash a client embeds changes.
- **E. No new wire version.** The normal form, hash function, headers and body
  encoding are untouched.
- **F. An old caller gets the callee's current default,** which is not a contract
  change and shows no skew when it later changes.
- **Related:** #2691 (agent schema lock: an added variant is additive) asks the
  same question of stored state. A response variant added stays breaking on the
  wire; the two rules do not conflict.

**Consequences.** Shape text carrying the `D()` marker is unreadable by an earlier `bynk`, which then fails closed: it drops a `bynk.contracts.lock` entry as unreadable (so the callee stops accepting that older hash) and cannot classify the skew. Upgrade every deployer together before recording a defaulted-field shape. `bynk deploy --context X --accept-compatible-skew` ships a
callee that added a defaulted field ahead of its callers. The classifier's
property test and a behavioural test (an old-hash, old-body call reaching the
new callee and reading the default) pin it. No language surface changes, so
hover, completion, semantic tokens and signature help are unchanged; the
`bynk/contractDiff` verdict now reads compatible for such a field.
