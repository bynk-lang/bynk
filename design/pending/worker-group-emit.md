---
level: minor
changelog: A `[deploy.workers.<key>]` group now builds as one Worker: its members share one `index.ts` that tries them in manifest order, one `wrangler.toml` that merges their bindings, crons, queues and addresses, and a group has at most one member that accepts WebSockets (`bynk.deploy.multiple_websocket_members`); a `bundle` build says it ignores the groups (`bynk.deploy.workers_ignored_on_bundle`) (#3562, G2b step 2c-4b)
probes: diagnostic_coverage, emit_diagnostics, explained_codes, ts_writes
---

`WorkerLayout::for_project` now reads `[deploy.workers]`, the one switch the
build, the checker and the editor share. A group's directory is `workers/<key>/`.
Each member keeps its own files under its prefix (`<member>.index.ts`,
`.compose.ts`, `.handlers.ts`, its manifests); the Worker's `index.ts`,
`wrangler.toml` and `containers.ts` are written once.

The group's `index.ts` tries members in the order `contexts` lists them. Only a
router's own miss (its `404`, its synthesised `405`/`204`) lets the next member
try; a handler's response, a `404` of its own included, is the answer. If every
member misses it returns the first `405`/`204` with the `Allow` of all of them
merged, else the single-context `Not Found`. `scheduled` runs each member whose cron matches, `queue` goes by
`batch.queue`, `email` by recipient. Agent, fan-out and container classes are
re-exported by name.

An unclaimed `/_bynk/call`, `/_bynk/event` or `/_bynk/migrate` request answers as
a standalone Worker answers it unauthenticated (the same `405`, `413` or `401`),
so an unsigned caller cannot list a group's services. A signed call to a service
no member declares therefore gets `401` in a group, where a standalone Worker
says `404`.

A WebSocket upgrade names no path or service to route by, so a group has at most
one member that declares `from websocket` services. The `@worker("…")` option
check reads a Worker's script name from the layout (the group's `name`, else its
key); the contract lock keys a Worker by its directory (the group's key).
`bynk deploy`, `revert` and `dev` still refuse groups (#3643).
