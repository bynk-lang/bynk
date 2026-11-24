//! #3894: `<context>.<service>.outcome(args)`, a cross-context call whose failure is a value.
//!
//! `Pay.authorise(total)` has the handler's type and faults when the call fails. Hung on the
//! service reference rather than on `Effect`, `Pay.authorise.outcome(total)` is the same call,
//! typed `Effect[CallOutcome[R]]` for the handler's `R`: grammar, not a method valid on some
//! effects and not others (ADR for #3894, Decision A). The call is recorded as the ordinary
//! [`Callee::Cross`] of the service, so the consumes, architecture and caller-admission facts
//! read an `.outcome` call as the call it is.

use super::calls::{check_cross_context_call, cross_context_prefix};
use super::*;

/// The name of the type `.outcome(…)` yields, declared in `bynk`.
const CALL_OUTCOME: &str = "CallOutcome";

/// Whether `receiver.method` spells `<context>.<service>.outcome`: `method` is `outcome`, and
/// `receiver` is a service reference, a consumed-context prefix and a service name. The prefix
/// is returned with the service name. A service that is itself named `outcome`
/// (`Pay.outcome(x)`) keeps its plain call, so a receiver that is already a context prefix is
/// not one.
pub(super) fn outcome_service_of<'a>(
    receiver: &'a Expr,
    method: &Ident,
    ctx: &Ctx,
) -> Option<(&'a Expr, &'a Ident, String)> {
    if method.name != "outcome" {
        return None;
    }
    let ExprKind::FieldAccess {
        receiver: prefix,
        field: service,
    } = &receiver.kind
    else {
        return None;
    };
    if ctx.lookup_root_ident(receiver).is_some() || ctx.root_ident_is_store_field(receiver) {
        return None;
    }
    if cross_context_prefix(receiver, ctx).is_some() {
        return None;
    }
    let consumed = cross_context_prefix(prefix, ctx)?;
    Some((prefix, service, consumed))
}

/// Check `<context>.<service>.outcome(args)` when that is what the call is, else `None`. The
/// service call is checked as `<context>.<service>(args)` is; its `Effect[R]` becomes
/// `Effect[CallOutcome[R]]`.
pub(super) fn check_outcome_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    let (prefix, service, consumed) = outcome_service_of(receiver, method, ctx)?;
    let tys = ctx.tys;
    let call = check_cross_context_call(prefix, &consumed, service, args, span, expr_id, ctx)?;
    let Ty::Effect(handled) = *tys.get(call) else {
        return Some(Some(call));
    };
    let Some(decl) = ctx.input.types.get(CALL_OUTCOME).cloned() else {
        ctx.errors.push(
            CompileError::new(
                "bynk.call.outcome_not_in_scope",
                method.span,
                "`outcome` yields `CallOutcome[_]`, but `CallOutcome` is not in scope here",
            )
            .with_note(
                "add the line `consumes bynk { CallOutcome }` to this unit, or add `CallOutcome` to its existing `consumes bynk { … }` list",
            ),
        );
        return Some(None);
    };
    let outcome = named_ty_with_args(&decl, vec![handled], tys);
    Some(Some(tys.intern(Ty::Effect(outcome))))
}

/// Whether `type_name` is a type only `.outcome(…)` and a test `stub` make: `CallOutcome` and
/// `CallFailure` (ADR for #3894, Decision F). `Refusal` is a plain enum of reasons, and a
/// refusal alone is no outcome, so it is not one.
fn is_call_outcome_type(type_name: &str) -> bool {
    matches!(type_name, CALL_OUTCOME | "CallFailure")
}

/// Refuse the construction of a variant of `type_name` in a context, when it is a `bynk` call
/// outcome type: a value of it says what a call came to, which only the compiled call knows.
/// Matching one is unaffected. Returns whether it was refused.
pub(super) fn reject_outcome_construction(type_name: &str, span: Span, ctx: &mut Ctx) -> bool {
    if !ctx.input.is_context || !is_call_outcome_type(type_name) {
        return false;
    }
    ctx.errors.push(
        CompileError::new(
            "bynk.call.outcome_construction",
            span,
            format!("cannot construct `{type_name}` here — only a call's `.outcome(…)` makes one"),
        )
        .with_note(
            "match on a `CallOutcome` or `CallFailure`, or have a test `stub` say what the call came to (`returns`, `rejected`, `faulted`, `unreachable`)",
        ),
    );
    true
}
