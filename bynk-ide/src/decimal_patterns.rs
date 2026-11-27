//! The hover of a `Decimal` literal pattern (#3973): it matches by value at any scale.
//!
//! A literal is a pattern, not an expression, when an arm arrow `=>` or an or-pattern `|` follows
//! it: neither can follow a literal in an expression. A literal nested in a payload
//! (`Some(1.25d)`) is not recognised here, since a `)` or `,` follows it in expressions too.

use bynk_syntax::lexer::{TokenKind, tokenize};

/// What the hover of a `Decimal` literal pattern says.
pub const DECIMAL_PATTERN_DOC: &str = "A `Decimal` literal pattern matches by value at any scale: `20d`, `20.0d` and `20.00d` match the same values. Like a `String` literal it never completes the type, so the `match` needs a `_` arm or a name.";

/// The hover for the `Decimal` literal at `offset`, when it is a pattern.
pub fn describe_at(text: &str, offset: usize) -> Option<String> {
    let tokens = tokenize(text).ok()?;
    let i = tokens.iter().position(|t| {
        t.kind == TokenKind::DecimalLit && t.span.start <= offset && offset <= t.span.end
    })?;
    matches!(
        tokens.get(i + 1).map(|t| t.kind),
        Some(TokenKind::FatArrow | TokenKind::Pipe)
    )
    .then(|| DECIMAL_PATTERN_DOC.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arm_literal_says_it_matches_by_value() {
        let src = "fn f(d: Decimal) -> Int {\n  match d {\n    20.0d => 1\n    5d | 6d => 2\n    _ => 3\n  }\n}\n";
        for needle in ["20.0d", "5d", "6d"] {
            let at = src.find(needle).unwrap() + 1;
            assert_eq!(describe_at(src, at).as_deref(), Some(DECIMAL_PATTERN_DOC));
        }
    }

    #[test]
    fn a_literal_in_an_expression_gets_nothing() {
        let src = "fn f(d: Decimal) -> Bool {\n  d == 20.0d\n}\n";
        let at = src.find("20.0d").unwrap() + 1;
        assert_eq!(describe_at(src, at), None);
    }
}
