# Summary

<!-- The Reference's contents (web presence track, slice 5): the sidebar of
     /reference/ is generated from this file by site/scripts/generate-sidebar.mjs,
     which also checks that every page under site/src/content/docs/reference/ is
     listed here once. Paths are relative to that folder. The sections follow
     the track's order; a page's folder decides its URL, this file its place. -->

[Reference](index.md)

# Language

- [Syntax & grammar](grammar.md)
  - [Complete grammar (appendix)](grammar-appendix.md)
- [Keywords](keywords.md)
- [Glossary](glossary.md)
- [Type system](types.md)
- [Generics and bounds](generics.md)
- [Refined-type API](refined-types.md)
- [Operators & built-ins](operators.md)
- [Named constants](constants.md)
- [Agents](agents.md)
- [Agent invariants](agent-invariants.md)
- [Capabilities & providers](capabilities.md)
- [First-party `bynk` capabilities](bynk-capabilities.md)
- [Actors](actors.md)
- [HTTP](http.md)
- [Cron](cron.md)
- [Queue](queue.md)
- [WebSocket](websocket.md)
- [Inbound email](inbound-email.md)
- [Static assets](assets.md)
- [Embedded text](embeds.md)
- [Test fixtures](fixtures.md)
- [Testing](testing.md)

# Architecture

- [Emission](tooling/emission.md)
- [Wire format](wire-format.md)
- [TypeScript client](typescript-client.md)
- [HTTP client](http-client.md)
- [Registry protocol](tooling/registry-protocol.md)

# Adapters

- [Adapters](adapters.md)

# Tooling

- [Tooling](tooling/index.md)
  - [CLI (`bynk`)](tooling/bynk-cli.md)
  - [`bynk.toml` manifest](tooling/manifest.md)
  - [Editor & tooling](tooling/editor-and-tooling/index.md)
    - [Check your environment with `bynk doctor`](tooling/editor-and-tooling/doctor.md)
    - [Format your code with `bynk-fmt`](tooling/editor-and-tooling/format.md)
    - [Set up editor support](tooling/editor-and-tooling/editor-support.md)
    - [Set up Neovim, Helix or Zed](tooling/editor-and-tooling/other-editors.md)
    - [Debug in VS Code](tooling/editor-and-tooling/debugging.md)
  - [`bynk-fmt`](tooling/bynk-fmt.md)
  - [`bynk-lsp`](tooling/bynk-lsp.md)
  - [`bynk mcp`](tooling/bynk-mcp.md)
  - [`bynk client`](tooling/bynk-client.md)
  - [`bynk openapi`](tooling/bynk-openapi.md)
  - [`tree-sitter-bynk`](tooling/tree-sitter-bynk.md)
  - [`vscode-bynk`](tooling/vscode-bynk.md)

# Diagnostics

- [Diagnostic index](diagnostics.md)

# Specification

- [The Bynk Language Specification](spec/index.md)
  - [§1 Scope & conformance](spec/scope.md)
  - [§2 Notation & conventions](spec/conventions.md)
  - [§3 Lexical grammar](spec/lexical-grammar.md)
  - [§4 Syntactic grammar](spec/syntactic-grammar.md)
  - [§5 Static semantics](spec/static-semantics.md)
  - [§6 The type system](spec/type-system.md)
  - [§7 Meaning by translation](spec/emission.md)
    - [§7.4 The runtime library](spec/runtime-library.md)
  - [§8 Compilation model](spec/compilation-model.md)
  - [§9 Diagnostics](spec/diagnostics.md)
  - [§10 Conformance & test corpus](spec/conformance.md)
  - [§11 Complete grammar](spec/grammar-appendix.md)
  - [Appendix A — Planned features](spec/appendix-planned.md)
  - [Appendix B — Version history](spec/appendix-version-history.md)

# Releases

- [Releases](releases/index.md)
  - {release pages}
- [Upgrading](upgrading.md)
