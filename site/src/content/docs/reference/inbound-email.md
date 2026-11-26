---
title: Inbound email
---
An email service receives mail sent to an address and decides what becomes of
it. Like HTTP, cron and queue handlers, it is declared in a `service` inside a
`context`; the address is bound on the service header (`from email("…")`), and
the handler is an `on message(…)`.

:::note
Inbound email is **experimental** ([#3328](https://github.com/bynk-lang/bynk/issues/3328)).
:::

## Handler form

```bynk,ignore
service <Name> from email("<address>") {
  on message(mail: InboundEmail) -> Effect[EmailVerdict] by Sender {
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
with the fixed reason `recipient not accepted`.

### Naming an address with a constant

An address written once can be shared by a service, a `Forward` in another service
and a test: `from email(…)` takes a string literal, or the name of a constant.

```bynk,ignore
commons shop.addresses

const Support: String = "support@shop.example"
const Billing: String = "billing@shop.example"
```

```bynk,ignore
context shop.support
uses shop.addresses

service support from email(Support) {
  on message(mail: InboundEmail) -> Effect[EmailVerdict] by Sender {
    Forward(Billing)
  }
}
```

The name resolves to its literal at compile time, so routing, dispatch, the
project-wide duplicate check, the generated route table, `wrangler.toml`'s
`addresses` and the deploy ledger are all the literal text. A constant is accepted
when it is declared exactly `String` and is bound to a literal or to another such
constant, in the same file, a sibling file or a used commons. A constant that is not
(a refined type such as `String where MaxLength(40)`, an `Int`) is
`bynk.email.invalid_address`, with a note naming the constant and its declared
type; an unknown name is `bynk.resolve.unknown_name`, and a private constant of
another unit is `bynk.visibility.private_item`. Two services claiming the same
address, one through a constant, are `bynk.email.duplicate_address`.

Routing is project-wide: Email Routing keeps one rule per address on the
account, so two *contexts* claiming the same address is also
`bynk.email.duplicate_address`. Dispatch runs on the envelope recipient
(`message.to`), the local part compared without regard to case. The order is an
exact claim on the recipient, then a claim on its base address (see below), then
the domain's catch-all.

### Plus-addressed mail

Email Routing supports subaddressing (RFC 5233). With it enabled for a domain,
`support+1234@shop.example` is matched by the rule for `support@shop.example`, and
the `+1234` part stays in `message.to`. Dispatch does the same: a recipient with a
`+` in its local part goes to a service claiming exactly that address first
(`from email("support+vip@shop.example")`, which stays legal and wins), then to
the service claiming its **base address**, the local part up to the first `+`
(`support@shop.example`), then to the catch-all. A tagged recipient with no claim
on its base address and no catch-all is rejected `recipient not accepted`.

The handler reads the tag as `mail.subaddress: Option[String]`: the text after the
first `+` of the recipient's local part, `None` without one. `to` keeps the full
recipient, and the case the sender wrote.

```bynk,ignore
service support from email("support@shop.example") {
  on message(mail: InboundEmail) -> Effect[EmailVerdict] by Sender {
    match mail.subaddress {
      Some(ticket) => Forward("tickets@shop.example"),
      None => Accept,
    }
  }
}
```

Subaddressing must be enabled in the domain's Email Routing settings. If it is off,
Cloudflare never routes a tagged recipient to the Worker, so the extra step never
runs. A project that relied on a catch-all to receive tagged mail now has it taken
by the base address's service, as Cloudflare's own rule order does.

## The message

`InboundEmail`, `EmailHeader`, `EmailVerdict`, `EmailReply`, `InboundAttachment`,
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
| `messageId` | `String` | The `Message-ID` header, as written (`<id@host>`). |
| `inReplyTo` | `Option[String]` | The message id the sender says this answers, as `messageId` is written. A claim. |
| `references` | `List[String]` | The `References` message ids, in the message's order. A claim. |
| `replyTo` | `List[String]` | The `Reply-To` addresses, bare (display names dropped). A claim: where to answer. |
| `cc` | `List[String]` | The `Cc` addresses, bare. A claim: who else received it, as the sender wrote it. |
| `date` | `Option[Instant]` | The sender's `Date` header. A claim, not the arrival time; `None` when it is missing or cannot be read. An RFC 5322 date has whole seconds, so a `system` case that sets `date` gets back its whole-second value. |
| `headers` | `Map[String, String]` | The headers, by lower-case name, the first occurrence of each. |
| `headerList` | `List[EmailHeader]` | Every header occurrence, in the order they appear, by lower-case name. |
| `text` | `Option[String]` | The plain-text body, if there is one. |
| `html` | `Option[String]` | The HTML body, if there is one. |
| `attachments` | `List[InboundAttachment]` | The files attached. |
| `size` | `Int` | The raw message, in bytes. |
| `subaddress` | `Option[String]` | The tag after the first `+` of the recipient's local part (`support+1234@…` gives `Some("1234")`), `None` without one. See [Plus-addressed mail](#plus-addressed-mail). |

Every field after `to` has a default, so a test builds a message with only what
it cares about. On a Worker the runtime fills the rest by parsing the raw
message with [`postal-mime`](https://github.com/postalsys/postal-mime) before
the handler runs; a test builds the fields directly.

`headers` keeps the first occurrence of each header, so a repeated one (`Received`) is read from `headerList`, an `EmailHeader` of `name` and `value`. Changing `headers` to hold every occurrence would break every handler that reads it, so it stays. `replyTo` and `cc` hold the address part only: a display name is dropped, as `from` and `to` are envelope addresses, and an address group is unwrapped. Every one of these is something the sender wrote, so treat it as a claim as you do `from`. They count toward the limits below: `headerList` cannot grow past the header-byte cap, and nothing new is logged. `attachments` carries each
part's decoded bytes. A message with no `Message-ID` has an empty `messageId`,
and a reply to it carries no `In-Reply-To` or `References`.

### Limits

The message is untrusted input. The Worker refuses, with a fixed reason and
without logging any of it, a message that:

| Limit | Value |
|---|---|
| Raw size (the platform's own cap) | 25 MiB; the stream is cut off at the cap whatever it declared |
| MIME parts | 200 |
| MIME nesting depth | 20 |
| Header bytes, over every part | 256 KiB |
| Inline `message/rfc822` nesting | 3 (deeper ones stay attachments) |
| Attachments | 100 |

A message over the size limit is rejected `message too large`; one over any
other limit, or one the parser cannot read, `message could not be read`.

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

A handler names its actor with `by`, and `by Sender` is the **`Sender`** actor. Omitting it is `bynk.actor.missing_by_on_email`, and `bynk check --fix=bynk.actor.missing_by_on_email` inserts `by Sender`. Its
scheme is the platform's own (`Internal`): Email Routing delivers and
authenticates the mail before the handler runs. Its identity is a
`SenderIdentity`, the envelope `address` and a `DmarcVerdict`, read only from
the **topmost** `Authentication-Results` header, and only if its authserv-id is
`mx.cloudflare.net`: Email Routing prepends its own, so a copy the sender wrote
sits below it. If the topmost header is another server's, or there is none, the
verdict is `DmarcNone`; the search never goes further down, because every lower
header can be the sender's.
Treat `InboundEmail.from` as a claim, and `dmarc` as the evidence.

To act only on authenticated mail, refine `Sender` rather than testing the verdict
by hand: `actor Authenticated = Sender where Dmarc(DmarcPass)`, then `by s: Authenticated`.
Mail with any other verdict is refused with `sender not authenticated` before the
handler runs, and `!Dmarc(DmarcFail)` is the more lenient rule that also admits mail
from a domain with no DMARC policy. See [Refinements over `Sender`](/reference/actors/).

Bind it to read it, and import the types it names:

```bynk,ignore
consumes bynk.cloudflare { InboundEmail, EmailVerdict, DmarcVerdict, SenderIdentity }

service support from email("support@shop.example") {
  on message(mail: InboundEmail) -> Effect[EmailVerdict] by s: Sender {
    if s.identity.dmarc == DmarcPass { Accept } else { Reject("sender not verified") }
  }
}
```

In a suite there is no Email Routing, so a handler reads the verdict from the case's own
`InboundEmail.headers`, by the Worker's rule: an `authentication-results: mx.cloudflare.net;
dmarc=pass` header reads `DmarcPass` at `unit`, `integration` and `system` alike, and a case with
no such header reads `DmarcNone`. A handler bound to a `Sender` refinement refuses a case's mail
the same way the Worker does, answering `Reject("sender not authenticated")` without running its
body.

## Faults

Every outcome is settled by the runtime; nothing escapes as a throw, because the
platform does not document what a throw does, so Bynk defines the behaviour:

| What happened | Outcome | Logged |
|---|---|---|
| The handler faulted, or returned something that is not a verdict | `Reject("message could not be processed")` | the fault's class name |
| The message could not be parsed, or is over a limit | `Reject("message could not be read")` or `message too large` | the parser's error code |
| A `Reply` the platform refuses (the sender failed DMARC, a second reply) | `Reject("reply refused")` | the error's class name |
| A `Forward` the platform refuses (the destination is not verified) | `Reject("forward refused")` | the error's class name |

**Nothing in the message is ever logged**: not its body, headers, attachments or
envelope. A log line is a fixed string, the address the service claims, and an
error code or class name. Log inside the handler, through the `Logger`
capability, if you want more.

## Replies

A `Reply` builds a message with `From` set to the receiving address, `To` the
sender, and `In-Reply-To` and `References` from the incoming `Message-ID`, then
hands it to the platform's `reply`. `subject` defaults to `Re: ` and the incoming
subject. Cloudflare allows one reply per message, and only to a sender whose
mail passed DMARC.

The MIME is written by a small first-party writer, not a library: headers are
RFC 2047 encoded when not plain ASCII, folded, and written with CRLF line
endings; bodies are base64; a handler's text and HTML become a
`multipart/alternative`. An address with a character a header could be split by,
or a line break in a subject, cannot reach a header.

## Routing

`bynk build --target workers` lists each Worker's addresses in its
`wrangler.toml` as wrangler's top-level `addresses`, lower-cased and sorted:

```toml
addresses = ["*@shop.example", "support@shop.example"]
```

`wrangler deploy` reconciles Email Routing rules to that list. It applies to
every environment, so it is top-level and is never copied into an `[env.*]`
table. See [Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/).

### Environments

The addresses a service names are the `default` environment's. A named environment
(`bynk deploy --env staging`) **claims no address** unless `bynk.toml` maps it one:

```toml
[deploy.email."support@shop.example"]
address = { staging = "support@staging.shop.example" }
```

`bynk deploy --env staging` then writes `addresses = ["support@staging.shop.example"]`
for that environment, and the Worker accepts mail to the mapped address and hands it
to the same service as the source address. With no entry for `staging`, the deploy
leaves `addresses` out, which Wrangler documents as changing no Email Routing rule, so
staging cannot take production's rules over. `default` is never mapped.

A key must be an address a `from email` service names; a mapped address must pass the
same rules as one written in a header (`bynk.email.invalid_address`); and no address
may be claimed by two environments, or by the source and an environment. See the
[manifest reference](/reference/tooling/manifest/#mapping-inbound-email-addresses-per-environment).

## Running it locally

`bynk dev` serves a Worker under `wrangler dev`, which accepts a raw message at
`/cdn-cgi/local/email`:

```sh
curl -X POST 'http://localhost:8787/cdn-cgi/local/email?from=a@example.org&to=support@shop.example' \
  --data-binary @message.eml
```

The body must be an RFC 5322 message and must include a `Message-ID` header.
Local mail carries no `Authentication-Results` header, so the handler sees
`DmarcNone`; add `Authentication-Results: mx.cloudflare.net; dmarc=pass` to the
file to try the authenticated path.

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

| | |
|---|---|
| Syntax, checks, `bynk check`, editor support | ✓ |
| `bynk test` driving a handler | ✓ |
| Parsing the raw message into `InboundEmail` | ✓ |
| The Worker's `email()` entry and dispatch by recipient | ✓ |
| Reply MIME and `addresses` routing | ✓ |

`bynk.email.workers_unsupported` (historical) was retired once the Worker
mapping landed.
