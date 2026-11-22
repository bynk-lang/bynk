---
title: "Generate a typed client for your front end"
---
`bynk client` writes a typed TypeScript client for a project's `from http`
services: one module a SvelteKit, Next.js or plain browser front end imports. It
is the compiler handing the front end the contract it already enforces, so a
request the server would refuse with `400` is refused in the client, before it is
sent, with the server's own message.

It is **experimental**: the module's member names may change in a minor version
until it is stabilised. The full surface is in the
[HTTP client reference](/reference/http-client/).

## A recipe

From the project root:

```sh
bynk client --out web/src/lib/api.ts        # commit the file
bynk client --service orders --out api.ts   # one service of several
```

The module has no imports and needs nothing installed. In the front end:

```ts
import { createClient } from "$lib/api";

export const api = createClient({ baseUrl: "https://api.example.com" });

const res = await api.orders.getOrderById(id);
if (res.kind === "Ok") render(res.body);   // res.body: Order, an optional field is `T | null`
else if (res.kind === "NotFound") notFound();
else report(res);                           // 400, or a CallFailure
```

A call returns a union of the route's own responses keyed by `status`, or a
`CallFailure`; it does not throw for an HTTP status or a network failure. Pass the
`fetch` your framework gives you (`createClient({ fetch })`), `headers` for a
bearer token, and `credentials` or `origin` for a cookie-authenticated API.

## In CI, and from `bynk build`

Commit the generated file, and let CI say when it is stale:

```sh
bynk client --out web/src/lib/api.ts --check
```

Or name it once in `bynk.toml`, so `bynk build` keeps it written and
`bynk client --check` needs no arguments:

```toml
[build.client]
out = "../web/src/lib/api.ts"
```

`--layout dir` writes `index.ts`, `types.ts`, `codecs.ts` and `runtime.ts` instead
of one module. See the [HTTP client reference](/reference/http-client/#layouts).

## When to use which

| You want | Use |
|---|---|
| A typed TypeScript front end that applies the server's validation | `bynk client` |
| Documentation, a Swagger UI, a client in another language, a gateway | [`bynk openapi`](/reference/tooling/bynk-openapi/) |
| A Worker that is not Bynk calling an `on call` service | the [TypeScript client](/reference/typescript-client/) |

`@form` and `@multipart` bodies are generated (a file part is a `Blob`), a typed
`cookies` record is written as a `Cookie` header for server-side calls, a `@raw` body
is the caller's bytes, and raw and streamed answers (server-sent events included) are
the caller's to read. A route the client cannot represent (a raw `RequestQuery` or
`RequestCookies` view, a signed route) is named on stderr and left out.
