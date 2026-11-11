---
title: Inbound email
---
An email service receives mail sent to an address and decides what becomes of
it. Like HTTP, cron and queue handlers, it is declared in a `service` inside a
`context`; the address is bound on the service header (`from email("…")`), and
the handler is an `on message(…)`.

:::note
Inbound email is **experimental**, and arrives in two slices of
[#3328](https://github.com/bynk-lang/bynk/issues/3328). The surface, the
checks and test-driving handlers in a suite ship first. Parsing the raw message,
the Worker's `email()` entry, and routing are still to come, so a
`from email` service does not build for `--target workers` yet
(`bynk.email.workers_unsupported`). See [Status](#status).
:::

## Handler form

```bynk,ignore
service <Name> from email("<address>") {
  on message(mail: InboundEmail) -> Effect[EmailVerdict] {
    …
  }
}
```

- **Address:** the recipient this service claims, on the header: a bare address
  (`"support@shop.example"`) or a whole domain's catch-all (`"*@shop.example"`).
  An address Email Routing would not match (a display name, a list, spaces) is
  `bynk.email.invalid_address`.
- **Parameter:** exactly one, an `InboundEmail` (`bynk.email.bad_params`).
- **Return type:** `Effect[EmailVerdict]` (`bynk.email.return_not_email_verdict`).
- **One handler:** a service has exactly one `on message`
  (`bynk.email.duplicate_handler`). To treat two addresses differently, declare
  two services.

`email` is a contextual word, like `websocket`: it is a protocol name only
after `from`, and stays an ordinary identifier everywhere else.

## Addresses

Mail is dispatched to a service by its **envelope recipient**, so each address
belongs to one service. Two services claiming the same address (compared
without regard to case) is `bynk.email.duplicate_address`. A literal address
and the catch-all for its domain may both be claimed: the literal address wins,
and the catch-all takes the rest. A recipient no service claims is rejected
with a fixed reason.

## The message

`InboundEmail`, `EmailVerdict`, `EmailReply`, `InboundAttachment`,
`DmarcVerdict` and `SenderIdentity` are exported by `bynk.cloudflare`; import
what the handler names:

```bynk,ignore
consumes bynk.cloudflare { InboundEmail, EmailVerdict, EmailReply }
```

| Field | Type | Meaning |
|---|---|---|
| `from` | `String` | The **envelope** sender. A claim: anyone can write any address. |
| `to` | `String` | The envelope recipient the service was routed by. |
| `subject` | `String` | The `Subject` header. |
| `messageId` | `String` | The `Message-ID` header. |
| `headers` | `Map[String, String]` | The headers, by lower-case name. |
| `text` | `Option[String]` | The plain-text body, if there is one. |
| `html` | `Option[String]` | The HTML body, if there is one. |
| `attachments` | `List[InboundAttachment]` | The files attached. |
| `size` | `Int` | The raw message, in bytes. |

Every field after `to` has a default, so a test builds a message with only what
it cares about. The Worker will fill `text`, `html` and `attachments` by
parsing the raw message (a later slice); a test builds them directly.

## The verdict

The handler **decides**, and the decision is named, as a queue handler's is
with `QueueResult` (the agency rule, ADR 0078):

| Verdict | Effect |
|---|---|
| `Accept` | Handled; nothing else happens. |
| `Reject(reason)` | A permanent SMTP rejection carrying `reason`. |
| `Forward(to)` | Pass the message to one destination, which must be a verified destination address in the Cloudflare account. |
| `Reply(message)` | Answer the sender, with an `EmailReply` of `subject`, `text` and `html`. The runtime fills in `From`, `To`, `In-Reply-To` and `References`, so a reply can only go to the sender, from the address that received the mail. |

One decision per message. Forwarding to several destinations, or forwarding and
replying, is not expressible yet.

`Accept`, `Reject`, `Forward` and `Reply` are the verdict's own bare names. They
collide with no other built-in or first-party variant. The sender's DMARC result
is `DmarcPass`, `DmarcFail` or `DmarcNone` for the opposite reason: a bare
`None` would be `Option`'s.

## The sender

A handler runs as the **`Sender`** actor unless it names another with `by`. Its
scheme is the platform's own (`Internal`): Email Routing delivers and
authenticates the mail before the handler runs. Its identity is a
`SenderIdentity`, the envelope `address` and a `DmarcVerdict`, read only from
the first `Authentication-Results` header from `mx.cloudflare.net`. Treat
`InboundEmail.from` as a claim, and `dmarc` as the evidence.

## Faults

A handler that faults rejects the message with a fixed, non-revealing reason and
logs the fault. A `Reply` the platform refuses (the sender failed DMARC, or a
second reply) falls back to `Reject("reply refused")` and is logged. The
platform does not document what happens on a throw, so Bynk defines this.

## Testing

A suite drives the handler by its service, as it does a queue's:

```bynk,ignore
suite shop.support {
  case "a refund is forwarded to billing" {
    let r <- support.message(InboundEmail {
      from: "a@b.example",
      to: "support@shop.example",
      subject: "refund",
    })
    expect r is Forward("billing@shop.example")
  }
}
```

## Platform

Inbound email is Cloudflare's. A `from email` service in a `--platform node` or
`browser` build is `bynk.target.vendor_required`.

## Status

| | Today | Coming |
|---|---|---|
| Syntax, checks, `bynk check`, editor support | ✓ | |
| `bynk test` driving a handler | ✓ | |
| Parsing the raw message into `InboundEmail` | | ✓ |
| The Worker's `email()` entry and dispatch by recipient | | ✓ |
| Reply MIME, `addresses` routing, the deploy plan | | ✓ |
