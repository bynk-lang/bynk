//! #3968: `parForEachWith`, `parTraverseWith`, `parTraverseAllWith` and
//! `parTraverseTryWith`: the concurrent `List` forms with a bound on how many
//! steps are in flight. Each is its plain form's typing plus a second argument, a
//! `bynk` `ParOptions { limit }`. Kept apart from `kernels.rs` so that file
//! does not grow.

use super::*;
use crate::kernel_op::ListOp;

/// The most steps that may be in flight (`ParOptions.limit`'s upper bound).
const MAX_LIMIT: i64 = 1000;

/// The plain form a `…With` form types its function argument like, and the
/// sequential form to suggest for `limit: 1`.
fn plain_and_sequential(op: ListOp) -> (ListOp, &'static str) {
    match op {
        ListOp::ParForEachWith => (ListOp::ParForEach, "forEach"),
        ListOp::ParTraverseWith => (ListOp::ParTraverse, "traverse"),
        ListOp::ParTraverseAllWith => (ListOp::ParTraverseAll, "traverseAll"),
        ListOp::ParTraverseTryWith => (ListOp::ParTraverseTry, "traverseTry"),
        _ => unreachable!("only the `…With` forms are routed here"),
    }
}

/// The `limit` field's expression when the options argument is written in place
/// as a `ParOptions { limit: … }` literal.
fn limit_field(options: &Expr) -> Option<&Expr> {
    let mut e = options;
    while let ExprKind::Paren(inner) = &e.kind {
        e = inner;
    }
    let ExprKind::RecordConstruction {
        type_name, fields, ..
    } = &e.kind
    else {
        return None;
    };
    if type_name.name != "ParOptions" {
        return None;
    }
    // A shorthand `ParOptions { limit }` has no expression here; it is judged at run time.
    fields
        .iter()
        .find(|f| f.name.name == "limit")
        .and_then(|f| f.value.as_ref())
}

/// The `Int` a `limit` expression is fixed to at compile time: a literal, or a
/// constant bound (through other constants) to one. `None` for anything computed.
fn static_limit(e: &Expr, ctx: &Ctx) -> Option<i64> {
    let literal = |k: &ExprKind| match k {
        ExprKind::IntLit { value, .. } => Some(*value),
        ExprKind::UnaryOp(UnaryOp::Neg, inner) => match &inner.kind {
            ExprKind::IntLit { value, .. } => value.checked_neg(),
            _ => None,
        },
        _ => None,
    };
    match &e.kind {
        ExprKind::Paren(inner) => static_limit(inner, ctx),
        // A local of the same name shadows a constant.
        ExprKind::Ident(id) if ctx.lookup(id.name.as_str()).is_none() => {
            let start = ctx.input.consts.get(id.name.as_str())?;
            let lit =
                crate::consts::chain_literal(start, BaseType::Int, ctx.input.consts.len(), |n| {
                    ctx.input.consts.get(n)
                })?;
            literal(&lit.kind)
        }
        k => literal(k),
    }
}

/// Whether `name`, in the project's type table, is the `ParOptions` the `bynk` adapter
/// exports, found by its declaration's file (as `bynk.cloudflare`'s `SqlStatement`
/// is). A record of the same name declared elsewhere is not it, and the emitted call
/// reads its `limit` without a check.
fn is_bynk_par_options(name: &str, ctx: &Ctx) -> bool {
    ctx.input.types.get(name).is_some_and(|decl| {
        crate::firstparty::unit(crate::firstparty::BYNK_UNIT)
            .is_some_and(|unit| decl.span.file == crate::project_model::firstparty_file_id(unit))
    })
}

/// Type one `…With` call: the function argument as its plain form, the options
/// argument as a `ParOptions`, and a `limit` fixed at compile time against
/// `1..=1000` (`bynk.list.par_limit_range`; `1` is `bynk.list.par_limit_one`).
pub(super) fn check_par_with(
    method: &Ident,
    op: ListOp,
    args: &[Expr],
    elem: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let name = method.name.as_str();
    if args.len() != 2 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!("`List.{name}` takes 2 arguments, got {}", args.len()),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    let (plain, sequential) = plain_and_sequential(op);
    let result =
        super::kernels::check_list_kernel_method(method, Some(plain), &args[..1], elem, span, ctx);
    let options = type_of(&args[1], None, ctx)?;
    let is_options = matches!(
        &*tys.get(options),
        Ty::Named { name, kind: NamedKind::Record, .. }
            if name == "ParOptions" && is_bynk_par_options(name, ctx)
    );
    if !is_options {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.type_mismatch",
                args[1].span,
                format!(
                    "the `List.{name}` options must be a `ParOptions`, but this is `{}`",
                    options.display(tys)
                ),
            )
            .with_note(
                "pass a `ParOptions { limit: 10 }`, the record from `consumes bynk { ParOptions }` (a record of the same name declared elsewhere does not count)",
            ),
        );
        return result;
    }
    if let Some(limit) = limit_field(&args[1]).and_then(|e| static_limit(e, ctx)) {
        let at = limit_field(&args[1]).map_or(args[1].span, |e| e.span);
        if !(1..=MAX_LIMIT).contains(&limit) {
            ctx.errors.push(
                CompileError::new(
                    "bynk.list.par_limit_range",
                    at,
                    format!(
                        "a concurrency `limit` is a whole number from 1 to {MAX_LIMIT}, but this is {limit}"
                    ),
                )
                .with_note(
                    "pick a `limit` from 1 to 1000, or use the plain form for an unbounded fan-out; a `limit` computed at run time outside that range faults the call instead",
                ),
            );
        } else if limit == 1 {
            ctx.errors.push(
                CompileError::new(
                    "bynk.list.par_limit_one",
                    method.span,
                    "`limit: 1` runs one step at a time, so nothing overlaps",
                )
                .with_note(format!(
                    "`{sequential}` is the sequential form of `{name}`; it also stops at the first failed step, where `{name}` still starts the rest"
                )),
            );
        }
    }
    result
}
