---
title: Query rows into a record without repeating the columns
---
**Goal:** read rows from a `D1` database into a record, with the `SELECT`
column list written from the record rather than spelt twice.

With `all` and `Json.decodeSql`, a query lists its columns in the SQL and again
in the record, and nothing ties the two together: a missing or misspelt column
is a decode failure at run time. `select[T]` and `firstAs[T]` close that gap.
You write the statement from `FROM` on, as a string literal; the compiler writes
`SELECT` and the column list from `T`'s fields.

## Declare the row and query it

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { D1 }

capability Deliveries = D1

type Activity = { id: String, event: String, receivedAt: Int }

service api from http {
	on GET("/activity/:repo") (repo: String) -> Effect[HttpResult[List[Activity]]] by Visitor given Deliveries {
		let rows <- Deliveries.select[Activity]("""sql
			FROM deliveries
			WHERE repo = ?1
			ORDER BY received_at DESC
			LIMIT 50
			""", [Text(repo)])
		match rows {
			Ok(list) => Ok(list)
			Err(_) => ServerError("the activity did not load")
		}
	}
}
```

The statement that runs is `SELECT "id", "event", "receivedAt" FROM deliveries
WHERE …`: each field is a quoted column of the same name, in declaration order.
There is no renaming: a field selects the column of the same name. A computed
column (`count(*) AS total`) stays with `all` and `Json.decodeSql`.

The `sql` after the opening `"""` is a language tag: editors highlight the
statement as SQL, and the value is the same as without it.

The tail is still a string literal, so the injection guarantee holds: data
reaches the statement only as a bound `?1`, `?2`, … parameter. It must not begin
with `SELECT` (`bynk.types.select_tail_has_select`), and a variable or an
interpolated string is `bynk.types.sql_not_literal`. `T` must be a row, as for
`Json.decodeSql` (`bynk.types.sql_row_shape`); `select[List[Activity]]` is
refused, because `select` already returns the `List`.

## One row

`firstAs[T]` is the single-row form, giving `Option[T]`: `None` when no row
matches.

```bynk,fragment
service find {
	on call(id: String) -> Effect[Result[Option[Activity], SqlReadError]] given Deliveries {
		Deliveries.firstAs[Activity]("FROM deliveries WHERE id = ?1", [Text(id)])
	}
}
```

## Handle the two failures

Both operations return `Result[…, SqlReadError]`, where `SqlReadError` is
`Sql(SqlError)` for the statement's own failure (a missing column, say) and
`Decode(JsonError)` for a row that did not fit `T`, a column whose stored value
disagrees with its field's type:

```bynk,fragment
fn describe(rows: Result[List[Activity], SqlReadError]) -> String {
	match rows {
		Ok(_) => "loaded"
		Err(Sql(_)) => "the query failed"
		Err(Decode(_)) => "a row did not match its type"
	}
}
```

**See also:** [Reference, SQL databases: `D1`](/book/reference/bynk-capabilities/#sql-databases-d1),
[Apply webhook events that arrive late, twice or out of order](/book/guides/effects-and-capabilities/apply-webhook-events-out-of-order/).
