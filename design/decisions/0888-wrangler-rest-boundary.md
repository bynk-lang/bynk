# 0888 — The wrangler/REST boundary and the Cloudflare client layer

- **Status:** Accepted (v0.640.69)

**Context.** Before back-half paydown 1D.3 (#4043), `bynk` reached Cloudflare in two
ways that shared nothing. It spawned wrangler from 24 places, each of which built its own
`Command`, read its own output and matched wrangler's prose to decide what had happened. It
also called the REST API through a client in `migrate::live` that was constructed separately
in ten places and reported every failure as a `String`, which callers then searched for
`"answered 403"` or `"\"code\":10007"`. Pagination was written ten times, and nothing was
retried. The track's §9 left two questions open: which operations belong to wrangler and
which to REST (Q4), and how the client layer should be shaped (Q7).

**Decision.**

- **The boundary (Q4).** Wrangler stays the tool for what only it does well: `deploy` (the
  push and its bundling), `dev`, `tail`, D1 migrations (remote and local) and `rollback`.
  REST is the tool for creating, reading and deleting account resources: KV, D1, R2,
  queues, Vectorize, secrets, triggers and deployment listings. Slice 1D.4 moves those
  resources from wrangler to REST, kind by kind, behind `CloudflareApi`, without touching a
  kind. Until then, every resource operation is still a wrangler call, and the matching on
  wrangler's text is confined to `bynk/src/cloudflare/error.rs` (`WranglerError`'s
  `already_exists`, `already_gone`, `worker_not_found` and the rest). Wrangler gives
  nothing better than its text, so that matching shrinks only as 1D.4 empties it.
- **Credentials.** Wrangler uses `wrangler login` (or the token in its environment). REST
  needs both `CLOUDFLARE_API_TOKEN` **and** `CLOUDFLARE_ACCOUNT_ID`; a missing one is
  reported by the first purpose that needs it, in that purpose's words
  (`MissingCredentials::message`). From 1D.4, `bynk doctor` checks both variables.
- **The layer (Q7).** There is one module, `bynk/src/cloudflare/`:
  - `CloudflareApi` is the one operation trait. Its 25 methods are one per account
    operation, and `Wrangler` is its process adapter (`fake::Recording` in tests). The
    policy (adopting what already exists, treating what is already gone as success)
    stays in the kinds, over a typed error.
  - `Rest` is the one REST client. It is built once per command run: `rest::for_purpose`
    shares one set of credentials and one TLS agent, read on first use. The twelve
    narrow traits (`HyperdriveApi`, `Listings`, `SecretsStores`, `AiSearchInstances`,
    `MtlsCertificates`, `ScriptVersions`, `DestinationAddresses`, `SendingDomains`,
    `revert::Account`, and `sweep::{Cloudflare, Secrets, Wake}`) stay as their callers'
    test seams over it.
- **Errors.** `CloudflareError` is a `Wrangler(WranglerError)`, a `Rest(RestError)`, a
  `Credentials` message, or `Other` text for an answer that succeeded but could not be used.
  Its `Display` is the text `bynk` printed before 1D.3, byte for byte, and that text is the
  user's contract. It is pinned by `cloudflare_messages.txt`, `cloudflare_rest_messages.txt`
  and `cloudflare_credentials.txt`, and a diff in any of them is a stop, never a bless.
  `RestError` answers `status()`, `has_code()` (an exact code, where the old substring
  search also matched `100070` for `10007`), `is_denied()` and `message_mentions()`, so no
  caller searches a REST error's text. Codes and messages are read at any depth of `errors`, an
  entry's `error_chain` included, as the old search of the printed text found them. Typed errors stop at the client boundary: above it,
  `DeployFailure` and each caller's own `format!` are unchanged.
- **Retries (Decision H).** `retry::Policy` retries only an idempotent call (a `GET`, `PUT`
  or `DELETE`, or a `POST` the caller marks idempotent, such as D1's `SELECT`; never
  `POST /hyperdrive/configs`), and only on a failure a retry could help: a request that did
  not complete, a 429, or a 5xx. It honours `Retry-After` up to a cap. **It ships at one
  attempt** (`Policy::RULED`) for as long as the freeze holds, because a retry changes how
  long a failure takes and what it prints. Turning it on means changing `RULED` to
  `Policy::BACKOFF` (three attempts, 0.5 s then 1 s, `Retry-After` capped at 10 s), in a
  ruled PR of its own. Wrangler's calls are never retried: its failures are text, so a
  transient one cannot be told from a real one.
- **Pagination (Decision K).** `paging` follows every listing. `Numbered` covers numbered
  pages to `total_pages`, capped, or while full; `cursor` covers cursors, with or without a
  stop on a cursor that repeats. Each site reports what it counted on a page and keeps its
  own stop rule exactly, so no request sequence changed. Two inconsistencies are kept and
  not fixed, because fixing either changes the requests made: a listing that omits
  `total_pages` is read as one page, and the Durable Object objects listing does not stop on
  a cursor that repeats.

**Consequences.** The gated growth probe `cloudflare_client_sites` reads 0. It counts a
wrangler spawn built at a call site, or a REST client constructed outside the module, so a
second channel to Cloudflare fails the gate. Slice 1D.4 can swap a kind's implementation
from wrangler to REST behind `CloudflareApi` without touching the kind's policy. The
direction in §12 of the track is what a later ADR would decide and supersede this one with:
a binding that describes typed REST calls, bundling owned by `bynk`, and uploading scripts
over REST.
