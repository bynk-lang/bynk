# 0868 — `AnalyticsSql` is a singleton binding over the Analytics Engine SQL API, remote-only under `bynk dev`

- **Status:** Accepted (v0.633)

**Context.** Analytics Engine datasets are written with `Analytics` and, until now, read only outside
Bynk. Proposal #3513 Part B, as the maintainer amended it, checked against Cloudflare's documentation
([the Workers binding](https://developers.cloudflare.com/analytics/sql-api/workers-binding/),
[local development](https://developers.cloudflare.com/workers/local-development/)).

**Decision.**

- **A singleton, `AnalyticsSql`.** The binding names no resource, so it is one per Worker and used bare,
  as `Ai` and `Version` are; `capability X = AnalyticsSql` is refused
  (`bynk.capability.base_not_instantiable`). It binds as `CF_ANALYTICS_SQL`, an `[analytics]` stanza,
  chosen so that an `Analytics` instance named `Sql` (`ANALYTICS_SQL`) cannot collide with it. It is
  experimental and `[env.*]` blocks copy the stanza, since bindings are not inherited.
- **`query(sql: String, params: List[SqlValue])`.** No new parameter type: D1's `SqlValue` is reused and
  parameters bind positionally as `$1`. Analytics SQL takes `Null`, `Integer`, `Real`, `Text` and
  `Boolean`; a `Blob`, an `Integer` outside the exactly-representable range and a non-finite `Real`
  come back as `Err(SqlRejected)` before anything is sent.
- **The result** is `AnalyticsSqlResult { data: String, rows: Int, statistics: AnalyticsSqlStatistics }`,
  `data` being the rows as JSON text and the statistics `elapsedMs`, `rowsRead` and `bytesRead`, the
  binding's `elapsed_ms`, `rows_read` and `bytes_read`. A reply that is not that shape is `SqlRejected`.
- **The error** is `AnalyticsSqlError = SqlRetryable(message) | SqlRejected(message)`, extensible, so a
  later release may add a variant. `retryable` is the platform's boolean on the thrown error, and an
  error without it is `SqlRejected`.
- **Remote-only under `bynk dev` (ADR 0834).** `[dev] analytics-sql = "allow"` is a new acknowledgement
  (`BYNK_DEV_ACK_ANALYTICS_SQL`). The provider warns once, on the first query, that Analytics SQL has no
  local implementation so the query runs on the account and reads its deployed datasets, with a link and
  no word of price. The development copy of the config marks `[analytics]` `remote = true`; the config
  `bynk deploy` reads never carries it.
- **Tests.** A test never reads Analytics Engine: `query` faults until stubbed, in both the unit and
  the integration double, as `Ai` does.
- **Documentation** states the SQL API's rules as the platform does: a dataset is
  `events.analyticsEngine.<DATASET>`, a query should carry a lower `timestamp` bound, and it must not
  filter on `accountTag` or `zoneTag` or add a `FORMAT` clause.

- **`Analytics.dataset() -> Effect[String]`.** The environment-qualified name of an `Analytics`
  instance's dataset, `events.analyticsEngine.<DATASET>`, so a query never hard-codes an environment.
  The qualified name exists only at deploy time, so the generated configuration carries it in a
  `[vars]` table, one `BYNK_DATASET_<BINDING>` entry per dataset, which `bynk deploy` qualifies in
  each `[env.*]` block exactly as it does the binding's `dataset` and merges into any `vars` there,
  never replacing one. `vars` is not inherited by `[env.*]`, which the #3577 guard test enforces for
  every table the emitter writes. The `BYNK_DATASET_` prefix is reserved: the generated
  configuration has no user-written `[vars]`, and the one place a project names variables for a
  Worker, a `[test.targets.*]` `vars` table, refuses it. In tests the unit double answers
  `events.analyticsEngine.test_<name>` and the integration harness sets the variable.

**Consequences.** A Worker can read its own events back, with nothing to provision. A query's text is
checked only by the platform. `bynk deploy` has nothing to create or link.
