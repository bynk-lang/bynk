//! #4246: `Query.filterMap`, `takeWhile`, `skipWhile` and the predicate form of `count`, the
//! #3963 `List` combinators on a query. Kept apart from `kernels.rs` so that file does not grow.
//!
//! `takeWhile` and `skipWhile` stop on order, which a query plan does not express as a filter,
//! so a query they read must have a declared one: an explicit `sortBy`/`sortByDescending`, or a
//! `store Log`, which is read oldest first. A `Map` scan, a joined, grouped or distinct query,
//! and a query held in a `let` have none the author can see, so the call is
//! `bynk.query.while_needs_order` rather than a guess.

use super::calls::check_arg;
use super::kernels::check_kernel_fn_arg;
use super::*;
use crate::kernel_op::QueryOp;

/// Type one of these methods. `count` reaches here only with its one predicate argument; the bare
/// `count()` is `check_query_kernel_method`'s. `receiver` is the call's receiver expression, or
/// `None` where the receiver is a `store Log` itself, which is ordered.
pub(super) fn check_query_more(
    method: &Ident,
    op: QueryOp,
    args: &[Expr],
    elem: TyId,
    receiver: Option<&Expr>,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let name = method.name.as_str();
    if args.len() != 1 {
        let takes = if op == QueryOp::Count {
            "0 arguments, or 1 (a predicate)"
        } else {
            "1 argument"
        };
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!("`Query.{name}` takes {takes}, got {}", args.len()),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    let predicate = |what: &str, ctx: &mut Ctx| {
        let p = Ty::Fn {
            params: vec![elem],
            ret: tys.intern(Ty::Base(BaseType::Bool)),
        };
        check_arg(&args[0], tys.intern(p), what, ctx);
    };
    match op {
        QueryOp::TakeWhile | QueryOp::SkipWhile => {
            predicate(&format!("the `Query.{name}` predicate"), ctx);
            if let Some(receiver) = receiver
                && !is_ordered(receiver, ctx)
            {
                ctx.errors.push(unordered_error(name, method.span));
                return None;
            }
            Some(tys.intern(Ty::Query(elem)))
        }
        QueryOp::Count => {
            predicate("the `Query.count` predicate", ctx);
            Some(tys.intern(Ty::Effect(tys.intern(Ty::Base(BaseType::Int)))))
        }
        QueryOp::FilterMap => {
            let ret =
                check_kernel_fn_arg(&args[0], vec![elem], "the `Query.filterMap` function", ctx)?;
            let Ty::Option(inner) = &*tys.get(ret) else {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.combinator_return_mismatch",
                        args[0].span,
                        format!(
                            "the `Query.filterMap` function must return `Option[U]`, but returns `{}`",
                            ret.display(tys)
                        ),
                    )
                    .with_note("return `Some(value)` to keep an element and `None` to drop it"),
                );
                return None;
            };
            Some(tys.intern(Ty::Query(*inner)))
        }
        _ => unreachable!("only these arms are routed here"),
    }
}

/// Does the query `receiver` have a declared order? It does when an explicit `sortBy` or
/// `sortByDescending` is in its chain, with only steps that keep the order after it, or when its
/// chain is rooted in a `store Log`.
fn is_ordered(receiver: &Expr, ctx: &Ctx) -> bool {
    match &receiver.kind {
        ExprKind::MethodCall {
            receiver: inner,
            method,
            ..
        } => match method.name.as_str() {
            "sortBy" | "sortByDescending" => true,
            "filter" | "map" | "filterMap" | "flatMap" | "skip" | "take" | "takeWhile"
            | "skipWhile" => is_ordered(inner, ctx),
            _ => false,
        },
        ExprKind::Paren(inner) => is_ordered(inner, ctx),
        ExprKind::Ident(id) => matches!(ctx.store_fields.get(&id.name), Some(StoreField::Log(_))),
        _ => false,
    }
}

fn unordered_error(name: &str, at: Span) -> CompileError {
    CompileError::new(
        "bynk.query.while_needs_order",
        at,
        format!("`{name}` stops on order, but this query has no declared order"),
    )
    .with_note(
        "put a `sortBy` (or `sortByDescending`) before it to say which order, or read a `store Log`, which is oldest first; a `Map` scan, a join, a grouping and a held query have no order to stop on",
    )
}
