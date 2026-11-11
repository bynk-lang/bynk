---
level: minor
probes: emit_diagnostics, emit_abi_shapes, ts_writes
changelog: A `Postgres` instance has typed reads `select[T]` and `firstAs[T]`, which write the column list from `T` and decode the rows (an `int8` into an `Int`, a `timestamptz` into an `Instant`), and `transaction`, which runs a fixed list of `PgStatement`s all or none
---

## ADR: hyperdrive-h3-typed-reads-and-transaction
title: Hyperdrive H3: typed reads and `transaction` for `Postgres`, and what stays apart from `decodeSql`
summary: Variant names, how an `Instant` and an `int8` are read, why `decodeSql` is not unified with the codec yet, and how a transaction reaches the driver

**Context.** ADR 0799 settled the driver and the row rules, and H1 built
`Postgres.all`, `first` and `run`. After H1 a `Postgres` read spells its columns
twice, in the SQL and in the row type, and a multi-statement write has no atomic
form. Slice H3 ([#3320](https://github.com/bynk-lang/bynk/issues/3320), part of
#3184) adds the typed reads of ADR 0728 and `transaction`, and decides the two
questions ADR 0799 F left for it. Decision C of #3320 (interactive transactions
are out of scope) was accepted as recommended and is carried out as written:
`transaction` runs a list that is fixed up front.

**Decision.**

- **A. `select[T]` and `firstAs[T]` reuse ADR 0728's lowering, with Postgres'
  names.** The checker records the same `Callee::D1Select`, now carrying whether
  the instance is a `Postgres`; the emitter lowers the call to the existing
  `all`/`first` followed by the decoder. The tail is a literal that continues
  from `FROM`; the compiler writes `SELECT` and `T`'s field names ahead of it as
  quoted identifiers (double quotes, an embedded quote doubled, which is the
  rule `D1`'s already used and is also Postgres'). They are not declared
  operations of `capability Postgres`, so no runtime operation and no binding
  change is involved. A `Postgres` read is not recorded as a `D1Sql` site: that
  site is prepared against a D1 migration's SQLite, which says nothing about
  Postgres, as for `all`, `first` and `run`. A row type is the same record of
  columns as for `D1`, and additionally may hold an `Instant`, which a
  `timestamptz` arrives as.
- **B. The error type is `PgReadError = PgSql(error: PgError) | PgDecode(error:
  DecodeError)`.** Checked against the unit's exports first: `SqlReadError`
  already has `Sql` and `Decode`, and a bare variant shared by two enums of one
  unit resolves silently to one of them, so `Sql` and `Decode` were not
  available. The `Pg` prefix is H1's convention. `PgStatement` and
  `PgReadError` are stable records of an experimental capability.
- **C. DECISION A of #3320: the boundary codec does not read an `Instant` from
  ISO text, so the rule is added on the `Postgres` path only.** Confirmed in the
  generated codec: an `Instant` field is read from a JSON number that is a safe
  integer (whole epoch milliseconds, ADR 0114), and the generated reader rejects
  a string. `Instant.fromIso` exists but is a function, not a codec rule. Adding
  an ISO rule to the codec would change the wire format of every `Instant` in
  every `Json.decode`, HTTP body and event, which is far wider than this slice.
  So a typed `Postgres` read converts the text before the ordinary codec runs, in
  a runtime helper (`__bynkPgRow`), at exactly the `Instant` and `Int` fields of
  the row (directly, through an `Option`, or under a refined or opaque type). The
  helper is imported only by a module that has such a read. The forms read are
  the ones a `timestamptz` renders as: the driver's `toISOString()`, and
  Postgres' own text (a space for the `T`; a fraction of up to six digits,
  truncated toward the earlier millisecond, as `Instant.fromIso` does; an offset
  that may be `+00`, `+05:30`, `-0800` or, for a zone with a historic offset,
  `+05:53:28`), each normalised to UTC. A text with no offset (a `timestamp`
  names no instant), `infinity`, a `BC` year, an expanded year and anything else
  is left as text for the codec to refuse, so the failure is a `PgDecode` naming
  the field. Each form has a runtime test.
- **D. An `int8` is read at an `Int` field when it is a whole number within
  ±(2^53 − 1).** ADR 0799 F has `int8` arrive as a string so nothing past 2^53 is
  lost, which means the codec's `Int` rule (a JSON number) would refuse every
  `bigint` column, including `1`. The same helper converts the text of such a
  column to a number; one past the safe range is left as text, so the decode
  fails rather than rounding. A `numeric` column is read as a `Decimal` or a
  `String` field, which the codec already reads from text. `Float` is not
  converted from text: a `float8` arrives as a number, and reading a `numeric`
  as a `Float` would be a silent loss the user should write out.
- **E. DECISION B of #3320: `decodeSql` is not unified with the codec now,
  because it would change D1's behaviour for no gain here.** Two SQL front ends
  now exist, and both are the same shape: parse the text to the JSON tree,
  apply a pass over the row's fields of one base type, then run the one
  generated codec. `decodeSql`'s pass turns `0` and `1` into `Bool`; the
  `Postgres` pass turns text into `Int` and `Instant`. They are separate
  because their rules are separate: `Json.decodeSql` is a D1 rule (SQLite has no
  boolean), a `Postgres` `boolean` arrives as `true` or `false`, and a `Postgres`
  `0` at a `Bool` field should stay a decode error. Folding the `0`/`1` rule into
  the codec itself would make `Json.decode` accept `0` for a `Bool` everywhere,
  which changes the wire format and D1's goldens beyond a refactor; folding the
  text rules in would do the same for `Instant` and `Int`. What is shared is
  already shared: `DecodeSource` selects the pass, the field walk is one function
  (`row_base_fields`) that `row_bool_fields` now calls, and the decoder wrapper
  is common. Revisit if a third dialect (`MySql`, H5) needs a pass of its own, at
  which point the passes can become one table of rules keyed by dialect, still
  outside the codec.
- **F. `transaction(statements: List[PgStatement]) -> Effect[Result[List[PgRun],
  PgError]]` is a declared operation, run by `sql.begin`.** `PgStatement` is
  `{ sql, params }` and is held to `SqlStatement`'s rules: its `sql` is a literal
  (`bynk.types.sql_not_literal`), and it is non-storable and non-boundary
  (`bynk.types.sql_statement_at_boundary`), so it stays where it is built. The
  checks are the same code, now keyed on either type's name, and the diagnostic
  names the statement type it found. The binding opens one connection, runs the
  statements in order inside `sql.begin`, and ends the client whether the
  transaction committed or not; the first failure throws, which rolls back every
  earlier statement, stops the rest and is returned as that statement's
  `PgError` (with the SQLSTATE class mapping H1 settled). Every statement is
  passed `{ prepare: true }` (ADR 0799 A); the runtime's guard test now fails if
  either the single-statement call or the transaction's own `unsafe` omits it.
  An empty list is `Ok([])` and opens no connection. The executor the compose
  passes is now a function with a `transaction` member, so the compose and the
  binding's constructor are unchanged.
- **G. Test tiers.** A typed read lowers over `all` and `first`, so stubbing
  those stubs it (the rows as JSON text); `transaction` faults until stubbed,
  like the other operations.

**Consequences.** No grammar change. A `Postgres` read no longer repeats its
column list, and a `bigint` and a `timestamptz` read into the obvious fields.
`Json.decode`, `Json.decodeSql` and every D1 golden are unchanged; the shared
`cloudflare*.ts` goldens move because the binding carries `transaction`. An
atomic multi-statement write exists; a read that decides a later write still
needs an interactive transaction, which is out of scope. The diagnostics
`bynk.types.sql_not_literal`, `sql_row_shape`, `select_tail_has_select` and
`sql_statement_at_boundary` now apply to `Postgres` too, and their explanations
say so. A `Postgres` `boolean` is read as `Bool` as it is, and the typed reads
are wired for a `Postgres` instance in the same context, as D1's are.
