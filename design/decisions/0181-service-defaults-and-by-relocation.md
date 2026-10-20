# 0181 — Service-level `by`/`given` defaults; `by` relocated after the return type

- **Status:** Accepted (v0.155)
- **Provenance:** design-review finding #544 (Bynk Language Design Review
  2026-07-05, §8 Language P1 #5) — "the handler head is over-dense … and hosts
  the one real visual ambiguity: `by Visitor (page: Page)` reads as a call
  `Visitor(page: Page)` … `by`/`given` repeat per handler with no service-level
  default." This ADR ships two of the finding's three parts and **closes #544**;
  the third (protocol-implied return sugar) is explicitly deferred (Decision E).
- **Realises:** (1) a handler head ordered `on <trigger> (params) -> <ret> [by
  <actor>] [given <caps>]` — the `by` clause moved from before the parameter list
  to after the return type, next to `given`; and (2) an optional `by`/`given`
  default on the service header (`service api from http by Visitor given Clock {
  … }`) that every handler inherits unless it declares its own.
- **Relates:** [ADR 0082](0082-by-clause-verify-then-body-defaults.md) (per-protocol default actors
  — HTTP has none, the gap the service default fills), [ADR 0088](0088-optional-by-binder.md)
  (the optional binder — the prior ceremony-reduction on the same clause),
  [ADR 0156](0156-editor-surface-tracks-language.md) (the tooling-surface discipline this
  increment reports against).

## Context

The handler head is the line users write most, and it carried seven clause slots
plus the language's one real visual ambiguity. `by Visitor (page: Page)` — the
actor clause immediately followed by the parameter list — reads exactly like a
call `Visitor(page: Page)`, the single place the surface reads against its own
`Name(args)` rule.

Separately, the actor and capability facts repeat per handler. HTTP has **no safe
default actor** ([ADR 0082](0082-by-clause-verify-then-body-defaults.md); `default_actor` returns
`None` for `Http`/`WebSocket`), so `by` is mandatory on every route — and a
service is almost always uniformly "public" or "bearer-authed". That uniform fact
was restated on every handler, with no way to say it once.

## The surface

```bynk
service api from http by Visitor {          // service-level default
  on GET("/ping") () -> Effect[HttpResult[View]] {          // inherits `by Visitor`
    Ok(View { ok: true })
  }

  on GET("/healthz") () -> Effect[HttpResult[View]] by User {   // overrides the default
    Ok(View { ok: true })
  }
}
```

The `by` clause is now written after the return type; the service header carries
the default, `by` before `given`.

## Decisions

**A — Relocate `by` after the return type; hard break, no transitional
double-grammar.** The clause moves to sit beside `given` (`… -> T by A given C`),
the two ambient clauses colocated after the value-shaped part of the head. The old
position no longer parses. Pre-1.0 the repo takes clean breaks over carrying two
grammars; the relocation is mechanical, so every in-repo handler, fixture,
example, and doc migrates in the same increment. A transitional "accept both,
warn" grammar was rejected: it keeps alive the very `by Actor (params)` ambiguity
the change exists to remove.

**B — Service defaults live on the header, not in a `defaults { }` body block.**
`service api from http by Visitor given Clock { … }` mirrors the handler order
(`by` then `given`) and keeps the identity fact next to the protocol it qualifies.
A body-position `defaults { }` section (a sibling of `cors`/`security`/`limits`)
was considered but adds a line and separates the default from the protocol; the
header form is the least ceremony for what is usually a one-word fact.

**C — Override, never merge.** A handler that names its own `by` (or its own
`given`) replaces the service default outright; the default fills only an *absent*
clause. Merging (e.g. unioning a handler's `given` with the default's) was
rejected as surprising — a handler's stated capability list should be exactly what
it says. For HTTP there is no "no actor at all" state to opt into: a route made
public against an authed default writes `by v: Visitor`, the ordinary public form,
so no new opt-out token is needed.

**D — Resolve defaults by a post-parse normalization pass, not at each consumption
site.** The parsed AST stays faithful to source (so `bynk fmt`, which parses
independently, round-trips the terse inheriting form). A single normalization pass
over the parsed tree injects each service default into the handlers that omit it,
before grouping/checking. Every downstream consumer — the checker, project
validation, the actor/capability seams, the emitter — then reads fully-formed
`handler.by_clause`/`given` and needs **no default-awareness**. The emitted
TypeScript is byte-identical to spelling every clause out. The injected clause
carries the service-header span, so a diagnostic about a malformed default points
at the header.

Two coverage consequences follow from validating a default *through its injected
copies*, and both are handled:

- A default inherited by **at least one** handler is validated (through those
  copies), but a malformed one reports its diagnostic once **per inheriting
  handler** — all pointing at the header span. An error-path wrinkle only; a valid
  default (the whole point) produces none.
- A default inherited by **no** handler — every handler overrides it — is injected
  into nothing and would escape validation entirely, so an unknown/typo'd default
  actor or capability could pass silently, then surface later at the header the
  moment an override is removed. The checker closes this by validating a service
  default **directly** at the header in exactly that zero-inheritor case (factored
  into one clause-resolution routine shared with the per-handler path, reported
  once). Coverage is therefore symmetric: a malformed default is caught whether it
  is inherited or fully shadowed.

**E — Protocol-implied return sugar is deferred.** The finding's third part —
`on GET("/x") (…) -> Int` meaning `Effect[HttpResult[Int]]` — is not shipped here.
It is a larger, more debatable surface (`-> Int` reads as "returns Int" while the
body still returns `HttpResult` variants), separable from the two edits above, and
left to a later increment so this one stays a focused, mechanical relocation plus
an additive default.

## Consequences

- **Grammar/AST:** the `by`-clause parser moves after the return type and is
  shared with a new service-header default; `ServiceDecl` gains `default_by` /
  `default_given`. `Handler` is unchanged in shape.
- **Checker/emitter:** unchanged — they consume the normalized handlers. The
  `missing_by_on_http` check stops firing for a handler that inherited a default
  and still fires when there is neither a handler `by` nor a service default.
- **Tooling (ADR 0156):** `by`/`given` remain keywords; hover, completion,
  semantic tokens, and signature help are unchanged.
- **Migration:** every in-repo handler head, the tree-sitter grammar + corpus, the
  VS Code snippets, and the Book move to the new order in this increment.
