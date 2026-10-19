---
title: Versioning & roadmap
---
Bynk is pre-1.0 and is built **spec-first**, in small increments. This page
explains the method and what it means for the docs and for you.

## The spec-first, incremental method

Each language increment (`v0.X`) starts as a written specification, is then
implemented behind a growing fixture suite, and only then is considered done.
Increments are deliberately small: a slice of grammar, a refinement to the type
checker, a new emission detail. This is why the version number moves in fine
steps (the book is written against v0.366) rather than in large releases.

The discipline that keeps it honest is the fixture suite: a large body of
positive examples (which must compile to the expected TypeScript) and negative
examples (which must fail with the expected diagnostic). A feature is not
"shipped" until it is fixtured, and — as of this documentation effort — until its
docs are updated in the same change.

## Document the present

A direct consequence for these docs: **they describe what compiles today.**
Every example in the book is run through the compiler. Where a feature is planned
but not yet shipped, it is marked as planned rather than written as if it exists.
The guiding rule is that aspirational design must never masquerade as current
behaviour.

## What's next

Forward work is planned as a queue of small, single-purpose increments rather
than a fixed version-by-version schedule — a schedule pinned to version numbers
on this page is exactly what goes stale the moment an increment ships. The live
queues are kept under `design/`: the active proposal in `design/proposals/`, and
the planning backlogs (`bynk-tooling-proposal-queue.md`,
`bynk-refactor-proposal-queue.md`). By theme, the current edges are:

- **Language surface.** Continuing to round out the service, effect, and
  standard-library surface — one single-purpose increment at a time.
  Language/stdlib work and platform-adapter work never share an increment
  (decision record 0023 in `design/decisions/`). One possible follow-up to
  one-line `--|` docs ([#1888](https://github.com/bynk-lang/bynk/issues/1888)) is
  an end-of-line doc for a member (`repo: Repo, --| The repository.`), if the
  doc-above form proves too tall for short records; today a `--|` after code is
  a comment.
- **Events.** Slices 0–2, 3a–3c, and 4 have shipped — `event` declarations,
  `given Events` emission with owner-only enforcement, `from Events(E)`
  subscription across contexts on every platform, structural pattern
  filtering on the subscription header (`from Events(E { field: value, .. })`,
  delivery filtering only — no static narrowing of the handler's parameter),
  a runtime envelope (`on event(e: E, env: EventEnvelope)`) enabling the
  `Idempotency`-based dedup idiom for effectful subscribers, field defaults
  on an event's own fields so an older wire event missing a newer key still
  deserialises, an optional `@schema(N)` annotation, the cross-build
  schema registry (`bynk.schema.lock`) that computes each event's version
  from its build history — auto-bumping on a purely additive shape change,
  failing the build on anything else, and verifying a declared `@schema(N)`
  against the computed value rather than trusting it outright — and
  `via schema(N)` version-aware dispatch, filtering delivery by the
  envelope's version. See
  [Understand events](/book/guides/events/understand-events/).
  Range-valued `via schema(...)` patterns (slice 4b,
  [#990](https://github.com/bynk-lang/bynk/issues/990)) and
  replay/backfill (split to a separate future track) are the track's only
  remaining open work.
- **Editor tooling.** Deepening the `bynkc-lsp` experience — completion,
  navigation, and diagnostics — and the VS Code extension that surfaces it. Its
  model views (sequence diagram, documentation, architecture map, wire
  contract) are now live: they refresh on edit, follow the cursor, and survive
  a reload.
  The extension runs in vscode.dev and github.dev from the WebAssembly
  compiler, with project-wide diagnostics, hover and completion
  ([#2102](https://github.com/bynk-lang/bynk/issues/2102)). Full `bynkc-lsp` on
  WebAssembly (navigation, rename, references) is a later follow-up.
- **Distribution.** Publishing the compiler, grammar, and extension through
  their registries as the project approaches a public 1.0.

The larger capabilities that are designed but intentionally held back are listed
next.

## What is deferred to v1

Some capabilities are designed but intentionally **deferred, not missing** — they
are scheduled for later increments on the road to v1:

- **Sagas** — coordinating multi-step workflows across contexts.
- **Event replay** — re-delivering past events to a subscriber that joins late or
  recovers, with ordering across concurrent publishes.

Storage kinds, once on this list, have shipped: an agent's `store` fields are
`Cell`, `Map`, `Set`, `Cache` or `Log` (see [Agents](/book/reference/agents/)).

"Deferred, not missing" matters because it shapes how you read the rest of the
book. Their absence is a roadmap decision, not an oversight; when they land, they
will arrive as specified increments with fixtures and docs, exactly like every
feature before them.

## Compatibility during 0.x

While Bynk is pre-1.0, increments may change behaviour. The reference
[changelog](/book/reference/changelog/) records what changed in each increment and
notes breaking changes. Full multi-version documentation is itself deferred until
the run-up to 1.0, when stability makes it worthwhile; until then the book tracks
a single current version.

## See also

- [Why Bynk exists](/book/about/why-bynk-exists/) — the motivation behind the design these
  increments are building toward.
- Reference: [version compatibility & changelog](/book/reference/changelog/).
