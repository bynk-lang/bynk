---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "**Breaking:** a context can no longer decode a type another context owns and exports `opaque`: `Json.decode`, `Json.decodeSql`, a `T: Codable` instantiation and an HTTP or queue handler input that reaches one are `bynk.context.external_decode`. Decoding created a value from the base type alone (the owner's predicate is not re-checked in the consumer), which the language otherwise forbids (`bynk.context.external_construction`). **Migration:** take the value from a service of the owning context, which returns it already validated, instead of decoding it; to accept raw input, take the base type and pass it to a service of the owner that builds the value. A reply from the owner's own service decodes as before (#2794)"
---

## ADR: no-decode-of-external-opaque
title: A context cannot decode a type another context owns and exports opaque
summary: Json.decode, Codable witnesses and HTTP or queue inputs refuse an externally-owned opaque type; owner service replies keep the structural cast

**Context.** A consumer's codec for a type another context exports `opaque`
validates the base type and casts (`Revalidation::StructuralOnly`, ADR 0236
[C]): the owner's predicate is the owner's secret, and the cross-context
contract form excludes it so tightening it causes no skew (ADR 0200). That is
sound for a value the owner produced, such as a reply from its service. It is
not sound where the bytes did not come from the owner: `Json.decode[Token](raw)`
in the consumer, an HTTP or queue handler parameter of that type, and, since
ADR 0696, a `Codable` witness instantiated at it all create a `Token` that never
met `Token.of`, while `.of`, `.unsafe` and record syntax are refused from outside
(`bynk.context.external_construction`). ADR 0696 decision 9 recorded the gap for
the witness and monomorphic paths together and deferred it. Proposed in #2794.

**Decision.**
1. **Refuse the decode (A1).** New `bynk.context.external_decode`, raised where a
   context decodes a type another context owns and exports opaque (or declares
   `opaque`), directly, inside a container or generic argument, or through the
   fields and payloads of a record or sum: at `Json.decode`, `Json.decodeSql`, a
   call whose `Codable` witness is such a type (a bounded generic's own definition
   is not flagged, the instantiation is), and at an HTTP or queue handler
   parameter. Re-checking the predicate in the consumer (A2) is rejected: it
   would publish what the contract form withholds. A validation round trip to the
   owner (A3) and keeping the gap (A4) are rejected.
2. **One predicate for both paths (B).** The witness path and the monomorphic
   path share one check over a type (`external_opaque_in`, `context_checks.rs`),
   run on the checked expressions and on handler inputs. The emitter has no
   check: it cannot know a site's trust.
3. **Trusted decode (C).** Only the generated decode of the owner's own service
   reply keeps `StructuralOnly`; nothing else a user writes reaches it.
4. **Encode (D).** Unchanged: encoding a held value carries only what the owner
   produced.

**Consequences.** Breaking narrowing, with a migration note in the changelog. The
inputs checked are the parameters of an HTTP, queue, WebSocket or `from Events(E)`
handler, and a WebSocket's inbound frame type. An event subscriber is an input
because the event arrives as the publisher's payload, decoded in the subscriber's
unit; the event declaration is walked like a record, so a payload field of such a
type is refused (an event published by the same unit names no consumed type, so is
unaffected). A `call` handler's parameters and a `cron` handler are not inputs in
this sense (the caller is a Bynk context holding the value). A consumed type that the owner exports
`transparent` and declares as a record or sum is unaffected, as is a transparent
refined scalar (re-checked inline, ADR 0236 [D]).
