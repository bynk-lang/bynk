---
title: Apply webhook events that arrive late, twice or out of order
---
**Goal:** keep a table of facts right when the deliveries that change it can
arrive in any order, more than once, or long after the event.

A webhook sender rarely promises order. GitHub, for one, makes no ordering
promise, and its *Redeliver* button replays old deliveries on demand. If every
delivery is applied as it arrives, "removed" can land before the "added" it
undoes, and the stale fact then stays until something else repairs it.

The pattern here keeps one rule in the SQL: **apply a change by the time the
event happened, not the time it was received.** The example is a table of
review requests on pull requests ("who is waiting on a review"), and the
pieces carry over to any set of facts that deliveries add to and remove from.
It is the shape the `github-hooks` service settled on; each rule below was a
review finding there before it was a rule.

## The tables

```text
pulls           repo, number, updated_at      -- the newest delivery recorded
review_requests repo, number, reviewer, requested_at
review_tombstones repo, number, reviewer, at  -- the latest removal of each
```

- **`requested_at`** is the event time that set the row, the delivery's own
  `updated_at`, never the time it was received. A row recorded before the column
  existed has none (`NULL`), and any removal may delete it.
- **A tombstone** records a removal at its own time. It is what lets a request
  that arrives *after* its removal know it was already withdrawn.

Timestamps here are ISO 8601 text in one format, so SQLite compares them as
strings and the comparison is chronological.

## The rules

| Delivery | Its time | Rule |
|---|---|---|
| a single request | the event's `updated_at` | Add it unless a tombstone is as new or newer, or the pull's row is strictly newer. If it is already there, keep the later `requested_at`. |
| a single removal | the event's `updated_at` | Delete the row only if it was requested no later. Leave a tombstone at this time, keeping the latest. |
| a full set | the event's `updated_at` | Replace the rows, then clear the tombstones it supersedes. |
| retention | the clock | Prune tombstones older than a day. |

### A removal that arrives first still wins

A removal deletes only what was requested no later than itself, and leaves a
tombstone at its own time either way. When the request it undoes arrives
afterwards, the tombstone stops it being added:

```bynk
context hooks

consumes bynk
consumes bynk.cloudflare { D1 }

capability Deliveries = D1

// Removes one request as of `at`. The delete spares a request made later; the
// tombstone is kept at the latest removal time, so a late, older removal
// never moves it back.
fn removeRequest(repo: String, number: Int, reviewer: String, at: String) -> List[SqlStatement] {
	[
		SqlStatement {
			sql: """sql
				DELETE FROM review_requests
				WHERE repo = ?1 AND number = ?2 AND reviewer = ?3 COLLATE NOCASE
				  AND (requested_at IS NULL OR requested_at <= ?4)
				""",
			params: [Text(repo), Integer(number), Text(reviewer), Text(at)],
		},
		SqlStatement {
			sql: """sql
				INSERT INTO review_tombstones (repo, number, reviewer, at) VALUES (?1, ?2, ?3, ?4)
				ON CONFLICT (repo, number, reviewer) DO UPDATE
				  SET at = max(review_tombstones.at, excluded.at)
				""",
			params: [Text(repo), Integer(number), Text(reviewer), Text(at)],
		},
	]
}
```

Both statements are string literals with `?1`… parameters, as `D1` requires
(`bynk.types.sql_not_literal` otherwise); run them together with
`Deliveries.batch(removeRequest(…))` so the delete and its tombstone apply as
one transaction.

### Ignore a single change the pull has already moved past

A single request is an `INSERT … SELECT … WHERE NOT EXISTS … ON CONFLICT`: the
`SELECT` yields a row only when two guards pass, and the `ON CONFLICT` keeps the
later time for a request already present:

```bynk
context hooks

consumes bynk
consumes bynk.cloudflare { D1 }

capability Deliveries = D1

fn addRequest(repo: String, number: Int, reviewer: String, at: String) -> List[SqlStatement] {
	[
		SqlStatement {
			sql: """sql
				INSERT INTO review_requests (repo, number, reviewer, requested_at)
				SELECT ?1, ?2, ?3, ?4
				WHERE NOT EXISTS (SELECT 1 FROM review_tombstones t
				                  WHERE t.repo = ?1 AND t.number = ?2 AND t.reviewer = ?3 AND t.at >= ?4)
				  AND NOT EXISTS (SELECT 1 FROM pulls WHERE repo = ?1 AND number = ?2 AND updated_at > ?4)
				ON CONFLICT (repo, number, reviewer) DO UPDATE
				  SET requested_at = max(coalesce(review_requests.requested_at, ''), excluded.requested_at)
				""",
			params: [Text(repo), Integer(number), Text(reviewer), Text(at)],
		},
	]
}
```

- **The tombstone guard** (`t.at >= ?4`) is the out-of-order case above. It uses
  `>=` so that a removal and a request in the *same second* resolve in favour of
  the removal. The order within a second is unknown; choose a side deliberately
  and let the next full set settle it.
- **The pull-row guard** (`updated_at > ?4`) handles *redelivery*. Once a
  strictly newer delivery has recorded the pull, its full set has already
  accounted for every earlier change, so an older single request, redelivered
  from a button press a week later, must change nothing. Without this guard a
  redelivered request re-adds state as soon as its tombstone has been pruned,
  because nothing is left to say it was withdrawn.

### Replace the set, and clear what it supersedes

A delivery that carries the whole set (here, the pull request itself, listing
everyone asked) is authoritative as of its time. It replaces the rows, then
clears the tombstones no later than itself, since those removals are now
reflected in the set. A tombstone *newer* than the set survives, and the guard
above still applies when the set's own rows are inserted:

```bynk
context hooks

consumes bynk
consumes bynk.cloudflare { D1 }

capability Deliveries = D1

fn replaceSet(repo: String, number: Int, updatedAt: String, reviewers: List[String]) -> List[SqlStatement] {
	[
		SqlStatement {
			sql: """sql
				DELETE FROM review_requests
				WHERE repo = ?1 AND number = ?2
				  AND NOT EXISTS (SELECT 1 FROM pulls WHERE repo = ?1 AND number = ?2 AND updated_at > ?3)
				""",
			params: [Text(repo), Integer(number), Text(updatedAt)],
		},
		SqlStatement {
			sql: """sql
				DELETE FROM review_tombstones
				WHERE repo = ?1 AND number = ?2 AND at <= ?3
				  AND NOT EXISTS (SELECT 1 FROM pulls WHERE repo = ?1 AND number = ?2 AND updated_at > ?3)
				""",
			params: [Text(repo), Integer(number), Text(updatedAt)],
		},
	].concat(
		reviewers.map(
			(r) => SqlStatement {
				sql: """sql
					INSERT OR IGNORE INTO review_requests (repo, number, reviewer, requested_at)
					SELECT ?1, ?2, ?3, ?4
					WHERE NOT EXISTS (SELECT 1 FROM pulls WHERE repo = ?1 AND number = ?2 AND updated_at > ?4)
					  AND NOT EXISTS (SELECT 1 FROM review_tombstones t
					                  WHERE t.repo = ?1 AND t.number = ?2 AND t.reviewer = ?3 AND t.at > ?4)
					""",
					params: [Text(repo), Integer(number), Text(r), Text(updatedAt)],
			}
		)
	)
}
```

Each statement carries the `updated_at` guard itself, so a stale set, delivered
late, is a no-op rather than a rollback to older state. Record the pull's own
row *after* the set, in the same `batch`, so the guards compare against the
previous newest time.

### Retention

A tombstone only needs to outlive the window in which a delivery it guards
against can still arrive. Prune past that, from a cron handler:

```bynk,fragment
service retention from cron {
	on schedule("0 * * * *") (at: Int) -> Effect[Result[(), SqlError]] given Deliveries {
		let cutoff = (Instant.fromEpochMillis(at) - 1.days).toIso()
		let Ok(_) <- Deliveries.run(
			"DELETE FROM review_tombstones WHERE at < ?1",
			[Text(cutoff)]
		) else match {
			Err(e) => Err(e),
		}
		Ok(())
	}
}
```

Pruning is safe only because of the pull-row guard: after the tombstone is gone,
a replayed single request is still refused while a newer row exists.

## Pin each rule with a test

Each rule was found by breaking it on purpose: change a guard, and a test must
fail. Write one case per rule, driving the deliveries in the *wrong* order:

- a removal delivered before its request leaves the table empty;
- a request made after a removal, and delivered first, stays;
- a late, **older** removal does not move a newer tombstone back;
- a full set clears the tombstones it supersedes, and keeps a newer one;
- the redelivery case: a request older than the pull's row, replayed after its
  tombstone is pruned, adds nothing;
- retention prunes only tombstones older than the window.

```bynk,fragment
case "a removal that arrives before the request it undoes still wins" {
	let _ <- api.POST("/hook", headers("pull_request", "rm-1"), removal("c-person", "2026-11-05T09:01:00Z")) by Webhook
	let _ <- api.POST("/hook", headers("pull_request", "rq-1"), request("c-person", "2026-11-05T09:00:00Z")) by Webhook
	let waiting <- Deliveries.first("SELECT group_concat(reviewer, ',') AS who FROM review_requests", [])
	expect waiting == requested("")
}
```

See [Testing](/book/guides/testing/) for driving a handler and reading the
database from a case. A mutation pass (break each guard, expect a failure) is
the quickest check that the suite pins every rule.

**See also:** [Verify an inbound webhook](/book/guides/actors/verify-webhooks/),
[Reference, SQL databases: `D1`](/book/reference/bynk-capabilities/#sql-databases-d1).
