---
title: Unreleased
description: The increments merged since the latest Bynk release.
---
The increments merged since **v0.661.1**, the latest release. They ship in
the next one. Each increment's row is added here when it merges.

| Version | Highlights |
|---|---|
| **v0.669.0** | A queue consumer's `on message` handler can take its delivery metadata as a second parameter, `QueueDelivery { attempts, sentAt }`, in either order; `q.message(m)` in a test takes an optional delivery |
| **v0.668.0** | `Ordering` is a prelude type, nameable with no import in every context and commons, and building it in a context is not a rebrand construction |
| **v0.667.0** | `Duration` gains `toSeconds`, `toMinutes`, `toHours` and `toDays` (rounding towards zero); `NonNegative`, `Positive`, `InRange` and the new one-sided `AtLeast` and `AtMost` apply to `Duration` (and `AtLeast`/`AtMost` to `Int`, `Float` and `Decimal`); `bynk.cloudflare` takes `KvTtl` and `KvCacheTtl` where it took a bare `Int` or an unchecked `Duration`; a type an adapter exports `transparent` is constructible by a consumer |
| **v0.666.0** | `parForEachWith`, `parTraverseWith`, `parTraverseAllWith` and `parTraverseTryWith` bound a concurrent fan-out with a `ParOptions { limit }` from `bynk` |
| **v0.665.0** | A statement-form `expect r is Ok(x)` keeps `x` bound for the rest of the block, so a case reads `expect r is Ok(greeting)` then `expect greeting.startsWith("Hello")` |
| **v0.664.0** | `Ordering` and a universal `compare`, with `then`, `reverse` and `List.sortWith`; `Bytes.compare` returns an `Ordering` |
| **v0.663.0** | A `fold` seeded with an empty collection (`Set.empty()`, `List.empty()`, `Map.empty()`) takes its accumulator type from the result the fold is expected to produce, and the error for one with no such context names the fix |
| **v0.662.1** | Every `401` Bynk sends carries a `WWW-Authenticate` challenge derived from the route's actor (`Bearer`, `Bynk-Signature`, `Bynk-Cookie` or `Bynk`), and a CORS service exposes it |
| **v0.662.0** | A failed `expect … is …`, and an `expect false` that is a `match` arm, report the value they tested (`r was {"tag":"Err",…}`), and `bynk test --format json` carries it as `actual` |
