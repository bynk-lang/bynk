//! Hyperdrive H5 (#3322): a literal `MySql` statement's SQL checked against a
//! MySQL parser, when the project supplies a `schema.sql`. It is H4's check
//! (see [`crate::pg_sql`]) with `sqlparser`'s `MySqlDialect`:
//!
//! - `bynk.mysql.sql_invalid` (a **warning**): the statement does not parse, or
//!   holds more than one command (the driver is not configured for several, so
//!   it would refuse it);
//! - `bynk.mysql.sql_param_count` (an error): the number of `?` placeholders
//!   differs from the number of parameters the call passes.
//!
//! `sqlparser` is not MySQL's own parser, so the syntax finding is a warning,
//! for the reason H4 gives: an advisory check must never block a program that
//! runs. The placeholder count follows the driver, not the SQL standard, since
//! what matters is how many values mysql2's `query` will consume: a `?` is a
//! parameter unless it is inside a single-quoted string (where a backslash
//! escapes and a doubled quote is a quote), a backtick name, or a comment
//! (`-- ` and `/* … */`). mysql2 (through `sql-escaper`) does **not** treat a
//! double-quoted string or a `#` comment as quoting, so a `?` in either is
//! counted, as it is substituted; write string literals with single quotes.

use crate::pg_sql::PgSqlProblem;

/// The problems with the literal statement `sql`, given the number of
/// parameters the call passes when the list is written out (`None` leaves the
/// count unchecked).
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
    use sqlparser::dialect::MySqlDialect;
    use sqlparser::parser::Parser;
    match Parser::parse_sql(&MySqlDialect {}, sql) {
        Err(e) => Some(
            e.to_string()
                .strip_prefix("sql parser error: ")
                .map(str::to_string)
                .unwrap_or_else(|| e.to_string())
                .trim()
                .to_string(),
        ),
        Ok(statements) if statements.len() > 1 => Some(
            "a statement is one command: the driver refuses several, so send each with its own call (or use `transaction`)"
                .to_string(),
        ),
        Ok(_) => None,
    }
}

/// The number of `?` placeholders a statement uses, by MySQL's lexical rules.
pub fn placeholder_count(sql: &str) -> usize {
    let b = sql.as_bytes();
    let (mut count, mut i) = (0usize, 0usize);
    while i < b.len() {
        match b[i] {
            q @ (b'\'' | b'`') => i = skip_quoted(b, i, q),
            // `--` is a comment only when whitespace (or the end) follows.
            b'-' if b.get(i + 1) == Some(&b'-')
                && b.get(i + 2)
                    .is_none_or(|c| c.is_ascii_whitespace() || c.is_ascii_control()) =>
            {
                i = line_end(b, i)
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = sql[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
            }
            b'?' => {
                count += 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    count
}

fn line_end(b: &[u8], i: usize) -> usize {
    b[i..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |n| i + n)
}

/// The index after the quoted run opening at `i`. A doubled quote is a quote in
/// it; in a string (`'`) a backslash escapes the next byte.
fn skip_quoted(b: &[u8], i: usize, quote: u8) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' if quote != b'`' => j += 2,
            c if c == quote && b.get(j + 1) == Some(&quote) => j += 2,
            c if c == quote => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const COUNTS: &[(&str, usize)] = &[
        ("select 1", 0),
        ("select ?", 1),
        ("select ?, ?, ?", 3),
        ("select * from t where a = ? and b = ?", 2),
        ("select '?' from t where a = ?", 1),
        ("select 'it''s ?', ?", 1),
        ("select 'it\\'s ?', ?", 1),
        // mysql2 substitutes inside a double-quoted string and after `#`.
        ("select \"?\" from t where a = ?", 2),
        ("select 1 # ?\n, ?", 2),
        ("select `a?b` from t where a = ?", 1),
        ("select 1 -- ?\n, ?", 1),
        ("select /* ? */ ?", 1),
        ("select 5--?", 1),
        ("select * from t limit ?, ?", 2),
        (
            "insert into t (a, b) values (?, ?) on duplicate key update b = values(b)",
            2,
        ),
    ];

    #[test]
    fn the_scanner_counts_by_mysql_rules() {
        for (sql, n) in COUNTS {
            assert_eq!(placeholder_count(sql), *n, "{sql}");
        }
    }

    /// Valid MySQL, in the forms its users write: none may be reported.
    pub(crate) const VALID_MYSQL: &[&str] = &[
        "insert into orders (name, qty) values (?, ?) on duplicate key update qty = qty + values(qty)",
        "insert ignore into orders (name) values (?)",
        "select `id`, `name` from `orders` where `name` like ? limit ?, ?",
        "update orders set qty = qty - 1 where id = ? limit 1",
        "delete from orders where id in (select id from old_orders where placed < ?)",
        "select id, json_extract(doc, '$.name') from events where json_contains(doc, ?)",
        "with recent as (select * from orders where placed > ?) select count(*) from recent",
        "select * from orders o join items i on i.order_id = o.id where o.id = ? for update",
        "create table if not exists t (id bigint unsigned auto_increment primary key, flag tinyint(1) not null default 0, at datetime(3) default current_timestamp(3))",
    ];

    #[test]
    fn valid_mysql_has_no_problem() {
        for sql in VALID_MYSQL {
            assert_eq!(check_statement(sql, None), vec![], "{sql}");
        }
    }

    #[test]
    fn a_syntax_error_is_invalid_and_a_wrong_count_is_reported() {
        let got = check_statement("selec 1", Some(0));
        assert!(
            matches!(got.as_slice(), [PgSqlProblem::Invalid(_)]),
            "{got:?}"
        );
        assert_eq!(
            check_statement("select ?, ?", Some(1)),
            vec![PgSqlProblem::ParamCount {
                expected: 2,
                passed: 1
            }]
        );
        assert_eq!(check_statement("select ?, ?", Some(2)), vec![]);
        assert_eq!(check_statement("select ?, ?", None), vec![]);
        let several = check_statement("select 1; select 2", Some(0));
        assert!(
            matches!(several.as_slice(), [PgSqlProblem::Invalid(m)] if m.contains("one command")),
            "{several:?}"
        );
    }
}
