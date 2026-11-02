---
level: minor
changelog: A generic instance method takes explicit type arguments, `b.cast[String]()`, naming the method's own parameters
---

## ADR: instance-method-explicit-type-args
title: A generic instance method takes explicit type arguments naming its own parameters
summary: x.m[T](…) on a user instance method; the receiver's parameters are not listed; ADR 0234 B and 0702 F lifted

**Context.** ADR 0234 B deferred `value.map[U](…)` and ADR 0702 B kept it refused
on an instance method while allowing it on a static (`Box.empty[Int]()`).
`fn Box.cast[U](self) -> Option[U]` was therefore uncallable without an expected
type. The parser and `ExprKind::MethodCall.type_args` already carried the list.
The maintainer accepted #2795 with decisions A to F as recommended.

**Decision.**
1. **Syntax (A).** `x.m[T](…)`, the shape already parsed. No new grammar.
2. **No new ambiguity (B).** Bynk has no postfix index, so `x.m[…]` cannot mean
   indexing. The same-line-`[` rule and the "must be followed by `(`" rule stand.
3. **The list names the method's own parameters only (C).** The receiver's are
   already ground from its type. Arity is `bynk.generics.type_arg_mismatch`, all or
   none. This differs from a static, whose type parameters come first.
4. **Emission keeps the list (D).** `Box.cast<number, string>(b)`: the receiver's
   arguments first, then the method's, the order of the emitted namespace member.
   A call without a list is emitted as before. A receiver argument with no
   TypeScript spelling drops the list and leaves `tsc` to infer.
5. **Other receivers are unchanged (E).** A list on a non-generic method, on a
   method with no type parameters of its own, or on a built-in receiver stays
   `bynk.generics.type_arg_mismatch`. The verdict for a value receiver waits for
   the receiver's type.
6. **Bounds and witnesses (F).** The list feeds the same substitution, so bounds
   and witnesses are checked and built as when inferred.

**Consequences.** ADR 0234 B and ADR 0702 F are lifted. Every existing golden is
byte-identical. Signature help skips the `[…]` list, so `b.map[String](` resolves
the same callee as `b.map(` and the list's commas are not argument separators.
Hover, completion and semantic tokens are unchanged. The inferred-argument hint is
not shown when the list is explicit.
