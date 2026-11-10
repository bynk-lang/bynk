---
title: Call a Worker outside the project
---
**Goal:** call a Cloudflare Worker that is not written in Bynk, such as an
existing TypeScript billing service, over a Service Binding rather than the
public internet.

`Service` is **experimental**: it carries no compatibility promise yet. Opt in
in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.Service"]
```

If the Worker you call is itself a Bynk context, `consumes` it instead: that
checks its contract, which `Service` cannot. A `@worker` naming one of your
project's own contexts is refused.

## Declare the Worker

Consume `Service` from `bynk.cloudflare`, and `bynk` for its `Request` and
`Response` types. Then declare one capability per Worker, naming it with
`@worker`:

```bynk,fragment
context shop.checkout

consumes bynk
consumes bynk.cloudflare { Service }

---
The billing system that predates this project.
---
capability Billing = Service @worker("legacy-billing")

service charge {
	on call(order: String) -> Effect[Int] given Billing {
		let req = Request {
			method: Post,
			url: "https://legacy-billing/charges",
			contentType: Some("application/json"),
			authorization: None,
			headers: Map.empty(),
			body: Some("{\"order\":\"\(order)\"}"),
		}
		let res <- Billing.send(req)
		match res {
			Ok(r) => r.status,
			Err(_) => 0,
		}
	}
}
```

`send` takes and returns what `Fetch.send` does, under the same header rules.
The binding decides where the request goes, so the URL's host is not used for
routing, but it must be absolute and the Worker sees it. A status the Worker
returns, 5xx included, is an `Ok` response; `Err(Network)` means the call
itself failed.

The call is untyped. For a typed client, write a provider over `Billing` that
builds the requests and decodes the responses, so the rest of the context sees
your own capability.

A bare `given Service` is refused (`bynk.capability.instance_only`): it would
name no Worker.

## What the build generates

The instance binds as `SERVICE_BILLING`, a `[[services]]` entry naming the
Worker as declared:

```toml
[[services]]
binding = "SERVICE_BILLING"
service = "legacy-billing"
```

The binding name joins the Worker's others under the duplicate check
(`bynk.context.duplicate_binding`).

## Test it

Under `bynk test` each instance has its own double, and it faults until its
`send` is stubbed, because a test never reaches the Worker:

```bynk,fragment
case "a declined charge is reported" {
  stub Billing.send(_) returns Ok(Response { status: 402, headers: Map.empty(), body: "" })
  let r <- charge.call("o1")
  expect r == 402
  expect Billing.send called once
}
```

In a `system` suite the Worker is not a participant, so its binding answers
every request with a 503 saying it is a stand-in.

## Run it locally

`bynk dev` runs a local stand-in for the Worker inside the context's own
`wrangler dev`, answering every request with a 503 that says what it is. To
call the real Worker, run it with `wrangler dev` in its own project and name
it:

```bash
bynk dev --local-worker legacy-billing
```

Wrangler's dev registry then connects the binding to that session. Until it is
up, the binding answers 503 (`Worker "legacy-billing" not found`).

## Deploy

`bynk deploy` binds the Worker but never creates or changes it: it is not
Bynk's. The plan shows the binding:

```text
service SERVICE_BILLING legacy-billing (foreign Worker; not provisioned)
deploy shop-checkout
```

After authenticating, `deploy` asks the account whether `legacy-billing`
exists and warns if it does not. Cloudflare rejects an upload whose Service
Binding names a missing Worker, so deploy that Worker first. Under `--env`, the
Worker's name is used as declared, not suffixed with the environment.
