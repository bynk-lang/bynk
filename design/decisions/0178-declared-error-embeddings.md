# 0178 — Declared error embeddings (`?` auto-converts a cross-context error)

- **Status:** Accepted (v0.154)
- **Provenance:** design-review finding #543 (Bynk Language Design Review
  2026-07-05, §8 Language P1 #4) — "`?` does no error conversion: every
  cross-context chain carries `.mapErr(toLocalError)` … the single largest
  ergonomic tax in the language's flagship pattern." This ADR ships the
  **declared error embedding** the finding recommended, the last of #543's three
  parts (after [ADR 0176](0176-effect-result-combinators.md) and
  [ADR 0177](0177-option-question-lift-into-httpresult.md)) — so it **closes
  #543**.
- **Realises:** a trailing `embeds E as V, …` clause on a sum-type declaration
  and, driven by it, automatic error conversion in `?`: applying `?` to a
  `Result[T, E]` inside a function returning `Result[_, F]` (or
  `Effect[Result[_, F]]`) wraps `Err(e)` into `Err(F.V(e))` when `F` declares
  `embeds E as V`, replacing the manual `.mapErr(V)` on every cross-context call.
- **Relates:** [ADR 0177](0177-option-question-lift-into-httpresult.md) /
  [ADR 0176](0176-effect-result-combinators.md) (the sibling #543 slices),
  [ADR 0158](0158-literal-patterns.md) (sum/variant surface),
  [ADR 0161](0161-contextual-keyword-hover.md) (the contextual-keyword tooling
  discipline), [ADR 0175](0175-oidc-jwks-scheme.md) ("who, not whose" — HTTP
  mapping is handler policy).

## Context

`?` propagated an `Err` only when the operand's error type matched the enclosing
function's exactly. Cross-context calls almost never match: `Payments.authorise`
returns `Result[AuthId, PaymentError]`, but the caller returns
`Result[Receipt, OrderError]`. So every step carried a manual lift —
`Payments.authorise(total, u).mapErr(Payment)?` — repeated verbatim down a
handler, and the design notes' own §20 worked example is a wall of `.mapErr`.
This is the "single largest ergonomic tax" the review names.

The fix is a **declared embedding**: `OrderError` states, once, that a
`PaymentError` lives in its `Payment` variant, and `?` uses that declaration to
convert automatically. The variant already exists (`Payment(reason:
PaymentError)`); the clause only records the wrapping so the compiler can apply
it.

## The surface

```bynk
type OrderError =
  | OutOfStock(sku: Sku)
  | Payment(reason: PaymentError)
  | Fulfilment(reason: ScheduleError)
  embeds PaymentError as Payment, ScheduleError as Fulfilment

on place(u: UserId, c: Cart) -> Result[Receipt, OrderError] given Payments {
  let authId <- Payments.authorise(c.total, u)   // Result[AuthId, PaymentError]
  let ok = authId?                               // Err(PaymentError) → OrderError.Payment
  …
}
```

## Decisions

**A — An explicit `embeds E as V` clause, not structural inference.** A tempting
alternative is to treat any single-field variant wrapping `E` *as* an embedding
with no keyword. Rejected: it is implicit and surprising (a variant that merely
happens to wrap `PaymentError` would silently become a `?` target), and it is
ambiguous when two variants wrap the same type. The explicit clause is
greppable, states intent, and lets a variant wrap a type it does **not** embed.

**B — `embeds` and `as` are contextual keywords.** The clause appears only in the
trailing position of a pipe-form sum body, so `embeds`/`as` are matched
positionally and stay ordinary identifiers everywhere else — no reserved-word
churn (the lexer, `KEYWORDS`, TextMate reserved list, and `keywords.md` are
untouched). It follows the `cors`/`security`/`limits` precedent: a
clause-introducing contextual word is **not** added to the `CONTEXTUAL_KEYWORDS`
LSP-hover registry (that table is for the `key`/`store` field markers); editor
*highlighting* is still added (tree-sitter `highlights.scm` + TextMate, by a
`\bembeds\b(?=\s+[A-Z])` lookahead). The `enum`-form sum body — all payloadless
— cannot embed and does not admit the clause.

**C — Validation makes the conversion total and unambiguous.** Each `embeds E as
V` requires `V` to be a variant of the same sum (`bynk.types.embeds_unknown_variant`)
with **exactly one payload field, of type `E`** (`bynk.types.embeds_variant_shape`
— that single field is where the value wraps). A source type may be embedded by
**at most one** variant (`bynk.types.embeds_ambiguous`), so `?` never has to
choose between two wrappings. The clause's source type resolves through the
ordinary reference walk, so an unknown type is the usual `bynk.resolve.*`.

**D — One level, not transitive.** `?` converts `E → F` only when `F` declares an
embedding of `E` **directly**. A chain `E → G → F` is not followed. This is the
simplest rule (no cycle detection, no ambiguity across chains), and it matches
the flat cross-context pattern — a handler embeds the errors of the contexts it
calls, one hop. Transitive embedding is left as a possible future extension.

**E — One rule, shared by checker and emitter, so they cannot diverge.** The
decision "does `F` embed `E`, and as which variant" is `checker::embedding_for`,
a single pub function. `check_question` calls it to *accept* the `?` (widening
the error-compatibility check), and the emitter calls the **same** function to
*lower* the wrap (`return Err(F.V(r.error));`). The emitter learns the enclosing
return type `F` from `LowerCtx.return_ty`, set at the function/handler
body-emission boundary (`emit_block_as_function_body_with_return`).
**`return_ty` follows the JS `return` scope**, not lexical nesting: a nested
control-flow block (`if`/`else`/`match` arm in statement position) whose `return`
exits the whole function **inherits** the function's `return_ty` (so a `?` there
still converts), while a construct that lowers to its own arrow — a lambda, or a
value-position `if`/`match`/block IIFE, where `return` exits the arrow — **rebinds**
it (to the lambda's own return, or clears it for an IIFE so an embedding `?`
behaves like a plain `?`). This is what keeps the checker (which holds the
function-level return through nested blocks) and the emitter aligned everywhere,
not just at the top level. No new checker→emitter side-table; no re-implemented
matching that could drift.

**F — `Result → HttpResult` is a non-goal.** #543's `?`-extension also imagined
lifting a domain `Err` straight into an HTTP status. Deliberately declined:
there is no honest universal status for a domain error, and the design's own
stance (the §20 example; [ADR 0175](0175-oidc-jwks-scheme.md)'s "who, not
whose") is that a `Result[_, DomainError]` is turned into HTTP by **one explicit
`match`** in the service handler. `embeds` composes domain errors
(`Result → Result`); HTTP mapping stays handler policy. `?` never guesses a
status.

## Consequences

- The `.mapErr(toLocalError)` tax on cross-context chains is gone: a sum names
  its embeddings once, and `?` converts. #543's three parts — the §2.8.3
  combinators (0176), the Option→HttpResult lift (0177), and error embeddings
  (this) — are all shipped; **#543 closes**.
- New AST (`SumBody.embeds`, `EmbedsClause`), a contextual-keyword parse, a
  formatter clause, three new validation diagnostics, and a widened `?`
  error-compat check. The `?` lowering gains an embedding-wrap arm; every other
  `?` (plain `Result`, the Option→HttpResult lift) is byte-identical.
- Grammar surface: the tree-sitter `sum_type` rule gains the inline clause (no
  new named rule, so the `grammar.md` bijection is untouched); `grammar.json`,
  the appendix, and the site JSON regenerate; a corpus case is added. No runtime
  change.

## Tooling (ADR 0156)

- **Semantic tokens / Highlighting:** `embeds` is added to tree-sitter
  `highlights.scm` (`@keyword.operator`, alongside `where`/`enum`) and the
  TextMate grammar (contextual lookahead). Semantic-token *legend* unchanged
  (no new symbol kind or modifier).
- **Hover / Completion / Signature help:** unchanged. Following the
  `cors`/`security`/`limits` precedent, the clause-introducing contextual word
  carries no dedicated hover/completion — it is not a member, method, or symbol
  kind. The `textmate_keywords`/`keywords_reference` drift guards concern
  reserved keywords, which `embeds` is not.

## Alternatives considered

- **Structural embedding (no keyword).** Rejected (Decision A) — implicit and
  ambiguous.
- **A reserved `embeds` keyword.** Rejected (Decision B) — a trailing
  clause-word needs no reservation, and reserving it would break any existing
  `embeds` identifier and touch the whole reserved-keyword surface.
- **Transitive conversion.** Deferred (Decision D) — cost (cycles, cross-chain
  ambiguity) outweighs a rare need.
- **Extending `?` to `Result → HttpResult`.** Declined (Decision F) — HTTP
  mapping is an explicit handler `match` by design.
