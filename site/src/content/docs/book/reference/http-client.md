---
title: HTTP client
---
A front end (SvelteKit, Next.js, anything that can `fetch`) calls a Bynk
`from http` service through a **typed TypeScript client** that `bynk client`
generates. It is one module, committed to the front end's repository: one
function per route, the boundary types as TypeScript types, and the checks the
server runs on a value applied again in the client, on the way out and on the
way in.

:::note
`bynk client` is **experimental**. The generated module's member names and shape
may change in a minor version until it is stabilised. Forms, multipart and raw
bodies, typed cookies and streamed responses are not generated yet; see
[Routes left out](#routes-left-out).
:::

This is the third surface a contract takes. [`bynk openapi`](/docs/tooling/bynk-openapi/)
is the language-neutral description, and the [TypeScript client](/book/reference/typescript-client/)
is the signed, Workers-only client for `on call` services. This one is for
browsers and framework servers calling `from http` routes.

| You want | Use |
|---|---|
| A typed TypeScript front end, with the server's validation and its `Option`, opaque and refinement behaviour on the client | `bynk client` |
| Documentation, a Swagger or Redoc UI, an API gateway, a client in another language, mocks or contract tests | `bynk openapi` |
| A Worker that is not Bynk calling an `on call` service | the [TypeScript client](/book/reference/typescript-client/) |

The client and the OpenAPI document read the same wire-contract model, so they
cannot disagree about a shape or a status.

## Generating it

```sh
bynk client --out web/src/lib/api.ts            # every `from http` service
bynk client --service orders --out api.ts       # one service; repeat to add more
bynk client > api.ts                            # stdout, to compose in a shell or CI
```

`INPUT` defaults to `.`, as for `bynk openapi`, and the project must check clean.
`--service` takes `Name` or `unit.Name` and may be given more than once. The
module is a single file with a `// @generated` header and no imports: a small
browser-safe runtime is inlined, so the front end installs nothing and a bundler
tree-shakes what a page does not call. Two services that answer the same method
and path are not an error here, since each has its own namespace.

The output is byte-identical between runs, so a diff of the committed file shows
an API change.

### Layouts

`--layout file` (the default) writes one module. `--layout dir` writes a directory
instead, `--out` naming it: `index.ts` (the client and the public surface),
`types.ts` (the boundary types and each route's response union), `codecs.ts` (the
encoders and decoders) and `runtime.ts` (the inlined runtime). Each file imports
exactly what it uses from the others, with type-only imports marked as such, so it
passes a build with `verbatimModuleSyntax`, `isolatedModules` and `noUnusedLocals`.
The imports have no extension (`"./types"`), which a bundler resolves (Vite,
SvelteKit, Next.js); a strict Node ESM loader needs a build step.

### Keeping it current

`bynk client --check` writes nothing. It generates the client in memory, compares
it with what is at `--out`, and exits non-zero when a file differs or is missing,
naming the first differing line:

```text
bynk: client is stale: run `bynk client --out web/src/lib/api.ts`
  web/src/lib/api.ts differs at line 142 (3 lines in all):
    - found:    readonly qty: number;
    + expected: readonly qty: Qty;
```

Run it in CI beside the front end's own checks. The header's compiler version and
the file's line endings do not count as drift, so an upgrade of `bynk` alone does
not break the gate.

### From the manifest

`[build.client]` in `bynk.toml` names where the client goes:

```toml
[build.client]
out = "../web/src/lib/api.ts"
layout = "file"          # or "dir"
service = ["orders"]     # optional; every service when left out
```

`bynk build` then writes the client beside its other outputs, leaving a file alone
whose text is already right. A project without the table builds exactly as before.
`bynk client` takes its `--out`, `--layout` and `--service` from the table when the
flags are left out, and a flag overrides the table, so `bynk client --check` needs
no arguments in CI. See [`[build.client]`](/docs/manifest/#buildclient).

## Calling it

```ts
import { createClient } from "$lib/api";

const api = createClient({ baseUrl: "https://api.example.com" });

const res = await api.orders.createOrder({ sku: "A1", qty: 2, note: null, tags: [] });
switch (res.status) {
  case 201: show(res.body); break;           // res.body: Order
  case 409: warn(res.error); break;          // `Conflict("…")`: res.error is the message
  case 400: log(res.error); break;           // the boundary's own refusal
  default: res satisfies CallFailure;        // Invalid, Transport or Unexpected
}
```

`createClient` takes a `ClientOptions`:

| Option | Meaning |
|---|---|
| `baseUrl` | The API's origin and any path prefix. Default: relative URLs. |
| `fetch` | The `fetch` to call. Default: the global one. Pass a framework's own (SvelteKit's `load` `fetch`, Next.js's server `fetch`) to get its cookie forwarding, relative URLs and caching. |
| `headers` | Headers for every call: a value, or a (possibly async) function read per call, so a bearer token needs no wrapper. The client never reads or stores a token. |
| `credentials` | Passed to `fetch`. `"include"` sends cookies to another origin; that needs a [`cors`](/book/reference/http/) policy with `credentials: true` and explicit origins. |
| `origin` | Sent as the `Origin` header. See [Cookies and CSRF](#cookies-and-csrf). |

Each function takes the route's parameters in order (path parameters, then the
`body`, then the `query`) and an optional last `init`, a standard `RequestInit`
merged over the client's defaults, so a `signal` or a framework's `cache` option
passes through untouched. Functions are named from the method and path:
`GET /orders/:id` is `getOrderById`, `POST /orders` is `postOrders`. Two routes of
a service that would share a name are refused, naming both.

## What a call returns

A call returns `Promise<RouteResponse | CallFailure>` and never throws for an HTTP
status or a network failure.

`RouteResponse` is a union keyed by `status`, one member for each answer `bynk openapi`
documents for the route: the declared success when the handler can return it, each
`HttpResult` variant it constructs, and the boundary's own `400` (and `413` and
`415` where they apply).

| The route answers | The member |
|---|---|
| `Ok(v)`, `Created(v)`, `Accepted(v)` | `{ status, kind, body: V }` |
| `NoContent`, `NotFound`, `Unauthorized`, … | `{ status, kind }` |
| `Conflict("…")`, `BadRequest("…")` and the other message variants | `{ status, kind, error: string }` |
| a redirect | `{ status, kind, location: string }` |
| the boundary's `400` | `{ status: 400, kind: "StructuralMismatch", error: BoundaryError }` |

`kind` is the `HttpResult` variant's name. Two answers can share a status (the
boundary's `400` and a handler's own `BadRequest`); both are members, told apart by
`kind`. A `switch (res.status)` that ends in a `never` check is the intended idiom.

A `fetch` follows redirects by default, so a redirect member appears only when the
caller passes `redirect: "manual"` in `init`, and a browser hides the response of a
manual redirect.

`CallFailure` has three kinds:

| `kind` | When |
|---|---|
| `Invalid` | A value failed a refinement or did not fit its type, and **nothing was sent**. `error` is a `BoundaryError`, the one the server's `400` would carry. |
| `Transport` | `fetch` rejected: a network failure or an abort. `cause` is what it threw. |
| `Unexpected` | The status is not one the route answers, or the body no longer decodes. The mark of a stale client. `status` and `text` are the response's; `error` says why a body did not decode. |

## Types

Boundary types are the same shapes as in the [TypeScript client](/book/reference/typescript-client/),
with one difference: **an optional value is `T | null`**, not Bynk's tagged `Option`.

| Bynk | TypeScript |
|---|---|
| `Int`, `Float`, `Instant`, `Duration` | `number` |
| `String` | `string` |
| `Bool` | `boolean` |
| `Bytes` | `Uint8Array` |
| `Decimal` | `{ readonly coefficient: bigint; readonly scale: number }` |
| `List[T]` | `readonly T[]` |
| `Option[T]` | `T \| null` |
| a record | an interface of `readonly` fields |
| a sum | a union discriminated by `tag` |
| a generic record `Page[T]` | a generic interface `Page<T>` |
| a refined type | its base type; the client applies the predicate |
| an opaque type | its base type, branded: you pass back a value you were given |

The client converts at the boundary with the codecs the server uses. A request
writes `Some(x)` and `None` in Bynk's tagged form, which the server's decoder
reads; a response is decoded whether the route writes `Option` tagged or as plain
JSON (`json { options: plain }`), so one route mode or the other gives the same
`T | null`. Because `null` cannot say "present but empty", an `Option[Option[T]]`
has no such form and a route that uses one is left out.

A `query` argument is its own type: a field that is an `Option`, or has a default,
may be omitted, and omitted means absent from the query string. A `List` field is
written by repeating the key. A field renamed with `@name` is written under its
wire name. A `List` with no default is required by the server, and an empty list
writes no key, so the server refuses it; give the field a default if it may be
empty.

## Refinements

The client runs the server's own predicates, from the same generator:

- **On the way out**, the encoded value is run through the decoder the server
  runs, and a violation is returned as `Invalid` before any request is made. The
  message and path are the server's (`$.qty`, `path.id`, `query.limit`).
- **On the way in**, the response is decoded by the same codecs, so a server that
  changed a refinement, or a proxy that altered a body, is caught as `Unexpected`
  and not carried into the UI as a wrong value.

An opaque type is never checked: the client cannot make one, because the owner's
`.of` is not there.

## Cookies and CSRF

A route guarded by a [`SignedCookie`](/book/reference/actors/) actor reads a
cookie. In a browser, pass `credentials: "include"` (cross-origin, with a
credentialed `cors` policy).

A state-changing request to such a route is admitted only with
`Sec-Fetch-Site: same-origin`, or an `Origin` that exactly matches a credentialed
`cors` origin. A framework's *server* sends neither by default, so a `POST` made
from, say, a SvelteKit action is answered `403`. Set `origin` to one of the
service's credentialed CORS origins when calling from a server. A browser sets its
own `Origin` and ignores the option.

A `Set-Cookie` in a response reaches the browser only through the framework's
server, which must forward it; a script cannot read it. The `Cookie` header of an
incoming browser request must likewise be forwarded explicitly by the caller.

## Routes left out

A route the client cannot yet represent faithfully is named on stderr and left out,
never generated wrong: a `@form`, `@multipart` or `@raw` body, a typed `cookies`
record, a route that can answer a raw or streamed body, a route guarded by a request
signature (the client holds no secret), and a route whose types hold an `Option`
inside a `Map`, a `Set` or a `Result`. If every selected route is left out the
command fails.

## Size

The generated source is a few kilobytes for a small API, and the types cost
nothing at run time. Bundled and minified, with the runtime tree-shaken to what a
page calls, a client with one route costs about 1.1 KB gzipped; five routes with
refinements, `Option`, a generic page and `Decimal` cost about 3.4 KB. The cost
grows with the codecs the routes reach, not with the project.
