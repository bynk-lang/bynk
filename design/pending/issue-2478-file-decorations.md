---
level: minor
changelog: "`vscode-bynk` decorates the Explorer: a context badge on each file, and a red tint on files whose tests failed"
---

## ADR: explorer-context-badge
title: The Explorer badges a file with a two-letter context abbreviation
summary: Abbreviated badge, full name in the tooltip, off by default for single-context projects

**Context.** In a multi-context project the file Explorer does not say which context (and so which Worker) a `.bynk` file belongs to, or which files hold failing tests. A file decoration provider can carry both, but a badge holds two characters and clutters a project where every file would carry the same one.

**Decision.** The badge is a two-letter abbreviation of the owning context (the initials of its first two words, else its first two letters), with the full name in the tooltip. `bynk.fileDecorations.contextBadge` defaults to `auto`, which badges only a project with more than one context; `always` and `never` override it. A test file carries the badge of the context it tests; commons and adapters carry none. A failing test tints its file red (`bynk.fileDecorations.failedTests`) until a run passes it. The owning context comes from the project sidebar's header scan, not a language-server request, so badges do not wait for the server. The provider is desktop-only, since the tint reads the Test Explorer, which the web entry does not have. Tooling delta: hover, completion, semantic tokens and signature help are unchanged.

**Consequences.** Two contexts can share an abbreviation (`billing`, `bills`); the tooltip disambiguates. The marker for contract breaks is left for the contract diagnostics (#2467), which it would read.
