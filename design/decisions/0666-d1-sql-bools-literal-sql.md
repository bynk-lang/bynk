# 0666 — D1 rows decode SQLite flags as Bool through Json.decodeSql, and D1 SQL is literal-only

- **Status:** Accepted (v0.452)

**Context.** ADR 0662 shipped `D1` with two gaps it routed out as language
work (ADR 0023), and ADR 0664 G kept `D1` experimental until both closed.

- SQLite has no boolean, so a flag column reaches the row JSON as `0` or `1`.
  The `Bool` codec accepts only `true`/`false`, so 0662 E had users declare a
  flag `Int`. The binding can't convert, because it doesn't know which integer
  column is a flag.
- The SQL text was any `String`, so `Orders.all("… WHERE id = '\(id)'", [])`
  compiled: SQL injection, beside a parameter mechanism (`SqlValue`) that made
  it unnecessary. K6c (#2600) had already made regex patterns literal-only.

The codecs are generated once per type and shared by every boundary that
decodes JSON: `Json.decode`, an HTTP request body, a service call, a queue
message, agent-store rehydration and a WebSocket frame.

**Decision.**

- **A. `Json.decodeSql[T](s) -> Result[T, JsonError]`,** a third `Json`
  static. It decodes as `Json.decode[T]` does, except that at each `Bool`
  field of the row (also under an `Option`, or as the base of a refined or
  opaque type) the numbers `0` and `1` decode as `false` and `true`. Any
  other value is left to the codec, so `2` or `"1"` still fails with
  `expected boolean`.
  - **`T` must be a row:** a non-generic record whose fields are `Int`,
    `Float`, `String`, `Bool` or `Bytes`, a refined or opaque type over one,
    or an `Option` of either; or a `List` of such records. Anything else is
    `bynk.types.sql_row_shape`, naming the field. A SQL column can't hold
    anything else, and the restriction means the `Bool` positions are known
    at compile time.
  - **It lowers to a pass before the ordinary codec.** After parsing, the
    wrapper rewrites the listed keys of the object (or of each object of the
    array), then calls the same `deserialise_T` as `Json.decode`. No codec
    changes, and nothing changes in a module that doesn't call `decodeSql`.
  - **Rejected: making the shared `Bool` codec accept `0`/`1`.** It would loosen
    every boundary at once: an HTTP client could send `"paid": 1`, the OpenAPI
    schema (`boolean`) would no longer describe what an endpoint accepts, and
    encode and decode would stop being inverses.
  - **Rejected: converting in the binding,** for 0662 E's reason. And a
    codec-carrying `Orders.all[Row](…)` stays D4's later Tier 2 upgrade.
- **B. A `D1` statement's SQL is a string literal.** The first argument of
  `all`, `first` and `run` on a derived instance of `bynk.cloudflare`'s `D1`
  must be a plain string literal (no interpolation). The check keys on the
  resolved base, so a user capability with an `all` operation is untouched.
  Anything else is `bynk.types.sql_not_literal`, whose note points at
  `?n` parameters.
  - **`batch` is checked where its statements are built.** Every
    `SqlStatement { … }` needs a literal `sql` (a shorthand `sql` reads a
    binding, so it isn't one), as does a spread that overrides `sql`. A
    spread that keeps `sql` copies a statement already built from a literal.
    A batch can still be built with `map`. **Rejected:** requiring a list
    literal argument, which would forbid that for no safety gain.
  - **Concatenation is refused,** as K6c's patterns are, since there's no
    constant folding. A long statement is one long literal until the language
    has multi-line string literals.
  - **No escape hatch** for dynamic table or column names. One can be proposed
    with its own review when there is demand.
- **C. `SqlStatement` never crosses a boundary.** A statement decoded from
  outside would carry SQL that was never a literal. As K4 does for crypto
  keys, a `SqlStatement` in a record or sum field, an event field, a service
  or agent handler signature, an agent key or store field, or as a
  `Json.encode`/`Json.decode`/`Json.decodeSql` target, is
  `bynk.types.sql_statement_at_boundary`. A capability operation signature is
  exempt, since `D1.batch` takes one, in-process. `SqlValue` stays codable:
  it's data, not code.

**Consequences.**

- Amends ADR 0662 E: a flag field is declared `Bool` and decoded with
  `Json.decodeSql`; declaring it `Int` still works. `Json.decode` is
  unchanged.
- SQL injection through `D1` is ruled out by construction. Data reaches a
  statement only as a bound `SqlValue`.
- Breaking for a program that built SQL at run time, or passed a
  `SqlStatement` across a boundary or kept one in a record. `D1` is
  experimental, so this needs no deprecation. Every in-tree call site was
  already literal.
- A user record can't hold a `SqlStatement`, even one used only in-process.
  The record-field rule is the boundary rule, as for crypto keys.
- With both gaps closed, ADR 0664 G's condition is met. Graduating `D1` to
  stable is a separate decision (P4d).
