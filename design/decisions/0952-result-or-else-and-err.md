# 0952 — Result.orElse and Result.err for pure error recovery

- **Status:** Accepted (v0.690)

**Context.** An `Effect[Result]` could recover from an error (`flatMapErr`, ADR 0176), but a plain `Result` could not: a fallback (try the cache, then the default; parse one format, else another) needed a `match`. And `ok()` dropped the error, but there was no `err()` to drop the value.

**Decision.** `r.orElse(f: E -> Result[T, F]) -> Result[T, F]` returns `r` if it is `Ok`, else `f(e)`, run only on `Err`. `f` takes the error, so recovery can inspect what failed and change the error type, as `flatMapErr` does (`Option.orElse` takes `() -> Option[T]` because `None` carries nothing). `r.err() -> Option[E]` returns the `Err` value and drops the success. Both are pure: effectful recovery stays `Effect[Result].flatMapErr`. They are plain `ResultOp`s, checked in a new module (`checker/result_more.rs`) and lowered inline like `mapErr` and `ok`; no runtime change. A function that does not return a `Result`, or whose success type does not match the receiver's, is `bynk.types.combinator_return_mismatch`; the wrong arity is `bynk.types.method_arity`. No new diagnostic code is introduced.

**Consequences.** Additive: a `Result` method that did not exist is added. As elsewhere, a bare `Ok(..)` or `Err(..)` as the lambda's body needs a typed context (the language's general inference rule), so a recovery is usually a call to a function with a declared `Result` return type.
