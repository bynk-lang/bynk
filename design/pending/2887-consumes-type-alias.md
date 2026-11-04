---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "`consumes U { T as Alias }` renames a type `U` exports for the consuming context, so `consumes bynk.crypto { Crypto, Digest as CryptoDigest }` no longer clashes with your own `Digest`; the `name_conflict` note names the way out (#2887)"
---

## ADR: consumes-type-alias
title: `consumes U { T as Alias }` renames an exported type for the consuming context
summary: How a consumed type that clashes with a local one is kept apart

**Context.** `consumes U { Cap }` brings *every* type `U` exports into scope, not
only the capabilities named in the braces, because the capabilities' signatures
use them. `bynk.crypto` exports `Digest`, `Signature`, `CryptoError`,
`SigningKey` and `VerifyKey`; a context with its own `Digest` (a `uses` commons
type, or a local one) was `bynk.consumes.name_conflict`, and the note
("restructure the import") named no form that worked (#2887). Three ways out
were considered: select only what is named (no help — `Crypto.digest`'s own
signature takes a `Digest`), let local declarations shadow consumed ones, or
rename. The owner chose renaming.

**Decision.** A braced `consumes` list item may be `T as Alias`. The exported
type `T` is then in scope in the consuming context only as `Alias`; the bare `T`
from that unit is not in scope, so it cannot clash. Un-aliased exports behave as
before (eagerly in scope, conflicts reported as before).

- *Identity.* The alias is the unit's real type under another name, not a new
  nominal type. Internally the consuming context's tables file the declaration
  under the alias (its references to other aliased exports renamed with it), and
  the signatures lifted from the consumed unit (capability operations, service
  parameters) are renamed the same way, so `Crypto.digest(CryptoDigest.Sha256, …)`
  and a local `Digest` stay distinct types. Emission imports the module's real
  declaration (`import { Digest as CryptoDigest } from …`), and a test module
  destructures it the same way.
- *Scope.* A context's `consumes` clauses apply to all its files, so an alias
  written in any file applies to the whole context; another file may repeat the
  same alias, and one that disagrees is refused.
- *Adapters only.* Only a type exported by an adapter (`bynk.crypto`, a user
  adapter) can be aliased; aliasing a context's type is refused
  (`bynk.consumes.type_alias_context`). A context's types cross its service
  boundary by name: the contract hash, the callee's codecs and the namespace the
  caller reaches them through all spell the declared name, so a caller-side
  alias would have to be translated in each of those. An adapter is linked
  in-process and has no such boundary, so its types are plain imports. Extending
  aliasing to contexts means threading the alias through those three; it is
  deliberately left out.
- *Only types.* Aliasing a capability (`Crypto as C`) is refused
  (`bynk.consumes.type_alias_capability`): a flattened capability is keyed by its
  bare name through `given`, the composition root and the test doubles, so a
  rename would need a second spelling threaded through all of them for no clash
  it could remove — capabilities already have the qualified `given U.Cap` form.
- *Refusals.* Naming something `U` does not export
  (`bynk.consumes.type_alias_unknown_export`); aliasing one type twice, or both
  bare and aliased (`bynk.consumes.type_alias_repeated`); an alias that collides
  with a type or value in scope, another export's name, or another alias
  (`bynk.consumes.type_alias_conflict`).
- The `bynk.consumes.name_conflict` note now names the way out
  (`consumes bynk.crypto { …, Digest as CryptoDigest }`).

Selective import, local-wins shadowing and qualified access (`crypto.Digest`)
were not taken: the first does not remove the clash, the other two change what an
unqualified name means for code that already compiles.

**Consequences.** Existing code compiles unchanged: no existing golden moves. The
syntax adds an optional `as Alias` after a braced item (tree-sitter, the
formatter and the IDE follow: hover and go-to-definition on the alias reach the
declaration, and completion offers nothing at the alias position). The
organise-imports action keeps a clause that aliases a name as written rather than
pruning and re-sorting it. A `system`-tier suite's participant view still files a
consumed type under its own name, so a `system` case naming an aliased type uses
the module's name for it; that is a follow-up if it proves to matter.
