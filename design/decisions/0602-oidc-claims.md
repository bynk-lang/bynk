# 0602 — Reading an Oidc token's claims, and trust roots from the environment

- **Status:** Accepted (v0.399)

**Context.** The `Oidc` actor (v0.151) verifies an RS256/ES256 JWT against a
JWKS and mints its identity from `sub`. The handler sees nothing else, even
though the runtime verifier already returns the whole verified payload.

Trusted publishing (first-party registry track §5, R4 #2381) binds trust to a
GitHub Actions repository and **workflow file**. GitHub puts the workflow in the
`job_workflow_ref` claim, and never in `sub`.

The actor's issuer, audience and JWKS are also literals. That means a service
can't be run end to end against a fake issuer, which is how R2 and R3 were
proven.

The accepted proposal is #2380.

**Decision.**

- **A. `who.claims: TokenClaims` on an `Oidc` binder.**
  - A new built-in type has two methods:
    - `get(name) -> Option[String]`: the claim, when it is a JSON string;
    - `has(name) -> Bool`: whether the claim is present, of any type.
  - The emitted seam passes the verifier's claims into `deps.claims`, and
    `who.claims` lowers to `deps.claims`.
  - Claims are refused at every boundary (`bynk.types.claims_at_boundary`),
    as `RequestHeaders` are.
  - Only an `Oidc` binder has them. Reading `.claims` on any other binder is
    `bynk.types.unknown_field`.
- **B. `trustFrom = "<ENV>"`.** An optional `Oidc` argument names an
  environment variable.
  - When the variable is set, its JSON `{"issuer", "jwks", "audience"?}`
    replaces the declared roots at run time (`oidcTrust` in the runtime).
  - Unset, the declaration applies.
  - Set but malformed, every request is `401`, and the fault log says why.
  - `trustFrom` must be a string (`bynk.actor.oidc_trust_from_not_string`),
    and may not name an actor's `secret`
    (`bynk.actor.oidc_trust_from_is_secret`).
- **C. Tests.**
  - At the unit and integration tiers, the call-site principal takes the
    claims as a second argument: `by Workload("id", TokenClaims.of(["name:
    value"]))`.
  - With none, the handler sees no claims.
  - Only an `Oidc` principal takes claims
    (`bynk.test.principal_claims_not_oidc`).
- **D. Tooling.**
  - `TokenClaims` is a built-in type name, with completion for its two
    methods.
  - The new diagnostics are registered, with an explain entry for
    `claims_at_boundary`.

**Deviations from the accepted proposal.**

- **The type is `TokenClaims`, not `Claims`.** The fixture corpus declares
  user types named `Claims`, and a built-in of that name would break those
  programs. This is the same reason `RequestHeaders` isn't `Headers` (L4).
- **The test claims are the principal's second argument.** The proposal wrote
  `Claims.of` as a second argument to `by`, which this is. There is still no
  separate `with` clause.

**Consequences.**

- An `Oidc` handler can make authorisation decisions on any string claim of a
  verified token.
- R4 binds a trusted publisher to a repository id and a workflow file, and
  proves the exchange against a fake issuer through `trustFrom`.
- Without `trustFrom`, and without a binder that reads `.claims`, the emitted
  output is unchanged apart from the binder's `deps.claims`.
