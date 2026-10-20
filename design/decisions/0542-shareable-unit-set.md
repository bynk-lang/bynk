# 0542 — A package shares commons and adapters, never a context

- **Status:** Accepted (v0.374)

**Context.** The packaging track (`design/tracks/packaging.md`, spine #843, settled in #2199) makes a
Bynk project shareable as a package. Its D1 keeps the spine's answer to "what can a package share?",
and slice 1 (#2207) is where the manifest first names the shared units. Two facts force the answer.
A **context** is a deployment unit: its Worker, its secrets and every deploy-ledger key derive from
its name and belong to the project that deploys it (`deployed_worker_name`, `bynk.deploy.lock`). A
**commons** or **adapter** is code and a host boundary: it has no deployed identity of its own, so it
can be compiled into any consumer. Behavioural library units (agents a consumer mixes into its own
context) are a later question, gated on the agent-capability-encapsulation track (track slice 6).

**Decision.**

- **The set.** A package shares **commons** and **adapters** (with their bindings and `requires`).
  A **context is never shareable**, so no dependency ever produces a Worker. A package may still hold
  contexts, for examples or its own tests; they are simply never exported.
- **`exports`.** `[package] exports` lists the shared units by their dotted names. Absent, it means
  every commons and adapter the package declares. Its shape (a list of unit names) is checked with
  the rest of the manifest, as a `ProjectPathsError`. Whether each entry names a commons or adapter
  the project declares needs the unit table, so `bynk check` checks it once units are known, as two
  coded project-level errors attributed to `bynk.toml`: `bynk.project.export_context` for a context,
  and `bynk.project.export_unknown` for a name the project doesn't declare (a first-party unit
  included), with a did-you-mean note. This is the one packaging-manifest rule that is a coded
  diagnostic (#2207 decision F).
- **One reading.** `bynk_project::read_package_config` reads `[package]` from the overlay's
  `bynk.toml` or the one on disk, as `[unstable]` is read (ADR 0468 D4). Both `run_checks`
  (`bynkc`, `bynk check`) and `analyse_project` (the language server) run the check after grouping,
  so the build and the editor agree.

**Consequences.**

- Adding a context to a package never changes what its consumers can reach.
- Under a `BYNK_BYNKC` override pinned to a `bynkc` older than this slice, `[package]` is an unknown
  table and the check fails loudly; it is never silently skipped.
- Slice 2 enforces the set from the consumer's side: only exported (or, absent `exports`, every
  shareable) unit of a dependency resolves through its alias. Slice 6's behavioural units extend the
  set by their own ADR.
