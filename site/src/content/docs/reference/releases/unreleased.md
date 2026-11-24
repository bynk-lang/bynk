---
title: Unreleased
description: The increments merged since the latest Bynk release.
---
The increments merged since **v0.661.1**, the latest release. They ship in
the next one. Each increment's row is added here when it merges.

| Version | Highlights |
|---|---|
| **v0.666.0** | `parForEachWith`, `parTraverseWith`, `parTraverseAllWith` and `parTraverseTryWith` bound a concurrent fan-out with a `ParOptions { limit }` from `bynk` |
| **v0.665.0** | A statement-form `expect r is Ok(x)` keeps `x` bound for the rest of the block, so a case reads `expect r is Ok(greeting)` then `expect greeting.startsWith("Hello")` |
| **v0.664.0** | `Ordering` and a universal `compare`, with `then`, `reverse` and `List.sortWith`; `Bytes.compare` returns an `Ordering` |
| **v0.663.0** | A `fold` seeded with an empty collection (`Set.empty()`, `List.empty()`, `Map.empty()`) takes its accumulator type from the result the fold is expected to produce, and the error for one with no such context names the fix |
| **v0.662.1** | Every `401` Bynk sends carries a `WWW-Authenticate` challenge derived from the route's actor (`Bearer`, `Bynk-Signature`, `Bynk-Cookie` or `Bynk`), and a CORS service exposes it |
| **v0.662.0** | A failed `expect … is …`, and an `expect false` that is a `match` arm, report the value they tested (`r was {"tag":"Err",…}`), and `bynk test --format json` carries it as `actual` |
