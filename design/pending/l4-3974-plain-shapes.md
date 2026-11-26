---
level: minor
changelog: Plain JSON for enums, `String`-keyed `Map`s and `Instant`s is decoded everywhere and written in plain mode (`options: plain`, `Json.encodePlain`)
breaking: A plain-JSON route and `Json.encodePlain` now write a payload-less sum (`enum`) as its variant's name (`"Opened"`, not `{"kind":"Opened"}`), a `String`-keyed `Map` as a JSON object and an `Instant` as an RFC 3339 string; a non-Bynk client of such a route must read those shapes. Bynk's decoder reads both forms, and tagged mode and cross-context calls are unchanged.
probes: ts_writes
raises: ts_writes — two new codec identifiers (`__<field>$ms`, and the map decoder's self-call) are built with `format!`, as the neighbouring codec names are; no TypeScript text is written
---

## ADR: plain-json-shapes
title: Enums, String-keyed Maps and Instants have plain JSON shapes, decoded everywhere
summary: The one decoder accepts the external shapes after the tagged form; plain mode and Json.encodePlain write them

**Context.** Plain mode changed only `Option`. Three shapes nearly every external API sends could not be decoded: a bare string for an enum, an object for a map and an RFC 3339 string for a timestamp.

**Decision.** (A) The decoder accepts each form at every boundary, after the tagged form; plain mode and `Json.encodePlain` write them, and tagged mode, cross-context calls, queues, stored state and contract hashes are unchanged. (B) An enum is a sum with no payload variant: a bare string decodes to the variant of that name, exactly and case-sensitively. (C) A map whose key is a `String`, or a refined or opaque `String`, also decodes from a JSON object, each key checked as the key type; other key types keep the pair array. The object's keys are read in `JSON.parse` order, which lists integer-like keys first, and a key repeated in the text keeps its last value; the decoder never sees the repeat, so a duplicate-key refusal is not available at this layer. (D) An `Instant` also decodes from an RFC 3339 string with an explicit offset (`Instant.fromIso`'s rules, a fraction truncated to milliseconds); plain mode writes UTC with three fraction digits. `Duration` stays milliseconds and `Decimal` stays text. (E) `bynk openapi` describes a plain route's enum as a string enum, its map as an object with `additionalProperties` and its `Instant` as `format: date-time`. `bynk client` decodes with the same lenient decoders, and its public types are unchanged.

**Consequences.** A webhook or third-party response keeps a typed boundary. A plain route's enum, map and instant fields change shape for its non-Bynk clients. A tagged-form decode that failed (an enum sent as a bare string) now succeeds, the same trade ADR 0450 made for `Option`.
