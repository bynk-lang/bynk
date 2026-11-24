//! #3981: the names a statement-form `expect … is P(x)` binds stay in scope to the end of the
//! block. The statement throws when the predicate fails, so after it the case knows the subject
//! matched, and the bindings are declared right after the guard as an `if` declares them at the
//! top of its then-branch (`gather_is_bindings_for_emit`, the same rule the checker scopes by).

use bynk_syntax::ast::Expr;

use super::super::lower_ctx::LowerCtx;
use super::is::gather_is_bindings_for_statement;

/// The `const name = …;` declarations for the `is` tests `predicate` proves, for the statements
/// after an `expect predicate`. Empty when it binds nothing.
pub(in crate::emitter) fn declarations(
    predicate: &Expr,
    cx: &mut LowerCtx,
) -> Vec<bynk_ts::TsStmt> {
    let mut out = Vec::new();
    let mut found = false;
    gather_is_bindings_for_statement(predicate, cx, &mut out, &mut found);
    out
}
