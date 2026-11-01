---
level: minor
probes: emit_diagnostics, diagnostic_coverage
changelog: "A `private` modifier on a commons or context `fn` (including a method), `type`, `messages`, `asset` or `embed` keeps it within its own unit: visible across the unit's files and to its suites, never through `uses`. A reference from another unit is `bynk.visibility.private_item`, a public signature naming a private type is `bynk.visibility.private_in_public`, and `private` on any other declaration is `bynk.visibility.private_not_allowed`. `private` is a contextual keyword, so `@cache(scope: private)` is unchanged ([#2618](https://github.com/bynk-lang/bynk/issues/2618))"
---

## ADR: private-declarations
title: Private declarations, a unit-local visibility for commons and context items
summary: `private` hides a `fn`, `type`, `messages`, `asset` or `embed` from importers; importers do not see private names at all, a unit suite sees its target's, a public signature may not name a private type, and emission is unchanged

**Context.** Every commons declaration was public. A library could not keep a
helper to itself, so each helper became API: renaming it could break any unit
that `uses` the commons, and the standard library documented helpers as
"exported because a commons has no private functions". Proposal #2618 asked
for an opt-in, unit-local visibility.

**Decision.** The decisions the proposal accepted:

- **A. Default stays public; `private` is opt-in.** Nothing that compiled
  before changes meaning.
- **B. Granularity is unit-private only.** A private declaration is visible
  across all of its unit's files. There is no package-level visibility.
- **C. A private type may not appear in a public signature**
  (`bynk.visibility.private_in_public`): a public function's or method's
  parameter and return types, a public type's record fields and variant
  payloads. This is about `uses` importers; the cross-context "transparent but
  unnamed" rule (spec §6.5) is unchanged.
- **D. `private` applies to `fn`, `type`, `messages`, `asset` and `embed`.**
  On a service, agent, capability, provider, actor or event it is
  `bynk.visibility.private_not_allowed`: their reach is a context's
  `exports`/`consumes` question.
- **E. Methods.** `private fn Type.helper(self)` hides the method while the
  type stays public.

The choices the proposal left open:

- **F. Importers do not see private declarations at all.** A used unit's
  private declarations are left out of the importer's composed tables (types,
  functions, methods, assets, embeds), out of the `uses` name-conflict check
  (spec §5.1) and out of the shadowed-import computation. So an importer's own
  declaration of the same name is no conflict, and two used commons may each
  have a private helper of one name. The alternative, importing them and
  rejecting each use, would make a private name a conflict risk for every
  importer, defeating the point of hiding it.
  - The `bynk.visibility.private_item` diagnostic comes from the reference
    first failing as the unknown name it now is (`unknown_function`,
    `unknown_type`, `unknown_name`, `unknown_static_member`,
    `method_not_found`), which is then rewritten when its subject (the same
    subject the broken-declaration echo filter reads) names a private
    declaration of a used commons. A name the importer declares itself is
    never rewritten.
- **G. A private type's methods are hidden with it,** marked `private` or not,
  and are exempt from decision C: an importer can reach neither the type nor
  a value of it.
- **H. A context's `exports` list may not name a private type**
  (`bynk.visibility.private_in_public`). Decision C applies to context items
  as well as commons items, for one rule everywhere, even though a context
  cannot be `uses`d.
- **I. The suite exception is the target's own table.** A suite below
  `system` checks against its target's privileged view, which already starts
  from the target's full table, so the target's private declarations are in
  scope. A commons the target, or the suite, `uses` keeps its private
  declarations: they are filtered from the view, and a reference to one is
  `bynk.visibility.private_item` as in any unit. A `system` harness filters
  the suite's own `uses` the same way and leaves the participants (the
  systems under test) as they were.
- **J. Emission is unchanged.** A private declaration is still emitted with
  `export`. Omitting it is not trivially safe: a unit suite's scaffold and
  the hidden-type closure (#1807) import from the declaring module, and the
  context rebrand forwards attached methods. Every existing golden is
  byte-identical. A generated test module, unit or `system` tier, does not
  bring a used commons' private functions, types, assets or embeds into a
  case's scope, matching the checker's suite view; otherwise one would
  redeclare a public name of the same spelling from another used commons.
- **K. `private` is contextual, not reserved.** It is the modifier only as an
  identifier immediately followed by `fn`, `type`, `messages`, `asset` or
  `embed` (or, for a precise rejection, `service`, `agent`, `capability`,
  `provides`, `actor` or `event`) at item position. `@cache(scope: private)`
  and a binding named `private` keep their meaning. `private` in an adapter
  is `bynk.visibility.private_not_allowed`. The tree-sitter grammar admits the
  modifier on the five declaration kinds only.
- **L. A private asset is still served.** `private` hides the asset's name
  from importers, not its file from the platform.
- **M. A private `messages` bundle** is accepted (decision D) and, like
  every private declaration, never reaches a unit through `uses`: a context
  that `uses` the commons does not negotiate its locale against that bundle.
  Since a bundle only ever serves a context that uses its commons, a private
  bundle has no effect today; it is admitted so that the modifier means the
  same on every declaration it applies to.
- **N. Constants.** Named constants (#2617) are landing separately. Whichever
  of the two changes merges second extends `private` to `const`.
- **O. Standard library.** No standard-library helper is marked `private`:
  every candidate (`bynk.url`'s `isScheme`, `isHost`, `isPath`, `parsePort`,
  `decodePair` and `decodeQuery`, and `bynk.locale`'s `renderArg`) is
  documented in the Book or imported by generated code, so hiding it would
  break a caller.
- **P. Editor support.** Hover renders the modifier and explains it; completion
  leaves out another unit's private functions, assets, embeds and types; the
  semantic-token legend appends a standard `keyword` type, given to `private`
  in modifier position, since the word is contextual.

**Consequences.** A library can now keep its helpers private and change them
freely. An importer that relied on a helper the library later marks `private`
gets `bynk.visibility.private_item`, which names the declaration and its unit.
Generated TypeScript still exports private declarations, so a hand-written
TypeScript caller is not prevented from reaching one; the guarantee is at the
Bynk level.
