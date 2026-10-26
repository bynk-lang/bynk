---
level: minor
changelog: "`bynk mcp` serves the toolchain's read-only answers (diagnostics, symbols, documentation, architecture, wire contracts, explanations, check) to any agent over the Model Context Protocol on stdio (#2466)"
---

## ADR: bynk-mcp
title: "`bynk mcp` is a read-only MCP server in the driver, with a hand-written stdio transport"
summary: The #2463 tool set for agents outside VS Code, answered from `bynk-ide` directly, with no MCP SDK

**Context.** #2463 gave chat agents inside VS Code seven read-only tools over
the toolchain's own answers, because a model's training data on a pre-1.0
language is thin and dated. Agents outside VS Code (Claude Code, Codex, Cursor,
Claude Desktop, CI bots) have no such route, and the Model Context Protocol is
the common way they take tools. The VS Code tools are TypeScript adapters over
a running language server, so they cannot be reused by a process with no editor.

**Decision.**

- **A subcommand of the driver.** `bynk mcp [PATH]` serves MCP on stdin and
  stdout. It calls `bynk-ide` (the project analysis and the documentation,
  architecture and wire-contract models) and `bynk-syntax` directly, not
  `bynk-lsp`: an agent has no editor holding a language server open, and the
  LSP's async runtime and transport would be all cost. Each call re-analyses the
  project from disk, so an edit the agent has just made is seen.
- **The same seven tools, names and inputs as #2463.** `bynk_get_diagnostics`,
  `bynk_check`, `bynk_explain_diagnostic`, `bynk_describe_symbol`,
  `bynk_get_documentation`, `bynk_get_architecture` and
  `bynk_get_wire_contract`, with the same Markdown shapes (`path:line:column`,
  1-based, workspace-relative, capped at about 12,000 characters with a note on
  how to narrow). The renderers are a Rust port of `lmRender.ts`. Where the VS
  Code tools default to the active editor, `bynk mcp` has none: `file` picks the
  project, and omitting it is an error only when the workspace holds several.
- **Read-only.** No tool writes, emits, runs a subprocess or reaches the
  network. A `file` argument must resolve (after `..` and symlinks) to a path
  inside `PATH`, because the tools run without confirmation and an agent steered
  by text it read must not be able to point one at another project. Every tool
  carries MCP's `readOnlyHint`.
- **A hand-written transport, no SDK.** JSON-RPC 2.0 over newline-delimited
  stdio, covering `initialize`, `ping`, `tools/list` and `tools/call` (and
  ignoring notifications), about a hundred lines with unit tests. The official
  Rust SDK would add an async runtime and its dependency tree to every `bynk`
  install for a protocol surface this small, and the driver is synchronous
  throughout. It was not evaluated against `deny.toml`: the argument above
  decided it first, and the hand-written loop adds no crate. If the surface
  grows (resources, prompts, streaming, progress), revisit.
- **The VS Code tools stay as they are.** Rewriting #2483's TypeScript onto this
  server would lose the web extension (there is no `bynk` process in vscode.dev),
  tie the tools to a binary on `PATH`, and give up answers from unsaved buffers
  and the already-running language server. The duplication is the renderers
  only; the tool names and input schemas are the contract that keeps the two in
  step, and `bynk mcp`'s integration test pins them.

**Consequences.** Any MCP client can ground its Bynk in the installed toolchain
with one line of configuration. The tool names and input schemas become a
surface agents and prompts depend on, shared with the VS Code tools: adding a
field is safe, renaming one is a break in both.

Two differences from the VS Code tools are deliberate. Answers come from files
on disk, so an agent must save before asking. And `bynk_describe_symbol` uses
the index-resolved rungs of the hover ladder (a resolved symbol, a local or
parameter, a declaration by name), not the whole of it: the ladder lives in
`bynk-lsp` on LSP types, and moving it into `bynk-ide` is a larger change than
this increment. A hover-only case (a `store` operation, an agent-state
reference, a keyword) answers "nothing is known" here until that move.

A `bynk_search_book` tool will follow once #2463's `bynk book` module (the Book
embedded in the binary, searched in Rust) lands, reusing that module rather
than a second index. It is not in this increment because that module has not
merged.
