---
title: Unreleased
description: The increments merged since the latest Bynk release.
---
The increments merged since **v0.661.1**, the latest release. They ship in
the next one. Each increment's row is added here when it merges.

| Version | Highlights |
|---|---|
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
