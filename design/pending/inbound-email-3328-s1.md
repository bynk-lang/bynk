---
level: minor
probes: explained_codes, emit_diagnostics, diagnostic_coverage
changelog: A service can receive email, `service support from email("support@shop.example")`, with an `on message(mail: InboundEmail) -> Effect[EmailVerdict]` handler tested in a suite; the Workers mapping follows (#3328 slice 1)
---

## ADR: inbound-email-protocol
title: Inbound email joins the closed protocol set as `from email(…)`, with a typed verdict
summary: The seventh protocol, its verdict and actor, and the decisions slice 2 follows

**Context.** A Bynk service could not receive mail. Cloudflare delivers inbound
mail to a Worker's `email(message, env, ctx)` export. The protocol set is
closed ([ADR 0079](0079-protocols-closed-set.md)), so admitting a protocol is a
deliberate extension, as Events was
([ADR 0285](0285-events-protocol-set-extension.md)). Proposed and accepted in
#3328 (parent #3184), sliced as `from websocket` was
([ADR 0131](0131-from-websocket-protocol-bundle.md)): slice 1 is the surface,
the checker and a runnable test vertical; slice 2 is the Workers mapping. The
coordinator moved MIME parsing from slice 1 to slice 2, because it needs
`postal-mime` emitted as a first-party npm dependency (ADR 0799 C), which is
not built yet. This record covers every decision, so slice 2 follows it.

**Decision.**

- **A. The header.** `from email("addr")`, the seventh member of the set. The
  address is a bare `local@domain` or a domain's catch-all `*@domain`. `email`
  is a contextual identifier, matched as plain `Ident` text as `websocket` is,
  not a reserved word. Several services may exist, dispatched by the envelope
  recipient: an exact address wins over the catch-all, and a recipient no
  service claims is rejected with a fixed reason (slice 2). Two services
  claiming the same address, compared case-insensitively, is
  `bynk.email.duplicate_address`; slice 1 checks this per context, and slice 2's
  routing emission, which sees every context, extends it across contexts. An
  address Email Routing would not match is `bynk.email.invalid_address`.
- **B. The verdict.** `EmailVerdict = Accept | Reject(reason) | Forward(to) |
  Reply(message)`, by the agency rule of [ADR 0078](0078-queueresult-typed-verdict.md):
  the handler makes a dispatch decision independent of success or failure. One
  decision per message. A forward to several destinations, or a forward plus a
  reply, can be added later (`ForwardAll(List[String])`).
- **Where the types live.** Unlike `QueueResult`, which is hard-wired through
  the checker, `InboundEmail`, `InboundAttachment`, `EmailVerdict`, `EmailReply`,
  `DmarcVerdict` and `SenderIdentity` are ordinary declared types exported by
  `bynk.cloudflare` (beside the outbound `EmailMessage`, ADR 0804). A test must
  *construct* an `InboundEmail`, which a hard-wired opaque built-in cannot
  support, and a hard-wired verdict would add a `TypeRef`/`Ty` arm at about
  forty sites for no gain. The cost is that a context imports them with
  `consumes bynk.cloudflare { … }`, and that the generated type lists of the
  fixtures that consume `bynk.cloudflare` move. The handler-shape check compares
  the written type names; the body is then checked against the real declarations.
- **Variant names.** The four verdict names collide with no variant of
  `HttpResult`, `QueueResult` or any `bynk.cloudflare` export (a fixture uses
  them beside `SqlError`'s `Failed`, `EmailError`'s `SendFailed` and
  `KvExpiration`'s `Never`). The DMARC verdict is the one that would have: the
  proposal's `Pass | Fail | None` puts `None` beside `Option`'s, the bare-name
  trap that resolved silently to one of two enums, so its variants are
  `DmarcPass | DmarcFail | DmarcNone`.
- **C. The reply is constrained by construction.** `EmailReply` carries only
  content (`subject`, `text`, `html`). The runtime fills in `From` (the
  receiving address), `To` (the sender), `In-Reply-To` and `References`, so
  two of the platform's preconditions cannot be broken and the others surface
  as a typed outcome (G). MIME is written in slice 2, by a small first-party
  writer or `mimetext` pinned under ADR 0799 C, decided on size and
  `nodejs_compat` needs. Types are shared with #3302's `EmailMessage` only
  where they genuinely coincide; they do not today.
- **D. Parsing.** Slice 2: `postal-mime`, pinned exactly, parses the message
  into `InboundEmail` before the handler runs. A parse failure is rejected
  with a fixed reason and logged without the body. The message is untrusted
  input and needs a security review (R-16) and a fuzzed boundary. In slice 1
  `text`, `html` and `attachments` are fields a test builds.
- **E. The actor.** `Sender` is the default actor of a `from email` handler:
  scheme `Internal` (the platform authenticates before the handler runs), and
  the only scheme admitted. Its identity is `SenderIdentity { address,
  dmarc: DmarcVerdict }`, the DMARC verdict read only from the first
  `Authentication-Results` header with authserv-id `mx.cloudflare.net`.
  `InboundEmail.from` is documented as a claim. Slice 1 declares the type and
  the actor; the Worker mints the identity, so until slice 2 `s.identity`
  reads as `()` in a suite, as for the other platform-tag actors. A scheme
  that refuses anything but `Pass` can follow.
- **F. Routing.** Slice 2 emits wrangler's top-level `addresses` from the
  services' headers; `wrangler deploy` reconciles the rules. `bynk deploy` shows
  the managed addresses in its plan, a removal goes behind the existing prune
  confirmation (`[]` deletes rules), and the literal `Forward` destinations
  are checked as verified addresses where the linked-resource lookup of #3301
  can, as a warning. It needs wrangler 4.113.0 or later, delivered by H0b (#3317).
- **G. A fault or an unsettled outcome.** A handler fault rejects with a
  fixed, non-revealing reason and is logged. A `Reply` the platform refuses
  (no DMARC pass, a second reply) falls back to `Reject("reply refused")` and is
  logged. The platform's behaviour on a throw is undocumented, so Bynk
  defines its own, and a workerd smoke pins it in slice 2.
- **H. Tests and dev.** A suite drives a handler with
  `support.message(InboundEmail { … })` and asserts `expect r is Forward(…)`;
  the record's defaults are the builder. At the `system` tier slice 2's harness
  posts raw MIME to the Worker's email entry, and `bynk dev` documents
  `POST /cdn-cgi/local/email` and may offer `--send-email <file.eml>`.
- **I. Platform lock.** Cloudflare-only. A context declaring a `from email`
  service is platform-native to `cloudflare` even where it reaches no
  capability (the types alone would not lock it), so `--platform node` or
  `browser` is `bynk.target.vendor_required`.
- **The slice-1 boundary.** A deployable `--target workers` build of a
  `from email` service is `bynk.email.workers_unsupported` ("inbound email is not
  yet emitted for Workers (#3328 slice 2)") rather than half-working output.
  `bynk test` runs on the bundle target and is unaffected.

**Consequences.** The set has seven members. The checker gains five codes
(`bynk.email.invalid_address`, `duplicate_address`, `bad_params`,
`return_not_email_verdict`, `duplicate_handler`) and the slice-1 refusal.
`bynk.cloudflare` exports six more types, so the golden type lists of the
fixtures that consume it move. Editors: `email` completes after `from`,
the verdict's names complete on `EmailVerdict.`, tree-sitter highlights `email`
as a protocol keyword, semantic tokens add no class and signature help is
unchanged (handlers are not called by name). Rejected: a hard-wired
`TypeRef::EmailVerdict` (cannot be constructed in a test, forty arms), and a
new reserved word `email` (would break user identifiers).
