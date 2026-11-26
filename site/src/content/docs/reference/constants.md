---
title: Named constants
---
A context or a commons names a fixed value with a `const` declaration. The
constant has the type its annotation declares and can be used wherever an
expression of that type can. See the
[grammar for `const_decl`](/reference/grammar/#rule-const_decl) and
[§5.12b of the specification](/reference/spec/static-semantics/#constants).

## Declaring a constant

```bynk,fragment
commons shop.limits

---
How many times a request is retried.
---
const MaxRetries: Int = 3

const Timeout: Duration = 30.seconds

const Greeting: String = "hello"

const PageSize: Int = 50

const DefaultPage: Page = Page {
	size: PageSize,
	offset: 0,
}

type Page = {
	size: Int,
	offset: Int,
}

fn attempts() -> Int {
	MaxRetries + 1
}
```

- **Type:** always written. `const MaxRetries = 3` is
  `bynk.parse.const_missing_type`.
- **Name:** upper-camel case, like a variant (`MaxRetries`, not `maxRetries`:
  `bynk.resolve.const_name_case`). A constant shares the unit's one namespace
  with functions, assets, embeds, types, events, services, agents and
  providers, so it may not repeat a constant (`bynk.resolve.duplicate_const`)
  or reuse one of those names (`bynk.resolve.name_conflict`). Nor may it take a
  built-in type's name or a variant's name, since a bare reference would read
  as either.
- **Where:** in a context or a commons, never an adapter
  (`bynk.resolve.const_outside_context_or_commons`). A unit sees its own
  constants and those of each commons it `uses`, like functions, and a sibling
  file of the same unit sees them too. A `let` binding or parameter of the same
  name shadows a constant.
- **Visibility:** public by default. `private const` keeps a constant within
  its own unit: its files and its suites see it, a unit that `uses` the
  commons does not (a reference from one is `bynk.visibility.private_item`).
  A public constant may not be typed with a private type of its unit
  (`bynk.visibility.private_in_public`). See
  [Design a library's public surface](/book/guides/program-structure/design-a-public-surface/).

## What an initialiser may be

The value is fixed when the module loads, so an initialiser is a *static*
expression: one that needs nothing evaluated at run time.

| Allowed | Example |
|---|---|
| a literal, including a negative number | `3`, `-1`, `0.5`, `true`, `"hello"`, `()` |
| a `Duration` literal | `30.seconds`, `1.minute` (singular with `1`, plural otherwise) |
| a record, list or variant over static values | `Page { size: 50, offset: 0 }`, `[1, 2, 3]`, `Circle(2)`, `Colour.Red` |
| `Some`, `None`, `Ok`, `Err` over static values | `Some(MaxRetries)` |
| an opaque type's `T.unsafe(lit)`, in its defining commons | `UserId.unsafe("admin")` |
| an empty collection | `List.empty()`, `Map.empty()`, `Set.empty()` |
| another constant | `MaxRetries` |

Anything else is `bynk.resolve.bad_const_initialiser`: a function call (even a
pure one), an operator (`1 + 2`), an interpolated string, a field access, a
lambda, `if` or `match`. The initialiser is then checked against the declared
type, so a literal is admitted to a refined type only when it satisfies the
refinement:

```bynk,fragment
commons net

type Port = Int where InRange(1, 65535)

const DefaultPort: Port = 8080
```

`const DefaultPort: Port = 0` is `bynk.refine.literal_violates`. A `Duration` literal is
admitted to a refined `Duration` the same way: with
`type Ttl = Duration where AtLeast(60.seconds)`, `const Retention: Ttl = 30.days` is
checked when the program compiles, and `const Short: Ttl = 30.seconds` is
`bynk.refine.literal_violates`.

Constants may refer to each other in any order, across files and through
`uses`, but not in a cycle: `const A: Int = B` with `const B: Int = A` is
`bynk.resolve.const_cycle`, which names the members of the cycle. The files
of a unit may import each other freely: `a.bynk` can declare `const A: Int = 1`
and `const C: Int = B` while `b.bynk` declares `const B: Int = A`, and either
file's functions can call the other's.

A constant can stand in a refinement predicate's argument
(`type Qty = Int where InRange(1, MaxQty)`), and in an annotation argument that
takes a literal of its type, when it is declared exactly that type (not a refined
one) and bound to a literal or to another such constant. In a predicate that
covers `InRange`, `AtLeast`, `AtMost`, `MinLength`, `MaxLength`, `Length` and
`Matches`, and the constant may come from a commons the unit `uses`; a constant
that does not meet the rule is `bynk.refine.bound_not_literal`, and one declared
the refined type whose bound it is is `bynk.resolve.const_cycle`. In an
annotation: `@name(VisitsCookie)`
([`@name`](/reference/types/#field-name)), `@discriminator(EventKey)`, `@ttl(SessionTtl)`,
`@retain`, `@cache(maxAge: Fresh, immutable: Pinned)`,
`@limit(maxBody: MaxUpload)`, `@schema`, `@raw(contentType: Png)` and
`@example`, and in a derived capability instance's options
(`Service @worker(BillingWorker)`, `Queue @name(OrdersQueue)`,
`RateLimit @limit(PerMinute)`; see
[Derived capabilities](/reference/capabilities/#derived-capabilities)). The compiler replaces the reference with the literal before it
reads the annotation, so the value is written once and checked as the literal
would be. A keyword argument such as `@cache(scope: public)` is a word, not a
value, and stays as written.

The `schedule(…)` of a cron handler (and a test's `svc.schedule(…)` address) takes
a constant too, and there the rule is the type the position expects rather than
"exactly the base type": a constant declared `String`, or exactly the refined
`CronSchedule`, is accepted, and the literal it resolves to is checked against
`Cron`. A constant of another refined type is refused, which keeps named types
distinct inside a context. See [Cron](/reference/cron/#naming-a-schedule-with-a-constant).

The address of a `from email(…)` service takes a constant too, declared exactly
`String` (there is no refined address type) and bound to a literal or to another
such constant; the literal it resolves to is checked as an address is. See
[Inbound email](/reference/inbound-email/#naming-an-address-with-a-constant).

## What it becomes

A constant is an exported TypeScript function of the declaring module, typed by
its annotation, that computes its value on the first call and returns the cached
value after that:

```typescript
var __const_MaxRetries: { v: number } | undefined;
export function MaxRetries(): number { return (__const_MaxRetries ??= { v: 3 }).v; }
```

Every reference to the constant is a call, `MaxRetries()`, and a sibling file or
a `uses`-ing unit imports it by name like any other function. Because a function
declaration is hoisted, a constant can be read whichever order the modules of a
unit load in, even when its files import each other. Constants are emitted
after the module's types and before its functions. A unit-tier `suite` case can
name its target's constants, private ones included, and the public constants of
the commons the target `uses`. `private` changes nothing in the emitted module:
it is a naming rule only.

## Values that differ per environment

A constant's initialiser may be `env(NAME)` or `env(NAME, default)`. The value is
read from `bynk.toml` when the project is built, and the constant is an ordinary
one from then on: everything that accepts a constant accepts it.

```bynk,fragment
const AppOrigin: String = env(APP_ORIGIN)                              // required
const BillingWorker: String = env(BILLING_WORKER, "legacy-billing")   // a default
```

```toml
# bynk.toml
[vars]                                   # every environment, `default` included
APP_ORIGIN = "https://app.shop.example"

[env.staging.vars]                       # this environment, over `[vars]`
BILLING_WORKER = "legacy-billing-staging"
```

These are build-time values, read from the committed `bynk.toml` and from
nothing else, never from the Worker's run-time environment or the build
machine's, so a deploy depends only on committed configuration. They are not
Wrangler's run-time `[vars]`, and a suite's `env { … }` (run-time, per suite or
case) keeps its own name and meaning.

- The value for the environment being built is its own `[env.<name>.vars]`,
  then `[vars]`, then the constant's default. With no default, every
  environment must give one (`bynk.env.missing`).
- `env(…)` is only ever a constant's whole initialiser, so a per-environment
  value is always a named constant (`bynk.env.not_initialiser`). `NAME` is
  written bare, capital letters, digits and `_` (`bynk.env.malformed_call`).
- The supported types are `String`, `Int`, `Bool`, `List[String]` and refinements
  over the first two declared in the same unit (`bynk.env.unsupported_type`). A
  value of another kind is `bynk.env.bad_value`; one that breaks the refinement
  is refused where the literal it resolves to is checked.
- A variable in `bynk.toml` that no constant reads is an error
  (`bynk.env.unknown_variable`), so a typo cannot leave a default in force; two
  constants reading one variable must give it the same default
  (`bynk.env.conflicting_default`).
- `bynk deploy --env <name>` builds with that environment's values, and the plan
  lists every `env(…)` constant with its resolved value and where it came from
  (`[env.<name>.vars]`, `[vars]` or the constant's default). A named environment
  that leaves a constant to `[vars]` or its default would use another
  environment's setting, so the plan lists it as `shared with default`, as it
  does a `Service @worker` or `Email @to` / `@allowedRecipients` whose value
  equals `default`'s, in text and in `--format json`. Setting the variable under
  `[env.<name>.vars]`, even to the same value, records the sharing as deliberate.
  `--strict-environments` refuses it.
- A package consumed as a dependency cannot declare an `env(…)` constant
  (`bynk.env.in_dependency`): reading deployment configuration is the
  application's business, and a library takes values as parameters.
- `bynk.toml` is committed, so a variable named `*_KEY`, `*_SECRET`, `*_TOKEN` or
  `*_PASSWORD` warns (`bynk.env.secret_like_name`): keep secrets in `Secrets`.

A plain build, `bynk check`, `bynk test` and `bynk dev` use the `default`
environment's values. `--env <name>` on `bynk check`, `build`, `compile`, `test`
and `dev`, as on `bynk deploy`, builds with another environment's: a required
variable with no value in that environment (and none in `[vars]`) is
`bynk.env.missing` there, so `bynk check --env staging` finds it before a deploy
does. A suite's `env { … }` is run-time and unaffected.

In the editor, hovering `env(NAME)` lists what each environment resolves the
variable to and where each value comes from; completing inside `env(` offers the
variables `bynk.toml` defines, with their `default` value; and signature help
shows `env(NAME, default)`. The variable name is a `property` semantic token.
Renaming it (F2 on the name) renames it in every constant that reads it and in
each `[vars]` and `[env.<name>.vars]` table of `bynk.toml` that sets it, and is
refused when the new name is already a variable or when a `bynk.toml` key is
written in a form the rename cannot edit (a dotted key, an inline table). Hover
and completion read `bynk.toml` from disk, so a saved change to it shows on the
next request.
