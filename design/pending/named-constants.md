---
level: minor
probes: emit_diagnostics, diagnostic_coverage
changelog: "A commons or context declares a named constant with `const Name: Type = <static expression>`, usable wherever an expression of its type is and imported through `uses` like a function. The type is required, the name is upper-camel, and the initialiser is a literal, a constructor over static values, `T.unsafe(lit)`, an empty collection, a `Duration` literal or another constant, with no cycles; it is emitted as an exported TypeScript `const` ([#2617](https://github.com/bynk-lang/bynk/issues/2617))"
---

## ADR: named-constants
title: Named constants are typed, upper-camel and statically initialised
summary: `const Name: Type = <static expression>` in a commons or context joins the unit's value namespace, takes ADR 0004's static set plus references to other constants, refuses cycles, and emits as an exported TypeScript `const` in dependency order

**Context.** A program that wants a named value (a retry limit, a default
page, a timeout) had two workarounds: a nullary function (`fn maxRetries() ->
Int { 3 }`), which reads as a computation and is called at every use, or a
literal repeated at each use. `embed` (#2387) already showed the shape of a
top-level declaration naming a value: in the unit's value namespace, imported
through `uses` and by sibling files like a function, and emitted as an
exported `const`. ADR 0004 already defined a closed set of *static
expressions* for agent state initialisers.

**Decision.**

- **A. The initialiser is a static expression (ADR 0004) or another
  constant.** The closed set is: an `Int`, `Float`, `Bool` or unit literal, a
  negative numeric literal, a plain string literal (no interpolation), a
  `Duration` literal, a parenthesised static expression, a list literal, a
  record construction with every field given (no spread, no shorthand), a
  sum-variant construction (bare or qualified nullary variant, or a variant
  call), `Some`/`None`/`Ok`/`Err`, each over static values, `T.unsafe(lit)` for
  an opaque type, `List.empty()`/`Map.empty()`/`Set.empty()`, and a reference
  to another constant. Anything else is `bynk.resolve.bad_const_initialiser`.
  The set is checked syntactically first; the initialiser is then typed
  against the annotation in an empty, pure scope, as a state initialiser is,
  so literal admission into refined types, the confinement of `.unsafe` to its
  defining commons, and variant resolution work as they already do. A
  diagnostic that typing raises (`bynk.refine.literal_violates`) is reported
  as it stands; a value of another type is `bynk.resolve.bad_const_initialiser`.
  Constants that refer to each other in a cycle are `bynk.resolve.const_cycle`,
  reported once per cycle at the member declared first in the file and naming
  the cycle in order.
  - **Rejected:** arithmetic on literals (`60 * 60`). It is arguably in ADR
    0004's spirit, but the set is closed on purpose, and widening it is a
    separate decision (a possible follow-up).
  - **Rejected:** a pure function call. The checker could prove it pure, but
    "static" then means "whatever the checker can evaluate", and the value
    would no longer be visible in the source.
- **B. The type annotation is required.** A constant is part of its unit's
  interface, read by a `uses`-ing unit, so its type is written, not inferred.
  `const Name = …` is a parse error, `bynk.parse.const_missing_type`, whose
  note shows the annotated form.
- **C. Constants in compile-time positions are out of scope.** A refinement
  bound (`InRange(1, MaxQty)`) or an annotation argument still takes a
  literal. That needs evaluation at check time and is a named follow-on.
- **D. The name is upper-camel,** the lexer's `constant_name` class
  (`/[A-Z][A-Za-z0-9_]*/`), so a constant reads differently from a binding. A
  lower-case name is `bynk.resolve.const_name_case`, with an upper-camel
  suggestion. The grammar takes any identifier and leaves the case to the
  checker, as it does for variants, so the error is a diagnostic with a fix
  rather than a parse failure.
- **E. Constants share the unit's one value namespace** (spec §5.1). A
  constant may not share a name with a function, asset, embed, type, event,
  service, agent or provider in the unit (`bynk.resolve.name_conflict`), nor
  with another constant (`bynk.resolve.duplicate_const`). `uses` composes them
  one level, a local declaration of any kind wins, and two used commons
  declaring the same name are `bynk.uses.name_conflict`. A `let` binding or a
  parameter shadows a constant, as it shadows an embed.

Choices this increment made that the proposal left open:

- **Placement.** Like `embed`, `const` parses in every body (commons, context,
  adapter); in an adapter it is
  `bynk.resolve.const_outside_context_or_commons`. A `suite` body does not
  declare constants; a unit-tier case names its target's constants and those of
  the commons the target `uses`, as with embeds. A `system` suite sees none.
- **Two more name rules,** both conservative. A constant may not take a
  built-in type's name (`List`, `Map`): the parser reads such a name as the
  type in every later reference, so the constant could never be used. And it
  may not take the name of a variant of a sum type in scope, including a
  predeclared `HttpResult` variant (`Accepted`, `NotFound`): a bare `Name`
  would read as either, and the emitter qualifies a bare variant by its type.
  Both are `bynk.resolve.name_conflict`.
- **Keyword tier.** `const` is a reserved contextual keyword, as `embed` is: a
  keyword only at item position, an ordinary identifier (a parameter, a field)
  everywhere else, so no existing program breaks.
- **Emission.** Each constant is `export const Name: T = <init>;`, the
  initialiser lowered by the ordinary expression lowering (the agent state
  initialiser's `StaticInit` mode), after the module's types (an initialiser
  may construct one) and before its functions. A module's constants are
  topologically ordered so each comes after every constant of the same file it
  names; a constant of a sibling file or a used commons is imported, as a
  function is. A module without constants emits byte for byte as before.
- **Editor support.** Constants get their own index kind and a new
  semantic-token type, `constant`, appended to the legend at index 15. Hover
  shows the declaration with its type and initialiser; completion, document
  symbols and workspace symbols use the LSP `CONSTANT` kind.

**Consequences.** A program names a fixed value once, with its type, and the
emitted module holds a real `const` a bundler can inline or drop. The checker
and emitter reuse the embed plumbing (unit tables, `uses` composition, the
file declaration index) and the static-initialiser typing, so a constant
behaves like an embed everywhere names are resolved.

- Two constants in *different files* of one unit that refer to each other's
  file in both directions (file A's constant naming file B's, and B's function
  or constant naming A's) give the emitted modules an import cycle, and an
  ES module cycle evaluates one module's `const` before the other's is
  initialised. A cycle *between constants* is refused, but this module-level
  shape is not detected; keeping mutually dependent constants in one file
  avoids it.
- A constant's type is resolved where the constant is used, in that unit's
  type namespace, as an imported function's signature is.
- The follow-ons are constants in compile-time positions (decision C) and,
  if wanted, arithmetic on literals in an initialiser.
