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

The [changelog](/book/reference/changelog/) is the history of *every* increment;
this page is only the ones that break something. An increment that is missing
here broke nothing, or its author did not say so: please
[open an issue](https://github.com/bynk-lang/bynk/issues/new?template=bug_report.yml).

<!-- stamp:breaking-entries -->

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

