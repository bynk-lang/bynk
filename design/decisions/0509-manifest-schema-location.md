# 0509 — The `bynk.toml` schema is committed under the docs site and checked in CI

- **Status:** Accepted (v0.356)

**Context.** #2096's DECISION C: keep the schema and the Rust types in lockstep, committed in the repo and checked against the generated output.

**Decision.** The schema has one committed copy, at `site/public/schemas/bynk.toml.json`, which the docs site serves at `https://bynk-lang.org/schemas/bynk.toml.json`, the schema's `$id`. `xtask`'s `manifest_schema` test fails when it differs from freshly generated output (`BYNK_BLESS=1` or `cargo xtask manifest-schema` rewrites it), when a table's keys disagree with `MANIFEST_TABLES`, when a stated `[fmt]` default isn't `FormatOptions::default()`, when the manifest reference's example sets a key the schema lacks (or omits one it has), or when the extension's `tomlValidation` stops naming the URL. `bynk-lsp`'s own test holds `LspSection` to the schema's `[lsp]` keys, defaults and modes. CI's `rust` filter covers the schema file and the reference page, and the `drift` job's `cargo test -p xtask` covers an extension-only change.

**Consequences.** The URL is stable and always serves the latest schema, deployed with the site from `main`; the extension's association resolves once the site deploys this change. Nothing is attached to the GitHub Release yet: the site copy is the published one.
