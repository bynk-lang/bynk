//! #1663: diagnosing a file the strict parser rejects, for the CLI's
//! single-file check. The editor's `bynk_ide::diagnose` follows the same rules
//! (Decisions A and B) on its own recovering parse.

use bynk_syntax::CompileError;
use bynk_syntax::ast::SourceUnit;
use bynk_syntax::lexer::Token;
use bynk_syntax::parser;

use crate::{checker, resolver};

/// #1663: the diagnostics for a file the strict parser rejects with `strict` —
/// that error, every other syntax error from a recovering parse, then the
/// resolver's and checker's findings in the declarations that did parse. References to a declaration recovery had to
/// skip are known names, not unknown ones (Decision B), and the checker's
/// diagnostics in a declaration the resolver rejected are its echoes (Decision
/// A); both are left out.
pub fn diagnose_unparsable(
    tokens: &[Token],
    source: &str,
    strict: Vec<CompileError>,
) -> Vec<CompileError> {
    diagnose_unlexable(tokens, source, Vec::new(), strict)
}

/// [`diagnose_unparsable`] for a file the lexer recovered from (#2807): `tokens`
/// are the recovering lex's, `lex_errors` its errors. They come first in the
/// result, and the syntax errors they caused are left out.
pub fn diagnose_unlexable(
    tokens: &[Token],
    source: &str,
    lex_errors: Vec<CompileError>,
    strict: Vec<CompileError>,
) -> Vec<CompileError> {
    let parser::Recovered {
        units,
        errors,
        broken_decl_names,
    } = parser::parse_units_recovering(tokens, source);
    let (recovered, _warnings) = bynk_syntax::partition_by_severity(errors);
    // #2807: lexical errors left gaps the parser tripped over.
    let recovered = bynk_syntax::lexer::without_lex_echoes(&lex_errors, tokens, recovered);
    let mut strict = strict;
    strict.splice(0..0, lex_errors);
    let mut out = parser::merge_syntax_errors(strict, recovered);
    if let Some(SourceUnit::Commons(commons)) = units.into_iter().next() {
        let item_spans: Vec<_> = commons.items.iter().map(|i| i.span()).collect();
        let (resolved, resolve_errors) = resolver::resolve_recovering(commons);
        let (shown, _hidden) =
            resolver::split_broken_decl_echoes(resolve_errors.clone(), &broken_decl_names);
        out.extend(shown);
        if let Err(checked) = checker::check(resolved) {
            let (checked, _warnings) = bynk_syntax::partition_by_severity(checked);
            // #1710: the checker reports some echoes of a skipped declaration
            // under its own codes (a method, a capability, an actor).
            let checked = resolver::without_resolve_echoes(checked, &resolve_errors, &item_spans);
            out.extend(resolver::split_broken_decl_echoes(checked, &broken_decl_names).0);
        }
    }
    out
}
