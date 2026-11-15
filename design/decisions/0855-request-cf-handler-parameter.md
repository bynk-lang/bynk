# 0855 — `request.cf` is a fifth special handler parameter, `cf: RequestCf`

- **Status:** Accepted (v0.626)

**Context.** Cloudflare derives facts about a request's connection (country, colo,
ASN, TLS version) and exposes them as `request.cf`. Bynk had one such fact,
`RequestHeaders.clientIp()`, portable because the header exists elsewhere. The rest
are Cloudflare-only and absent in some environments (a test, the dashboard preview).
Proposal #3513 Part C, accepted. Cloudflare's documentation was checked
([Request](https://developers.cloudflare.com/workers/runtime-apis/request/),
`IncomingRequestCfProperties`) before building, and it differs from the proposal in
two places, recorded below.

**Decision.**

- `cf` is a special handler parameter beside `query`, `headers`, `cookies` and `body`,
  recognised by name and only when it is not a path parameter. It must be typed
  `RequestCf` from `bynk.cloudflare` (`bynk.http.cf_wrong_type`). Naming the type is the
  platform lock (ADR 0017), and per-file `consumes` applies (ADR 0849). A `:cf` path
  segment stays a path parameter, and an unbound `cf` was already
  `bynk.http.extra_param`, so no valid program changes meaning.
- **Optional means each field.** Every field is an `Option` defaulting to `None`; the
  parameter is not an `Option`. A missing `request.cf` gives all `None`, and a property
  of an unexpected type is `None`, never a fault.
- **The record literal is the builder** (`RequestCf { country: Some("GB") }`,
  `RequestCf {}`); no kernel op. At the `system` tier the request is real, so the
  argument a case passes is not sent.
- **`cf` is not client input.** `bynk client` and `bynk openapi` leave it out, so a
  handler's contract is the same with or without it.
- **Privacy is documented, not enforced:** the facts derive from the client's IP
  address and are best-effort.
- No `bynk dev` warning (nothing remote is called); the type is experimental.
- **Amendment 1 (latitude and longitude).** The proposal typed them `Option[Float]` and
  said a wrongly typed property becomes `None`. Cloudflare sends them as numeric
  strings (`"30.27130"`), so a strict reading would make them `None` on every request.
  They stay `Option[Float]` and the runtime reads the numeric string; a value that does
  not parse is `None`.
- **Amendment 2 (`isEuCountry`).** Cloudflare's `isEUCountry` is a string, `"1"` in the
  EU and otherwise omitted or `false`. `isEuCountry: Option[Bool]` is `Some(true)` for
  `"1"` or `true`, `Some(false)` for `false` or `"0"`, and `None` when absent. The
  spelling follows Bynk's camel case, not Cloudflare's.

**Consequences.** The initial field set is fifteen fields; more are added later as
defaulted fields, which does not break a handler. Plan-dependent properties (bot
management, client certificates) are left for their own optional records. Nothing
changes for a program that does not use `cf`: its emission is byte-identical, apart
from the standard library's `cloudflare.ts` now carrying the `RequestCf` type.
