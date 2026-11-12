---
title: Connect an existing Postgres database
---
**Goal:** read and write a Postgres database you already run (Neon, Supabase,
RDS, your own) from a Bynk Worker, through Cloudflare Hyperdrive, and test the
handler without a database.

`Postgres` is **experimental**: it carries no compatibility promise yet. Opt in
in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.Postgres"]
```

Bynk does not own the schema. There is no `migrations/` folder and no check of
your SQL against it: you manage the database as you do today, and Postgres judges
each statement.

## Declare the database

A database is a named instance of `Postgres`; a bare `given Postgres` is refused.
Consume the adapter for the capability and for the types its operations use:

```bynk,fragment
context shop.orders

consumes bynk
consumes bynk.cloudflare { Postgres, PgError, PgValue, PgRun }

capability Main = Postgres
```

`Main` binds as `HYPERDRIVE_MAIN`, a `[[hyperdrive]]` entry in the generated
`wrangler.toml`. Declare as many instances as you have databases.

## Query it

The SQL is a string literal and its values are `$1`, `$2`, … parameters, each a
`PgValue`, so data never reaches the statement as text. Every operation answers a
`PgError` rather than faulting:

```bynk,fragment
service orders from http {
	on POST("/orders/:name") (name: String) -> Effect[HttpResult[String]] by Visitor given Main {
		let ran <- Main.run("insert into orders (name, qty) values ($1, $2)", [PgText(name), PgInteger(1)])
		match ran {
			Ok(r) => Ok("inserted")
			Err(PgConstraint(_, _)) => Ok("already there")
			Err(_) => Ok("failed")
		}
	}

	on GET("/orders") () -> Effect[HttpResult[String]] by Visitor given Main {
		let rows <- Main.all("select id, name from orders where qty > $1", [PgInteger(0)])
		match rows {
			Ok(json) => Ok(json)
			Err(_) => Ok("failed")
		}
	}
}
```

`all` and `first` return the rows as JSON text, for `Json.decode`: `bigint` and
`numeric` columns arrive as strings, `timestamptz` as ISO-8601 text, `bytea` as
base64, and `json` and `jsonb` nested. The
[reference](/book/reference/bynk-capabilities/#postgres-databases-postgres) has the
full rules and the `PgError` variants.

## Read rows into a record

Writing a column list in the SQL and again in a row type drifts. `select[T]` and
`firstAs[T]` take only the statement from `FROM` on and write the `SELECT` from
the fields of `T`, quoted as Postgres quotes them:

```bynk,fragment
consumes bynk.cloudflare { Postgres, PgReadError }

type Order = { id: Int, name: String, placed: Instant, note: Option[String] }

service orders from http {
	on GET("/orders/count") () -> Effect[HttpResult[Int]] by Visitor given Main {
		let rows <- Main.select[Order]("FROM orders WHERE qty > $1 ORDER BY id", [PgInteger(0)])
		match rows {
			Ok(list) => Ok(list.length())
			Err(PgSql(_)) => Ok(-1)
			Err(PgDecode(_)) => Ok(-2)
		}
	}
}
```

`firstAs[T]` gives an `Option[T]`. A `bigint` column reads into an `Int` (it
fails the decode past 2^53, rather than losing digits) and a `timestamptz` into an
`Instant`, whatever offset Postgres writes. A field takes the column of its own
name, so alias a computed column in a statement you write with `all` instead.

## Change several rows, all or none

`transaction` runs a list of statements in order on one connection. If one fails,
the earlier ones are rolled back and you get that statement's `PgError`:

```bynk,fragment
consumes bynk.cloudflare { Postgres, PgStatement }

service orders from http {
	on POST("/orders/:name") (name: String) -> Effect[HttpResult[String]] by Visitor given Main {
		let done <- Main.transaction([
			PgStatement { sql: "insert into orders (name, qty) values ($1, 1)", params: [PgText(name)] },
			PgStatement { sql: "update stock set qty = qty - 1 where name = $1", params: [PgText(name)] },
		])
		match done {
			Ok(_) => Ok("placed")
			Err(PgConstraint(_, _)) => Ok("out of stock")
			Err(_) => Ok("failed")
		}
	}
}
```

The list is fixed up front. A read whose answer decides a later write cannot sit
between them; put the decision in one statement (`update … where qty > 0`) and
check the `rowCount`.

## Give Bynk the connection string

`bynk deploy` makes the Hyperdrive configuration, and `bynk dev` needs a local
database. Both read the connection string from an **environment variable** that
`bynk.toml` names, because the string is a secret and `bynk.toml` is committed:

```toml
[deploy.hyperdrive.Main]
connection_env = "MAIN_DATABASE_URL"

[dev.hyperdrive.Main]
connection_env = "MAIN_LOCAL_DATABASE_URL"
```

```sh
export MAIN_LOCAL_DATABASE_URL="postgres://postgres:postgres@localhost:5432/shop"
bynk dev

export MAIN_DATABASE_URL="postgres://user:password@db.example.com:5432/shop"
bynk deploy
```

`bynk dev` hands the local string to that Worker's `wrangler dev` as
`CLOUDFLARE_HYPERDRIVE_LOCAL_CONNECTION_STRING_HYPERDRIVE_MAIN`, so the Worker
reaches your own Postgres. Deploy makes the config from the other variable, syncs
it from that variable on every deploy, and records its id, so there is no
`wrangler hyperdrive create` and no id to paste. A team whose config already exists
names it with `config_id` instead and needs no string at deploy. The
[deploy guide](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#postgres-databases-hyperdrive)
has the details, and
[running locally](/book/guides/projects-build-and-deployment/run-locally/#a-local-postgres-database)
the dev side.

The connection string is a secret. Bynk reads it from the binding and passes it to
the driver; it is never logged and never appears in an error. Keep it out of source
and out of `wrangler.toml`.

The Worker also needs the `postgres` npm package, which the build adds to
`out/package.json` (pinned to an exact version). Run `npm install` in the build
directory before `bynk dev` or `bynk deploy`; both stop and name the package if it
is missing.

## Mind the connection limit

Each statement opens its own connection and closes it before it answers.
Cloudflare allows six simultaneous connections per Worker invocation, so a
[`par`](/book/guides/effects-and-capabilities/run-independent-calls-at-the-same-time/)
over more than six statements runs them in turns, not all at once.

## Test it with stubs

A test never reaches a database, so every operation faults until the case stubs
it, as a `Service` does. A typed read goes through `all` and `first`, so stub those with the rows as JSON
text; `transaction` is stubbed as `stub Main.transaction(_) returns …`. A stub can
answer a value or force any `PgError`:

```bynk,fragment
suite shop.orders

case "an insert is answered" {
	stub Main.run(_, _) returns Ok(PgRun { rowCount: 1 })
	let r <- api.POST("/orders/:name", "a") by Visitor
	expect r is Ok("inserted")
}

case "a duplicate is answered" {
	stub Main.run(_, _) returns Err(PgConstraint("23505", "duplicate key value"))
	let r <- api.POST("/orders/:name", "a") by Visitor
	expect r is Ok("already there")
}
```

## Test it against a real Postgres

Stubs cannot tell you that your SQL is right. To run the statements, opt in to
PGlite, Postgres compiled to WebAssembly, in `bynk.toml`:

```toml
[test]
postgres = "pglite"
```

Keep your schema in a `schema.sql` at the project root. Every case starts from an
empty in-memory database with that file applied, so the tables are there and the
rows from the case before are not:

```sql
CREATE TABLE orders (
	id serial PRIMARY KEY,
	name text NOT NULL UNIQUE
);
```

The same case, with nothing stubbed, now runs `insert into orders …` for real, so
the second `POST` of a name meets the `UNIQUE` constraint and the handler sees a
`PgConstraint` with SQLSTATE `23505`. A `stub` still answers before the database,
which is how a case forces what one connection cannot: a `PgSerialization`
failure needs two transactions at once, so stub it to test your retry.

The first `bynk test` of a project that opts in installs PGlite (about 27 MB), and
each run starts it once, about 1.5 seconds per `Postgres` instance. Leave the
setting out and tests stay as quick as before, with every operation stubbed. See
[Postgres under test](/book/reference/testing/#postgres-under-test).

## Catch a bad statement at compile time

The same `schema.sql` switches on a check. `bynk check` counts the `$n`
placeholders of each literal statement against the list you pass (an error), and
parses the statement (a warning, since the parser is not Postgres' own), so a typo
or a missing parameter shows in the editor instead of as a `PgFailed` on the first
request:

```bynk,fragment
let _ <- Main.all("selec name from orders", [])                      // warning: bynk.postgres.sql_invalid
let _ <- Main.all("select name from orders where id = $1", [])       // error: bynk.postgres.sql_param_count
```

It does not check names against your tables: Bynk does not own the schema. Without
a `schema.sql` there is no check. See
[Postgres SQL checked for syntax](/book/reference/bynk-capabilities/#postgres-sql-checked-for-syntax).

**See also:** [Reference: Postgres databases](/book/reference/bynk-capabilities/#postgres-databases-postgres),
[Run independent calls at the same time](/book/guides/effects-and-capabilities/run-independent-calls-at-the-same-time/).
