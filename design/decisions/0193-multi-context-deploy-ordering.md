# 0193 — Multi-context deploy: Service Bindings resolve at upload, so the order is a correctness barrier

- **Status:** Accepted (v0.170)
- **Amended** for Worker groups (#3562, the "Worker groups: several contexts in one
  Worker" ADR, D7 and D8): **D1** does not apply to a group's Service Binding to
  itself, so the ordering treats an intra-group edge as no edge. **D2**'s
  acyclicity precondition holds between Workers only if the Worker-level graph is
  acyclic: a group and any other Worker (another group or a single context) can
  form a cycle with no cycle among the contexts. The check-time error
  `bynk.deploy.group_cycle` enforces it, and must land before `bynk deploy`
  accepts a group.
- **Provenance:** #601, deploy track slice 2 (spine #558). Builds on slice 0
  (#583, v0.154).
- **Relates:** [[0179]] (provisioning state — the ledger this extends), [[0180]]
  (deploy plans before provisioning and is idempotent), [[0192]] (multi-context
  local dev — the same topology, served), [[0017]] (platform lock per deployment
  unit), [[0084]] (the driver pre-flight).

## Context

Slice 0 shipped `bynk deploy` for exactly one context: more than one was refused
as an ambiguity (`select_context(&workers, None)`). A real application is several
contexts — several Workers, one per [[0017]] — so the flagship architecture could
not be shipped at all. [[0192]] had just made that same topology *runnable
locally*; deploy was the remaining half.

Slice 2's proposal (#601) made one question **gating**, to be answered before any
code: does Cloudflare resolve a Service Binding **by name at request time**
(making upload order soft — a nicety that avoids a transient half-wired window),
or **at upload time** (making it hard — a correctness barrier)? The proposal's
working assumption was *soft*, and it named the alternative as the slice's
headline risk.

## Decision

**(D1) The assumption was wrong: resolution is at upload, and the order is a
correctness barrier.** Cloudflare's documentation states it outright:

> "the target Worker (Worker B in the examples above) must be deployed first,
> before Worker A. Otherwise, when you attempt to deploy Worker A, deployment
> will fail, because Worker A declares a binding to Worker B, which does not yet
> exist."

So a wrong order does not open a transient window — it **fails the upload**.
`deploy` therefore topologically sorts the binding graph and uploads
dependencies first, and this is load-bearing rather than tidy. The finding is
recorded here per #601's requirement that the ADR state which world we shipped.
The evidence is documentary, not a live run: the finding only *hardens* the
design (it makes an order we would have wanted anyway mandatory), so being wrong
about it would cost an unnecessary guarantee, not a broken deploy — an asymmetry
that makes doc-confirmation proportionate.

**(D2) The two-pass deploy the hard world implies is unnecessary, because the
language already forbids the cycle that would need it.** In the hard world a
`consumes` cycle cannot be uploaded in one pass at all: each Worker's upload
requires the other to exist. #601 accordingly reserved a two-pass
(upload-without-binding, then re-upload) escape. **It is not built, and should
not be.** `bynkc` already rejects a `consumes` cycle as
`bynk.context.consumes_cycle` (`bynk-emit/src/project/graph.rs`) — directly,
transitively, and self-referentially — before emit; `deploy` compiles before it
orders; so a cyclic project cannot reach the ordering step. The checker's
acyclicity invariant *is* the precondition Cloudflare's upload rule demands.

This is the slice's real finding. Cloudflare's constraint would force a two-pass
protocol on a general-purpose deploy tool; it costs Bynk nothing, because the
language does not admit the shape that needs it. #601's [DECISION C] (cycle =
warning or error?) therefore **dissolves rather than resolves**: there is no
user-reachable cycle to decide about. The topo-sort still *reports* a cycle
rather than looping forever, as defence in depth against a hand-edited build
tree — an error path with no route from source.

**(D3) The graph is read from the emitted `[[services]]`, not from the checker's
`consumes` map.** Both describe the same relation, but the emitted config is the
file wrangler actually uploads and Cloudflare actually validates — so ordering
against it orders against the thing that can fail. It also keeps the rule honest
if emission ever filters an edge (adapters are already dropped: they are not
Workers). A binding naming a Worker outside this project's build is left in the
config and *not* ordered against — an externally-managed Worker is Cloudflare's
to accept or reject, not ours to invent a node for.

**(D4) A run is resumable, never transactional, and never skips.** Each context's
state is written to the ledger as it lands ([[0180]]'s incremental posture): a
failure stops the run, keeps what landed, and names what did not. There is **no
rollback** — a half-deployed project is a real state the next plan describes, not
an error to unwind. Re-running re-pushes every selected context in order rather
than skipping the ones already recorded: `wrangler deploy` is idempotent, and
skipping on the strength of a ledger flag would silently fail to ship a context
whose *code* changed since. #601's "a re-run skips done contexts" is read as its
stated intent — a re-run is cheap and safe because KV is reused and pushes are
idempotent — not as a licence to withhold a push. The plan says `redeploy`, not
`deploy`, so the word matches the act.

The run stops at the first failure rather than continuing past it: everything
later in the order either binds to what just failed or would land in a topology
other than the one the plan described.

**(D5) The ledger gains a `workers` table; `--context` deploys one context and
does not chase its dependencies.** `--context NAME` is a targeted re-push, not a
dependency-closure deploy (#601 D4): it assumes its binding targets are live.
Knowing whether they are requires the ledger to record **deployment**, which
slice 0's KV-only state could not — a context with no KV has no `kv` entry at
all, so KV presence cannot answer "does this Worker exist?". Hence
`environments.<env>.workers.<name> = { deployed = true }`: additive, `default`ed,
so a slice-0 ledger still reads. A `--context` whose target was never deployed is
**named and refused** before any Cloudflare contact, rather than pushed into an
upload that Cloudflare would reject in its own vocabulary.

## Consequences

A multi-context project ships in one command, in an order that is correct by
construction rather than by luck, and the plan discloses that order before
anything is touched. The `deploy` half of the flagship architecture is now level
with the `dev` half ([[0192]]).

The word "ambiguous" leaves the driver entirely. It existed because two commands
each acted on one context — `dev` (ADR 0096 D3, superseded by [[0192]]) and
`deploy` (slice 0). Both now act on all of them, so `SelectError::Ambiguous` and
the singular `select_context` are deleted rather than reworded: several contexts
is the expected shape of a Bynk project, and nothing about it is a question.

Deliberately still open: environments beyond `default`, secrets (slice 3, #602),
and DO migrations / queue provisioning (slice 1, #600) — slice 1's per-context
steps drop into this slice's loop without touching the ordering, which is why
slice 2 landing first costs nothing. Rollback and release engineering remain
non-goals (track §2, Q5).
