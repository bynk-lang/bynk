---
title: Call your API from a front end
---
**Goal:** call a Bynk `from http` service from a SvelteKit or Next.js front end (or
anything that can `fetch`) with typed requests and responses, the server's own
validation, and a CI gate that fails when the client is out of date.

`bynk client` writes one TypeScript module for your routes. This page is the
workflow and the recipes; the [HTTP client reference](/book/reference/http-client/)
has every flag, type and rule.

:::note
`bynk client` is **experimental**: member names may change in a minor version.
The framework snippets below were written from the frameworks' adapters and are
marked where they depend on a framework version. Check them against the framework's
current documentation before relying on them.
:::

## Which tool

| You want | Use |
|---|---|
| A typed TypeScript front end, with the server's validation and its `Option`, opaque and refinement behaviour on the client | `bynk client` |
| Documentation, a Swagger or Redoc UI, a client in another language, mocks | [`bynk openapi`](/docs/tooling/bynk-openapi/) |
| A Worker that is not Bynk calling an `on call` service | the [TypeScript client](/book/reference/typescript-client/) |

## The API

The recipes use this service. It has a refined type, an optional field, a
route that can answer `409`, and a Bearer-guarded route:

```bynk,fragment
type Sku = String where NonEmpty
type Qty = Int where InRange(1, 99)

type NewOrder = { sku: Sku, qty: Qty, note: Option[String] }
type Order = { id: String, sku: Sku, qty: Qty, note: Option[String] }

service orders from http {
	on POST("/orders") (body: NewOrder) -> Effect[HttpResult[Order]] by v: Visitor {
		Created(Order { id: "o1", sku: body.sku, qty: body.qty, note: body.note })
	}

	on GET("/orders/:id") (id: String) -> Effect[HttpResult[Order]] by v: Visitor {
		NotFound
	}
}
```

## Generate it, commit it, check it

From the Bynk project, write the module into the front end:

```sh
bynk client --out ../web/src/lib/api.ts
```

Commit the file. A change to the API then shows as a diff in the front end's own
review. To stop the file drifting, name it once in `bynk.toml`:

```toml
[build.client]
out = "../web/src/lib/api.ts"
```

and gate it in CI, beside the front end's other checks:

```sh
bynk client --check    # exits non-zero, naming the first differing line, when it is stale
```

`--check` writes nothing, and an upgrade of `bynk` alone does not trip it.

### Regenerate from your editor

The editor needs no command of its own: `bynk client` reads `[build.client]`, so it takes
no arguments. In VS Code, a task puts it in the command palette (**Tasks: Run Task**),
and a second one runs the drift check:

```json
// .vscode/tasks.json
{
  "version": "2.0.0",
  "tasks": [
    { "label": "bynk: generate client", "type": "shell", "command": "bynk client", "problemMatcher": [] },
    { "label": "bynk: check client", "type": "shell", "command": "bynk client --check", "problemMatcher": [] }
  ]
}
```

Run them from the Bynk project's folder. `bynk build` also writes the client when
`[build.client]` is set, so a task that runs a build keeps it current.

## Call a route

```ts
import { createClient } from "$lib/api";

const api = createClient({ baseUrl: "https://api.example.com" }).orders;

const res = await api.postOrders({ sku: "A1", qty: 2, note: null });
```

The function names come from the method and path: `POST /orders` is `postOrders`,
`GET /orders/:id` is `getOrderById`. An optional value is `T | null`.

## Handle every outcome

A call never throws for an HTTP status or a network failure. It returns the
route's own answers, plus three failures, and every one carries a `kind`, so you
narrow on that:

```ts
const res = await api.postOrders({ sku: "A1", qty: 2, note: null });
switch (res.kind) {
  case "Created":
    show(res.body);                 // Order
    break;
  case "StructuralMismatch":
    log(res.error);                 // the boundary's own 400
    break;
  case "Invalid":
  case "Transport":
  case "Unexpected":
    report(res);                    // a CallFailure
    break;
  default:
    res satisfies never;
}
```

The route's members also carry their HTTP `status`, but a `CallFailure` other than
`Unexpected` has none, so `res.status` alone does not type-check on the union.
Ending the `switch` in a `satisfies never` check means that when the server starts
answering something new, the front end stops compiling until it handles it.

A `CallFailure` says what went wrong:

```ts
if (res.kind === "Invalid") {
  // A value broke a refinement (qty: 100) and nothing was sent.
  // res.error carries the message and path the server's 400 would have had.
} else if (res.kind === "Transport") {
  // The network failed, or the call was aborted. res.cause is what fetch threw.
} else if (res.kind === "Unexpected") {
  // A status or body the route no longer matches: the client is stale.
  console.warn(res.status, res.text);
}
```

Show an `Invalid` result next to the field before the user submits: it needs no
round trip.

## Send an auth header

For a Bearer-guarded route, `headers` is a value or a function read on every call,
so a token that refreshes needs no wrapper. The client never reads or stores one:

```ts
const api = createClient({
  baseUrl: "https://api.example.com",
  headers: async () => ({ Authorization: `Bearer ${await getToken()}` }),
}).orders;
```

A browser on another origin also needs the service's `cors` block to name the
front end. The preflight already allows `Authorization` on a Bearer route.

## Cookies and a signed-cookie session

A route guarded by a `SignedCookie` actor reads a cookie. From a browser on another
origin, send cookies with `credentials: "include"`, against a credentialed policy
with explicit origins (a wildcard is a compile error):

```bynk,ignore
service orders from http {
  cors { origins: ["https://app.example.com"], credentials: true }
  // …
}
```

```ts
const api = createClient({
  baseUrl: "https://api.example.com",
  credentials: "include",
}).orders;
```

A `POST` made by a framework's **server** is refused with `403` unless it names an
origin the policy admits, because a server sends neither `Origin` nor
`Sec-Fetch-Site`. Set `origin` on a server-side client, and forward the browser's
`Cookie` header:

```ts
const api = createClient({
  baseUrl: "https://api.example.com",
  origin: "https://app.example.com",
  headers: { Cookie: request.headers.get("cookie") ?? "" },
}).orders;
```

A `Set-Cookie` in a response reaches the browser only if your server relays it. See
[Cookies and CSRF](/book/reference/http-client/#cookies-and-csrf).

## A browser-only singleton

When every call is made in the browser, build the client once:

```ts
// src/lib/client.ts
import { createClient } from "./api";

export const api = createClient({ baseUrl: import.meta.env.VITE_API_URL }).orders;
```

## SvelteKit

The client's `fetch` option is the whole integration. In a `load` function, pass
SvelteKit's own `fetch`, which forwards cookies and resolves relative URLs
(**unverified** against the current SvelteKit documentation):

```ts
// src/routes/orders/[id]/+page.ts
import { createClient } from "$lib/api";
import { error } from "@sveltejs/kit";

export async function load({ fetch, params }) {
  const api = createClient({ fetch }).orders;
  const res = await api.getOrderById(params.id);
  if (res.kind !== "Ok") error(res.kind === "NotFound" ? 404 : 502);
  return { order: res.body };
}
```

A form action calls it the same way:

```ts
// src/routes/orders/new/+page.server.ts
import { createClient } from "$lib/api";
import { fail } from "@sveltejs/kit";

export const actions = {
  default: async ({ request, fetch }) => {
    const data = await request.formData();
    const api = createClient({ fetch }).orders;
    const res = await api.postOrders({
      sku: String(data.get("sku")),
      qty: Number(data.get("qty")),
      note: null,
    });
    if (res.kind !== "Created") return fail(400, { message: describe(res) });
    return { order: res.body };
  },
};
```

## Next.js

On the server, a component or route handler uses Next.js's extended `fetch`.
Extra options, such as `next: { revalidate: 60 }`, go in the per-call `init`, which
is passed through untouched (**unverified**: Next.js's caching defaults have changed
between major versions):

```ts
// app/orders/[id]/page.tsx
import { createClient } from "@/lib/api";

const api = createClient({ baseUrl: process.env.API_URL }).orders;

export default async function Page({ params }: { params: Promise<{ id: string }> }) {
  const { id } = await params;
  const res = await api.getOrderById(id, { next: { revalidate: 60 } });
  if (res.kind !== "Ok") return <p>Not available.</p>;
  return <p>{res.body.sku} × {res.body.qty}</p>;
}
```

A route handler that proxies a mutation:

```ts
// app/api/orders/route.ts
import { createClient } from "@/lib/api";

const api = createClient({ baseUrl: process.env.API_URL }).orders;

export async function POST(request: Request) {
  const res = await api.postOrders(await request.json());
  return Response.json(res, { status: "status" in res ? res.status : 502 });
}
```

## A front end on Cloudflare

A SvelteKit or Next.js front end deployed on Cloudflare can call the Bynk Worker
through a Service Binding: no public hop and no CORS. Declare the binding in the
front end's `wrangler.toml`:

```toml
[[services]]
binding = "API"
service = "orders-api"
```

and spread `viaBinding` into `createClient`:

```ts
import { createClient, viaBinding } from "$lib/api";

const api = createClient({ ...viaBinding(env.API) }).orders;
```

Where `env` comes from depends on the adapter and its version, and is listed in
the [reference](/book/reference/http-client/#calling-through-a-service-binding).
A binding call carries no `Origin`, so a state-changing route under a signed cookie
needs `origin` as above; your front end's server must authenticate its caller.

## Streams and uploads

A route that streams server-sent events returns an `events` iterator, and a
`@form` or `@multipart` route takes the typed record, with a browser's file input
value passed as a `Blob`:

```ts
const res = await api.getTicks();
if (res.kind === "Streaming") {
  for await (const tick of res.events) console.log(tick);
}
```

See [Raw bodies and streams](/book/reference/http-client/#raw-bodies-and-streams)
and [Forms and uploads](/book/reference/http-client/#forms-and-uploads).

## Test without a server

The client takes any `fetch`, so a unit test passes a fake and needs no network:

```ts
const api = createClient({
  baseUrl: "http://test",
  fetch: async () => Response.json({ id: "o1", sku: "A1", qty: 2, note: null }, { status: 201 }),
}).orders;

const res = await api.postOrders({ sku: "A1", qty: 2, note: null });
// res.kind === "Created"
```

**See also:** [Reference — HTTP client](/book/reference/http-client/),
[Handle an HTTP request](/book/guides/entry-points/http/),
[`bynk client`](/docs/tooling/bynk-client/).
