---
title: Send transactional email
---
**Goal:** send a receipt from a handler through Cloudflare Email Service, answer
the outcomes a handler can act on, and test it without sending anything.

`Email` is **experimental**: it carries no compatibility promise yet. Opt in in
`bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.Email"]
```

## Declare a mailer

A mailer is a named instance of `Email`. `@allowedSenders` lists the addresses it
may send as; `@to` and `@allowedRecipients` restrict who it may send to. All are
optional deploy-time policy:

```bynk,fragment
context shop.orders

consumes bynk
consumes bynk.cloudflare { Email, EmailError, EmailMessage, RateLimit }

capability Mail = Email @allowedSenders(["orders@shop.example"])

capability ReceiptLimit = RateLimit @limit(5) @period(60.seconds)
```

A bare `given Email` is refused. `Mail` binds as `EMAIL_MAIL`, a `[[send_email]]`
entry in the generated `wrangler.toml`.

## Send, and answer the refusals

`send` returns the message id, or an `EmailError` a handler can act on:

```bynk,fragment
service receipts from http {
	on POST("/receipt") (to: String) -> Effect[HttpResult[String]] by Visitor given Mail, ReceiptLimit {
		let allowed <- ReceiptLimit.allow(to)
		if !allowed {
			Ok("slow down")
		} else {
			let sent <- Mail.send(EmailMessage {
				from: "orders@shop.example",
				to: [to],
				subject: "Your receipt",
				text: Some("Thanks for your order"),
			})
			match sent {
				Ok(id) => Ok(id)
				Err(RateLimited) => Ok("try again later")
				Err(DailyLimit) => Ok("try again tomorrow")
				Err(_) => Ok("not sent")
			}
		}
	}
}
```

The `RateLimit` is deliberate: a handler anyone can call that sends mail is a
spam relay. Breaking a platform limit (more than 50 recipients, 32 attachments or
25 MiB, no `text` or `html`) or setting a reserved header (`From`, `To`, `Cc`,
`Bcc`, `Subject`, `Reply-To`, or any name or value with a line break) is a
programming error, so the call faults before anything is sent. See
[Outbound email](/book/reference/bynk-capabilities/#outbound-email-email).

## Deploy

`bynk deploy` checks each `@allowedSenders` domain has Email Sending enabled on
the account, and fails naming the domain when it does not. It never configures
one: onboard the domain under Email Service > Email Sending in the dashboard
first. See [Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#sender-domains-email-sending).

## Test it

A test needs no stub. The double records every message, so you can assert on it,
and a stub forces an error:

```bynk,fragment
suite shop.orders

case "a receipt is sent" {
	let r <- api.POST("/receipt", "buyer@example.com") by Visitor
	expect r is Ok("test-message-1")
	expect Mail.send called once
	expect Mail.send called with message.subject == "Your receipt"
}

case "a refused send is answered" {
	stub Mail.send(_) returns Err(RateLimited)
	let r <- api.POST("/receipt", "buyer@example.com") by Visitor
	expect r is Ok("try again later")
}
```
