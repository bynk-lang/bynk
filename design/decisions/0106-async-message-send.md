# 0106 — Asynchronous message send (`~>`): the caller chooses the message form, an error gate keeps it safe, and a distinct glyph keeps the call site honest

- **Status:** Accepted (v0.79; 2026-06-23). **D2 and D3 superseded, D1 stands; `~>` deprecated by [[0824]]**: a detached reply is never silently lost, because `e.detach()` reports a failure and a successful value is discarded by an explicit `detach`.
- **Spec:** `site/src/content/docs/book/spec/syntactic-grammar.md` §4.8.5; `site/src/content/docs/book/spec/static-semantics.md` §5.5; `site/src/content/docs/book/spec/emission.md` §7.3.2; the v0.79 async-message-send proposal (deleted on merge).
- **Realises:** the `let _ <- Logger.info(...)` wart on unit-returning effects, and pre-establishes the call-site form for the designed-but-unshipped async channels (`Events`, `Push`, `Queue`).
- **Relates:** corrects `design/bynk-design-notes.md` §1071 (its convention of reusing `<-` as a fire-and-forget leading marker is superseded by `~>`).

## Context

`Effect[T]` conflates two independent axes: whether a reply carries a **value**,
and whether the caller must **wait**. A durable `Kv.putTtl(...) -> Effect[()]`
returns unit yet MUST be awaited (read-your-writes, atomic commit; design notes
§388); a `Logger.info(...) -> Effect[()]` returns unit and need *not* be awaited.
Before v0.79 the language had only one effect statement — `let <name> <- e`,
always a binder, always an await — so "fire-and-forget" had no spelling, and the
design notes (§1071) proposed reusing `<-` *without a binder* for it. That
rebuilds the very ambiguity it would relieve: a reader could not tell from a bare
`<- X.y()` whether the next line waits.

## Decisions

**D1 — The caller chooses the message form; the operation only publishes a
contract.** An operation's signature stays `-> Effect[...]`, describing the
*reply*. There are **no `call`/`cast` handler keywords and no `Oneway` return
type**. Sync-versus-async is a call-site choice, bounded by what the contract
permits. Rationale: the recipient cannot know whether *this* caller needs the
completion, so the choice belongs at the call site, not the declaration.

**D2 — An error gate restricts `~>` to `Effect[()]`.** A send drops its reply, so
the reply MUST be unit (`bynk.send.requires_unit` for a non-unit `Effect[T]`;
`bynk.send.non_effect` for a non-effect; `bynk.send.in_pure_context` outside an
effectful body). This is broader than "no error": a value *or* an error would be
silently discarded, so the rule is "must be `Effect[()]`", not merely "must not
error". The honest spelling for "await and discard a real reply" stays
`let _ <- e`. The await×value grid is total:

|              | await            | don't await           |
|--------------|------------------|-----------------------|
| **value**    | `let r <- e`     | forbidden (D2)        |
| **no value** | `let _ <- e`     | `~> e` (`Effect[()]`) |

**D3 — The marker is a leading `~>` glyph, not `<-` and not a keyword.** It MUST
differ from `<-` (reusing the await arrow rebuilds the conflation). A `send`
keyword was rejected: a hard keyword collides with `Fetch.send` / the designed
`Queue.send` because member names after `.` are parsed as identifiers, and a
*contextual* keyword needs a statement-start disambiguation rule. The `~>` glyph
has zero identifier collision and is LL(1) at statement head (no expression
begins with `~`), satisfies the surface constraints (ligature-friendly,
parse-tractable), and coincides with the author's own notation (`op(args) ~>
Recipient`). A regression fixture asserts `Fetch.send(req)` still parses as a
member call.

## Consequences

- **Emission, one tier only.** `~> e` lowers on the Workers target to
  `deps.__exec.waitUntil(<e>)` — the execution context is threaded
  `fetch`/`scheduled`/`queue` → `compose(env, ctx)` → `deps.__exec`, gated on the
  context containing a send so non-sending contexts stay byte-identical. This is
  the *immediate, best-effort* delivery tier. The *buffered/at-commit* tier
  (DECISION E in the proposal) is **deferred** to the `Events` increment: nothing
  shipped uses it, and building it now would be untestable dead code.
- **No migration yet.** First-party `Logger`, the examples, and the guides keep
  `let _ <- Logger.info(...)`; re-spelling them to `~>` is a named follow-on,
  gated on a second async channel existing to justify the visible churn.
- The `capability`/tier mechanism (how a capability declares immediate vs
  at-commit) is **not** designed here; it rides with `Events`.
