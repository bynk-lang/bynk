---
level: patch
changelog: Internal restructuring: the `[env.*]` block of a named-environment deploy is rendered by the emitter from one table of `wrangler.toml` keys; `wrangler.toml` is unchanged
probes: wrangler_key_literals, env_protocol_literals, ts_writes
raises: ts_writes — the block renderer's three `format!` calls moved from the CLI (`bynk/src/deploy/config.rs`) into `bynk-emit` (1C.2); they write TOML, not TypeScript, and are not new
---
refactor: `bynk-emit` renders the `[env.*]` block, from one table of wrangler keys (1C.2)

Closes #4039. Part of #3676.

No output changes. `bynk-project` gains `WranglerKey`, every top-level key the emitter writes, with what a named environment does with each as one exhaustive `match`, and the environment name functions. `bynk-emit`'s `wrangler::environment::render` writes the `[env.<name>]` block from the generated file the deploy has materialised, through the same `toml` layout as before, so every byte is the same. The emitter's `TomlBlock` headers take a `WranglerKey`, so a table it writes cannot be left out of the block by omission (#3577, #3586), and the patchers spell keys through the vocabulary too. The CLI's `synthesise_environment_block` and its list of 22 tables are deleted. The block and each Worker's final configuration are pinned by fixtures that were added before any code moved. New gated growth probe `wrangler_key_literals` (91 to 0); `env_protocol_literals` 4 to 3.
