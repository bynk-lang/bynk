# 0600 — A publish or yank never goes through a mirror

- **Status:** Accepted (v0.397.7)

**Context.** A client opens a registry through a mirror when a
`[registries.<m>] mirror-of = "<name>"` entry serves it (#2330). Index lines
stay safe that way, because they are signed by the origin's key, but a mirror's
`config.json` is unsigned and is the mirror's word. `bynk publish` and
`bynk yank` took their publish `api` from it, so the upload, and the token
configured for the mirror's entry, went to whatever origin the mirror named.
The read-back that confirms a write also came through the mirror. R3.1 (#2378)
already refuses account commands on a mirrored registry for the same reason.

**Decision.** `bynk publish` and `bynk yank` refuse a registry that a mirror
serves, with `bynk.registry.publish_refused` naming the mirror. The check uses
`[registries]` alone, so it runs before any request, and `bynk publish
--dry-run` refuses too, so the dry run agrees with the real one. The other
option was to write to the origin, opening the origin's own `config.json` for
the `api`. It was rejected for three reasons:
- The read-back goes through the mirror, so a mirror that lags would turn every
  successful upload into "accepted, but its index doesn't list it".
- It would need a second, origin-keyed opened registry.
- The origin may have no usable `[registries.<name>]` entry of its own.

**Consequences.** A project that reads a registry through a mirror publishes
from a project, or a checkout, without the mirror. A mirror receives no uploads
and no tokens meant for writing. Writes and account calls now follow one rule:
anything that carries a credential to a registry's API goes only to the
registry itself.
