---
level: minor
changelog: `bynk.cloudflare` gains `Ai`, a one-per-Worker Workers AI capability whose `run(model, input)` runs any model over JSON text through the `[ai]` binding, with a typed `AiError`; every operation faults until stubbed in a test
probes: emit_abi_shapes, ts_writes
---

## ADR: workers-ai-capability
title: Workers AI is a one-per-Worker `Ai` capability: `run` over JSON text, typed operations layered on it
summary: The `[ai]` singleton, its error type, the model-id rule and why typed reads are not used

**Context.** Workers AI was reachable only through a REST adapter that spends a round trip, an account id and an API token (#3200). The in-Worker `[ai]` binding needs none of them, and the platform's payloads differ per model: a chat model takes `messages`, an embedding model `text`, an image model answers bytes. ADR 0045 forbids an untyped `Json` value, and #3300 (ADR 0798) added the one-per-Worker kind this binding fits.

**Decision.** `bynk.cloudflare` gains `Ai`, a `Singleton` (`[ai] binding = "AI"`), used bare (`given Ai`); `capability A = Ai` is `bynk.capability.base_not_instantiable`, and the stanza is emitted only for a Worker whose closure reaches `Ai`. It is experimental. The surface is layered: a text-level `run(model, input: String) -> Effect[Result[String, AiError]]` that covers every model today and tomorrow (the caller builds the input with `Json.encode` and decodes the reply in a second step), with typed `generate` and `embed` and a streamed `runStream` added by later slices of #3478. Model ids are strings, not a catalogue: the binding checks only that the id is non-empty, at most 128 bytes and made of `[A-Za-z0-9@/._:-]`. An `input` that is not a JSON object, or a bad id, is a fault (a programming error); a platform refusal is an `AiError` value: `ModelNotFound` (5007, 3042, 5028), `QuotaExceeded` (3036, 5035), `Overloaded` (3040), `InvalidInput` (5004, 3003, 3006) and `AiFailed` for everything else, so a code Cloudflare adds never becomes a fault. The catch-all is `AiFailed`, not `Failed`: `Failed` is already a variant of `SqlError`, and two unqualified `Failed` variants in scope are `bynk.resolve.ambiguous_variant`; the registry's own convention (`SendFailed`, `PgFailed`) is a prefix. A binary reply is `AiFailed`. Typed reads (#3458, ADR 0830) were evaluated and declined for `run`: an undecodable reply to a `Result` payload is a fault there, and a model's output shape is the data most likely to drift, where the two-step form returns a matchable `Err` carrying the path. AI Gateway and third-party models are a follow-on.

In a test every `Ai` operation faults until `stub Ai.run(_, _) returns …` replaces it, in both the unit and the integration tier: there is no fake model, since a fabricated reply would make a test pass for the wrong reason. Under `bynk dev` a call runs the model on Cloudflare, as the platform does.

**Consequences.** New names in `bynk.cloudflare`; a program that does not consume `Ai` emits byte-identically, and no `[ai]` stanza appears for a Worker that does not reach it. A project's own adapter exposing a capability called `Ai` is unaffected, since the names live in different units. The tooling states no prices, allowances or billing rules anywhere; it links to Cloudflare's pages, so nothing goes stale when Cloudflare changes them.
