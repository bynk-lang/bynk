---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "A user generic record or sum over a `Codable` type parameter (`Box[T]` in `fn f[T: Codable]`) now encodes and decodes, with the JSON and error paths of the codec for a concrete `Box[Order]`; `bynk.generics.codec_generic_at_rigid` is retired (#2792)"
---

## ADR: codec-factory-for-generic-types
title: A generic type over a Codable type parameter gets its codec from a per-declaration factory
summary: The unit that names a generic record or sum at a rigid parameter builds a factory from the declaration and applies it to the argument witnesses

**Context.** ADR 0696 decision 3 gave `List`, `Option`, `Set`, `Map` and `Result`
over a type parameter their codec from runtime combinators, and refused a user
generic type there (`bynk.generics.codec_generic_at_rigid`), because a codec for
`Box[T]` needs the declaration's field codecs with `T` replaced by a witness. The
refusal left the envelope type (`Paginated[T]`, ADR 0197) unusable in generic
code. Proposed in #2792.

**Decision.**
1. **Mechanism (A1).** For each generic record or sum that a witness names at a
   type variable, the unit emits a factory `__codecOf_<Decl>(…witnesses)` returning
   the `{ encode, decode }` codec of the instantiation. Its body is the
   monomorphised codec of the declaration applied to stand-in parameter names
   (`__P_T`), nested in the factory so the calls for a parameter reach the witness;
   every other codec it calls is the module's ordinary one. The JSON, the error
   paths and the per-field revalidation are therefore those of the monomorphised
   codec, by construction, with no second implementation to keep equal. Nested
   generics compose (`__codecOf_Box(__codecOf_Pair(w, …))`); a built-in container
   of one wraps it in the existing combinator.
2. **Where it is derived (B).** The factory is generated in the unit that builds
   the witness, from the declaration it can see, as ADR 0236 requires of a
   consumer's codecs; it is never imported from the owner. It is emitted on
   demand, so code with no such witness emits byte-identically.
3. **Scope (C).** Records and sums of any arity. A recursive generic stays refused
   at a boundary (`bynk.generics.recursive_generic_at_boundary`), and a parameter
   that is not `Codable` stays `bynk.generics.missing_codable_bound`.
4. **The code (D).** `bynk.generics.codec_generic_at_rigid` is removed from the
   registry with its generated documentation; nothing else raised it.

**Consequences.** `fn f[T: Codable](b: Box[T])` and `Json.decode[Box[T]]` compile
and agree with the concrete codec. Behavioural fixtures compare the two paths
(encoding, decoding and error paths, records, sums, two parameters, a container of
the parameter inside the type, nested generics, and use across units with a
refined field). The factory's field codecs for a consumed opaque type keep today's
`StructuralOnly` behaviour (ADR 0696 decision 9, #2794); the factory neither adds
nor widens that gap. Existing emitted output is unchanged.
