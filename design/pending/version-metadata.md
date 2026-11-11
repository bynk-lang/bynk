---
level: minor
probes: emit_diagnostics, emit_abi_shapes, ts_writes
changelog: `bynk.cloudflare.Version` (experimental) reports the running Worker version, the first one-per-Worker platform capability, bound as `[version_metadata]` only when reached
---

## ADR: one-per-worker-platform-capabilities
title: One-per-Worker platform capabilities are used bare and bound only when reached
summary: A registry kind for bindings a Worker has once, starting with `Version` over Version metadata

**Context.** Every `bynk.cloudflare` capability had either named instances
(`capability Orders = D1`) or, for bare `Kv`, a default instance beside them.
Some Cloudflare bindings exist exactly once per Worker: Version metadata
(`[version_metadata]`), Workers AI (`[ai]`), Images (`[images]`). The
standard-library track anticipated them (D3 rule 2: "Workers AI … stays a plain
consumed capability"), but the registry had no shape for one. Whether a Worker
declared its bare `KV` namespace was decided by whether its closure reached the
`bynk.cloudflare` unit at all, so a second bare capability of that unit would
have brought an unused `[[kv_namespaces]]` stanza with it. Proposal #3300
(parent #3184).

**Decision.**

- **A. A registry kind.** `bynk_std::Unit` gains `singletons`, each a
  `Singleton { capability, binding, wrangler_table }`: the capability's fixed
  binding name and the plain `wrangler.toml` table that declares it. A
  singleton is not in `Unit::instantiable`, so `capability V = Version` is the
  existing `bynk.capability.base_not_instantiable`, now with a note saying the
  capability is used bare. Its provider takes `env` and defaults its binding
  parameter to the registry's name, which a test pins.
- **B. Bound only when reached, per capability.** The bare-resources walk
  (`ProviderNamespaces::BareResources`) records a singleton it reaches as
  `<unit>#<capability>` instead of the unit's name. The marker resolves to no
  platform, so reaching `Version` never implies the bare `KV` namespace, and
  the driver reads the markers to emit each reached singleton's binding
  (`BindingSource::Singleton`), after the D1 databases and before the Durable
  Objects. A Worker that reaches none emits byte-identically.
- **C. `Version` is the first singleton.** `Version.current() ->
  Effect[WorkerVersion]`, with `WorkerVersion = { id: String, tag:
  Option[String], deployed: Instant }`: an empty platform tag is `None`, and
  the ISO-8601 `timestamp` becomes an `Instant` (an unreadable one faults). It
  is its own capability rather than a field of `Logger`, because a health route
  and an Analytics point need it too, and it is effectful because it reads the
  environment. Experimental until a first-party service reports it.
- **D. Tests and environments.** The unit double reports a fixed version (id
  `"test"`, no tag, `deployed` at the epoch); `stub Version.current()` replaces
  it. At `system` each Worker binds `__makeIntegrationVersionMetadata()`, id
  `"integration"`. `bynk deploy` copies `[version_metadata]` into each
  `[env.*]` block, since bindings are not inherited, and has nothing to create.

**Consequences.** Workers AI and Images can join as further `Singleton`
entries with their own `env` type and double; a singleton that later needs
options (an AI Gateway id, say) adds them to the entry. The marker string is an
internal convention of one walk, documented beside `bare_singleton_marker`.
Rejected: modelling a singleton as an `Instantiable` with a forced single
instance, which would admit `capability V = Version` and a second, meaningless
binding; and keeping the unit-level KV gate, which would give every Worker that
reads its version an unused KV namespace for `bynk deploy` to create.
