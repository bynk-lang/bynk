---
title: Tooling
---
Documentation for the tools around the Bynk compiler. If you just want editor
support, the how-to [Set up editor support](/docs/editor-and-tooling/editor-support/)
is the quickest path; the pages here are the reference for each tool.

| Tool | Binary / package | Purpose |
|---|---|---|
| [`bynk-fmt`](/docs/tooling/bynk-fmt/) | `bynkc fmt` | The canonical source formatter. |
| [`bynk-lsp`](/docs/tooling/bynk-lsp/) | `bynkc-lsp` | The language server (diagnostics, hover, go-to-definition, formatting). |
| [`bynk mcp`](/docs/tooling/bynk-mcp/) | `bynk mcp` | A Model Context Protocol server, so AI agents outside VS Code can ask the toolchain about Bynk. |
| [`bynk openapi`](/docs/tooling/bynk-openapi/) | `bynk openapi` | An OpenAPI 3.1 document for a project's `from http` services, for client codegen, docs and CI. |
| [`tree-sitter-bynk`](/docs/tooling/tree-sitter-bynk/) | — | The grammar used for highlighting and structural tooling. |
| [`vscode-bynk`](/docs/tooling/vscode-bynk/) | VS Code extension | Editor integration built on `bynkc-lsp`. |

All are built from the same repository as the compiler, so they track the
language version for version (currently v0.447).
