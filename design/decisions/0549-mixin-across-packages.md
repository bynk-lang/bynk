# 0549 — A `uses` of a dependency's commons mixes in exactly as a local one does

- **Status:** Accepted (v0.376)

**Context.** The packaging track deferred the semantics of using another package's units to slice 2
(track D8). Slice 2a folds a dependency's units into the same flat unit set every phase already
checks, under canonical names.

**Decision.**

- A `uses` of a dependency's commons mixes its declarations in exactly as a local `uses` does; a
  dependency adapter is `consumes`-able by its aliased name and its providers are wired as a local
  adapter's are (ADR 0019).
- Nominal type identity is per `(package, unit)`: one package reached through two aliases (the
  consumer's `money`, another dependency's `m`) is one package with one set of types, under
  single-version resolution (track D6).
- Opaque and transparent export and sealed construction hold across a package boundary exactly as
  across a context boundary: a consumer can't construct a dependency's sealed types.

**Consequences.** No checker phase distinguishes a dependency unit from a local one beyond its name;
every existing rule applies to it unchanged.
