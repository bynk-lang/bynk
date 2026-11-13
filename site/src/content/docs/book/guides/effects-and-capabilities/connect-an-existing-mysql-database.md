---
title: Connect an existing MySQL database
---
**Goal:** read and write a MySQL or MariaDB database you already run (PlanetScale,
RDS, your own) from a Bynk Worker, through Cloudflare Hyperdrive, and test the
handler without a database.

`MySql` is the twin of [`Postgres`](/book/guides/effects-and-capabilities/connect-an-existing-postgres-database/):
the same operations, rows and transactions, with `?` placeholders and MySQL's types
and errors. It is **experimental**; opt in in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.MySql"]
```

Bynk does not own the schema. There is no `migrations/` folder: you manage the
database as you do today, and MySQL judges each statement.

## Declare the database

A database is a named instance of `MySql`; a bare `given MySql` is refused. The
optional `@tls` says how Hyperdrive secures its connection to the database
(`"required"`, the default, `"verify-ca"` or `"verify-identity"`):

```bynk,fragment
context shop.orders

consumes bynk
consumes bynk.cloudflare { MySql, MySqlError, MySqlValue, MySqlRun }

capability Legacy = MySql @tls("required")
```

`Legacy` binds as `HYPERDRIVE_LEGACY`, a `[[hyperdrive]]` entry in the generated
`wrangler.toml`.

## Query it

The SQL is a string literal and its values are `?` parameters, each a
`MySqlValue`. Every operation answers a `MySqlError` rather than faulting:

```bynk,fragment
service orders from http {
	on POST("/orders/:name") (name: String) -> Effect[HttpResult[String]] by Visitor given Legacy {
		let ran <- Legacy.run("insert into orders (name, qty) values (?, ?)", [MySqlText(name), MySqlInteger(1)])
		match ran {
			Ok(r) => Ok("inserted as " + r.insertId.toString())
			Err(MySqlConstraint(1062, _)) => Ok("already there")
			Err(_) => Ok("failed")
		}
	}

	on GET("/orders") () -> Effect[HttpResult[String]] by Visitor given Legacy {
		let rows <- Legacy.all("select id, name from orders where qty > ? limit ?, ?", [MySqlInteger(0), MySqlInteger(0), MySqlInteger(20)])
		match rows {
			Ok(json) => Ok(json)
			Err(_) => Ok("failed")
		}
	}
}
```

MySQL has no `RETURNING`, so `run` reports the `AUTO_INCREMENT` id it inserted as
`insertId` (an id past 2^53 is not an `Int`, so `run` answers `MySqlFailed` instead of a `0`; the statement has still run, so do not retry on it blindly). `all` and `first` return the rows as JSON text, for `Json.decode`:
`BIGINT` and `DECIMAL` columns arrive as strings, a `TINYINT(1)` as `true` or
`false`, a `BLOB` as base64, a `JSON` column nested. A `DATETIME` or `TIMESTAMP`
arrives as the server's text, `2026-11-12 09:30:00`, with no zone: read it as a
`String`. The
[reference](/book/reference/bynk-capabilities/#mysql-databases-mysql) has the full
rules.

**Unsigned integers, flags and dates.** An `Int` is signed, so a `BIGINT UNSIGNED`
binds as `MySqlUnsigned("18446744073709551615")` and reads as a `String` (or an
`Int`, while it fits). `MySqlBoolean(true)` binds `1`, which is what a
`TINYINT(1)` holds. A `DATETIME` has no time zone, so it is its own thing:
`MySqlDateTime("2026-11-12 09:30:00")` binds wall-clock text, and
`MySqlTimestamp(instant)` binds an `Instant` as UTC text.

## Read rows into a record, and change several rows

`select[T]` and `firstAs[T]` write the `SELECT` from the fields of `T`, quoted with
backticks, and `transaction` runs a fixed list of `MySqlStatement`s, all or none:

```bynk,fragment
consumes bynk.cloudflare { MySql, MySqlReadError, MySqlStatement }

type Order = { id: Int, name: String, placed: String, paid: Bool }

service orders from http {
	on GET("/orders/count") () -> Effect[HttpResult[Int]] by Visitor given Legacy {
		let rows <- Legacy.select[Order]("FROM orders WHERE qty > ? ORDER BY id", [MySqlInteger(0)])
		match rows {
			Ok(list) => Ok(list.length())
			Err(MySqlSql(_)) => Ok(-1)
			Err(MySqlDecode(_)) => Ok(-2)
		}
	}

	on POST("/orders/:name") (name: String) -> Effect[HttpResult[String]] by Visitor given Legacy {
		let done <- Legacy.transaction([
			MySqlStatement { sql: "insert into orders (name, qty) values (?, 1)", params: [MySqlText(name)] },
			MySqlStatement { sql: "update stock set qty = qty - 1 where name = ?", params: [MySqlText(name)] },
		])
		match done {
			Ok(_) => Ok("placed")
			Err(MySqlConstraint(_, _)) => Ok("out of stock")
			Err(_) => Ok("failed")
		}
	}
}
```

A row field is an `Int`, `Float`, `String`, `Bool`, `Bytes` or `Decimal`; an
`Instant` field is refused (`bynk.types.sql_row_shape`), because a MySQL date has no
zone to make an instant from.

## Give Bynk the connection string

As for Postgres, `bynk.toml` names an **environment variable** and never holds the
string, which is a secret and a `mysql://` URL:

```toml
[deploy.hyperdrive.Legacy]
connection_env = "LEGACY_DATABASE_URL"

[dev.hyperdrive.Legacy]
connection_env = "LEGACY_LOCAL_DATABASE_URL"
```

```sh
export LEGACY_LOCAL_DATABASE_URL="mysql://root:root@localhost:3306/shop"
bynk dev

export LEGACY_DATABASE_URL="mysql://user:password@db.example.com:3306/shop"
bynk deploy
```

Deploy makes the Hyperdrive config, with the `@tls` mode, and syncs it on every
deploy. `"verify-ca"` and `"verify-identity"` need a CA certificate your account
holds: deploy stops and names the commands to upload it and make the config by
hand, which you adopt with `config_id`. See the
[deploy guide](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#mysql-databases).

The Worker needs the `mysql2` npm package, which the build adds to
`out/package.json` pinned to an exact version. `bynk dev`, `bynk deploy` and
`bynk test` install it into the build directory with your own package manager
when it is missing; [`--no-install`](/docs/bynk-cli/#installing-npm-packages)
stops instead, naming the command to run. The connection's password is a secret:
it is never logged and never appears in an error.

## Test it with stubs

A test never reaches a database, so every operation faults until the case stubs it.
There is no in-memory MySQL double. A stub can answer a value or force any
`MySqlError`, and `expect … called with` reads the statement:

```bynk,fragment
suite shop.orders

case "a duplicate is answered" {
	stub Legacy.run(_, _) returns Err(MySqlConstraint(1062, "Duplicate entry 'a' for key 'name'"))
	let r <- api.POST("/orders/:name", "a") by Visitor
	expect r is Ok("already there")
	expect Legacy.run called with sql == "insert into orders (name, qty) values (?, ?)"
}
```

## Catch a bad statement at compile time

A `schema.sql` at the project root switches on the same check `Postgres` has, with
MySQL's parser: `bynk check` counts the `?` placeholders against the list you pass
(`bynk.mysql.sql_param_count`, an error) and parses the statement
(`bynk.mysql.sql_invalid`, a warning, since the parser is not MySQL's own). See
[MySQL SQL checked for syntax](/book/reference/bynk-capabilities/#mysql-sql-checked-for-syntax).

**See also:** [Reference: MySQL databases](/book/reference/bynk-capabilities/#mysql-databases-mysql).
