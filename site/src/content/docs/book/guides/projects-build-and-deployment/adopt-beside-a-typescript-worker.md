---
title: Adopt Bynk beside a TypeScript Worker
---
**Goal:** introduce Bynk next to a Cloudflare Worker you already run in
TypeScript, without rewriting it: let the TypeScript Worker call a new Bynk
context through a typed client, and let the Bynk context call the TypeScript
Worker back.

Both directions go over Service Bindings, never the public internet. The
TypeScript Worker stays yours: `bynk deploy` never writes to a Worker it doesn't
own.

## The TypeScript Worker calls Bynk

Say the existing Worker, `billing-svc`, should record ledger entries in a new
Bynk context, `shop.ledger`.

### 1. Declare it as a foreign caller

In `shop.ledger`, declare the TypeScript Worker as an actor with the `Foreign`
scheme, and admit it on each service it may call:

```bynk,ignore
context shop.ledger

actor Billing { auth = Foreign(secret = "BILLING_CALL_KEY", worker = "billing-svc") }

service lookup {
  on call(id: EntryId) -> Effect[Result[Entry, LedgerError]] by Billing { … }
}
```

`Billing` is now a caller the handler can name, distinct from every Bynk
context (`by who: Caller | Billing` admits both and tells them apart). A
service that doesn't admit `Billing` refuses it with a `403`.

### 2. Build, and take the client

Build the project as usual. Because `shop.ledger` declares a foreign caller,
the build also writes a typed client for it,
`workers/shop-ledger/bynk-client.ts`. Copy it into the TypeScript Worker's
repository, and add the runtime package it imports, `@bynk/wire-client`, to that
Worker's dependencies (see
[the runtime package](/book/reference/typescript-client/#the-runtime-package)).

The client is generated code: when the context's services change, regenerate it
rather than editing it.

### 3. Deploy, and hand over the secret

`bynk deploy` mints `BILLING_CALL_KEY`, sets it on `shop-ledger`, and prints the
command that sets the same value on the TypeScript Worker:

```text
printf '%s' '<value>' | npx wrangler secret put BYNK_CALL_TO_SHOP__LEDGER --name billing-svc
```

The command names your project's own runner: `pnpm dlx wrangler …` under pnpm, `bunx wrangler …` under bun, `npx wrangler …` otherwise.

Run it (or pass it to whoever operates `billing-svc`). The value is shown only
this once; `bynk deploy --force` mints a new one.

### 4. Bind and call

Give the TypeScript Worker a Service Binding to the context's Worker, in its
own `wrangler.toml`:

```toml
[[services]]
binding = "LEDGER"
service = "shop-ledger"
```

Then call through the client:

```ts
import { connect, type Env } from "./bynk-client.js";

interface WorkerEnv extends Env {
  readonly LEDGER: Fetcher;
}

export default {
  async fetch(request: Request, env: WorkerEnv): Promise<Response> {
    const ledger = connect({ caller: "Billing", binding: env.LEDGER, env });
    const page = await ledger.recent();
    return Response.json(page.items.map((e) => e.id));
  },
};
```

Every argument and result is typed, and a service `Billing` may not call isn't
on the client at all. The [reference](/book/reference/typescript-client/) lists
what the client exports and how a failed call surfaces.

### 5. Pin the client

Record which contract the TypeScript Worker's client was generated at, so a
later change to `shop.ledger` that would break it is refused at deploy rather
than discovered by a `409` in production. Create `bynk.foreign.toml` beside
`bynk.toml`, with the lines from the client's `CONTRACTS` doc (the first deploy
prints them too):

```toml
["shop.ledger".Billing]
lookup = "586b853a77dcb029"
post = "27debbca3698ad27"
recent = "b8c0680d223be288"
```

Commit it. From then on, when a change to `shop.ledger` alters one of those
contracts, `bynk deploy` stops with `bynk.deploy.foreign_contract_skew`. The
way through is deliberate: regenerate the client, update the TypeScript Worker,
then update the pin to the new hashes.

## Bynk calls the TypeScript Worker

The other direction needs no client. Declare the TypeScript Worker as a
`Service` capability and send it requests, as
[Call a Worker outside the project](/book/guides/effects-and-capabilities/call-a-foreign-worker/)
describes:

```bynk,ignore
capability Billing = Service @worker("billing-svc")
```

`bynk deploy` adds the binding and warns if the Worker doesn't exist yet.

## Run both locally

`bynk dev` runs the Bynk contexts and gives each foreign caller's secret the
fixed local key `bynk-local-call-key`. Run the TypeScript Worker beside it with
the same value, and wrangler's dev registry joins the two bindings:

```bash
npx wrangler dev --var BYNK_CALL_TO_SHOP__LEDGER:bynk-local-call-key
```

With pnpm use `pnpm dlx wrangler …`; with bun, `bunx wrangler …`.

## Moving more over

Each Worker can move on its own schedule. While the TypeScript Worker calls a
service, keep the service admitting its actor and keep its pin current. When a
caller becomes a Bynk context itself, it calls through `consumes` instead, and
its contract is checked by the compiler; drop its actor and its pin.
