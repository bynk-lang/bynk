---
level: patch
changelog: The design of Worker groups (`[deploy.workers]`) is recorded in an ADR, which amends ADR 0193 (D2) and ADR 0017 (#3562, G7)
---

## ADR: worker-groups
title: Worker groups: several contexts in one Worker
summary: How a `[deploy.workers.<key>]` group is identified, laid out, dispatched, checked and (not yet) deployed, and what it amends in 0017 and 0193

- **Provenance:** #3562 (#551), G1 to G6 groundwork: #3566, #3569, #3571, #3574,
  #3575, #3576, #3628, #3635, #3640, #3641, #3642, #3643, #3647, #3649, #3655,
  #3658, #3659.
- **Amends:** [[0017]] (the deployment unit) and [[0193]] D2 (why no two-pass
  deploy); D1 of [[0193]] gets a stated exemption.
- **Relates:** [[0180]], [[0192]], [[0179]].

**Context.** A context is one Worker ([[0017]]), so a handful of contexts that
belong together cost a Worker and a network hop each. A group puts several in one
Worker: a `[deploy.workers.<key>]` table in `bynk.toml` lists contexts, and the
build emits one Worker for them. Everything below is built and merged for the
**build**; `bynk deploy`, `revert` and `bynk dev` still refuse a group
(`bynk.deploy.worker_groups_unsupported`) until G4 and G6.

**Decision.**

*D1: identity and names.* The group's **key** is its identity: its directory
`workers/<key>/`, its ledger entry, its future `--worker` selector. `name`
overrides only the Cloudflare script name (default: the key), which is what the
Worker's `wrangler.toml` and a Service Binding to it use. Names derived for
resources (D1 databases, R2 buckets, analytics datasets, Vectorize indexes) and
the contract lock's `worker` follow the key, so renaming a script moves nothing.
Lists are explicit: no globs and no `"*"` group in v1, so adding a context to a
group is a visible diff, and a context in no group stays one Worker named by its
dashed name.

*D2: one layout, one switch.* `WorkerLayout::for_project` reads the table and is
where the build, the checker and the editor learn where each context's Worker is.
A member's own files carry its prefix in the group's directory
(`<member>.index.ts`, `.compose.ts`, `.handlers.ts`, `.events_fanout.ts` and its
manifests); the files that belong to the Worker as a whole (`index.ts`,
`wrangler.toml`, `containers.ts`, assets) are written once. Groups are a Workers
topology: any other target has each context alone, and a `bundle` build warns that
it ignores them (`bynk.deploy.workers_ignored_on_bundle`).

*D3: one `wrangler.toml`.* Bindings merge by name: an identical capability
declaration in two members is one binding (the point of co-location is shared
state), a differing one is `bynk.deploy.binding_collision`, and an agent is never
shared (its class is the code of the context that declares it). Crons, queues,
email addresses and class tombstones union; assets, edge cache and the wasm rule
are added once.

*D4: dispatch.* The Worker's `index.ts` tries the members in the order `contexts`
lists them. A member's own router answers are **marked** (a module-level
`WeakSet`, read through the exported `isMiss`, never a header): the final `404`,
the synthesised `405`/`204` with `Allow`, an unknown internal target. A response a
handler returned, including its own `404`, is never marked, so a route a member
owns does not fall through. A marked response is a miss and the next member is
tried; if every member misses, the answer is the first marked non-`404` with the
`Allow` of all of them merged (`GET /x` in one member and `POST /x` in another is
legal, so the `POST` must reach the second), else the single-context `Not Found`.
A marker beats a separate `handles(request)` predicate, which would re-derive each
exit and drift. For the same reason a member decides **ownership before it reads
the body or verifies a signature** on `/_bynk/call`, `/_bynk/event`,
`/_bynk/migrate` (and the dev-only `/_bynk/inspect`), so a miss leaves the body
intact for the next member. `scheduled` runs every member whose cron matches,
`queue` goes by `batch.queue`, `email` by recipient (an exact address before the
domain's catch-all). Agent, fan-out and container classes are re-exported by name:
every member entry exports `isMiss`, so `export *` would collide.

*D5: unclaimed internal requests answer as an unauthenticated standalone Worker
does.* When no member claims a `/_bynk/call`, `/_bynk/event` or `/_bynk/migrate`
request, the group answers it as a standalone Worker answers it unsigned: the same
method gate, then the runtime's own `verifyCall` (`413` for an oversized body,
else `401`). A `404` for an unknown service next to a `401` for a known one would
let an unsigned caller list a group's services. The one visible difference: an
authenticated call to a service **no member declares** gets `401` in a group,
where a standalone Worker says `404`. A `bynk dev` inspect request is answered the
same way (`405`, then `401`), and a deploy build carries no inspect route at all.

*D6: checks over a group.* Beyond the manifest's shape (`bynk.deploy.empty_group`,
`context_in_two_groups`, `group_name_conflict`, `unknown_context`,
`not_a_context`, `duplicate_hostname`): two members may not declare the same HTTP
route (`route_conflict`, keyed on method and path) or a service of one name
(`service_name_conflict`, since `/_bynk/call/<service>` names no callee context);
a queue's single consumer and an email address's single claimant are project-wide
rules (`bynk.queue.duplicate_consumer`, `bynk.email.duplicate_address`), grouped or
not; a group has at most one member that declares `from websocket` services
(`multiple_websocket_members`), because an upgrade request names no path or
service to route by; a fixed asset path two members serve from different files, or
one that would be a folder of the other's, is `asset_path_conflict`, and a fixed
asset shadowing another member's literal route is `bynk.asset.shadows_route`
across members (assets merge into one directory, so only overlaps are errors); and
a group has at most one member that publishes events
(`bynk.deploy.multiple_publishers`), which is **temporary**: each publisher has its
own events fan-out class, and the restriction lifts when G3 merges a group's
fan-out into one class routing every member's events.

*D7: a group's self-binding is exempt from [[0193]] D1.* An edge between two members
is a Service Binding from the group's Worker **to itself**, which Cloudflare
accepts on a first deploy (probed on the scratch account with wrangler 4.162.0;
wrangler then exits non-zero with `10007` from a follow-up lookup after the upload,
which G4 must treat as non-fatal once the script is confirmed to exist). The rule
that the target Worker must exist before the binding is declared cannot apply to a
Worker binding to itself, so the dependencies-first ordering treats an intra-group
edge as no edge.

*D8: amendments.*

- **[[0017]]:** the deployment unit under `workers` is a context **or a group of
  them**. A platform-native capability in any member locks the whole Worker, and
  the diagnostic names the group, not a context.
- **[[0193]] D2:** the checker's `consumes` acyclicity is the precondition for
  uploading in one pass **only between Workers whose nodes are contexts**. With
  groups the nodes are Workers, and two groups can form a cycle with no cycle among
  their contexts (`A1 -> B1` and `B2 -> A2`, with `A1`, `A2` in one group and
  `B1`, `B2` in another). D2's conclusion, that no two-pass deploy is needed,
  stands **if** the group-level graph is acyclic: that check is G3's (the group
  cycle check), and G4 does not deploy a group before it exists.

**Consequences.**

- A group builds today; it cannot yet be deployed or run in `bynk dev`. Both
  refuse it with `bynk.deploy.worker_groups_unsupported`, which G4 (deploy,
  `--worker`, ledger and facts versions, the `10007` handling) and G6 remove.
- **Still open, G3:** in-process edges and events between members (an intra-group
  call is a Service Binding to itself today), a merged events fan-out (lifting
  `multiple_publishers`), the group cycle check, and a conformance harness.
- **Still open, G5:** moving an agent-owning context into or out of a group is
  refused in v1 (`bynk.deploy.agent_move_refused`); the scratch-account probe
  showed a data-preserving class transfer works with the declarative `exports` map
  (a binding-free receiver, a `transferred` tombstone on the source, a cleanup
  deploy), so supported moves are a follow-on before 1.0, with revert.
- **Still open, G6 (the rest of it):** `bynk dev` addressing a group's Worker. The
  inspect listing needs a stated contract: today the first member that hosts an
  agent answers `/_bynk/inspect/agents` and the request is verified for, and the
  response reports, **that member's** context, while `instances` covers the whole
  Worker's registry. The client must learn which member lists before it can sign the
  request, and either sign for the group's listing member or have each instance
  carry its member. The routes also want a runtime test once `bynk dev` enables
  groups.
- **Known limits.** A traced build emits one trace line per member tried. A
  WebSocket upgrade goes to the one member that declares `from websocket` services,
  whatever the path. Each member's `compose` runs per request until one owns it; it
  only builds closures.
