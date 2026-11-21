//! Whether an expression awaits (1A.21, #3722, Decision C): the structural form of the
//! `v.contains("await")` text inspection `Json.encodePlain` made of its lowered argument. It
//! looks inside nested arrows and object methods too, as the text search did, so an argument that
//! awaits anywhere is hoisted ahead of the flag it must not run under.

use crate::program::{TsArrowBody, TsExpr, TsObjectEntry, TsStmt, TsStmtKind};

/// Whether `expr` contains an `await`, at any depth. A [`TsExpr::VerbatimExpr`] is text, so it is
/// searched as text.
pub fn expr_contains_await(expr: &TsExpr) -> bool {
    match expr {
        TsExpr::Await(_) => true,
        TsExpr::Lit(_) => false,
        // An `Ident` or a `VerbatimExpr` may carry expression text (a still-text operand wrapped for
        // a node-taking caller), so both are searched as text: a false positive only hoists.
        TsExpr::Ident(text) | TsExpr::VerbatimExpr(text, _) => text.contains("await"),
        TsExpr::Member { object, .. } | TsExpr::OptionalMember { object, .. } => {
            expr_contains_await(object)
        }
        TsExpr::TypeApp { callee, .. } => expr_contains_await(callee),
        TsExpr::Index { object, index } | TsExpr::OptionalIndex { object, index } => {
            expr_contains_await(object) || expr_contains_await(index)
        }
        TsExpr::DefaultIife { default, body, .. } => {
            expr_contains_await(default) || body.iter().any(stmt_contains_await)
        }
        TsExpr::Arrow { body, .. } => match &**body {
            TsArrowBody::Expr(e) => expr_contains_await(e),
            TsArrowBody::Block(stmts) | TsArrowBody::Multiline(stmts) => {
                stmts.iter().any(stmt_contains_await)
            }
        },
        TsExpr::Call { callee, args }
        | TsExpr::New { callee, args }
        | TsExpr::OptionalCall { callee, args } => {
            expr_contains_await(callee) || args.iter().any(expr_contains_await)
        }
        TsExpr::Sequence(items) => items.iter().any(expr_contains_await),
        TsExpr::Function { body, .. } => body.iter().any(stmt_contains_await),
        TsExpr::Object { entries, .. } => entries.iter().any(|e| match e {
            TsObjectEntry::Prop { value: v, .. } | TsObjectEntry::Spread(v) => {
                expr_contains_await(v)
            }
            TsObjectEntry::Shorthand(_) => false,
            TsObjectEntry::Method { body, .. } => body.iter().any(stmt_contains_await),
        }),
        TsExpr::Array { items, .. } | TsExpr::LinesArray(items) => {
            items.iter().any(expr_contains_await)
        }
        TsExpr::TemplateLit { exprs, .. } => exprs.iter().any(expr_contains_await),
        TsExpr::As { expr, .. } | TsExpr::Unary { expr, .. } => expr_contains_await(expr),
        TsExpr::Paren(e)
        | TsExpr::Spread(e)
        | TsExpr::NonNull(e)
        | TsExpr::AsConst(e)
        | TsExpr::Yield(e) => expr_contains_await(e),
        TsExpr::Binary { left, right, .. } => {
            expr_contains_await(left) || expr_contains_await(right)
        }
        TsExpr::Conditional {
            test,
            consequent,
            alternate,
        }
        | TsExpr::MultilineConditional {
            test,
            consequent,
            alternate,
        } => {
            expr_contains_await(test)
                || expr_contains_await(consequent)
                || expr_contains_await(alternate)
        }
        TsExpr::Assign { target, value } | TsExpr::AddAssign { target, value } => {
            expr_contains_await(target) || expr_contains_await(value)
        }
    }
}

/// Whether `stmt` contains an `await` in an expression of its own or of a nested statement.
fn stmt_contains_await(stmt: &TsStmt) -> bool {
    match &stmt.kind {
        TsStmtKind::Const { init, .. } => expr_contains_await(init),
        TsStmtKind::Let { init, .. } => init.as_ref().is_some_and(expr_contains_await),
        TsStmtKind::ExprStmt(e) | TsStmtKind::Throw(e) | TsStmtKind::Increment(e) => {
            expr_contains_await(e)
        }
        TsStmtKind::Return(e) => e.as_ref().is_some_and(expr_contains_await),
        TsStmtKind::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            expr_contains_await(cond)
                || stmt_contains_await(then_branch)
                || else_branch.as_deref().is_some_and(stmt_contains_await)
        }
        // A `for await` awaits each step, whatever its body does.
        TsStmtKind::ForOf {
            iter,
            body,
            is_await,
            ..
        } => *is_await || expr_contains_await(iter) || stmt_contains_await(body),
        TsStmtKind::While { cond, body } => expr_contains_await(cond) || stmt_contains_await(body),
        TsStmtKind::Labelled { body, .. } => stmt_contains_await(body),
        TsStmtKind::LetList(decls) => decls
            .iter()
            .any(|d| d.init.as_ref().is_some_and(expr_contains_await)),
        TsStmtKind::For {
            init, test, body, ..
        } => expr_contains_await(init) || expr_contains_await(test) || stmt_contains_await(body),
        TsStmtKind::TryCatch {
            try_block,
            catch_block,
            finally_block,
            ..
        } => {
            stmt_contains_await(try_block)
                || catch_block.as_deref().is_some_and(stmt_contains_await)
                || finally_block.as_deref().is_some_and(stmt_contains_await)
        }
        TsStmtKind::Block(stmts) | TsStmtKind::InlineBlock(stmts) => {
            stmts.iter().any(stmt_contains_await)
        }
        TsStmtKind::Assign { target, value } => {
            expr_contains_await(target) || expr_contains_await(value)
        }
        TsStmtKind::Switch {
            discriminant,
            cases,
        } => {
            expr_contains_await(discriminant)
                || cases.iter().any(|c| {
                    c.labels.iter().any(expr_contains_await)
                        || c.body.iter().any(stmt_contains_await)
                })
        }
        TsStmtKind::Verbatim { text, .. } | TsStmtKind::Raw(text) => text.contains("await"),
        TsStmtKind::Decl(_)
        | TsStmtKind::Continue(_)
        | TsStmtKind::Break(_)
        | TsStmtKind::Comment(_)
        | TsStmtKind::DocComment(_)
        | TsStmtKind::Blank => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{TsBinaryOp, TsLit, TsParam, TsStmt};

    fn id(n: &str) -> TsExpr {
        TsExpr::Ident(n.to_string())
    }

    fn awaiting() -> TsExpr {
        TsExpr::Await(Box::new(TsExpr::Call {
            callee: Box::new(id("f")),
            args: vec![],
        }))
    }

    #[test]
    fn a_plain_expression_does_not_await() {
        let e = TsExpr::Binary {
            op: TsBinaryOp::Add,
            left: Box::new(id("a")),
            right: Box::new(TsExpr::Lit(TsLit::Num("1".to_string()))),
        };
        assert!(!expr_contains_await(&e));
    }

    /// Text in an `Ident` is searched as text, as a `VerbatimExpr` is.
    #[test]
    fn an_await_hidden_in_ident_text_is_found() {
        assert!(expr_contains_await(&id("(await f())")));
        assert!(!expr_contains_await(&id("plain")));
    }

    #[test]
    fn an_await_is_found_at_any_depth() {
        let nested = TsExpr::Call {
            callee: Box::new(id("g")),
            args: vec![TsExpr::Paren(Box::new(TsExpr::As {
                expr: Box::new(awaiting()),
                ty: crate::program::TsType::named("T"),
            }))],
        };
        assert!(expr_contains_await(&awaiting()));
        assert!(expr_contains_await(&nested));
    }

    #[test]
    fn an_await_inside_a_nested_arrow_body_is_found() {
        let arrow = TsExpr::Arrow {
            params: vec![TsParam::new("x".to_string(), None)],
            is_async: true,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Block(vec![TsStmt::return_stmt(
                Some(awaiting()),
                None,
            )])),
        };
        assert!(expr_contains_await(&arrow));
        let quiet = TsExpr::Arrow {
            params: Vec::new(),
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(id("x")))),
        };
        assert!(!expr_contains_await(&quiet));
    }
}
