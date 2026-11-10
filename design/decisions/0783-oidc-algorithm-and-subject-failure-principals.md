# 0783 — Two reserved failure principals for an Oidc route: WrongAlgorithm and NoSubject

- **Status:** Accepted (v0.533)

**Context.** The `system` driver is the issuer for an `Oidc` route (ADR 0749),
and four failure principals let a suite check the seam's signature, `iss`, `aud`
and `exp`. Two checks that OIDC verification most often gets wrong had none: the
algorithm allow-list (`alg: none`, and a symmetric `HS*` token keyed with the
published public key, the classic algorithm confusion) and a usable `sub` (the
identity is minted from it, and is often refined, for example `String where
NonEmpty`). A suite could not show the seam refuses them (#3225).

**Decision.**

- **Two principals, each covering two tokens.** `by WrongAlgorithm` presents a
  token with `alg: none` (no signature) and an `HS256` token whose MAC key is the
  published public key (its DER encoding). `by NoSubject` presents a correctly
  signed token with an empty `sub` and one with no `sub` claim. In every token
  everything else is valid (`iss`, `aud`, `exp`, and for the `sub` tokens the
  signature), so exactly one thing is wrong. Both are reserved call-site
  principals like the existing ones: they take no identity, they are `system`
  tier only (`bynk.test.forged_needs_system`), and they must address a route with
  an `Oidc` actor (`bynk.test.wrong_algorithm_needs_oidc_route`,
  `bynk.test.no_subject_needs_oidc_route`).
- **One principal, not one per token, and the call reads the first admission.**
  The driver presents the tokens in order and returns the first outcome that is
  not the seam's refusal (`Rejected(Unauthorized)`); when every token is refused
  it returns that refusal. So a single `expect r is Rejected(Unauthorized)` fails
  if the seam admits any token, and what the case observes is the outcome the
  seam actually gave. It stops at the first admission rather than sending the
  rest, so a handler a lax seam let through runs once. The two cases of each
  principal are not separately nameable by an author; the alternative, four
  principals, would add names for what are two variants of one claim ("this
  seam refuses a bad algorithm" and "this seam refuses a bad subject").
- **The seam is unchanged.** The verifier already refuses all four tokens
  (`alg` outside `RS256`/`ES256` is `unsupported alg`, before any key is tried;
  an empty or absent `sub` is `missing sub`, before the handler and so before the
  identity type's refinement is consulted). This was confirmed by driving the
  real seam, not by reading it. A mutation test removes each of the two
  refusals from the emitted verifier in turn (the `return Err("unsupported alg")`
  and the `return Err("missing sub")` statements) and expects exactly the
  matching cases to fail and the others to stay green, so each case is sensitive
  to the one refusal it relies on. It does not model a lax verifier that admits
  the token: with the algorithm refusal removed the verifier goes on to read
  `params.kty` of a `null`, so that mutant fails on a thrown error, not on a
  token it let through. A mutant that makes the confusion token genuinely verify
  would test more and is not part of this decision. The production Worker is
  byte-identical; only generated test modules change.
- **Added.** The reserved names `WrongAlgorithm` and `NoSubject` (completion and
  hover), and the diagnostics `bynk.test.wrong_algorithm_needs_oidc_route` and
  `bynk.test.no_subject_needs_oidc_route`.

**Consequences.** A suite can show that an `Oidc` route refuses algorithm
confusion and an unusable `sub`, including when the actor's identity is refined
and so would otherwise reject it only at the handler. The test issuer holds the
public key's DER encoding alongside its signing keys, in the test module only.
Not covered: a `Bearer` route's `alg` check (it verifies `HS256` only, so the
attack has no analogue), and RS256 verification, which remains covered by the
compiler's own `oidc_auth.rs`. Against a remote target these principals fail as
the other `Oidc` principals do, with a message naming the cause.
