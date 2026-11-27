---
title: Upgrading
---
Bynk is pre-1.0, and increments can break existing programs. This page is the
record of every breaking change, newest first: what breaks, the diagnostic you
will see, and the fix (`bynk check --fix` where it is machine-applicable).

To move a project from one release to another, read the entries between the two
versions. Each [GitHub release](https://github.com/bynk-lang/bynk/releases)
repeats the entries since the previous release under **Upgrading from vX**, so
the release you are moving to tells you what to change.

The [releases](/reference/releases/) are the history of *every* increment;
this page is only the ones that break something. An increment that is missing
here broke nothing, or its author did not say so: please
[open an issue](https://github.com/bynk-lang/bynk/issues/new?template=bug_report.yml).

<!-- stamp:breaking-entries -->

## v0.834.0

A `Queue @name("…")`, `from queue("…")` or `deadLetter: "…"` whose name starts with `bynk-ev-` or `bynk-dl-` now fails with `bynk.queue.reserved_name`; rename the queue (any other name, including other `bynk-` names, is unaffected)

## v0.833.0

A `uses` of a unit that declares a `messages` bundle no longer brings that unit's `render` in (`bynk.messages.cross_unit_render`), and a literal `message("code")` in a unit that declares a bundle must name a code of its `@reference` block (`bynk.messages.unknown_code`). Declare the code in the unit's reference bundle, or export a function from the unit that owns the bundle that renders it and call that, as `greeter.web` calls `greet` from `greeter.messages`.

## v0.826.0

An `out` frame that holds an `Option` now carries `{ "kind": "Some", "value": … }` or `{ "kind": "None" }`, where it carried the runtime's `{ "tag": "Some", "value": … }` or `{ "tag": "None" }`. A client that read the old shape must read the new one. A `Decimal` or `Instant` field, which the old encoding could not write (a `Decimal` threw) or wrote in its runtime form, is now its codec text or epoch milliseconds. A frame of strings, numbers, booleans, lists and plain records is unchanged. Connections open across the deploy keep the old `Option` shape until they reconnect: a socket accepted before this change records no service name and is sent as before.

## v0.825.0

Every deploy of a project that consumes, sends to or dead-letters into a queue now needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` (the token with "Queues: Edit"), even when nothing is created, because each deploy checks its queues; `wrangler login` alone is no longer enough. So does a `--prune` that deletes an orphaned queue. Without them the deploy stops before its confirmation with `set CLOUDFLARE_… to check, create or delete the account's queues: …`, having changed nothing. Set both variables. A failed queue call now reads as a REST failure, such as `POST /queues: Cloudflare answered 403 […]`, instead of wrangler's text.

## v0.820.2

A project with a `Postgres` instance has `postgres` 4.1.1 in its `bynk.lock` and `package.json` now (was 4.0.0). A build run with `--locked` is refused with `bynk.package.lock_out_of_date` ("`bynk.lock` is out of date with the manifests, and `--locked` was given") until you run `bynk fetch` and commit the lock. Nothing else changes: parameter binding, result types and the statement's wire sequence read the same as on 4.0.0.

## v0.820.0

Two HTTP routes of one context whose emitted handler names collide (`GET /a-b` beside `GET /a_b`, both `http_GET_a_b`, in one service or two), where one handler silently answered both paths, are now refused as `bynk.http.handler_name_collision`; spell one route differently

## v0.818.0

Every deploy of a project that declares a `Vectorize` now needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` (the token with "Vectorize: Edit"), even when nothing is created, because each deploy checks its indexes; `wrangler login` alone is no longer enough. Without them the deploy stops before its confirmation with `set CLOUDFLARE_… to check, create or delete the account's Vectorize indexes: …`, having changed nothing. Set both variables. A failed Vectorize call now reads as a REST failure, such as `GET /vectorize/v2/indexes?page=1: Cloudflare answered 403 […]`, still followed by the permission hint when the token lacks it.

## v0.817.0

A non-GET route whose body calls `Actor.issue(...)` now answers 403 to a request that is cross-site, or that sends neither `Sec-Fetch-Site` nor an `Origin` the service lists, where it used to run the body. A browser sign-in from another origin needs the service to declare `cors { origins: [...], credentials: true }` with that origin; a service with no `cors` accepts an issuing request from its own origin only. A non-browser client (a native app or a server) posting to a sign-in route must send an `Origin` listed in `cors` or `Sec-Fetch-Site: same-origin`, or the route should use a Bearer actor instead.

## v0.812.1

`bynk check <dir>` and `--emit` on a directory that has no `bynk.toml` or `src/` of its own but sits in a project's `[paths] include` trees now run on the enclosing project, where they used to treat the directory as a standalone source tree. `--fix` still edits only the directory named. A run that passed because the manifest was not read (its `[unstable] allow`, `[paths]` and `[env]` ignored) can now fail, or report other findings, as `bynk check` from the project root always did. A directory outside the project's sources, excluded from them, or with its own `src/` is still checked as itself. To check a tree standalone, run `bynk check` from a directory outside any project, or pass a single `.bynk` file; `--each` keeps its refusal inside a project.

## v0.812.0

Creating an R2 bucket or a D1 database now needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` (the token with "Workers R2 Storage: Edit" or "D1: Edit"), where `wrangler login` used to be enough. A deploy that needs them and lacks one stops before its confirmation with `set CLOUDFLARE_… to create the account's R2 buckets: …` (or `… D1 databases: …`), having created nothing. Set both variables. A redeploy whose buckets and databases are all recorded still works with `wrangler login` alone, and D1 migrations still run through wrangler. A failed create now reads as a REST failure after the unchanged `could not create …:` prefix, such as `POST /r2/buckets: Cloudflare answered 409 […]`.

## v0.811.0

`Response`, `BytesResponse` and `StreamResponse` gain a `setCookies` field (default `[]`), so a program that `Json.encode`s one now writes `"setCookies":[]` too, and a hand-written TypeScript binding that returns one must supply `setCookies`. A Bynk `Response { … }` literal, including every `stub Fetch.send(_) returns Ok(Response { … })`, still compiles unchanged.

## v0.810.2

building Bynk from source now needs Rust 1.98 or newer (the minimum was 1.95), because `oxc` 0.159 and later set that floor; with `rustup`, run `rustup update stable` or let the repository's `rust-toolchain.toml` install it. A released `bynk` binary, and the output it writes, are unaffected.

## v0.809.0

a `match` that covers an `HttpResult` (at top level, or inside a `Result` or `Option`) by listing every variant, with no wildcard or binding arm, is now the error `bynk.types.extensible_match_needs_wildcard`. Add a last arm `_ => …` (`Ok(_) => …` or `Some(_) => …` where it is wrapped) for the statuses a later release may add; the editor's fill-missing-arms quick-fix inserts it. A `match` in a suite case and `expect r is …` are unaffected.

## v0.807.0

Creating or pruning a KV namespace now needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` (the token with "Workers KV Storage: Edit"), where `wrangler login` used to be enough. A deploy that needs them and lacks one stops before its confirmation with `set CLOUDFLARE_… to create or delete the account's KV namespaces: …`, having created nothing. Set both variables (`bynk doctor` shows which is missing). A redeploy whose namespaces are all recorded still works with `wrangler login` alone, with a plan note that the account was not checked for namespaces deleted by hand. A failed KV operation now reads as a REST failure after the unchanged `could not create …:` prefix, such as `POST /storage/kv/namespaces: Cloudflare answered 400 […]`, instead of wrangler's banner.

## v0.805.0

A `Bearer` actor that already wrote `issuer` or `audience`, which were silently ignored, now enforces them, so tokens that lack or differ in that claim start getting `401`: mint them with the claim (`bynk token <Actor>` does) or remove the key. Any other `Bearer` config key is now `bynk.actor.bearer_unknown_key`. Two `Bearer` actors on one secret are now `bynk.actor.bearer_secret_shared`: declare a distinct `audience` on each, mint each actor's tokens with its own (`bynk token <Actor>`), or give each its own secret.

## v0.802.0

A WebSocket frame over 1 MiB, which used to be parsed, is now closed with `1009` unless the service sets a larger `limits { maxFrame }`. A service whose `in:` is `Bytes` now reads and sends binary messages, where it exchanged base64 JSON text; and a binary message to any other service is closed with `1003`, where it used to be read as UTF-8 and parsed. A frame the server sends over `maxFrame` now faults the send.

## v0.800.0

a decode failure on a `@name`d field of a `cookies`, `headers`, `query` or `@form`/`@multipart` `body` record now reports the wire name in its error path (`query.page-size`), not the Bynk name (`query.pageSize`), as every other codec path does; and an enum read from a path, query, header or cookie value is read by its variant's wire name. A client that matches the path text should read the wire name. The codes `bynk.types.field_name_not_request_record` and `bynk.types.field_name_record_not_boundary` are retired, because the restriction they enforced is lifted.

## v0.795.0

a `cors` origin that a browser can never send is now `bynk.http.cors_invalid_origins` at the entry, with the form to write: a trailing slash, a path, a query or fragment, an upper-case scheme or host, no scheme, a default port written out (`:443` for `https`, `:80` for `http`), or a wildcard subdomain (`https://*.example.com`). Such an entry compiled before and matched no request, so every cross-origin call from that site was refused. Write the origin as `scheme://host[:port]` in lower case, for example `"https://app.example.com"`. The origin `"null"` beside `credentials: true` is now `bynk.http.cors_wildcard_credentials`; drop `"null"` or set `credentials: false`.

## v0.791.1

a literal `Route` tag that breaks the header rules (not `X-`, characters other than letters, digits and `-`, a line break in a value) written in a helper function is now the error `bynk.email.bad_tag`, as it already was in a handler body; before, the runtime dropped the entry. Rename the tag or fix the value.

## v0.787.2

A WebSocket client that closes with a code now sees that code and reason echoed back in the server's close frame where it used to see `1005` (no status) when `on close` removed the held connection; a client that relied on `1005` there sees its own code.

## v0.786.2

A WebSocket connection opened with a Bearer token is now closed with `1008` at the first frame after that token's `exp`, where it used to keep acting indefinitely; the client reconnects with a fresh token. A connection accepted before this release has no `exp` recorded and is not closed until it reconnects.

## v0.786.0

a `Producer` refinement that names a context declaring no `Queue @name(...)` for the queue is now the error `bynk.queue.producer_never_sends`; declare the `Queue` in that context or remove the name from the refinement.

## v0.784.0

`Request.authorization` is now `Option[Authorization]`, not `Option[String]`. Build one from a `Secret`: `authorization: Some(token.bearer())`, or `Some(password.basic("alice"))` for HTTP Basic; a string such as `Some("Bearer …")` no longer type-checks, and a credential that must be text goes in `headers` with a visible `reveal()`. A request is no longer `Codable` or comparable with `==`, because it holds a secret. A hand-written `Fetch` binding reads `req.authorization.value.reveal()`.

## v0.781.0

A `from websocket` service without an `on close` handler is now the error `bynk.ws.missing_close`; add an `on close` that releases the connection `on open` handed to an agent (with the same leading route parameters and `by` as `on open`), which a departed client's connection needed anyway, since a later broadcast to it raised.

## v0.777.0

`DecodeError.kind` is a `DecodeErrorKind`, not a `String`, so `e.kind == "Malformed"` and `e.kind.concat(…)` are type errors (`bynk.types.type_mismatch`; `bynk check --fix` rewrites a comparison with a kind's name to `e.kind == Malformed`, and a string that names no kind gets no fix), and `"\(e.kind)"` still prints the name. A single-file program can no longer read `e.kind` at all: `DecodeErrorKind` is declared in the first-party `bynk.decode` commons, which a single-file program cannot reach (as it cannot reach `ValidationRule` to read `e.rule`), so there `e.kind` is `bynk.types.unknown_field` where it was a `String`; move the code into a project to keep reading it (#4475 tracks lifting this for every prelude type). The `400` bodies are unchanged.

## v0.776.0

`Secrets.get(name)` is now `Effect[Option[Secret]]` and `StoredSecret.get()` is `Effect[Secret]`, not `String`. Read the text where it must leave with `secret.reveal()`, compare with `secret.matches(candidate)` (`==` and `!=` on a `Secret` are `bynk.types.secret_not_comparable`), and stub one with `Some(Secret.of("…"))` (`consumes bynk { Secret }`). A `Secret` cannot be interpolated, JSON-encoded, used as a `Key`, or put in a record, event or `store` field, or a handler's parameter or result (`bynk.types.secret_at_boundary`). A hand-written binding that reads `Secrets.get` gets a `Secret`, not a string: call `.reveal()`.

## v0.771.0

A plain-JSON route and `Json.encodePlain` now write a payload-less sum (`enum`) as its variant's name (`"Opened"`, not `{"kind":"Opened"}`), a `String`-keyed `Map` as a JSON object and an `Instant` as an RFC 3339 string; a non-Bynk client of such a route must read those shapes. Bynk's decoder reads both forms, and tagged mode and cross-context calls are unchanged.

## v0.770.1

A WebSocket client that offers the Bearer token alone, `new WebSocket(url, [token])`, is now refused with `400` and a fixed message; offer the pair `["bynk.bearer", token]`. Only a client that does not enforce the subprotocol standard (Node's `ws`, a test harness) could use the old form.

## v0.768.0

`AnalyticsSql.query(sql, params)` is now `AnalyticsSql.query(dataset, sql, params)` and refuses SQL built at run time (`bynk.types.sql_not_literal`): pass the `Analytics` instance first and write `{dataset}` where the table name was interpolated; `stub AnalyticsSql.query(_, _)` becomes `_, _, _`.

## v0.762.0

A generated client's response union for a route with `@rateLimit` gains a `RateLimited` member (`{ status: 429, kind: "RateLimited", ... }`), so a `switch (res.kind)` that ends in an exhaustive `never` check on such a route no longer type-checks until it handles it; regenerate the client with `bynk client` and add a `case "RateLimited":` (for example, wait `Retry-After` seconds and retry). The route's contract hash moves with it, for `@rateLimit` routes only.

## v0.761.0

`bynk deploy --env <name>` no longer writes the source's `from email` addresses for a named environment (Wrangler applies `addresses` at the top level only, so they were production's, and a deploy either took production's rules over or failed after the Worker was live); the environment now claims none, and the plan says `email: none (no mapping for ...)`. To give it an address, map one: `[deploy.email."support@shop.example"]` with `address = { staging = "support@staging.shop.example" }`

## v0.760.0

every generated `wrangler.toml` gains the `nodejs_als` compatibility flag, which gives a Worker the `AsyncLocalStorage` the trace is held in; it is left out where the project's `[workers] compatibility_flags` already lists `nodejs_compat`, which provides it. A project that commits its generated `wrangler.toml` or reads its flags sees a diff on the next build. A `Logger.log` record gains `fields.trace` inside a request, replacing a field of that name, and the text of a fault or call log line ends ` trace=<id>`; a log query that matches those lines exactly needs to allow the suffix.

## v0.758.0

Mail to `user+tag@domain` now reaches the service that claims `user@domain` instead of the domain's catch-all; a catch-all that relied on receiving tagged mail should claim the tagged address explicitly (`from email("user+tag@domain")`) or read `mail.subaddress` in the service that now receives it

## v0.752.0

On an actor sum, a bare `by Forged`, `by Expired` or `by Stale` that more than one member could refuse is now `bynk.test.bad_principal_needs_arm`; name the member, as `by Forged(Browser)` (it used to act on the sum's first Bearer or Signature member).

## v0.743.0

`BoundaryRejection` gains `RateLimited(detail)`: a `match` over it with no wildcard stops compiling (`bynk.types.non_exhaustive_match` names the missing variant); add an arm for `RateLimited(_)`, or a wildcard

## v0.724.1

a program that gives an `Effect` to a generic type's parameter where the type's declaration puts that parameter directly under an `Effect` (`type Wrap[T] = { v: Effect[T] }` used as `Wrap[Effect[Int]]`, written or returned by a generic call) now gets `bynk.types.nested_effect`; such a program type-checked before but had a field `Effect[Effect[Int]]`, which cannot exist at run time.

## v0.724.0

An email `on message` handler with no `by` clause, and no `by` on its service header, is now a compile error (`bynk.actor.missing_by_on_email`). It meant "any mail, whatever its DMARC verdict" before; the fix inserts `by Sender`, which means the same, so behaviour is unchanged once migrated

## v0.723.0

A route whose typed headers record has a required field reading a header the route's actor consumes (authorization on a Bearer, Oidc or sum route, a signature header, cookie on a SignedCookie route) no longer compiles; the field was never present, except `cookie` on a SignedCookie route, which was present when the request carried a second cookie and is refused on purpose, so read cookies through a `cookies` record and the identity through the actor.

## v0.718.0

A bounded generic type applied to a type parameter that lacks the bound (`fn mk[U](xs: List[U]) -> Sorted[U]` over `type Sorted[T: Ord]`), or built by a literal whose inferred argument fails the bound (`Sorted { items: [true] }`), is now an error: `bynk.generics.missing_ord_bound` (and `missing_eq_bound`, `missing_key_bound`, `missing_codable_bound`) or `bynk.types.not_orderable` (and `not_comparable`, `not_keyable`, `not_codable`); `bynk check --fix` declares the missing bounds down a chain of generic types and functions

## v0.709.0

An `on message` handler with no `by` clause, and no `by` on its service header, is now a compile error (`bynk.actor.missing_by_on_queue`). It meant "any producer on the Cloudflare account" before; the fix inserts `by Producer`, which means the same, so behaviour is unchanged once migrated

## v0.706.0

`Instant.toString()` and `Duration.toString()` no longer give the bare millisecond count. They give RFC 3339 UTC text (`2026-11-22T10:00:00.000Z`) and unit text (`1h30m`). Code that read the old text back as a number should call `toEpochMillis()` or `toMillis()`. No diagnostic marks it, because the new text names its units and cannot be mistaken for the old number.

## v0.701.0

a program that writes `Effect[Effect[T]]`, or that instantiates a generic function so it wraps one effect in another (`wrap(e)` with `e: Effect[_]`, or `Effect.pure(e)`), now gets `bynk.types.nested_effect`; such a program type-checked before but failed at run time when a method was called on the flattened value. Sequence the effects with `andThen` or `<-` instead.

## v0.694.0

A project that sends a type its own consumer cannot accept now fails to compile (`bynk.queue.message_type_mismatch`); it already failed at run time as a message the consumer could not decode

## v0.693.0

a unit that declares its own two-parameter `type MapEntry[A, B]` now gets `bynk.resolve.duplicate_type`, because `MapEntry[K, V]` is the prelude record; rename the type.

## v0.692.0

A response with `Content-Type: text/html` now carries `Content-Security-Policy: frame-ancestors 'none'` and `Referrer-Policy: strict-origin-when-cross-origin`, so a page meant to be embedded in another site's frame stops rendering there; name the embedder with `security { frameAncestors: ["https://embedder.example"] }`.

## v0.688.0

`QueueSend.delay` is an `Option[QueueDelay]` instead of an `Option[Duration]`, so a computed `Duration` is refused with `bynk.types.some_value_mismatch` until it goes through `QueueDelay.of(d)`

## v0.687.0

A route guarded by a `Bearer`, `Oidc`, `SignedCookie` or `Signature` actor now sends `Cache-Control: private` on every response that has no `@cache` of its own, and `@cache(scope: public)` on such a route is refused with `bynk.http.cache_public_authenticated`; move a response that is the same for everyone to a `Visitor` route, or use `scope: private`.

## v0.684.0

A body-taking HTTP route with no `@limit` or `limits { maxBody }` now answers `413` for a body over 1 MiB; the build hints `bynk.http.body_cap_defaulted` name those routes, and `@limit(maxBody: …)` on a route or `limits { maxBody: … }` on the service raises the cap.

## v0.680.0

`parTraverse` on a `store Map` or a held `Map[K, Connection]` (the query form that runs a step per element concurrently and returns `()`) is now `parForEach`; the old spelling is the error `bynk.query.par_traverse_renamed`, and `bynk check --fix` renames the call.

## v0.679.0

`QueueResult` has a third variant, `RetryAfter`, so a `match` over a `QueueResult` (in a test case or elsewhere) must now name it or have a wildcard

## v0.678.0

a project whose `derives Ord` type is reordered, past what was recorded, fails the build with `bynk.types.ord_order_changed` until `bynk lock accept-order` accepts it; a project that does not reorder is unaffected

## v0.677.2

A `400 RefinementViolation` body no longer carries `violation.value`; a client that read it must stop

## v0.675.0

A queue message that does not decode is no longer retried (and eventually deleted); it is dead-lettered or dropped visibly, a `system` case that expected `Retry` for one now sees `Undecodable`, and a `match` of a queue verdict in a case must now name `Undecodable` or have a wildcard

## v0.672.0

An HTTP route whose agent call hits `AgentBusy` now answers `503` with `Retry-After: 1` instead of `500`, and is no longer reported to the platform as a fault, so a client or alert that matched the `500` must match the `503`; a cross-context call that does so is `503` too, unmarked, so its caller reads it as unreachable rather than `CalleeFaulted`.

## v0.667.0

`Kv.putTtl` takes a `KvTtl` (a `Duration` of at least 60 seconds) instead of `Int` seconds, `KvExpiration.After` takes a `KvTtl` and `KvGet.cacheTtl` an `Option[KvCacheTtl]`, so `Kv.putTtl(k, v, 86400)` is a type error: write `1.day`, or `KvTtl.of(d)` for a computed span; a TTL under 60 seconds (30 for `cacheTtl`) is now a compile error or a `ValidationError` instead of a run-time fault, and a part second rounds towards zero rather than up

## v0.664.0

`Bytes.compare` returns an `Ordering` (`Less`, `Equal` or `Greater`) instead of `-1`, `0` or `1`; a program that compares the result with an `Int` must match on the `Ordering`

## v0.660.0

a `Matches` or regex-method pattern that is valid without the `u` flag but not with it (an identity escape such as `\-` outside a class, a `[\w-.]` range) is `bynk.types.invalid_regex`, with a note naming Unicode mode; drop the needless backslash

## v0.659.0

Routes that overlap are now tried by position, not by spelling: at the first segment where two routes of a method differ, a literal beats a `:name` (so `GET /a/b/:y` now beats `GET /a/:x/c` for `/a/b/c`, where the alphabetical tie-break picked `/a/:x/c`), and a route that every other route of its method answers first is refused with `bynk.http.unreachable_route`: remove it or make it match a path the others do not. Literal routes also now ignore empty segments (`/items/` reaches `GET /items`) and compare percent-decoded.

## v0.658.0

A cron `Err` is no longer logged as `cron <expr> failed <payload>`; the log is now a structured record with the error's type and variant only, so an alert keyed on the old line stops matching, and the payload no longer reaches the log

## v0.656.0

A context that writes a bare `consumes bynk` (no braces) and declares its own `CallOutcome`, `CallFailure` or `Refusal` now fails as `bynk.consumes.name_conflict`; rename the type, or consume only what you use with `consumes bynk { … }`.

## v0.655.1

`BoundaryRejection` gains `MalformedPath(detail)`: a `match` over it with no wildcard stops compiling (`bynk.types.non_exhaustive_match` names the missing variant); add an arm for `MalformedPath(_)`, or a `_` arm. A request whose path cannot be decoded now answers `400`, where it answered `500` and was reported as a fault.

## v0.654.0

A JSON `body` route now answers `415 UnsupportedMediaType` to a request whose `Content-Type` is present and is neither `application/json` nor a `+json` type, where it tried to parse the body (a form post got a `400 MalformedJson`). A client that posts JSON labelled as a form, such as `curl -d '{…}'` without a header, must send `Content-Type: application/json`; a request with no `Content-Type` is still accepted.

## v0.652.0

A cron schedule with irregular whitespace (a double space, a tab, a leading or trailing space) now fails as `bynk.cron.invalid_schedule`, as does any schedule Cloudflare would reject (`61 * * * *`); write the fields separated by single spaces (the note gives the corrected form). A schedule that smuggled a `"` or `\` into a field, which the old field-count check let reach `wrangler.toml`, is refused too, closing that TOML-injection path.

## v0.651.0

`1.hours` (and every `1.<plural unit>`) no longer parses as a `Duration` literal: it is `bynk.duration.unit_agreement`, with a machine-applicable fix-it (`bynk check --fix`) writing `1.hour`. `5.hour` is the same error, and a word after an integer literal that names no unit (`5.hr`) is `bynk.duration.unknown_unit`, where it used to be a field access on an `Int`.

## v0.648.1

A handler on a `by` route that reads `headers.bearer()` or `headers.get("authorization")` (or the `Signature` header) now gets `None`, because the seam consumed it; take the identity from the `by` binding instead.

## v0.647.0

`String` `<`, `sortBy`, `min`, `max` and `Ord` now order by code point, not UTF-16 code unit, so text mixing a character above U+FFFF with one in U+E000..U+FFFF sorts differently; `MinLength`, `MaxLength` and `Length` now count code points at run time too (an emoji counts once). No diagnostic; no fix-it is needed for BMP-only text.

## v0.646.0

"`List.parTraverse` now returns `Effect[List[U]]`, the steps' values in input order; the old behaviour (steps returning `Effect[()]`, result `Effect[()]`) is `parForEach`. A call whose step returns `()` still type-checks but is the warning `bynk.list.par_traverse_unit`, and where its result is used as `Effect[()]` it is also a type error; `bynk check --fix` renames the call to `parForEach` in both cases. `parTraverse` on a `store` query or a held `Map` is unchanged."

## v0.645.0

`T.of(x)` on a named type with no predicate (`type Id = Int`) now returns `T`, not `Result[T, ValidationError]`, so `Id.of(n)?` or a `match` on it stops compiling (`bynk.types.question_on_non_result`, with a machine-applicable "drop the `?`" fix-it); the emitted TypeScript `Id.of(n)` returns the branded value, not a result object. Adding a predicate later turns `.of` back into a `Result`. A `Float` named type stays fallible.

## v0.644.0

A cron handler declared `(at: Int)` is now `bynk.cron.bad_params`, and a test's `svc.schedule("…", 3600000)` address needs an `Instant`; write `(at: Instant)`, `Instant.fromEpochMillis(3600000)`, and `at.toEpochMillis()` where the body needs the number.

## v0.642.0

An `on call` handler with no `by` clause, on the handler or the service header, is now `bynk.actor.missing_by_on_call`; it used to default silently to `Caller`. Run `bynk check --fix=bynk.actor.missing_by_on_call` to insert `by Caller`, which is what the handler did before, then narrow it if you want fewer callers. Agents, cron, queue and `Events` handlers are unchanged.

## v0.641.0

The compiled JavaScript moves from `<output>/../out-js` to `<output>/out-js`, and the emitted `tsconfig.json` now has `"outDir": "./out-js"` and excludes `out-js`; anything that read `../out-js` (a script running the `tsc`-built JavaScript, a Wrangler or CI step pointing at it) must read `<output>/out-js`, and a `tsconfig.json` copied from an earlier output needs the same two changes. `out-js` is now compiler-owned: a later `bynk build` or `bynk test` into the same `--output` removes the `.js`, `.map` and `.json` files it did not write, so copy `out-js` elsewhere (or run it) before the next build.

## v0.640.0

A route containing `*`, `{` or `}`, such as `on GET("/files/*key")`, used to match those characters literally and is now refused with `bynk.http.reserved_route_syntax`. To match one literally, take the segment as a `:name` parameter and check its value in the handler.

## v0.639.57

A service that declares two `on call` handlers no longer compiles (`bynk.service.duplicate_call_handler`). Only the last one ever ran, so delete the other, or move it into a service of its own.

## v0.639.35

An `expect` predicate that holds an effectful call, such as `expect [Logger.log("x")].length() == 1`, is now `bynk.expect.impure_predicate`, and an effectful call in a `with` predicate is now `bynk.observe.impure_with`. Run the call first with `let x <- …` and state the claim about `x`.

## v0.633.0

a `[test.targets.*]` `vars` entry named `BYNK_DATASET_…` is now a manifest error ("`vars.BYNK_DATASET_…` is reserved"), because `bynk` writes that name to carry an `Analytics` dataset's qualified name; rename the variable

## v0.619.0

`consumes` is per file, for every unit and for suites: a file that names a type (or a variant), a capability or a service that its own `consumes` does not bring in now fails `bynk check` with `bynk.consumes.type_not_selected`, `bynk.consumes.capability_not_selected` or `bynk.consumes.unit_not_consumed` (these were warnings since v0.614.0). Run `bynk check --fix=bynk.consumes.type_not_selected,bynk.consumes.capability_not_selected,bynk.consumes.unit_not_consumed` to add the clauses to each file, on this version or on v0.614.x, where the fixes are attached.

## v0.611.0

A bundle with a composition root (some context consumes another context's service or capability, emits events, or derives a capability) whose context requires a capability it declares but does not provide now fails `bynk check` with `bynk.capability.not_provided`; its `compose.ts` never type-checked. Add `provides Cap = …` to that context; a test `stub` still replaces it in the cases that stub it

## v0.606.1

A project with a `Postgres` instance has `postgres` 4.0.0 in its `bynk.lock` and `package.json` now (was 3.4.5). A build run with `--locked` is refused with `bynk.package.lock_out_of_date` ("`bynk.lock` is out of date with the manifests, and `--locked` was given") until you run `bynk fetch` and commit the lock. Bynk keeps postgres.js 3's parameter typing (`infer: true`), so `PgTimestamp`, `PgBoolean` and `PgBytea` bind as documented. One difference remains: a statement with several commands and no parameters (`create table a(…); create table b(…)`) now reports an empty row for each command and a `rowCount` of the commands, where it reported none and 0; a single statement is unchanged.

## v0.605.0

`~>` is removed; run `bynk check --fix` on v0.603.x first, or rewrite `~> e` as `e.detach()` (written `do e.detach()` as a statement). A program that still contains `~>` fails with `bynk.parse.removed_send_arrow`.

## v0.603.10

The release added the standard-library names `Claim`, `Claimed` and `AlreadyClaimed` (`bynk`) and `ContainerError`'s variants `StartFailed`, `Unavailable`, `Timeout` and `Network` (`bynk.cloudflare`). They were in scope for any `consumes bynk` or `consumes bynk.cloudflare`, even a selective one, so a suite `type Claim` or `type Claimed` was refused with `bynk.suite.type_name_clash`, `firstAs[Claim]` in a suite read the standard-library enum (`bynk.types.sql_row_shape`), and a bare `Unavailable(…)` of your own sum was `bynk.resolve.ambiguous_variant`. Rename the declaration, or qualify the variant (`ClaudeOutcome.Unavailable(…)`).

## v0.603.8

A project with an inbound email service has `postal-mime` 4.2.0 in its `bynk.lock` and `package.json` now (was 4.1.4). A build run with `--locked` is refused with `bynk.package.lock_out_of_date` ("`bynk.lock` is out of date with the manifests, and `--locked` was given") until you run `bynk fetch` and commit the lock.

## v0.603.6

`bynk doctor`, `bynk dev` and `bynk deploy` now want wrangler 4.162.0 or newer: an older one is warned about, and `bynk deploy` refuses to push a context with an agent on it (`bynk: wrangler X is older than 4.162.0 …`); upgrade with `npm install -g wrangler@4`.

## v0.602.1

`IndexInfo.metric` (the result of the experimental `Vectorize.describe`) is now `Option[String]`, because a real index reports no metric; code that reads `info.metric` as a `String` fails to type-check, so match it as `Some(metric)` or `None`

## v0.594.1

A `Vectorize` instance declaring `@dimensions` below 32 is now `bynk.capability.invalid_option` at compile time; Cloudflare refuses to create such an index, so a deploy could never have succeeded. Declare at least 32 dimensions.

## v0.587.0

`Idempotency.dedup[T]` and `remember[T]` now require `T: Codable` (bynk.types.not_codable otherwise), so `dedup[()]`/`remember[(), ()]`, the presence-marker idiom, is refused: use `Idempotency.claim(key, …)` for a key-only check, or `dedup[Bool]`/`remember[Bool]` for a cached marker. A Worker that calls `dedup`/`remember` now gets an implicit D1 database (`<worker>-idempotency`) created by `bynk deploy`, and starts with an empty cache.

## v0.584.0

The `Idempotency` test double now honours `expiresAfter` on the harness clock, so a case that `advance`s `Clock` past a `remember` window sees `dedup` return `None`, and a zero or negative `expiresAfter` lapses at once; before, the entry never lapsed.

## v0.583.0

A user-declared capability operation whose return type mentions a `Codable` type parameter was `bynk.generics.codable_op_return`; it is now `bynk.generics.codable_op_return_not_first_party`. Return plain data, such as a `String`, and decode it where you call the operation with `Json.decode[T]`.

## v0.582.0

A `claim` in a service handler that fails (a fault, a returned `Err`, or a 5xx `HttpResult`) is now released, so a retry can claim again; before this change a claim was held for its whole window whatever the outcome.

## v0.579.0

The `Idempotency` test double now honours `expiresAfter` on the harness clock, so a case that `advance`s `Clock` past a `remember` window sees `dedup` return `None`, and a zero or negative `expiresAfter` lapses at once; before, the entry never lapsed.

## v0.578.0

A `Cookie { name, value }` literal named for a field of a `cookies` record whose literal `value` that field's decoder rejects (`value: "dark"` for an enum read as `Dark`) is now the error `bynk.http.cookie_value_undecodable`, and an untyped `Cookie` literal named like a `SignedCookie` actor's session cookie is `bynk.actor.signed_cookie_write_literal`; fix the value, write it with `Record.field.cookie(v)`, or write the session cookie with `Actor.issue`/`Actor.expire`.

## v0.576.1

A `MySql.run` whose `insertId` or `affectedRows` is past 2^53 now answers `Err(MySqlFailed(0, …))`, where it read as `0`; match `MySqlFailed` as for any other failure.

## v0.549.4

Generated Workers pin `compatibility_date = "2026-08-04"`, so `bynk dev` needs wrangler 4.120.1 or newer; `bynk doctor` and `bynk dev` warn about an older one. Update with `npm install -g wrangler@4`.

## v0.545.0

`parTraverse`, `parTraverseAll` and `parTraverseTry` closures that write an agent's `store` (`put`, `remove`, `update`, `upsert`, `add`, `append`, or `:=`) are now refused with `bynk.par.store_write_in_closure`: run the loop with `forEach` (or `traverseAll`/`traverseTry`), or do the writes after the parallel part.

## v0.542.0

`BoundaryRejection` gains `MalformedForm(detail)`: a `match` over it with no wildcard stops compiling (`bynk.types.non_exhaustive_match` names the missing variant); add an arm for `MalformedForm(_)`, or a `_` arm.

