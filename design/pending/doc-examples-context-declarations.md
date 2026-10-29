---
level: patch
changelog: '`bynk test` checks declaration examples in a context''s doc comments'
---

## ADR: doc-examples-context-declarations
title: Declaration doc examples in a context are checked in a copy of it
summary: A context's declaration fence is checked in a copy where its items replace the documented declarations

**Context.** [ADR 0625](0625-doc-examples-checked.md) checked a declaration
fence in a commons as a sibling that `uses` it, but skipped one in a context: a
`uses` names a commons, and appending the fence to a copy of the context collides
with the declaration the doc usually shows (`type Order = …` on `type Order`).
Those examples could still rot unnoticed.

**Decision.** A declaration fence in a context is checked in a synthetic copy of
the context with the fence's items added. A fence item that redeclares an existing
name replaces that declaration in the copy, so the example is checked against
everything else in the context. Only diagnostics inside the fence are reported;
an error the replacement causes elsewhere in the copy is not. Two fences cannot
share one copy, so each is checked in its own project check (a layer holds the
n-th such fence of every context). Projects with no such fence run the same
checks as before.

**Consequences.** A broken declaration example in a context now fails `bynk test`
at the fence; `bynk fragment` and `bynk ignore` remain the escape hatch. A project
pays one extra project check per declaration fence in a context, and only then. No
grammar, checker, emitter or runtime change.
