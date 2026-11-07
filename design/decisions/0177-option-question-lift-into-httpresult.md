# 0177 — `?` lifts an `Option` into an HttpResult handler (`None` → `NotFound`)

- **Status:** Accepted (v0.153)
- **Provenance:** design-review finding #543 (Bynk Language Design Review
  2026-07-05, §8 Language P1 #4) — "`?` works beautifully in pure `Result`
  functions but cannot reach `Effect[HttpResult[T]]` handlers — exactly where
  errors live — so the KV-read → decode → respond pattern is a two-deep match
  pyramid." This ADR ships the **`Option → HttpResult`** half of that finding's
  `?`-extension recommendation.
- **Realises:** inside a function whose declared return peels to `HttpResult[T]`
  (a bare `HttpResult[T]` function, or `Effect[HttpResult[T]]`), `option?` lifts
  the `Option`: `Some(v)` is the expression value `v`; `None` **early-returns
  `HttpResult.NotFound` (404)**. The postfix `?` collapses the outer half of the
  read → respond pyramid to one operator.
- **Relates:** [ADR 0126](0126-httpresult-rfc9110-status-vocabulary.md) (the
  `HttpResult` status vocabulary whose `NotFound` variant is the lift target),
  [ADR 0176](0176-effect-result-combinators.md) (the sibling #543 slice — the
  `Effect[Result]` combinators), [ADR 0048](0048-combinators-as-kernel-methods.md)
  (`Option.okOr`, the escape hatch named in the new diagnostic).

## Context

`?` propagated only a `Result`'s `Err`, in a function returning `Result[_, E]`
or `Effect[Result[_, E]]` (the operand match was a bare `Ty::Result`, the return
a bare `Result`/one-level `Effect[Result]`). A storage read, though, yields an
`Option` — `Kv.get(key) : Effect[Option[T]]` — and an HTTP handler returns
`Effect[HttpResult[T]]`. Neither type met `?`, so the overwhelmingly common
"read a value; if it's absent, 404; otherwise use it" was a two-deep `match`
pyramid repeated verbatim across handlers.

The pieces to close the gap already existed: the checker has a recursive
`peel_to_http_result` helper (peeling `Effect`) used to resolve bare `HttpResult`
variant names in handler bodies, and `HttpResult.NotFound` is a nullary runtime
variant typed `HttpResult<never>` — assignable into any `HttpResult<T>`. So the
lift is a targeted extension of `check_question` and the `?` lowering, no new
grammar, type, or runtime surface.

## Decisions

**A — `?` accepts an `Option` operand only where the enclosing return peels to
`HttpResult`.** `check_question` gains an `Option[T]` arm: if
`peel_to_http_result(return_ty)` is `Some`, the `?` expression has type `T`
(`Some`'s payload); otherwise the operand is a `Result` and the existing rules
apply. The gate is the *enclosing return*, not the operand alone — an `Option`
outside an HttpResult handler has no error channel to lift into.

**B — `None` maps to `NotFound` (404), a fixed, unconfigurable status this
slice.** 404 is the canonical "the addressed resource does not exist" answer, and
a storage miss is exactly that. The lift deliberately offers **no** way to choose
a different status inline — a per-call override (e.g. `opt.orElse(Conflict)`, or a
`?`-with-status form) is a possible later refinement, not part of the minimal,
unsurprising default. `Some(v)` yields `v`, exactly mirroring the `Ok(v)` case of
the `Result` lift.

**C — `Option?` outside an HttpResult handler is a new diagnostic,
`bynk.types.question_option_outside_http`.** Rather than fold the case into the
existing `question_on_non_result` (whose message is "requires a `Result`"), a
dedicated code says precisely why the `Option` was rejected *here* and points at
the fix — `.okOr(err)` turns an `Option` into a `Result` in a `Result`-returning
function. The `question_on_non_result` message is widened to mention the Option
case ("… or an `Option[T]` in an HttpResult handler"). A `Result` under `?` is
entirely unchanged.

**D — The `Result → HttpResult` direction is out of scope, a separate later
slice.** #543 also wants `?` to lift a `Result`'s `Err` into an HttpResult
handler — but there is **no honest default status** for an arbitrary domain
error, and inventing one (silently 500) is the wrong ergonomics. That direction
belongs with the finding's *declared error embedding* (`embeds E as Variant`):
the author declares how a domain error becomes an `HttpResult`, and `?` uses the
declaration. That is a grammar-and-conversion increment of its own (a new
keyword, a resolution pass, an emitter conversion), so it is deferred and **#543
stays open**. This slice ships only the `Option` lift, which needs no such
mechanism because `None` carries no payload to convert — its status is universal.

**E — Lowering reuses the existing check-and-early-return; a commons that names
`HttpResult` now imports it, in both single-file and project mode.** The `?`
emit branches on the operand's checked type: a `Result` still emits
`if (r.tag === "Err") return r;`; an `Option` emits
`if (o.tag === "None") return HttpResult.NotFound;`, then the expression is
`o.value`. Emitting `HttpResult.NotFound` from a *free* `fn -> HttpResult[T]`
(no service handler) exposed a latent import gap: neither header imported
`HttpResult` for a non-handler use — the single-file header imported it for no
body at all, and the project header (`write_header`) only for a Service HTTP
handler. Both headers now import it via one **structural** scan
(`file_mentions_http_result` — any signature or type declaration naming
`HttpResult`), so the import is present exactly when needed and never spuriously
(a string-literal/comment mention cannot trigger it). This fixes commons
`HttpResult` emission generally, not just the `?` lift. The emitter's operand
branch also carries a `debug_assert!` that the `?` operand is a typed
`Option`/`Result`, so any future typing gap surfaces in tests rather than
silently emitting the `Result` branch on an untyped operand.

## Consequences

- The read → respond pyramid's outer `match` collapses to `let v = opt?` in any
  HttpResult handler; a storage miss is a 404 with no boilerplate.
- One new diagnostic (`bynk.types.question_option_outside_http`); the
  `question_on_non_result` message widens. No grammar, type, or runtime change.
- Single-file commons that return/construct `HttpResult` now emit a correct
  import (previously a latent gap, unreachable without a commons `HttpResult`
  fixture).
- The `Result → HttpResult` lift and the `embeds` declared error embedding remain
  the open remainder of #543.

## Tooling (ADR 0156)

- **Hover / Completion / Semantic tokens / Signature help:** unchanged. `?` is a
  postfix operator, not a keyword or method — it adds no completion candidate,
  hover target, token type, or signature. The change is a typing/lowering rule,
  invisible to the editor surface beyond the new diagnostic.

## Note (#2839): `let … else` is the explicit-status spelling

`?` on an `Option` means `404 NotFound`, and is documented as exactly that. The
spelling that names another status is `let Some(x) = e else { BadRequest("…") }`
(#2010); it needs no new `?` rule. Point D (a `Result → HttpResult` lift) stays
deferred with #543. For the case where the default is almost always wrong, a
missing `RequestHeaders.get(…)`, `bynk.http.header_question_not_found` warns and
carries a machine-applicable rewrite, and hovering the `?` states the `404`.

## Alternatives considered

- **Lift `Result` too, defaulting `Err` to 500.** Rejected (Decision D) — no
  honest universal status for a domain error; that direction is the `embeds`
  slice.
- **Reuse `question_on_non_result` for the misplaced-`Option` case.** Rejected
  (Decision C) — a dedicated code states the actual rule and names `.okOr`.
- **A configurable status (`opt?` choosing something other than 404).** Rejected
  for v1 (Decision B) — the minimal default is 404; an override can be added
  later without breaking it.
