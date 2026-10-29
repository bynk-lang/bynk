# 0650 — The hover ladder lives in `bynk-ide`, shared by the language server and `bynk mcp`

- **Status:** Accepted (v0.436.3)

**Context.** `bynk_describe_symbol` re-implemented three rungs of hover (a
resolved symbol, a local, a declaration by name) because the full ladder lived in
`bynk-lsp`, which the driver cannot call. An agent outside VS Code got a thinner
answer than the editor's, and the two could drift.

**Decision.** `hover_content` and what it reads move to `bynk-ide`: the binding
index queries (`index_queries`), the doc-link URL rewrite (`doc_link_urls`), the
first-party doc table (`first_party_docs`) and the capability-path hover section
(`capability_path`). The ladder takes the cursor's file as a `Path`, not an LSP
`Url`, and builds `file://` links with the `url` crate (the one `lsp-types`
re-exports), so the language server's output is unchanged. `bynk-lsp` re-exports
the old paths and keeps only what builds `lsp_types` values (semantic tokens, the
code action). `bynk mcp` calls the ladder; it keeps its own definition-site
lookup. The hand-written stdio transport is unchanged; no MCP SDK is adopted.

**Consequences.** `bynk_describe_symbol` gains every rung the editor has — a
handler header's wire contract, doc-link previews, unit docs, keywords, agent
state, first-party symbols. The ladder's rung order stays pinned by the existing
`bynk-lsp` hover tests, which now exercise it across the crate boundary.
`bynk-ide` (and so `bynk-wasm`) gains a `url` dependency.
