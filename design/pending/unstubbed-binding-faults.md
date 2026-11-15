---
level: patch
changelog: An unstubbed `Ai`, `AiSearch` or `Container` call in an integration test now faults the case instead of returning `Err`, so a test that passed on that `Err` may now fail until it stubs the operation
---

## ADR: unstubbed-binding-faults
title: An integration double's not-stubbed fault is marked and never mapped to an error value
summary: The marker the `bynk.cloudflare` bindings rethrow, shared with `Images`

**Context.** The integration doubles for `Ai`, `AiSearch` and `Container` throw when a case has not stubbed the operation, but `WorkersAi`, `WorkersAiSearch` and `WorkersContainer` turn any thrown error into `Err(…)`, so the "not stubbed" fault became an ordinary value and a case could pass without exercising the capability (#3589).

**Decision.** Every integration double that throws for an unstubbed call marks the error (`bynkNotStubbed: true`, made by `markNotStubbed` in the runtime). A binding that maps platform failures to `Err` rethrows a marked error first. This is the marker the `Images` double uses (#3578).

**Consequences.** A case that relied on the swallowed `Err` now faults and must stub the operation. The Secrets Store, Hyperdrive, Postgres and MySql bindings already let the fault through; the tests pin that.
