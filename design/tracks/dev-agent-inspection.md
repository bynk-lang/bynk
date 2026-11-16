# `bynk dev` agent inspection — which instances exist, what state they are in, how they got there

- **Status:** Accepted direction. The maintainer accepted the proposal with Decisions B and D as recommended, settled A, C and E, and made it a track (Decision E) on [#3581](https://github.com/bynk-lang/bynk/issues/3581), which is the spine, in [this comment](https://github.com/bynk-lang/bynk/issues/3581#issuecomment-6901057440). No language surface.
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md), run GitHub-native per [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md). It qualifies on two axes: it spans several increments (list and read, then history), and it adds a security-relevant runtime surface, which must be absent from deploy builds.
- **Consumer:** [vscode-bynk#145](https://github.com/bynk-lang/vscode-bynk/issues/145) highlights an instance's current state on the `bynk/agentStateModel` diagram. That work is not part of this track; the JSON shapes in §3 are what it is built against.
- **Sibling track:** the retired `bynk dev` request-traces track ([closing summary](../archive/retired-tracks.md); [#3580](https://github.com/bynk-lang/bynk/issues/3580)) establishes the dev-only emission pattern this track follows: an opt-in `CompileOptions` flag, a runtime module emitted only when emitted code imports it (ADR 0615), and a golden that pins deploy output.
- **Governing records:** [ADR 0152](../decisions/0152-observation-is-auto-recorded-at-the-capability-seam.md) (production carries no test cost), [ADR 0505](../decisions/0505-bynk-dev-event-stream.md) (the `bynk dev` event stream) and the layout-2 freeze documented in `bynk-emit/runtime/src/entries.ts`.

## 1. The theme

While `bynk dev` runs there is no way to see which agent instances exist, what state each is in, or how it got there. Three things only the toolchain can provide:

- the storage layout (`entries.ts`), frozen for 1.0, which has to be decoded;
- keys, which `makeWorkersAgent` hashes into a Durable Object id with `idFromName` and never stores;
- transitions, which exist only inside `commitState`.

Reading wrangler's local SQLite files from outside would tie the extension to wrangler's private layout and still could not recover keys or history.

The track adds, under `bynk dev` only: a way to list the instances of an agent class by key, a way to read one instance's decoded state and schema version, and (slice 1) its recent transitions.

## 2. Decisions (settled)

- **A. How instances are listed.** A dev-only **registry Durable Object** per Worker. `makeWorkersAgent` adds `{class, key, id}` to it on first address. A Durable Object does not depend on the project having a KV binding. It is emitted only in a dev build.
- **B. How state is read.** A dev-only `/_bynk/inspect` route on the agent's Durable Object. It reuses the class's load path and never migrates or commits: it is read-only. It returns the state in wire shape, with collections paged.
- **C. History.** A bounded ring buffer appended in `commitState`, in dev builds only. It is stored under a reserved dev-only storage key outside layout 2's namespaces, so it survives eviction. Each entry is `{handler, old, new, at}`, the shape `emit_agent_history_driver` returns. It is its own slice, after list and read.
- **D. Security.** Compiled out of deploy builds entirely, and a golden asserts the route and registry are absent. In dev, the route requires the signature of `LOCAL_CALL_KEY`, as internal calls do.
- **E. Track.** This issue is the spine. Slice 0 is list and read; slice 1 is history. A behavioural fixture confirms that outcomes are unchanged with inspection on.

## 3. Architecture

- **The dev-only switch.** `CompileOptions::inspect_agents`, set only by `bynk dev` and off for every other build, in the way `trace_requests` is. `bynk dev` patches the registry's binding and class into the *development copy* of each Worker's `wrangler.toml`, as it does for AI Search, so the config `bynk deploy` reads is untouched. It is the compile-time half of what the issue calls "emitted only under `BYNK_DEV`": a deploy build never receives it, so there is no runtime check to get wrong. Without it the emitted Worker and Durable Object code is byte-identical to today's, and a golden pins that (`Decision D`).
- **The registry.** A Durable Object class, `__BynkInspectRegistry`, with its own binding and its own migration entry in the *development* `wrangler.toml` only. It is addressed by a fixed name and stores one record per instance, keyed `<class>/<serialised key>`, holding `{class, key, id}` where `key` is the key in wire form. `makeWorkersAgent` writes it on first address from a Worker (best effort, never failing the call: it is a fire-and-forget fetch from the factory, which has no `ExecutionContext` to `waitUntil` on, so a write can be dropped if the request ends first, and the instance then appears once it is addressed again. "First address" is per isolate and the dedupe is cleared when a write fails; the put is idempotent, so a rebuild that resets it only repeats a write). The registry never holds state, only the fact that an instance was addressed.
- **The inspect route.** The context Worker answers `POST /_bynk/inspect/agents[?class=C]` (the registry's listing) and `POST /_bynk/inspect/agent/<Class>?id=<id>` (one instance; it is addressed by the Durable Object id the listing returns, so a key never appears in a path and needs no encoding there), the latter forwarded to the class's Durable Object as `/_bynk/inspect`. They are `POST`s with no body because the call signature covers the method and the published signer signs `POST`; both are read-only. Both are verified with `__verifyCall` under a reserved caller, `bynk.dev`, whose edge key `bynk dev` passes to each Worker as `BYNK_CALL_FROM_BYNK__DEV`. A request without a valid signature is refused with a bare 401, as for `/_bynk/call/`, so an unauthenticated sender learns nothing.
- **A read never writes.** The agent's `fetch` does not run `loadState`, because `loadState` upgrades an older layout by writing. It applies the same gates, `checkStoredLayout` and `checkStoredSchemaVersion`, to the raw stored record and returns what is stored, which is the wire shape (Decision B's "reuses the load path" is read as the same gates, not the same method). A record behind the build is reported with `migrationPending: true`, not migrated; one from a later build is `readable: false` with the kind of fault, never its detail. It reads storage directly and never calls `readStored` or `loadState`, so it never touches the per-instance bookkeeping those keep (`bases`, `lazyReads`, `noteOrder`, keyed by the instance); a read that landed while a handler was suspended on outbound I/O therefore cannot replace that handler's diff base. It runs inside the Durable Object, so it sees storage consistently; the in-memory bookkeeping is protected by not sharing it, not by ordering. Slice 0c's on/off fixture includes a read that lands between a handler's load and its commit.
- **Collections are paged.** State comes back as `{cells, collections}`: the `Cell` fields in full, and each collection as `{kind, size, entries?}` with entries returned a page at a time (`?field=<f>&after=<cursor>&limit=<n>`, bounded; a layout-2 collection is paged by its entry keys, a wholesale one by position). A large collection is never dumped.
- **The history buffer (slice 1).** Appended in `commitState` after a successful write, in a dev build only, under the reserved key `__bynk_dev/history`, which lies outside layout 2's namespaces (`state`, `m/`, `s/`, `l/`, `i/`, `x/`, `c/`) and is not read by `readStored` or counted by `reportUnknownStoredKeys`. Bounded (N entries, oldest dropped). A refused transition is not committed, so it does not appear there; the refusal is reported as `{handler, old, refusal}` if the slice settles that it should be (see §5).
- **The driver.** `bynk dev inspect agents [--context C] [--class A]` and `bynk dev inspect agent <Class> <key>` find the running session, sign a request with the published call signature (the helper the migrate sweep already uses), and print JSON. `bynk dev` records the context-to-port map for the session in a file under `.bynk/` for the duration of the session, so a second process can find it; it is removed when the session stops.
- **JSON shapes** are documented in the `bynk dev` reference (`site/src/content/docs/docs/bynk-cli.md`) so the extension can be built against them.

## 4. Slices

| Slice | Scope | State |
|---|---|---|
| 0a | Emitter and runtime: the dev-only option, the registry, the `/_bynk/inspect` routes and the read-only load, paged collections. A golden pins that deploy output has neither route nor registry. | tracked on #3581 |
| 0b | Driver and CLI: `bynk dev inspect`, the session file, the signed requests, JSON output. | tracked on #3581 |
| 0c | Docs (the reference, the run-locally guide, the runtime chapter of the spec). A behavioural fixture runs the same suite with inspection on and off and compares outcomes, under real workerd in CI as `system::trace_requests_workerd` does. | tracked on #3581 |
| 1 | History: the ring buffer in `commitState`, `history` in the read, a fixture with a refused transition. Retirement PR: removes this doc and carries `Closes #3581`. | tracked on #3581 |

Slice state lives on the spine, not here.

## 5. Risks and open points

- **Shipping an inspection route to production.** Not emitted unless the option is set, and only `bynk dev` sets it. A golden pins deploy output, and a test asserts the route and registry are absent.
- **A dev build behaving differently from a deploy build.** The registry write and the history append never change a commit or a call's result. The behavioural fixture compares outcomes with inspection on and off.
- **Breaking the frozen layout 2.** The registry is a separate Durable Object, and the history key is reserved and outside every layout-2 namespace; nothing in layout 2 changes.
- **A refused transition in history.** Whether a refusal is an entry is settled in slice 1 against the issue's `{handler, old, new, refusal?}` wording; the decision comment fixes the committed entry shape as `{handler, old, new, at}`.
- **Data in the response.** The state is the instance's own data, returned only to a caller holding the local key, in dev.

## 6. Retirement

The last slice's PR is the retirement PR: it removes this doc, appends the closing summary to [`../archive/retired-tracks.md`](../archive/retired-tracks.md), and carries `Closes #3581`.
