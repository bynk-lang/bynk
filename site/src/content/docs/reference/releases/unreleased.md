---
title: Unreleased
description: The increments merged since the latest Bynk release.
---
The increments merged since **v0.661.1**, the latest release. They ship in
the next one. Each increment's row is added here when it merges.

| Version | Highlights |
|---|---|
| **v0.696.3** | The stamp keeps a ledger of the pending files it has consumed and never stamps one twice, and `check-pending` fails a branch that carries a consumed file |
| **v0.696.2** | A `Map` or `Log` store query that asks `any` or `all`, or a `Log` query that asks `first`, `firstOrElse` or `take(n).collect()`, stops reading pages as soon as the answer is known |
| **v0.696.1** | A SQLite row may hold a `Decimal` read from a `TEXT` column, the checker's admitted row bases are listed explicitly, and a `Decimal` read from a numeric-affinity column warns (`bynk.d1.decimal_numeric_affinity`) |
| **v0.696.0** | `Effect[T]` gains `map(f)` and `andThen(f)`, legal in a pure helper, and a missing `Effect` method is `method_not_found` naming the real ones |
| **v0.695.0** | A refined pattern on an `Int` (`_ where InRange(a, b)`, `Positive`, `NonNegative`) warns with `bynk.match.refined_pattern_is_range` and a fix to the range pattern that counts towards exhaustiveness |
| **v0.694.0** | A `sendValue`, `sendValueWith` or `sendValueBatch` to a queue the project consumes is now checked against the consumer's message type (`bynk.queue.message_type_mismatch`), and the untyped `send`, `sendWith` and `sendBatch` to such a queue warn (`bynk.queue.untyped_send`) |
| **v0.693.1** | Releases build both macOS binaries again, `aarch64-apple-darwin` cross-compiled on the Intel runner, and `install.sh` installs on macOS |
| **v0.693.0** | `MapEntry` is a nameable prelude record, so `Map.of([MapEntry { key: k, value: v }])` builds a map from entries written in source |
| **v0.692.0** | An HTML response gets `Content-Security-Policy: frame-ancestors 'none'` and `Referrer-Policy: strict-origin-when-cross-origin` by default, and `security { }` gains `frameAncestors`, `referrerPolicy` and an opt-in verbatim `csp`; a JSON response is unchanged |
| **v0.691.0** | `String.findRegexNamed(pattern)` reads the named capture groups of a regex literal by name, as an `Option[Map[String, Option[String]]]` |
| **v0.690.0** | `Result` gains `orElse(f)`, pure recovery from an `Err` that may change the error type, and `err()`, the error as an `Option` |
| **v0.689.0** | An `invariant` or `transition` may read the `size()` of a `Map` or `Set` store field, and any other read of a collection is the targeted `bynk.invariant.collection_scan` / `bynk.transition.collection_scan` |
| **v0.688.0** | A `Queue` send delay is a refined `QueueDelay` (0 seconds to 24 hours): a literal out of range is a compile error and a computed span goes through `QueueDelay.of`; the documented maximum is 24 hours, not 12 (a part second still rounds up); a literal delay between 12 and 24 hours, a run-time fault before, is now accepted |
| **v0.687.0** | A route guarded by a credential-carrying actor (`Bearer`, `Oidc`, `SignedCookie`, `Signature`, or a sum with one) sends `Cache-Control: private` on every response by default, `@cache(scope: public)` on it is refused, and `@cache(noStore: true)` sends `no-store` with no `ETag` on any route |
| **v0.686.0** | `bynk explain` on a retired diagnostic code says when it was retired, why, and what replaced it, and the diagnostics reference lists retired codes |
| **v0.685.0** | A `for all` over a `String where Matches(…)` draws members of the pattern and shrinks to shorter ones, and one over `IsoDate` draws real calendar dates |
| **v0.684.0** | Every body-taking HTTP route has a 1 MiB request-body cap by default (JSON, `@form` and `@multipart`; `@raw` still declares its own); `@limit` and `limits { maxBody }` raise it up to 524,288,000 bytes, there is no unlimited setting, the `413` says how to raise the cap, and a build hint lists the routes that rely on the default |
| **v0.683.1** | `bynk test --smoke` no longer stops on a hint, so a project with a hint-level diagnostic synthesises its smoke suite as before |
| **v0.683.0** | A constant can stand as a refinement predicate's argument (`InRange(1, MaxQty)`, `MaxLength(MaxNote)`, `Matches(SlugPattern)`), substituted as the literal it is bound to |
| **v0.682.0** | `Agent(key).op.outcome(args)` returns a `CallOutcome`, as a cross-context call's `.outcome(…)` does: `AgentBusy` is `Unreachable(None)`, and a fault in the handler, an invariant violation or a rehydration violation is `Faulted` |
| **v0.681.0** | At the `integration` tier a stubbed consumed service's arguments and result cross that context's wire codecs, so a value the wire cannot carry fails the call |
| **v0.680.0** | `parTraverse` on a store or held `Map` is renamed `parForEach`, matching `List`, and the old spelling is an error with a machine-applicable fix |
| **v0.679.0** | A queue consumer can return `RetryAfter(reason, delay)` to redeliver a message after a delay (whole seconds, 1 second to 24 hours: a literal outside that range is `bynk.queue.retry_delay_out_of_range`, a computed delay is rounded up and clamped with the clamp logged); `Retry` keeps retrying after the consumer's `retryDelay` |
| **v0.678.2** | The diagnostic reference is generated one file per code, and its index is assembled at site build time, so two changes that add different diagnostics no longer conflict |
| **v0.678.1** | The generated `tests/main.ts` marks the fixed part of its `main()` with comments, so the snapshot harness can share it between fixtures |
| **v0.678.0** | `bynk.schema.lock` records each `derives Ord` type's field and variant order, and a reorder is an error until `bynk lock accept-order` accepts it |
| **v0.677.3** | `bynk test` no longer panics when a suite checks a function that returns a consumed type renamed with `as` |
| **v0.677.2** | A `400 RefinementViolation` body no longer echoes the rejected value; `violation` carries `field` and `message` only |
| **v0.677.1** | A type selected from a consumed context now wins over an unselected `bynk` export of the same name (`Refusal`, `CallOutcome`, `CallFailure`) |
| **v0.677.0** | `List` gains `filterMap`, `zipWith`, `takeWhile`, `skipWhile` and `count(p)` |
| **v0.676.0** | `String` gains `isEmpty`, `trimStart`, `trimEnd`, `padStart`, `padEnd`, `repeat`, `lines`, `lastIndexOf` and `replaceFirst`, `List[String]` gains `join(sep)`, and `bynk.string` is deprecated |
| **v0.675.0** | A queue message whose body cannot be decoded is no longer retried: it is sent unchanged to the consumer's dead-letter queue and acked, or, with none, acked and reported as a fault, with one log line naming the queue, type and decode error but never the body; a `system` case reads it as `Undecodable` |
| **v0.674.0** | An HTTP handler's `headers` parameter may be a record, decoded and validated at the boundary as `query` and `cookies` are (`@name("if-match")` names a header, names match without case, a `List` field splits a comma-separated value), and `RequestHeaders` remains the raw view |
| **v0.673.0** | A `for all` draws each plain type across its realistic domain (a wide `Int`, a `String` with whitespace and astral characters, and `Decimal`, `Instant`, `Duration`, `Bytes` and `Float` with boundaries), so a seed recorded earlier may now find a different counterexample |
| **v0.672.0** | An agent call that waits past its queue limit (`AgentBusy`) now answers `503` with `Retry-After: 1` at an HTTP route, logged as busy rather than reported as a fault; `@busyAfter(d)` before `agent` lowers the 30-second wait |
| **v0.671.2** | Releases no longer publish prebuilt macOS binaries for now (GitHub's macOS runners were holding releases back); on macOS, `install.sh` says so and points to `cargo install bynk bynk-lsp` |
| **v0.671.1** | A failing `for all` over a `Duration` or `Instant`, and a failed `expect … is …` whose subject holds one, print it in Bynk's form (`3.seconds`, `Instant.fromEpochMillis(0)`) instead of as bare milliseconds |
| **v0.671.0** | A context's `transparent` types are constructible from a consumer on every target; under Workers the consumer builds them with its own copy of the guards |
| **v0.670.0** | A path parameter takes the types a typed `query` field does (`Int`, `Bool`, `Decimal`, enums, and refined or opaque types over them), decoded after the route matches with a `400` at `path.<name>` |
| **v0.669.0** | A queue consumer's `on message` handler can take its delivery metadata as a second parameter, `QueueDelivery { attempts, sentAt }`, in either order; `q.message(m)` in a test takes an optional delivery |
| **v0.668.0** | `Ordering` is a prelude type, nameable with no import in every context and commons, and building it in a context is not a rebrand construction |
| **v0.667.0** | `Duration` gains `toSeconds`, `toMinutes`, `toHours` and `toDays` (rounding towards zero); `NonNegative`, `Positive`, `InRange` and the new one-sided `AtLeast` and `AtMost` apply to `Duration` (and `AtLeast`/`AtMost` to `Int`, `Float` and `Decimal`); `bynk.cloudflare` takes `KvTtl` and `KvCacheTtl` where it took a bare `Int` or an unchecked `Duration`; a type an adapter exports `transparent` is constructible by a consumer |
| **v0.666.0** | `parForEachWith`, `parTraverseWith`, `parTraverseAllWith` and `parTraverseTryWith` bound a concurrent fan-out with a `ParOptions { limit }` from `bynk` |
| **v0.665.0** | A statement-form `expect r is Ok(x)` keeps `x` bound for the rest of the block, so a case reads `expect r is Ok(greeting)` then `expect greeting.startsWith("Hello")` |
| **v0.664.0** | `Ordering` and a universal `compare`, with `then`, `reverse` and `List.sortWith`; `Bytes.compare` returns an `Ordering` |
| **v0.663.0** | A `fold` seeded with an empty collection (`Set.empty()`, `List.empty()`, `Map.empty()`) takes its accumulator type from the result the fold is expected to produce, and the error for one with no such context names the fix |
| **v0.662.1** | Every `401` Bynk sends carries a `WWW-Authenticate` challenge derived from the route's actor (`Bearer`, `Bynk-Signature`, `Bynk-Cookie` or `Bynk`), and a CORS service exposes it |
| **v0.662.0** | A failed `expect … is …`, and an `expect false` that is a `match` arm, report the value they tested (`r was {"tag":"Err",…}`), and `bynk test --format json` carries it as `actual` |
