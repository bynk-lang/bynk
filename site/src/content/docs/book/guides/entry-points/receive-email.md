---
title: Receive email
---
**Goal:** receive mail sent to an address, and decide per message whether to
accept it, reject it, forward it to a person, or reply.

:::note
Receiving email is **experimental**
([#3328](https://github.com/bynk-lang/bynk/issues/3328)).
:::

An email service goes in a `service` inside a `context`. The address is bound
on the header, `from email("…")`; the handler is an `on message` that takes the
`InboundEmail` and returns an `EmailVerdict`.

## Decide a message

```bynk,ignore
context shop.support

consumes bynk.cloudflare { InboundEmail, EmailVerdict, EmailReply }

service support from email("support@shop.example") {
  on message(mail: InboundEmail) -> Effect[EmailVerdict] by Sender {
    if mail.size > 10000000 {
      Reject("too large")
    } else if mail.subject == "refund" {
      Forward("billing@shop.example")
    } else {
      Reply(EmailReply { text: Some("Thanks, we have your message.") })
    }
  }
}
```

The handler never calls a mail API: it returns a verdict and the runtime acts
on it. `Accept` ends it quietly, `Reject(reason)` refuses it for good,
`Forward(to)` passes it to one verified destination, and `Reply(message)`
answers the sender, with `From`, `To` and the threading headers filled in for
you.

## Catch a whole domain

`from email("*@shop.example")` claims every address on the domain that no other
service names. Two services cannot claim the same address; a named address and
the catch-all together are fine, and the named address wins.

## Share an address through a commons

An address often appears in more than one place: the service header, a `Forward` in
another service, a test. Write it once as a constant and name it in the header:

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

The constant is declared `String`, and the compiler resolves it to its literal, so
routing and deploy see `support@shop.example` as if you had written it there. A
change is made in one place, and two services that end up claiming the same address
are still `bynk.email.duplicate_address`.

## Don't lose mail when something fails

When the handler faults, or a message cannot be parsed, the message is rejected, which is a
permanent SMTP error: the sender is told it was not accepted, even if the cause was a few
seconds' outage of a database. A `fallback` forwards such mail to a verified address instead:

```bynk,ignore
service support from email("support@shop.example") {
    fallback { forward: "ops@shop.example" }

    on message(mail: InboundEmail) -> Effect[EmailVerdict] by Sender given Tickets { … }
}
```

The forward carries `X-Bynk-Fallback: handler-fault` (or `unreadable`) so whoever receives it
knows why. The address must be a verified destination address on your Cloudflare account, and not
one of your service addresses. Mail over the size limit, a `Reply` the platform refuses and a
`Forward` it refuses are still rejected.

To never reject at all, accept every message and put the work on a queue, so an outage delays
processing instead of bouncing mail. The queue brings its retries and a dead-letter queue (see
[Receive a queue message](/book/guides/entry-points/queue/)):

```bynk,ignore
service support from email("support@shop.example") {
    on message(mail: InboundEmail) -> Effect[EmailVerdict] by Sender given Work {
        let _ <- Work.sendValue(Ticket { from: mail.from, subject: mail.subject, text: mail.text })
        Accept
    }
}
```

A queue message is at most 128 KB, so put the parsed fields on the queue, not the attachments.

## Do not trust `from`

`mail.from` is the envelope sender, and anyone can write any address there. The
handler states its actor with `by` (`by Sender` is "any mail"), and `Sender` carries the address and whether
Cloudflare found the sender's domain authenticated with DMARC
(`DmarcPass`, `DmarcFail` or `DmarcNone`). Forward or reply on a claim; act
on the evidence. To act only on authenticated mail, name a refinement of `Sender`
(`actor Authenticated = Sender where Dmarc(DmarcPass)`) in the `by` clause, and the
Worker refuses other mail before your handler runs.

## Deploy it

`bynk build --target workers` gives the Worker an `email()` export and lists the
addresses in its `wrangler.toml`; `bynk deploy` pushes it, and Cloudflare Email
Routing delivers mail for those addresses to it. Turn Email Routing on for the
domain first, and verify any address you `Forward` to: a forward to an address
the account has not verified is refused (and falls back to
`Reject("forward refused")`).

The build's `package.json` lists `postal-mime`, which the Worker parses messages
with, at an exact version. `bynk dev`, `bynk deploy` and `bynk test` install it
for you, with the package manager you already use, into the build directory
(`.bynk/dev`, `.bynk/deploy`, `out`), and only when it is missing. For the
output directory of a plain `bynk compile`, run your package manager's `install`
there yourself. See [Installing npm packages](/reference/tooling/bynk-cli/#installing-npm-packages).

A reply goes out only for a sender whose mail passed DMARC, and only once per
message. The runtime does the sending, so the handler stays a pure decision.

## Try it locally

`bynk dev` runs the Worker under `wrangler dev`, which takes a raw message:

```sh
curl -X POST 'http://localhost:8787/cdn-cgi/local/email?from=a@example.org&to=support@shop.example' \
  --data-binary @message.eml
```

Write a small RFC 5322 file for `message.eml`. It must have a `Message-ID` header.

## Test it

```bynk,ignore
suite shop.support {
  case "a refund goes to billing" {
    let r <- support.message(InboundEmail {
      from: "a@b.example",
      to: "support@shop.example",
      subject: "refund",
    })
    expect r is Forward("billing@shop.example")
  }
}
```

Fields after `to` have defaults, so a case writes only what it cares about.
`bynk test` runs it with no Worker and no mail.

**See also:** [Reference — Inbound email](/reference/inbound-email/).
