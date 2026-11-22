# Capability parameters — a commons function uses a capability its caller binds

- **Status:** Settling in the draft PR that adds this doc. Spine issue: #3959,
  which carries the live slice status.
- **Realises:** the step [ADR 0726](../decisions/0726-given-on-context-functions.md)
  deferred (#2868 Decision A: "A later step could add capability parameters to
  commons functions"), and the capability review's carried-over item,
  capability-polymorphic functions in a commons.
- **Posture:** Feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md). It qualifies on two
  axes (§1.2). It is not a security boundary, but a binding error is a
  data-integrity hazard (§11).
- **Governing records:** [ADR 0726](../decisions/0726-given-on-context-functions.md)
  (`given` on context functions: the inclusion rule and the hidden `deps`
  parameter), [ADR 0469](../decisions/0469-derived-capabilities.md) (derived
  capabilities belong to the context that declares them),
  [ADR 0693](../decisions/0693-exec-context-in-functions.md) (the hidden
  execution-context parameter), [ADR 0011](../decisions/0011-consumes-capability-selection.md)
  (capability selection with `consumes U { Cap }`),
  [ADR 0152](../decisions/0152-observation-is-auto-recorded-at-the-capability-seam.md)
  (observation at the capability seam). Two ADRs land up front (§13).

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
   parameter explicitly. That much is decided on the spine. This doc proposes
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
- **Its surface is not settled.** The binding syntax, the set of admissible
  kinds, how a commons names a kind at all, and how parameters meet `stub` and
  `expect … called` are open (§14).
- **It is not a security boundary** (§11). The integrity hazard of binding the
  wrong instance is real, which is why the binding is explicit.

## 2. What's true today

### 2.1 The language

- A free function in a context may declare `given` after its return type. A
  function in a commons or a suite may not
  (`site/src/content/docs/book/reference/capabilities.md:62-89`; spec,
  `site/src/content/docs/book/spec/static-semantics.md:1441-1446`).
- The caller's `given` must include the callee's, checked at each call and at
  each reference to the function as a value, so the rule holds transitively
  (`capabilities.md:78-82`; ADR 0726 B).
- A capability may be declared only in a context or an adapter
  (`bynk.capability.outside_context`; `static-semantics.md:1431-1432`). A
  commons cannot declare a derived instance either
  (`bynk.capability.derived_outside_context`; ADR 0469 D4).
- A commons cannot write `consumes` at all (`static-semantics.md:2425-2426`;
  `site/src/content/docs/book/reference/grammar.md:374`). ADR 0011 made
  `consumes` the effectful edge and `uses` the pure one. **So a commons today
  has no way to name `Kv` or `Logger`**, not even as a bound. This is the
  first thing the track has to open (Q2).
- A derived instance is an ordinary capability with its base's operations and
  its own resource (`capabilities.md:363-369`). Each instance has its own
  in-memory double under `bynk test` (`capabilities.md:418-419`).
- Of `bynk.cloudflare`'s instantiable kinds, only `Kv` is also usable bare;
  `Service`, `R2`, `Queue`, `D1` and the rest are instance-only
  (`bynk.capability.instance_only`; `capabilities.md:409-410`). The registry
  records this as `Instantiable::bare` (`bynk-std/src/lib.rs:541-559`), and
  kinds bound once per Worker (`Version`, `Ai`, `Images`, …) as
  `Unit::singletons` (`bynk-std/src/lib.rs:518-524`, `1169-1181`).
- A capability operation may be generic (`dedup[T]`), with an explicit type
  argument at the call (`capabilities.md:246-282`). No capability kind itself
  takes a type argument; `Queue[Order]` was considered and rejected for now in
  #3879 (Decision A).
- `stub` and `expect … called` apply to a capability, not to a call site, so
  they observe calls made inside a context function
  (`capabilities.md:83-84`; ADR 0726, Tests). Recording is ambient at the seam
  (ADR 0152).
- A suite may target a commons (`grammar.md:361`). A commons has no
  capabilities, so a commons suite has no seams to stub today.

### 2.2 The compiler

- **Refusal.** `given_fn_refusal` refuses `given` outside a context with
  `bynk.given.fn_outside_context`, and on a method, a non-`Effect` return or
  `Events` with `bynk.given.fn_unsupported`
  (`bynk-check/src/checker.rs:1620-1688`).
- **Inclusion.** `require_callee_given` checks the callee's `given` against
  the caller's `given_remaining`, marks each as used, and reports
  `bynk.given.missing_for_call` with a machine-applicable fix
  (`bynk-check/src/checker/calls.rs:802-870`). It returns early for an imported
  function, because a `given` on a commons function is always refused today
  (`calls.rs:803-813`). That early return is where slice 1 starts.
- **Syntax.** A `given` clause is a list of `CapRef`s, each a name or a
  qualified `B.Cap` (`bynk-syntax/src/ast.rs:654-661`;
  `bynk-syntax/src/parser/declarations.rs:2400-2427`, `4476-4486`). On a
  function it follows the return type, before `where` and the contract clauses
  (`bynk-syntax/src/parser/statements.rs:64-68`). `given` is a reserved token
  and appears nowhere in the expression grammar. The only call-site clause
  today is the test-only `by <Actor>(…)` on an effect-let, attached to the
  statement rather than the expression (`statements.rs:869-880`;
  `ast.rs:3329`).
- **Emission.** A `given` function takes a hidden `deps` parameter typed as
  exactly the capabilities it names, after the comparator witnesses and before
  `__exec` (`bynk-emit/src/emitter/emit.rs:1115-1137`). Every call passes the
  caller's own `deps` through (`bynk-emit/src/emitter/lower.rs:3375-3379`),
  and a reference as a value closes over it (`lower.rs:2990-2994`). In a
  composed provider body `deps` is `this.deps`
  (`bynk-emit/src/emitter/lower_ctx.rs:473-485`). The execution context is a
  separate parameter: `__exec` in a function, `deps.__exec` in a handler
  (`lower_ctx.rs:459-470`).
- **Types.** A derived instance is emitted as a type alias of its base's
  interface (`export type Sessions = bynk_cloudflare.Kv;`,
  `emit.rs:2544-2549`), so `deps.Sessions` already satisfies a parameter typed
  `Kv`.

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
  (§14 Q6).
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
fn copyAll(prefix: Option[String]) -> Effect[Int] given From: Kv, To: Kv, Logger {
	let keys <- From.list(prefix)
	let _ <- keys.forEach((k) => copyOne(k, given Src = From, Dst = To))
	let _ <- Logger.info("copied \(keys.length()) keys")
	Effect.pure(keys.length())
}
```

The existing refusals still apply: only a free function returning
`Effect[T]`, and never `Events` (ADR 0726 A).

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

// a lambda
let _ <- ids.parTraverse((id) => cached[Session](id, () => loadSession(id), given Store = Sessions))

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
`MtlsCertificate` (`bynk-std/src/lib.rs:995-1168`).

To name a kind, a commons may write a **capability selection**,
`consumes bynk.cloudflare { Kv }`, limited to first-party units and to the
braced form. In a commons the selection imports the contract only:

- no provider is instantiated and no binding is emitted, because a commons
  is not a deployment unit;
- the platform lock still falls where it does today. A context can bind a
  `Kv` parameter only to a `Kv` it holds, and it holds one only by consuming
  `bynk.cloudflare` itself;
- a commons that selects capabilities but uses none of them gets the
  existing unused-import treatment.

A valid argument for a parameter bounded by `K` is a capability the caller
holds in its `given` that is one of:

- a derived instance whose base is `K` (`Sessions`, from
  `capability Sessions = Kv`);
- the bare `K`, when the kind allows a bare use (`Instantiable::bare`; only
  `Kv` today);
- the caller's own parameter bounded by `K` (forwarding, §7);
- in a commons suite, a double of `K` (§9).

The kind must match exactly. A `Kv` instance cannot bind a `D1` parameter,
whatever their operations.

### 6.2 Shared capabilities

A commons may name a **non-instantiable** first-party capability bare, as a
shared capability: `bynk`'s `Clock`, `Random`, `Logger`, `Fetch`, `Secrets`,
`Locale` and `Idempotency`, and `bynk.cloudflare`'s singletons (`Version`,
`Ai`, `Images`, `AnalyticsSql`). The callee's `given Logger` requires the
caller's `given Logger`, exactly as between context functions today.

The rule is mechanical: **instantiable means parameter, not instantiable
means shared.**

- An instantiable kind named bare in a commons `given` (`given Kv`) is
  refused, with a fix that turns it into a parameter (`given Store: Kv`). The
  caller may still bind its bare `Kv`: `given Store = Kv`. A library never
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
	Effect.pure(v)
}
```

and `cached` calls it with `given Store = Store`. The left `Store` is the
callee's parameter; the right is the caller's.

The inclusion rule extends ADR 0726 B without changing its shape:

- each binding's right-hand side must be in the caller's own `given`, and
  counts as used there;
- each shared capability of the callee must be in the caller's `given`, as
  today;
- a commons body can bind only to its own parameters. It cannot name a
  context instance, because it can't see one. This is checked at the call, so
  it holds along every chain, and recursion needs no special case.

As with an `Ord` bound (`site/src/content/docs/book/reference/generics.md:65-66`),
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
  (`emit.rs:2544-2549`), so `deps.Sessions` checks against `Kv`.
- **The execution context.** `__exec` is its own parameter, after `deps`
  (`emit.rs:1134-1137`), filled by `exec_node` (`lower_ctx.rs:459-470`), not
  read from the callee's `deps`. Building a fresh `deps` object therefore
  drops nothing that `within`, `race` or `detach` need.
- **Providers.** A Bynk-bodied provider's `deps` is `this.deps`
  (`lower_ctx.rs:473-485`); the built object reads from it the same way.
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
  `cached[Session](id, load, given Store = Kv)`. The suite stubs and observes
  `Kv.get`. Each kind's double is fresh per case, as an instance's double is
  (`capabilities.md:418-419`). A shared capability the commons selects gets
  its double the same way, so `expect Logger.info called` works.
- **Two doubles of one kind.** A function with `From: Kv, To: Kv` needs two
  distinct doubles to be tested honestly. A commons suite may declare a
  suite-local instance, `capability Archive = Kv`, which is a fresh double
  with no binding and no deploy effect. This lifts
  `bynk.capability.derived_outside_context` for suites only.
- **Effectful suite helpers.** #3983 proposes suite helpers with `given`. A
  helper in a commons suite could then take capability parameters too. This
  track does not depend on it.

## 10. Risks

- **Reopening `consumes` in a commons.** ADR 0011 made `consumes` the
  effectful edge. Mitigated by allowing only the braced selection of
  first-party capabilities, contract only (§6.1).
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
reach a resource unless some context names it.

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
commons can read any secret the caller can. The grant is still visible in
every signature on the path, as for context functions. If commons are later
distributed from a registry, this is the line to revisit.

## 12. Slices

| # | Slice | Depends on |
|---|---|---|
| 0 | Settle this doc; land D1 and D2 | — |
| 1 | Declare, bind, call: braced first-party `consumes` in a commons; `given Name: Kind` and shared capabilities on a commons free function; in-body use; binding at a call from a context body or a provider; the inclusion rule; emission through `deps` | 0 |
| 2 | Forwarding and generics: binding from one commons function to another, type parameters alongside, recursion, the refusal of an unbound function value | 1 |
| 3 | Tests: kind doubles in a commons suite, suite-local instances, `stub` and `expect … called` through a bound instance, fixtures in both kinds of suite | 1 |
| 4 | Tooling and the book: hover, completion of instances at a binding, the add-binding fix (§11), signature help, architecture model, the reference and spec, and a guide ("Write a library that uses a capability") | 1; 2 and 3 for the complete pages |

## 13. Decisions (ADRs to land up front)

- **D1. Capability parameters and their binding.** A commons function
  declares `given Name: Kind`. Every call binds each parameter explicitly,
  inside the argument list after a `given` (§5), to a capability in the
  caller's `given`. Never inferred or defaulted, by the compiler or by a
  machine-applicable fix with more than one candidate. Supersedes the
  `bynk.given.fn_outside_context` refusal for commons free functions.
- **D2. Kinds and shared capabilities.** A commons may select first-party
  capabilities with a braced `consumes`, contract only. An instantiable kind
  is always a parameter; a non-instantiable one is always shared and named
  bare; `Events` is refused (§6).

Recorded in their slices: emission through `deps` (slice 1); forwarding
(slice 2); tests name capabilities, never parameters, and suite-local
instances (slice 3).

## 14. Open questions — proposed answers

Each answer is **proposed for review, not settled.** Marking the PR ready
asserts these have been tested in review. Three points need the
maintainer's input before then:

- **Q2's commons `consumes`.** Opening even a contract-only selection in a
  commons changes ADR 0011's split between `consumes` and `uses`. The
  alternative is qualified bounds with no import (`given Store:
  bynk.cloudflare.Kv`, `given bynk.Logger`), which is noisier.
- **Q1 and Q7 depart from the spine's sketch** (in-argument binding; stubs
  name the instance, not the parameter).
- **Q6** closes context functions to parameters for this track.

1. **The binding syntax.**
   *Proposed:* inside the argument list, after the ordinary arguments:
   `f(x, given Store = Sessions)`. The binding itself is explicit at every
   call (decided).
   *Why:* the closing `)` bounds the clause, so it works unchanged in a `<-`, a
   `par` block, a lambda, a method chain and a nested argument (§5.2). `given`
   cannot start an expression, so parsing is unambiguous.
   *Rejected:* the trailing `f(x) given Store = Sessions` from the spine. It
   has no end token, so chains need parentheses and a nested call's commas are
   ambiguous (§5.3).

2. **What may bound a parameter.**
   *Proposed:* an instantiable first-party kind the commons selects with a
   braced `consumes` (§6.1). No commons-declared capabilities, no generic
   kinds, no structural bounds. Arguments: an instance of that kind, the bare
   kind where it allows one (`Kv`), or a forwarded parameter.
   *Why:* a commons can name nothing today, and first-party kinds are the
   case that motivates the track. A commons-declared contract would need a
   new provision story; `Queue[Order]` does not exist (#3879).
   *Rejected:* a structural "any capability with these operations" bound. It
   turns kinds into duck types and makes a wrong binding easier.

3. **Shared capabilities.**
   *Proposed:* instantiable means parameter, non-instantiable means shared.
   `Clock`, `Random`, `Logger`, `Fetch`, `Secrets`, `Locale`, `Idempotency`
   and the Cloudflare singletons are named bare; `Events` is refused. A bare
   `Kv` in a commons is refused with a fix to a parameter, and a caller may
   bind its bare `Kv`.
   *Why:* the registry already records which kinds have instances, so the rule
   needs no new list. A library never means "the default namespace" by
   accident.
   *Rejected:* letting a bare `given Kv` mean the caller's bare `Kv`. It reads
   like a shared capability but isn't one in a context with instances.

4. **Naming and case.**
   *Proposed:* capitalised, under #918's case rule. Shadowing a visible
   capability or kind, a type parameter or another capability parameter is
   refused. A parameter is never a value.
   *Why:* inside the body `Store.get` must mean one thing. A shadowed `Kv`
   would make `Kv.get` and `Store.get` silently different.
   *Rejected:* allowing shadowing with a warning.

5. **Forwarding.**
   *Proposed:* a commons function binds the callee's parameters to its own,
   `given Inner = Store`, and may bind only to names it can see. Shared
   capabilities follow the existing inclusion rule.
   *Why:* checked per call, like ADR 0726 B and the `Ord` bound, so it holds
   along chains and through recursion without a graph walk.
   *Rejected:* implicit forwarding when names match. It is inference by
   another name.

6. **Context functions.**
   *Proposed:* no. A context function keeps #2868's form. A helper over two
   instances lives in a commons, which the context `uses`.
   *Why:* a context function reads with concrete instances only, and one place
   holds capability-polymorphic code. Moving the helper is cheap.
   *Rejected:* allowing parameters everywhere. It is the same machinery, so it
   can be added later without breaking anything if real code asks for it.

7. **Tests.**
   *Proposed:* `stub` and `expect … called` name capabilities, never
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
   *Proposed:* a function with capability parameters cannot be named as a
   value. `cached given Store = Sessions` is not a value in this track; write
   a lambda that binds.
   *Why:* ADR 0726 lets a `given` function close over the caller's `deps`
   because nothing needs choosing. Here a binding must be chosen, and a lambda
   already says it explicitly.
   *Rejected:* partial application of bindings. It adds a new value form for
   a small saving.

9. **Emission.**
   *Proposed:* confirmed. The call builds `{ Store: deps.Sessions, Logger:
   deps.Logger }`, one object per call, with no runtime or binding change.
   `__exec` stays a separate parameter, so `within`, `race` and `detach`
   still thread (§8).
   *Why:* verified against `emit.rs:1115-1137` and `lower_ctx.rs:459-485`.
   *Rejected:* passing the caller's whole `deps` and renaming inside the
   callee. It needs a name map at run time.

10. **Exports and providers.**
    *Proposed:* commons only. Contexts export types and capabilities, not
    functions (`grammar.md:473-482`), so a context-exported function does not
    arise. A Bynk-bodied provider may call a commons function and bind
    to its own `given` capabilities, emitted from `this.deps`.
    *Why:* a provider body is a `given` body already. Refusing it would push
    libraries back to function values inside providers.
    *Rejected:* refusing provider callers in the first slice.

