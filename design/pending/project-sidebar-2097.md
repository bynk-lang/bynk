---
level: patch
changelog: The VS Code extension has a Bynk project sidebar. An activity-bar view lists the project's contexts, commons, adapters and tests, each expanding to its `consumes` edges in both directions and its declarations, every node click-to-source, with inline sequence-diagram and wire-contract actions on handlers. It is built from `bynk/architectureModel` and refreshes on the `bynk.views.refresh` trigger
---

## ADR: project-sidebar
title: The extension's project sidebar is a second projection of the architecture model
summary: An activity-bar tree from `bynk/architectureModel` plus lazy document symbols, sharing the live views' refresh and cache

**Context.** The architecture map (#851) shows a project's shape as a diagram,
but there was no persistent, navigable view of it: finding which contexts
consume a unit, or a unit's handlers, meant opening the map or searching
symbols. The extension contributed no activity-bar container or tree view
(#2097).

**Decision.** A `bynk` activity-bar container (shown in a Bynk project) holds a
**Project** tree. Its structure is a second projection of the model the map
renders: units and `consumes` edges from `bynk/architectureModel`, plus a cheap
header scan of the project's `.bynk` files for the commons and test units the
model does not cover. Each unit shows both edge directions as **Consumes** /
**Consumed by** groups. Declarations are fetched lazily from
`textDocument/documentSymbol` when a unit expands, which keeps the initial
request small; a handler is a `Method` child of a service or agent `Class`
symbol, and carries inline actions that open the sequence-diagram and
wire-contract views. The tree refreshes on the live views' trigger
(`bynk.views.refresh`) while visible, and reads the model through their shared
cache, keyed by project root, so the map and the tree share one request per
refresh.

**Consequences.** No server change. The tree is project-scoped and edge-aware,
so it complements rather than duplicates the per-file Outline. If lazy symbol
requests prove too chatty on large projects, a later increment can add
declaration lists to the architecture model. A future dev-server view (#2094)
has a container to live in.
