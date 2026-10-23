# 0604 — Per-organisation publish-token slots in the credentials file

- **Status:** Accepted (v0.400)

**Context.** A first-party publish token acts for one organisation (R3,
#2363), but the credentials file had one publish-token slot per registry,
`[registries.<name>] token`. Publishing for two organisations on one registry
meant exporting `BYNK_REGISTRY_<NAME>_TOKEN` before each publish, or
`bynk org token mint --save --force` (R3.1, #2378) each time you switched,
which minted a new token and left the old one live. R3.1's decision E deferred
per-organisation slots to their own slice.

**Decision.**

- **A. The slot is `[registries.<name>.organisations.<org>] token`,** nested
  under the registry beside the registry-wide `token`. `organisations` is the
  manifest's word for the same idea. Organisation names are bare TOML keys. An
  older `bynk` reads only the registry's `token` and ignores the table.
- **B. The organisation is set once per `Client`.** `bynk publish` and
  `bynk yank` already build one client for one organisation; it now acts for
  that organisation, and the whole command (`config.json`, the write, the
  read-back) carries one token. Resolution and fetch act for none, and read
  exactly what they read before. `bynk registry conformance` acts for its
  `--org`. A per-call token on the write alone was rejected: it would fail a
  registry that protects `config.json` when only an organisation's slot holds
  a token.
- **C. Precedence: the variable, then the organisation's slot, then the
  registry's slot.** The variable overrides everything, as before, so CI is
  unchanged. A per-organisation variable was rejected:
  `BYNK_REGISTRY_<NAME>_<ORG>_TOKEN` collides, because `-` maps to `_`
  (registry `foo` with organisation `bar` is registry `foo-bar`'s variable).
- **D. `token mint --save` writes the organisation's slot,** and the `--force`
  guard covers only that slot. The registry-wide slot is left alone, with a
  note that it still serves other organisations; the warning about the
  variable says it shadows every saved token.
- **E. Refusals name the token's source** (the variable or which slot, never
  the value). A `401` with no token, or a `403` from the registry-wide slot,
  suggests `bynk org token mint <org> --save`.
- **F. Proof.** Unit tests of the lookup, the slot writes and the hints; an
  integration test publishing and yanking for two organisations on one
  `bynk registry serve` with saved slots and no variable, plus the fallback,
  the variable's override and each refusal's source; and
  `services/registry/scripts/check-accounts.sh` saving tokens for two
  organisations and publishing for each against the real service.

**Consequences.** Someone who publishes for several organisations mints and
saves one token for each, once, and `bynk publish` picks the right one. Files
written before keep working through the registry-wide fallback; a file holding
only organisation slots gets a `401` from an older toolchain.
