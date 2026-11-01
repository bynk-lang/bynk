# 0683 — Private declarations, a unit-local visibility for commons and context items

- **Status:** Accepted (v0.467)

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
  - This leaves a deliberate asymmetry: a `system` suite can name a
    participant context's private type, while a context that `consumes`
    that participant cannot (it sees only what the participant exports, and
    decision H keeps a private type out of `exports`). It is kept because a
    harness test often needs to build or inspect a value the system keeps to
    itself. Spec §5.1c and §5.9 state it, and the positive fixture
    `2618_private_type_in_system_suite` pins it.
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
  Negotiation reads a bundle's visibility off its `@reference` block alone,
  and emission is unchanged (decision J), so the commons' own `render` still
  reads every block. `private` on a `messages` block therefore never does
  what it suggests, and it warns with `bynk.visibility.private_messages`:
  - on the `@reference` block, because no context that uses the commons will
    negotiate against the bundle (the note suggests removing `private`);
  - on any other block of a bundle whose `@reference` block is public,
    because it has no effect: the block's locale is still negotiated.

  The warning applies to a commons only. A `messages` block in a context or
  adapter is already `bynk.messages.outside_commons`, and a bundle with no
  `@reference` block, or more than one, is already an error, so neither
  warns as well. It fires whether or not any context uses the commons yet:
  the warning is about what the modifier does, not about a particular user.
  The modifier is still admitted so that it means the same on every
  declaration it applies to. The positive fixture
  `2618_private_messages_warns` pins both warnings and that `render` still
  reads a private bundle.
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
