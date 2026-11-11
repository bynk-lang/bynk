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

## Create the Hyperdrive configuration by hand

`bynk deploy` does **not** create the Hyperdrive configuration yet (that, and the
`bynk dev` wiring, are the next slice). Until then:

1. Create the configuration, once per database and environment, with the
   connection string of your database:

   ```sh
   npx wrangler hyperdrive create shop-main --connection-string="postgres://user:password@db.example.com:5432/shop"
   ```

   It prints the configuration's `id`.
2. Replace the placeholder in the generated `wrangler.toml`:

   ```toml
   [[hyperdrive]]
   binding = "HYPERDRIVE_MAIN"
   id = "<the id wrangler printed>"
   ```

   A rebuild writes the placeholder again, so keep the id where your deploy step
   can substitute it.
3. For `bynk dev`, point the binding at a local database. Either add
   `localConnectionString` to the `[[hyperdrive]]` entry, or set the environment
   variable for the binding:

   ```sh
   export CLOUDFLARE_HYPERDRIVE_LOCAL_CONNECTION_STRING_HYPERDRIVE_MAIN="postgres://user:password@localhost:5432/shop"
   ```

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
it, as a `Service` does. A stub can answer a value or force any `PgError`:

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

**See also:** [Reference: Postgres databases](/book/reference/bynk-capabilities/#postgres-databases-postgres),
[Run independent calls at the same time](/book/guides/effects-and-capabilities/run-independent-calls-at-the-same-time/).
