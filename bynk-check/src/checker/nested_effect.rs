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
        | Ty::HttpResult(a, _)
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

/// The type variables of `t` that `subst` fills with a type carrying an `Effect`.
fn effect_vars(t: TyId, subst: &HashMap<String, TyId>, tys: &Types, out: &mut Vec<String>) {
    match &*tys.get(t) {
        Ty::Var(n) if subst.get(n).is_some_and(|s| mentions_effect(*s, tys)) => {
            out.push(n.clone());
        }
        Ty::Result(a, b) | Ty::Map(a, b) => {
            effect_vars(*a, subst, tys, out);
            effect_vars(*b, subst, tys, out);
        }
        Ty::Effect(a) | Ty::Option(a) | Ty::List(a) | Ty::Set(a) | Ty::Query(a) | Ty::Stream(a) => {
            effect_vars(*a, subst, tys, out)
        }
        Ty::Fn { params, ret } => {
            params.iter().for_each(|p| effect_vars(*p, subst, tys, out));
            effect_vars(*ret, subst, tys, out);
        }
        Ty::Named { args, .. } => args.iter().for_each(|a| effect_vars(*a, subst, tys, out)),
        _ => {}
    }
}

/// A generic call's instantiated return type, from the signature's return `pattern` under
/// `subst`: refused when the instantiation, not the signature, nests an effect. The error is at
/// the first argument whose parameter (in `params`) mentions a type variable that `subst` fills
/// with an effect, else at `at`.
pub(super) fn instantiated(
    pattern: TyId,
    subst: &HashMap<String, TyId>,
    args: &[Expr],
    params: &[Option<TyId>],
    at: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let ret = substitute(pattern, subst, tys);
    // #4364: a generic type that holds its parameter under an `Effect` nests too, once its
    // argument is an effect.
    let wrapper = |t: TyId| super::nested_wrapper::nests_through_wrapper(t, tys, &ctx.input.types);
    let nests = |t: TyId| has_nested_effect(t, tys) || wrapper(t).is_some();
    if !nests(ret) || nests(pattern) {
        return Some(ret);
    }
    let note = wrapper(ret);
    let mut carriers = Vec::new();
    effect_vars(pattern, subst, tys, &mut carriers);
    let span = args
        .iter()
        .zip(params)
        .find(|(_, p)| {
            p.is_some_and(|p| {
                let mut vs = Vec::new();
                effect_vars(p, subst, tys, &mut vs);
                vs.iter().any(|v| carriers.contains(v))
            })
        })
        .map_or(at, |(a, _)| a.span);
    let mut err = nested_effect_error(span, &ret.display(tys));
    if let Some(note) = note {
        err = err.with_note(note);
    }
    ctx.errors.push(err);
    None
}

/// A method call's result `r`: refused when it nests an effect, which a kernel method builds
/// when it wraps a caller-chosen type in an `Effect` (`q.fold(e0, ..)` with `e0: Effect[Int]`).
/// The error is at the call, `at`; a call already refused returns `None` and is left alone.
pub(super) fn method_result(r: Option<TyId>, at: Span, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    let t = r?;
    let note = super::nested_wrapper::nests_through_wrapper(t, tys, &ctx.input.types);
    if !has_nested_effect(t, tys) && note.is_none() {
        return Some(t);
    }
    let mut err = nested_effect_error(at, &t.display(tys));
    if let Some(note) = note {
        err = err.with_note(note);
    }
    ctx.errors.push(err);
    None
}
