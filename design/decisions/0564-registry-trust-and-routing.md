# 0564 — A registry's key is pinned or trusted on first use, the lock records the trust anchor, and an organisation resolves in one place

- **Status:** Accepted (v0.383)

**Context.** Track §6.2 and D12 settled trust on first use with a locked fingerprint, rotation by chain, and
organisation routing that makes dependency confusion structural.

**Decision.**

- A client trusts a key pinned in `[registries.<name>] key`; else the fingerprint `bynk.lock` recorded; else the
  newest key in `config.json` on first use, which it announces and records. It accepts every key the rotation
  chain reaches **from** that trusted key. The lock records that trust **anchor**, not the newest key, so a
  planned rotation changes no lock and lines signed before it still verify. A config whose keys don't lead
  from the anchor, or that declares a different `id` than the lock recorded, is `bynk.registry.key_mismatch`.
  Recovering from a compromised key is a manual pin and `bynk update`.
- An organisation routes to the registry whose `organisations` lists it — the root's (or workspace's) map
  only, with no per-dependency override — else to the first-party registry, which isn't live yet
  (`bynk.registry.unavailable`; one constant enables it in 4b). A locked package whose organisation now routes
  elsewhere is `bynk.registry.source_mismatch`, in every mode.
- A mirror is a copy of the origin's files (its `config.json` included); lines read through it are verified
  against the origin's trust.
- A token comes from `BYNK_REGISTRY_<NAME>_TOKEN` or `~/.config/bynk/credentials.toml` and is sent only to the
  exact origins of the registry's index and archive base; redirects are followed by hand, a cross-origin one
  drops it, and `https`→`http` is refused (`bynk.registry.http_failed`). It never appears in a file or a message.

**Consequences.** A locked build with its trees in the store reads no registry at all; a rotation is followed
the next time an index is read.
