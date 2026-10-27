---
level: minor
changelog: Completion inside a doc comment offers the names a `[link]` would resolve, in resolution order
---

## ADR: doc-link-completion
title: Completion after `[` in a doc comment offers the names a doc link resolves
summary: `[` triggers completion only inside doc ranges; candidates come from the resolver's own scope order

**Context.** Doc links (`[Name]`, `[Owner.member]`) resolve through `doc_scope` and `resolve_doc_link`, but typing `[` in a doc comment offered nothing: completion had no doc context and `[` was not a trigger character.

**Decision.** `[` is added to the completion trigger characters, and the handler gates it strictly: only when the cursor is inside a `---` or `--|` doc range, outside fenced code, after an unclosed `[` followed by identifier characters, does it offer doc-link items; a `[` trigger anywhere else returns nothing (a `[` there starts a list literal). The candidates come from one function, `doc_link_suggestions`, which walks the same `doc_scope` list as `resolve_doc_link` and keeps only names `resolve_doc_link` itself resolves, so the order (today the unit, then `uses`, then `consumes`) and any later change to it flow through without a second copy of the scope rules. Within a unit, names sort alphabetically. After `Owner.` it offers that owner's members. Items insert `Name]`, omitting the `]` when one already follows. A name that is ambiguous in its first matching unit would not link, so it is not offered; offering a qualified form (`unit.Name`) for it waits on qualified doc-link names existing. The web (WebAssembly) server keeps its trigger characters unchanged: it has no project index.

**Consequences.** Authors get the same candidates, in the same order, that a link would resolve to, and the top suggestion is the one the link resolves to. Own-part candidates and qualified forms arrive with the scope change that introduces them, with no completion change.
