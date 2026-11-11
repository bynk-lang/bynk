---
level: minor
probes: emit_diagnostics, emit_abi_shapes, ts_writes
changelog: `bynk.cloudflare.Email` (experimental) sends mail through Email Service from a named instance, with typed send errors; `bynk deploy` checks each `@allowedSenders` domain and never configures one
---

## ADR: outbound-email-instance-kind
title: Outbound email is an `Email` instance kind over `send_email`, with typed send errors
summary: A list-valued instance option, a binding that checks limits before the call, and a deploy-side sender-domain check

**Context.** A Worker could not send transactional mail from Bynk. Cloudflare's
`send_email` binding (Email Service) takes a message builder and returns
`{ messageId }`, or throws an `Error` with a `.code` (`E_SENDER_NOT_VERIFIED`,
`E_RECIPIENT_NOT_ALLOWED`, `E_RATE_LIMIT_EXCEEDED`, `E_DAILY_LIMIT_EXCEEDED`,
and others). Proposal #3302 (parent #3184); inbound email (`from email`) is out
of scope.

**Decision.**

- **A. Errors.** A platform refusal is a value: `send` returns
  `Result[String, EmailError]`, with `SenderNotVerified`, `RecipientNotAllowed`,
  `RateLimited`, `DailyLimit` and `SendFailed(message)` for any other code
  (`EmailError` is extensible). Breaking a documented limit (51 recipients, 33
  attachments, 25 MiB, neither `text` nor `html`, 16 KB of custom headers)
  faults before the call, as `Analytics.write` does. The catch-all is `SendFailed`,
  not the proposal's `Failed`: `SqlError` already has a `Failed` variant in the
  same unit, and a bare `Failed(…)` then resolved to `SqlError`'s with no
  diagnostic, a trap for every user. Renamed on review of #3330; the name
  resolution itself is a separate follow-up.
- **B. Addresses** are `String`; a refinement can follow in a commons.
- **C. Restrictions** are a closed set of instance annotations, `@to`,
  `@allowedRecipients` and `@allowedSenders`, written to `[[send_email]]`
  (keyed by `name`) as `destination_address`, `allowed_destination_addresses`
  and `allowed_sender_addresses`. The registry gains a list-of-strings option
  kind (`OptionKind::StrList`); each address is checked as `local@domain`
  with no whitespace, control, angle-bracket, quote or comma character.
- **D. Sender domains.** The deploy checks, never configures. For each distinct
  `@allowedSenders` domain it finds the zone (the domain, then its parents) and
  asks whether Email Sending is enabled on the domain or a covering wildcard,
  and fails before the confirmation, with every problem named. Cloudflare's API
  has no "verified" field, so `enabled` is the signal; a domain still
  propagating can pass and surface at run time as `SenderNotVerified`. The
  kind is not `linked`: nothing is looked up and recorded in the ledger, so the
  check lives beside `linked.rs` in `senders.rs` and adds no ledger state.
- **E. Header injection.** `headers` may not set `From`, `To`, `Cc`, `Bcc`,
  `Subject` or `Reply-To` (any case), nor hold a line break in a name or value,
  nor use an empty or colon-, space- or control-bearing name; each faults before
  the call naming the header, never echoing a value. Message bodies, header
  values and attachment content are never logged and never in a fault.
- **F. Test double.** The unit double is the binding's own `WorkersEmail` over
  the integration tier's in-memory outbox (`makeIntegrationEmail`), so the
  limit and header checks are the binding's, not a copy; `expect Mail.send
  called with …` needs no stub and a stub can force an `EmailError`. Each
  `system` participant binds its own outbox.
- **G. Stability.** Experimental.
- **Attachments** go to the platform as base64 strings, not `ArrayBuffer`s:
  the local simulator cannot serialise a buffer, and accepts the text (shown on
  `wrangler dev` by the workerd smoke), so no `bynk dev` warning or `--remote`
  advice is needed. An inline attachment's content id is its filename.
- **Names.** The records are `EmailMessage` and `EmailAttachment`, not
  `Message` and `Attachment`: a context that consumes `bynk.cloudflare`
  whole and declares its own `Message` would otherwise fail with
  `bynk.consumes.name_conflict`.

**Consequences.** `bynk.cloudflare` exports three more types, so the generated
type-import lists of existing fixtures move. Rejected: a refined address type (a syntax rule the platform may not
share); configuring a sender domain from the deploy (DNS belongs to the account);
`ArrayBuffer` attachments (the simulator cannot serialise them).
