---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "`D1` gains a typed `select[T]` and `firstAs[T]`: the compiler writes the `SELECT` column list from the row type, and failures are a `SqlReadError`"
---

## ADR: d1-typed-select
title: D1 typed reads write the column list from the row type
summary: select[T]/firstAs[T] on a D1 instance, lowered over all/first and the decodeSql codec

**Context.** A `D1` query returning a record spells the record's fields twice, in the literal `SELECT` list and in the type `Json.decodeSql[T]` decodes into. A helper that shares the column list is impossible by design, because the SQL must be a literal (`bynk.types.sql_not_literal`), so the duplication drifts silently into run-time decode failures (#2865).

**Decision.** A derived `D1` instance has two compiler-lowered operations, `select[T](tail, params)` returning `Result[List[T], SqlReadError]` and `firstAs[T](tail, params)` returning `Result[Option[T], SqlReadError]`. (A) The tail is a string literal that starts at `FROM` and must not begin with `SELECT` (`bynk.types.select_tail_has_select`); the checker writes `SELECT` and the quoted field names of `T` ahead of it at compile time, so the statement stays a constant. (B) There is no column renaming: a field selects the column of its own name. (C) `SqlReadError = Sql(error: SqlError) | Decode(error: JsonError)`. (D) `T` must satisfy `Json.decodeSql`'s `sql_row_shape` rule, checked at the call; `T` is the row itself, so a `List` is refused. They are not declared operations of `capability D1`: the checker records a `Callee::D1Select` and the emitter lowers the call to the existing `all`/`first` followed by the `decodeSql` codec, so `WorkersD1` and every D1 double are unchanged and no runtime operation is added.

**Consequences.** No grammar or runtime change. Adding a column means editing the record, the `INSERT` and the migration, not each `SELECT`. A typed read is decoded through the same codec as `Json.decodeSql`, so the external-opaque rule (#2794) applies to it too. Completion, signature help and hover know the two operations by name for a `D1`-derived capability.
