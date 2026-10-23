---
level: minor
changelog: "The account client (first-party registry R3.1, [#2378](https://github.com/bynk-lang/bynk/issues/2378)): `bynk login` signs in to a registry's accounts through GitHub's device flow and keeps the account token in the credentials file's `[accounts.<name>]`; `bynk logout` signs out; `bynk org` claims, shows, blocks and unblocks an organisation, adds and removes members, and mints and revokes publish tokens, with `token mint --save` writing the new token where `bynk publish` reads it"
---

## ADR: registry-account-client
title: The account client — `bynk login`, `bynk logout` and `bynk org` drive a registry's account API
summary: Top-level commands; the account API beside the verified `api`; the account token kept apart from publish tokens; `token mint --save`

**Context.** The hosted first-party registry gained accounts in R3 (#2363):
GitHub device-flow sign-in that mints `bynka_…` account tokens, organisations
with owners and members, claim-blocking, and expiring, revocable `bynk_…`
publish tokens. Driving them meant `curl`: copying `poll` ids between calls,
polling by hand, and pasting a publish token into the credentials file. R3
named a client subcommand as its follow-on. The obvious home, `bynk registry`,
is operator tooling over a registry directory, and already has `token` (mint
for `bynk registry serve`) and `revoke` (a delegated key).

**Decision.**

- **A. Top-level `bynk login`, `bynk logout` and `bynk org`.** Managing an
  account is a client's job, like `bynk publish` and `bynk yank`. `member
  add|remove` and `token mint|revoke` nest under `org`, as the API nests them
  under `/account/orgs/<org>/…`.
- **B. Which registry, and where its accounts are.** `org` commands route by
  organisation through `[registries]`, as `bynk publish` does; `login` and
  `logout` take `--registry` (default: the first-party registry); `--registry`
  overrides everywhere. The account API is the `account` sibling of the `api`
  in the registry's verified `config.json` — the first-party service's
  convention, not a protocol field (a `config.json` key can be added later,
  backward-compatibly). A registry read through a mirror is refused, so an
  account token reaches only the origin.
- **C. Polling follows RFC 8628.** Wait `interval`, add five seconds on
  `slow_down`, stop on `expired_token` or `access_denied`, and stop at
  `expiresIn` locally whatever the server says. The client branches on the
  JSON `error` code, never the status: `400` and `403` each mean more than one
  thing. It prints the URL and code and doesn't open a browser.
- **D. The account token is kept apart.** It lives in the credentials file's
  `[accounts.<name>]` (`token`, `login`, `expires-at`), which `bynk publish`
  never reads, and `BYNK_ACCOUNT_<NAME>_TOKEN` overrides it.
  `BYNK_REGISTRY_<NAME>_ACCOUNT_TOKEN` was rejected: it is registry
  `<name>-account`'s publish variable. The file is edited with `toml_edit`,
  keeping the person's entries and comments, and written through a `0600`
  temporary file renamed into place. The client sends whatever token it finds,
  so an operator token with `operate` drives `org claim --owner`.
- **E. `token mint` prints the token once, alone on stdout; `--save` writes it
  to `[registries.<name>] token`.** That slot is one per registry, not per
  organisation, so `--save` refuses — before minting — to replace a different
  token without `--force`, and warns when `BYNK_REGISTRY_<NAME>_TOKEN` shadows
  it.
- **F. Refusals are `bynk.registry.account_refused`,** naming the registry,
  the status, and the API's `error` and `message`; a `401` says to sign in
  again. `member remove` takes a login (resolved to GitHub's id through the
  organisation's view) or an id; `token revoke` takes an id or a unique prefix
  of one.
- **G. Proof.** Unit tests over a stubbed API seam (every poll answer, the
  five-second slow-down, the local deadline, credentials edits); integration
  tests of the real binary against a loopback fake of the account API (login,
  every `org` command, `--save`, the environment variable, the mirror refusal);
  and `services/registry/scripts/check-accounts.sh` signing in with `bynk
  login` against the fake GitHub and publishing with a saved token.

**Consequences.** A person goes from nothing to a first publish with `bynk
login`, `bynk org claim`, `bynk org token mint --save` and `bynk publish`,
never handling a token. The account API's location is a convention the client
now relies on; a second implementation that puts accounts elsewhere needs a
`config.json` field first. Publish tokens still have one slot per registry, so
publishing for two organisations on one registry needs the environment variable
or `--force`; per-organisation slots would be their own change. `bynk publish`
still sends its publish token through a mirror's `api`, which this slice does
not change.
