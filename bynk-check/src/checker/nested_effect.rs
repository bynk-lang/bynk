//! #4324: an `Effect` cannot wrap another `Effect`.
//!
//! An effect is a promise at runtime, and promises flatten, so `Effect[Effect[T]]` cannot
//! exist there: after `let inner <- nested(e)` the checker would call `inner` an `Effect[T]`
//! while at runtime it is already the `T`, and `inner.map(..)` would throw. The shape is
//! refused instead of boxed, because boxing would add a wrapper to every generic effect path
//! for a shape nobody needs: to sequence effects, write `andThen`, or `<-` in an effectful body.
//!
//! Two places refuse it. The resolver refuses a *written* `Effect[Effect[_]]` (at any depth, and
//! inside any type argument). This module refuses an *instantiated* one: a type variable that
//! a call fills with an `Effect` where the signature wraps it in another, as `Effect.pure(e)`
//! and `wrap(e)` do. A generic signature such as `mapE[T, U]` stays legal; only the
//! instantiation is refused.

use super::*;

/// The error for a nested effect at `span`; `shape` is the offending type, as written or inferred.
pub(crate) fn nested_effect_error(span: Span, shape: &str) -> CompileError {
    CompileError::new(
        "bynk.types.nested_effect",
        span,
        format!(
            "`{shape}` nests an `Effect` in another `Effect`, which cannot exist at runtime: an effect is a promise, and promises flatten"
        ),
    )
    .with_note(
        "to sequence effects, use `andThen` (or `<-` in an effectful body), and return `Effect[T]` rather than an effect of an effect",
    )
}

/// Does `t` contain an `Effect` whose payload is itself an `Effect`, at any depth?
pub(crate) fn has_nested_effect(t: TyId, tys: &Types) -> bool {
    match &*tys.get(t) {
        Ty::Effect(inner) => {
            matches!(&*tys.get(*inner), Ty::Effect(_)) || has_nested_effect(*inner, tys)
        }
        Ty::Result(a, b) | Ty::Map(a, b) => {
            has_nested_effect(*a, tys) || has_nested_effect(*b, tys)
        }
        Ty::Option(a)
        | Ty::HttpResult(a)
        | Ty::HttpOutcome(a)
        | Ty::List(a)
        | Ty::Set(a)
        | Ty::Query(a)
        | Ty::Stream(a)
        | Ty::TestConnection(a)
        | Ty::Connection(a) => has_nested_effect(*a, tys),
        Ty::Fn { params, ret } => {
            params.iter().any(|p| has_nested_effect(*p, tys)) || has_nested_effect(*ret, tys)
        }
        Ty::Named { args, .. } => args.iter().any(|a| has_nested_effect(*a, tys)),
        _ => false,
    }
}

/// Is `t` an `Effect`, or does it carry one?
fn mentions_effect(t: TyId, tys: &Types) -> bool {
    match &*tys.get(t) {
        Ty::Effect(_) => true,
        Ty::Result(a, b) | Ty::Map(a, b) => mentions_effect(*a, tys) || mentions_effect(*b, tys),
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => mentions_effect(*a, tys),
        Ty::Named { args, .. } => args.iter().any(|a| mentions_effect(*a, tys)),
        _ => false,
    }
}

/// `Effect.pure(inner)`: the effect of `inner_ty`, refused when `inner_ty` is already an effect.
pub(super) fn pure(inner_ty: TyId, inner_span: Span, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    let ty = tys.intern(Ty::Effect(inner_ty));
    if !has_nested_effect(ty, tys) {
        return Some(ty);
    }
    ctx.errors
        .push(nested_effect_error(inner_span, &ty.display(tys)));
    None
}

/// A generic call's instantiated return type `ret` (from the signature's `pattern`): refused when
/// the instantiation, not the signature, nests an effect. The error is at the argument that
/// carried the effect, else at `at`.
pub(super) fn instantiated(
    pattern: TyId,
    subst: &HashMap<String, TyId>,
    args: &[Expr],
    arg_tys: &[Option<TyId>],
    at: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let ret = substitute(pattern, subst, tys);
    if !has_nested_effect(ret, tys) || has_nested_effect(pattern, tys) {
        return Some(ret);
    }
    let span = args
        .iter()
        .zip(arg_tys)
        .find(|(_, t)| t.is_some_and(|t| mentions_effect(t, tys)))
        .map_or(at, |(a, _)| a.span);
    ctx.errors
        .push(nested_effect_error(span, &ret.display(tys)));
    None
}
