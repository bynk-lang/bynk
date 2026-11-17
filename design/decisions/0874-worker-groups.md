# 0874 — Worker groups: several contexts in one Worker

- **Status:** Accepted (v0.634.6)

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
across members (assets merge into one directory, so only overlaps are errors). A
group has **one events fan-out** for every member that publishes (G3-1): a member
emits no fan-out of its own and tags each event with its name, the group's
`events_fanout.ts` routes by `<publisher>/<type>` (two members may declare an
event of one name) and signs each delivery as the publisher, and its entry exports
`__EventsFanout` once. The temporary `bynk.deploy.multiple_publishers` is gone.
The fan-out signs only for the group's own publishing members, and refuses an
event naming any other. Every publishing member's events now go through **one**
fan-out Durable Object instance (`idFromName("singleton")`), where a context on its
own Worker had one per publisher. That is a deliberate choice: delivery order
within a group is one total order, and the cost is that a group's events share one
instance's throughput.

*D7: a group's self-binding is exempt from [[0193]] D1.* An edge between two members
is a Service Binding from the group's Worker **to itself**, which Cloudflare accepts
on a first deploy (probed on the scratch account with wrangler 4.162.0: first and
second deploys and a rollback with the self-binding all succeed). The rule that the
target Worker must exist before the binding is declared cannot apply to a Worker
binding to itself, so the dependencies-first ordering treats an intra-group edge as
no edge. By contrast two **different** Workers bound to each other cannot be
uploaded at all: Cloudflare rejects the upload of a Worker whose binding target does
not exist (code 10143, probed), which is why `bynk deploy` refuses that cycle.

The `10007` this decision first recorded was **not** a property of the
self-binding. It is a race on *any* first deploy of a name: after the upload lands,
wrangler looks the Worker up and can be told `This Worker does not exist on your
account [code: 10007]`, exiting non-zero (seen once on the scratch account, with
`--secrets-file`, straight after deleting the same name; the identical deploys
passed when repeated, with and without the self-binding). The upload is not what
failed, so `bynk deploy` forgives it, and only when the account itself says the
upload landed: the ledger has no record of the Worker, wrangler's output carries
`[code: 10007]`, the account had **no** script of that name before the push, and it
has **exactly one** version after it. The last two compare Cloudflare's state before
and after, so no local clock is involved, and a same-named script that predates the
deploy (another project, a lost ledger) is never mistaken for the upload. wrangler's lookup comes after the upload and before its triggers step
(workers.dev, routes, custom domains, crons), which it therefore never ran, so a forgiven
failure is followed by `wrangler triggers deploy`, and a failure of that is the
deploy's failure. If the account cannot be asked (no API token), the failure stays as
wrangler reported it.

Edge keys are named per member context (the runtime signs as the calling context): an edge between Workers is one secret, held by the callee as `BYNK_CALL_FROM_<m>` for each member `m` of the caller and by the caller as `BYNK_CALL_TO_<m>` for each member of the callee, and a group holds both for each of its own members. The members of a Worker share one `env`, so a key held for the Worker is held by all of them: within a Worker, a caller's identity between members is by convention, not cryptographically separable (any member could sign as another); across Workers it is per Worker edge.

*D8: amendments.*

- **[[0017]]:** the deployment unit under `workers` is a context **or a group of
  them**. A platform-native capability in any member locks the whole Worker, and
  the diagnostic names the group, not a context.
- **[[0193]] D2:** the checker's `consumes` acyclicity is the precondition for
  uploading in one pass **only between Workers whose nodes are contexts**. With
  groups the nodes are Workers, and a group and any other Worker (another group or a
  single context) can form a cycle with no cycle among their contexts: `A1 -> B1`
  and `B2 -> A2` with the `A`s in one group and the `B`s in another, or `A1 -> C`
  and `C -> A2` with `C` on its own. D2's conclusion, that no two-pass deploy is
  needed, stands **if** the Worker-level graph is acyclic. That is a check-time
  error, `bynk.deploy.worker_cycle`, over the Worker-level graph, for a cycle that
  **involves a group** (a group is what makes a cycle with no cycle among contexts),
  naming the Workers and one context edge per hop; it lands before G3, and G4 does
  not deploy a group without it. A cycle among contexts in no group (a subscriber
  that also calls its publisher binds back through the events fan-out) is not
  refused at check time, since `bynk dev` and the tests run it fine, and
  `bynk deploy` already refuses it before uploading (`OrderError::Cycle`):
  Cloudflare rejects the upload of a Worker whose binding target does not exist
  (code 10143, probed on the scratch account), so neither Worker of such a pair can
  go first.

**Consequences.**

- A group builds today; it cannot yet be deployed or run in `bynk dev`. Both
  refuse it with `bynk.deploy.worker_groups_unsupported`, which G4 (deploy,
  `--worker`, ledger and facts versions, the `10007` handling) and G6 remove.
- **Still open, G3:** in-process edges and events between members (an intra-group
  call is a Service Binding to itself today). The merged events fan-out has
  landed (G3-1), and so has the conformance harness (G3-0). The Worker cycle check
  (`bynk.deploy.worker_cycle`) lands before G3.
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
