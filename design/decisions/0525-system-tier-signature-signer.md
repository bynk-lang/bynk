# 0525 — The `system` driver signs `Signature` routes, and `Forged`/`Stale` drive their failures

- **Status:** Accepted (v0.365)

**Context.** A `system` case could not reach a `Signature`-secured handler. The
driver presented a credential only for a Bearer route, ADR 0489 recorded that
`Signature` still had no `system`-tier signer, and #2132 refused such a call
(`bynk.test.signature_needs_in_process`) rather than send a request the seam
could only reject. `bynk-lang/shop`'s webhook scenarios (signed, forged, ten
minutes stale, the same notice twice) lived in Bash, signed with `openssl`. A
`Signature` actor already declares everything a signer needs (`secret`,
`header`, `timestamp`, `tolerance`), and ADR 0207 D3 set the precedent: the
framework signs the credential; the real seam verifies it (end-to-end-testing
track §3.2, proposal #2135).

**Decision.**

- **A plain call signs (D-A).** At `system`, every typed and `Wire` call to a
  route whose actor is a `Signature` actor is signed, with no `by`: the actor
  has no identity to name. `unit`/`integration` are unchanged (the seam is
  skipped in process), so the body is the same at every tier.
- **The failure principals (D-B).** `by Nobody` is relaxed to `Signature`
  routes and sends no signature headers. `by Forged` signs with a secret the
  seam does not hold. `by Stale` signs correctly, timestamped `tolerance + 60`
  seconds in the past. Each yields the outcome sum, a `401` decoding to
  `Rejected(Unauthorized)` (ADR 0212 D2). Like `Nobody` they are reserved
  call-site names that take no identity. `Forged` and `Stale` are legal only on
  a `Signature` route (`bynk.test.forged_needs_signature_route`) at `system`
  (`bynk.test.forged_needs_system`). `Stale` needs an actor with a `timestamp`
  header (`bynk.test.stale_needs_timestamp`), and also a `tolerance`: without
  one the seam has no replay window, so a stale signature would be accepted.
  `by Signed(secret = …, at = …)` stays re-openable if a test needs one
  specific timestamp.
- **The secret (D-C).** The suite's `env` entry for the actor's `secret`
  (#2134), else a fixed test secret set in `process.env`, as the Bearer path
  does.
- **The signed bytes (D-D).** Exactly what the driver sends: the serialised
  JSON of a typed call, the raw string of a `Wire(…)` call. So a case can check
  a valid signature over a malformed body.
- **Actor sums.** A sum route's drivers do not know which member a call acts
  as, so they do not sign. A `system` call to a sum route as its `Signature`
  member (or with no principal, when that member is the primary) stays
  `bynk.test.signature_needs_in_process`, now narrowed to that case. This
  supersedes ADR 0489's "no `system`-tier signer" note for single-actor routes.

**Mechanism.** The signer, `__bynkSignHmacSha256`, is a test-only helper in
`emitter/test_runtime/hmac_signer.ts`, next to the JWT signer. It computes
HMAC-SHA256 over `<timestamp>.<body>` (or the body alone with no timestamp
header), sent as `sha256=<hex>`, as `verifySignatureHmacSha256` checks. Each
signed route has the typed and raw signing drivers, `noauth_`/`rawnoauth_`, and
`forged_`/`rawforged_` and, with a tolerance, `stale_`/`rawstale_`. The
production Worker and its seam are untouched.

**Consequences.** A webhook flow is tested end to end at `system` against the
real seam, and signer–verifier drift fails a behavioural fixture. `Stale` is
relative to `Date.now()` at send time, which the seam also reads, so it does
not flake.
