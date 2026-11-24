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

