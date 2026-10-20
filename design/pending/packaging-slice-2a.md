---
level: minor
probes: emit_diagnostics, diagnostic_coverage
changelog: Packaging slice 2a — path dependencies work end to end. A project declares `[dependencies] money = { package = "acme/money", path = "…" }` and reaches the package's exported commons and adapters as `money.currency`; `bynk check`, `test`, `dev`, `deploy` and the editor build the same graph, a dependency's units are emitted under `out/deps/<org>/<package>/`, and only the units a project reaches are compiled. Git and registry sources, `[dev-dependencies]`, and a dependency adapter's npm `requires` are refused with the slice that brings them
---

## ADR: path-dependencies-and-cross-package-names
title: Path dependencies resolve into one compile, under canonical names source can't spell
summary: Packaging slice 2a — the package graph from path dependencies; a dependency unit's canonical name `$org_package.unit`; per-file alias resolution; exports enforced; only reached units compiled; `out/deps/<org>/<package>/`

**Context.** Slice 1 (#2207, ADRs 0542–0544) gave a project an `org/package` identity, `exports`, and
a checked-but-refused `[dependencies]`. Slice 2a (#2223) makes path dependencies work. Its decision C
keys a dependency's units by `(package, name)` so no consumer name can collide with them, while the
root project and the first-party units keep their bare keys (every existing output byte-identical).
The compiler's tables, the emitter's path sites (`commons_dir_for`, `commons_path`, the test barrels)
and three TypeScript-identifier sites (`replace('.', "_")`) all key on one dotted string, so the key
had to be a string those sites already handle.

**Decision.**

- **The graph.** `bynk_project::resolve_graph(project_root, overlay)` reads the project's
  `[dependencies]` and each path dependency's own `bynk.toml` — overlay first, then disk, as
  `read_package_config` already reads `[package]` — and returns one entry per package (single
  version): its directory, its source trees (`[paths] include`, default `src`, never `tests`), its
  `exports` and its alias map. The build (`run_checks`, `compile_project`, `check_project`), the
  editor (`analyse_project`), the CLI's source walk and project discovery (the language server's
  content sweep, the test kit) all derive the same trees from it.
- **Canonical names.** A dependency unit `u` of `org/package` is `$<org>_<package>.u` inside the
  compiler, with each `-` of the organisation written `$`. An organisation is `[a-z][a-z0-9-]*`, so it
  never contains `_` and the first `_` ends it: the encoding is injective and decodes back. `$` is never
  part of a Bynk identifier, so no project can declare a colliding unit; the name is a valid
  TypeScript identifier once `.` becomes `_`, so the identifier sites need no change; and
  `commons_dir_for` maps the encoded segment to `deps/<org>/<package>`, so the output layout and
  name/path alignment (`unit_path_matches`) follow from the one helper. Diagnostics and generated
  headers show the name as `org/package:unit` (`bynk_project::display_names`, applied in
  `ErrorSink::push_for`).
- **Folding in.** Right after parsing, `bynk_check::packages::apply_graph` tags each dependency file
  with its package (by its tree), gives it the `deps/…` source path and its unit the canonical name,
  and resolves every `uses`/`consumes` (and a suite's target) against **its own file's** package: a
  first segment naming one of that package's aliases becomes the dependency's canonical name; a
  dependency's reference to its own unit becomes its own canonical name; `bynk.*` and everything else
  is left alone. A whole-unit `consumes` of a dependency unit keeps the spelling source wrote as its
  alias, so `money.tokens.Jwt` still names the capability through the existing alias path.
- **What crosses the boundary.** Only units in the dependency's `exports` (or, absent `exports`,
  every commons and adapter) are reachable: `bynk.package.unit_not_exported` (still resolved, so it is
  reported once) and `bynk.package.unknown_unit` (the clause is dropped, so it isn't reported again
  as an unknown commons). A dependency's contexts and suites are never compiled, and only the
  dependency units reachable from the project's own units are kept — adding an unused dependency
  leaves the output byte-identical (standard-library P-5).
- **Graph rules.** `bynk.package.name_mismatch` (a directory isn't the declared package),
  `duplicate_source` (one package from two directories), `dependency_cycle`,
  `dependency_not_found`, and `bynk.package.alias_shadows_unit` (an alias equal to the first
  segment of one of the project's own units). A path dependency declared **by a dependency** is
  honoured only inside the workspace the project belongs to, compared with symlinks followed
  (`bynk.dependency.path_outside_root`); the project's own path entries are its author's and are
  honoured wherever they point. Only `[package]`, `[paths]` and `[dependencies]` are read from a
  dependency's manifest; the consuming project's `[unstable]`, `[workers]` and `[test]` govern the
  whole build.

**Consequences.**

- Worker names and `bynk.deploy.lock` are untouched: no dependency produces a Worker (ADR 0542), and
  the project's own units keep their names (ADR 0543).
- A dependency's binding module is found beside the file it was read from, and vendored under
  `deps/<org>/<package>/`.
- Editor completion and hover read file text, not the folded graph, so they don't yet offer
  `<alias>.<unit>` targets or show a dependency unit's package; that is follow-up tooling work.

## ADR: mixin-across-packages
title: A `uses` of a dependency's commons mixes in exactly as a local one does
summary: Track D8 — mixin, nominal identity, export visibility and sealed construction are unchanged across a package boundary

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

## ADR: packaging-slice-2-split
title: Packaging slice 2 ships as 2a (path) and 2b (git, store, lock)
summary: Slice 2 splits — 2a resolves path dependencies with no new crates and no network; 2b adds git sources, the content-addressed store, sha256, `bynk.lock`, `--locked`/`--offline` and `[dev-dependencies]`; range syntax moves to slice 4

**Context.** Track §9's slice 2 held an in-compiler half (graph resolution, cross-package loading,
alias naming, exports, emit layout, the editor) and a fetch half (git, the store, sha256, the lock,
offline modes, git hardening). A path dependency needs none of the fetch half: it is the user's own
working copy, never locked by hash (§6.3).

**Decision.**

- **2a** (#2223) resolves path dependencies end to end, with no new crate and no network.
- **2b** adds git sources, the content-addressed store, sha256, `bynk.lock`, `--locked` and
  `--offline`, and `[dev-dependencies]` — refused until then, with the slice named, rather than
  accepted and ignored.
- **Range syntax** is validated in slice 4, where registry ranges are first interpreted; path and git
  dependencies carry no range.
- A dependency adapter's npm `requires` is refused until slice 3 folds them; a dependency's `asset`
  is refused until assets get a per-package namespace.

**Consequences.** Each refusal names the slice that lifts it, so a manifest never builds while
ignoring what it declares (#1665).
