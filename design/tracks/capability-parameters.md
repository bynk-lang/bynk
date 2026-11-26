# Capability parameters — a commons function uses a capability its caller binds

- **Status:** Settling in the PR that adds this doc. Every maintainer ruling
  was given on 26 November 2026, and Q2's fork was closed by
  [ADR 0990](../decisions/0990-commons-consumes-types.md) Decision E (§14).
  Spine issue: #3959, which carries the live slice status.
- **Realises:** the step [ADR 0726](../decisions/0726-given-on-context-functions.md)
  deferred (#2868 Decision A: "A later step could add capability parameters to
  commons functions"), and the capability review's carried-over item,
  capability-polymorphic functions in a commons.
- **Posture:** Feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md), run per
  [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md). It
  qualifies on two axes (§1.2). It is not a security boundary, but a binding
  error is a data-integrity hazard (§11).
- **Governing records:** [ADR 0726](../decisions/0726-given-on-context-functions.md)
  (`given` on context functions: the inclusion rule and the hidden `deps`
  parameter), [ADR 0469](../decisions/0469-derived-capabilities.md) (derived
  capabilities belong to the context that declares them),
  [ADR 0693](../decisions/0693-exec-context-in-functions.md) (the hidden
  execution-context parameter), [ADR 0011](../decisions/0011-consumes-capability-selection.md)
  (capability selection with `consumes U { Cap }`),
  [ADR 0990](../decisions/0990-commons-consumes-types.md) (a commons' braced,
  type-only `consumes`; Decision E reserves it for capability contracts),
  [ADR 0983](../decisions/0983-commons-names-prelude-types.md) (a commons
  reaches `bynk`'s transparent types without the clause, and no capability),
  [ADR 0977](../decisions/0977-effectful-suite-helpers.md) (a suite helper may
  declare `given`), [ADR 0974](../decisions/0974-test-only-commons.md) (a
  test-only commons),
  [ADR 0152](../decisions/0152-observation-is-auto-recorded-at-the-capability-seam.md)
  (observation at the capability seam). Two ADRs land up front (§13).
- **Code on `main`:** cited by symbol, never by line, as of `2f315d8c`. The
  book's reference pages are cited from `site/src/content/docs/reference/`.

## 1. The theme

### 1.1 What this track settles

A context's free functions can declare `given` and use capabilities (#2868).
A commons function can't. A commons is shared by contexts that may consume
different capabilities, so a library takes function values instead:
`log: (String) -> Effect[()]`, or a query lambda per call.

The real obstacle is instances. `D1`, `R2`, `Queue`, `Service`, `Analytics`
and most other Cloudflare kinds are used only through derived instances, and
only a context declares one (`capability Orders = D1`). A commons function
has no name for the instance it needs.

This track adds **capability parameters**. A commons function declares one
like a type parameter with a bound, `given Store: Kv`. Every call binds it to
one of the caller's own capabilities, by name:

```bynk
commons shop.cache

consumes bynk { Logger }
consumes bynk.cloudflare { Kv }

fn cached[T: Codable](key: String, load: () -> Effect[T]) -> Effect[T] given Store: Kv, Logger {
	let hit <- Store.get(key)
	match hit {
		Some(text) => match Json.decode[T](text) {
			Ok(v) => Effect.pure(v),
			Err(_) => refresh[T](key, load, given Store = Store),
		},
		None => refresh[T](key, load, given Store = Store),
	}
}
```

```bynk
context shop

consumes bynk { Logger }
consumes bynk.cloudflare { Kv }
uses shop.cache

capability Sessions = Kv

fn session(id: String) -> Effect[Session] given Sessions, Logger {
	cached[Session](id, () => loadSession(id), given Store = Sessions)
}
```

The track settles four things:

1. **The binding rule and its syntax.** Every call binds every capability
   parameter explicitly. That much is decided on the spine. This doc settles
   where the binding is written (§5).
2. **What a parameter's kind may be,** and which capabilities a commons may
   name bare as shared capabilities (§4, §6).
3. **How tests reach a parameter:** doubles in a commons suite, and what
   `stub` and `expect … called` name (§9).
4. **How the binding is emitted:** through the hidden `deps` parameter ADR
   0726 already uses (§8).

### 1.2 Why a track (the ADR 0076 trigger)

- **It spans several increments.** Declaration and in-body use; binding at the
  call and the inclusion rule; emission; forwarding through chains of commons
  functions beside type parameters; tests and doubles; tooling and the book.
- **Its surface was not settled when the track opened.** The binding syntax,
  the set of admissible kinds, how a commons names a kind at all, and how
  parameters meet `stub` and `expect … called` were open (§14, now settled).
- **It is not a security boundary** (§11). The integrity hazard of binding the
  wrong instance is real, which is why the binding is explicit.

## 2. What's true today

### 2.1 The language

- A free function in a context may declare `given` after its return type. A
  function in a commons may not (`bynk.given.fn_outside_context`;
  `reference/capabilities.md` §Capabilities in functions; spec,
  `reference/spec/static-semantics.md` §5.5). That holds for a test-only
  commons too (`commons x.y for tests`, ADR 0974, whose Consequences name the
  limit). A suite helper may now declare `given`, naming capabilities of the
  suite's target, and takes the case's `deps` (ADR 0977; spec §5.9).
- The caller's `given` must include the callee's, checked at each call and at
  each reference to the function as a value, so the rule holds transitively
  (`bynk.given.missing_for_call`; ADR 0726 B).
- A capability may be declared only in a context or an adapter
  (`bynk.capability.outside_context`; spec §5.5). A commons cannot declare a
  derived instance either (`bynk.capability.derived_outside_context`; ADR 0469
  D4).
- A commons may now write `consumes`, but only the braced list of **types** an
  adapter exports (ADR 0990; spec §5.8; `reference/grammar.md`
  §commons_body_item). A capability name in that list is refused with
  `bynk.commons.consumes_not_types`. Separately, a commons reaches `bynk`'s
  transparent types with no clause at all (ADR 0983), and writing an explicit
  `consumes bynk { … }` turns that implicit reach off for the file. **So a
  commons today can name `TimedOut` or `KvEntry`, but still not `Kv` or
  `Logger`**, not even as a bound. ADR 0990 Decision E reserves this same
  braced list for capability contracts, which is how this track opens it (Q2).
- A derived instance is an ordinary capability with its base's operations and
  its own resource. Each instance has its own fresh in-memory double under
  `bynk test` (`reference/capabilities.md` §Derived capabilities).
- Of `bynk.cloudflare`'s instantiable kinds, only `Kv` is also usable bare;
  `Service`, `R2`, `Queue`, `D1` and the rest are instance-only
  (`bynk.capability.instance_only`; spec §5.5). The registry records this as
  `Instantiable::bare` (`bynk-std`, the `bynk.cloudflare` unit's
  `instantiable` list), and kinds bound once per Worker (`Version`, `Ai`,
  `Images`, `AnalyticsSql`) as that unit's `singletons`; `bynk_std::instantiable`
  looks a kind up.
- A capability operation may be generic (`dedup[T]`), with an explicit type
  argument at the call (`reference/capabilities.md` §Generic operations). No
  capability kind itself takes a type argument; `Queue[Order]` was considered
  and rejected for now in #3879 (Decision A).
- `stub` and `expect … called` apply to a capability, not to a call site, so
  they observe calls made inside a context function or a `given` suite helper
  (ADR 0726, Tests; ADR 0977 C). Recording is ambient at the seam (ADR 0152).
- A suite may target a commons (`reference/grammar.md` §suite_decl). A commons
  has no capabilities, so a commons suite has no seams to stub today.

### 2.2 The compiler

- **Refusal.** `given_fn_refusal` (`bynk-check/src/checker.rs`) refuses
  `given` outside a context with `bynk.given.fn_outside_context`, and on a
  method, a non-`Effect` return or `Events` with `bynk.given.fn_unsupported`.
  Its first branch is the one slice 1 narrows for a commons.
- **Inclusion.** `require_callee_given` (`bynk-check/src/checker/calls.rs`)
  checks the callee's `given` against the caller's `given_remaining`, marks
  each as used, and reports `bynk.given.missing_for_call` with a
  machine-applicable fix. It returns early for a function in
  `imported_fn_names`, because a `given` on a commons function is always
  refused today. That early return is where slice 1 starts.
- **Syntax.** A `given` clause is a list of `CapRef`s (`bynk-syntax/src/ast.rs`),
  each a name or a qualified `B.Cap`, read by `parse_given_clause` and
  `parse_cap_ref` (`bynk-syntax/src/parser/declarations.rs`). On a function it
  follows the return type, before `where` and the contract clauses
  (`parse_fn_decl`, `bynk-syntax/src/parser/statements.rs`). `given` is a
  reserved token (`TokenKind::Given`) and appears nowhere in the expression
  grammar. The only call-site clause today is the test-only `by <Actor>(…)` on
  an effect-let, attached to the statement rather than the expression
  (`parse_call_site_actor`, `CallSiteActor`).
- **Emission.** A `given` function takes a hidden `deps` parameter typed as
  exactly the capabilities it names, after the comparator witnesses and before
  `__exec` (`emit_free_fn_in`, `bynk-emit/src/emitter/emit/functions.rs`, the
  one path for a context's, a commons' and a suite's free functions).
  Every call passes the caller's own `deps` through (`lower_call`,
  `bynk-emit/src/emitter/lower/calls.rs`), and a reference as a value closes
  over it (`lower_ident`, `lower/ident.rs`). In a composed provider body
  `deps` is `this.deps` (`LowerCtx::cap_deps_node`,
  `bynk-emit/src/emitter/lower_ctx.rs`). The execution context is a separate
  parameter: `__exec` in a function, `deps.__exec` in a handler
  (`LowerCtx::exec_node`).
- **Types.** A derived instance is emitted as a type alias of its base's
  interface (`export type Sessions = bynk_cloudflare.Kv;`,
  `emit_derived_capability`, `emit/capability.rs`), so `deps.Sessions` already
  satisfies a parameter typed `Kv`.

## 3. Goals and non-goals

**Goals.**

- A commons free function may declare capability parameters
  (`given Store: Kv`) and shared capabilities (`given Logger`), and use them in
  its body and its lambdas.
- Every call binds each capability parameter explicitly to a capability the
  caller holds. Nothing is inferred, even when only one candidate exists.
- The inclusion rule holds transitively, through context and commons bodies
  alike.
- A unit-tier suite of a commons can bind parameters to in-memory doubles.
- The emitted code passes the bound instances through the existing hidden
  `deps` parameter, with no runtime change.

**Non-goals.**

- **Structural capability bounds** ("any capability with a `get` and a
  `put`"). A bound names a kind.
- **Generic capability kinds** (`Queue[Order]`). None exist (#3879).
- **Capabilities declared in a commons.** A commons may name kinds that a
  first-party unit exports; it may not declare its own capability contract
  (§14 Q2).
- **Capability parameters on context functions,** methods, or suite helpers
  (§14 Q6). A suite helper keeps ADR 0977's plain `given`; in a commons suite
  it names the doubles a case holds (§9).
- **A capability selection from a project adapter.** A commons selects only
  first-party capabilities; ADR 0990 B's project adapters stay types-only
  here.
- **A partially bound function as a value** (§14 Q8).
- **Inference or defaulting of a binding,** in the compiler or in tooling.

## 4. Declaration

A commons free function declares its capabilities in the `given` clause it
already parses on context functions. An entry is one of two forms:

- **A capability parameter,** `Name: Kind`. `Kind` is an instantiable kind the
  commons selects (§6.1). Inside the body, `Name` is called like a
  capability: `Store.get(key)`.
- **A shared capability,** a bare `Kind` such as `Logger`. It means the
  caller's one capability of that kind, unbound (§6.2).

```bynk
fn copyOne(key: String) -> Effect[()] given Src: Kv, Dst: Kv

fn copyAll(prefix: Option[String]) -> Effect[Int] given From: Kv, To: Kv, Logger {
	let keys <- From.list(prefix)
	let _ <- keys.forEach((k) => copyOne(k, given Src = From, Dst = To))
	let _ <- Logger.info("copied \(keys.length()) keys")
	Effect.pure(keys.length())
}
```

`copyOne`'s body is elided; it returns `Effect[()]`, which is what
`forEach` takes (`List.forEach(f: T -> Effect[()])`).

The existing refusals still apply: only a free function returning
`Effect[T]`, and never `Events` (ADR 0726 A).

A test-only commons (`commons x.y for tests`, ADR 0974) is a commons, so the
same declarations are open there. That gives suites of several targets one
home for effectful setup, `fn seed() -> Effect[()] given Store: D1`, which
ADR 0974 had to leave out because a commons has no one capability set; a
case binds it to its target's instance (`seed(given Store = Orders)`).
Although ADR 0974 D checks a test-only commons' items as a suite helper is,
its `given` follows the **commons** rule, not the suite-helper one: a bare
instantiable kind (`given Kv`) is `bynk.given.kind_needs_parameter`, as in
any commons, and the parameter form is the only way in. A bare kind means a
double only in a suite helper, whose suite has a target (§9).

**Naming.** A parameter is capitalised, because it sits where a capability
does and is never a value (#918's case rule: capitalised names a closed
descriptor in type-like position, never a value). It is refused if it shadows
any capability name visible in the commons, a selected kind included
(`given Kv: Kv`), if two parameters share a name, or if it shares a name
with one of the function's type parameters. A parameter is not a value:
`let s = Store` is refused, as `let l = Logger` is today.

**Parsing.** `given Name: Kind` extends `CapRef` with an optional bound. A
function's `given` clause is followed by `where`, a contract clause or `{`, so
the `:` is unambiguous. Handlers and providers keep the plain form.

## 5. Binding at the call

### 5.1 The syntax

Bindings are written **inside the call's argument list, after the ordinary
arguments, introduced by `given`**:

```bynk
cached[Session](id, () => loadSession(id), given Store = Sessions)
copyAll(None, given From = Sessions, To = Archive)
warmUp(given Store = Sessions)
```

- `given` cannot start an expression, so the parser knows the ordinary
  arguments have ended. Everything after it up to `)` is `Name = Capability`
  pairs separated by commas.
- The argument list becomes
  `"(" ( args | (args ",")? "given" binding ("," binding)* )? ")"`, with
  `binding = Name "=" CapRef`. The ordinary arguments may be empty, as in
  `warmUp(given Store = Sessions)`; when there are some, a comma separates the
  last from `given`. No trailing comma follows the last pair, matching
  ordinary call arguments, which take none today (`parse_arg_list_body`,
  `bynk-syntax/src/parser/expressions.rs`). If call arguments later gain a
  trailing comma, the binding list gains it too.
- Each capability parameter of the callee is bound exactly once. A missing,
  repeated or unknown name is an error, and a binding on a callee without
  parameters is an error. The pairs may come in any order; the formatter keeps
  the author's.
- Shared capabilities are never bound. `given Logger = Logger` is refused.
- The right-hand side is a capability name the caller can see: a local name,
  a flattened one, or one of the caller's own parameters (§7).

The form does not clash with existing syntax. `given` today appears only
after a handler's or function's return type, in a service header, and after a
provider's name (`provides X = Impl given Y`), all in declaration position.
The test-only call-site `by` is a statement clause on an effect-let, not part
of the call, and the two compose: `let r <- f(x, given S = A) by User("bob")`
parses as before.

### 5.2 In every expression position

The binding is part of the call expression, so it goes wherever a call goes:

```bynk
// an effect-let, and `do`
let s <- cached[Session](id, load, given Store = Sessions)
do warmUp(given Store = Sessions)

// a `par` block: two instances, joined
par {
	let a <- cached[Session](id, load, given Store = Sessions)
	let b <- cached[Price](sku, quote, given Store = PriceCache)
}

// a lambda (`parTraverse` takes `T -> Effect[()]`; `evict` returns `Effect[()]`)
let _ <- ids.parTraverse((id) => evict(id, given Store = Sessions))

// a method chain, which is Bynk's pipeline
let r <- cached[Session](id, load, given Store = Sessions).within(2.seconds)
```

### 5.3 The rejected form

The spine sketched a trailing clause, `cached[Session](id, load) given Store =
Sessions`. It is rejected because the clause has no closing token:

- a method chain needs parentheses, `(f(x) given S = A).within(d)`;
- inside another call's arguments the comma is ambiguous,
  `g(f(x) given S = A, B = C)`: is `B = C` a binding or `g`'s next argument?
- a lambda body followed by more arguments has the same problem.

Putting bindings in the type-argument brackets
(`cached[Session, given Store = Sessions](…)`) was also considered. Type
arguments are usually inferred and the brackets omitted; bindings never are,
so every call would need brackets for a reason unrelated to types.

## 6. What may bound a parameter

### 6.1 Parameter kinds

A bound names an **instantiable kind** that the commons selects from a
first-party unit. Today that is `bynk.cloudflare`'s `Kv`, `Service`, `R2`,
`RateLimit`, `Queue`, `Analytics`, `D1`, `EdgeCache`, `StoredSecret`, `Email`,
`Postgres`, `MySql`, `Vectorize`, `Container`, `AiSearch` and
`MtlsCertificate` (the `instantiable` list of `bynk.cloudflare` in
`bynk-std`).

To name a kind, a commons lists it in the braced `consumes` that ADR 0990
already gives a commons: `consumes bynk.cloudflare { Kv }`. This is ADR 0990
Decision E carried out. The list stays braced and may still hold types beside
capabilities (`consumes bynk.cloudflare { Kv, KvEntry }`), but a capability
name is admitted only from a **first-party** unit; from a project adapter it
is still `bynk.commons.consumes_not_types`, whose note gains the first-party
rule. In a commons the selection imports the contract only:

- no provider is instantiated and no binding is emitted, because a commons
  is not a deployment unit;
- the platform lock still falls where it does today. A context can bind a
  `Kv` parameter only to a `Kv` it holds, and it holds one only by consuming
  `bynk.cloudflare` itself;
- a unit that `uses` the commons gets no capability through it, as ADR 0990
  D already says for types;
- a commons that selects capabilities but uses none of them gets the
  existing unused-import treatment.

Naming a `bynk` capability (`consumes bynk { Logger }`) is an explicit
`consumes bynk`, so it turns off ADR 0983's implicit reach for that file, as
any explicit `consumes bynk { … }` does today: the file then lists every
`bynk` type it names beside its capabilities, and one it leaves out is
`bynk.consumes.type_not_selected`, with the existing fix.

A valid argument for a parameter bounded by `K` is a capability the caller
holds in its `given` that is one of:

- a derived instance whose base is `K` (`Sessions`, from
  `capability Sessions = Kv`);
- the bare `K`, when the kind allows a bare use (`Instantiable::bare`; only
  `Kv` today);
- the caller's own parameter bounded by `K` (forwarding, §7);
- in a commons suite, the double of `K`, written `K`, or a suite-local
  instance of `K` (§9).

A suite case has no `given` of its own. For this rule, what a case holds
follows ADR 0977 B: in a context's suite, its target's capabilities; in a
commons suite, the double of every kind its commons selects, instantiable and
shared alike, plus every suite-local instance the suite declares. A `given`
suite helper holds what its `given` names, as a function does.

The kind must match exactly. A `Kv` instance cannot bind a `D1` parameter,
whatever their operations.

### 6.2 Shared capabilities

A commons may name a **non-instantiable** first-party capability bare, as a
shared capability: `bynk`'s `Clock`, `Random`, `Logger`, `Fetch`, `Secrets`,
`Locale` and `Idempotency`, `bynk.cloudflare`'s singletons (`Version`, `Ai`,
`Images`, `AnalyticsSql`), and `bynk.crypto`'s `Crypto`. The rule covers every
first-party unit, so a kind a unit adds later is classified with no new
list. `bynk.testing`'s `TypedReads` is classified the same way, but its unit
is experimental, so a commons that names it needs the usual `[unstable]
allow`. The callee's `given Logger` requires the
caller's `given Logger`, exactly as between context functions today.

The rule is mechanical: **instantiable means parameter, not instantiable
means shared.**

- An instantiable kind named bare in a commons `given` (`given Kv`) is
  refused, with a fix that turns it into a parameter (`given Store: Kv`). The
  caller may still bind its bare `Kv`: `given Store = Kv`. On the right of a
  binding in a context, `Kv` names the context's own bare `Kv` capability,
  which must be in its `given` like any other argument; it is a choice the
  caller writes, not a default the library falls back to. A library never
  silently means "the default namespace".
- A non-instantiable kind used as a bound (`given L: Logger`) is refused.
  There is only one, so a binding would add a name and no choice.
- `Events` stays refused, for ADR 0726's reason: `Events.emit` buffers until
  its handler commits, and an event belongs to the emitting context.

## 7. Forwarding and the inclusion rule

A commons function calling another binds the callee's parameters to its own:

```bynk
fn refresh[T: Codable](key: String, load: () -> Effect[T]) -> Effect[T] given Store: Kv, Logger {
	let v <- load()
	let _ <- Store.put(key, Json.encode(v))
	let _ <- Logger.info("refreshed \(key)")
	Effect.pure(v)
}
```

and `cached` calls it with `given Store = Store`. The left `Store` is the
callee's parameter; the right is the caller's. `refresh` uses `Logger` itself;
`cached` counts its `Logger` as used through the call, as a context function
does today, so neither warns with `bynk.given.unused_capability`.

The inclusion rule extends ADR 0726 B without changing its shape:

- each binding's right-hand side must be in the caller's own `given`, and
  counts as used there. In a commons suite case, which has no `given`, it
  must be one of the doubles the case holds (§6.1);
- each shared capability of the callee must be in the caller's `given`, as
  today;
- a commons body can bind only to its own parameters. It cannot name a
  context instance, because it can't see one. This is checked at the call, so
  it holds along every chain, and recursion needs no special case.

As with an `Ord` bound (`reference/generics.md`, *Ord at run time*),
the capability appears at every link of a chain. A lambda in the body closes
over the parameters (ADR 0033), so `Store.get` inside a lambda needs no extra
syntax.

Binding one instance to two parameters (`given From = Sessions, To =
Sessions`) is allowed. The binding is explicit, so a reader sees the
aliasing.

## 8. Emission

A function with capability parameters takes the same hidden `deps` parameter
as a context function, typed by parameter and shared names:

```ts
// types abridged; `__exec` follows `deps` when the function needs it
export async function cached<T>(key: string, load: () => Promise<T>,
	deps: { Store: Kv; Logger: Logger }): Promise<T> {
	const hit = await deps.Store.get(key);
	…
}
```

A call builds the callee's `deps` from the caller's:

```ts
await cached(id, () => loadSession(id, deps), { Store: deps.Sessions, Logger: deps.Logger });
```

- **Cost.** One object literal per call. No runtime library change, no new
  Worker binding: `deps.Sessions` is the instance the composition root already
  built. A callee with shared capabilities only keeps today's pass-through
  of the caller's `deps`.
- **Types.** A derived instance's type is its base's interface
  (`emit_derived_capability`), so `deps.Sessions` checks against `Kv`.
- **The execution context.** `__exec` is its own parameter, after `deps`
  (`emit_free_fn_in`), filled by `LowerCtx::exec_node`, not
  read from the callee's `deps`. Building a fresh `deps` object therefore
  drops nothing that `within`, `race` or `detach` need.
- **Providers.** A Bynk-bodied provider's `deps` is `this.deps`
  (`LowerCtx::cap_deps_node`); the built object reads from it the same way.
- **Observation.** In the test build `deps.Sessions` is already the recording
  proxy (ADR 0152), so calls made through `Store` are recorded against
  `Sessions` with no extra work.

The emitted parameter is codegen, not published ABI (ADR 0310 D1, as ADR
0726 noted for context functions).

## 9. Tests, stubs and observation

**Principle.** `stub` and `expect … called` name **capabilities**, never
parameters. ADR 0726 settled that a stub applies to the capability, not the
call site. A parameter's name is local to one signature, and two functions
may each call theirs `Store`.

- **In a context's suite.** A call through `Store` reaches the bound
  instance, so the case stubs and observes that: `stub Sessions.get(_)
  returns None`, `expect Sessions.put called once`. A stub inside the commons
  call sees exactly what a direct call to `Sessions.get` would.
- **In a commons suite.** A commons suite case binds a parameter to the
  kind's in-memory double, named by the kind:
  `cached[Session](id, load, given Store = Kv)`. In a suite, a kind's name
  on the right of a binding, or as the subject of `stub` or `expect …
  called`, denotes that kind's double; in a bound, `Store: Kv`, it is still
  the kind. The suite stubs and observes `Kv.get`. Each kind's double is
  fresh per case, as an instance's double is (`reference/capabilities.md`
  §Derived capabilities). A shared capability the commons selects gets its
  double the same way, so `expect Logger.info called` works.
- **Two doubles of one kind.** A function with `From: Kv, To: Kv` needs two
  distinct doubles to be tested honestly. A commons suite may declare a
  suite-local instance, `capability Archive = Kv`, which is a fresh double
  with no binding and no deploy effect. This lifts
  `bynk.capability.derived_outside_context` for commons suites only; a commons
  itself still cannot declare one.
- **Effectful suite helpers.** ADR 0977 shipped suite helpers with `given`,
  which take the case's `deps`. In a commons suite, such a helper's `given`
  names the doubles a case holds (`fn seed() -> Effect[()] given Kv`), which
  the bare-`Kv` refusal of §6.2 does not touch, because there `Kv` is a
  double, not a kind. A helper takes no capability parameters (Q6); setup
  over a parameter lives in a test-only commons (§4), which a case calls with
  a binding.
- **A stub on a parameter** (`stub Store.get(_) returns None`) is
  `bynk.stub.not_a_seam`, with a note naming the bound instance (context
  suite) or the kind's double (commons suite). A parameter is never in scope
  in a case, so the note is reached only when the name matches a parameter of
  a function the case calls.

## 10. Risks

- **Widening a commons' `consumes`.** ADR 0011 made `consumes` the effectful
  edge, and ADR 0990 opened a commons to types only. Mitigated by admitting
  only first-party capability names in the same braced list, contract only,
  as ADR 0990 E intended (§6.1).
- **Wrong-instance binding.** See §11.
- **Ceremony.** Explicit binding at every call is more to write than
  inference. It is the decided trade: libraries are called far less often
  than they are read in review.
- **Diagnostics through chains.** A missing binding three commons deep must
  name the function where it's missing, not the context. The rule is checked
  per call, so each error is local.
- **Test-double confusion.** A stub on a parameter name would look plausible.
  It is refused with a note naming the bound instance (context suite) or the
  kind's double (commons suite).
- **Tooling drift.** The `given` clause gains a second form that hover,
  completion, signature help, the formatter, tree-sitter and the
  architecture model all need (ADR 0156).

## 11. Threat model

**None as a security boundary,** because a capability parameter grants
nothing the caller does not already hold. The inclusion rule makes every
binding's right-hand side a capability in the caller's own `given`, and a
commons can still declare no instance (ADR 0469 D4), so a library cannot
reach a resource unless some context names it. The one exception is a
commons suite's suite-local instance (§9). It is a fresh in-memory double
with no binding and no deploy effect, and exists only under `bynk test`, so
it reaches no resource.

**The integrity risk is binding the wrong instance:** reading through
`PriceCache` where `Sessions` was meant, or writing to the wrong D1 database.
The compiler can't detect it, because both are well-typed. The mitigation is
the settled rule: the instance is written at every call site, where review
sees it, and is never inferred, even with one candidate. Two tooling rules
keep that intact (slice 4):

- the "add binding" fix offers one action per candidate and is
  machine-applicable only when there is exactly one, so `bynk fix` never picks
  between instances;
- hover and the architecture model attribute a call through a parameter to
  the instance bound, so a review can trace which data a library touches.

Shared capabilities are broader than they look. `Secrets.get(name)` in a
commons can read any secret the caller can, and with `given Secrets, Fetch`
it can send one anywhere. The grant is still visible in every signature on
the path, as for context functions, so a caller sees it before calling.
Commons are already shareable as packages ([`packaging.md`](packaging.md)
D1), so the library may be a dependency's. **Ruled (26 November 2026):**
`Secrets` stays shared in this track. If dependency commons reaching secrets
becomes a real concern, with `Secrets` plus `Fetch` as the concrete case to
watch, a narrower grant (naming the secrets a function may read) is the line
to revisit, in its own increment.

## 12. Slices

| # | Slice | Depends on |
|---|---|---|
| 0 | Settle this doc; land D1 and D2 | — |
| 1 | Declare, bind, call: first-party capability names in a commons' braced `consumes` (ADR 0990 E); `given Name: Kind` and shared capabilities on a commons free function; in-body use; binding at a call from a context body or a provider; the inclusion rule; emission through `deps` | 0 |
| 2 | Forwarding and generics: binding from one commons function to another, type parameters alongside, recursion, the refusal of an unbound function value | 1 |
| 3 | Tests: kind doubles in a commons suite, suite-local instances (lifting `bynk.capability.derived_outside_context` in a commons suite only), `stub` and `expect … called` through a bound instance, the `bynk.stub.not_a_seam` note for a parameter, fixtures in both kinds of suite and for a test-only commons | 1 |
| 4 | Tooling and the book: hover, completion of instances at a binding, the add-binding fix (§11), signature help, architecture model, the reference and spec, and a guide ("Write a library that uses a capability") | 1; 2 and 3 for the complete pages |

## 13. Decisions (ADRs to land up front)

- **D1. Capability parameters and their binding.** A commons function
  declares `given Name: Kind`. Every call binds each parameter explicitly,
  inside the argument list after a `given` (§5), to a capability in the
  caller's `given`. Never inferred or defaulted, by the compiler or by a
  machine-applicable fix with more than one candidate. Supersedes the
  `bynk.given.fn_outside_context` refusal for commons free functions.
- **D2. Kinds and shared capabilities.** A commons may list first-party
  capabilities in ADR 0990's braced `consumes`, contract only (ADR 0990 E
  carried out). An instantiable kind
  is always a parameter; a non-instantiable one is always shared and named
  bare; `Events` is refused (§6). A commons still declares no capability or
  instance; the suite-local instances of slice 3 are the only exception, and
  only in a commons suite.

Recorded in their slices: emission through `deps` (slice 1); forwarding
(slice 2); tests name capabilities, never parameters, and suite-local
instances, which lift `bynk.capability.derived_outside_context` in a commons
suite only (slice 3).

### 13.1 Diagnostic codes

The codes are named here so that D1 and D2 can cite them; the slice that
first raises a code registers it with its explainer. Where an existing code
already says the right thing, it is reused.

| Code | Raised when | Slice |
|---|---|---|
| `bynk.given.binding_missing` | a call leaves a callee's capability parameter unbound; the fix is machine-applicable only with one candidate (§11) | 1 |
| `bynk.given.binding_unknown` | a binding names no parameter of the callee, or the callee has none | 1 |
| `bynk.given.binding_duplicate` | a call binds one parameter twice | 1 |
| `bynk.given.binding_shared` | a call binds a shared capability (`given Logger = Logger`) | 1 |
| `bynk.given.binding_kind_mismatch` | the right-hand side is not of the bound's kind (a `Kv` instance for a `D1` parameter) | 1 |
| `bynk.given.missing_for_call` (reused) | the right-hand side is not a capability the caller holds | 1 |
| `bynk.given.kind_needs_parameter` | a commons, a test-only one included, names an instantiable kind bare (`given Kv`); the fix makes it `given Store: Kv` | 1 |
| `bynk.given.shared_as_parameter` | a non-instantiable kind is used as a bound (`given L: Logger`) | 1 |
| `bynk.given.parameter_name_conflict` | a parameter shadows a visible capability or kind, a type parameter, or another parameter | 1 |
| `bynk.given.parameter_outside_commons` | `given Name: Kind` on a context function, a method, a handler, a provider or a suite helper (Q6) | 1 |
| `bynk.given.parameterised_fn_value` | a function with capability parameters is named as a value (Q8) | 2 |
| `bynk.commons.consumes_not_types` (reused, note widened) | a commons lists a capability from a unit that is not first-party | 1 |
| `bynk.stub.not_a_seam` (reused, note added) | a `stub` or `expect … called` names a capability parameter (§9) | 3 |

A parameter used as a value (`let s = Store`) gets the error a capability
used as a value gets today; slice 1 adds a note naming it a parameter.
`bynk.given.fn_outside_context` loses its last trigger in slice 1 and moves
to the retired codes (`Retired` in `bynk-syntax/src/diagnostics.rs`).

## 14. Open questions — settled answers

Every question was reviewed on the settling PR, and the points that needed
the maintainer were ruled on 26 November 2026, each on the proposed option:

- **Q2's commons `consumes`** was closed by ADR 0990 (#4044), whose Decision
  E names the braced, type-only list as the home for capability contracts.
  The qualified-bound alternative (`given Store: bynk.cloudflare.Kv`) is
  rejected.
- **Q1 and Q7 depart from the spine's sketch:** binding inside the argument
  list, and stubs that name the instance, not the parameter. Both accepted.
- **Q6** closes context functions to parameters for this track. Accepted.
- **`Secrets`** stays a shared capability (§11). Accepted.
- **Diagnostic codes** are named in §13.1; a slice may rename one when it
  registers it, and says so in its ADR.

1. **The binding syntax.**
   *Settled:* inside the argument list, after the ordinary arguments:
   `f(x, given Store = Sessions)`. The binding itself is explicit at every
   call (decided).
   *Why:* the closing `)` bounds the clause, so it works unchanged in a `<-`, a
   `par` block, a lambda, a method chain and a nested argument (§5.2). `given`
   cannot start an expression, so parsing is unambiguous.
   *Rejected:* the trailing `f(x) given Store = Sessions` from the spine. It
   has no end token, so chains need parentheses and a nested call's commas are
   ambiguous (§5.3).

2. **What may bound a parameter.**
   *Settled:* an instantiable first-party kind the commons selects with a
   braced `consumes` (§6.1). No commons-declared capabilities, no generic
   kinds, no structural bounds. Arguments: as listed in §6.1 (an instance of
   that kind, the bare kind where it allows one, a forwarded parameter, or a
   double in a commons suite).
   *Why:* a commons can name types only today (ADR 0990), and first-party
   kinds are the case that motivates the track. ADR 0990 E already names the
   braced list as the home for capability contracts. A commons-declared
   contract would need a new provision story; `Queue[Order]` does not exist
   (#3879).
   *Rejected:* a structural "any capability with these operations" bound. It
   turns kinds into duck types and makes a wrong binding easier. Qualified
   bounds with no import (`given Store: bynk.cloudflare.Kv`, `given
   bynk.Logger`): noisier, and a second way to name a unit's export beside
   `consumes`.

3. **Shared capabilities.**
   *Settled:* instantiable means parameter, non-instantiable means shared.
   `Clock`, `Random`, `Logger`, `Fetch`, `Secrets`, `Locale`, `Idempotency`,
   the Cloudflare singletons and `Crypto` are named bare; `Events` is refused. A bare
   `Kv` in a commons is refused with a fix to a parameter, and a caller may
   bind its bare `Kv`.
   *Why:* the registry already records which kinds have instances, so the rule
   needs no new list. A library never means "the default namespace" by
   accident.
   *Rejected:* letting a bare `given Kv` mean the caller's bare `Kv`. It reads
   like a shared capability but isn't one in a context with instances.

4. **Naming and case.**
   *Settled:* capitalised, under #918's case rule. Shadowing a visible
   capability or kind, a type parameter or another capability parameter is
   refused. A parameter is never a value.
   *Why:* inside the body `Store.get` must mean one thing. A shadowed `Kv`
   would make `Kv.get` and `Store.get` silently different.
   In a commons suite, a kind's name on the right of a binding is that
   kind's double (§9); in a context, `given Store = Kv` binds the context's
   own bare `Kv` (§6.2). Neither is a "default namespace" reading.
   *Rejected:* allowing shadowing with a warning.

5. **Forwarding.**
   *Settled:* a commons function binds the callee's parameters to its own,
   `given Inner = Store`, and may bind only to names it can see. Shared
   capabilities follow the existing inclusion rule.
   *Why:* checked per call, like ADR 0726 B and the `Ord` bound, so it holds
   along chains and through recursion without a graph walk.
   *Rejected:* implicit forwarding when names match. It is inference by
   another name.

6. **Context functions.**
   *Settled:* no. A context function keeps #2868's form. A helper over two
   instances lives in a commons, which the context `uses`.
   *Why:* a context function reads with concrete instances only, and one place
   holds capability-polymorphic code. Moving the helper is cheap.
   *Rejected:* allowing parameters everywhere. It is the same machinery, so it
   can be added later without breaking anything if real code asks for it.

7. **Tests.**
   *Settled:* `stub` and `expect … called` name capabilities, never
   parameters: the bound instance in a context's suite, the kind's double in a
   commons suite (`given Store = Kv`). A commons suite may declare a
   suite-local instance (`capability Archive = Kv`) for a second double. A
   stub inside a commons call sees what a direct call to the bound instance
   would.
   *Why:* ADR 0726 settled that stubs apply to capabilities, not call sites.
   Parameter names are local to a signature and can collide across functions.
   *Rejected:* `stub Store.get(…)` naming the parameter, as the spine
   sketched.

8. **Function values.**
   *Settled:* a function with capability parameters cannot be named as a
   value. `cached given Store = Sessions` is not a value in this track; write
   a lambda that binds.
   *Why:* ADR 0726 lets a `given` function close over the caller's `deps`
   because nothing needs choosing. Here a binding must be chosen, and a lambda
   already says it explicitly.
   *Rejected:* partial application of bindings. It adds a new value form for
   a small saving.

9. **Emission.**
   *Settled:* confirmed. The call builds `{ Store: deps.Sessions, Logger:
   deps.Logger }`, one object per call, with no runtime or binding change.
   `__exec` stays a separate parameter, so `within`, `race` and `detach`
   still thread (§8).
   *Why:* verified against `emit_free_fn_in`, `LowerCtx::exec_node` and
   `LowerCtx::cap_deps_node` as of `2f315d8c`.
   *Rejected:* passing the caller's whole `deps` and renaming inside the
   callee. It needs a name map at run time.

10. **Exports and providers.**
    *Settled:* commons only. Contexts export types and capabilities, not
    functions (`reference/grammar.md` §exports_decl), so a context-exported function does not
    arise. A Bynk-bodied provider may call a commons function and bind
    to its own `given` capabilities, emitted from `this.deps`.
    *Why:* a provider body is a `given` body already. Refusing it would push
    libraries back to function values inside providers.
    *Rejected:* refusing provider callers in the first slice.

### 14.1 Decisions index

| | Question | Outcome |
|---|---|---|
| Q1 | Binding inside the argument list, after `given` | Ruled: as recommended |
| Q2 | First-party kinds, selected in ADR 0990's braced `consumes` | Closed by ADR 0990 E |
| Q3 | Instantiable means parameter, otherwise shared; `Events` refused | Accepted in review: as recommended; `Secrets` ruled shared |
| Q4 | Capitalised, no shadowing, never a value | Accepted in review: as recommended |
| Q5 | Forwarding binds only to the caller's own names | Accepted in review: as recommended |
| Q6 | No capability parameters on context functions | Ruled: as recommended |
| Q7 | Stubs and `expect … called` name capabilities, never parameters | Ruled: as recommended |
| Q8 | A parameterised function is not a value | Accepted in review: as recommended |
| Q9 | One `deps` object per call; `__exec` separate | Accepted in review: as recommended |
| Q10 | Commons only; a Bynk-bodied provider may call one | Accepted in review: as recommended |
