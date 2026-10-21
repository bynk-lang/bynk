# 0560 — `bynk.lock` records every npm package a dependency may bring in

- **Status:** Accepted (v0.382)

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
