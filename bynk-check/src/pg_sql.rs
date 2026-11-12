//! Hyperdrive H4 (#3321, ADR 0799 G): a literal `Postgres` statement's SQL
//! checked against Postgres' own parser, when the project supplies a
//! `schema.sql`.
//!
//! Bynk does not own the Postgres schema, so there is nothing to prepare a
//! statement against, as `bynk.d1.sql_invalid` does for D1 (ADR 0763). What can
//! be checked without one is the text:
//!
//! - `bynk.postgres.sql_invalid`: the statement does not parse (libpg_query, the
//!   parser Postgres itself is built from), or holds more than one command (a
//!   prepared statement takes exactly one, so the driver would refuse it);
//! - `bynk.postgres.sql_param_count`: the statement's highest `$n` differs from
//!   the number of parameters the call passes.
//!
//! The check is off unless `schema.sql` is at the project root: its presence is
//! the project saying it keeps a Postgres schema Bynk may take an interest in.
//!
//! The parameter count is [`placeholder_count`], a scanner of this file's own
//! (the `agrees_with_libpg_query` test pins it to the parser's `$n` nodes), so it
//! holds on every target. The syntax check needs libpg_query, which is C and
//! has no `wasm32-unknown-unknown` build, so on `wasm32` (the playground, which
//! has no `schema.sql` either) a statement is never reported as invalid.

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

#[cfg(not(target_arch = "wasm32"))]
fn syntax_problem(sql: &str) -> Option<String> {
    match pg_query::parse(sql, 0) {
        Err(e) => Some(parse_message(&e.to_string())),
        Ok(parsed) if parsed.protobuf.stmts.len() > 1 => Some(
            "a statement is one command: a prepared statement refuses several, so send each with its own call (or use `transaction`)"
                .to_string(),
        ),
        Ok(_) => None,
    }
}

/// libpg_query's message, without the prefix the crate puts in front of it.
#[cfg(not(target_arch = "wasm32"))]
fn parse_message(text: &str) -> String {
    let text = text
        .strip_prefix("Parse error: ")
        .or_else(|| text.strip_prefix("Invalid statement: "))
        .unwrap_or(text);
    text.trim().to_string()
}

#[cfg(target_arch = "wasm32")]
fn syntax_problem(_: &str) -> Option<String> {
    None
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    /// The statements the scanner is held to: parameters in every position and
    /// text that only looks like one.
    const STATEMENTS: &[&str] = &[
        "select 1",
        "select $1",
        "select $1, $2, $3",
        "select $2, $1",
        "select * from t where a = $1 and b = $1",
        "select '$9'",
        "select E'it\\'s $9', $1",
        "select 'it''s $9', $2, $1",
        "select \"$9\" from t where a = $1",
        "select 1 -- $9\n, $1",
        "select /* $9 /* nested $8 */ $7 */ $1",
        "select $$ $9 $$, $1",
        "select $tag$ $9 $tag$, $1, $2",
        "select $1::int + $10::int, $2, $3, $4, $5, $6, $7, $8, $9",
        "insert into t (a, b) values ($1, $2) returning id",
        "update t set a = $1 where id = $2",
        "select array[$1, $2]",
        "select $1 as e from t where x ilike '%' || $2 || '%'",
    ];

    /// The highest `$n` libpg_query's parse tree holds, from the `ParamRef`
    /// nodes in its debug rendering (its `nodes()` walk does not descend into
    /// every list, a `VALUES` row among them).
    fn parser_count(sql: &str) -> usize {
        let tree = format!("{:?}", pg_query::parse(sql, 0).expect("parses").protobuf);
        let mut highest = 0;
        let mut rest = tree.as_str();
        while let Some(at) = rest.find("ParamRef") {
            rest = &rest[at + "ParamRef".len()..];
            let Some(at) = rest.find("number: ") else {
                break;
            };
            let digits: String = rest[at + "number: ".len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            highest = highest.max(digits.parse().unwrap_or(0));
        }
        highest
    }

    #[test]
    fn agrees_with_libpg_query() {
        for sql in STATEMENTS {
            assert_eq!(placeholder_count(sql), parser_count(sql), "{sql}");
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
            matches!(got.as_slice(), [PgSqlProblem::Invalid(m)] if m.contains("syntax error")),
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
