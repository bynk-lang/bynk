---
level: minor
changelog: The documentation panel makes intra-doc links (`[Name]`) clickable
---

## ADR: documentation-panel-clickable-links
title: The documentation panel's intra-doc links are resolved by the server and clickable
summary: Where a panel link goes, and where it is resolved

**Context.** The documentation panel rendered doc prose, but `[Name]` intra-doc
links in it were inert: only headings linked back to code, and webview links
are gated to `http(s)`. Hover and `documentLink` already resolve these links.

**Decision.** A link goes to the target's section in the panel when the page
documents the declaration, otherwise to the declaration's source (an in-panel
anchor, else open source), so browsing documentation stays in the
documentation. Resolution happens server-side in the `bynk/documentationModel`
request, through the existing `resolve_doc_link`: each entry (and the unit
doc) carries a list of resolved targets (`entry`, `source`, or `external`)
and its Markdown refers to them as `#bynk-doc-link-N` anchors, so the webview
only routes clicks and holds no resolution logic. The `bynk-ide` model is
unchanged (path-based, no `Url`); the targets are built in `bynk-lsp`.

**Consequences.** An unresolved link stays as authored. The `external` target
kind is modelled but not yet produced, and routes through the existing
`openExternalHref` gate. The language-model documentation tool strips the
anchors back to the authored `[Name]`.
