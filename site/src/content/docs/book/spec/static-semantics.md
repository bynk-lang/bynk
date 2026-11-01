---
title: "§5 Static semantics"
---
A program that parses ([§3](/book/spec/lexical-grammar/), [§4](/book/spec/syntactic-grammar/)) is
not yet known to be well-formed. This chapter states the **well-formedness
rules**: the conditions a program MUST satisfy beyond parsing, each tied to the
`bynk.*` diagnostic a conforming implementation emits when the rule is violated
([§1.3](/book/spec/scope/)). A program is well-formed exactly when it provokes no such
diagnostic.

> [!NOTE]
> Lexical and grammatical errors — the `bynk.lex.*` and `bynk.parse.*` codes —
> are *syntactic*: they report a text that does not match the grammar, and are
> governed by §3 and §4. This chapter covers only **post-syntactic**
> well-formedness. This note is informative.

The rules are organised by theme. Each theme states its load-bearing rules and
cites the governing codes; the **exhaustive** code-by-code catalogue is the
[diagnostic index](/book/reference/diagnostics/) (and §9). Where a theme maps to a
single construct, its full set of governing diagnostics is surfaced inline with
`{{#grammar-semantics}}`.

## §5.1 Name resolution & visibility

Every referenced name MUST resolve to a declaration in scope
(`bynk.resolve.unknown_name`, `bynk.resolve.unknown_type`,
`bynk.resolve.unknown_function`, `bynk.resolve.unknown_field`). A name used where
a value is expected MUST denote a value, not a type (`bynk.resolve.type_in_expr`,
`bynk.resolve.type_as_function`); a function MUST be called, not referenced bare
(`bynk.resolve.fn_without_call`).

Within a scope, names MUST be unique: duplicate types, functions, methods,
services, capabilities, providers, agents, record fields, variants, and
parameters are each rejected (the `bynk.resolve.duplicate_*` codes). A `let`
binding MUST NOT shadow a function or a type (`bynk.resolve.let_shadows_fn`,
`bynk.resolve.let_shadows_type`), but a `let` MAY re-bind a name already bound
by a prior `let` in the same scope — a deliberate ML-family idiom the
locals-in-scope tooling (ADR 0064) resolves as "latest in-scope def wins" —
with each occurrence free to change type. The emitter gives a re-bound name
its own emitted identifier so each `let` still lowers to a distinct binding.

A unit's top-level declarations share one namespace, except capabilities and
actors, which each keep their own. A function, an asset, a type or event, a
service, an agent and a provider with a body (by its implementation name,
`provides Cap = Name`) MUST NOT share a name, across all of the unit's files
(`bynk.resolve.name_conflict`; two types, or a type and an event, are
`bynk.resolve.duplicate_type`). They are all one scope of the emitted module.

A bare reference to a **named function** is a value only where a function
type is expected (v0.20a); elsewhere it MUST be called
(`bynk.resolve.fn_without_call`). A call on an in-scope **value** is legal
only when the value's type is a function type
(`bynk.resolve.param_as_function` otherwise) — both judgments are made by the
checker, with the type information they require; a *type name* is never
callable (`bynk.resolve.type_as_function`). Call resolution prefers declared
functions, then variant constructors, then agents, then in-scope values —
scope-first call resolution would change the meaning of existing programs, so
the pre-existing ident/call precedence asymmetry is preserved deliberately.

A `commons` is imported with `uses`, which MUST name an existing `commons`, not a
context, and MUST NOT be self-referential or introduce a colliding name
(`bynk.uses.unknown_commons`, `bynk.uses.target_is_context`,
`bynk.uses.self_reference`, `bynk.uses.name_conflict`). The visibility of types
across context boundaries is governed by `exports` and `consumes`
([§5.8](#58-boundaries--cross-context)).

`uses` is one level for naming: a unit names the types its `uses` targets
declare, not the types *they* use. An imported declaration's types are still
resolved where it was declared. If `t.web uses t.model`, and `t.model uses
t.core` to declare `type Run = { repo: Repo }`, then in `t.web` a `Run`
literal's `repo` is checked as a `Repo`, and `r.repo` is a `Repo`. The same
holds for an imported function's signature, for a reached type's methods, and
for a type that a consumed context's export reaches through that context's
`uses`. A reached type MUST NOT be named in the unit (`bynk.resolve.unknown_type`).
To name it, `uses` its commons. A unit's own declaration MUST NOT share a
reached type's name when that type's commons is not one the unit `uses`
(`bynk.uses.name_conflict`), since the imported declaration would then be typed
by the wrong one.

A unit's own declaration MAY shadow a type of a commons it `uses` directly. An
imported declaration that names the shadowed type still means its own commons'
type, so a value MUST NOT cross between the unit's code and such a position
(`bynk.uses.name_conflict`, at the crossing): a value the unit supplies for a
record field, a parameter or a variant payload declared with that type, and a
value the unit takes from one (a field read, a call's result, a bound payload).
A value moved from one such position directly into another that names the same
types is not a crossing. If `t.app` declares `type Repo = Int` beside `uses
t.core` and `uses t.model`, then `Run { repo: first(r) }` is legal in `t.app`,
and `Run { repo: 42 }` and `r.repo + 1` are not.

A unit's own declaration also shadows a function or asset of the same name
from a commons it `uses`, whatever the local declaration's kind, as in the
emitted module: the import is not in scope. Where the local declaration cannot
answer a reference (a call to a type or service, a bare event name, an asset
used as a function), the reference is `bynk.uses.name_conflict`, naming both
declarations. A local declaration that can answer one does: a call to a local
agent constructs the agent. An unused shadow is legal.

A local value (a function, asset, service, agent or provider) likewise hides a
type of the same name from a commons the unit `uses`. The type stays
resolvable, so an imported declaration that uses it still checks and a value
of it still flows through the unit's code (`isDone(current())`). But the unit
MUST NOT write its name (`bynk.uses.name_conflict`): not in a type position, a
construction, a receiver (`Status.Done`), a pattern's qualifier or an `is`
test, and not by constructing one of its variants bare (`Done`), which names
the type in the emitted module too. Matching a bare variant is legal. A local
value that can answer a receiver still does: a suite's `Status.GET("/")`
addresses a service `Status`, and `Status.url` reads an asset `Status`. A unit
suite may name no more than its target. A local type or event over an imported
type is the type shadowing above.

A context's local value MUST NOT share its name with a type a context it
`consumes` exports (`bynk.consumes.name_conflict`, at the `consumes` clause),
as a local type of that name already may not.

### §5.1a Module documentation {#module-docs}

A doc-block ([§3.3.2](/book/spec/lexical-grammar/#332-doc-blocks)) immediately
above a `commons`, `context` or `adapter` header is that unit's **module doc**.
A unit whose files share its qualified name
([§8.3](/book/spec/compilation-model/#83-source-layout-and-test-ness)) MUST carry
at most one module doc across all of them: when two or more of its files carry
one, each such file is rejected (`bynk.project.duplicate_module_doc`), and the
diagnostic names every such file. A `suite` is not part of the unit it targets,
so a suite's own doc-block is not counted. Nor is a doc-block whose content is
empty or only whitespace: it documents nothing, so it is not a module doc.

> [!NOTE]
> The rule exists so that a unit's documentation is one piece of prose,
> independent of file order. The tooling shows the module doc on hover over the
> unit's name (its header, a `uses` target, a `consumes` target) and as the lede
> of the unit's documentation page, which merges every file of a multi-file
> unit. This note is informative.

### §5.1b One doc per target {#one-doc-per-target}

A declaration or member ([§3.3.2](/book/spec/lexical-grammar/#332-doc-blocks))
MUST carry at most one doc. A doc directly followed by another, of either
shape, with no blank line between, is rejected (`bynk.parse.multiple_docs`); a
doc of more than one line is written as a doc-block. A comment or doc in a
retired spelling ([§3.3.3](/book/spec/lexical-grammar/#333-retired-spellings))
is rejected (`bynk.lex.legacy_comment`, `bynk.lex.legacy_doc_line`,
`bynk.lex.slash_doc`), each with a machine-applicable fix; a retired comment is
still read as a comment, so no unmigrated source is accepted with another
meaning.

## §5.2 Well-typedness

Every expression MUST have the type its position requires. A function or method
argument MUST match the parameter type (`bynk.types.argument_mismatch`), and a
call MUST supply the right number of arguments (`bynk.resolve.arity_mismatch`,
`bynk.types.method_arity`). A returned value MUST match the declared return type
(`bynk.types.return_mismatch`); a `let` value MUST match any annotation
(`bynk.types.let_annotation_mismatch`); a record field MUST be given a value of
its type (`bynk.types.field_value_mismatch`), and every required field MUST be
supplied (`bynk.resolve.missing_field`).

An `if` condition MUST be a `Bool` and both branches MUST **join** to a common
type — their least upper bound, so a refined type and its base (or two refined
types over one base) agree at the base, but unrelated types do not
(`bynk.types.if_non_bool_cond`, `bynk.types.if_branch_mismatch`). The payloads of
`Ok`, `Err`, `Some`, and the like MUST match the expected component type (the
`bynk.types.*_value_mismatch` codes). A bare `Ok` takes `Result` or `HttpResult`
from its expected type when that type is either, and otherwise from the enclosing
return type, so it is never ambiguous (#1659).

**Lambdas** (v0.20a). Against an expected function type, a lambda's
parameters take the expected types (an annotation MUST agree), its body is
checked against the expected return — a pure body auto-lifts into an
effectful expectation — and arity MUST match (`bynk.types.lambda_mismatch`).
In a position with no expected function type, every parameter MUST be
annotated (`bynk.lambda.unannotated_param`) and the lambda's type is read off
its body: a body that performs an effect operation (an `<-` bind, a
capability call, a call returning `Effect`) makes the lambda **effectful**,
wrapping its result in `Effect` — effectfulness is judged by the *presence*
of effect operations, never by a pre-declared result type, which is what
dissolves the apparent circularity. A nested lambda's effects are its own.

**Value application** (v0.20a). Applying a function-typed value checks
arguments against the function type's parameters
(`bynk.types.argument_mismatch`, `bynk.types.call_arity`).

**Recursion** (#2612). A function MAY call itself, and functions MAY call one
another in a cycle: a free function, a generic function, and a method alike. A
call cycle stays within one unit, because `uses` clauses may not form a cycle;
a recursive function in a unit another `uses` is called like any other function.
A recursive call is checked like any other call, and a recursive generic
function instantiates its type parameters at each call as usual. A lambda has no
name of its own, and a `let` binding is not in scope in its own initialiser, so
`let f = (x) => f(x)` is rejected (`bynk.resolve.unknown_function`): a lambda
cannot call itself. Termination is not checked: a well-formed program may recurse without
bound, and what happens at run time is specified in
[§7.3.2](/book/spec/emission/#732-expressions).

A free function cannot name a capability ([§5.5](#55-effects-capabilities--providers)),
so it cannot perform an effect of its own accord. It can perform the effects of
an effectful function value it is given, so a recursive function can drive
capability calls on behalf of the handler that passed it one:

```bynk
commons loops {
  fn repeat(n: Int, step: Int -> Effect[()]) -> Effect[()] {
    if n <= 0 {
      Effect.pure(())
    } else {
      do step(n)
      repeat(n - 1, step)
    }
  }
}
```

Iteration over a collection is the kernel's (`fold`, `forEach`, the traverse
family, [§5.10](#510-collections)), which needs no recursion and has no depth
limit; recursion is for structures and loops the kernel does not cover.

**Numeric operators** (v0.21). The arithmetic operators `+ - * /` are
defined on `Int` operands (yielding `Int`) and on `Float` operands
(yielding `Float`). They MUST NOT mix the two: an `Int` and a `Float`
operand in the same operation is `bynk.types.no_numeric_coercion` — there
is **no implicit numeric coercion** in either direction. The same rule
applies to the comparison operators `< <= > >=` and to `==`/`!=`. Refined
numeric types widen to their base in operator positions, as before.

**Ordering** (constrained generics, slice 1). `< <= > >=` accept two operands
of the **same `Ord` type**; a type that is not `Ord` is
`bynk.types.not_orderable`. A type is `Ord` when it is one of:

- `Int`, `Float`, `String`, `Duration`, `Instant` or `Bytes`, or a refined type
  over one of them;
- `Option[T]` or `List[T]` where `T` is `Ord`;
- a record, sum, enum or opaque type that declares `derives Ord` (§4.2.1),
  provided every field and payload type is `Ord`
  (`bynk.types.derives_field_not_orderable` otherwise). A generic one is `Ord`
  at an instantiation whose type arguments are;
- a type parameter declared `T: Ord`.

`Bool`, `Result`, `Map`, `Set`, function types and held resources are not `Ord`,
and an opaque type is `Ord` outside its defining unit only if it declares
`derives Ord`. A type with no `derives Ord` is never ordered implicitly, because
opting in makes declaration order significant.

Every `Ord` type has a **total order**, and an implementation MUST compute it as
follows:

- `Int`, `Duration` and `Instant` order numerically; a refined type orders as
  its base.
- `String` orders by **UTF-16 code unit**, element by element, a proper prefix
  first. This is not Unicode code-point or locale order.
- `Bytes` orders **lexicographically over unsigned octets**, a proper prefix
  first.
- `Float` orders as a **total order**: `NaN` sorts after every other value and
  equals itself, and `-0.0` equals `0.0`. This applies wherever the order is
  used as an order, that is the ordering kernels (`sortBy`, `min`, `max`),
  `bynk.list.sort`/`min`/`max`, and every `T: Ord` witness. The operators
  `< <= > >=` on `Float` keep the IEEE meaning (`NaN < x` is `false`), and
  `==` is unchanged.
- `Option` orders `None` before every `Some`, and `Some` by payload.
- `List` orders lexicographically by element, a proper prefix first.
- A record orders by its fields in **declaration order**, the first differing
  field deciding.
- A sum or enum orders by **variant declaration order**, then by payload (fields
  in declaration order) between two values of the same variant.

Reordering the fields or variants of a `derives Ord` declaration therefore
changes how its values sort. The contract hash ignores field and variant
order, so the change is not a contract change.

**Equality is structural** (#1652). `==`/`!=` compare **values**, not
references. Two records are equal when every field is equal; two sum values when
they are the same variant with equal payloads; two `Option`/`Result`/`List`/`Map`
values element by element; two `Bytes` byte by byte. This holds at any depth, and
for an opaque type it compares the representation. `==` is defined only on an
**equality-supporting** type: a type that contains a function, `Effect`, `Query`,
`Stream` or held `Connection` **anywhere** inside it (in a field, a payload, a
container element, or a generic argument written in the type, such as
`Box[(Int) -> Int]`) is not equality-supporting, and `==` on it is rejected
(`bynk.types.not_comparable`, `bynk.types.stream_not_comparable`,
`bynk.types.held_not_comparable`). Inside a generic function or method, `==` on
a type that mentions a type parameter is accepted only when that parameter is
**declared `Eq`** (or `Ord`, which implies it): by a `T: Eq` bound on the function, a `where A: Eq` entry on
a method (for its receiver type's parameter), or a bound its receiver type
declares; otherwise it is `bynk.generics.missing_eq_bound`. Passing a type
parameter to an `Eq`-bounded parameter of another generic is the same rule,
reported at the call, so a bound is declared at every link of a chain of calls.
Each call MUST instantiate an `Eq`-bounded parameter with an equality-supporting
type (the same codes, reported at the call: `same(f, h)` on two functions is
rejected for `fn same[T: Eq](a: T, b: T) -> Bool { a == b }`, and `b.has(dec)`
for `fn Box.has(self, x: A) -> Bool where A: Eq { self.value == x }` on a
`Box[(Int) -> Int]`). Every `missing_eq_bound` carries a machine-applicable fix
that adds the bound. One case is not yet checked, and
falls back to comparing such values by identity: a record imported from another
unit is not walked through its fields. A nullary variant read from storage or decoded
from JSON equals the same variant written in source.

**`Float` equality** (v0.21). `==`/`!=` on `Float` follow the host's IEEE
754 semantics, and both classic surprises apply: `0.1 + 0.2 != 0.3`
(decimal fractions are not exact doubles), and a `NaN` produced by
arithmetic is **unequal to itself**, including inside a record or a list
(structural equality compares each `Float` with these same semantics). Exact `Float` equality is rarely the
test a program needs — compare with an explicit tolerance, or work in
`Int` units. Division by zero and overflow in `Float` arithmetic follow
the host (`Infinity`/`NaN`); no Bynk-level guard applies **in arithmetic**
(boundaries are guarded: [§7.2](/book/spec/emission/#72-targets)).

**The `Int` domain** (#1657). An `Int` is a JS **safe integer**, from
−(2^53 − 1) to 2^53 − 1 (±9007199254740991), the range the runtime represents
exactly. Every entry point enforces it:
- **literals:** an integer literal with a larger magnitude is
  `bynk.lex.integer_overflow`;
- **the JSON boundary:** decoding rejects a number that is not a safe integer
  (`1e300`, `3.5`), and encoding an `Int` that is not one faults rather than
  writing `null`;
- **`Int.parse`:** see below;
- **`Int` division by zero:** a runtime fault, not `Infinity`/`NaN`;
- **`round`/`floor`/`ceil`/`truncate`:** a runtime fault when the result is not a
  safe integer (a non-finite `Float`, or one past ±2^53).

Overflow in `Int` `+`, `-` and `*` is not checked: past 2^53 the result is
imprecise, as host arithmetic is (ADR 0042, "arithmetic host-defined,
boundaries guarded"). Such a value can still not leave through a boundary
undetected.

**The numeric kernel** (v0.21, extended v0.22a). Conversion between the
numeric types is explicit, via built-in value methods on the bare base
types: `i.toFloat() -> Float` (total) on `Int`; `f.round()`, `f.floor()`,
`f.ceil()`, `f.truncate()` (each `-> Int`, named and lossy) on `Float` —
there is deliberately no ambiguous `toInt`. v0.22a (ADR 0048) adds, on
**both** numeric types, `x.abs()`, `a.min(b)`, `a.max(b)`, and
`x.clamp(lo, hi)` (arguments take the receiver's type — mixing is the
no-coercion error), and on `Float` only, `f.isNaN()` and `f.isFinite()`
(`-> Bool`). v0.42 (ADR 0074) adds `x.toString() -> String` on **both** types
(total — the render direction `Int.parse` lacks); for `Float` the result is the
**host's number→string** (ECMAScript `Number::toString` — shortest round-trip),
pinned to the platform the same way ADR 0046 pins the string kernel. Wrong arity
is `bynk.types.method_arity`; an unknown method on a numeric receiver is
`bynk.types.method_not_found`.

**Integer division companions and power** (standard-library K6b, #2598). On
`Int`, each faulting on a zero divisor as `/` does:
- `a.rem(b)` is the remainder of `/` (which truncates). It takes the
  dividend's sign: `(-7).rem(2) == -1`, and `a == b * (a / b) + a.rem(b)`.
- `a.div(b)` is floor division (`(-7).div(2) == -4`), and `a.mod(b)` its
  remainder, taking the divisor's sign: `(-7).mod(2) == 1`, and
  `a == b * a.div(b) + a.mod(b)`. For a positive `n`, `x.mod(n)` is in
  `0..n-1`.
- `a.pow(n)` is an exact power. A negative `n` faults (there is no `Int`
  result), and so does a result past ±(2^53 − 1). Unlike `*`, whose overflow
  is unchecked, `pow` overflows too readily for an imprecise result to be
  tolerable. `0.pow(0) == 1`.

**`Float` functions** (K6b). On `Float`: `f.sqrt()`, `f.pow(y)`, `f.exp()`,
`f.ln()`, `f.log10()`, `f.log2()`, and in radians `f.sin()`, `f.cos()`,
`f.tan()`, `f.asin()`, `f.acos()`, `f.atan()` and `y.atan2(x)` (the angle of
the point `(x, y)`), each `-> Float`. They are the host's `Math` functions, so
outside a function's domain the result is host-defined like the rest of `Float`
arithmetic (ADR 0042): `(0.0 - 1.0).sqrt()` is `NaN` and `0.0.ln()` is
`-Infinity`, with no fault. The statics `Float.pi()` and `Float.e()` give the
two constants.

**The numeric parse statics** (v0.22a). `Int.parse(s) -> Option[Int]` and
`Float.parse(s) -> Option[Float]` — statics, per 0041's rule (ways to
*obtain* a value). Parsing is **full-string** in a strict decimal grammar
(#1657): `Int.parse` accepts `[+-]?[0-9]+` only, and `Float.parse` accepts
`[+-]?` then digits with an optional fraction (`5.`, `.5`), then an optional
exponent (`1e3`). Anything else is `None`: surrounding whitespace, a
`0x`/`0b`/`0o` prefix, an exponent or fraction in an `Int`, the empty string.
A value outside the safe-integer range (`Int`) or non-finite (`Float`) is `None`,
and `Int.parse("-0")` is `0`. `parse` is the only static on the numeric types
(`bynk.resolve.unknown_static_member`).

**The string kernel** (v0.22a, ADR 0046). `String` is opaque — no direct
character access — so its operations are built-in value methods:
`s.length() -> Int`, `s.split(sep) -> List[String]`, `s.trim()`,
`s.toUpper()`, `s.toLower()`, `s.concat(t)`, `s.contains(sub)`,
`s.startsWith(sub)`, `s.endsWith(sub)` (`-> Bool`),
`s.replace(a, b)`, `s.slice(lo, hi)`, `s.indexOf(sub) -> Option[Int]`,
and `s.chars() -> List[String]`. **Semantics are UTF-16 code units**,
normatively, with two pinned exceptions: `replace` replaces **every**
occurrence (not TS's first-only string form) and inserts `b` **literally**
(no `$&`, `$1`, `` $` ``, `$'` or `$$` expansion, #1650), and `chars()` splits by
**code points** (so `s.length() != s.chars().length()` when `s` contains
astral characters). `slice` clamps negative indices to `0` — there is no
wrap-around. `indexOf` returns `None` for a missing substring, never a
sentinel `-1`.

**Percent-encoding** (standard-library K2, #2594) works on the **UTF-8 bytes**,
not code units. `s.percentEncode() -> String` keeps RFC 3986's unreserved set
(`A–Z a–z 0–9 - . _ ~`) and writes every other byte as `%XX` in upper-case hex,
so `! ' ( ) *` are encoded too, and a lone surrogate is encoded as U+FFFD.
`s.percentDecode() -> Option[String]` reads `%XX` in either case and is `None`
for a `%` without two hex digits after it, or for bytes that are not UTF-8; it
leaves `+` as `+`. `percentDecode(percentEncode(s))` is `Some(s)`. URL structure
is the `bynk.url` commons, written in Bynk over these.

**Regular expressions** (standard-library K6c, #2600). Six string methods take a
pattern: `s.matchesRegex(p)` (the whole string, anchored as `Matches` is) and
`s.containsRegex(p)` (anywhere), `-> Bool`; `s.findRegex(p) ->
Option[List[Option[String]]]` (the first match: element 0 the whole match,
element `i` group `i`, `None` for a group that did not take part);
`s.findAllRegex(p) -> List[List[Option[String]]]` (every match, left to right,
an empty match advancing by one code unit); `s.replaceRegex(p, with) -> String`
(every match, `with` read with JS replacement syntax: `$1`, `$<name>`, `$&`,
`$$`); and `s.splitRegex(p) -> List[String]` (JS `split`, without the captured
groups it would splice in).
- The pattern MUST be a string literal (`bynk.types.regex_not_literal`), so it is
  checked at compile time exactly as a `Matches` pattern is: ECMAScript syntax
  (`bynk.types.invalid_regex`), no nested unbounded quantifier and no ambiguity
  the analysis finds (`bynk.types.catastrophic_regex`,
  `bynk.types.polynomial_regex`; no length bound can excuse a polynomial one
  here).
- Semantics are JS `RegExp` with `Matches`' flag set: no `u`, so `.` and
  classes work on UTF-16 code units. A pattern may open with one inline-flag
  group, `(?i)`, `(?m)`, `(?s)` or a combination, which becomes the `RegExp`
  flags; an unknown or repeated flag is `invalid_regex`. The ambiguity analysis
  runs under those flags: under `s`, `.` matches every code unit, and under
  `i`, every character set is case-folded, so `(?i)(a|A)+` is refused.
- `matchesRegex` anchors with lookarounds, `(?<![\s\S])(?:…)(?![\s\S])`,
  not `^…$`, so an `m` flag cannot turn it into a line match.

**Refined receivers inherit the base kernel** (v0.143, ADR 0168). A method
call whose receiver is a **refined** type resolves against that type's declared
instance methods first; on a miss, it resolves against its **base type's
read-only kernel** — the numeric, string, `Duration`, `Instant`, and `Bytes`
kernels above. The result is **base-typed**: `n.toUpper()` on a `Name = String
where …` has type `String`, never `Name` — the same widening a refined value
already undergoes in arithmetic (D2/D3) and comparison. A declared method always
takes precedence over the inherited kernel, so it can never be shadowed. `Bool`
has no kernel, so a `Bool`-based refinement inherits nothing. An **opaque** type
does **not** widen and therefore inherits nothing — a kernel call on an opaque
receiver stays `bynk.types.method_not_found`. Refined arguments to an inherited
method widen to the base like any other argument.

**String interpolation** (v0.43, ADR 0075). An interpolated string
`"… \(e) …"` has type `String`. Each hole expression `e` must have type
`String`, `Int`, `Float`, `Bool`, or a **refinement** of one of those (which
widens to its base for display) — these are the types with a well-defined
string form (`Int`/`Float` via the ADR 0074 `toString` contract, `Bool` as
`true`/`false`). Any other hole type — `record`, `sum`, `Option`, `Result`,
`List`, an opaque type (whose base is hidden — `.raw` it first), … — is a
static error (`bynk.types.interpolation_non_scalar`): map the value to a
`String` first. The conversion is implicit only here, in a display context; it
does **not** generalise to arithmetic or comparison (ADR 0046 is unchanged —
`+` stays numeric, `concat` stays a method).

**The `Option`/`Result` kernel** (v0.22a, ADR 0048). The combinators are
built-in value methods on the compiler-known generic receivers — *not*
free functions, which would collide by bare name on `uses` import
(`bynk.resolve.duplicate_fn`). On `Option[T]`: `o.map(f)`,
`o.andThen(f)` (the function MUST return an `Option`), `o.getOrElse(x)`,
`o.isSome()`, `o.okOr(e) -> Result[T, E]`. On `Result[T, E]`: `r.map(f)`,
`r.andThen(f)` (the function MUST return a `Result` with the receiver's
error type), `r.mapErr(f)`, `r.getOrElse(x)`, `r.isOk()`. The function
argument's parameters type contextually from the receiver and its return
is read from the actual (the v0.20a pass-2 rule) — so a lambda body that
itself needs an expected type (a bare `Ok`/`Err`/`None`/`[]`) annotates a
`let`, exactly as with lambdas passed to generic calls.

```bynk
commons checkout {
  fn parseQty(s: String) -> Int {
    Int.parse(s.trim()).map((n) => n.clamp(1, 99)).getOrElse(1)
  }

  fn label(name: Option[String]) -> Result[String, String] {
    name.map((n) => n.toUpper()).okOr("missing name")
  }
}
```

**The `Effect[Result[T, E]]` kernel** (v0.152, ADR 0176 — design doc §2.8.3).
`Effect[Result[T, E]]` is the universal shape of cross-context calls, and the
volume of that composition earns four compiler-synthesised combinators directly
on the receiver — so success/error reshaping and effectful recovery need no
intervening `<-` peel and `match`:

| Method | Result |
|---|---|
| `e.mapOk(f: T -> U)` | `Effect[Result[U, E]]` |
| `e.mapErr(f: E -> F)` | `Effect[Result[T, F]]` |
| `e.flatMapOk(f: T -> Effect[Result[U, E]])` | `Effect[Result[U, E]]` |
| `e.flatMapErr(f: E -> Effect[Result[T, F]])` | `Effect[Result[T, F]]` |

`mapOk`/`mapErr` map the two sides of the success/error split; `flatMapOk`
chains a further effectful-fallible step on success (keeping the single error
type `E`, exactly as `?` does); `flatMapErr` attempts an effectful recovery on
error (its recovery MUST produce the receiver's success type `T`). The naming is
verb-first, matching `map`/`mapErr` on `Result`. A method outside the four is
`bynk.types.method_not_found`; a `flatMapOk`/`flatMapErr` argument that does not
return `Effect[Result[…]]`, or whose error/success side does not line up, is
`bynk.types.combinator_return_mismatch`. Only an `Effect` wrapping a
`Result` carries these — any other `Effect[_]` has no kernel methods. Unlike the
eager `List.forEach`/`traverseTry` iterators, these **produce** an `Effect`
rather than running one, so they are **not** effectful-context-confined: a pure
helper may reshape an `Effect[Result[…]]` it was handed and return it. `.map`
and `.flatMap` on such a receiver remain Effect's own (operating on the whole
`Result`); the four named methods remove the "which `.map`?" ambiguity. Other
`Effect`-of-X shapes (`Effect[Option[T]]`, `Effect[List[T]]`) have no synthesised
methods — write `e.map((r) => …)` explicitly.

**Declared error embeddings and `?` conversion** (v0.154, ADR 0178). A sum type
MAY declare error embeddings — `type OrderError = … embeds PaymentError as
Payment, ScheduleError as Fulfilment` — where each `embeds E as V` names a
variant `V` of the same sum that MUST have **exactly one payload field, of type
`E`** (`bynk.types.embeds_unknown_variant` if `V` is not a variant;
`bynk.types.embeds_variant_shape` if its shape does not match). A given source
type MAY be embedded by at most one variant, so the conversion is unambiguous
(`bynk.types.embeds_ambiguous`). The `?` operator then uses the embedding: in a
function returning `Result[_, F]` (or `Effect[Result[_, F]]`), applying `?` to a
`Result[T, E]` propagates as before when `E` matches `F`, and otherwise — when
`F` declares `embeds E as V` — **auto-wraps** the `Err(e)` into `Err(F.V(e))`
instead of requiring a manual `.mapErr`. The conversion is **one level**: `E`
must match a declared embedding of `F` directly. When neither the types match
nor an embedding applies, it is `bynk.types.question_error_mismatch`.

**The typed JSON codec** (v0.22b, ADR 0045). `Json.encode(v) -> String` and
`Json.decode[T](s) -> Result[T, JsonError]` are compiler-backed statics on
the built-in `Json` module: `encode` dispatches to the generated
`__serialise_<T>` for the value's checked type; `decode` to `JSON.parse` +
`__deserialise_<T>`. The **domain of `T`** (and of `encode`'s argument) is any
boundary-legal shape — base types, named types, and the built-in containers
over them; functions, effects, `HttpResult`, the error builtins, and type
variables are `bynk.types.json_uncodable`. `decode`'s target is given
explicitly (`Json.decode[Order](s)`, any boundary-legal type-ref including
`Json.decode[List[Order]]`) or inferred from an expected
`Result[T, JsonError]`; with neither, `bynk.generics.uninferable_type_arg`.
`encode` is `-> String` but **throws on a value containing a non-finite
`Float`** — the 0040 contract violation, documented rather than `Result`-ified
(the program itself created that state). A user-declared type named `Json`
shadows the built-in module.

**`JsonError`** (v0.22b, ADR 0047). The decode error is a compiler-known
record — `kind`, `path`, `message`, all `String` — putting a boundary
failure in the program's hands for the first time (the `ValidationError`
precedent). `kind` is `"Malformed"` for unparseable input, else the
boundary kind (`"StructuralMismatch"`, `"RefinementViolation"`); `path` is
the tracked field path (`$.items[2].qty`); decode failures are runtime
values, never compile diagnostics.

```bynk
commons store {
  type Item = {
    sku: String,
    price: Float,
    qty: Int,
  }

  fn snapshot(i: Item) -> String {
    Json.encode(i)
  }

  fn restore(s: String) -> Result[Item, JsonError] {
    Json.decode[Item](s)
  }

  fn restoreError(s: String) -> String {
    match Json.decode[Item](s) {
      Ok(i) => i.sku
      Err(e) => e.kind.concat(" at ").concat(e.path)
    }
  }
}
```

**Generic instantiation** (v0.20a). A generic function's type arguments are
inferred from its arguments by argument-directed unification: non-lambda
arguments first, left to right; lambda arguments after, against the
substituted expectations — a lambda whose expected *parameter* types remain
undetermined is rejected unless fully annotated, and an expected *return*
variable is captured from the lambda's actual type. Conflicting inferences
MUST agree exactly (`bynk.generics.type_arg_mismatch`); a type parameter
neither inferable nor given explicitly (`name[T](…)`) is rejected
(`bynk.generics.uninferable_type_arg`), as is a bare generic function passed
as a value. There is no inference between lambdas and none from the call's
own expected type. Only a record or sum *type* declaration may be generic — type
parameters on a refined or opaque body are rejected
(`bynk.generics.generic_non_record`); a
type parameter MUST NOT shadow a declared type. Within a generic function's
body its type parameters are rigid: equal only to themselves. The checker
maintains the invariant that a type-variable-bearing expected type imposes
no constraint on expression checking.

**Bounds** (constrained generics, slice 0). A type parameter MAY carry bounds,
`T: Eq` or `T: C1 + C2`, on a `fn`, a `type` declaration or — as a `where`
clause — a method (§4.3.1b). The constraints are `Eq` and, from slice 1, `Ord`;
any other name is `bynk.generics.unknown_constraint`. **`Ord` implies `Eq`**: a
`T: Ord` parameter may be compared with `==`, and `T: Eq + Ord` is the warning
`bynk.generics.redundant_eq_bound`, whose fix removes `Eq`.

A parameter that a body orders must be declared `Ord`: `<`, `<=`, `>`, `>=`,
an ordering kernel key, or passing it to an `Ord`-bounded parameter of another
generic, on a `T` without the bound is `bynk.generics.missing_ord_bound`, with a
machine-applicable fix that declares `: Ord` (replacing an existing `Eq`). Each
call MUST instantiate an `Ord`-bounded parameter with an `Ord` type
(`bynk.types.not_orderable`). A bounded generic named as a value, such as
`xs.map(top)`, is instantiated at the reference from its expected function type;
if the type arguments cannot be resolved there it is
`bynk.generics.ambiguous_instantiation`.

A bound on a generic capability operation's
type parameter is `bynk.generics.bound_on_capability_op`, and a `where` entry
that names no type parameter of the method or its receiver type is
`bynk.generics.unknown_where_param`. A bound on a type declaration is checked
where the type is applied to a written type argument: `type Set2[T: Eq]` rejects
`Set2[(Int) -> Int]` with the equality diagnostics below. An `Eq` bound adds nothing to emission. An
`Ord` bound adds one hidden trailing comparator parameter to the emitted
function (the receiver type's parameters first, then the method's own, in
declaration order), which the caller builds from the concrete type; it is
internal and not part of the published ABI.

{{#grammar-semantics if_expr}}

### §5.2a Numeric literals — digit separators (v0.142) {#numeric-literals}

> [!NOTE]
> This is a *lexical* rule (governed normatively by §3), surfaced here because it
> concerns how a numeric value is written.

An `Int` or `Float` literal MAY carry an underscore **`_`** as a **digit
separator** between digit groups — `1_048_576`, `1_000.5`, `26_214_400`. A
separator MUST fall between two digits: a leading (`_1`), trailing (`1_`), or
doubled (`1__2`) separator, or one adjacent to the decimal point or exponent
marker, is a lexical error. The separators are **purely visual** — they are
stripped before the value is parsed, so `1_000` and `1000` denote the same value.
As with `Float` literals, the **as-written lexeme is preserved**, so `bynkc fmt`
keeps the author's grouping verbatim.

## §5.3 Refinement & admission

A refinement's predicates MUST apply to the type's base — a string predicate on
an `Int` is rejected (`bynk.types.predicate_base_mismatch`) — and MUST be
internally consistent: an `InRange` MUST NOT be inverted
(`bynk.types.inverted_range`), a length MUST NOT be negative
(`bynk.types.negative_length`), a `Matches` regex MUST be valid
(`bynk.types.invalid_regex`) and MUST be safe to backtrack (below), and the
predicates together MUST admit at least one value (`bynk.types.empty_refinement`
— on `Float`, `Positive` excludes the lower endpoint `0.0`, so
`InRange(-1.0, 0.0) && Positive` is empty).

**A `Matches` pattern MUST be unambiguous enough to backtrack in linear time**
(#1651). The emitted boundary check runs the pattern under a backtracking
`RegExp` on untrusted input, so a pattern that can match the same text along many
paths is a denial-of-service risk. The compiler decides this on the pattern's
automaton:

- **Exponential ambiguity is rejected** (`bynk.types.catastrophic_regex`): a
  repetition whose body can match the same text in two ways, so each repetition
  doubles the paths. This covers nested repetition (`(a+)+`), overlapping
  alternatives (`(a|a)+`, `(\d|\d\d)+`, `(a|aa)*`), a bounded repeat inside a
  loop (`(a{1,2})+`, `(a{2,3})+`), and a bounded repeat of an ambiguous body
  (`(?:a|a){0,24}`), which is exponential in its count. Alternatives that overlap but are uniquely
  decodable are linear and accepted: `(foo|foobar)+`.
- **Polynomial ambiguity is rejected** (`bynk.types.polynomial_regex`): two
  repetitions that can divide the same text between them (`\d*\d*`), so matching
  time grows with the input's length to a power, the **degree** (the longest
  chain of repetitions that can split text this way: `\d*\d*\d*` is 3). When
  the refinement also has a `MaxLength` or `Length` bound `L` with
  `L^degree ≤ 10⁷` (3,162 characters at degree 2, 215 at degree 3), it is a
  **warning** (`bynk.types.polynomial_regex_capped`) instead: length predicates
  are checked before every `Matches`, wherever they are written, so the bound
  caps the cost. A larger bound is still an error.
- **A pattern the analysis cannot bound is rejected**
  (`bynk.types.catastrophic_regex`): a backreference under an unbounded
  quantifier, an unbounded quantifier over a body that can match the empty
  string (`(a?b?)*`), or a pattern too large or too complex to analyse.

The analysis over-approximates in the safe direction: it reads assertions and
lookarounds as matching anything (analysing each lookaround body as a pattern of
its own), and a backreference as an optional copy of its group. So it can reject
a pattern that is in fact safe, never accept one that is not.

`InRange` bounds MUST match the numeric base (v0.21): integer bounds on
`Int`, float bounds on `Float`. A bound of the other numeric type, or a
mixed pair, is `bynk.types.no_numeric_coercion`.

A **literal** written where a refined type is expected is admitted at compile
time ([§6.4](/book/spec/type-system/#64-admission--construction)) in these positions:
return (block tail), a `let` with a type annotation, an `Ok`/`Some`/`Err`
payload, and a refined-typed call argument — including a cross-context call's
(`t.pay.charge(250)`, from a consuming context or a `system` case). A literal
in a cross-context argument, passed directly or nested in a `Some`/`Ok`/`Err`
payload, a list element, or a field of a record literal of the callee's own
type, a generic one included with its type arguments substituted into the
field types (`t.pay.maybe(Some(250))`, `Page { items: [250] }` for a
`Page[Cents]` parameter), is checked against the declaration of the type at
its position in the parameter's type, in the consumed context, so the caller
need not `uses` its commons, and a type of the same name the caller declares
plays no part. A `None`, `[]`, `List.empty()` or `Map.empty()` in such an
argument takes its position's type the same way. A shorthand field
(`Page { items, size }`) is a value, not a literal: it is checked by its
binding's type, beside literal fields admitted as above. The
literal MUST satisfy the predicate, or it is rejected (`bynk.refine.literal_violates`); an admitted
literal MUST be a compile-time literal, not an expression or identifier. A refined
type is thus constructed at run time through `.of` alone and has **no** unchecked
`.unsafe` — the unchecked escape hatch is opaque-only (`Age.unsafe(x)` is rejected
as `bynk.resolve.unknown_static_member`). **Opaque
types are excluded** from admission and MUST be constructed through `.of`,
`.unsafe`, or `.raw`, never record syntax (`bynk.resolve.opaque_record_construction`); `.raw` MUST be used only within the
defining `commons` (`bynk.types.opaque_raw_outside`) and `.unsafe` only within
the defining context (`bynk.types.opaque_unsafe_outside`).

{{#grammar-semantics refined_type}}

## §5.4 Agents & state

An `agent` MUST be declared inside a context (`bynk.agent.outside_context`) and
MUST NOT declare `from http`, `from cron`, or `on message` handlers (the
`bynk.parse.*_in_agent` codes). Each agent handler's return type MUST be an
`Effect` (`bynk.agent.return_not_effect`).

Every `store` field MUST have a defined initial value: either an **explicit
initialiser** — a compile-time constant of the field's (element) type, not
referencing `self`, parameters, or capabilities (`bynk.agents.bad_state_initialiser`)
— or an **implicit zero** (`Int` → `0`, `Bool` → `false`, `String` → `""`,
`Option[T]` → `None`, a record of zeroable fields). A field with neither is
rejected (`bynk.agents.non_zeroable_state_field`).

A `:=` store write MUST target a `store Cell` field (`bynk.cell.invalid_target`),
and its right-hand side MUST NOT read the cell being written
(`bynk.cell.self_reference`) — a read-modify-write reads the old value into a
`let` first. A handler's writes are staged and committed atomically when it
returns (ADR 0109). Constructing or calling an agent MUST use the right key arity
and type and a declared handler (`bynk.agent.construction_arity`,
`bynk.agent.key_mismatch`, `bynk.agent.handler_arity`, `bynk.agent.handler_not_found`).

### §5.4.1 Invariants (v0.80)

An **invariant** is a universally-quantified property that MUST hold of every
committed agent state (`design/bynk-design-notes.md` §14; ADR 0107). Its predicate
references the agent's `store` fields by bare name and is a *pure, agent-local
`Bool` expression*:

- the predicate MUST have type `Bool` (`bynk.invariant.not_bool`);
- it MUST be pure — no capabilities, no effects, no test-only constructs
  (`bynk.invariant.impure_predicate`);
- it MUST NOT reference another agent (`bynk.invariant.cross_agent_reference`) —
  invariants constrain a single agent's reachable states; a property that spans
  agents belongs in a saga or scenario;
- invariant names MUST be distinct within an agent (`bynk.invariant.duplicate_name`).

The predicate language is ordinary expressions plus `implies` (logical
implication, `P implies Q` ≡ `!P || Q`) and `is` (pattern-matching as a `Bool`
expression). Invariants are **runtime-checked at the commit boundary**: each is
evaluated against the state staged by the handler's `store` writes, before it is
persisted. A violation is a **fault** (`InvariantViolation`), not an outcome — see
§7 and the emission model. "Revert" is the **non-persistence of the staged
state**, not whole-handler rollback (ADR 0107 D6): effects already performed by
the handler stand.

#### §5.4.1-i Step invariants — transitions (v0.116) {#step-invariants}

A **transition** is the invariant predicate widened to a *step*: a claim about the
move from the last committed state (`old`) to the state a commit would persist
(`new`) — ADR 0151. It sits in the same phase as invariants and reads state fields
through two contextual bindings, `old` and `new`, each the agent's synthetic state
record (so `old.status` / `new.balance` are ordinary record accesses); `old`/`new`
are ordinary identifiers outside a `transition`. The predicate is a *pure,
agent-local `Bool` expression* in the same predicate language:

- the predicate MUST have type `Bool` (`bynk.transition.not_bool`);
- it MUST be pure (`bynk.transition.impure_predicate`);
- it MUST NOT reference another agent (`bynk.transition.cross_agent_reference`);
- transition names MUST be distinct within an agent
  (`bynk.transition.duplicate_name`);
- it MUST reference `old` or `new` (`bynk.transition.no_step_reference`) — a
  predicate over neither is a snapshot claim, which is an `invariant`.

Placement is structural: a `transition` is an agent-body declaration only, so there
is no "transition on a non-agent" rule to state. Transitions are **runtime-checked
at the commit boundary**, alongside the snapshot invariants, but only **from the
second commit onward**: the genesis commit (an agent's first) has no `old` and is
skipped (its state is still constrained by the snapshot invariants). A violation is
the same **fault** (`InvariantViolation`), with the same non-persistence semantics.
A transition is **not** attacked by the generative runner — a valid agent state is
not necessarily reachable (ADR 0149), so behavioural generation over steps is a
handler-sequence concern (a later increment), not state fabrication.

### §5.4.1a Function contracts (v0.115)

A **contract** is the invariant predicate attached to a pure function (ADR 0150).
A `fn` carries any number of named `requires` (preconditions) and `ensures`
(postconditions) between its return type and body; each predicate is a pure `Bool`
expression in the same predicate language as an invariant (`implies`, `is`,
operators, pure methods):

- a `requires` predicate is typed with the parameters in scope; a `ensures`
  predicate is typed with the parameters **and** `result` in scope, where `result`
  has the declared return type (the awaited element `T` for an `Effect[T]` return);
- `result` MUST NOT appear in a `requires` (`bynk.contract.result_in_requires`) —
  the return value is not bound on entry;
- each predicate MUST have type `Bool` (`bynk.contract.not_bool`);
- each MUST be pure — no capabilities, effects, or test-only constructs
  (`bynk.contract.impure_predicate`);
- clause names MUST be distinct across a function's `requires` and `ensures`
  (`bynk.contract.duplicate_name`).

A contract is checked at two points, and stripped from the deploy build (the
runtime library and emission model describe the guard and the runner attack). A
`case`/`property` whose predicate is syntactically / α-equivalent to a declared
clause over the same bound arguments is flagged `bynk.contract.restated_by_test`
(conservative — under-flagging is acceptable, over-flagging is not).

### §5.4.2 Held-resource linearity (v0.102)

A **held resource** is a value of the closed `Held` kind — currently the single
type `Connection[F]` (§6.2). A held value is **runtime-produced** (no surface
constructor) and governed by an **ownership discipline** (linearity): at every
point a held binding is in one of three states — **owned**, **borrowed**, or
**consumed** — and a static *linearity pass* tracks each binding through them and
enforces (ADR 0130):

- **Disposal before scope exit.** An owned held binding MUST be **disposed** —
  stored, closed, or transferred to a handler that takes ownership — before its
  scope exits; one still owned at the end is `bynk.held.leak`.
- **No use after consume.** A *consuming* operation (`c.close()`, a `put`/`take`
  into storage, or transfer) ends the binding's lifetime; any later use is
  `bynk.held.use_after_consume`. A *non-consuming* operation (`c.send(f)`) leaves
  the binding owned.
- **Consistent disposal across branches.** Every branch of an `if`/`match` MUST
  leave a held binding in the same ownership state (`bynk.held.branch_divergence`).
- **Borrows are non-consuming.** A borrowed held binding — e.g. the closure
  parameter of a `forEach`/`parTraverse` over a `Map[K, Connection]` — admits only
  non-consuming operations; a consuming op on a borrow is
  `bynk.held.consume_on_borrow`.
- **Guards borrow.** A `match` arm's `if` guard runs before its arm is chosen, so
  it borrows every held binding it can see, the arm's own pattern bindings
  included. A consuming use inside a guard is `bynk.held.consume_on_borrow`, and
  the guard leaves every binding's ownership state unchanged.

A held value is **non-boundary and not value-comparable** (§6.5,
`bynk.types.held_at_boundary` / `bynk.types.held_not_comparable`). It may be stored
**only** in `Cell[Option[Connection]]` or `Map[K, Connection]` — a `put` consumes
the value and a `remove` removes-and-closes it — and a `Set`/`Log`/`Cache` rejects
it (`bynk.held.unsupported_storage`); a held `Map` rejects the transforming
`update`/`upsert` ops (`bynk.held.unsupported_map_op`). On an abnormal exit a
connection owned within a handler is implicitly closed by the runtime, and a stored
one rolls back with agent state (ADR 0130 Q5). Held resources are produced by the
`from websocket` protocol ([§5.7](/book/spec/static-semantics/#57-handlers)).

### §5.4.3 Rehydration validation (v0.97)

An agent's persisted state is **stored in the wire shape and decoded when it is
loaded** (ADR 0124, #1649). A commit writes each value position — a `Cell`'s `T`, a
`Map`/`Cache`'s `V`, a `Log`'s `T` — through the **same boundary serialiser** the
HTTP/queue seams use, so stored state has the documented wire form of §7.2. A load
runs each stored value position, and textual `Set` elements / `Map` keys, through
the **same boundary deserialiser**, against the **current** type definition, and
the handler reads the decoded values. A failure is an internal **fault**, `RehydrationViolation` — the
load-time twin of `InvariantViolation` (§5.4.1) — **not** a caller-facing `400`:
the supplier of stored state is trusted past-self, not an untrusted caller. A
refinement that **tightens** across a deploy therefore faults on load (orphaned
data is indistinguishable from corruption); breaking migrations remain
by-convention (no coercion, no silent drop).

**Additive evolution is automatic:** a `store` field absent from the stored record
(added in a later deploy) takes its zero/initialiser
([§5.4](/book/spec/static-semantics/#54-agents--state)) instead of reading as absent,
and is not decoded. A stored key the current definition does not declare (a renamed
or removed field) is carried through unchanged and written back, never silently
dropped. A value that has no wire form, such as a non-finite `Float`, cannot be
stored: the commit faults and nothing persists.

**Recorded state shapes.** A build also reconciles each agent class's stored shape
against the project's `bynk.schema.lock`, keyed `<context>.<Agent>`. A field's
shape is its storage kind and its value types, expanded structurally (record
fields, sum variants and variant payload fields by name; type names do not
appear, since stored bytes carry none), with and without refinements. A new class is recorded at version 1.
An unchanged shape keeps its version. Adding a field, or changing only a
refinement, increments it; the latter also warns (`bynk.agent.refinement_changed`),
since a narrowing faults on load as above. Removing a field, changing its value
types, or changing its storage kind is `bynk.agent.non_additive_state_change`,
and so is renaming one, which is a removal plus an addition. A recorded class no
longer declared is `bynk.agent.class_removed`. As for events, `bynkc check` and
`bynkc test` reconcile without writing the lock, and only a build records a new
version.

**The schema-version marker.** A build with the lock stamps every committed record
with `__v`, its class's recorded version, in the same write as the record. A load
reads `__v` from the raw stored record before merging or decoding anything. An absent
marker is version 1. A marker equal to the current version decodes as above. A
marker from the oldest version the class reads without a migration step (every later
change additive) up to the current version decodes by the same zero-merge. A marker
above the current version faults, `RehydrationViolation` with detail `NewerSchema`
(the record was written by a newer build), and any other marker, including one that
is not a positive integer, faults with `NoMigrationPath`. A faulting record is never
decoded or written back. A build without the lock emits none of this and carries a
stored marker through unchanged. A stored key the current definition does not
declare, other than `__v`, is reported once per isolate by name, never by value, and
is still carried through.

**Migration steps.** An agent may declare `migrate from N (old: T) { … }` steps
between its `store` fields and its handlers.
- **The old shape.** `T` names a record type whose fields MUST match, on structure,
  the same-named fields the lock recorded at version `N`
  (`bynk.agent.migrate_shape_mismatch`). A field of type `T` matches a `Cell` of `T`,
  and a field of type `Map[K, V]` matches a `Map` of `K, V`. A difference in
  refinements alone warns (`bynk.agent.refinement_changed`). Such a record type may
  declare field defaults.
- **The version.** `N` MUST be a recorded version, at or above the floor and below
  the current version (`bynk.agent.migrate_unknown_version`). At most one step may
  read each `N` (`bynk.agent.migrate_duplicate`).
- **Below the floor.** A step with `N` below the floor is not an error. It warns
  (`bynk.agent.migrate_below_floor`) and emits nothing, because no stored instance
  can hold version `N`. Versions are numbered contiguously from 1, so every version
  below the floor was once recorded. Only a complete `bynk migrate sweep` raises the
  floor, and raising it drops the lock's snapshots below it.
- **The body.** It holds `f := e` for a current `Cell` or `Map` field `f`, at most
  once per field (`bynk.agent.migrate_duplicate_assignment`); `let` bindings; and
  `drop f`. A bare field name reads the field's carried value. The body MUST be pure
  and MUST NOT read the agent's key (`bynk.agent.migrate_impure`).
- **Coverage.** Against the current shape, a step MUST read (`old.f`) or `drop`
  every field version `N` recorded that no longer exists. It MUST assign every field
  whose structure or storage kind changed; a `Set`, `Log` or `Cache` field may
  instead be dropped (`bynk.agent.migrate_incomplete`). A non-additive change into
  version `K` is accepted, with `additive = false`, exactly when a step from `K-1`
  passes these rules.
- **Additive predecessors.** A step from `N` also covers every version `M` at or
  above the floor whose path from `M` to `N` is entirely additive. A version from
  the floor to the current one that no step covers and that has no all-additive
  path to current warns (`bynk.agent.version_without_path`).
- **At load.** After the marker check, a record whose version a step covers runs
  the nearest such step: the step with the smallest `N` at or above the stored
  version. A step takes precedence over the zero-merge.
  - The declared old fields are decoded through `T`. A field the record lacks takes
    `T`'s field default, or else its type's zero.
  - The fields whose structure is unchanged carry over, and the body's assignments
    replace fields.
  - Keys version `N` did not record are kept.
  - The result is then zero-merged and validated as above.
  - A step that throws while running faults as `RehydrationViolation` with detail
    `MigrationFailed`, and nothing is written.

**Renaming and removing a class.** `agent New replaces Old` and the context item
`removed agent Old` declare what becomes of a recorded class's stored data.
- **What may be named.** The old name MUST NOT also be declared as an agent
  (`bynk.agent.replaces_live`, `bynk.agent.removed_live`). Each old name has at most
  one fate: renamed once, or removed (`bynk.agent.class_conflict`).
- **What the lock does.** With a lock, `replaces Old` MUST name a class the lock
  records for the same context (`bynk.agent.replaces_unknown`), unless the rename has
  already been recorded. `New` MUST NOT have a history of its own
  (`bynk.agent.class_conflict`). The lock entry `<context>.Old` moves to
  `<context>.New` with its versions and shapes. A removal drops its entry.
- **What the Worker gets.** The Worker's `exports` gain a `renamed` tombstone naming
  `New`, or a `deleted` one, and the old class gets no binding.
- **Everything else is refused.** A recorded class that disappears with neither
  declaration is `bynk.agent.class_removed`.

## §5.5 Effects, capabilities & providers

Bynk separates **pure** from **effectful** code. An `<-` bind MUST occur in an
effectful position and MUST be applied to an `Effect`
(`bynk.effect.bind_in_pure_context`, `bynk.effect.bind_on_non_effect`); a
capability call or a cross-context call MUST NOT occur in a pure context
(`bynk.effect.capability_in_pure_context`, `bynk.effect.cross_context_in_pure_context`).

An **asynchronous send** (`~>`, §4.8.5) MUST likewise occur in an effectful
position (`bynk.send.in_pure_context`) and MUST be applied to an `Effect`
(`bynk.send.non_effect`). Because a send does not await its reply and binds
nothing, its reply MUST be `Effect[()]` — the **error gate**: a send whose
operation returns a non-unit `Effect[T]` is rejected (`bynk.send.requires_unit`),
since the value or error `T` would be silently discarded. "No value" and "no
need to wait" are independent: to *await* a unit-returning effect (a durable
write that must join the commit) keep the `<-` bind; to await and discard a
**valued** reply, write `let _ <- e`. A send is a statement, never an expression.

A send MUST NOT occur in an **agent handler** body, a closure within one included
(`bynk.send.in_agent_handler`): an agent handler runs inside the agent's Durable
Object and has no execution context to keep a fire-and-forget effect alive past
it, so its effects are awaited (`let _ <- e`, `do e`). A send of the agent's own
**store operation** is refused with its own code (`bynk.send.store_op`): a store
write is staged and committed atomically at the handler boundary, so there is
nothing to send without waiting for — write it as `do field.op(…)`.

A **`do` statement** (`do e`, §4.8.6) performs a unit effect without a binder: it
MUST occur in an effectful position (`bynk.effect.do_in_pure_context`) and MUST
be applied to an `Effect[()]` (`bynk.effect.do_on_non_effect`,
`bynk.effect.do_requires_unit`). Unlike a send, the effect *is* awaited and joins
the enclosing computation — `do e` is exactly `let _ <- e` when the awaited value
is unit. The unit gate is the send's error gate transplanted: a **valued** reply
(`Effect[T]`, `T ≠ ()`) is rejected so that discarding a real value stays the
explicit `let _ <- e`.

A **`par` block** (`par { … }`, §4.8.7b; #2640) joins independent effects: it
issues every effect it holds, in source order, before awaiting any of them, and
completes when all have settled. It MUST occur in an effectful position
(`bynk.effect.bind_in_pure_context`) and holds only `let x <- e` (optionally typed)
and `do e`; each binding is checked exactly as the same statement written outside
the block, against the scope *outside* it. Anything else is
`bynk.par.invalid_statement`, naming the statement: a `let` with a refutable
pattern and an `else`, a plain `let` (it has no effect to overlap), `~>` (a
fire-and-forget send has nothing to join), `:=` or a mutating `store` operation
(`store` writes stay sequential), `expect`, `advance`, a nested `par`, and an
empty block. A `?` in a binding is `bynk.par.question_in_join` (a `?` inside a
lambda belongs to the lambda); propagating the first `Err` in source order across a
join is a later increment. The bindings run together, so **no binding may mention
a name another binding of the block binds, and no two bind one name**
(`bynk.par.sibling_name`) — even where the name would also resolve to a binding from
before the block, so a stale value is never picked up silently. A binding's own name
is not a sibling: `let n <- F(n + 1)` reads the earlier `n`. The bound names are in
scope after the closing brace, and re-binding an earlier name follows §5.1. A block
with one binding is `bynk.par.single_binding`, a non-failing warning (ADR 0117). The
block has no value; it is a statement.

*Faults.* If any effect of the block faults, the block still waits for every other
to settle, then raises the **first fault in source order**, whichever faulted first
in time; later faults are attached to it for the log and cannot be caught
separately. Nothing the block started is still running when it completes or
raises, so no work outlives the handler, and the fault a program sees does not
depend on timing. Effects that completed before the fault keep their effects
(ADR 0109 D4: a cross-agent `<-` call commits as it returns; `store` writes are
staged and discarded; `~>` sends stand). A `par` block is legal in an agent handler:
an input gate that is open while a handler awaits a subrequest is open for a join
too, so a join adds no interleaving a sequence of `<-`s does not already allow —
the one thing it removes is the synchronous gap between two adjacent lines, which
nothing inside the block can rely on, because no binding can read another's result.

**Implicit unit tail (§4.8.1).** A block with no trailing expression has value
`()`: the tail auto-lift (an `Effect[T]` context lifts a tail of type `T`) then
lifts it to `Effect[()]` where the enclosing return type is `Effect[()]`, so an
effectful unit body may simply *end* rather than close with `Effect.pure(())`.
Against a **valued** expected type the synthesised `()` is an ordinary type
mismatch, reported where any wrong-typed tail would be.

**Else-less `if` (§4.6.3).** An `if` with no `else` defaults the missing branch
to `()`. It is legal only when the then-branch is unit (`()` or `Effect[()]`);
otherwise the missing value has no default and it is rejected
(`bynk.types.if_without_else_requires_unit`). A value-producing `if` still
requires an explicit `else`, and the "both branches agree" rule (§5) is
unchanged — the synthesised `()` branch simply matches the unit then-branch.

A capability MUST be declared inside a context or an adapter
(`bynk.capability.outside_context`); a bodied provider MUST implement exactly its
capability's operations — no missing, no extra, signatures matching
(`bynk.provider.missing_operation`, `bynk.provider.extra_operation`,
`bynk.provider.signature_mismatch`) — and every provider MUST name an existing
capability (`bynk.provider.unknown_capability`). A handler or provider MUST
declare every capability it uses with `given`, and `given` MUST name a real
capability; a call to an undeclared capability is rejected and an unused one
warned (`bynk.given.unknown_capability`, `bynk.given.undeclared_capability`,
`bynk.given.unused_capability`). A provider whose body calls an agent handler
MUST also list in its `given` every capability that handler is `given`, and,
transitively, every capability `given` to the agent handlers it calls
(`bynk.provider.agent_call_undeclared_capability`). A service or agent handler
is exempt: its deps are widened to cover the agent handlers it calls. Providers MUST NOT form a dependency cycle
through `given` (`bynk.provider.dependency_cycle`).

**Derived capabilities** (standard-library S4a, #1966). `capability X = K`
declares a capability `X` whose operations are those of `K`, as a **distinct
instance** of `K`, not an alias: two derivations of `Kv` are two namespaces,
just as `type Id = Int` is a distinct type over `Int` (§6.1.2). No `provides`
is written. `X` is provided by `K`'s platform provider, bound to `X`'s own
resource. Otherwise `X` is an ordinary capability: `given X`, `X.op(…)`, `stub`
and `expect` apply to it unchanged. The rules:
- `K` MUST be a capability the unit selects with `consumes U { K }`
  (`bynk.capability.unknown_base`).
- `K` MUST be a platform resource kind with instances. `bynk.cloudflare`'s
  `Kv`, `Service`, `R2`, `RateLimit`, `Queue`, `Analytics` and `EdgeCache` have them
  (`bynk.capability.base_not_instantiable`).
- Each `@annotation` MUST be one of `K`'s instance options
  (`bynk.capability.unknown_option`), given at most once, with one positional
  literal of the option's kind (`bynk.capability.invalid_option`): a string for
  `@worker`, `@jurisdiction` and `@name`, a positive `Int` for `@limit`, a
  `Duration` for `@period`. Every option `K` requires MUST be present
  (`bynk.capability.missing_option`). `Kv` takes none. `R2` takes an optional
  `@jurisdiction("eu" | "fedramp")`. `RateLimit` requires `@limit(n)` and
  `@period(d)`, where `d` is exactly 10 or 60 seconds (standard-library P3).
  `Service` requires `@worker("name")`, where `name` is 1 to 63 lowercase
  letters, digits, `-` or `_`, not starting or ending with `-`, and not the
  Worker of a context in the project (`bynk.capability.invalid_option`): the
  Worker outside the project the instance reaches (interop I5, #1972).
  `Queue` requires `@name("queue")`, where `queue` is 1 to 63 lowercase letters,
  digits or `-`, starting and ending with a letter or digit
  (`bynk.capability.invalid_option`): the queue the instance sends to
  (standard-library P2, #2578). `Analytics` takes none (standard-library P3b,
  #2582).
- A kind used only through instances MUST NOT be named bare in a `given`
  clause (`bynk.capability.instance_only`). `Service`, `R2`, `RateLimit`,
  `Queue`, `Analytics` and `EdgeCache` are such kinds.
- A context MUST declare at most one `EdgeCache` instance
  (`bynk.cloudflare.edge_cache_duplicate`). The instance has no binding: it
  makes the Worker's `wrangler.toml` carry `[cache] enabled = true`. An agent
  handler MUST NOT reach it through its `given` clause, directly or through
  the `given` of a provider of the context, at any depth
  (`bynk.cloudflare.edge_cache_in_agent`). Nor may a queue consumer's
  handler (`bynk.cloudflare.edge_cache_in_queue`), an `on schedule` handler
  (`bynk.cloudflare.edge_cache_in_schedule`), or an exported capability
  through its provider (`bynk.cloudflare.edge_cache_exported`). In such a
  context, a `@cache(scope: public)` route's `by` MUST name only open actors
  (`bynk.cloudflare.edge_cache_public_authenticated`). A purge reaches the
  cache only from the Worker's `fetch` (L7, #2414).
- `X` MUST be declared in a context (`bynk.capability.derived_outside_context`).
  An instance belongs to the context that owns its data.
- `X` MUST NOT be exported (`bynk.exports.derived_capability`). Another context
  reaches the data through this context's services.
- `X` shares the unit's capability namespace and its conflict rules.
- On Workers, `X`'s binding is `K`'s prefix and `X` in screaming snake case
  (`Sessions` → `KV_SESSIONS`, `Billing` → `SERVICE_BILLING`). It MUST NOT
  clash with another of the Worker's bindings
  (`bynk.context.duplicate_binding`).

A write to an instance commits as it returns and is not rolled back on a fault,
like any platform-resource write (ADR 0109 D4).

Calling an **effectful function value** — one whose type's return is
`Effect[_]` — is an effect operation: it MUST occur in an effectful context
(`bynk.effect.fn_value_in_pure_context`), exactly as a capability call must.
`Effect[T]` remains non-storable in pure contexts; this feature opens no back
door (the eager-`Promise` translation makes an un-bound effectful call
observable, so the confinement is load-bearing).

**An `Effect` value in an effectful body MUST be awaited in order** (#1658).
The eager-`Promise` translation starts an effectful call as soon as it is
built, so a call that is built and not bound runs unawaited, racing everything
after it. An `Effect[T]` value MUST therefore be:
- bound with `<-`;
- sequenced with `do`;
- returned as the body's value; or
- passed as an argument, whose parameter decides what happens to it (a
  parameter typed `Effect[T]` binds it).

Building one in a value position is `bynk.effect.unbound_effect`. That covers:
- the right-hand side of a plain `let` (including `let _ = …` and an
  `Effect`-annotated `let`);
- a list literal's element;
- the payload of `Some`/`Ok`/`Err`, a sum-variant constructor, or a record
  field.

**Provider placement follows the unit kind.** A provider in a *context* MUST
have a Bynk body (`bynk.context.external_provider`); a provider in an *adapter*
MUST be external — bodiless, its implementation supplied by the binding
(`bynk.adapter.provider_has_body`); a provider anywhere else is rejected
(`bynk.provider.outside_context`). An **external** provider's `given` resolves
exactly as a bodied provider's does — each bare name MUST be a local capability
or one flattened from a `consumes` selection
([§5.8](#58-boundaries--cross-context)) — and external providers participate in
the same dependency-cycle check.

{{#grammar-semantics given_clause}}

**A capability operation's own type parameter (v0.235, ADR 0281).** A
`capability_op`'s optional `[T, …]` is resolved only from an explicit type
argument at the call site (`Cap.op[SomeType](…)`) — never inferred from the
arguments or the expected type. Omitting it where no parameter's type mentions
`T` is rejected (`bynk.generics.uninferable_type_arg`); the wrong number of
type arguments is `bynk.generics.type_arg_mismatch`. A capability with a
generic operation MUST be implemented by an **external** provider
(`bynk.provider.generic_op_requires_external`) — a Bynk-bodied provider's `T`
has no expressive use, since its body can only return a value it already has —
so a generic operation is declarable only inside an `adapter`, whose providers
are always external, never a plain `context`. A generic operation MUST NOT be
named in a test `stub` clause (`bynk.stub.generic_op`): a stub body has no way
to construct a value of an unconstrained `T`; stubbing a different, non-generic
operation of the same capability is unaffected.

## §5.6 Pattern matching

A `match` MUST be **exhaustive** — every variant of the scrutinised sum,
`Result`, or `Option` covered (`bynk.types.non_exhaustive_match`) — and its
scrutinee MUST be a sum type, a primitive, or a record matched by record
patterns (`bynk.types.match_non_sum_discriminant`). Its arms
MUST **join** to a common result type — their least upper bound, so a refined
type and its base agree at the base (`bynk.types.match_arm_mismatch`), MUST NOT
repeat a
variant (`bynk.types.duplicate_variant_arm`), and MUST NOT be unreachable
(`bynk.types.unreachable_arm`): every arm MUST match some value that no earlier
unguarded arm matches. A guarded arm or a refined pattern (`n where P`) never
covers a later arm, since its guard or predicate may fail.

**Extensible enums** (standard-library S3, #1963). A standard-library enum
marked extensible in the first-party registry (`FetchError`, `Method`) may gain
variants in a minor release, so its coverage counts one more shape than its
declared variants: a future variant that only an irrefutable pattern (`_` or a
binding) covers. A `match` that covers an extensible enum at any depth
(`Err(Network)` … inside `Result`, or at top level) without such a pattern is
`bynk.types.extensible_match_needs_wildcard`. Once the wildcard is present it
covers the future variant, so it is not `bynk.types.unreachable_arm`. The enum
is identified by the unit that provides it, not by its name, so a user's own
enum of the same name is unaffected. Suite bodies are not checked under this
rule: a test is not under the compatibility promise, and its views lack the
unit's provenance.

**Record patterns** (patterns track, slice 1, #2641, [§4.7.11](/book/spec/syntactic-grammar/#4711-record_pattern)).
In `T { f, g: p, .. }`:

- `T` MUST be a record type — not a sum, enum, refined or opaque type, or a
  built-in (`bynk.types.record_pattern_on_non_record`) — and the matched value
  MUST be of type `T` (`bynk.types.pattern_type_mismatch`). For a generic record
  the type arguments are the scrutinee's.
- Each listed field MUST exist (`bynk.types.unknown_field`) and appear at most
  once (`bynk.types.duplicate_pattern_field`). A bare `f` binds a local `f` of
  the field's type; in `f: p`, `p` is checked against the field's type. Under an
  or-pattern the alternatives MUST bind the same names at the same types, as
  for any pattern.
- A record pattern is subject to the visibility rules of a field read: it MUST
  NOT name a type the unit may not inspect, such as another context's
  `exports opaque` type (`bynk.context.opaque_inspection`, reported at the
  pattern).
- **Coverage is conservative.** A record pattern covers its type only when every
  field sub-pattern is irrefutable (a name or `_`); otherwise it is partial,
  and a later irrefutable arm is needed for the `match` to be exhaustive
  (`bynk.types.non_exhaustive_match`). An arm is `bynk.types.unreachable_arm`
  when an earlier unguarded arm's record pattern is no more specific on any
  field. Precise multi-field coverage is a later increment.
- After `is`, the names bound by a record pattern in scope are its flat fields:
  bare `f` and `f: name`, like a variant's flat payload bindings.

A pattern MUST name a real variant (`bynk.types.unknown_variant_in_pattern`) and
real payload fields (`bynk.types.unknown_pattern_field`), bind the right number
of fields (`bynk.types.pattern_arity`), and MUST NOT mix named and positional
bindings (`bynk.types.mixed_pattern_bindings`). An `is` check MUST be applied to a
value of the matching base or sum (`bynk.types.is_base_mismatch`,
`bynk.types.is_non_sum`, `bynk.types.is_unknown_variant`).

**Where an `is` binding is in scope** (#1654, type-system §2.3.6). The names an
`e is P` test binds are in scope wherever the test is known to have matched.
Which tests a Boolean expression proves follows its structure:
- `e is P` proves itself when true;
- `a && b` proves both operands' tests when true;
- `a || b` proves both operands' negated tests when false;
- `!e` swaps true and false;
- `a implies b` (`!a || b`) proves `a`'s tests and `b`'s negated tests when
  false.

So the bindings are in scope in:
- an `if`'s then-branch, for what its condition proves when true;
- its else-branch, for what the condition proves when false
  (`if !(o is Some(v)) { 0 } else { v }`);
- the right operand of `&&` and `implies`, for what the left operand proves
  when true, since the right is evaluated only then
  (`o is Some(v) && v > 0`, `(state is Held(g, r)) && useGuestRoom(g, r)`).

They are never in scope in the right operand of `||` or past the `if`
(`bynk.resolve.unknown_name`).

{{#grammar-semantics match_expr}}

**`let … else`** (#2010, [§4.8.4a](/book/spec/syntactic-grammar/#484a-let_else_stmt-2010)).
A `let` whose left side is a pattern, followed by the statements `S…` and the
tail `t` of its block, means exactly

```bynk,ignore
let v <- e               // or `let v = e`
match v { P => { S… t }, _ => E }
```

for `let P <- e else E`, where `v` is a name the program cannot write; for
`let P <- e else match { arms }` the arms replace `_ => E`. So:

- `E` is the value of the **enclosing block** when `e` does not match `P`,
  not of the enclosing function. It MUST have the type the rest of the block
  produces (`bynk.types.match_arm_mismatch`). A `?` inside `E` still returns
  from the function, as a `?` anywhere does.
- `P`'s names are in scope from the end of the statement to the end of the
  block, like a `let` binding's.
- The `else match` arms MUST cover the values `P` does not
  (`bynk.types.non_exhaustive_match`) and MUST NOT be unreachable, which
  includes an arm `P` already covers (`bynk.types.unreachable_arm`,
  `bynk.types.duplicate_variant_arm`).
- If `P` matches every value of `e`'s type, the `else` can never run, which is
  `bynk.types.unreachable_arm`. Bind the value with a plain `let` instead.
- `<-` follows the `effect_let_stmt` rules (§5.5): the body MUST be effectful,
  and `P` matches the `Effect`'s payload.

Each statement nests the rest of its block one level, as the `match` it stands
for does, so a single block holds at most as many steps as the nesting limit
allows (`bynk.parse.nesting_too_deep`).

{{#grammar-semantics let_else_stmt}}

## §5.7 Handlers

A `service` MUST be declared inside a context (`bynk.service.outside_context`) and
every service handler MUST return an `Effect` (`bynk.service.return_not_effect`).

An HTTP handler MUST return `Effect[HttpResult[T]]`
(`bynk.http.return_not_effect_http_result`); its route MUST be well-formed and
unique, MUST NOT use the reserved `/_bynk/` prefix
(`bynk.http.invalid_path`, `bynk.http.duplicate_route`, `bynk.http.reserved_prefix`),
and each `:name` segment MUST bind to a string-constructible parameter
(`bynk.http.unbound_path_param`, `bynk.http.path_param_not_stringy`,
`bynk.http.extra_param`); `GET` and `DELETE` MUST NOT take a `body`
(`bynk.http.body_on_get_or_delete`). Constructing a `Raw` variant (v0.111) MUST
supply exactly two arguments — a `Bytes` body then a `String` content-type
(`bynk.types.variant_arity`, `bynk.types.argument_mismatch`) — and yields
`HttpResult[()]`, the JSON body parameter `T` being unused, as for `Streaming`
and the redirect variants. Constructing `RawCreated` (the `201` variant, #2403)
MUST supply the same two arguments, with the same codes and result. Constructing
`RawStreamed` (#2623) MUST supply exactly two — a `Stream[Bytes]` body then a
`String` content-type — with the same codes and result. Constructing
`RawTagged` MUST supply exactly three — a
`Bytes` body, a `String` content-type, then a `String` entity tag — with the same
codes and result; the tag's syntax is checked at run time
([§7.4.3](/book/spec/runtime-library/#743-httpresult)), not here. An cron handler MUST take at most one
`Int` parameter, a valid five-field schedule, and return `Effect[Result[(), E]]`
(the `bynk.cron.*` codes); an `on message` handler MUST take exactly one `message`
parameter, a Cloudflare-valid queue name, and the same return shape (the `bynk.queue.*`
codes).

### §5.7.1 CORS policy (v0.131) {#cors}

A `from http` service MAY declare a single **`cors { }`** policy in header position
(before the handlers). It is legal only on a `from http` service
(`bynk.http.cors_not_http`) and at most once per service
(`bynk.parse.duplicate_cors`). The grammar accepts any `name: value` field; the
checker enforces the closed field set — an unknown field is
`bynk.http.cors_unknown_field`. The fields:

- **`origins`** — REQUIRED; a non-empty list of string literals
  (`bynk.http.cors_invalid_origins`). The allowlist of origins, or the single
  wildcard `["*"]`.
- **`headers`** — OPTIONAL; a list of string literals
  (`bynk.http.cors_invalid_field`). Overrides the default `Access-Control-Allow-Headers`.
- **`credentials`** — OPTIONAL; a boolean literal (`bynk.http.cors_invalid_field`).
- **`maxAge`** — OPTIONAL; a `Duration` literal (`bynk.http.cors_invalid_field`).

`credentials: true` combined with a wildcard origin (`["*"]`) is rejected
(`bynk.http.cors_wildcard_credentials`) — the Fetch standard forbids that pair.
`Access-Control-Allow-Methods` is **not** a field: it is derived from the service's
declared route methods (plus `HEAD` where `GET` is declared, plus `OPTIONS`) — the
same derivation that drives the always-on `Allow` header. A service with no
`cors { }` policy emits no `Access-Control-*` headers (and a cross-origin preflight
carries no CORS grant, so the browser blocks the read); it still answers the
always-on method contract — a plain `OPTIONS` is a `204 + Allow`, a wrong method a
`405 + Allow`, and `HEAD` mirrors `GET`. See [§7 emission](/book/spec/emission/)
for the synthesised preflight, the method-contract fall-through, and header
stamping, whose ordering (the preflight and the `405`/`OPTIONS` answers precede the
handler authentication seam) is normative.

### §5.7.2 Handler caching annotation (v0.140) {#cache}

A `GET` handler MAY carry a single **`@cache`** annotation in handler position
(immediately before `on`). It is the first handler-position annotation; the grammar
admits any `@name(args)` before a handler, so a dangling annotation with no
following `on` is a parse error (`bynk.parse.dangling_handler_annotation`) and any
name outside the closed set is `bynk.http.unknown_handler_annotation`. `@cache` is
legal **only on an `on http GET` handler** — on any other method, protocol, or an
agent handler it is `bynk.http.cache_on_non_get` — and at most once per handler
(`bynk.http.cache_duplicate`). Its arguments:

- **`maxAge`** — REQUIRED; a positive `Duration` literal resolving to a whole
  number of seconds (`bynk.http.cache_bad_max_age` if not positive;
  `bynk.http.cache_max_age_fractional_seconds` if positive but not an exact
  multiple of one second — `Cache-Control: max-age` has no finer granularity,
  so a fractional-second value would otherwise silently drop the remainder
  rather than honour it). The freshness window, lowered to `Cache-Control:
  max-age` in whole seconds.
- **`scope`** — OPTIONAL; the bare identifier `public` or `private`
  (`bynk.http.cache_bad_scope`), defaulting to `private`.
- **`immutable`** — OPTIONAL; a `Bool` literal (`bynk.http.cache_bad_immutable`),
  defaulting to `false`. `true` appends the RFC 8246 `immutable` directive to the
  `Cache-Control`.

Any other argument is `bynk.http.cache_unknown_arg`. The annotation governs only
the opt-in **freshness** half; the conditional **`ETag`/`304`** half is automatic
for every eligible `GET` (one returning `Ok`, or a `Raw` body of at most 1 MiB)
and carries no author surface; `RawTagged` carries the handler's own validator.
Because eligibility is a value-level property — the returned variant, not the
`Effect[HttpResult[T]]` return type — the runtime attaches the validator only to
those responses. `@cache`'s `Cache-Control` applies to an `Ok`, `Raw` or
`RawTagged` response; on a `GET` that returns only `Streaming` it is a harmless
no-op for the stream (a static diagnostic for that case is a named follow-on). See [§7.4.3](/book/spec/runtime-library/#743-httpresult)
for the runtime lowering and the normative composition order.

### §5.7.3 Security-headers policy (v0.141) {#security}

A `from http` service MAY declare a single **`security { }`** policy in header
position (beside `cors { }`). It is legal only on a `from http` service
(`bynk.http.security_not_http`) and at most once per service
(`bynk.parse.duplicate_security`). The grammar accepts any `name: value` field; the
checker enforces the closed field set — an unknown field is
`bynk.http.security_unknown_field`. The fields:

- **`nosniff`** — OPTIONAL; a boolean literal (`bynk.http.security_invalid_field`).
  Whether `X-Content-Type-Options: nosniff` is stamped. Defaults to **`true`**.
- **`hsts`** — OPTIONAL; a **positive** `Duration` literal
  (`bynk.http.security_invalid_field`). Opts in to `Strict-Transport-Security`,
  lowered to `max-age` in whole seconds. Absent ⇒ no HSTS.

Unlike `cors`, the safe header is **on by default**: a `from http` service with no
`security { }` policy (or one that omits `nosniff`) still stamps
`X-Content-Type-Options: nosniff` on every response — the compiler synthesises a
default policy (`nosniff: true`, no HSTS) for **every** `from http` service. Only an
explicit `nosniff: false` suppresses it. The headers are stamped uniformly across
every response family and the synthesised preflight, `405`/`OPTIONS`, and `304`,
composing with the CORS headers (the two sets are disjoint, so the stamping order is
not observable). `Content-Security-Policy` and `X-Frame-Options` are **not** emitted
— they constrain markup, which the Bynk HTTP surface does not serve. See
[§7.4.3](/book/spec/runtime-library/#743-httpresult) for the runtime lowering.

### §5.7.4 Request body-size limits (v0.142) {#body-limits}

A `from http` service MAY declare a single **`limits { }`** section in header
position (beside `cors { }` and `security { }`), and a body-taking handler MAY
carry a single **`@limit`** annotation that overrides it. Both bound the size of a
request body, in bytes.

The **`limits { }`** section is legal only on a `from http` service
(`bynk.http.limits_not_http`) and at most once per service
(`bynk.parse.duplicate_limits`). The grammar accepts any `name: value` field; the
checker enforces the closed field set:

- **`maxBody`** — the only field; a positive `Int` byte count
  (`bynk.http.limits_invalid_field`). Any other field is
  `bynk.http.limits_unknown_field`.

The **`@limit`** annotation is a handler-position annotation (the `@cache`
position, ADR 0111). It is legal **only on an `on http POST`/`PUT`/`PATCH`
handler** — a body-taking method — and on a `GET`/`DELETE` (or any bodyless
method) it is `bynk.http.limit_on_bodyless`; at most once per handler
(`bynk.http.limit_duplicate`). Its arguments:

- **`maxBody`** — the only argument; a positive `Int` byte count
  (`bynk.http.limit_bad_max_body`). Any other argument is
  `bynk.http.limit_unknown_arg`.

**Effective cap and precedence.** For a body-taking route the effective cap is the
route `@limit` `maxBody` if present, otherwise the service `limits { maxBody }` if
present, otherwise **none**. A route with no effective cap reads the body
unchanged, so this surface is **opt-in** (the CORS posture, not the security
default-on posture): a service declaring neither `limits { }` nor any `@limit` is
byte-for-byte unchanged.

**Enforcement.** A capped route rejects a request whose `Content-Length` exceeds
the effective cap with a synthesised **`413 PayloadTooLarge`** (`{ kind:
"PayloadTooLarge", details: … }`), produced **before the body is read** and
**before the `by`/Bearer auth seam** — the boundary posture of the method-semantics
`405` ([§5.7](#57-handlers)). It reuses the existing `413` status; the closed
`HttpResult` registry is **unchanged**. The `413` is stamped with the CORS and
security headers so a cross-origin caller can read it. Because it keys on
`Content-Length` — which may be absent (chunked) or spoofed — this is a fast-reject,
not a hard guarantee on its own. A capped body is therefore also read through a
counting reader: the bytes actually read are counted against the cap, and the read
is cancelled with the same `413` as soon as it crosses it, so a chunked body, or one
whose `Content-Length` understates it, cannot exceed the cap. This covers a JSON
body, a `@raw` body and a multi-actor route (whose wrapper returns `PayloadTooLarge`).
`maxBody` is an `Int` byte count in this version; a byte `Size` literal is a named
follow-on.

**`@raw`.** A third handler annotation, `@raw(contentType: "<type>/<subtype>")` (or
`"*/*"`), marks a handler's `body: Bytes` as the raw request body. It is legal only
on a `POST`/`PUT`/`PATCH` handler with a `body` (`bynk.http.raw_on_bodyless`), at
most once (`bynk.http.raw_duplicate`), with the one argument `contentType`
(`bynk.http.raw_unknown_arg`) a media type (`bynk.http.raw_bad_content_type`). The
`body` MUST be `Bytes` (`bynk.http.raw_body_not_bytes`) and the route MUST have an
effective cap (`bynk.http.raw_body_unbounded`). The router answers a missing or
different `Content-Type` (parameters stripped, compared case-insensitively) with a
synthesised **`415 UnsupportedMediaType`** (`{ kind: "UnsupportedMediaType",
details: … }`) ahead of the auth seam and the body read, reads the body under the
cap, and passes the octets to the handler undecoded. A `Signature` actor verifies
over those octets. A `Bytes` body without `@raw` keeps its base64-in-JSON form. See
[§7.4.3](/book/spec/runtime-library/#743-httpresult) for the runtime lowering and
its position in the dispatch order.

A **`from websocket(in: I, out: O)`** service (v0.103) binds the inbound frame
type `I` and the server-sent frame type `O` on its header, and declares the
connection-lifecycle handlers:

- **`on open`** — exactly one per service. The upgrade is authenticated **at the
  edge**: like a `from http` handler it MUST name its actor with `by` (there is no
  anonymous upgrade), and the boundary admits `None`/`Bearer` actors and rejects
  `Signature` (a browser cannot set an `Authorization` header). The handler
  receives a fresh, **owned** `connection: Connection[O]` that it MUST dispose of
  under the held-resource linearity discipline
  ([§5.4.2](/book/spec/static-semantics/#542-held-resource-linearity-v0102)) — the canonical
  disposal being transfer into an agent. On the Workers target that transfer MUST
  resolve to exactly one routable agent (`bynk.ws.open_transfer_shape`).
- **`on message`** (v0.106) — an inbound frame arrived; its parameters are the
  route params plus the decoded frame of type `I`.
- **`on close`** (v0.106) — the connection ended; the stored connection is
  disposed (typically via the owning agent).

Inbound frames are dispatched to the agent that owns the connection, so the
service holds **exactly one** `on open` and broadcasts by iterating a held
`Map[K, Connection]` ([§5.4.2](/book/spec/static-semantics/#542-held-resource-linearity-v0102)).

{{#grammar-semantics http_handler}}

## §5.7a Actors & the `by` clause (v0.45)

An `actor` MUST be declared inside a context (`bynk.actor.outside_context`). Its
`auth` scheme MUST be compiler-known (`bynk.actor.unknown_scheme`) — `None`,
`Internal`, `Bearer` (v0.47), and `Signature` (v0.51) are supported. A `Bearer`
actor MUST name its signing secret (`auth = Bearer(secret = "<ENV>")`,
`bynk.actor.bearer_missing_secret`) and MUST declare a string-constructible
`identity` — a refined or opaque `String`, minted from the JWT `sub` claim
(`bynk.actor.bearer_identity_not_string_constructible`); `Bearer` is admissible
only on `from http` handlers. A `Signature` actor (HMAC over the request body)
MUST name its secret (`bynk.actor.signature_missing_secret`) and its signature
`header` (`bynk.actor.signature_missing_header`); a `tolerance` requires a
`timestamp` header (`bynk.actor.signature_tolerance_without_timestamp`); a
`Signature` actor takes **no** `identity` — the signature attests authenticity,
not a principal (`bynk.actor.signature_identity_unsupported`) — and is admissible
only on `from http` handlers. An **`Oidc`** actor (v0.151) verifies an
asymmetrically-signed (RS256/ES256) JWT against a provider's published JWKS. It
MUST name its `issuer` (`bynk.actor.oidc_missing_issuer`), its `audience`
(`bynk.actor.oidc_missing_audience`), and its `jwks` endpoint URL
(`bynk.actor.oidc_missing_jwks`) — public trust parameters, **no secret** — and
MUST declare a string-constructible `identity`, minted from the verified `sub`
claim (`bynk.actor.oidc_identity_not_string_constructible`); `Oidc` is admissible
only on `from http` handlers and, this slice, only as a **single** actor — never a
sum member (`bynk.actor.oidc_not_in_sum`) and not a refinement base. A declared
`identity = T` MUST be a context-ownable
value type, so the verified identity is sealed — minted only inside the owning
context (`bynk.actor.identity_not_sealed`). A **`Foreign`** actor (#1971) is a
caller that is not Bynk, declared by the context it calls. It MUST name the
secret its calls are signed with (`auth = Foreign(secret = "<ENV>")`,
`bynk.actor.foreign_missing_secret`) as a string
(`bynk.actor.foreign_secret_not_string`). No other actor in the context MAY
name the same secret (`bynk.actor.foreign_duplicate_secret`), and it MUST NOT begin with an edge key's
prefix, `BYNK_CALL_FROM_` or `BYNK_CALL_TO_` (`bynk.actor.foreign_reserved_secret`);
it MAY name the foreign Worker (`worker = "<name>"`). Its name MUST NOT be one a
context could have (`bynk.actor.foreign_name_is_context_name`): it is what the
caller sends as `X-Bynk-Caller`, so it must never select a context's edge key.
It takes **no** `identity` (`bynk.actor.foreign_identity_unsupported`), and is
admissible only on `on call` handlers.

> **Who, not whose.** An actor contract answers *who* is at the boundary — it
> authenticates a party and seals its identity. It deliberately does **not**
> answer *whose* a given object is: object-level authorisation (may *this* user
> read *this* record?) is domain logic and lives in the handler body, by design.
> The `where`-clause authorisation invariants below narrow *who* (a claim the
> party carries), never *whose*.

The **refinement form** `actor Admin = User where <predicate>` (v0.53) declares
an **authorisation invariant**: an `Admin` is a `User` who additionally satisfies
the predicate. Its base MUST be a declared `Bearer` actor — only `Bearer` carries
claims to authorise against (`bynk.actor.refinement_base_unsupported`) — and its
`where` predicate MUST be in the closed claim-predicate set: `hasClaim("name")`
(the claim is present and truthy) and `claimEquals("name", "value")` (string
equality), composed with `&&`, `||`, `!` (`bynk.actor.refinement_predicate_unsupported`).
A refinement actor is a handler's sole `by` contract, never a sum member
(`bynk.actor.refinement_in_sum`, §5.7a.1). By refinement elimination an `Admin`
is usable wherever its base `User` is: a `by a: Admin` binder yields the base
`User` identity. The invariant is discharged at the boundary (§7.3.4a): the scheme
is verified (failure → 401), then the predicate is checked against the verified
claims (failure → **403**, distinct from 401), then the identity is minted and the
body runs.

A handler consumes an actor on its `by (<binder>:)? <Actor>` clause. The named
actor MUST resolve to a declared actor or a prelude actor (`Visitor`,
`Scheduler`, `Producer`, `Caller`) (`bynk.actor.unknown_actor`), and its scheme
MUST be admissible on the handler's protocol — HTTP admits `None`, `Bearer`,
`Signature`, and `Oidc`; the internal protocols (call/cron/queue) admit `Internal`,
and `call` admits `Foreign` too (`bynk.actor.scheme_not_admissible`). In a sum,
`Foreign` peers are exempt from the one-peer-per-scheme rule, since each is told
apart by name. A `Signature` handler MUST take a `body`
parameter — the signature is computed over the request body, so a bodyless signed
request is meaningless (`bynk.actor.signature_requires_body`). A handler that
omits `by` inherits its protocol's default actor; an **HTTP handler has no safe
default and MUST declare `by`** (`bynk.actor.missing_by_on_http`).

The `Caller` prelude actor (the `on call` default) yields a **live `CallerId`**
(v0.54): a cross-context `on call … by c: Caller (…)` handler binds `c.identity`
to the **calling context's qualified name**, established at the boundary over the
internal Service Binding before the body runs. The name is proved by the call's
per-edge signature (emission §7.3.4d); a call whose caller holds no key at the
callee is refused (`401`) before any handler. `Caller` admits Bynk contexts
only: a call from a foreign caller reaches a handler only if its `by` clause
names that `Foreign` actor, and is otherwise refused (`403`). A binder-less
`on call` captures nothing and is unaffected.
The **binder is optional** (v0.50): with `by <binder>: <Actor>` the verified
identity binds to `<binder>` and is read as `<binder>.identity` — a sealed value,
minted at the boundary before the body runs and never re-checked downstream; with
the binder-less `by <Actor>` the contract is still declared and verified
fail-closed, but no identity is captured (anonymous / verify-and-discard). `_`
MUST NOT be used as the binder (omit it instead). A named binder MUST NOT collide
with a handler parameter (`bynk.actor.binder_shadows_param`).

### §5.7a.1 Multi-actor sum dispatch (v0.52)

A `by` clause MAY name an **ordered sum of peer actors**
(`by who: A | B | …`): distinct parties distinguished by **scheme**, resolved
**first-wins**. The boundary tries each peer's scheme in declared order and binds
the first that verifies; the body `match`es on the resolved actor, each arm
yielding that actor's identity directly (`User(u)` ⇒ `u` is the `User` identity;
a unit-identity peer such as `Visitor` or a `Signature` webhook binds nothing).
A sum is well-formed when:

- it **binds the resolved actor** — a sum MUST have a binder, since the body
  learns *which* peer verified by matching it (`bynk.actor.sum_requires_binder`);
- its members are **peer base actors** — a refinement actor (`actor A = B where
  …`) MUST NOT be a member (every `A` is a `B`, so the arm is dead,
  `bynk.actor.refinement_in_sum`); narrowing belongs *inside* the resolved arm;
- **no two members share a scheme** — peers are distinguished by scheme, so a
  second same-scheme member is unreachable (`bynk.actor.duplicate_sum_scheme`);
- a **`None`-scheme (catch-all) member is last** — it accepts every caller, so any
  member after it is unreachable (`bynk.actor.unreachable_sum_arm`);
- **every member is admissible** on the handler's protocol (in practice a sum is
  HTTP-only — the only protocol with more than one admissible non-internal
  scheme) (`bynk.actor.scheme_not_admissible`);
- the body `match` is **exhaustive** over the members (the ordinary
  sum-exhaustiveness rule, `bynk.types.non_exhaustive_match`).

The reachability checks are **decidable and scheme-level**; the compiler does not
reason about predicate-level disjointness. Total verification failure (no member
verifies) is **fail-closed → 401**; a sum's members carry no invariants, so there
is no 403 path. Verification is side-effect-free and idempotent: first-wins
short-circuits, so the set and order of verifications attempted is observable, and
audit/logging belongs *after* resolution.

## §5.7b Events (v0.238–v0.244) {#events}

An **event** is a typed fact one context emits and any number of contexts
receive. Delivery is at-least-once and fire-and-forget: an emitter never learns
whether, or how, a subscriber ran.

**Declaration.** An `event` ([§4.1.12](/book/spec/syntactic-grammar/#4112-event_decl))
MUST be declared in a context (`bynk.event.outside_context`). Its one admitted
annotation is `@schema(N)` (`bynk.event.unknown_annotation`), at most once, with
`N` a single positive `Int` literal (`bynk.event.bad_schema_version`). A field
default (`field: T = expr`) is admitted only on an event's own fields
(`bynk.event.default_outside_event`), and `expr` MUST be a static,
wire-representable value of `T`: a literal admitted to `T`, a sum variant,
`Some`/`None`/`Ok`/`Err`, a record, or `T.unsafe(lit)` for an opaque `T`
(`bynk.event.bad_field_default`). A default is used when a received payload has
no key for the field; a present key, `None` included, decodes as sent.

**Emission.** `Events` is a first-party capability of the `bynk` adapter,
`emit[E](event: E) -> Effect[()]`, consumed with `consumes bynk { Events }` and
required by `given Events`; its type argument is always written, never inferred.
`E` MUST be an `event` (`bynk.event.emit_not_an_event`) declared in the emitting
context itself (`bynk.event.emit_outside_owner`): a context that can name a
foreign event, to subscribe to it, still cannot emit it. An emission is released
only when the emitting handler commits ([§5.4](#54-agents--state)); a handler that
faults, an `InvariantViolation` included, emits nothing.

**Subscription.** A `from Events(E)` service
([§4.4.7b](/book/spec/syntactic-grammar/#447b-events-subscriptions-v0238)) MUST name an
event declared in its own context or a consumed one
(`bynk.event.unknown_subscription`), and it MUST have exactly one handler, an
`on event` (`bynk.parse.empty_service` for none, `bynk.service.mixed_protocols`
for any other kind, `bynk.event.duplicate_handler` for a second `on event`). An `on event` handler
takes the payload, of type `E` (`bynk.event.handler_param_type_mismatch`), and
optionally a second parameter of type `EventEnvelope`; any other parameter list
is `bynk.event.bad_params`. It MUST return `Effect[()]`
(`bynk.event.return_not_effect_unit`): nothing receives a subscriber's result.

**Delivery filters.** A header pattern's fields MUST be fields of `E`
(`bynk.event.pattern_unknown_field`), each listed once
(`bynk.event.pattern_duplicate_field`), with a value compatible with the field's
type (`bynk.event.pattern_type_mismatch`); a variant value MUST exist on the
field's sum type (`bynk.event.pattern_unknown_variant`) and carry no payload
(`bynk.event.pattern_variant_payload`). Listed fields combine with AND. A
`via schema(N)` clause MUST give a single positive `Int` literal
(`bynk.event.bad_schema_dispatch`) and matches an emission whose envelope
`schemaVersion` equals `N`. Both filters are **deliver-and-filter**: every
emission of `E` is delivered to every subscriber of `E`, and each subscriber
evaluates its own filters as a guard on entry, running its body only on a
match. A filter does not narrow the handler parameter's type, and two
subscribers whose filters overlap are not diagnosed.

**The envelope.** `EventEnvelope`, exported by `bynk`, is
`{ eventId: String, publisherId: String, emittedAt: Instant, schemaVersion: Int }`.
`eventId` identifies the emission: every subscriber of one emission observes the
same value. `publisherId` is the emitting context's qualified name.

**Schema versions.** Every build reconciles each event's field shape — names,
types, and which fields carry a default — against the project's committed
schema registry, `bynk.schema.lock`. A new event starts at version 1, or at its
`@schema(N)`. An unchanged shape keeps its version; an additive change, in which
every added field has a default and no field is removed, retyped, or loses a
default, increments it by one; any other change is
`bynk.event.non_additive_schema_change`. A declared `@schema(N)` MUST equal the
version so computed (`bynk.event.schema_version_mismatch`). The computed version
is the envelope's `schemaVersion`. A check (`bynkc check`) and a test run
(`bynkc test`) reconcile against the committed registry the same way and report
the same diagnostics, but never write it; only a build records a new version.

**Idempotent handling.** `Idempotency` is a first-party capability of the `bynk`
adapter with two operations, `dedup[T](key: String) -> Effect[Option[T]]` and
`remember[T](key: String, value: T, expiresAfter: Duration) -> Effect[()]`; each
call's key is scoped to the calling handler's qualified name, so two handlers
never share a key by accident. A subscriber that must not apply one emission
twice deduplicates on `env.eventId` with this pair: the idiom is a documented
convention, not syntax ([ADR 0294](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0294-events-idempotency-idiom-is-documented-convention.md)).

## §5.8 Boundaries & cross-context

`consumes` MUST appear only in a context or an adapter (a `commons` or `suite`
cannot declare one: `bynk.parse.expected_item`),
MUST name an existing context or adapter — not a `commons`
(`bynk.consumes.unknown_context`, `bynk.consumes.target_is_commons`) — and not the
consumer itself (`bynk.consumes.self_reference`), and MUST NOT produce colliding
names or aliases (the `bynk.consumes.*` codes). Calling another context's service
requires a `consumes` declaration (`bynk.resolve.unconsumed_context`), and units
MUST NOT form a `consumes` cycle (`bynk.context.consumes_cycle`).

A **capability selection** (`consumes b { Cap, … }`) flattens each named
capability into the consumer's local namespace under its bare name, so it reads
as `given Cap` / `Cap.op(…)`. Each selected name MUST be a capability the target
**exports** (`bynk.given.cross_context_unknown_capability`), and a flattened bare
name MUST NOT collide with a locally declared capability or with a name
flattened from another unit (`bynk.consumes.capability_name_clash`) — a clash is
resolved by the qualified `given b.Cap` form or an alias.

An **adapter's** `consumes` is further restricted: it MUST use the
capability-selection form — an adapter has no services to call, so the
whole-unit and aliased forms are rejected
(`bynk.adapter.consumes_requires_selection`) — and it MUST target an adapter,
never a context (`bynk.adapter.consumes_context`).

`exports` MUST name declared types or capabilities, MUST NOT export a name twice
or with conflicting visibility, and an exported capability MUST have a provider
(the `bynk.exports.*` codes). A value crossing a boundary MUST be structurally
compatible with the receiving side ([§6.5](/book/spec/type-system/#65-type-compatibility--boundaries),
`bynk.boundary.structural_mismatch`); outside its owning context, a context-owned
type MUST NOT be built through its constructors (`.of`, `.unsafe`, a variant) and
an opaque export MUST NOT be built with record syntax or inspected
(`bynk.context.external_construction`, `bynk.context.opaque_inspection`); a
field read on a value of an opaque export is an inspection, however the value
was bound. A transparent record's fields are shared, so a consumer MAY build
one with record syntax. A consumer MAY read the fields of a value whose type the
providing context declares but does not export; the read is checked against
that declaration ([§6.5](/book/spec/type-system/#65-type-compatibility--boundaries)). These rules hold in every body — functions, service and agent handlers,
and provider operations.

**Adapters are the host boundary.** An adapter MUST NOT declare a `service` or an
`agent` (`bynk.adapter.disallowed_item`); it MAY declare at most one `binding`
clause (`bynk.adapter.duplicate_binding`), and MUST declare one if it declares
any external provider (`bynk.adapter.no_binding`). A binding's `requires` ranges
MUST be pinned: a range MUST name at least one version digit — `*`, `x`,
`latest`, and digit-free ranges are rejected
(`bynk.requires.unpinned_dependency`). Every `requires` entry for one package
MUST give the same range; a different range for a package another adapter
already requires is rejected (`bynk.requires.conflicting_range`). A reached
adapter of a git or registry dependency MUST NOT require a package at a range
that `bynk.lock` doesn't approve (`approved = true` on its `[[npm]]` row;
`bynk.requires.unapproved`); the project's own adapters and its path
dependencies' need no approval. The `bynk` namespace is **reserved for the
toolchain**: no user unit's name may have `bynk` as its first segment
(`bynk.namespace.reserved`); the toolchain's own first-party adapters — the
ambient `bynk` surface and the `bynk.<platform>` platform adapters
([§7.3.6](/book/spec/emission/#736-adapters)) — live inside that reserved prefix and are
injected when a unit consumes them.

**The platform lock** (v0.19). A capability of a **platform adapter**
(`bynk.cloudflare`) is *platform-native*: its binding runs only on that
platform. A **deployment unit** — each context under `--target workers`; the
whole program under `bundle`, where co-location shares the lock — is locked to
the union of native platforms its **in-process closure** reaches: the providers
its composition would instantiate, walked through `given` and flattening edges.
A service `consumes` edge between contexts is RPC under `workers` and does
**not** propagate the lock. The selected `--platform` MUST be the locked
platform (`bynk.target.vendor_required`), and one deployment unit MUST NOT span
two mutually-exclusive native platforms (`bynk.target.vendor_conflict`). The
`bynk` surface and library adapters impose no lock. New operations on an
already-native capability (v0.23: `Kv.putTtl`, `Kv.list`) inherit the lock
unchanged — no per-operation rules exist.

A `system`-tier test ([§5.9c](#59c-tiers-v0118)) derives its wired participant set from
the unit under test's transitive `consumes` closure; the inferred set MUST span at
least two contexts, or the target MUST expose an `http` or `queue` service
(`bynk.tier.system_needs_wire`). There is no participant list,
so the set cannot drift from the dependency graph and every consumed dependency is
wired by construction.

{{#grammar-semantics consumes_decl}}

{{#grammar-semantics adapter_decl}}

{{#grammar-semantics binding_decl}}

## §5.9 Testing constructs

An `expect` MUST occur only in a `case` body and MUST be given a `Bool` predicate
(`bynk.expect.outside_case`, `bynk.expect.not_bool`) — the same invariant
predicate surface as `invariant`/`ensures` (one predicate surface, ADR 0144). A
`suite` MUST target an existing unit and MUST NOT duplicate a case description
(`bynk.suite.unknown_target`, `bynk.suite.duplicate_case_name`).

`Val[T]` (v0.114, retiring `Mock[T]`) MUST occur only in a test body
(`bynk.val.outside_test`), name a resolvable type (`bynk.val.unknown_type`), and
receive pins that are compile-time literals of the right arity satisfying the
type (`bynk.val.pin_not_literal`, `bynk.val.arity`, `bynk.val.literal_violates`);
a type that cannot be fabricated MUST be pinned (`bynk.val.needs_pin`,
`bynk.val.pin_unsupported`, `bynk.val.unsupported_kind`).

{{#grammar-semantics val_expr}}

A `Wire(...)` raw argument
([§4.6.24](/book/spec/syntactic-grammar/#4624-wire_expr)) MUST occur only as an
argument of a service address in a `system`-tier `case`
(`bynk.test.wire_needs_system`): it hands unvalidated input to the boundary,
which only a `system` case runs.

An `advance Clock by <amount>` statement
([`advance_stmt`](/book/reference/grammar/#rule-advance_stmt)) MUST occur only in
a `case` body (`bynk.test.advance_outside_case`), MUST name `Clock`
(`bynk.test.advance_not_clock`), and its amount MUST be a `Duration`
(`bynk.test.advance_not_duration`). It moves the case's harness clock, which
starts at the epoch; the `Clock` test double reads it at `unit` and
`integration`, and every participant's first-party `Clock` reads it at
`system`. A negative amount faults the case at run time.

In a `system`-tier `case`, a `svc.schedule("…", …)` address runs the cron
handler through the participant Worker's `scheduled` entry point and yields the
handler's `Result`, and a `svc.message(m)` address delivers `m`, after a JSON
round trip, through its `queue` entry point and yields `Ack` or `Retry(reason)`
as the consumer acknowledged or retried it. The scheduled time is required when
the handler declares it, as at the other tiers.

An http address in a `system`-tier `case` MUST NOT reach a route whose actor
(for an actor sum, the member the call-site principal names, or the primary
member when it names none) has an `Oidc` scheme
(`bynk.test.oidc_needs_in_process`): the `system` driver has no `Oidc` token
issuer, so the call could only be refused at the seam. The driver presents the
acting actor's credential: a signed token for a Bearer actor, a signature for a
`Signature` actor (#2135), and on an actor sum the credential of the member the
principal names (#2155).

`Nobody`, `Forged` and `Stale` are reserved call-site principals, not
declarations, and take no identity (`bynk.test.actor_no_identity`). `by Nobody`
MUST address a route with a Bearer or `Signature` actor, alone or as a sum
member (`bynk.test.nobody_needs_secured_route`). `by Forged` and `by Stale` MUST
address a route with a `Signature` actor, alone or as a sum member
(`bynk.test.forged_needs_signature_route`); on a sum they sign as its first
`Signature` member. On an actor sum with an open member (`Visitor`, or an
actor with no auth scheme), none of the three is admitted: the seam resolves a
missing or bad credential as that member and never answers `401`. `by Stale` needs that actor to declare `timestamp` and
`tolerance` (`bynk.test.stale_needs_timestamp`). All
three MUST occur in a `system`-tier case (`bynk.test.credential_needs_system`,
`bynk.test.forged_needs_system`), and each address yields an `HttpOutcome`.

A service address in a `case` (`svc.call(…)`, `svc.<VERB>("/path", …)`,
`svc.schedule("…")`, `svc.message(m)`, `svc.event(e)`) has the addressed handler's declared
return type, an `Effect[T]` (#1934). An http address that carries a `Wire(...)`
argument, is driven `by Nobody`, or names a method the path declares no handler
for has type `Effect[HttpOutcome[T]]` instead, where the handler returns
`Effect[HttpResult[T]]` (`T` is `()` for a method with no handler).
`HttpOutcome[T]` is the built-in sum `Rejected(BoundaryRejection) |
Handled(HttpResult[T])`, and `BoundaryRejection` is the built-in sum
`RefinementViolation(detail: String) | MalformedJson(detail: String) |
StructuralMismatch(detail: String) | Unauthorized | MethodNotAllowed |
PayloadTooLarge(detail: String) | UnsupportedMediaType(detail: String)`. Neither
type can be named in source. An asset fetch (`svc.GET(asset)`) has type
`Effect[HttpResult[()]]`.

An event address `svc.event(e)` or `svc.event(e, env)` (#2052) MUST name a
`from Events(E)` service (`bynk.test.service_bad_address`) and pass one or two
arguments (`bynk.test.service_call_arity`). `e` MUST have type `E`: another
event the target declares or consumes is `bynk.test.event_not_subscribed`, any
other type `bynk.types.argument_mismatch`. `env` MUST have type `EventEnvelope`
(`bynk.types.argument_mismatch`). The address MUST NOT occur in a `system`-tier
`case` (`bynk.test.event_needs_in_process`). Delivery evaluates the service's
pattern filter and `via schema(N)` guard as a fan-out delivery does. An omitted
`env` is a fresh envelope: a new `eventId` per call, `E`'s publisher as
`publisherId`, `Instant.fromEpochMillis(0)` as `emittedAt`, and `E`'s current
schema version. The publisher is the target if it declares `E`, else the one
context it consumes that declares `E`. With no such unique context, `env` MUST
be given (`bynk.test.event_publisher_ambiguous`).

### §5.9c Tiers (v0.118)

*(ADR 0153)* A `case` runs at one of three **tiers** — `unit` | `integration` |
`system` — declared by an optional `as <tier>` clause on the `case` header. `unit`
is the default and is elided. `as` also sits on the `suite` header as an inherited
default; a case's effective tier is `case.tier ?? suite.tier ?? unit`, so a case
overrides the suite default. A tier is metadata on the header, **not** an
executable statement: promotion changes substitution, not assertion, and between
`unit` and `integration` the case body is identical.

- **`system` is a suite of its own (#1738).** A `system` case runs against the
  contexts stood up as Workers and addresses a service by its context path, so a
  suite whose effective tiers include `system` MUST contain only `system` cases
  (`bynk.tier.mixed_system_suite`).
- **Crossing a context requires `system` (#1737).** Below `system` a case runs
  in-process: no consumed context is stood up, and `stub` substitutes capability
  seams only. A `unit` or `integration` case, or a `property` (which has no tier
  and always runs in-process), that reaches a consumed context's service,
  directly or through a service or agent handler of the unit under test that
  (transitively) calls one, MUST be rejected
  (`bynk.tier.cross_context_needs_system`).

- **Tiers are `case`-only.** A `property` generates and does not promote, so a
  suite-level `as` binds its `case` members only, and a `property` header has no
  tier production (a tier there is a parse error).
- **Participants are inferred (DECISION K).** For `system` the wired participant
  set is the unit under test's transitive `consumes` closure — there is no `wires`
  clause. `system` is the wired tier: its inferred set MUST span at least two
  contexts, or its target MUST expose an `http` or `queue` service
  (`bynk.tier.system_needs_wire`);
  `integration` is real collaborators **within one context, no wire**, and carries
  no ≥ 2-context rule.
- **The agent-state lifecycle is fixed across tiers (DECISION D7).** The unit under
  test is always a real in-memory instance, keyed normally, fresh per case; only
  the realness of its collaborators and whether sends cross a serialisation
  boundary change with the tier. Snapshot and step invariants are checked at the
  commit boundary and therefore fire at every tier.
- **At `unit`, an un-overridden seam keeps its real provider (DECISION D8).** Full
  auto-stubbing (a synthesised return per collaborator) is a named follow-on, not
  part of this increment; `unit` and `integration` differ in the default provision
  discipline the author follows, and `system` is the tier that differs
  mechanically (it wires participants across the real boundary).

The tier names `unit` / `integration` / `system` are **contextual** — parsed only
in the `as`-clause position after a case/suite header; elsewhere they remain
ordinary identifiers.

### §5.9d Test-double provision — `stub` (v0.118)

*(ADR 0154; keyword `stub` since #548, formerly a pun on `provides`)* A
`stub Cap.method(<args>) returns <value> | fails` clause
overrides one capability seam's provision under test. `as <tier>` sets the
*default* provision of every seam; a `stub` clause overrides one. It names a
consumed seam of the unit under test (`consumes` declares, `given` requires,
`stub` substitutes under test), scoped to a test.

- **Seam resolution and legality (DECISION M).** `stub` is **capability-only**:
  its target MUST be a capability the unit under test consumes / has in scope via
  `given` (`bynk.stub.not_a_seam`), and `method` MUST be one of that
  capability's declared operations (`bynk.stub.unknown_op`).
- **Scope and precedence.** A `stub` MAY appear at suite scope (applies to every
  case) and at case scope (overrides for one case); precedence is case `stub` >
  suite `stub` > the tier default.
- **Tier (#2026).** A `stub` MUST NOT appear in a `system`-tier case, nor at the
  scope of a suite that runs `system` cases (`suite … as system`, or one with a
  `case … as system`) (`bynk.stub.at_system`): each participant runs as
  the Worker it deploys as, whose composition builds its own capability providers,
  so no seam is open to a double.
- **Argument patterns (DECISION D3).** Each parameter takes a pattern from the one
  predicate surface — `_` (any) or a literal / pure value the recorded argument must
  equal, plus `is` narrowing. Multiple clauses for one method form an ordered match
  list, tried top to bottom, **first match wins**, so a specific clause MUST precede
  a fallback.
- **RHS typing (DECISION D2).** The right-hand side is a *value* or the fault atom
  `fails`, never a computed body. A `returns <value>` whose type disagrees with the
  operation's declared return type is `bynk.stub.rhs_type`.
- **Sequenced provision (DECISION V).** `returns each [<outcome>, …]` supplies one
  outcome per call, in order; each outcome is a value, `fails`, or `ok(v)`. On
  **exhaustion the last outcome repeats** (steady state). A malformed sequence
  (e.g. empty) is `bynk.stub.bad_sequence`.

Bare observation ([§5.9b](#59b-observation)) needs no `stub` — calls are
recorded at the seam regardless; a `stub` is written only when a case depends on
a collaborator's *return*.

#### Suite configuration — `env` (#2134) {#suite-env}

An `env { NAME = "value" … }` clause gives the variables and secrets a suite's
participants see. It sets the environment and overrides no seam, so it is legal
at every tier, `system` included.

- **Values.** Each entry's value MUST be a `String` literal
  (`bynk.suite.env_not_literal`).
- **Names.** A name MUST NOT be given twice at one scope: across a suite's own
  `env` clauses, or across one case's (`bynk.suite.duplicate_env`).
- **Scope and precedence.** A case runs with its suite's entries overlaid by its
  own, name by name. A suite's entries apply to its own cases only; a `property`
  runs with none.
- **Meaning by tier.** At `system` the case's entries are put on each
  participant's Worker `env` before the harness's own keys and bindings. At
  `unit` and `integration` they are what the `Secrets` test double answers:
  `Some(value)` for a name an entry gives, `None` otherwise.
- **Bindings (`system`).** A name MUST NOT be a binding a participant's Worker
  declares, nor a key the harness sets on it (a call-edge key, a `Foreign`
  caller's secret) (`bynk.suite.env_binding_clash`). The check needs the Worker bindings,
  so it runs on an otherwise clean build.
- **Test secrets.** An entry naming a `Bearer` actor's declared `secret` is the
  secret the `system` driver signs with, and the one the seam verifies with,
  in place of the fixed test secret.

### §5.9a Generative properties

*(v0.114)* A `property` is a generative test: its body is a single `for all`
binder over inhabitants the runner produces. Each binding `x: T` binds `x` to a
generated inhabitant of `T`; an optional `where <pred>` filters generated tuples
before the body runs; the body is one or more `expect`s (the one predicate
surface). The `where` filter MUST type to `Bool` (`bynk.property.where_not_bool`),
as MUST each `expect` (`bynk.expect.not_bool`).

**Generation (DECISION P): a type is its own inhabitant space.** The generator
for `T` produces only values satisfying `T`'s refinements, so a generated subject
is valid by construction. A `T` used in `for all` (or `Val`) MUST be
refinement-generable: a `String where Matches(re)` has no refinement-driven
generator and MUST be pinned instead (`bynk.val.needs_pin`); an **agent** MUST
NOT be generated (`bynk.val.agent_not_generable`) — a fabricated agent state need
not be reachable, so behavioural agent generation is handler-*sequence*
generation, deferred to the history rung.

**Restating a refinement is redundant.** A property whose predicate merely
re-checks a refinement the bound variable's type already guarantees is flagged
(`bynk.property.restates_refinement`). The check is **conservative** — it fires
only when the predicate is syntactically the refinement over the bound variable
(under-flagging is acceptable; over-flagging is not).

{{#grammar-semantics property_decl}}

{{#grammar-semantics for_all}}

### §5.9b Observation

*(v0.117)* An **observation** asserts over a unit's *interaction* with a
capability, inside a `case` — ADR 0152. Its subject is a `Cap.op` reference (a
capability and one of its operations, named not called). Well-formedness:

- the observation MUST occur inside a `case` body — it is an `expect`, so outside a
  test body it is rejected as one (`bynk.expect.outside_case`);
- `Cap` MUST be a capability the unit under test consumes / has in scope via
  `given` (`bynk.observe.not_a_seam`); `op` MUST be one of its declared operations
  (`bynk.observe.unknown_op`);
- a `with <pred>` predicate is the one predicate surface with the operation's
  parameters in scope by their declared names; it MUST type to `Bool`
  (`bynk.observe.with_not_bool`) and MUST be pure (`bynk.observe.impure_with`);
- a call count is `once` or a non-negative integer literal followed by `times`
  (the grammar admits nothing else) — `called once` desugars to a count of one.

**Issue order.** The recorder logs a call when it is *made*, so `A.op before B.op`
compares **issue** order, not completion order. Inside a `par` block (§4.8.7b) the
first capability call of each binding is issued in source order, so between two
bindings' first calls it is source order. A later call inside a composite binding —
a user function that makes several calls — is issued when the earlier ones complete,
so its place against another binding's calls follows completion and microtask
order. Neither is evidence that one effect *finished* first; a test that needs
completion order is testing a race and belongs against a stub that controls it.

`trace(Cap.op)` is a **test-only builtin** yielding `List[<CallRecord>]`, where
`<CallRecord>` is a synthetic record of the operation's parameters at their declared
types (`{ msg: String }` for `Logger.log`), registered into the test-body type table
the way a fabricated agent-state record is. It types as an ordinary `List`
(field access on its elements, `length()`, `all`/`any`, indexing); `trace` outside a
`case` is `bynk.observe.trace_outside_test`. Recording is *ambient at the seam and
test-build-only* — no source declares it, and the deploy build carries none of it
(see [emission](/book/spec/emission/)).

### §5.9e Fault claim

A **fault claim**, `expect <call> [by <Actor>(<identity>)] faults` (#1706, #1812), asserts that awaiting an
effectful call throws rather than returns. A fault is untyped and no caller can
handle it ([§7.4.12](/book/spec/runtime-library/#7412-the-stub-clause-v0118)), so
the claim is how a `case` observes one, not a way to catch it. Well-formedness:

- the claim MUST occur inside a `case` body — it is an `expect`, so outside a test
  body it is rejected as one (`bynk.expect.outside_case`);
- the subject MUST be a call expression whose type is `Effect[_]`
  (`bynk.expect.faults_not_effect`) — a value bound from a call has already run it;
- awaiting is an effect, so the claim MUST sit in an effectful body, as `<-` does
  (`bynk.effect.bind_in_pure_context`);
- the claim's optional call-site principal, written between the call and
  `faults`, is validated against the addressed handler exactly as an effect-let's
  is ([`call_site_actor`](/book/reference/grammar/#rule-call_site_actor)): an addressed handler that requires an
  identity and is given no `by` is `bynk.test.principal_required`, and a supplied
  identity MUST type as the handler's;
- the case MUST NOT be `system`-tier (`bynk.test.faults_needs_in_process`): at
  `system` a handler's fault crosses the deployed Worker's boundary as an error
  response, never a throw. The restriction is deliberate (#1811): a fault is an
  in-process concept, and at `system` the observable is the response. A `system`
  case asserts `r is ServerError(_)`, which does not distinguish a fault from a
  handler's deliberate `ServerError`;
- the principal MUST NOT be `by Nobody`, at any tier
  (`bynk.test.faults_needs_in_process`): `Nobody` exercises the `system`-tier auth
  seam, where the claim cannot hold.

The claim types as `Bool`.

{{#grammar-semantics faults_expr}}

## §5.10 Collections

*(v0.20b)* `List[T]` and `Map[K, V]` are built-in generic types
([§6.2](/book/spec/type-system/#62-built-in-generic-types)); this section is their
static semantics. `Set[T]` (standard-library K6a, #2596) joins them, and like
them `Set` is a reserved built-in type name (`bynk.resolve.reserved_builtin_type`
for a `type Set`). Its
element type MUST be value-keyable, by the `Map`-key rule
(`bynk.types.unkeyable_set_element`), checked at a written `Set[T]` and at the
inferred element of `Set.of`. `Set.empty()` obeys the empty-collection rule
below; `insert` and `remove` propagate an expected `Set` down their receiver
chain. A `Set`'s compatibility at a boundary requires its element type to match
exactly, as a `Map` key's does.

**Construction.** A list literal `[a, b, c]`
([§4 list_literal](/book/reference/grammar/#rule-list_literal)) types each
element against the **expected element type** when one is supplied — so
refined literals admit ([§5.3](#53-refinement--admission)) — and a mismatched
element is `bynk.types.list_element_mismatch`. With no expected type, the
first element fixes the element type. An **empty `[]` MUST have an expected
type** (`bynk.types.uninferable_element_type`); the qualified statics
`List.empty()` and `Map.empty()` obey exactly the same rule — an expected
type is their only source of type arguments. `insert`, `prepend`, `append`,
and `concat` propagate an expected collection type down their receiver chain,
so `let m: Map[String, Int] = Map.empty().insert("a", 1)` infers, and so does
`let xs: List[Int] = [].append(1)`. `concat`'s argument checks against the
receiver's `List[T]`, so `xs.concat([])` needs no annotation.

**The kernel.** The built-in operations are compiler-known special forms,
dispatched on the receiver's checked type before declared-method lookup;
they may be generic in their accumulator without declared generic methods
existing (ADR 0037). The whole kernel:

| Receiver | Operation | Type |
|---|---|---|
| `List[T]` | `length()` | `Int` |
| `List[T]` | `get(i: Int)` | `Option[T]` |
| `List[T]` | `prepend(x: T)` | `List[T]` |
| `List[T]` | `append(x: T)` | `List[T]` |
| `List[T]` | `concat(other: List[T])` | `List[T]` |
| `List[T]` | `fold(init: A, f: (A, T) -> A)` | `A` |
| `List[T]` | `foldEff(init: A, f: (A, T) -> Effect[A])` | `Effect[A]` |
| `List[T]` | `forEach(f: T -> Effect[()])` | `Effect[()]` |
| `List[T]` | `parTraverse(f: T -> Effect[()])` | `Effect[()]` |
| `List[T]` | `traverseAll(f: T -> Effect[Result[U, E]])` | `Effect[List[Result[U, E]]]` |
| `List[T]` | `parTraverseAll(f: T -> Effect[Result[U, E]])` | `Effect[List[Result[U, E]]]` |
| `List[T]` | `traverseTry(f: T -> Effect[Result[U, E]])` | `Effect[Result[List[U], E]]` |
| `List[T]` | `parTraverseTry(f: T -> Effect[Result[U, E]])` | `Effect[Result[List[U], E]]` |
| `List[T]` | `map(f: T -> U)` | `List[U]` |
| `List[T]` | `filter(p: T -> Bool)` | `List[T]` |
| `List[T]` | `flatMap(f: T -> List[U])` | `List[U]` |
| `List[T]` | `sortBy(key: T -> K)` | `List[T]` |
| `List[T]` | `take(n: Int)` / `skip(n: Int)` | `List[T]` |
| `List[T]` | `distinct()` | `List[T]` |
| `List[T]` | `distinctBy(key: T -> K)` | `List[T]` |
| `List[T]` | `joinOn(other: List[U], left: T -> K, right: U -> K, into: (T, U) -> V)` | `List[V]` |
| `List[T]` | `leftJoin(other: List[U], left: T -> K, right: U -> K, into: (T, Option[U]) -> V)` | `List[V]` |
| `List[T]` | `join(other: List[U], on: (T, U) -> Bool, into: (T, U) -> V)` | `List[V]` |
| `List[T]` | `groupBy(key: T -> K, into: (K, List[T]) -> V)` | `List[V]` |
| `List[T]` | `count()` | `Int` |
| `List[T]` | `any(p: T -> Bool)` / `all(p: T -> Bool)` | `Bool` |
| `List[T]` | `first()` | `Option[T]` |
| `List[T]` | `firstOrElse(default: T)` | `T` |
| `List[T]` | `sum(key: T -> K)` | `K` |
| `List[T]` | `min(key: T -> K)` / `max(key: T -> K)` | `Option[K]` |
| `List[T]` | `average(key: T -> K)` | `Option[Float]` |
| `Map[K, V]` | `length()` | `Int` |
| `Map[K, V]` | `keys()` | `List[K]` |
| `Map[K, V]` | `values()` | `List[V]` |
| `Map[K, V]` | `get(k: K)` | `Option[V]` |
| `Map[K, V]` | `insert(k: K, v: V)` | `Map[K, V]` |

A method outside the kernel is `bynk.types.method_not_found`; a wrong arity
is `bynk.types.method_arity`. **`foldEff`, `forEach`, `parTraverse`,
`traverseAll`, `parTraverseAll`, `traverseTry`, and `parTraverseTry` are effect
operations**: each runs its
effectful function value, so calling one in a pure context is
`bynk.effect.fn_value_in_pure_context`, exactly the function-value confinement of
[§5.5](#55-effects-capabilities--providers). `forEach(f: T -> Effect[()])`
(v0.146, [ADR 0170](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0170-do-statement-implicit-unit-and-list-foreach.md))
is the `Query.forEach` terminal over an eager list — the effect-per-element loop,
**sequential** (each element awaited in order). `parTraverse(f: T -> Effect[()])`
(v0.147, [ADR 0171](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0171-list-partraverse.md))
is its **concurrent** sibling — the `Query.parTraverse` fan-out over an eager
list, issuing every element's effect at once and awaiting them together, so a
slow element does not head-of-line-block the rest; the order in which side
effects interleave is unspecified. Unlike the sequential `forEach` (which
short-circuits on the first failure and never issues the rest), a rejecting
element does **not** cancel siblings already issued — every element's effect is
in flight before the first failure surfaces, and each runs to completion.

The **collect-all** pair `traverseAll` / `parTraverseAll` (v0.148,
[ADR 0172](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0172-list-collect-all-iterators.md))
take a **`Result`-returning** function `f: T -> Effect[Result[U, E]]` and return
`Effect[List[Result[U, E]]]` — every element's outcome, `Ok` or `Err`, gathered
into the result list in input order. Because a `Result` `Err` is a **value**, not
a fault, neither short-circuits: `traverseAll` awaits each element in turn,
`parTraverseAll` issues all at once and collects them together (its interleaving
order, like `parTraverse`, unspecified). The function *must* return
`Effect[Result[U, E]]`; a non-`Result` effect is `bynk.types.combinator_return_mismatch`.
They are the fault-gathering counterpart to `traverse` (the short-circuiting
sequential collect); the collecting **short-circuit** `parTraverse` overload and
`traverse`'s `Result` overload remain a later slice. Like `forEach`/`parTraverse`,
both also apply over a lazy `Query[T]` and a lifted `store Map[K, V]` (v0.149,
[ADR 0173](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0173-map-values-and-broadcast-collect-all.md)),
so the fan-out reaches a held `Map[K, Connection]` — each connection is
**borrowed** into the closure (`send` allowed, `close`/transfer rejected as
`bynk.held.consume_on_borrow`) and the map keeps ownership.

The **short-circuit** pair `traverseTry` / `parTraverseTry` (v0.150,
[ADR 0174](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0174-short-circuit-collect-iterators.md))
take the same `f: T -> Effect[Result[U, E]]` but **stop at the first `Err`**,
returning `Effect[Result[List[U], E]]` — `Ok` of the collected values, or the
first `Err` encountered. `traverseTry` awaits each element in order and bails on
the first `Err` (later elements never run); `parTraverseTry` issues all at once,
awaits them, then returns the first `Err` in input order (in-flight calls are not
cancelled). They are the fault-**propagating** counterpart to the fault-gathering
`traverseAll`/`parTraverseAll`, and likewise apply over the `Query`/`Map`
broadcast.

*(v0.88, [ADR 0116](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0116-query-vocabulary-and-ordering.md))*
The builder/terminal rows above are the **eager in-memory half** of the query
algebra ([design notes §11](https://github.com/bynk-lang/bynk/blob/main/design/bynk-design-notes.md)) —
the same combinator names a lazy storage `Query[T]` will carry. **Ordering keys**
(`sortBy`/`min`/`max`) on a `List` are any `Ord` type (§5.2) — else
`bynk.types.key_not_orderable`. On a storage `Query` the key is still drawn from
the closed base set — `Int`, `Float`, `String`, `Duration`, `Instant` (refined
types widening; an opaque key is **not** orderable) — because a user-type
comparator has no storage lowering; the same code is reported. **Numeric keys**
(`sum`/`average`) are `Int`/`Float`/`Duration` (not `Instant` — instants are not
summable), else `bynk.query.sum_needs_numeric`;
`average` of a `Duration` is a `Duration` (integer-rounded millis), otherwise a
`Float`. **`distinct`/`distinctBy`** need a value-keyable element/key (the
`Map`-key rule, incl. opaque), else `bynk.types.unkeyable_distinct`. **Empty
aggregates are total**: `first`/`min`/`max`/`average` return `Option` (`None` on
empty); `sum` returns the zero, `count` returns `0`. The aggregate terminals take
a **projection** `T -> K`, uniform with the storage half where a record field is
the common key.

*(v0.94, [ADR 0120](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0120-join-group-combiner-form.md))*
**Joins & grouping take a combiner** — bynk has no pair type, so the join row
exists only inside `into`, which names the result. `joinOn`/`leftJoin` are
equi-joins hashing on a **value-keyable** key (the `Map`-key rule); the left and
right key functions MUST return the **same** key type, else
`bynk.query.join_key_mismatch`. `join` is a general-predicate (nested-loop) join.
`groupBy` partitions by a value-keyable key in **first-seen** key order, projecting
each `(K, List[T])` group through `into`. The `other` side is a `List`/`Query` of
the **matching shape** (a `List` joins a `List`, a `Query` joins a `Query` — a bare
`store Map` used as a value lifts to a `Query` over its values). The same names
carry to a storage `Query` (the builders return `Query[V]`, still lazy). Arguments
are **positional** in v1 (labelled call arguments are a named deferral). The
cross-shape `Map × Log` join lands with the storage `Log` slice.

*(v0.92, [ADR 0115](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0115-query-model-lazy-eager-dispatch.md)/[0119](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0119-durable-object-query-lowering.md))*
The same combinator names form a **lazy** query over a `store` `Map[K, V]` field —
dispatched by **receiver provenance** (ADR 0110, generalised from op-set to
evaluation strategy). A chain rooted in a store map is lazy: a builder lifts the
map's **values** into a `Query[V]` (`reservations.filter(r => …)`) and chains
build further `Query`s; a **terminal** executes it and is **`Effect`-typed**
(`.collect() -> Effect[List[V]]`, awaited with `<-`), folding into the storage
capability the `store` fields carry — no new `given`. **`Query[T]` is a
first-class, by-reference, non-storable and non-boundary type** (like
`Effect`/`Fn`): nameable in a pure helper's return, passable as an argument, but
rejected in any storable or boundary position (`bynk.types.query_at_boundary`).
`flatMap`'s function returns a `Query` over storage (`T -> Query[U]`). Joins and
`groupBy` arrive with a later slice. A query is **agent-local** (it reaches only
the owning agent's storage) and reads **staged** state (read-your-writes); it
lowers to a scan over the in-memory `Record` of the wholesale-persisted map, or
to an **index lookup** when an `@indexed` field routes it (below).

*(v0.93, [ADR 0118](https://github.com/bynk-lang/bynk/blob/main/design/decisions/0118-indexed-indexing-model.md))*
A `store Map[K, V]` field may carry `@indexed(by: f, …)` to maintain a **secondary
index** on one or more of its value type's fields. Each `by:` target MUST name a
**value-keyable field** of `V` (the `Map`-key rule — `String`/`Int`, incl.
refined/opaque over them); a non-`by:` argument is `bynk.index.bad_argument`, a
field the value type lacks is `bynk.index.unknown_key`, and a non-keyable field
is `bynk.index.unkeyable_key`. The runtime maintains a sibling posting-list
`Record` per indexed field **inside the same atomic commit** (ADR 0109) as the
map it indexes — re-indexed on every `put`/`update`/`upsert`/`remove`
(last-write-wins). An **equality `filter` directly on the map**
(`reservations.filter(r => r.f == v)`, with `v` not mentioning the row) **routes**
to a posting-list lookup instead of a scan; any other predicate (a comparison, a
compound condition, a filter deeper in a chain) still scans. **Index hygiene is
build-time *warnings*** (non-failing, ADR 0117): an equality filter on a
non-indexed keyable field is `bynk.index.missing` (add the index), and a declared
index no equality filter routes through is `bynk.index.unused` (it costs
maintenance on every write). Under the wholesale-`Record` representation the index
is a **CPU** optimisation (the map loads whole regardless); the I/O scaling awaits
per-entry storage keys. The most-selective tie-break, and a note when two indexes
could serve one filter, arrive with compound-predicate routing (a later slice).

**Keys.** A `Map` key type MUST be value-keyable — `String`, `Int`, or a
refined/opaque type over them; anything else is
`bynk.types.unkeyable_map_key`, checked at every written `Map[K, V]`
reference. A type parameter is admitted in key position: it can only be
instantiated through a concrete reference elsewhere, which is checked.

**Order.** A `List` is ordered by construction. A `Map` is
**insertion-ordered**, normatively: `keys()` enumerates in insertion order,
and `insert` on an existing key updates in place, keeping its position.

**Boundaries.** Collections serialise: a handler may take or return a
`List` or `Map`, and both may appear in record fields, sum payloads, agent
state, and capability signatures. The function-type confinement of
[§5.8](#58-boundaries--cross-context) **looks through** collections — a
`List[Int -> Int]` in a boundary position is still
`bynk.types.function_at_boundary`. The wire forms are
[§7.3.7](/book/spec/emission/#737-collections).

**The combinator stdlib.** Everything derivable from the kernel is ordinary
Bynk in the first-party `bynk.list` / `bynk.map` commons
([§8.4](/book/spec/compilation-model/#84-build-pipeline--conformance-to-typescript)),
imported with `uses bynk.list` like any commons: `map`, `filter`, `find`,
`any`, `all`, `reverse`, `traverse` (sequential); `values`, `contains`,
`getOr`. There is no `Map.fromList` — Bynk has no pair type to spell its
argument with; maps build via `Map.empty()` + `insert` folds.

> **Deprecated (v0.91, ADR 0116 D6).** The free functions whose `List` method
> forms exist — `map`, `filter`, `find`, `any`, `all` — emit a non-failing
> `bynk.list.deprecated_function` warning at each call site (the build still
> succeeds; [§9](/book/spec/diagnostics/)), with a machine-applicable fix to the method
> form. They still work; `reverse` and `traverse` keep their free-function form.

```bynk
context jobs

uses bynk.list

capability Clock {
	fn now() -> Effect[Int]
}

provides Clock = FixedClock {
	fn now() -> Effect[Int] {
		42
	}
}

service stamps {
	on call(names: List[String]) -> Effect[Result[List[Int], ()]]
			given Clock {
		let stamped <- traverse(names, (name) => Clock.now())
		Ok(stamped)
	}
}
```

## §5.11 Message bundles (v0.228–v0.234) {#messages}

A **message bundle** maps message codes to templates, one `messages` block
([§4.1.13](/book/spec/syntactic-grammar/#4113-messages_decl)) per locale. A
`messages` block MUST be declared in a commons (`bynk.messages.outside_commons`),
and that commons MUST `uses bynk.locale` and `uses bynk.locale.types`
(`bynk.messages.missing_locale_dependency`). The blocks of one commons, across
all its files, form one bundle, and the following rules hold over the bundle:

- Each block's tag MUST be a valid `LocaleTag`, a well-formed BCP-47 tag in
  canonical case (`bynk.messages.invalid_locale_tag`), and no tag may be
  declared twice (`bynk.resolve.duplicate_message_locale`).
- Exactly one block MUST carry `@reference` (`bynk.messages.missing_reference`,
  `bynk.messages.multiple_reference`).
- Every other locale MUST declare every code the reference declares
  (`bynk.messages.incomplete`).
- A template is ICU MessageFormat and MUST parse as the supported subset of it
  (`bynk.messages.malformed_icu_syntax`). For each code, every locale's template
  MUST use the same set of placeholder names, in any order
  (`bynk.messages.placeholder_mismatch`), each with the same ICU format kind
  (`bynk.messages.format_mismatch`).

The bundle gives its commons a generated
`render(tag: LocaleTag, msg: Message) -> String`. `render` is total: it uses the
template for `msg.code` in `tag`'s block, else in the reference block, else
renders the code and its parameters; a placeholder with no matching parameter is
left as its literal text. Nothing checks that a `Message` built at a call site
supplies the parameters its code's templates name.

**The `Locale` capability** (v0.221). `Locale` is a first-party capability of the
`bynk` adapter, `current() -> Effect[LocaleTag]`. On Cloudflare, in a context
whose direct `uses` reach exactly one message-bundle commons, it negotiates the
request's `Accept-Language` against that bundle's locales and falls back to its
reference locale; otherwise it returns `"en"`. A context that consumes `Locale`
and whose direct `uses` reach two or more bundles is
`bynk.locale.multiple_message_bundles`.

## §5.12 Static assets {#assets}

An **asset** declaration, `asset name = "path"`
([§4.1.12a](/book/spec/syntactic-grammar/#4112a-asset_decl)), names one static
file the platform serves beside a Worker, at a content-hashed URL, without
running the Worker. An asset is therefore public whatever the actors of the
declaring context say.

- An `asset` MUST be declared in a context or a commons
  (`bynk.asset.outside_context_or_commons`).
- Its path is relative to the project root. It MUST NOT be empty, absolute
  (`/x`, `\x`, `C:/x`), or contain a `..` segment, under either separator
  (`bynk.asset.unsafe_path`); and it MUST name a file in the project's
  `assets/` folder (`bynk.asset.outside_assets_dir`), so a declaration can
  never publish `.env` or a source file. `./` prefixes, repeated separators and
  `\` separators are normalised (`./assets//board.css` is `assets/board.css`).
- The file MUST exist as a regular file in that folder; a symbolic link is not
  followed (`bynk.asset.missing_file`), and it MUST be readable
  (`bynk.asset.unreadable_file`). A check with no filesystem (an in-memory
  check) skips this; a build with none treats the folder as empty. Only the
  files a declaration names are read; other files in the folder are not.
- Its name MUST NOT be declared twice in one unit (`bynk.asset.duplicate`), and
  MUST NOT equal the name of another declaration in the unit's namespace
  ([§5.1](#51-name-resolution--visibility)): a function, type, event, service,
  agent or provider (`bynk.resolve.name_conflict`).
- A unit sees its own assets and those of each commons it `uses`, one level, as
  with functions; a local declaration wins. Two used commons that declare the
  same name are `bynk.uses.name_conflict`.

In an expression, an asset's name has type `Asset`. `Asset` is a built-in,
non-generic type with one field, `url : String`; any other field is
`bynk.types.unknown_field`. `Asset` is reserved as a built-in type name, so
`type Asset` is `bynk.resolve.reserved_builtin_type`. An `Asset` is not data:
it MUST NOT appear in a record field, a sum-variant payload, a boundary
position, or an agent `store` field (`bynk.types.asset_at_boundary`); its `.url`
may.

**Fixed paths** (`asset favicon = "assets/favicon.ico" at "/favicon.ico"`), for
clients that fetch a file by name without reading a page:

- The `at` path MUST be a URL path naming one file: starting with `/`, not
  ending in `/`, with no empty, `.` or `..` segment, only `[A-Za-z0-9._~-]`
  between slashes, not under `/_assets/` or `/_bynk/`, and not one of the root
  files the platform reads as configuration (`/_headers`, `/_redirects`,
  `/.assetsignore`) (`bynk.asset.invalid_at_path`). The asset's `.url` is that
  path.
- A Worker MUST NOT serve two different files at one fixed path, nor files at
  two fixed paths where one is a folder of the other (`/logo` and
  `/logo/icon.png`) (`bynk.asset.path_conflict`).
- The platform answers a fixed path before the Worker runs, for every method.
  So a context's HTTP route equal to a fixed path its Worker serves is
  unreachable (`bynk.asset.shadows_route`), and a parameterised route that
  matches one (`/:page`) no longer sees that path (the warning
  `bynk.asset.may_shadow_route`).
- HTTP routes MUST NOT use the `/_assets/` prefix, where content-hashed assets
  are served (`bynk.http.reserved_prefix`).

**Translation.** A content-hashed asset's `.url` is
`/_assets/<stem>.<hash>.<ext>`: `<hash>` is the FNV-1a 64 of the file's
content as 16 lowercase hex digits, `<stem>` and `<ext>` the file name's,
split at its last `.` (no `.<ext>` when it has none), with any character
outside `[A-Za-z0-9._-]` replaced by `-`. It depends only on the name and the
content, so builds are reproducible, and a content change changes the URL. The
declaring unit's module exports the asset as a constant of TypeScript type
`{ readonly url: string }`; a unit that uses it imports it like a function.

On the Workers target, each context's Worker ships the content-hashed files
declared by every unit it reaches over `uses` **and** `consumes`, transitively —
wider than name visibility, since a used commons, or a consumed context's
service, may hand back another unit's `.url`, which then resolves against this
Worker. Fixed-path files ship only over `uses`: they belong to the root of the
Worker that declares them. They are
written to the Worker's `assets/` directory, beside (never containing) its
code, under their served paths, with a `_headers` file giving `/_assets/*`
`Cache-Control: public, max-age=31536000, immutable`. Its `wrangler.toml`
gains an `[assets]` block with `run_worker_first = false` (a matching asset is
served without running the Worker), `not_found_handling = "none"` (any other
request reaches the Worker) and `html_handling = "none"` (an asset is served
only at its exact path). A Worker with no assets gets none of these. The Bundle target ships no
files, so a deployable Bundle build (one without tests) warns about each
asset whose `.url` it emits (`bynk.asset.not_served`). `bynk deploy` records each Worker's served paths
in `bynk.deploy.lock`.

**Single files.** A single file has no project root and no Worker, so building
one that declares an asset is `bynk.asset.needs_project`. Checking it is
unaffected.

**In suites.** A case sees exactly the assets the target's Worker ships: the
target's own and its `uses`d commons' (content-hashed and fixed-path), plus the
content-hashed assets of the contexts it consumes. A consumed context's
fixed-path assets, and those of a `system` suite's own `uses` commons, are not
nameable, nor is an asset the target hides behind its own declaration of the
same name ([§5.1](#51-name-resolution--visibility)); naming it in a unit suite
is `bynk.uses.name_conflict`, as in the target. Its file still ships, since the
commons that declares it may use it. `svc.GET(a)`, for
an `Asset` `a` and any `from http` service `svc` of the target (any other
service is `bynk.test.service_bad_address`), fetches `a` from the asset tier
rather than a handler: it yields `Raw(body, contentType)` when the target's
Worker ships `a.url` (with the content type the platform sends for the file's
extension), and `NotFound` otherwise. The method MUST be `GET`
(`bynk.test.asset_address_method`), the asset MUST be the only argument
(`bynk.test.service_call_arity`), and the call MUST NOT carry a `by`
(`bynk.test.principal_on_asset`).

## §5.12a Embedded text {#embeds}

An **embed** declaration, `embed name = "path"`
([§4.1.12b](/book/spec/syntactic-grammar/#4112b-embed_decl)), names one project
file whose text is the value of `name`: a `String` constant compiled into the
code. Unlike an asset ([§5.12](#assets)) an embed is never served, so it is
private to the code that reads it.

- An `embed` MUST be declared in a context or a commons
  (`bynk.embed.outside_context_or_commons`).
- Its path is relative to the project root. It MUST NOT be empty, absolute
  (`/x`, `\x`, `C:/x`), or contain a `..` segment, under either separator
  (`bynk.embed.unsafe_path`); and it MUST name a file in the project's
  `embeds/` folder (`bynk.embed.outside_embeds_dir`), so a declaration can
  never pull `.env` or a source file into the deployed code. `./` prefixes,
  repeated separators and `\` separators are normalised.
- Its name MUST NOT be declared twice in one unit (`bynk.embed.duplicate`), and
  MUST NOT equal the name of another declaration in the unit's namespace
  ([§5.1](#51-name-resolution--visibility)): an asset, a function, a type, an
  event, a service, an agent or a provider (`bynk.resolve.name_conflict`).
- A unit sees its own embeds and those of each commons it `uses`, one level, as
  with functions; a local declaration wins. Two used commons that declare the
  same name are `bynk.uses.name_conflict`. A package dependency MUST NOT
  declare an `embed`, as with an asset.

In an expression, an embed's name has type `String`, and its value is the text
of the file it names.

- The file MUST exist as a regular file in `embeds/` (`bynk.embed.missing_file`);
  a symbolic link is not followed. It MUST be readable
  (`bynk.embed.unreadable_file`), valid UTF-8 (`bynk.embed.invalid_encoding`), and free
  of NUL bytes (`bynk.embed.binary_content`). A check with no filesystem (an
  in-memory check) skips the existence check; a build with none treats the
  folder as empty. Only the files a declaration names are read.
- It MUST NOT exceed 256 KiB, and the files the project's declarations name
  MUST NOT total more than 1 MiB, each file counted once
  (`bynk.embed.too_large`).
- The text is the file's contents verbatim, except that a leading byte-order
  mark is dropped and `\r\n` and a bare `\r` are each replaced by `\n`, so the
  same source builds to the same value on every checkout. A trailing newline is
  kept. The text has no interpretation: `\(` and `{{ }}` in the file mean
  nothing to the language.

An embed is never served. It is emitted as an exported `string` constant of the
declaring module, imported like a function by a unit that `uses` it; a bundler
drops one that nothing references. A unit-tier `suite` case names the embeds of
its target and of the commons the target `uses`. A single file has no project
root to find `embeds/` under, so building one that declares an embed is
`bynk.embed.needs_project`; checking it is unaffected.
