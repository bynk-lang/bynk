# 0865 — `bynk dev` lists agent instances and reads their state through a dev-only registry and a signed inspect route

- **Status:** Accepted (v0.629.5)

**Context.** While `bynk dev` runs, nothing shows which agent instances exist or
what state each is in. A key is hashed into a Durable Object id and never stored,
the storage layout (`entries.ts`) is frozen for 1.0, and only the toolchain can
decode it. The track is `design/tracks/dev-agent-inspection.md`; its spine is
#3581.

**Decision.**

- **Dev-only by option, not by a runtime check.** `CompileOptions::inspect_agents`
  is off by default and only `bynk dev` sets it. The recording and the routes are
  emitted only when it is on, in a new runtime module, `inspect`, which ADR 0615
  emits only when emitted code imports it. A deploy build is byte-identical, and a
  test pins that: it has no route, no registry and no recording.
- **A registry Durable Object lists instances.** An agent's factory hands its
  agent back through `__noteAgentInstance`, which records `{class, key, id}` in the
  Worker's `BynkInspectRegistry` the first time a key is addressed. The key is in
  wire form, through the agent's key codec. Recording is best effort and never
  changes a call: it is neither awaited nor allowed to reject.
- **A read never writes.** Each agent's `fetch` answers `/_bynk/inspect` from its
  own storage. It does not run `loadState`, which upgrades an older layout by
  writing; it applies `checkStoredLayout` and `checkStoredSchemaVersion` to the raw
  record and returns what is stored, which is the wire shape. A record behind the
  build is reported with `migrationPending`, not migrated, and one from a later
  build as unreadable with the kind of fault, never its detail. Collections are
  paged: a layout-2 collection by its entry keys, a wholesale one by position.
- **The route is signed as an internal call is.** The Worker's `/_bynk/inspect/`
  route verifies the call signature before it looks anything up, under the local
  edge key and the reserved caller `bynk.dev`; anything else gets a bare `401`.
  It admits only that caller: a request naming any other, a sibling context that
  holds an edge key to the Worker included, is refused before the signature is
  checked. It is a `POST` with no body, because the signature covers the method
  and the published signer signs `POST`. The signature covers the path and not the
  query, so a captured request can be replayed within the signature window against
  another instance; this is local development, and the key is the well-known local
  one.
- **The dev config is patched, not emitted.** The registry's binding and class are
  added to the development copy of `wrangler.toml` by `bynk dev`, as the AI Search
  proxy is, so the config `bynk deploy` reads is unchanged.

**Consequences.** A dev build differs from a deploy build in each agent factory,
each agent's `fetch` and each hosting Worker's entry, so a behavioural fixture
runs the same suite with inspection on and off and compares outcomes (slice 0c).
An instance created before the session is listed once it is addressed again. The
history ring buffer (slice 1) is stored under a reserved key outside layout 2's
namespaces.
