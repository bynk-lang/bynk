---
level: patch
changelog: The VS Code model views (sequence diagram, documentation, architecture map, wire contract) are live. They refresh on save (`bynk.views.refresh`: `onSave`, `onType` or `manual`), the sequence and wire-contract views follow the cursor between handlers unless pinned, open views are restored after a window reload, and each view has an editor title-bar button and an opt-in `Wire contract` handler CodeLens (`bynk.viewsCodeLens`)
---

## ADR: live-model-views
title: The extension's model views are live, client-pulled previews
summary: Model views re-pull on save, armed until the server's next committed round; updates post into the live page

**Context.** The four model views (#846, #847, #851, #855) were one-shot
snapshots: each command created a panel, filled it from one custom request,
and never updated it. The server side is pull-based by design (no
`workspace/*/refresh` for a custom method), and each request is served from
the last *committed* analysis round, which lags an edit by the server's
diagnostics debounce. Only the extension knows which panel is visible, which
editor is active and where the cursor is (#2093).

**Decision.** The extension decides when to re-pull. A shared refresh hub
turns editor events into refreshes under `bynk.views.refresh` (`onSave` by
default; `onType` debounced about 500 ms; `manual`). Because a re-pull fired at
the moment of a save can still see the previous round, a trigger fires at once
and then stays armed: the next diagnostics publish (a committed round) fires
again, and a settle timer fires a last time for a project that publishes no
diagnostics. A panel re-pulls only while visible, drops responses older than
its latest request, and skips a payload identical to the one it shows. Updates
are posted into the running page (`{type: "update", payload, view}`) rather
than replacing its HTML, so scroll and in-page state survive. The handler-scoped
views follow the cursor by default, retargeting only when the enclosing handler
(found from document symbols) changes, with an in-panel pin. Each view registers
a `WebviewPanelSerializer` and restores from the target the page persists with
`setState`, rendering the empty state when the target no longer resolves. The
editor title bar carries a button per view. The server already puts a
`Show Sequence` lens on every handler, so the opt-in lens (`bynk.viewsCodeLens`,
off) adds only `Wire contract` beside it. Model fetches go through one cache per refresh, so
views showing the same model share a request.

**Consequences.** No server, protocol or language change. A refresh costs at
most a few requests per save, deduplicated before they reach the page. A
server that one day pushes a refresh for custom requests can drive the same
hub in place of the diagnostics heuristic. The `vscode`-free scheduling core
(`refreshScheduler.ts`) is unit-tested; the panel behaviour is covered by the
extension's integration suite through internal test-seam commands.
