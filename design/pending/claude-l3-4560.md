---
level: minor
probes: ast_importers
changelog: An enum's wire name is readable as text: `e.wireName()` and `Status.fromWireName(text)`, which agree with the codecs by construction (`@name` included)
---

## ADR: enum-wire-name-accessor
title: An enum's wire name is a built-in method and static that read the codecs' own table
summary: `wireName()` and `fromWireName(text)` on enums only, core, a declared method of the same name wins

**Context.** #4560: the codecs write an enum as its variant's wire name (its `@name`, else its own name), but the language gave no way to read that text, so binding an enum to a SQL parameter, a header, a log field or a key took a hand-written `match` that had to follow every added variant and silently diverged from the codec when `@name` renamed one. The maintainer accepted the proposal on the issue.

**Decision.** (A) `e.wireName() -> String` on a value of an enum, and `E.fromWireName(text: String) -> Option[E]` on the type. The spelling states the contract (the wire name, not the Bynk name) and leaves `toString` free; the inverse is a static, as `Int.parse` is. (B) Enums only: a non-generic sum with at least one variant and none carrying a payload, however it is written. A sum with a payload variant has no single text, so `wireName` is `bynk.types.method_not_found` on it and `fromWireName` is `bynk.resolve.unknown_static_member`. `@discriminator` is sum-only and does not affect an enum's bare-string form. (C) Both read the wire name the codecs read (`Variant::wire_name`, in the emitter from the type's declaration, as the codec tables are built), so they agree by construction; the match on `fromWireName` is exact and case-sensitive, like the decoders'. They lower inline, an object literal read by the value's `tag` and a chain of comparisons answering `Some(variant)` or `None`, with no runtime helper. (D) A type that declares its own `wireName` method or `fromWireName` static keeps it: the built-in answers only where the type has declared none, so no program that compiled before changes meaning (the same rule `compare` follows). (E) It is core, not experimental: it is additive and small, and the issue's own condition (experimental if it lands after 1.0) does not apply before it. Completion offers `wireName` on an enum value and `fromWireName` after `E.`, and hover and signature help read the same table; semantic tokens are unchanged, since a method name tokenises as one already.

**Consequences.** `Text(e.wireName())` replaces the hand-written `match`, and a variant added or renamed later is followed automatically. The wire name is now part of what a program can observe, so changing a variant's `@name` changes `wireName()` as it already changed the codec's output; that is the point of the single table. No existing program changes: a name collision resolves to the user's declaration.
