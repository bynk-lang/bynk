---
level: minor
changelog: "**Breaking:** a `bynk.crypto` `SigningKey` or `VerifyKey` is no longer `Codable`, at any depth: `Json.encode`, `Json.decode`, a `T: Codable` bound and `Queue.sendValue` refuse one (`bynk.types.json_uncodable` / `bynk.types.not_codable`, with a note naming the key), so a key can no longer be encoded and sent. A program that encoded or decoded a key must keep the key bytes in `Secrets` and import the key again where it is used. The declaration-position check (`bynk.types.key_at_boundary`) and in-process uses such as `Crypto.sign(key, …)` are unchanged (#2784)"
---

## ADR: crypto-keys-are-not-codable
title: A crypto key is not Codable
summary: The shared Codable predicate rejects SigningKey and VerifyKey at any depth

**Context.** `bynk.crypto` documents `SigningKey` and `VerifyKey` as never
stored, logged or sent. That held only in declaration positions
(`reject_fn_types`, `bynk.types.key_at_boundary`). The `Ty`-side codec predicate
treated both as opaque `Bytes`, so `Json.encode(key)`, `Json.decode[SigningKey]`,
a `T: Codable` call with a key and, since #2724, `Queue.sendValue(key)` (a
capability operation, exempt from the declaration check) all compiled and could
put a key's encoding on a wire.

**Decision.** `json_codable`, the one predicate shared by `Json.encode`,
`Json.decode` and the `Codable` bound, answers `false` for a `bynk.crypto` key
type, which by recursion covers a key inside `Option`, `List`, `Set`, `Map`,
`Result` and generic instantiations. The existing diagnostics are raised
(`json_uncodable`, `not_codable`) with an added note naming the key and the
`Secrets` remedy; no new code. The declaration-position check is unchanged, so a
record or event field holding a key is still `key_at_boundary`. Capability
signatures stay exempt, so `Crypto.sign(key, …)` is unaffected.

**Consequences.** Breaking for a program that encoded or decoded a key; the
encoded value of a non-extractable key was meaningless, and the program relied on
a documented invariant being unenforced. Not machine-fixable. Released as a minor
increment, as other breaking language changes have been.
