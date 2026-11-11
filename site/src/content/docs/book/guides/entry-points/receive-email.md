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
  on message(mail: InboundEmail) -> Effect[EmailVerdict] {
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

## Do not trust `from`

`mail.from` is the envelope sender, and anyone can write any address there. The
handler's default actor is `Sender`, which carries the address and whether
Cloudflare found the sender's domain authenticated with DMARC
(`DmarcPass`, `DmarcFail` or `DmarcNone`). Forward or reply on a claim; act
on the evidence.

## Deploy it

`bynk build --target workers` gives the Worker an `email()` export and lists the
addresses in its `wrangler.toml`; `bynk deploy` pushes it, and Cloudflare Email
Routing delivers mail for those addresses to it. Turn Email Routing on for the
domain first, and verify any address you `Forward` to: a forward to an address
the account has not verified is refused (and falls back to
`Reject("forward refused")`).

The build's `package.json` lists `postal-mime`, which the Worker parses messages
with, at an exact version. Run `npm install` in the build directory (`.bynk/dev`
for `bynk dev`, `.bynk/deploy` for `bynk deploy`, the output directory of a
`bynk compile`) before running it: `bynk dev` and `bynk deploy` stop before
wrangler, naming any package that is not installed.

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

**See also:** [Reference — Inbound email](/book/reference/inbound-email/).
