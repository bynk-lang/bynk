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
may change in a minor version until it is stabilised. See [Routes left out](#routes-left-out)
for what it does not generate.
:::

This is the third surface a contract takes. [`bynk openapi`](/reference/tooling/bynk-openapi/)
is the language-neutral description, and the [TypeScript client](/reference/typescript-client/)
is the signed, Workers-only client for `on call` services. This one is for
browsers and framework servers calling `from http` routes.

| You want | Use |
|---|---|
| A typed TypeScript front end, with the server's validation and its `Option`, opaque and refinement behaviour on the client | `bynk client` |
| Documentation, a Swagger or Redoc UI, an API gateway, a client in another language, mocks or contract tests | `bynk openapi` |
| A Worker that is not Bynk calling an `on call` service | the [TypeScript client](/reference/typescript-client/) |

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
The imports end in `.js` (`"./types.js"`), which TypeScript resolves to the `.ts`
files under both `bundler` and `node16`/`nodenext` resolution, so the same output
works in Vite, SvelteKit and Next.js and in strict Node ESM.

### Keeping it current

`bynk client --check` writes nothing. It generates the client in memory, compares
it with what is at `--out`, and exits non-zero when a file differs or is missing,
naming the first differing line:

```text
bynk: error: client is stale: run `bynk client --out web/src/lib/api.ts`
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
no arguments in CI. See [`[build.client]`](/reference/tooling/manifest/#buildclient).

## Calling it

```ts
import { createClient } from "$lib/api";

const api = createClient({ baseUrl: "https://api.example.com" });

const res = await api.orders.createOrder({ sku: "A1", qty: 2, note: null, tags: [] });
switch (res.kind) {
  case "Created": show(res.body); break;            // res.body: Order
  case "Conflict": warn(res.error); break;          // `Conflict("…")`: res.error is the message
  case "StructuralMismatch": log(res.error); break; // the boundary's own refusal
  default: res satisfies CallFailure;               // Invalid, Transport or Unexpected
}
```

`createClient` takes a `ClientOptions`:

| Option | Meaning |
|---|---|
| `baseUrl` | The API's origin and any path prefix. Default: relative URLs. |
| `fetch` | The `fetch` to call. Default: the global one. Pass a framework's own (SvelteKit's `load` `fetch`, Next.js's server `fetch`) to get its cookie forwarding, relative URLs and caching. |
| `headers` | Headers for every call: a value, or a (possibly async) function read per call, so a bearer token needs no wrapper. The client never reads or stores a token. |
| `credentials` | Passed to `fetch`. `"include"` sends cookies to another origin; that needs a [`cors`](/reference/http/) policy with `credentials: true` and explicit origins. |
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

`RouteResponse` is a union with one member for each answer `bynk openapi`
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
| `Raw`, `RawTagged`, `RawCreated`, `RawStreamed`, `RawError` | `{ status, kind, contentType, body, bytes(), text() }`, with `etag` where the server sends one; see [Raw bodies](#raw-bodies-and-streams) |
| `Streaming(s)` | `{ status, kind, events: AsyncIterable<string> }`; see [Server-sent events](#server-sent-events) |

`kind` is the `HttpResult` variant's name. Two answers can share a status (the
boundary's `400` and a handler's own `BadRequest`); both are members, told apart by
`kind`. Every member and every `CallFailure` has a `kind` and only `Unexpected` of the
failures has a `status`, so narrow on `kind`: a `switch (res.kind)` that ends in a
`never` check is the intended idiom.

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

Boundary types are the same shapes as in the [TypeScript client](/reference/typescript-client/),
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

## Forms and uploads

A `@form` route's `body` is the record's public type, written as
`application/x-www-form-urlencoded` under the same leaf rules the server reads it
with: a `List` repeats its key, an `Option` is its value or absent, an enum is its
variant's name, and a field renamed with `@name` is written under its wire name. A
refinement is checked first, rooted at `body` as the server's error is.

A `@multipart` route's `body` is the record's public type with each file part a
`Blob`: `File` (the standard type) is `Blob`, `Option[File]` is `Blob | null`, and
`List[File]` is `readonly Blob[]`. A browser's `File` input value is a `Blob`, so it
passes as it is. The part is named by the field's wire name, and sent under the
`File`'s own name; a bare `Blob` has none, so it is sent as `blob`, as a browser
does, because a part with an empty file name is read back as text. The client does
not read a file's bytes: its text fields are checked by the record's decoder, with
an empty file standing in for each file part, and the size limits (`maxFile`,
`maxParts`, `maxBody`) are the server's to enforce, answered as the boundary's
`413`.

The content type of a multipart body carries its boundary, so `fetch` sets it; do
not set it in `headers`.

## Cookies, typed

A route that takes a typed `cookies` record gets a `cookies` argument of its own
type: a field that is an `Option`, or has a default, may be omitted, and a field
renamed with `@name` is written under that name (`__Host-sid`). It is written as the
request's `Cookie` header (`a=1; b=x`).

**It is for server-side calls**: a framework's server, a Worker, Node. A browser
sets its own `Cookie` header and drops one a script writes, so it would lose the
cookies without a word. Where `document` exists, a call with typed cookies given
returns `CallFailure` `Invalid` (path `cookies`) instead of sending a request that
would lose them; a page sends its cookies with `credentials: "include"` and calls
the route without the argument it cannot set. The check is `typeof document`, so a
test run under a DOM emulation such as jsdom sees a browser too.

## Headers, typed

A route that takes a typed `headers` record gets a `headers` argument of its own type: a field
that is an `Option`, or has a default, may be omitted, a `List` is sent as one comma-separated
value, and a field renamed with `@name` is written under that name (`if-match`). The fields are
written as request headers, and replace a header of the same name set in the client's `headers`
option. A value with a line break or another control character is refused as `Invalid` (path
`headers.<field>`) before any request is made.

**A browser sets some headers itself** (`Content-Length`, `Host`, `Cookie`, …) and drops one a
script writes, so a route that reads one of those is for a server-side call.

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

A route guarded by a [`SignedCookie`](/reference/actors/) actor reads a
cookie. In a browser, pass `credentials: "include"` (cross-origin, with a
credentialed `cors` policy).

A state-changing request to such a route is admitted only with
`Sec-Fetch-Site: same-origin`, or an `Origin` that exactly matches a credentialed
`cors` origin. A framework's *server* sends neither by default, so a `POST` made
from, say, a SvelteKit action is answered `403`. Set `origin` to one of the
service's credentialed CORS origins when calling from a server. A browser sets its
own `Origin` and ignores the option.

Whether a server runtime may set `Origin` was checked, rather than assumed: the
system tier runs the generated client inside a real `workerd`, calling a Bynk
Worker with `origin` set, and the route sees the header. Node's `fetch` sends it
too. A browser forbids a script setting it, and ignores the option.

### A CORS recipe for a browser front end

A front end on another origin needs the service's `cors` policy to name it. For a
cookie-authenticated API, the policy must be credentialed with explicit origins
(a wildcard is a compile error), and the client sends cookies with
`credentials: "include"`:

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
});
```

A Bearer-guarded API needs no `credentials`: pass the token with `headers`, which
the preflight already allows (`Authorization`). A route's `Allow-Methods` and the
`Vary: Origin` of a reflected origin are the server's, derived from its routes.

A `Set-Cookie` in a response reaches the browser only through the framework's
server, which must forward it; a script cannot read it. The `Cookie` header of an
incoming browser request must likewise be forwarded explicitly by the caller.

## Raw bodies and streams

A `@raw` route's `body` is the caller's bytes, which Bynk does not own the form of (so no
codec runs): `Uint8Array | ArrayBuffer | Blob | ReadableStream<Uint8Array>`. It is sent
under the content type the route declares, whatever `headers` holds; a route declared
`@raw(contentType: "*/*")` names none, so the caller's own is sent. A `ReadableStream` is a
streamed upload, which `fetch` takes only with `duplex: "half"` (the client sets it) and
which Chromium-based browsers, Node and Workers can make; other browsers cannot, so pass
a `Blob` or bytes there.

An answer whose body the author owns (`Raw`, `RawTagged`, `RawCreated`, `RawStreamed`,
`RawError`) is not read or decoded. Its member is

```ts
{ status, kind, contentType: string, etag?: string | null,
  body: ReadableStream<Uint8Array> | null, bytes(): Promise<Uint8Array>, text(): Promise<string> }
```

`body` is the response's own stream, so a large download is not buffered; `bytes()` and
`text()` read the same body, so use one of the three, once. `etag` is present where the
route can send one: a `string` when every raw answer at that status is a `RawTagged`, and
`string | null` where a `Raw` can answer (a small `GET` of a `Raw` carries a weak `ETag` the
server made). `RawError` answers the status its handler names; only a literal status is in
the route's set, as `bynk openapi` reports it.

The wire cannot tell one raw answer from another at the same status, so they are one
member, whose `kind` is the union of the variants that can make it. A status that also
answers in JSON (a `404` message, the boundary's `400`) is told apart by its content type:
`application/json` is the JSON answer, any other body, a missing content type included, is the raw one.
So a raw body that is itself `application/json`, at a status that also has a bodiless or message
answer, is read as that answer. At runtime a raw member's `kind` is the first variant listed for
its status (a `Raw` before a `RawTagged`), whichever the server made, since the wire cannot say;
its `etag` is the header, as sent. A raw answer that should carry an `ETag` and does not (a
`RawTagged`'s status, with none sent) is `Unexpected`, with the body's text.

### Server-sent events

`Streaming(s)` takes a `Stream[String]`, so an event is a `string` and nothing is decoded.
The server writes each element as one event: a `data:` line for each line of the element,
then a blank line, under `content-type: text/event-stream`, and nothing else (no `event:`,
`id:` or `retry:`, no comments). The client reads the body with `fetch`, not `EventSource`
(which cannot send headers or a body), and yields each event's `data`:

```ts
const res = await api.orders.getTicks();
if (res.kind === "Streaming") {
  for await (const tick of res.events) console.log(tick);
}
```

The reader follows the specification, so it also reads a stream another server wrote: CRLF,
LF or CR line ends, `data:` with or without the space, comments and other fields ignored.
An event the stream ends in the middle of is dropped. Breaking out of the loop, or aborting
the call with a `signal`, cancels the body. The body is read only when iteration starts: a member
that is never iterated leaves the response open until the `signal` aborts it, so check the
`status` first and abort what you will not read. A stream that fails after its status was sent
cannot become a `CallFailure`: the iteration throws a `StreamFailure` (exported, with `reason`
holding what the body rejected with), the one place the client throws. Two edges are the server's framing: an
element containing a bare `\r` is split by a reader (the server splits on `\n` only), so it
does not round-trip, and an empty element is an event with empty data.

## Calling through a Service Binding

A front end whose server runs on Cloudflare can reach a Bynk Worker without the public
network: no public hop, no CORS, no egress. A Service Binding's `fetch` takes what `fetch`
does, so the client's `fetch` option is the whole seam; `viaBinding` fills it in:

```ts
import { createClient, viaBinding } from "./api";

const api = createClient({ ...viaBinding(env.API) }).orders;
```

`viaBinding(binding, baseUrl?)` returns `{ baseUrl, fetch }`. It calls the binding's `fetch`
as a method, which a binding needs: `fetch: env.API.fetch` fails ("Illegal invocation"), as
a `Transport` failure. A binding ignores the host of the URL it is given but Cloudflare
requires a valid, fully qualified one, so `baseUrl` is the inert `https://bynk.invalid`
unless you pass another; a Worker sees it in `request.url`. A binding is for the server
only, since it exists only in a Worker: a browser keeps using `baseUrl`.

Declare the binding in the front end's `wrangler.toml`, naming the Bynk Worker:

```toml
[[services]]
binding = "API"
service = "orders-api"
```

Where the binding comes from depends on the framework (read from the adapters' own code, at
the versions named; the frameworks' documentation pages were not reachable when this was
written):

| Framework | Reaching `env.API` |
|---|---|
| SvelteKit, `@sveltejs/adapter-cloudflare` 8 (Kit 3) | `import { env } from "cloudflare:workers"`; not available in a prerenderable route |
| SvelteKit, `@sveltejs/adapter-cloudflare` 7 (Kit 2) | `platform.env.API` in `load`, actions and endpoints |
| Next.js, `@opennextjs/cloudflare` 1.x | `getCloudflareContext().env.API`; `await getCloudflareContext({ async: true })` in a static route; `initOpenNextCloudflareForDev()` in `next.config` for `next dev`; add `API` to the `CloudflareEnv` interface for its type |

Streaming works through a binding: the response arrives as headers first, with the body
still coming, and `events` and `body` read it as they do over the network.

**Authentication.** A binding call bypasses the browser, so it carries no `Origin` and no
`Sec-Fetch-Site`. A route that changes state under a signed cookie therefore answers `403`
unless the call names an origin, exactly as a server-side `fetch` does (see
[Cookies and CSRF](#cookies-and-csrf)): pass `origin` with one of the service's credentialed
CORS origins. Forward the browser's `Cookie` header with `headers`. The front end's own
server must authenticate its caller, because the Bynk Worker no longer sees the browser.

The client's types use the DOM `fetch` types (`Request`, `Response`, `Headers`), which a
SvelteKit or Next.js project has; a project that loads only `@cloudflare/workers-types`
without the DOM library needs the DOM library added for the generated file.

## Contracts and drift

The module carries the contract it was generated for:

```ts
export const WIRE_VERSION = "1";
export const API_CONTRACTS = {
  orders: { service: "9f2c…", routes: { getOrderById: "41b0…" } },
} as const;
```

A route's hash is FNV-1a over the facts a client acts on: its method and path, what it takes
(each path, `query`, `cookies`, `headers` and `body` parameter, with its type as it is on the wire, so
refinements, opaque types and `@name` keys count), and what it can answer (each status, variant
and kind of body). It moves when that does and **not** for a handler's body, a doc comment, the
order of a record's fields or a CORS change. A service's hash is the hash of its routes' hashes,
sorted. The hash is the compiler's skew detector, not a security control.

`WIRE_VERSION` is the wire format's version. The HTTP JSON encoding follows wire version 1, the
same version the cross-context format has, since both are read from the same wire model.

**Informational only.** Nothing rejects a mismatch, on the client or the server. A public API's
browsers and cached bundles lag the server for days, so a hard refusal would turn every additive
deploy into an outage for stale tabs. A stale client shows as `Unexpected` (a status or body the
route no longer matches); the hash is there to be compared, logged and checked in CI with
[`bynk client --check`](/reference/tooling/bynk-client/).

### Sending the hash

`createClient({ sendContract: true })` sends each call's route hash as `X-Bynk-Contract`, so a
server's log can tell which contract its callers were generated for. It is **off by default**: a
custom header makes a cross-origin browser call preflighted, and a service's
`Access-Control-Allow-Headers` is only `content-type` (and `authorization` on a Bearer route)
unless its `cors` block lists more, so the preflight would refuse the call. To send it from a
browser, list it, remembering that an explicit `headers:` list replaces the default:

```bynk,ignore
cors {
  origins: ["https://app.example.com"],
  headers: ["content-type", "authorization", "x-bynk-contract"],
}
```

A server-side call (a framework's server, Node) has no preflight and can set it freely, and
`viaBinding(...)` turns it on for a Service Binding call (override it with `sendContract: false`
after the spread). The server ignores the header on a `from http` route.

## Routes left out

A route the client cannot yet represent faithfully is named on stderr and left out,
never generated wrong: a raw `query: RequestQuery`, `cookies: RequestCookies` or `headers: RequestHeaders` view,
a route guarded by a request signature (the client holds no secret), and a route whose types hold an `Option`
inside a `Map`, a `Set` or a `Result`. If every selected route is left out the
command fails.

## Size

The generated source is a few kilobytes for a small API, and the types cost
nothing at run time. Bundled and minified, with the runtime tree-shaken to what a
page calls, a client with one route costs about 1.1 KB gzipped; five routes with
refinements, `Option`, a generic page and `Decimal` cost about 3.4 KB. The cost
grows with the codecs the routes reach, not with the project.
