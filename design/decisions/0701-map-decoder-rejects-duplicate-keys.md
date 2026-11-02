# 0701 — The Map boundary decoder rejects a repeated key

- **Status:** Accepted (v0.480.16)

**Context.** ADR 0690 (H) kept the `Map` boundary decoder's last-entry-wins on a
repeated key, because rejecting it changed how existing primitive-keyed maps
decode, while the `Set` decoder already refused a repeated member. A wire map
that names one key twice is ambiguous: silently dropping an entry hides a
malformed or tampered payload. Slice 2 made the decoder build a keyed map for a
composite, `Decimal` or `Bytes` key, so `has` is already value equality on the
canonical key (`Decimal` `1.20` and `1.2` are one key). Follow-up #2707.

**Decision.** Decoding `Map[K, V]` from `[[k, v], …]` refuses an entry whose
decoded key equals an earlier entry's key, as `StructuralMismatch` at the path of
the repeated key (`$[i][0]`), expected "a distinct map key", actual "a
duplicate". The comparison is on the decoded key (the canonical key for a keyed
map), never the raw wire text. It applies to every key type and every boundary
that decodes a `Map` through the generated `__deserialise_Map_*` helpers or the
`Json.decode` / `Codable` codec (`__bynkCodecMap`): HTTP bodies, service call
arguments and results, events, the generated TS client (`client.rs` reuses
`deserialise_ref_via`), and a value `Map` held in a `Cell` or record, which rides
the same boundary codec (ADR 0690 decision 5). Not changed: store `Map`
rehydration (ADR 0124 D5), which persists as a JSON object keyed by `String(k)`
and cannot repeat a key. This supersedes ADR 0690 decision 8's
second sentence.

**Consequences.** A payload that repeated a key and used to decode now fails; a
well-formed payload (one entry per key, which `Map` serialisation always
produces) is unaffected. No wire format change.
