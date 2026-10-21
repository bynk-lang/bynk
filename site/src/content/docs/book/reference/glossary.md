---
title: Glossary
---
Terse, exact definitions of Bynk's load-bearing terms. Each links to its fuller
reference page where one exists.

### agent {#term-agent}

A keyed, stateful entity whose state lives in `store` fields and changes only
through writes inside its handlers. See [Agents](/book/reference/agents/).

### context {#term-context}

A bounded context: an isolated module with its own services, agents, and provided
capabilities, reachable only across an explicit boundary.

### service {#term-service}

A named group of handlers (`on call`, `from http`, `from cron`, `from queue`) declared
inside a context. See [HTTP](/book/reference/http/).

### capability {#term-capability}

An interface of effectful operations a context may depend on. See
[Capabilities & providers](/book/reference/capabilities/).

### provider {#term-provider}

A `provides` block implementing a capability, optionally `given` other
capabilities it uses. See [Capabilities & providers](/book/reference/capabilities/).

### commit {#term-commit}

The atomic persistence of a handler's `store` writes. A handler's `:=` writes are
staged and committed together when it returns, after invariants are checked; a
faulting handler commits nothing. See [Agents](/book/reference/agents/).

### branded type {#term-branded-type}

A compiled type carrying a unique tag so values of distinct Bynk types cannot be
interchanged in the emitted TypeScript; how opaque types stay distinct. See
[Emission](/docs/emission/).

### opaque type {#term-opaque-type}

A type whose representation is hidden outside its defining module; constructed and
inspected only through its API. See [Type system](/book/reference/types/).

### refined type {#term-refined-type}

A base or named type narrowed by a `where` predicate, e.g. `Int where Positive`.
See [Refined-type API](/book/reference/refined-types/).

### sum type {#term-sum-type}

A tagged union of variants, each optionally carrying a payload. See
[Type system](/book/reference/types/).

### record {#term-record}

A product type of named fields, each with a type and optional default. See
[Type system](/book/reference/types/).

### enum {#term-enum}

A sum type whose variants all carry no payload. See [Type system](/book/reference/types/).

### refinement predicate {#term-refinement-predicate}

A built-in constraint used in a `where` clause (`Positive`, `NonNegative`,
`InRange`, `Matches`, `MinLength`, …). See [Refined-type API](/book/reference/refined-types/).

### admission {#term-admission}

The compile-time rule by which a literal that provably satisfies a refined type's
predicate is accepted directly (lowering to an inline brand cast), with no
`Result`. See [Refined-type API](/book/reference/refined-types/).

### `.of` / `.unsafe` {#term-of-unsafe}

`.of` validates at run time and returns a `Result` — the constructor for both
refined and opaque types. `.unsafe` constructs without a check and is
**opaque-only**, usable within the opaque type's defining `commons`; a refined
type has no `.unsafe` (ADR 0182). See
[Refined-type API](/book/reference/refined-types/).

### zeroable {#term-zeroable}

A type with a defined implicit zero value, letting an agent state field omit an
initialiser. See [Agents](/book/reference/agents/).

### `Effect` {#term-effect}

The type of a computation that performs effects; produced by handlers and
capability operations and sequenced with `<-`.

### `Result` / `Option` {#term-result-option}

Errors-as-values types: `Result` is `Ok` or `Err`; `Option` is `Some` or `None`.
See [Type system](/book/reference/types/).

### `Duration` {#term-duration}

A base type for a span of time, written as a unit literal (`5.minutes`); composes
with `Instant`. See [Type system](/book/reference/types/#duration).

### `Instant` {#term-instant}

A base type for an absolute point in time, minted by `Clock.now()`; no literal,
orderable but not numeric, advanced by a `Duration`. See
[Type system](/book/reference/types/#instant).

### `Query` {#term-query}

A lazy read over a `store`'s storage, carrying the same combinator vocabulary as
the eager `List` methods but dispatched by receiver provenance; non-storable and
non-boundary. See [Type system](/book/reference/types/#query).

### `Val[T]` {#term-val}

A test-only expression that fabricates a valid inhabitant of type `T` drawn from
its refinement domain, optionally pinned to a chosen value (`Val[T](v)`,
refinement-checked at compile time). Replaces the retired `Mock[T]` (v0.114). See
[Testing](/book/reference/testing/).

### `property` {#term-property}

A generative test, the sibling of `case` in a `suite`: `for all x: T` binds `x` to
a *generated* inhabitant of `T` and the body's `expect`s must hold across many. On
failure it reports a shrunk counterexample and a seed to reproduce. See
[Testing](/book/reference/testing/).

### contract {#term-contract}

A function's `requires` (precondition) and `ensures` (postcondition) clauses — the
invariant predicate attached to a pure `fn`. Checked at every call in the dev/test
build and generated against by the runner; stripped from the deploy build. A
contract is a property that is always on. See
[Contracts](/book/reference/testing/#contracts).

### `requires` {#term-requires}

A function precondition: `requires <name>: <pred>`, a pure `Bool` over the
parameters. Guards the call and filters the runner's generated arguments (like a
`for all … where`). `result` is not in scope in a `requires`.

### `ensures` {#term-ensures}

A function postcondition: `ensures <name>: <pred>`, a pure `Bool` over the
parameters and `result`. Checked at every call and generated against by the runner.

### `result` {#term-result}

The contextual binding for a function's return value, in scope only inside an
`ensures` predicate (the awaited element for an `Effect` return). Everywhere else
`result` is an ordinary identifier.

### `transition` {#term-transition}

An agent **step invariant**: `transition <name>: <pred over old/new>`, the invariant
predicate widened from a single committed state to the *move* between two. Declared
beside an agent's `invariant`s and checked at the commit boundary (from the second
commit onward). Unlike a `contract`, it is not attacked by the runner — a fabricated
agent state is valid but not necessarily reachable.

### `old` / `new` {#term-old-new}

The contextual bindings inside a `transition` predicate: `old` is the last committed
state, `new` the state the current commit would persist — each the agent's state
record (`old.status`, `new.balance`). Special only inside a `transition`; everywhere
else `old`/`new` are ordinary identifiers.

### observation {#term-observation}

An `expect` over a capability seam, inside a `case` — asserting *interaction*
(that `Cap.op` was called, with what arguments, how often, in what order) rather
than a value. The sugar forms: `called`, `never called`, `called once` / `called
<n> times`, `called … with <pred>`, and `A.op before B.op`. Calls are recorded
automatically at the seam in the test build, so a pure-observation case needs no
`stub`. A `with <pred>` is the invariant predicate over the operation's
parameters, in scope by name.

### `trace` {#term-trace}

The observation escape hatch: `trace(Cap.op)` yields the recorded calls of a
capability operation as a `List` of per-operation records (fields = the operation's
parameters), in call order, asserted with the ordinary `List` surface. A test-only
builtin — an ordinary identifier outside a `case`.

### `Cap.op` subject {#term-cap-op-subject}

The subject of an observation: a capability and one of its operations, *named, not
called* (no argument list) — e.g. `Logger.log` in `expect Logger.log called`.

### tier {#term-tier}

How much of the real world is present when a `case` runs, set by an `as <tier>`
clause on the `case` (or `suite`) header — one of `unit` / `integration` /
`system`. A tier is **one body promoted, not a distinct kind of test**: promotion
changes only the header, never the body. `as` on the suite sets a default the case
may override (case wins); tiers are a `case`-only affordance (a `property` has no
tier, and its header has no place for one). See
[Test tiers](/book/guides/testing/integration/).

### `unit` / `integration` / `system` {#term-tiers}

The three tiers, borrowed from the testing pyramid. **`unit`** (the default,
elided) runs the unit in process with collaborators you control seam by seam
(`stub`). **`integration`** runs its real collaborators **within one context,
no wire**. **`system`** stands contexts up as the Workers they deploy as and wires
them across the real serialise → JSON → deserialise edge; a `system` case must span
≥ 2 contexts (`bynk.tier.system_needs_wire`). Participants for `integration` /
`system` are **inferred** from the `consumes` graph — there is no `wires` clause.

### `stub` (test double) {#term-stub}

A per-seam test-time provider override: `stub Cap.method(<pattern>) returns
<value> | fails`, at case or suite scope (precedence: case > suite > tier default).
The left is a call pattern (the one predicate surface — `_`, literals, `is`, first
match wins); the right is a *value* or `fails`, **never a computed body**. The
sequenced form `returns each [<outcome>, …]` supplies one outcome per call, the
last repeating. Capability-only. It substitutes a consumed seam of the unit under
test — the *supply* leg of the seam triad (`consumes` declares, `given` requires,
`stub` substitutes). Its own keyword since the keyword-hygiene batch (#548);
formerly a pun on `provides`. Replaces the retired `mocks` block (v0.118), after
which "mock" is gone from the language. See [`stub`](/book/reference/testing/#stub).

### project vs legacy mode {#term-project-vs-legacy-mode}

*Project mode* is a `bynk.toml`-driven directory layout (a `src`/`tests` split,
`bynkc test`); *legacy mode* compiles a single `.bynk` file as a standalone unit,
with no manifest. See [`bynk.toml` manifest](/docs/manifest/).

### package {#term-package}

A project with a `[package]` identity, `organisation/package` (`acme/money`),
that can be shared with other projects. A package shares its commons and
adapters, never a context. Another project depends on it under
`[dependencies]` and reaches its units through an alias. See
[`[package]`](/docs/manifest/#package) and
[Share code between projects](/book/guides/projects-build-and-deployment/share-code-between-projects/).

### dependency {#term-dependency}

A package a project uses, declared under `[dependencies]`. Path dependencies
resolve today; git and registry dependencies come later. Only the dependency's
exported units are reachable, and only the ones the project reaches are
compiled. See [`[dependencies]`](/docs/manifest/#dependencies).

### lock (`bynk.lock`) {#term-lock}

The file `bynk` writes beside `bynk.toml` (or at a workspace root) pinning a
project's resolved dependencies: each git dependency's commit and a checksum
of its files, and each path dependency's directory. Commit it; `--locked`
builds exactly what it says. See [`bynk.lock`](/docs/manifest/#bynklock).

### store {#term-store}

The per-user cache (`$BYNK_HOME/store`) holding each fetched git dependency's
files, named by their checksum and read-only. Only `bynk` writes it; `bynkc`
and the editor read it and never fetch.

### alias {#term-alias}

The key of a `[dependencies]` entry: the name source reaches that dependency's
units by, as `<alias>.<unit>` (`uses money.currency`). A dependency's units are
never reachable by their bare names, so adding one never changes what an
existing name means.

### workspace {#term-workspace}

A directory whose `bynk.toml` holds `[workspace]`: a group of projects (its
members) that `bynk check`, `fmt` and `test` run across from the root. A
workspace isn't a project and has no identity. See
[Workspaces](/book/guides/projects-build-and-deployment/layout/#workspaces).

### member {#term-member}

A project that belongs to a workspace: a directory the workspace's `members`
patterns match, minus its `exclude` patterns, holding its own `bynk.toml`.

### registry {#term-registry}

A server, or a static file tree, that serves packages by
`organisation/package`. A project routes each organisation to one registry in
`[registries]`, or to the first-party registry by default. Configuration only
for now: nothing fetches yet. See [`[registries]`](/docs/manifest/#registries).
