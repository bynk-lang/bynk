//! The small TypeScript node builders, defined once (back-half paydown 1B.4a, #4051).
//!
//! Every function here is one `bynk_ts` constructor over its parameters: `ident` is
//! `TsExpr::Ident`, `member` is `TsExpr::Member`, `const_` is `TsStmt::const_stmt` with no type
//! and no span. Before 1B.4a each file spelled its own, in up to five names for one shape
//! (`str_lit`, `lit`, `lit_str`, `str_node`, `hist_str`), and one of them differed in behaviour
//! under the same name (the list kernel's `method_call` brackets its receiver; it is now
//! `lower_list::bracketed_method_call`). Every file now calls a builder by its name here (1B.4b),
//! and the `node_builder_defs` probe, which reads 0, guards against a local wrapper or an `as`
//! rename coming back.
//!
//! What does not belong here: a helper that composes constructors and says something about a
//! construct rather than about TypeScript (`if_err`, `applied`, `tag_const`, `verify_call`), and
//! the constant helpers (`undefined`, `any_ty`) that `emit_silent_fallbacks` reads by spelling.
//! They stay in their files, written over these builders. Where `bynk_ts` already has the
//! constructor (`TsExpr::array`, `TsExpr::object_entries`, `TsParam::new`), callers use
//! it directly. Statement builders pass `span: None`, as every local one did: a statement that
//! carries a source span is built with `bynk_ts` directly.

use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsExpr, TsLit, TsParam, TsStmt, TsSwitchCase, TsType,
    TsUnaryOp,
};

// ---- Literals and names ----

/// `name`.
pub(crate) fn ident(s: impl Into<String>) -> TsExpr {
    TsExpr::Ident(s.into())
}

/// `"text"`.
pub(crate) fn str_lit(s: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Str(s.into()))
}

/// A numeric literal from its digits (`0`, `1.5`); anything that prints as the digits will do
/// (`u32`, `&str`, `String`).
pub(crate) fn num_lit(digits: impl ToString) -> TsExpr {
    TsExpr::Lit(TsLit::Num(digits.to_string()))
}

/// `true` or `false`.
pub(crate) fn bool_lit(b: bool) -> TsExpr {
    TsExpr::Lit(TsLit::Bool(b))
}

/// `null`.
pub(crate) fn null_lit() -> TsExpr {
    TsExpr::Lit(TsLit::Null)
}

/// A literal printed exactly as given: one of the fixed pieces of a hand-shaped line.
pub(crate) fn raw_lit(text: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Raw(text.into()))
}

// ---- Access and calls ----

/// `object.property`.
pub(crate) fn member(object: TsExpr, property: impl Into<String>) -> TsExpr {
    TsExpr::Member {
        object: Box::new(object),
        property: property.into(),
    }
}

/// `object[index]`.
pub(crate) fn index(object: TsExpr, index: TsExpr) -> TsExpr {
    TsExpr::Index {
        object: Box::new(object),
        index: Box::new(index),
    }
}

/// `callee(args)`. A callee that is a name is `call(ident(name), args)`.
pub(crate) fn call(callee: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::Call {
        callee: Box::new(callee),
        args,
    }
}

/// `object.method(args)`: the object is not bracketed, so `__xs.map(..)` and `a.b.c()` print
/// bare, and the printer brackets an object that needs it.
pub(crate) fn method_call(object: TsExpr, method: impl Into<String>, args: Vec<TsExpr>) -> TsExpr {
    call(member(object, method), args)
}

/// `new callee(args)`.
pub(crate) fn new(callee: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::New {
        callee: Box::new(callee),
        args,
    }
}

/// `callee<A, B>`.
pub(crate) fn type_app(callee: TsExpr, type_args: Vec<TsType>) -> TsExpr {
    TsExpr::TypeApp {
        callee: Box::new(callee),
        type_args,
    }
}

// ---- Operators and wrappers ----

/// `(inner)`.
pub(crate) fn paren(inner: TsExpr) -> TsExpr {
    TsExpr::Paren(Box::new(inner))
}

/// `...inner`.
pub(crate) fn spread(inner: TsExpr) -> TsExpr {
    TsExpr::Spread(Box::new(inner))
}

/// `await expr`.
pub(crate) fn await_expr(expr: TsExpr) -> TsExpr {
    TsExpr::Await(Box::new(expr))
}

/// `expr as ty`.
pub(crate) fn as_expr(expr: TsExpr, ty: TsType) -> TsExpr {
    TsExpr::As {
        expr: Box::new(expr),
        ty,
    }
}

/// `!expr`.
pub(crate) fn not_expr(expr: TsExpr) -> TsExpr {
    TsExpr::Unary {
        op: TsUnaryOp::Not,
        expr: Box::new(expr),
    }
}

/// `typeof expr`.
pub(crate) fn typeof_expr(expr: TsExpr) -> TsExpr {
    TsExpr::Unary {
        op: TsUnaryOp::Typeof,
        expr: Box::new(expr),
    }
}

/// `left op right`.
pub(crate) fn binary(op: TsBinaryOp, left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

/// `left === right`.
pub(crate) fn strict_eq(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(TsBinaryOp::StrictEq, left, right)
}

/// `left !== right`.
pub(crate) fn strict_neq(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(TsBinaryOp::StrictNotEq, left, right)
}

/// `left && right`.
pub(crate) fn and_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(TsBinaryOp::And, left, right)
}

/// `left || right`.
pub(crate) fn or_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(TsBinaryOp::Or, left, right)
}

/// `test ? consequent : alternate`.
pub(crate) fn cond_expr(test: TsExpr, consequent: TsExpr, alternate: TsExpr) -> TsExpr {
    TsExpr::Conditional {
        test: Box::new(test),
        consequent: Box::new(consequent),
        alternate: Box::new(alternate),
    }
}

// ---- Arrows ----

/// An arrow over any body, expression or block.
pub(crate) fn arrow_of(params: Vec<TsParam>, is_async: bool, body: TsArrowBody) -> TsExpr {
    TsExpr::Arrow {
        params,
        is_async,
        generics: vec![],
        return_type: None,
        body: Box::new(body),
    }
}

/// An expression-bodied arrow; `is_async` makes it `async`.
pub(crate) fn arrow_with(params: Vec<TsParam>, is_async: bool, body: TsExpr) -> TsExpr {
    arrow_of(params, is_async, TsArrowBody::Expr(Box::new(body)))
}

/// A synchronous expression-bodied arrow: `(params) => body`.
pub(crate) fn arrow(params: Vec<TsParam>, body: TsExpr) -> TsExpr {
    arrow_with(params, false, body)
}

/// An arrow with a multi-line block body, one statement per line.
pub(crate) fn block_arrow(params: Vec<TsParam>, is_async: bool, body: Vec<TsStmt>) -> TsExpr {
    arrow_of(params, is_async, TsArrowBody::Multiline(body))
}

// ---- Statements ----

/// `expr;`.
pub(crate) fn expr_stmt(expr: TsExpr) -> TsStmt {
    TsStmt::expr_stmt(expr, None)
}

/// `{ stmts }`.
pub(crate) fn block(stmts: Vec<TsStmt>) -> TsStmt {
    TsStmt::block(stmts, None)
}

/// `if (cond) then_branch`.
pub(crate) fn if_(cond: TsExpr, then_branch: TsStmt) -> TsStmt {
    TsStmt::if_stmt(cond, then_branch, None)
}

/// `const name = init;`.
pub(crate) fn const_(name: impl Into<String>, init: TsExpr) -> TsStmt {
    TsStmt::const_stmt(TsBindingName::Ident(name.into()), None, init, None)
}

/// `const name: ty = init;`, or without the annotation when `ty` is `None`. Accepts a bare
/// [`TsType`] too.
pub(crate) fn const_typed(
    name: impl Into<String>,
    ty: impl Into<Option<TsType>>,
    init: TsExpr,
) -> TsStmt {
    TsStmt::const_stmt(TsBindingName::Ident(name.into()), ty.into(), init, None)
}

/// `let name = init;`.
pub(crate) fn let_init(name: impl Into<String>, init: TsExpr) -> TsStmt {
    let_decl(name, None, Some(init))
}

/// `let name: ty;`.
pub(crate) fn let_typed(name: impl Into<String>, ty: TsType) -> TsStmt {
    let_decl(name, Some(ty), None)
}

/// `let name: ty = init;` with either part optional.
pub(crate) fn let_decl(
    name: impl Into<String>,
    ty: Option<TsType>,
    init: Option<TsExpr>,
) -> TsStmt {
    TsStmt::let_stmt(TsBindingName::Ident(name.into()), ty, init, None)
}

/// `return expr;`.
pub(crate) fn ret(expr: TsExpr) -> TsStmt {
    return_(Some(expr))
}

/// `return expr;`, or a bare `return;` for `None`.
pub(crate) fn return_(expr: Option<TsExpr>) -> TsStmt {
    TsStmt::return_stmt(expr, None)
}

/// `target = value;`.
pub(crate) fn assign(target: TsExpr, value: TsExpr) -> TsStmt {
    TsStmt::assign(target, value, None)
}

/// `for (const binding of iter) body`.
pub(crate) fn for_of(binding: impl Into<String>, iter: TsExpr, body: TsStmt) -> TsStmt {
    TsStmt::for_of(binding, iter, body, None)
}

/// `for await (const binding of iter) body`.
pub(crate) fn for_await_of(binding: impl Into<String>, iter: TsExpr, body: TsStmt) -> TsStmt {
    TsStmt::for_await_of(binding, iter, body, None)
}

/// `switch (discriminant) { cases }`.
pub(crate) fn switch_stmt(discriminant: TsExpr, cases: Vec<TsSwitchCase>) -> TsStmt {
    TsStmt::switch_stmt(discriminant, cases, None)
}

/// #4382: `actual` (a call's recorded type key, `"[Quote, Channel]"`) answers the type arguments
/// a `stub` or an observation names (`key`, `"[Quote]"`): the same list, or one that starts with
/// it. `dedup[A]` and `dedup[B]` stay apart; `dedup[Quote]` meets a call that inferred its key.
pub(crate) fn type_key_matches(actual: TsExpr, key: &str) -> TsExpr {
    let prefix = [key.strip_suffix(']').unwrap_or(key), ", "].concat();
    or_expr(
        strict_eq(actual.clone(), str_lit(key)),
        call(
            member(
                call(member(ident("globalThis"), "String"), vec![actual]),
                "startsWith",
            ),
            vec![str_lit(&prefix)],
        ),
    )
}

#[cfg(test)]
mod tests {
    //! Each builder against the literal variant it replaces. `TsExpr` carries no span and every
    //! statement builder passes `None`, so equality here is equality of the printed tree.

    use super::*;

    fn x() -> TsExpr {
        TsExpr::Ident("x".into())
    }

    fn y() -> TsExpr {
        TsExpr::Ident("y".into())
    }

    fn boxed(e: TsExpr) -> Box<TsExpr> {
        Box::new(e)
    }

    #[test]
    fn names_and_literals_are_the_variants() {
        assert_eq!(ident("x"), x());
        assert_eq!(ident(String::from("x")), x());
        assert_eq!(str_lit("s"), TsExpr::Lit(TsLit::Str("s".into())));
        assert_eq!(num_lit("1.5"), TsExpr::Lit(TsLit::Num("1.5".into())));
        assert_eq!(num_lit(7u32), TsExpr::Lit(TsLit::Num("7".into())));
        assert_eq!(bool_lit(true), TsExpr::Lit(TsLit::Bool(true)));
        assert_eq!(null_lit(), TsExpr::Lit(TsLit::Null));
        assert_eq!(raw_lit("a b"), TsExpr::Lit(TsLit::Raw("a b".into())));
    }

    #[test]
    fn access_and_calls_are_the_variants() {
        assert_eq!(
            member(x(), "p"),
            TsExpr::Member {
                object: boxed(x()),
                property: "p".into()
            }
        );
        assert_eq!(
            index(x(), y()),
            TsExpr::Index {
                object: boxed(x()),
                index: boxed(y())
            }
        );
        assert_eq!(
            call(x(), vec![y()]),
            TsExpr::Call {
                callee: boxed(x()),
                args: vec![y()]
            }
        );
        assert_eq!(
            new(x(), vec![y()]),
            TsExpr::New {
                callee: boxed(x()),
                args: vec![y()]
            }
        );
        assert_eq!(
            type_app(x(), vec![TsType::named("T")]),
            TsExpr::TypeApp {
                callee: boxed(x()),
                type_args: vec![TsType::named("T")]
            }
        );
    }

    /// `method_call` does not bracket its receiver: the list kernel's bracketing twin is
    /// `lower_list::bracketed_method_call`, and the two must stay different.
    #[test]
    fn method_call_leaves_its_receiver_bare() {
        assert_eq!(
            method_call(x(), "m", vec![y()]),
            TsExpr::Call {
                callee: boxed(TsExpr::Member {
                    object: boxed(x()),
                    property: "m".into()
                }),
                args: vec![y()]
            }
        );
        assert_ne!(
            method_call(x(), "m", vec![]),
            call(member(paren(x()), "m"), vec![])
        );
    }

    #[test]
    fn operators_and_wrappers_are_the_variants() {
        assert_eq!(paren(x()), TsExpr::Paren(boxed(x())));
        assert_eq!(spread(x()), TsExpr::Spread(boxed(x())));
        assert_eq!(await_expr(x()), TsExpr::Await(boxed(x())));
        assert_eq!(
            as_expr(x(), TsType::named("T")),
            TsExpr::As {
                expr: boxed(x()),
                ty: TsType::named("T")
            }
        );
        assert_eq!(
            not_expr(x()),
            TsExpr::Unary {
                op: TsUnaryOp::Not,
                expr: boxed(x())
            }
        );
        assert_eq!(
            typeof_expr(x()),
            TsExpr::Unary {
                op: TsUnaryOp::Typeof,
                expr: boxed(x())
            }
        );
        let bin = |op| TsExpr::Binary {
            op,
            left: boxed(x()),
            right: boxed(y()),
        };
        assert_eq!(binary(TsBinaryOp::Add, x(), y()), bin(TsBinaryOp::Add));
        assert_eq!(strict_eq(x(), y()), bin(TsBinaryOp::StrictEq));
        assert_eq!(strict_neq(x(), y()), bin(TsBinaryOp::StrictNotEq));
        assert_eq!(and_expr(x(), y()), bin(TsBinaryOp::And));
        assert_eq!(or_expr(x(), y()), bin(TsBinaryOp::Or));
        assert_eq!(
            cond_expr(x(), y(), x()),
            TsExpr::Conditional {
                test: boxed(x()),
                consequent: boxed(y()),
                alternate: boxed(x())
            }
        );
    }

    #[test]
    fn arrows_and_parameters_are_the_variants() {
        let p = || vec![TsParam::new("a", Some(TsType::named("A")))];
        let arrow_node = |is_async, body| TsExpr::Arrow {
            params: p(),
            is_async,
            generics: vec![],
            return_type: None,
            body: Box::new(body),
        };
        assert_eq!(
            arrow_with(p(), true, x()),
            arrow_node(true, TsArrowBody::Expr(boxed(x())))
        );
        assert_eq!(
            arrow(p(), x()),
            arrow_node(false, TsArrowBody::Expr(boxed(x())))
        );
        assert_eq!(
            block_arrow(p(), false, vec![expr_stmt(x())]),
            arrow_node(
                false,
                TsArrowBody::Multiline(vec![TsStmt::expr_stmt(x(), None)])
            )
        );
        assert_eq!(
            arrow_of(p(), true, TsArrowBody::Expr(boxed(y()))),
            arrow_node(true, TsArrowBody::Expr(boxed(y())))
        );
    }

    #[test]
    fn statements_pass_no_span() {
        let name = || TsBindingName::Ident("n".into());
        assert_eq!(expr_stmt(x()), TsStmt::expr_stmt(x(), None));
        assert_eq!(block(vec![]), TsStmt::block(vec![], None));
        assert_eq!(
            if_(x(), block(vec![])),
            TsStmt::if_stmt(x(), TsStmt::block(vec![], None), None)
        );
        assert_eq!(
            const_("n", x()),
            TsStmt::const_stmt(name(), None, x(), None)
        );
        // `const_typed` takes an annotation bare or optional.
        let ty = || TsType::named("T");
        assert_eq!(
            const_typed("n", ty(), x()),
            TsStmt::const_stmt(name(), Some(ty()), x(), None)
        );
        assert_eq!(
            const_typed("n", Some(ty()), x()),
            TsStmt::const_stmt(name(), Some(ty()), x(), None)
        );
        assert_eq!(
            const_typed("n", None, x()),
            TsStmt::const_stmt(name(), None, x(), None)
        );
        assert_eq!(
            let_init("n", x()),
            TsStmt::let_stmt(name(), None, Some(x()), None)
        );
        assert_eq!(
            let_typed("n", ty()),
            TsStmt::let_stmt(name(), Some(ty()), None, None)
        );
        assert_eq!(
            let_decl("n", Some(ty()), Some(x())),
            TsStmt::let_stmt(name(), Some(ty()), Some(x()), None)
        );
        assert_eq!(ret(x()), TsStmt::return_stmt(Some(x()), None));
        assert_eq!(return_(None), TsStmt::return_stmt(None, None));
        assert_eq!(assign(x(), y()), TsStmt::assign(x(), y(), None));
        assert_eq!(
            for_of("i", x(), block(vec![])),
            TsStmt::for_of("i", x(), TsStmt::block(vec![], None), None)
        );
        assert_eq!(
            for_await_of("i", x(), block(vec![])),
            TsStmt::for_await_of("i", x(), TsStmt::block(vec![], None), None)
        );
        assert_eq!(
            switch_stmt(x(), vec![]),
            TsStmt::switch_stmt(x(), vec![], None)
        );
    }
}
