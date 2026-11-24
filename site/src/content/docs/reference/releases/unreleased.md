---
title: Unreleased
description: The increments merged since the latest Bynk release.
---
The increments merged since **v0.655.0**, the latest release. They ship in
the next one. Each increment's row is added here when it merges.

| Version | Highlights |
|---|---|
| **v0.656.0** | `<context>.<service>.outcome(args)` returns a `CallOutcome` (`Handled`, `Rejected`, `Faulted`, `Unreachable`), so a caller can handle a failed cross-context call; `bynk` gains `CallOutcome`, `CallFailure` and `Refusal`, and a consumed service's `stub` gains `rejected`, `faulted` and `unreachable` |
| **v0.655.2** | A virtual-time stub (`returns … after <d>`) at the `unit` tier no longer fails `tsc` with TS2559 |
| **v0.655.1** | A path with malformed percent-encoding is a `400 MalformedPath` rejection, no longer a `500` fault |
