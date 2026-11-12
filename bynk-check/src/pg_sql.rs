//! Hyperdrive H4 (#3321, ADR 0799 G): a literal `Postgres` statement's SQL
//! checked against Postgres' own parser, when the project supplies a
//! `schema.sql`.
//!
//! Bynk does not own the Postgres schema, so there is nothing to prepare a
//! statement against, as `bynk.d1.sql_invalid` does for D1 (ADR 0763). What can
//! be checked without one is the text:
//!
//! - `bynk.postgres.sql_invalid` (a **warning**): the statement does not parse
//!   with `sqlparser`'s Postgres dialect, or holds more than one command (a
//!   prepared statement takes one, so the driver would refuse it);
//! - `bynk.postgres.sql_param_count` (an error): the statement's highest `$n`
//!   differs from the number of parameters the call passes.
//!
//! `sqlparser` is a pure-Rust parser and is not Postgres' own, so it can refuse
//! valid Postgres (a newer or rarer form). That is why the syntax finding is a
//! warning: an advisory check must never block a program that runs. The
//! parameter count is exact (it is a scanner of this file's own, below, pinned
//! by tests to Postgres' rules), so a mismatch is an error. `libpg_query`
//! (`pg_query`) would be exact on syntax too, but needs libclang to build.
//!
//! The check is off unless `schema.sql` is at the project root: its presence is
//! the project saying it keeps a Postgres schema Bynk may take an interest in.
//! The parser has no C, so it builds on every target, `wasm32` included.

/// What is wrong with a statement's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PgSqlProblem {
    /// The statement does not parse, or is not one command. Carries the
    /// parser's own words.
    Invalid(String),
    /// The statement uses `expected` parameters and the call passes `passed`.
    ParamCount { expected: usize, passed: usize },
}

/// The problems with the literal statement `sql`, given the number of
/// parameters the call passes when the list is written out (`None` when it is
/// not, which leaves the count unchecked).
pub fn check_statement(sql: &str, passed: Option<usize>) -> Vec<PgSqlProblem> {
    let mut out = Vec::new();
    if let Some(message) = syntax_problem(sql) {
        out.push(PgSqlProblem::Invalid(message));
    }
    if let Some(passed) = passed {
        let expected = placeholder_count(sql);
        if expected != passed {
            out.push(PgSqlProblem::ParamCount { expected, passed });
        }
    }
    out
}

fn syntax_problem(sql: &str) -> Option<String> {
    use sqlparser::dialect::PostgreSqlDialect;
    use sqlparser::parser::Parser;
    match Parser::parse_sql(&PostgreSqlDialect {}, sql) {
        Err(e) => Some(parse_message(&e.to_string())),
        Ok(statements) if statements.len() > 1 => Some(
            "a statement is one command: a prepared statement refuses several, so send each with its own call (or use `transaction`)"
                .to_string(),
        ),
        Ok(_) => None,
    }
}

/// The parser's message, without the prefix it puts in front of it.
fn parse_message(text: &str) -> String {
    text.strip_prefix("sql parser error: ")
        .unwrap_or(text)
        .trim()
        .to_string()
}

/// The number of parameters a statement uses: its highest `$n`, which is how
/// many Postgres asks the call for. A `$n` inside a string literal (`'$9'`,
/// `E'…'`), a quoted identifier, a comment or a dollar-quoted body is text, not a
/// parameter.
pub fn placeholder_count(sql: &str) -> usize {
    let b = sql.as_bytes();
    let mut highest = 0usize;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' => i = skip_string(b, i, is_escape_string(b, i)),
            b'"' => i = skip_quoted(b, i, b'"'),
            b'-' if b.get(i + 1) == Some(&b'-') => {
                i = b[i..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(b.len(), |n| i + n);
            }
            b'/' if b.get(i + 1) == Some(&b'*') => i = skip_block_comment(b, i),
            // A `$` after an identifier byte is part of the name (`price$1`), not a parameter.
            b'$' if i > 0 && is_ident_byte(b[i - 1]) => i += 1,
            b'$' => {
                let digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
                if digits > 0 {
                    // Saturating: a number past `usize` is far past any list.
                    let n = sql[i + 1..i + 1 + digits]
                        .parse::<usize>()
                        .unwrap_or(usize::MAX);
                    highest = highest.max(n);
                    i += 1 + digits;
                } else if let Some(end) = dollar_quote_end(b, i) {
                    i = end;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    highest
}

/// Whether `c` can be part of an identifier after its first character: Postgres
/// allows letters, digits, `_`, `$` and any non-ASCII byte.
fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80
}

/// Whether the `'` at `i` opens an escape string (`E'…'`), where a backslash
/// escapes the next character. The `E` must not be the tail of a longer name.
fn is_escape_string(b: &[u8], i: usize) -> bool {
    i > 0
        && matches!(b[i - 1], b'e' | b'E')
        && (i == 1 || !(b[i - 2].is_ascii_alphanumeric() || b[i - 2] == b'_' || b[i - 2] == b'$'))
}

/// The index after the string literal opening at `i`. A doubled `''` is a quote
/// in it; in an escape string so is a backslash-quote.
fn skip_string(b: &[u8], i: usize, escapes: bool) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' if escapes => j += 2,
            b'\'' if b.get(j + 1) == Some(&b'\'') => j += 2,
            b'\'' => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

/// The index after the quoted identifier opening at `i` (`""` is a quote in it).
fn skip_quoted(b: &[u8], i: usize, quote: u8) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        if b[j] == quote {
            if b.get(j + 1) == Some(&quote) {
                j += 2;
                continue;
            }
            return j + 1;
        }
        j += 1;
    }
    b.len()
}

/// The index after the block comment opening at `i`. They nest in Postgres.
fn skip_block_comment(b: &[u8], i: usize) -> usize {
    let (mut depth, mut j) = (1usize, i + 2);
    while j < b.len() && depth > 0 {
        if b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
            depth += 1;
            j += 2;
        } else if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
            depth -= 1;
            j += 2;
        } else {
            j += 1;
        }
    }
    j
}

/// The index after the dollar-quoted body opening at `i` (`$$…$$`, `$tag$…$tag$`),
/// or `None` when the `$` opens none. A tag is an identifier that does not start
/// with a digit, so `$1` is a parameter, never a tag.
fn dollar_quote_end(b: &[u8], i: usize) -> Option<usize> {
    let tag_len = b[i + 1..]
        .iter()
        .enumerate()
        .take_while(|(k, c)| {
            c.is_ascii_alphabetic() || **c == b'_' || (*k > 0 && c.is_ascii_digit())
        })
        .count();
    if b.get(i + 1 + tag_len) != Some(&b'$') {
        return None;
    }
    let tag = &b[i..i + tag_len + 2];
    let body = i + tag.len();
    Some(
        b[body..]
            .windows(tag.len())
            .position(|w| w == tag)
            .map_or(b.len(), |n| body + n + tag.len()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The statements the scanner is held to: the highest `$n`, with text that
    /// only looks like a parameter.
    const COUNTS: &[(&str, usize)] = &[
        ("select 1", 0),
        ("select $1", 1),
        ("select $1, $2, $3", 3),
        ("select $2, $1", 2),
        ("select * from t where a = $1 and b = $1", 1),
        ("select '$9'", 0),
        ("select E'it\\'s $9', $1", 1),
        ("select 'it''s $9', $2, $1", 2),
        ("select \"$9\" from t where a = $1", 1),
        ("select 1 -- $9\n, $1", 1),
        ("select price$1 from t where a = $2", 2),
        ("select a$tag$ from t where b = $1", 1),
        ("select a$1, $2 from t", 2),
        ("select /* $9 /* nested $8 */ $7 */ $1", 1),
        ("select $$ $9 $$, $1", 1),
        ("select $tag$ $9 $tag$, $1, $2", 2),
        ("select $1::int + $10::int", 10),
        ("insert into t (a, b) values ($1, $2) returning id", 2),
        ("select array[$1, $2]", 2),
        ("select x ilike '%' || $2 || '%' from t", 2),
    ];

    #[test]
    fn the_scanner_counts_by_postgres_rules() {
        for (sql, n) in COUNTS {
            assert_eq!(placeholder_count(sql), *n, "{sql}");
        }
    }

    /// Valid Postgres, in the forms the docs and the tests of this repository
    /// use: none of them may be reported.
    pub(crate) const VALID_POSTGRES: &[&str] = &[
        "select id, data->>'name' as name, data->'tags'->0 from events where data @> $1::jsonb",
        "insert into orders (name, qty) values ($1, $2) on conflict (name) do update set qty = orders.qty + excluded.qty returning id, qty",
        "insert into orders (name) values ($1) on conflict do nothing",
        "update orders set qty = qty - 1 where id = $1 returning *",
        "delete from orders where id = any($1::int8[])",
        "select $1::timestamptz, $2::numeric(12, 2), $3::text[]",
        "with recent as (select * from orders where placed > $1) select count(*) from recent",
        "select distinct on (name) name, qty from orders order by name, qty desc",
        "select id from orders where name ilike $1 order by id limit $2 offset $3",
        "select id, row_number() over (partition by name order by id) from orders",
        "select * from orders where id = $1 for update skip locked",
        "select generate_series(1, $1)",
        "insert into archive select * from orders where placed < $1",
        "select * from orders o join items i on i.order_id = o.id where o.id = $1",
        "create table if not exists t (id serial primary key, doc jsonb not null default '{}'::jsonb, at timestamptz default now())",
    ];

    #[test]
    fn valid_postgres_has_no_problem() {
        for sql in VALID_POSTGRES {
            assert_eq!(check_statement(sql, None), vec![], "{sql}");
        }
    }

    #[test]
    fn a_valid_statement_with_the_right_count_has_no_problem() {
        assert_eq!(check_statement("select $1, $2", Some(2)), vec![]);
        assert_eq!(check_statement("select 1", Some(0)), vec![]);
        // A list that is not written out leaves the count unchecked.
        assert_eq!(check_statement("select $1, $2", None), vec![]);
    }

    #[test]
    fn a_syntax_error_is_invalid() {
        let got = check_statement("selec 1", Some(0));
        assert!(
            matches!(got.as_slice(), [PgSqlProblem::Invalid(_)]),
            "{got:?}"
        );
    }

    #[test]
    fn several_commands_are_invalid() {
        let got = check_statement("select 1; select 2", Some(0));
        assert!(
            matches!(got.as_slice(), [PgSqlProblem::Invalid(m)] if m.contains("one command")),
            "{got:?}"
        );
    }

    #[test]
    fn a_wrong_count_is_reported_both_ways() {
        assert_eq!(
            check_statement("select $1, $2", Some(1)),
            vec![PgSqlProblem::ParamCount {
                expected: 2,
                passed: 1
            }]
        );
        assert_eq!(
            check_statement("select $1", Some(3)),
            vec![PgSqlProblem::ParamCount {
                expected: 1,
                passed: 3
            }]
        );
    }
}
