---
level: minor
changelog: `bynk.toml` gets editor support from a JSON Schema at `https://bynk-lang.org/schemas/bynk.toml.json`, generated from the manifest's Rust types by `cargo xtask manifest-schema` and associated with `bynk.toml` by the VS Code extension, so a schema-aware TOML extension completes, documents and validates every table and key
---

## ADR: manifest-json-schema
title: `bynk.toml` editing support is a JSON Schema generated from the manifest's types
summary: Schema first, through a TOML language server; the LSP opening `bynk.toml` is deferred

**Context.** `bynk.toml` is every project's root configuration, but editing it got no help: no completion, no hover, and no validation until a `bynkc check`. The manifest's shape lives in Rust, split across its owners: `bynk-project` checks the table set and the keys of `[project]`, `[paths]`, `[workers]` and `[unstable]` against `MANIFEST_TABLES`, reading them by hand from a `toml::Table`; `bynk-fmt`'s `RawFmt` reads `[fmt]`; `bynk-lsp`'s `LspSection` reads `[lsp]`. A description kept anywhere else would drift (#2096, DECISION A).

**Decision.** Editor support for `bynk.toml` is a JSON Schema (draft 07, which taplo-based TOML language servers support best), not server-side handling. It is generated, never hand-edited: `bynk-fmt` derives `[fmt]` with `schemars` from `RawFmt` itself, behind a `schema` feature; `bynk-project`'s `manifest_schema` module, behind the same feature, holds schema types for the hand-read tables and for `[lsp]` (whose reader lives in `bynk-lsp`, which the generator can't depend on). `cargo xtask manifest-schema` assembles them in `MANIFEST_TABLES` order and panics on a table with no schema. Field doc comments are the hover text, and each table links its section of the manifest reference. The VS Code extension associates the schema with `bynk.toml` through `contributes.tomlValidation`, and recommends a TOML extension that honours it (Even Better TOML) without depending on one: a one-time suggestion, remembered once dismissed. Other editors use a `#:schema` directive. The language server opening `bynk.toml` as a document, for checks that need the project, is deferred.

**Consequences.** Completion, hover and validation reach VS Code, Zed, Helix and anything else that speaks taplo, with no server code. The schema only checks shape: whether an `[unstable] allow` entry names a real item is still `check`'s and the server's to say. The editing experience in VS Code depends on a third-party extension, which is recommended rather than required. The optional `schemars` dependency is off in every shipped binary's own build.

## ADR: manifest-schema-strictness
title: The `bynk.toml` schema refuses unknown tables and keys, as the CLIs do
summary: DECISION B adopted as far as JSON Schema allows: no severity, so it follows the toolchain

**Context.** #2096's DECISION B recommended warning on unknown keys rather than rejecting them, so that turning silent ignores into errors wouldn't break manifests on upgrade. Since #1665, though, `check_manifest_str` already refuses an unknown table or key at the CLI, and `bynk-fmt` refuses an unknown `[fmt]` key. JSON Schema has no severity: a schema either allows a key or flags it, and the TOML extension chooses how to show it.

**Decision.** Every table, and the root, sets `additionalProperties: false`, so the schema flags exactly the tables and keys a build refuses, and no table or key that a build accepts. Where a reader accepts more shapes than its documentation, the schema accepts them too: `[paths] include` and `exclude` take a single string as well as a list. Value types are another matter. The schema checks them as documented, which is stricter than some readers that are lenient by omission: `[paths]` drops a non-string list item silently, `[project] name` and `version` are never type-checked (nothing reads them), and the server reads an unrecognised `[lsp] diagnostics_mode` as `"live"`. The schema flags all of these, because each is a manifest that doesn't mean what it says, not a shape anyone relies on. Elsewhere types and enum values match the readers' refusals exactly (`[fmt] indent` is `"tab"` or `"spaces"`, `max_line_width` is at least 1, `compatibility_flags` and `allow` are lists of strings).

**Consequences.** The editor flags a manifest as it is typed. An unknown table or key is flagged for the same reason a build refuses it. A badly typed value can be flagged where the build would quietly ignore it, so the schema is the stricter of the two on values only. There is no warning tier for any of this; adding one would need the deferred server-side handling.

## ADR: manifest-schema-location
title: The `bynk.toml` schema is committed under the docs site and checked in CI
summary: One copy at `site/public/schemas/bynk.toml.json`, served at a stable URL

**Context.** #2096's DECISION C: keep the schema and the Rust types in lockstep, committed in the repo and checked against the generated output.

**Decision.** The schema has one committed copy, at `site/public/schemas/bynk.toml.json`, which the docs site serves at `https://bynk-lang.org/schemas/bynk.toml.json`, the schema's `$id`. `xtask`'s `manifest_schema` test fails when it differs from freshly generated output (`BYNK_BLESS=1` or `cargo xtask manifest-schema` rewrites it), when a table's keys disagree with `MANIFEST_TABLES`, when a stated `[fmt]` default isn't `FormatOptions::default()`, when the manifest reference's example sets a key the schema lacks (or omits one it has), or when the extension's `tomlValidation` stops naming the URL. `bynk-lsp`'s own test holds `LspSection` to the schema's `[lsp]` keys, defaults and modes. CI's `rust` filter covers the schema file and the reference page, and the `drift` job's `cargo test -p xtask` covers an extension-only change.

**Consequences.** The URL is stable and always serves the latest schema, deployed with the site from `main`; the extension's association resolves once the site deploys this change. Nothing is attached to the GitHub Release yet: the site copy is the published one.
