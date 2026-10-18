---
level: minor
changelog: The language server offers an organise `uses`/`consumes` source action (`source.organizeImports`), runnable on save through `editor.codeActionsOnSave`
---

## ADR: lsp-refactorings-and-source-actions
title: LSP refactorings and source actions re-check their edit before they are offered
summary: Refactor and source actions ship in slices, re-check their edit before offering it, and are computed only when their kind is asked for

**Context.** The language server's code actions were diagnostic quick fixes,
capability-header fixes, and extract-variable/extract-function. Issue #2100
groups the rest of a typed-language refactoring set into slices: organise
`uses`/`consumes`, fix all, fill match arms, inline variable, generate a
provider stub, and extend extract-function to carry capabilities. Each needs
resolved names and types that only the checker has. A refactor that breaks the
program is worse than none.

**Decision.** The slices ship in the order organise `uses`/`consumes`, fix all,
fill match arms, inline variable, generate provider stub, then extract function
(#2100's DECISION A). Organise and fix-all come first because they make
`editor.codeActionsOnSave` useful with the least design risk. Extract-function
comes last because it has the most semantic edge cases.

Every new action computes its edit, re-analyses the project with that edit
applied (every other file pinned to the round's snapshot), and is offered only
if the re-analysis has no diagnostic the round didn't already have (DECISION B).
Organise tries its full edit first. If a drop fails the re-check, it falls back
to the sort-and-deduplicate-only edit.

Fix-all applies only suggestions marked machine-applicable. A suggestion that
needs a human choice stays an individual quick fix (DECISION C).

Source actions (`source.*`) are computed only when the request's
`context.only` names their kind or a parent of it. That covers the Source
Action menu, the organise-imports command and `editor.codeActionsOnSave`. They
are never computed for the unfiltered request a client sends on every cursor
move, so the re-check runs only when someone asks for the action. This takes
the place of the proposal's `codeAction/resolve` mitigation.

Organise reads "unused" from the binding index over every file of the unit,
including the suites on it. A reference inside a `uses`/`consumes` clause is
not a use. A whole-unit clause on a target outside the project is never
dropped. A braced selection from the first-party `bynk` surface is pruned
against first-party capability references. A clause moves with its comments
and is never dropped while it carries one. The edit keeps a `bynk-fmt`-canonical
file canonical.

**Consequences.** The new actions never offer an edit that introduces a
diagnostic, but each one costs a whole-project analysis when it is requested.
Clients must ask for source actions by kind. A client that only ever sends
unfiltered requests won't see them. Organise can't detect an unused whole-unit
clause on a first-party target such as `uses bynk.list`, so it keeps it. No
editor setting turns any of these on by default. Users opt in through
`editor.codeActionsOnSave`.
