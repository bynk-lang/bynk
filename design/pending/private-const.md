---
level: minor
changelog: "`private` now applies to `const`: a `private const` is visible across its own unit's files and to its suites, never through `uses`. A reference from another unit is `bynk.visibility.private_item` (\"a private constant\"), and a public constant whose type annotation names a private type of its unit is `bynk.visibility.private_in_public`"
---

## ADR: private-const
title: `private` extends to named constants
summary: `private const` hides a constant from importers exactly as `private` hides a function, and a public constant may not be typed with a private type

**Context.** Named constants (ADR 0678, #2617) and private declarations
(ADR 0683, #2618) were built in parallel. Neither could cover the other, so
ADR 0683 named the gap as its decision N: whichever of
the two merged second would extend `private` to `const`. Constants merged
first. Until now a constant was the one kind of commons value declaration a
library could not keep to itself: a tuning value such as a retry step became
API as soon as it was declared.

**Decision.** `private` applies to `const` in a commons or context, with the
same meaning it has on `fn`, `type`, `asset` and `embed`. Each rule of ADR
0683 carries over unchanged:

- **Visibility (its decisions B and F).** A private constant is visible across
  all of its unit's files. An importer does not see it at all: it is left out
  of the importer's composed constants, the `uses` name-conflict check, the
  shadowed-import computation and the types reached through used constants'
  annotations. So an importer may declare, or import from another commons, a
  public constant of the same name without a conflict. A reference that
  resolves to nothing but a used commons' private constant first fails as
  `bynk.resolve.unknown_name` and is rewritten to
  `bynk.visibility.private_item`, with the kind "constant": "`Step` is a
  private constant of `limits`".
- **Private types in public positions (its decision C).** A public constant's
  type annotation is part of what an importer reads (ADR 0678, decision B, is
  why the annotation is required), so it is checked like a public function's
  signature: naming a private type of the constant's own unit is
  `bynk.visibility.private_in_public` ("public constant `Default` names
  `Window`, a private type of `limits`"). A private constant may be typed with
  a private type.
- **Suites (its decision I).** A suite below `system` sees its target's
  private constants. It does not see a used commons' private constants, in the
  checker's view or in the generated test module, whose scaffold no longer
  destructures them; otherwise one would redeclare a public constant of the
  same name from another used commons. A `system` suite sees no constants at
  all (ADR 0678), so it needs no filter.
- **Emission (its decision J).** Unchanged: a private constant is still an
  exported TypeScript `const`.
- **Syntax (its decision K).** `private` stays contextual. It is the modifier
  only immediately before `const` (and the other kinds) at item position. In
  an adapter it is `bynk.visibility.private_not_allowed`, beside the existing
  `bynk.resolve.const_outside_context_or_commons` placement error for the
  constant itself. The tree-sitter grammar admits
  `visibility_modifier` before `const_decl`, and the TextMate grammar
  highlights `private` before `const`.
- **Editor support (its decision P).** Completion leaves out another unit's
  private constants; hover renders the modifier on a constant and explains it
  before `const`.

**Level.** `minor`: it widens the language. Source that was a parse error
(`private const X: Int = 1`, where `private` was read as a stray identifier)
now compiles, and a public constant typed with a
private type is a new error. No program that compiled before changes meaning:
no constant was private before, so nothing it imports is hidden by this.

**Consequences.** A library can keep its constants private and change them
freely. The set of declarations `private` applies to is now `fn`, `type`,
`messages`, `asset`, `embed` and `const`; the reasons the rest stay out (their
reach is a context's `exports` and `consumes` question) are unchanged.
