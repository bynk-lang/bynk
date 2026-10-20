# Bynk — The 1.0 Definition

*Decision record for [#540](https://github.com/accuser/bynk/issues/540) §7(4),
the "define 1.0" call. Third of the strategy records, after
[`bynk-positioning.md`](bynk-positioning.md) (what Bynk is) and
[`bynk-adoption-sequencing.md`](bynk-adoption-sequencing.md) (what to build next);
this one settles *when Bynk is 1.0*. A strategy record, not a language-defining
call. The §7(4) bundle also carries monthly milestone cadence and README/about
drift guards — now settled in [`bynk-release-discipline.md`](bynk-release-discipline.md);
this record settles the 1.0 bar itself.*

---

## What 1.0 has to mean here

Bynk today ships **daily breaking increments**. Each increment may change the
language, and the migration is a **one-time, in-repo hand rewrite** of the only
Bynk source that exists — this repository's own corpus — because there is no
external code to break and a shipped codemod would parse a retired surface
"forever for no caller" ([ADR 0123](decisions/0123-state-block-cutover-and-codemod.md)).
That posture is correct *precisely while Bynk has no outside users.*

**1.0 is the version where that stops.** It is the first release that makes a
**compatibility promise to code the maintainer did not write** — the moment
"migrate the corpus by hand" stops being available because the corpus is no longer
the only source in the world. So 1.0 is not a feature count and not a maturity
adjective; it is a **stability commitment**, and the definition has to say exactly
what is committed and what is not.

## The definition

> **Bynk 1.0 = Foundations-layer stability + `deploy` + state migrations.**

Three gates. They are not a wish-list of "things that would be nice by 1.0" — each
is load-bearing for one specific promise, and together they are exactly *"you can
run Bynk in production and safely upgrade it."* The unifying frame:

| Gate | Guarantees | Without it |
|---|---|---|
| **`deploy`** | you can get to production at all | there is nothing to keep stable |
| **Foundations-layer stability** | your **source** keeps compiling across an upgrade | every release is a rewrite |
| **State migrations** | your deployed agent's **persisted state** stays valid across an upgrade | a stable-source upgrade still corrupts live data |

Deploy gets you to production; Foundations stability keeps your *code* valid across
the next release; migrations keep your *data* valid across it. A production service
survives an upgrade only if all three hold — which is why 1.0 is precisely their
conjunction, and no smaller set.

### Gate 1 — Foundations-layer stability

The **Foundations layer** — "core Bynk" — is defined in the design notes §2
(*Layered surface*): bounded contexts, the service/agent split, actor declarations
for authentication, value types with opaque/transparent visibility, handlers
returning `Result[T, E]`, atomic-handler semantics, the core storage surface
(`store` fields and their write forms), and cross-agent calls — constructing an
agent with its key and calling a handler, `Counter(id).bump()`, from a service or
from another agent (the shipped form; the `Ref[A]` type earlier drafts named was
never built, #1669). It is the smallest set that reaches a working HTTP-fronted
service with a persistent agent, and most code lives there.

**Stability commits to:** a 1.0 Foundations-layer program keeps **compiling** and
keeps its **documented static and runtime semantics** across every 1.x release. A
breaking change to that layer is a **2.0 event**, not a minor — and even then is
expected to be rare and codemod-backed, not the daily hand-rewrite of today.

**Stability does *not* freeze:**

- **The coordination and advanced layers** (§2) — capabilities, events, sagas, the
  query algebra, rich storage kinds, agent invariants, held connections. These
  evolve **additively** through 1.x (below).
- **The emitted TypeScript.** The compile target is an implementation detail, not
  part of the frozen contract; the codegen may improve within a 1.x release as
  long as documented behaviour holds. (The README's "readable output" is a
  property, not a byte-for-byte promise.) The exception is the published
  **cross-context wire format** (`reference/wire-format.md`, #1969), which
  follows its own version rather than the compiler's or the language's, and
  the **stored-state encoding** — what an agent's state is written as — which
  Gate 3 freezes: a 1.x release may change it only by shipping the
  compiler-generated step that re-encodes stored data, otherwise it is a 2.0
  event ([`tracks/state-migrations.md`](tracks/state-migrations.md) §3.7).
- **The tooling surface** (LSP, formatter, driver flags) — versioned with the
  toolchain, not the language-stability promise.

The point of naming the layer is that 1.0 does **not** wait for the whole language
to stop moving. It freezes the part real services stand on, and lets the rest keep
growing.

**The standard library** (amended by the standard-library track, #1936, D2;
recorded in the stability-tiers ADR that slice S3, #1963, lands). Every
first-party stdlib item is marked *stable* or *experimental* in the `bynk-std`
registry, and the promise depends on its layer:

- **Stable kernel and commons items** (L0 built-in types and kernel methods, L1
  `bynk.*` commons) are **Foundations-level**: a breaking change to one is a 2.0
  event. Nearly every Foundations program calls them, so leaving them out of the
  frozen layer would make the promise hollow.
- **Stable capabilities** (L2 the `bynk` surface, L3 platform adapters such as
  `bynk.cloudflare`) are **additive-only** through 1.x, which is what "evolve
  additively" above means for them. They may gain items and operations. An enum
  may gain a variant only if it is marked *extensible*, because a `match` covering
  an extensible enum must already carry a wildcard. An existing stable item may be
  deprecated in a minor release (a non-failing warning, ADR 0117) but removed or
  reshaped only in a major one.
- **Experimental items** carry **no promise**. They may change or disappear in any
  release, and a project uses one only by listing it under `[unstable] allow` in
  `bynk.toml`, so the edge of the promise is visible in the manifest.
- **The platform carve-out.** A stable L3 item may break in a *minor* release only
  when the platform withdraws or incompatibly changes the feature underneath it,
  and that release's cumulative migration note names it. A Bynk-side redesign is
  never covered by the carve-out.

### Gate 2 — `deploy` (satisfied)

`bynk deploy` — provisioning + remote deploy — must exist. This is the first
[adoption blocker](bynk-adoption-sequencing.md) and the capstone of the
`doctor → new → dev → deploy` driver arc. The rationale is definitional: a
**stability** promise is meaningless for software that cannot reach
production in the first place. There is nothing to keep stable until there
is something deployed. The `deploy` feature track shipped all six slices
(v0.154–v0.220.2) and retired — see its closing summary in
[`archive/retired-tracks.md`](archive/retired-tracks.md).

### Gate 3 — state migrations

A **state-schema evolution** story — the second [adoption blocker](bynk-adoption-sequencing.md)
(track: [`tracks/state-migrations.md`](tracks/state-migrations.md), spine
[#2188](https://github.com/bynk-lang/bynk/issues/2188)) — must exist. This is the runtime-data counterpart to Gate 1:
Foundations stability keeps *source* compiling, but a deployed agent owns
**persisted state**, and the first breaking change to that state's shape is
unshippable without a migration path. Note this is **distinct** from the platform
DO-migration application `deploy` already wires up
([ADR 0194](decisions/0194-deploy-queues-and-delegated-do-migrations.md)),
which drives Cloudflare's own mechanism; Gate 3 is evolving the *Bynk* state
schema a stable-source upgrade reads. Source-compatible and data-compatible
are two promises, and a real upgrade needs both.

## What is explicitly post-1.0 (additive)

**1.0 is a stability milestone, not a feature-completeness milestone** — the single
most important thing this definition decouples. The design notes describe an
aspirational v1 language far larger than 1.0's bar; conflating "1.0" with "the
whole v1 vision" would push 1.0 out indefinitely and defeat the point of making a
stability promise early.

So the v1 **coordination layer and beyond — events, sagas/compensation, the query
algebra and rich storage kinds, agent invariants, held connections** — is
**explicitly post-1.0 and additive.** These land as **minor** 1.x releases *after*
1.0, adding capability without breaking the Foundations layer. They are the
reason 1.x keeps moving; they are not a reason to delay 1.0.

## What 1.0 is *not*

- **Not feature-complete** against the design notes' v1 language (that is the 1.x
  additive line above).
- **Not the ecosystem/registry.** Ecosystem posture is the *third* adoption
  blocker and is adoption-critical, but **1.0-optional** — a language can be stable
  and deployable before its package registry exists
  ([`bynk-adoption-sequencing.md`](bynk-adoption-sequencing.md)). It lands around
  and past 1.0, not as a gate on it.
- **Not a maturity claim beyond the compatibility promise.** 1.0 says "Foundations
  is stable, you can deploy, and your state can evolve" — nothing more is being
  asserted.

## Interlocks

- **With the adoption sequence (§7(2)).** Two of the three gates — `deploy` and
  state migrations — *are* the first two adoption blockers, in order. So the
  [sequencing decision](bynk-adoption-sequencing.md) **is the road to 1.0**: ship
  deploy, ship migrations, hold the Foundations layer stable, and Bynk is at the
  1.0 line. The third blocker (ecosystem) carries past it.
- **With the semver/cadence story (§7(4)).** A stability promise needs a release
  discipline that can *keep* it: named milestones with cumulative migration notes
  ([ADR 0123](decisions/0123-state-block-cutover-and-codemod.md) is the
  codemod/cutover template), and drift guards so the docs never over-promise what
  compiles. Those are the *mechanism* that makes 1.0's promise sustainable, now
  settled in [`bynk-release-discipline.md`](bynk-release-discipline.md).
- **With the two-deployment bar (§7(5)).** The review separately proposes an
  **empirical** 1.0 bar — two external production deployments carried through one
  breaking increment and one state migration — as *evidence the promise actually
  holds*. That is a **validation** bar layered on top of this **definition** bar:
  this record says what 1.0 *is*; §7(5) says how you'd know it's real. §7(5)
  stays open on the tracking issue.

## Out of scope here

This record settles the 1.0 definition; the other §7(4) bullets (monthly
milestone cadence, README/about drift guards) are settled in
[`bynk-release-discipline.md`](bynk-release-discipline.md). The rest of §7 — the
honest comparison page (§7(3)) and the two-production-deployment validation bar
(§7(5)) — remains open on [#540](https://github.com/accuser/bynk/issues/540).
