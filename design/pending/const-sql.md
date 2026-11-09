---
level: minor
changelog: A `String` constant bound to a literal is accepted as a database call's SQL
probes: diagnostic_coverage, emit_diagnostics, ts_writes
---

## ADR: const-string-as-sql
title: A String constant bound to a literal counts as a literal for the SQL rules
summary: Relaxes bynk.types.sql_not_literal for compile-time constants only

**Context.** ADR 0666 B made a `D1` call's SQL a string literal, ruling out run-time assembly and so injection. It also ruled out a `const` string, whose value is just as fixed, so a query used at several calls had to be pasted at each (#3065; bynk-lang/github-hooks#73 inlined one query up to eight times in a suite).

**Decision.**

- **A. What counts.** A bare reference to a `String` constant whose value, following constant-to-constant references, is a plain string literal (a `"""` block included). Constants imported from a commons or a sibling file are in scope by the usual rule. Rejected: concatenation or interpolation of constants (needs constant folding, and neither is a valid constant initialiser); any `String`-typed expression.
- **B. Soundness.** A constant's initialiser is the closed static set (`bynk.resolve.bad_const_initialiser`): no operators, calls or interpolation, and cycles are refused. So the reference resolves at check time to the literal text in the source, and the checker judges the exact bytes the call sends. A local binding that shadows a constant name, a parameter, a field or a `let` is not a constant and stays refused. The declared type must be exactly `String`.
- **C. Where.** One helper (`sql_literal_text`) serves `all`/`first`/`run`, the `select[T]`/`firstAs[T]` tail (whose `FROM`/no-`SELECT` shape rules apply to the resolved text), and `SqlStatement { sql }` including a spread override.
- **D. No new diagnostic.** `bynk.types.sql_not_literal` keeps its code; its message and note mention the constant form.

**Consequences.** A query is declared once. No grammar, emitter or runtime change: the argument is an ordinary constant read, and a typed read's composed SQL is substituted at compile time as for an inline literal. Hover, completion, semantic tokens and signature help are unchanged, because the argument is an ordinary constant reference.
