# 0463 — The first-party sources live in a data-only `bynk-std` crate, with the platform layer in its own subtree

- **Status:** Accepted (v0.330.9)

**Context.** [[0086]] made the first-party sources real files embedded by `include_str!`, under a
"package-shaped" tree, as "the cheap hedge that keeps a future publish a lift-and-shift". The tree
ended up inside the type checker: `bynk-check/src/firstparty/`, next to a 266-line
`bynk-check/src/firstparty.rs` that was almost entirely data. It held unit names, the `*_SRC` and
binding consts, `Platform`, `platform_of`, `provider_takes_env` and `FIRSTPARTY_SOURCES`. Library
content and checker logic therefore shared a crate, and there was no home a later unbundling could
lift out whole. The standard-library track (spine #1936, `design/archive/retired-tracks.md` D1
and D9) settled both questions, and S0 (#1940) carries them out.

**Decision.**

- **D1 — A data-only leaf crate owns the first-party sources.** `bynk-std` holds every first-party
  `.bynk` source, every TS binding, and the metadata the toolchain reads about them. It has no
  dependencies, in-workspace or otherwise. It holds no logic: parsing and injecting the sources
  (`firstparty_parsed`, the injection blocks in `project_model.rs`) stays in `bynk-check`, which
  *reads* the library rather than containing it. This is a downward extraction in [[0099]]'s sense.
  `bynk-check`'s `firstparty` module re-exports `bynk-std` unchanged, a downward re-export rather
  than an upward façade, so every `bynk_check::firstparty::*` consumer resolves as before.
- **D2 — The platform layer sits in its own subtree.** `lib/core/` holds the portable layers (the
  `bynk` surface, its per-platform bindings, the commons). `lib/cloudflare/` holds the
  `bynk.cloudflare` adapter and its binding. Nothing in `lib/core/` depends on `lib/cloudflare/`.
  The commitment is the track's D9 and no more: a later unbundling of the platform layer renames no
  unit and changes no user source. Whether to unbundle belongs to the packaging track (#843).
- **D3 — Published, in lockstep.** `bynk-std` is versioned with the workspace and published before
  `bynk-check` in `release.yml`'s order, because `bynk-check` cannot publish without it. Its first
  publish goes through `release-bootstrap.yml`, since trusted publishing attaches only to a crate
  that already exists.
- **D4 — Checks that name the sources' location follow them, and fail when blind.** ADR 0310's
  gated `emit_abi_shapes` probe reads both binding directories and treats a missing or `.ts`-free
  directory as a failure, not as zero leaks. Pointed at one directory, non-recursively, it would
  have silently stopped checking `cloudflare.binding.ts`. The tree-sitter conformance corpus and
  the #901 drift guard now read `bynk-std`.

**Consequences.** Emitted output is byte-identical: no fixture's expected output changes. The
crate-layering diagram gains `bynk-std` as a second root, since it depends on nothing, with
`bynk-check` listing `+ std`. The `emit_diagnostics` probe's *naive* `bynk-check` count drops by
three, because three unit-name literals (`"bynk.cloudflare"` and others) no longer live in
`bynk-check/src`. Its gated *true* count is unchanged. The registry that replaces the hand-coded
injection blocks (track slice S1) builds on this crate.
