---
level: minor
changelog: Doc links resolve context-first — a declaration's own fields, operations, handlers, methods and parameters, then the unit scope, then unit-qualified names like `[window.decide]`; suite doc comments link too
---

## ADR: context-aware-doc-links
title: Doc links resolve a declaration's own parts first, then the unit scope, then unit-qualified names
summary: Own parts shadow unit names; parameters link to their binding site; dotted names try Owner.member before unit.Name

**Context.** An intra-doc link `[x]` resolved against the declaring unit, its `uses` and its `consumes` only, and a bare name matched top-level kinds only. Inside a record's doc `[remaining]` could not find the record's own field, nor could a fn's doc find its parameter, and a suite file's doc comments got no links at all.

**Decision.** A candidate in a doc comment attached to declaration `D` resolves in order: (1) `D`'s own parts — its fields, operations, handlers and methods from the index, and its parameters from the per-file locals table; (2) the existing unit scope; (3) a unit-qualified name, the last segment read as a top-level declaration of the unit named by the rest. An own part shadows a unit-level name of the same spelling (lexical scoping), rather than making the clash ambiguous. A parameter links to its binding site. A dotted name tries `Owner.member` first and `unit.Name` second, so every link that resolved before still does; the scanner accepts any number of identifier segments. A suite's doc comments resolve against its target unit's scope. The attached declaration is the first identifier after the doc comment, matched to an index definition site; `resolve_doc_link_in` takes it as an optional context and `resolve_doc_link` delegates without one.

**Consequences.** A name that is both an own part and a unit-level name now links to the own part. Prose such as `[e.g.]` is still not a candidate, as every segment must be an identifier. Tooling only: no grammar, checker, emitter or runtime change.
