---
level: patch
changelog: Internal tooling: a module-size ratchet for the back-half crates
---
refactor: the module-size ratchet (2.0)

Closes #4085. Part of #3676.

Slice 2.0 of Phase 2. `xtask` only: no emitter, driver or runtime code changes,
so emitted output, goldens, source maps and pinned clients are untouched.

- `xtask/module-line-ceilings.txt`, `xtask/src/module_lines.rs` and
  `xtask/tests/module_lines_ratchet.rs` gate the six back-half crates (10
  ceilings); `cargo xtask module-lines [--apply]` prints and rewrites them.
- `test_mod_ranges` recognises `#[cfg(test)] pub(crate) mod tests {`, so
  `module_lines` and `test_density` stop counting eight test modules as
  production. Both are trend rows (no gated probe moves, so there is no
  `probes:` line) and both move on this definition.
- `AST_IMPORTER_ORIGINS` is the empty origin table for `ast_importers`; each
  split adds its new `bynk-emit` files to it.
