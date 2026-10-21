---
level: minor
changelog: Packaging slice 3 — a dependency's adapter can wrap an npm library. When a project reaches that adapter, its `binding … requires { … }` packages install with the project's own in the generated `package.json` (pinned ranges only; two adapters, in any packages, requiring one npm package at different ranges is `bynk.requires.conflicting_range`, naming both as `org/package:adapter`), and `bynk.lock` gains `[[npm]]` rows recording every npm package a dependency may bring in and the adapters requiring it, so `--locked` stops a build when a dependency adds one
---

## ADR: npm-propagation-across-packages
title: A reached dependency adapter's npm `requires` fold into the build, with exact-string conflicts
summary: Packaging slice 3 — the slice 2a refusal is lifted; the existing fold runs over every compiled adapter, a dependency's included; only reached adapters contribute; conflicts stay exact-string; no consumer override yet

**Context.** Slice 2a (#2223) refused a dependency adapter with `requires`, so a package could not wrap an
npm library. The fold that builds `out/package.json` (#2201) already refuses an unpinned range and two
different ranges for one package.

**Decision.**

- The refusal is lifted (an `asset` in a dependency stays refused). The existing fold runs over every adapter
  the build compiles, whichever package it came from. Slice 2a's reach-closure keeps only reached dependency
  units, so an unused dependency's adapter contributes nothing (P-5), and a dev-dependency's adapter reaches
  only a test build (#2240).
- The rules carry over unchanged: pinned ranges only (`bynk.requires.unpinned_dependency`), and two different
  range strings for one npm package — between the project's adapter and a dependency's, or two
  dependencies' — are `bynk.requires.conflicting_range`, naming both adapters as `org/package:adapter`.
- npm-style range intersection and a consumer override table are recorded as follow-ons, not built: the first
  needs a faithful npm-semver implementation, and the second hands the consumer a lever over code it didn't
  write, which is the adapter-trust slice's question (slice 5).

**Consequences.** One root `package.json` still serves every Worker. A conflict across packages is fixed by
aligning the ranges in source, upstream if need be.

## ADR: the-locks-npm-rows
title: `bynk.lock` records every npm package a dependency may bring in
summary: `[[npm]]` rows — `package`, `range`, `required-by` (`org/package:adapter`) — written by `bynk` at resolve time from every resolved dependency's adapters; the superset of what a build installs; covered by `--locked`

**Context.** Track §5.2 and §7.2 put the npm sub-layer's provenance in the lock, for slice 5's approve-and-diff
to read. The lock is written by `bynk` before any compile, so it can't know which adapters a build reaches.

**Decision.**

- While resolving, `bynk` parses each resolved dependency's adapters (path and git alike, from the package's
  source roots) with `bynk_project::lock::adapter_requires`, and records one `[[npm]]` row per
  `(package, range)`, its `required-by` the sorted `org/package:adapter`s naming it. Rows follow the
  `[[package]]` rows in a fixed order. The lock stays version 1: the rows are additive.
- The rows cover every dependency adapter, reached or not — what *could* enter a build — while
  `out/package.json` holds what a build's reached adapters *do* require. The project's own adapters aren't
  recorded: the author wrote them.
- The rows are part of the lock's text, so `--locked` refuses a change to them: a dependency that starts
  requiring an npm package stops a `--locked` build until the lock is committed.

**Consequences.** Until slice 5 adds approval, committing the lock is the human checkpoint for a dependency's
new npm packages (ADR 0020 stays Open).
