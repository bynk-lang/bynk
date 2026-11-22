---
title: Read your analytics in the Worker
---
**Goal:** read back the events a Worker writes to an Analytics Engine dataset, with a SQL query run
from the Worker itself, and test the code without reaching your account.

`Analytics` writes events (a sign-up, a page view) to a dataset; `AnalyticsSql` reads them back
through Cloudflare's [Analytics Engine SQL API](https://developers.cloudflare.com/analytics/sql-api/workers-binding/).
This guide writes one event, counts the day's events with a query, and shows what changes between
environments. Both capabilities are **experimental**: they carry no compatibility promise yet. Opt in
in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.Analytics", "bynk.cloudflare.AnalyticsSql"]
```

## Write an event

A dataset is a named instance of `Analytics`. It takes no options: its name is derived from the
context and the capability, and the platform creates it on the first write, so there is nothing to
provision.

```bynk,fragment
context shop.signups

consumes bynk
consumes bynk.cloudflare { Analytics, DataPoint }

capability Signups = Analytics

service join from http {
	on POST("/join/:country") (
		country: String
	) -> Effect[HttpResult[String]] by Visitor given Signups {
		let _ <- Signups.write(DataPoint { index: Some(country), blobs: [country], doubles: [1.0] })
		Created("counted")
	}
}
```

The country is stored as `blob1`: blobs are numbered from 1 in the order you give them, and the
SQL below refers to it by that name. See [Writing events: `Analytics`](/reference/bynk-capabilities/#writing-events-analytics)
for the limits on a data point.

## Read them back

`AnalyticsSql` is bound once per Worker and used bare, as `Ai` is: there are no named instances. Its one
operation takes the query text and its parameters, and answers a result or an error:

```bynk,fragment
consumes bynk.cloudflare { Analytics, AnalyticsSql, AnalyticsSqlError }

type Row = { n: Int }

service report from http {
	on GET("/joined/:country") (country: String) -> Effect[HttpResult[Int]] by Visitor given AnalyticsSql, Signups {
		let table <- Signups.dataset()
		let sql = "SELECT count() AS n FROM \(table) WHERE blob1 = $1 AND timestamp > NOW() - INTERVAL '1' DAY"
		let res <- AnalyticsSql.query(sql, [Text(country)])
		match res {
			Ok(r) => {
				let rows = Json.decode[List[Row]](r.data)
				match rows {
					Ok([first]) => Ok(first.n),
					_ => ServerError("the reply was not one count"),
				}
			},
			Err(SqlRetryable(_)) => ServerError("try again shortly"),
			Err(_) => ServerError("the query was refused"),
		}
	}
}
```

This continues the `shop.signups` context above: its `consumes bynk.cloudflare` line replaces the earlier one.

The count is **in the data**, not in `rows`. `SELECT count()` with no `GROUP BY` always returns exactly one row,
so `rows` would be `1` whether nobody or thousands had joined. The reply's `data` is `[{"n":4}]`, and the handler
decodes it into rows and reads `n` from the only one.

A successful result is `AnalyticsSqlResult`: `rows` is how many **result rows** came back (not what a
`count()` counted), `data` is those rows as JSON text (an array of one object per row, keyed by column), and
`statistics` says what the query cost the
platform (`elapsedMs`, `rowsRead`, `bytesRead`).

The rules of the SQL API apply as Cloudflare states them, and the platform, not Bynk, checks the query:

- **Bound the time range from below.** The query above reads the last day (`timestamp > NOW() - INTERVAL '1' DAY`),
  not the whole retention period. A query with no lower bound on `timestamp` reads everything the dataset
  keeps, and gets slower as the dataset grows.
- **Do not filter on `accountTag` or `zoneTag`**, and do not add a `FORMAT` clause. The binding scopes the
  query to your account and returns JSON itself.

## Pass values as parameters

Put a value in the query with a parameter, never by building the query text around it. `$1` is the first
element of the parameter list, `$2` the second, and so on:

```bynk,fragment
let sql = "SELECT count() AS n FROM \(table) WHERE blob1 = $1 AND double1 > $2 AND timestamp > NOW() - INTERVAL '7' DAY"
let res <- AnalyticsSql.query(sql, [Text(country), Real(0.5)])
```

Interpolating `country` into the string instead would let a caller choose part of your query. A parameter is
always a value, never SQL. Parameters are the same `SqlValue`s `D1` takes, and Analytics SQL accepts `Null`,
`Integer`, `Real`, `Text` and `Boolean`: a `Blob`, an `Integer` too large to be read exactly, or a `Real` that
is not finite comes back as `Err(SqlRejected(…))` before anything is sent.

The one exception is the table name, which a parameter cannot stand for. That is what `dataset()` is for.

## Name the dataset for the environment you run in

The dataset a Worker writes to differs by environment: `shop_signups_signups` in the default one and
`shop_signups_signups_staging` under `--env staging`. Hard-coding either name makes the other environment read
the wrong table. `dataset()` gives the table for the environment the Worker is running in:

```bynk,fragment
let table <- Signups.dataset()
```

In the default environment that is `events.analyticsEngine.shop_signups_signups`; under `--env staging` it is
`events.analyticsEngine.shop_signups_signups_staging`. You write neither name. The name comes from one `[vars]`
entry in the generated configuration, `BYNK_DATASET_ANALYTICS_SIGNUPS` for `Signups`, which `bynk deploy`
qualifies for each environment the same way it qualifies the dataset the Worker writes to. Names starting
`BYNK_DATASET_` are reserved: a `[test.targets]` `vars` entry with one is a manifest error.

## Handle the two errors

`AnalyticsSqlError` has two variants. `SqlRetryable` is a failure the platform marks as worth another try,
which is the case for a transient fault; `SqlRejected` is everything else, including a query the platform does
not accept and a parameter Analytics SQL has no counterpart for. Retrying a `SqlRejected` query only fails
again. The enum is **extensible**, so a later release may add a variant: a `match` over it needs an `Err(_)`
arm, as the handler above has.

## Deploy

`bynk deploy` has nothing to create or link. The binding is `CF_ANALYTICS_SQL`, an `[analytics]` stanza that
names no resource, and it is copied into each environment. The only thing that differs between environments is
the dataset name, handled above.

## Run it locally

There is no local implementation of Analytics SQL, so under `bynk dev` a query runs on your Cloudflare account
and reads your deployed datasets. `bynk dev` warns once, on the first query, and you silence it once you have
read it:

```toml
[dev]
analytics-sql = "allow"
```

## Test it

A test never reads your account. `query` **faults** until you stub it, so a test cannot reach Analytics Engine by
accident, and there is no fake data to pass for the wrong reason. `dataset()` answers a stand-in name
(`events.analyticsEngine.test_signups`) unless you stub it:

```bynk,fragment
suite shop.signups {
	consumes bynk
	consumes bynk.cloudflare { AnalyticsSql, AnalyticsSqlError, AnalyticsSqlResult, AnalyticsSqlStatistics }

	case "an unstubbed query faults" {
		expect report.GET("/joined/:country", "uk") by Visitor faults
	}

	case "a stubbed result is what the handler sees" {
		stub AnalyticsSql.query(_, _) returns Ok(AnalyticsSqlResult { data: "[{\"n\":4}]", rows: 1, statistics: AnalyticsSqlStatistics { elapsedMs: 3, rowsRead: 9, bytesRead: 72 } })
		let r <- report.GET("/joined/:country", "uk") by Visitor
		expect r is Ok(4)
	}

	case "a retryable failure is matched" {
		stub AnalyticsSql.query(_, _) returns Err(SqlRetryable("busy"))
		let r <- report.GET("/joined/:country", "uk") by Visitor
		expect r is ServerError("try again shortly")
	}

	case "the query reads the environment's own dataset" {
		stub Signups.dataset() returns "events.analyticsEngine.shop_signups_signups_staging"
		stub AnalyticsSql.query(_, _) returns Ok(AnalyticsSqlResult { data: "[{\"n\":0}]", rows: 1, statistics: AnalyticsSqlStatistics { elapsedMs: 1, rowsRead: 0, bytesRead: 0 } })
		let r <- report.GET("/joined/:country", "uk") by Visitor
		expect r is Ok(0)
		expect AnalyticsSql.query called with sql == "SELECT count() AS n FROM events.analyticsEngine.shop_signups_signups_staging WHERE blob1 = $1 AND timestamp > NOW() - INTERVAL '1' DAY"
	}
}
```

The reference is [Reading events: `AnalyticsSql`](/reference/bynk-capabilities/#reading-events-analyticssql).
