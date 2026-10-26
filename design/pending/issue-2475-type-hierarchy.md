---
level: minor
changelog: "`bynk-lsp` serves LSP type hierarchy: a refined, opaque or alias type's supertype is its primitive base, and a primitive's subtypes are every such type across the project; an opaque type's representation is shown only inside its declaring unit"
---

## ADR: lsp-type-hierarchy
title: LSP type hierarchy exposes a type's primitive base, hiding opaque representations outside their unit
summary: A supertype is a refinement's base or an opaque type's representation; an opaque base shows only inside its declaring unit

**Context.** Bynk has no inheritance, but refined, opaque and alias types sit over a base type. Nothing answered "what refines `Int` here?". The proposal (#2475) assumed chains such as `Quantity` refining `PositiveInt`; in the language a base is always a builtin primitive (`TypeBody::Refined`/`Opaque` carry a `BaseType`), so the chain is one step. Opacity is the point of `opaque`, so showing its representation everywhere would leak it.

**Decision.** `bynk-lsp` serves `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes` and `typeHierarchy/subtypes` from the binding index's refinement edges. A supertype is a refined/alias type's base, or an opaque type's representation type; capability → provider stays with go-to-implementation. A primitive is the only node with subtypes (every refined/opaque/alias type over it, project-wide). An opaque type's representation is visible only to a hierarchy opened inside its declaring unit; from any other unit it has no supertypes and is omitted from its primitive's subtypes. The index's `RefineEdge` records whether the type is opaque. The primitive node has no source, so it is anchored on the declaration it was reached from. The server's `lsp-types` has no type-hierarchy server capability, so the feature is registered dynamically at `initialized`.

**Consequences.** One-step hierarchies today; a future language change allowing a refinement over a declared type would extend `supertypes` without changing the wire shape. A client without dynamic registration gets no type hierarchy. No language, checker, emitter or `vscode-bynk` change: VS Code renders the view itself.
