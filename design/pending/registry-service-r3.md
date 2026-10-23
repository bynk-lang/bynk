---
level: patch
changelog: "The first-party registry service has accounts (R3, [#2363](https://github.com/bynk-lang/bynk/issues/2363)): people sign in with GitHub's device flow, claim organisations, add members, block names, and mint expiring, revocable publish tokens; operator tokens remain for bootstrap and CI"
---

## ADR: registry-service-r3
title: The first-party registry's accounts and organisations
summary: GitHub device-flow sign-in (not OIDC: GitHub has no ID token for people), Organisation/PublishToken/Session agents, two token kinds, and operator tokens kept

**Context.** After R2, every publish token was minted by an operator and pasted
into the `REGISTRY_TOKENS` secret, so the hosted registry couldn't take users.
The track (`design/tracks/first-party-registry.md` §5, D4, D9) settled the
following:

- people sign in "with an OIDC provider (GitHub first), through the `Oidc`
  actor";
- organisations have an owner and members;
- names can be claim-blocked;
- publish tokens are hashed, scoped, expiring and revocable.

The accepted proposal is #2363.

**Premise correction (re-settles part of D4 and D9).** GitHub issues OIDC
tokens only to GitHub Actions workflows. That is R4's case, and the `Oidc`
actor fits it. A person signing in with GitHub uses OAuth 2.0 and gets no ID
token, so the `Oidc` actor can't be the sign-in path. GitHub stays the only
provider (D9). The mechanism becomes GitHub's OAuth **device flow**, which the
maintainer confirmed on #2363. A web authorisation-code flow was the
alternative, but it needs a response-headers language slice for cookies first.
Accepting a personal access token was rejected, because it hands the registry
a credential into the person's GitHub account.

**Decision.**

- **A. Sign-in is the device flow, run by the service.**
  - `POST /account/login` starts a device authorisation with the public OAuth
    app (`GITHUB_CLIENT_ID`; it has no client secret).
  - `POST /account/login/poll` makes one request to GitHub per call, so the
    client does the polling. It answers with RFC 8628's shape:
    - `400 authorization_pending`;
    - `429 slow_down`;
    - `410 expired_token`;
    - `403 access_denied`.
  - On success the service asks `GET /user` who the access token belongs to,
    then discards the access token without storing it. It mints a 30-day
    account token.
  - `GITHUB_BASE` and `GITHUB_API_BASE` are configurable, so tests can use a
    fake GitHub.
- **B. Two token kinds, each stored only as its sha256.**
  - An account token (`bynka_…`) manages organisations and can't publish.
  - A publish token (`bynk_…`) publishes and/or yanks for one organisation,
    for 1 to 365 days. It can be revoked by the organisation's owner, or by the
    member who minted it.
- **C. One `Organisation` agent per name.** It holds the owner, the members
  (by GitHub numeric id, with the login kept for display), whether the name is
  blocked, and the tokens it has listed.
  - Claims are first come and atomic.
  - A reserved name (`registry.paths.isReserved`, track Q6) is refused,
    except to an operator.
  - A blocked organisation refuses every publish.
  - Only the owner manages members and blocking. Members mint tokens.
- **D. A `PublishToken` agent per token digest.** A publish makes one call to
  it to decide the token: unknown, lapsed, revoked, another organisation,
  another capability, or allowed. It also makes one call to `Organisation` to
  check for a block.
  - **Minting** lists the token on the organisation first, then writes its
    grant. A failure in between leaves a listed token that authorises nothing.
  - **Revoking** revokes the grant first, then unlists the token. A half-done
    revocation therefore never leaves a working token that the owner can't see.
- **E. Operator tokens stay.** `REGISTRY_TOKENS` is now optional. It holds
  non-expiring operator tokens, which are the bootstrap and CI path. A token
  with `operate` may act on the account API as any organisation's owner,
  including claiming a reserved name on someone's behalf.
- **F. Proven end to end.** `scripts/check-accounts.sh` runs in CI's
  `registry-service` job against `scripts/fake-github.py`. It:
  - signs two people in through the device flow;
  - claims names, adds a member, and checks that a reserved name is refused
    and that an operator can claim one for someone;
  - mints tokens and passes all 23 checks of the full `bynk registry
    conformance` run with an account-minted token;
  - checks that a revoked token, a blocked name, a stranger and a signed-out
    session are each refused.

  Unit tests cover the agents' decisions and the device flow with GitHub
  stubbed.

**Deviations from the accepted proposal.**

- **`REGISTRY_TOKENS` is optional.** A registry with no table has no operator
  tokens. Before R3, a missing table was a `500`.
- **A blocked name doesn't hide an index file already in the bucket.** The
  read path is unchanged. The README says to block a name before anything is
  seeded into it.

**Consequences.**

- Anyone with a GitHub account can claim an organisation and publish with
  tokens that expire and can be revoked.
- The registry keeps no passwords and no GitHub credentials.
- Trusted publishing from CI (R4) adds the `Oidc` actor beside the account
  tokens.
- A `bynk registry login` / `account` client subcommand is a follow-on.
- The protocol and the client are unchanged.
