---
title: Keywords
---
<!-- GENERATED FILE — do not edit by hand.
     Source: bynk-syntax/src/keywords.rs (`render_markdown`).
     Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it keywords_reference:: -->

Bynk reserves names in four tiers. The first two are lexer keywords; the third are words the lexer treats as plain identifiers; the fourth are compiler-known type names. Only the **hard keywords** can never be used as an identifier.

## Hard keywords

Reserved everywhere — these **60** words can never be used as an identifier.

| Keyword | Meaning |
|---|---|
| `Bool` | The boolean base type. |
| `Bytes` | The binary base type — an immutable octet sequence, erased to `Uint8Array` (`Bytes.fromUtf8(s)`). |
| `Duration` | The time-span base type, in milliseconds (`5.minutes`). |
| `Effect` | The effectful-computation type, `Effect[T]`. |
| `Err` | The error variant of `Result`. |
| `Float` | The floating-point base type. |
| `Instant` | The absolute-time base type, in epoch milliseconds (`Clock.now()`). |
| `Int` | The integer base type. |
| `JsonError` | The JSON-decode error type, `Result[T, JsonError]` from `Json.decode`. |
| `None` | The empty variant of `Option`. |
| `Ok` | The success variant of `Result`. |
| `Option` | The optional-value type, `Option[T]`. |
| `Result` | The success-or-error type, `Result[T, E]`. |
| `Some` | The present variant of `Option`. |
| `String` | The string base type. |
| `ValidationError` | The error type returned by a refined type's `.of`. |
| `actor` | Declare an actor — a boundary contract a handler consumes via `by`. |
| `adapter` | Declare an adapter — the host boundary (capability contract + binding). |
| `agent` | Declare a stateful, keyed agent inside a context. |
| `as` | Alias a consumed context (`consumes X as Y`). |
| `binding` | Name an adapter's TypeScript binding module (`binding "<module>"`). |
| `by` | Name the actor a handler consumes, after the return type — or a service-level default on the header (`… -> T by <name>: <Actor>`). |
| `capability` | Declare a capability (a dependency interface) in a context, or a distinct instance of a platform resource (`capability Sessions = Kv`). |
| `commons` | Declare a pure, stateless module of types and functions. |
| `consumes` | Declare a dependency on another context's services. |
| `context` | Declare a deployable context (services, agents, capabilities). |
| `cron` | The cron protocol on a service header (`from cron`). |
| `do` | Perform a unit effect as a statement (`do e` — the binder-free `let _ <- e`). |
| `else` | The alternative branch of an `if`, or what a block produces when a `let` pattern does not match (`let Ok(x) = r else { … }`). |
| `ensures` | Declare a function postcondition — a pure `Bool` clause over the parameters and `result` (`ensures <name>: <pred>`). |
| `enum` | Declare a payloadless sum type (`enum { A, B }`). |
| `expect` | Assert a predicate inside a test case (`expect <bool-predicate>`). |
| `exports` | Declare which types a context exposes, and how. |
| `false` | The boolean literal `false`. |
| `fn` | Declare a function. |
| `from` | Name the protocol a service conforms to (`service X from http`). |
| `given` | Declare the capabilities a handler requires. |
| `http` | The HTTP protocol on a service header (`from http`). |
| `if` | A conditional expression. |
| `implies` | Logical implication (`P implies Q` ≡ `!P || Q`), used in invariant predicates. |
| `invariant` | Declare an agent invariant — a predicate that must hold of every committed state. |
| `is` | Test a value against a variant pattern, yielding a `Bool`. |
| `let` | Bind a local value (`let x = …`, or `let x <- …` for an effect), or match a pattern with a fallback (`let Ok(x) = r else { … }`). |
| `match` | Pattern-match over a sum type, `Result`, or `Option`. |
| `opaque` | Declare an opaque type, or export a type opaquely. |
| `property` | Declare a generative test inside a `suite` (`property "…" { for all … }`). |
| `protocol` | Reserved keyword (protocols are a closed, compiler-known set). |
| `provides` | Provide an implementation of a capability. |
| `queue` | The queue protocol on a service header (`from queue("name")`). |
| `record` | Reserved keyword (records are written `type X = { … }`). |
| `requires` | Declare a function precondition — a pure `Bool` clause over the parameters (`requires <name>: <pred>`). |
| `self` | The current agent instance, inside a handler. |
| `service` | Declare a service (a group of handlers) in a context. |
| `stub` | Stub a consumed capability operation at a test seam (`stub Cap.op(…) returns <v>` / `fails`). |
| `transition` | Declare an agent step invariant over the `old`/`new` state pair (`transition <name>: …`). |
| `transparent` | Export a type with its structure visible (`exports transparent { … }`). |
| `true` | The boolean literal `true`. |
| `type` | Declare a type: alias, record, sum, opaque, or refined. |
| `uses` | Bring a commons into scope. |
| `where` | Attach refinement predicates to a base type. |

## Contextual keywords

Reserved only in the one position named below; elsewhere (a field, parameter, or other identifier) they are ordinary names.

| Keyword | Meaning |
|---|---|
| `asset` | Declare a static file the platform serves, public by construction (`asset name = "path"`). |
| `case` | Declare a test case inside a `suite` (`case "…" { … }`). |
| `embed` | Declare a project file whose text is a `String` constant, compiled into the code and never served (`embed name = "path"`). |
| `event` | Declare a typed fact a context may emit (`event Name = { fields }`), inside a context. |
| `messages` | Declare a message bundle for one locale (`messages "<tag>" { "code" => "template" }`), inside a commons. |
| `on` | Begin a handler declaration (`on call`, `on GET(…)`, `on message`, `on open`/`on close`). |
| `suite` | Declare a test suite targeting a unit (`suite <target> { case … }`). |

## Contextual words

Not lexer keywords: the lexer treats these as ordinary identifiers, and they carry meaning only in the position each entry describes (inside an agent body or a `migrate` step, for instance). You can still use them as a field, parameter, or other name elsewhere.

| Word | Meaning |
|---|---|
| `advance` | Move the test harness's clock forward in a `case` (`advance Clock by 15.minutes`). |
| `drop` | Inside a `migrate` step, discard a stored field's value on purpose (`drop owner`). |
| `key` | The agent's identity field — one per agent; keys the store. |
| `migrate` | An agent's migration step: carries a stored record from an older schema version to the current shape (`migrate from 1 (old: CartV1) { … }`). |
| `removed` | Declares an agent class gone (`removed agent Old`): its stored data is deleted at the next deploy, after a named confirmation. |
| `replaces` | Renames an agent class (`agent New replaces Old`): the old class's stored data and schema history move to the new one. |
| `store` | A persisted agent-state field. |

## Built-in type names

Compiler-known type constructors. They are not lexer keywords — you may use them as an identifier in value position — but they are reserved in type position: a `type` declaration may not reuse one of these names (`bynk.resolve.reserved_builtin_type`).

| Name | Meaning |
|---|---|
| `Asset` | A static file declared by `asset name = "path"`; its `.url` is the content-hashed URL the platform serves it at. |
| `BoundaryRejection` | Why the router refused a request before any handler ran; the parameter of `on rejected`. |
| `Connection` | A held WebSocket connection, `Connection[F]`. |
| `History` | A generated call-history generator, `History[Agent]` (test-only). |
| `HttpResult` | The HTTP handler result type, `HttpResult[T]`. |
| `List` | The immutable list type, `List[T]`. |
| `Map` | The immutable map type, `Map[K, V]`. |
| `Query` | The lazy storage-read type, `Query[T]`. |
| `QueueResult` | The queue handler result type (non-generic). |
| `RequestHeaders` | An HTTP handler's read-only view of the request's headers (`headers: RequestHeaders`). |
| `Stream` | The value-over-time primitive, `Stream[T]`. |
| `TokenClaims` | An `Oidc` actor's verified token claims, read through its binder (`who.claims`). |
