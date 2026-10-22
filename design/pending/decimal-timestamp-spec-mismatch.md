---
level: patch
changelog: The type-system spec names its primitives `Float` and `Instant`, as the compiler does, in place of `Decimal` and `Timestamp`
---

## ADR: spec-adopts-float-and-instant
title: The type-system spec adopts the built primitive names `Float` and `Instant`
summary: `Decimal` and `Timestamp` leave the spec's `PrimType` set; there is no exact-decimal primitive

**Context.** `bynk-type-system.md` §1.1 listed `Int | Decimal | String | Bool | Bytes | Timestamp | Duration | Unit` as the primitive set. The compiler shipped `Float` (ADR 0040) for non-integer numbers and `Instant` (ADR 0114) for absolute time, and the user-facing reference already uses those names. The design documents disagreed about what `Decimal` was: the roadmap called it the spec name for `Float`, the feature checklist called it an unbuilt primitive and cited a proposal that does not exist. The spec's own §2.5.0 argues against modelling money as a fractional number.

**Decision.** The spec is amended; the compiler is not. §1.1, and every later use, name the primitives `Float` and `Instant`. `Timestamp` was the same type as `Instant` under another name, except that the spec called it unsigned; `Instant.fromEpochMillis` takes any `Int`, so the spec drops "unsigned". `Float` is specified as what it is, an IEEE 754 double, and the spec states that Bynk has no exact-decimal primitive: exact decimal quantities, money above all, are integers in their smallest unit. The money examples in the spec and the design notes now use integer minor units.

**Consequences.** No language or compiler change, and no breaking rename of shipped types across the checker, emitter, standard library, editor tooling and user code. An exact-decimal type, if one is ever wanted, is new language work under a new decision, not a gap in the current spec.
