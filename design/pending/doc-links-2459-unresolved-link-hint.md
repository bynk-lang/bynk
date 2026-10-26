---
level: minor
probes: diagnostic_coverage
changelog: An unresolved doc link `[Name]` shows an opt-in editor hint (`bynk.doc.unresolved_link`) with a did-you-mean quick fix, set by `bynk.docLinks.reportUnresolved`
---

## ADR: unresolved-doc-link-hint
title: An unresolved doc link is an LSP-only hint with a did-you-mean fix
summary: Severity, setting and suggestions for the unresolved doc-link diagnostic

**Context.** An unresolved `[Name]` in a doc comment renders as plain text
(the render-only decision, #848), so typos and links broken by an edit go
unnoticed. Making `bynk check` fail on doc prose would be out of proportion.

**Decision.** `bynk-lsp` publishes `bynk.doc.unresolved_link` for each doc-link
candidate that `resolve_doc_link` leaves unresolved. It is LSP-only: the checker
and `bynk check` never emit it, though the code is registered (as a warning) so
`bynk explain` and the diagnostics reference know it. The severity is the
`bynk.docLinks.reportUnresolved` setting, `off` / `hint` / `warning`, default
`hint` (faint dots, not a squiggle). The client sends it as
`initializationOptions` and on `workspace/didChangeConfiguration`; a change
re-publishes every open project. A quick fix offers up to three in-scope names
within a third of the name's length in edit distance (a transposition counts as
one edit). `\[TODO]` and a bracket inside an inline code span are never
reported. Names whose owner is a first-party item are exempt, since first-party
symbols are not in the project index; the exemption is redundant once
first-party names link to the Book, and harmless after.

**Consequences.** Authors see broken doc links without leaving the editor, and
teams wanting strictness can raise the level. The hint has no effect on
compilation. Prose such as `[TODO]` is hinted unless escaped; that is the cost
of a default-on hint, bounded by the `off` setting.
