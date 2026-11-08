---
level: patch
changelog: "`bynk build` builds a project with no flags, taking `out`, `target`, `platform` and `emit` from a new `[build]` table in `bynk.toml`; `bynk compile` is unchanged and ignores it (#3054)"
---

## ADR: bynk-build-verb
title: `bynk build` and the `[build]` table
summary: build infers from the project, compile is told; the first slice has no profiles

**Context.** The crate-hygiene track left `bynk` owning the project model, but a project still had no way to say how it builds: `bynk compile` needs `-o` and takes topology, platform and artefact language from flags, and `[paths] out` was removed. Proposed in #3054.

**Decision.** `bynk build [INPUT]` resolves `out`, `target`, `platform` and `emit` as flag, else `[build]`, else the built-in default (`out`, `bundle`, `cloudflare`, `ts`), and runs the same body as `bynk compile`. `build` takes a project directory only and prints one summary line. `bynk compile` stays explicit and never reads `[build]`, so the editor's Show Compiled Output cannot change with the manifest; it is not aliased or deprecated. `[build]` describes the artefact; the reserved `[deploy]` (#551) will describe how artefacts map to deployment units. `dev`, `deploy`, `tail` and `test` fix their own topology and output and do not read `[build]`. `out` must be relative and inside the project. Named profiles (`dev`/`release`, `build` defaulting to `release`, precedence CLI over profile over `[build]`) and per-context progress output are later slices.

**Consequences.** `bynk build` equals `bynk compile . -o out` for a project with no `[build]`. A custom `out` is not added to discovery's skip set: the output holds no `.bynk` files, so nothing re-reads it; only the default `out` is skipped.
