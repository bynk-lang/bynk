---
title: Build a small HTTP service
---
In this tutorial we start a **URL shortener** — the running example we will grow
across the rest of the tutorials. We begin with its HTTP front door: a service
with a couple of endpoints, compiled to a ready-to-run Cloudflare Worker. Along
the way you will meet `context`, `service`, HTTP handlers, and `HttpResult`.

This builds on [Tutorial 1](/book/tutorials/01-first-program/). You need `bynkc` installed.

## Start a project

A service is more than one file's worth of output, so instead of compiling a
single file we work with a **project directory**. Create one:

```sh
mkdir url-shortener
cd url-shortener
```

Inside it, create `shortener.bynk` with a single endpoint — looking up a short
code:

```bynk
context shortener

service api from http {
  on GET("/links/:code") (code: String) -> Effect[HttpResult[String]] by Visitor {
    NotFound
  }
}
```

A few new things:

- `context shortener` declares a
  **[context](/book/reference/glossary/#term-context)** rather than a `commons`. Contexts
  are the unit Bynk deploys — each becomes one Worker.
- `service api from http { … }` groups request handlers.
- `on GET("/links/:code") by Visitor (code: String)` is a handler: it answers
  `GET /links/<something>`, binds the `:code` path segment to the `code`
  parameter, and returns `Effect[HttpResult[String]]`.
- We have no storage yet — that arrives in [Tutorial 5](/book/tutorials/05-stateful-agent/) —
  so every lookup honestly returns `NotFound`, the `HttpResult` variant for
  `404`.

> The router derives the rest of the method contract from these routes for free:
> a wrong method to a live path is a `405` with an `Allow` header (not a `404`),
> a plain `OPTIONS` is answered, and `HEAD` mirrors `GET` — nothing to write. See
> [HTTP § Method semantics](/book/reference/http/#method-semantics).

> The file's name must match the context's name: `context shortener` lives in
> `shortener.bynk`. The compiler uses the source layout to determine each unit's
> identity.

## Compile to a Worker

Compile the project, targeting Cloudflare Workers:

```sh
bynkc compile . --output out --target workers
```

This writes a complete Worker under `out/`:

```text
out/
├── package.json    # marks the output as ES modules
├── __bynk_runtime/   # only the runtime modules the program uses
├── tsconfig.json
└── workers/
    └── shortener/
        ├── handlers.ts     # your handler logic
        ├── index.ts        # the Worker entry point + router
        ├── compose.ts      # dependency wiring
        └── wrangler.toml    # Cloudflare config
```

Open `out/workers/shortener/handlers.ts` and find your handler:

```typescript
export const api = {
  async http_GET_links_Param_code(code: string, deps: {}): globalThis.Promise<HttpResult<string>> {
    return HttpResult.NotFound;
  },
};
```

The routing lives in `index.ts`, which Cloudflare calls for every request. It
matches the path, pulls out the `:code` parameter, and calls your handler:

```typescript
const __m = __matchPath("/links/:code", path);
if (method === "GET" && __m) {
  const code = __m.params["code"];
  const result = await surface.http_GET_links_Param_code(code);
  return __httpResultToResponse(result, (v: any) => v as __JsonValue);
}
```

You wrote the *what* (answer `GET /links/:code` with `NotFound`); `bynkc`
generated the *how* (the router, the response encoding, the Worker scaffold).

## Accept a request body

Now the endpoint that *creates* a short link from a JSON body. First we need a
type for the request. Update `shortener.bynk`:

```bynk
context shortener

type CreateLinkRequest = {
  target: String,
}

service api from http {
  on GET("/links/:code") (code: String) -> Effect[HttpResult[String]] by Visitor {
    NotFound
  }

  on POST("/links") (body: CreateLinkRequest) -> Effect[HttpResult[String]] by Visitor {
    Created(body.target)
  }
}
```

`CreateLinkRequest` is a **record type** — you will learn records properly in
[Tutorial 3](/book/tutorials/03-modelling-data/). The new handler takes a special `body`
parameter typed as `CreateLinkRequest`, and returns `Created(…)` — the
`HttpResult` variant for `201 Created`. (For now it just echoes the target back;
real storage and a minted code come later.)

Recompile (`bynkc compile . --output out --target workers`) and look again at
`handlers.ts`. Your handler is there:

```typescript
async http_POST_links(body: CreateLinkRequest, deps: {}): globalThis.Promise<HttpResult<string>> {
  return HttpResult.Created(body.target);
},
```

…and `bynkc` has *also* generated a validator that parses and type-checks the
incoming JSON before your handler ever runs:

```typescript
export function __deserialise_CreateLinkRequest(json: __JsonValue, path: string = "$"): Result<CreateLinkRequest, __BoundaryError> {
  if (typeof json !== "object" || json === null || globalThis.Array.isArray(json)) {
    return Err({ kind: "StructuralMismatch", path, expected: "object", actual: typeof json });
  }
  const obj = json as { [k: string]: __JsonValue };
  if (typeof obj["target"] !== "string") {
    return Err({ kind: "StructuralMismatch", path: `${path}.target`, expected: "string", actual: typeof obj["target"] });
  }
  const __target = obj["target"];
  return Ok({ target: __target } as CreateLinkRequest);
}
```

The router calls it before your handler and rejects a malformed body with `400`
at the boundary, so inside the handler `body` is always a well-formed
`CreateLinkRequest`:

```typescript
const __r_body = handlers.__deserialise_CreateLinkRequest(__body_json, "$");
if (__r_body.tag === "Err") return new globalThis.Response(globalThis.JSON.stringify(__r_body.error), { status: 400, headers: { "content-type": "application/json" } });
const body = __r_body.value;
const result = await surface.http_POST_links(body);
```

## The `HttpResult` variants

`NotFound`, `Created`, and `Ok` are three of the `HttpResult` variants. The full
set covers the common, modern HTTP status codes (RFC 9110) — success (`Ok` 200,
`Created` 201, `Accepted` 202, `NoContent` 204), redirects carrying a `Location`
URL (`Found` 302, `SeeOther` 303, `PermanentRedirect` 308, …), and the
client/server failures (`BadRequest` 400, `NotFound` 404, `TooManyRequests` 429,
`ServerError` 500, …). See the [HTTP reference](/book/reference/http/) for the
complete list and the status code each maps to.

## Run it

The fastest way to serve the project locally is `bynk dev`, which compiles and
runs [Wrangler](https://developers.cloudflare.com/workers/wrangler/) for you in
one step:

```sh
bynk dev
```

It comes up on `http://localhost:8787`. Under the hood the emitted
`out/workers/shortener/` directory is a standard Cloudflare Worker, so you can
also run it the manual way — `cd out/workers/shortener && npx wrangler dev`. See
[Run your project locally](/book/guides/projects-build-and-deployment/run-locally/)
for more.

Then `POST /links` with `{"target":"https://example.com"}` returns a `201`, and
`GET /links/anything` returns `404` (until we add storage).

## What you have done

You built the shortener's HTTP front door, compiled it to a Cloudflare Worker,
and saw how `bynk` generates the router and boundary validation around the
handler logic you wrote. You returned several `HttpResult` variants and accepted
a typed request body.

Those record types we glossed over deserve a proper look — that is next, and we
will start modelling the shortener's data in earnest.

➡️ **[Tutorial 3: Model your data with types](/book/tutorials/03-modelling-data/)**

---

*Curious why a context maps to a Worker, or how the boundary validation fits the
design? See [How a Bynk program is shaped](/book/guides/program-structure/how-a-program-is-shaped/).*
