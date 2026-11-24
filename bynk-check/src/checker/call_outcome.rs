//! #3894: `<context>.<service>.outcome(args)`, a cross-context call whose failure is a value.
//!
//! `Pay.authorise(total)` has the handler's type and faults when the call fails. Hung on the
//! service reference rather than on `Effect`, `Pay.authorise.outcome(total)` is the same call,
//! typed `Effect[CallOutcome[R]]` for the handler's `R`: grammar, not a method valid on some
//! effects and not others (ADR for #3894, Decision A). The call is recorded as the ordinary
//! [`Callee::Cross`] of the service, so the consumes, architecture and caller-admission facts
//! read an `.outcome` call as the call it is.
//!
//! #3933: `Agent(key).op.outcome(args)` is the same form for an agent operation, typed the same
//! `Effect[CallOutcome[R]]` and recorded as the ordinary [`Callee::Agent`].

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
    let call = check_cross_context_call(prefix, &consumed, service, args, span, expr_id, ctx)?;
    Some(wrap_in_call_outcome(call, method, ctx))
}

/// `Effect[R]` as `Effect[CallOutcome[R]]`, or `bynk.call.outcome_not_in_scope` (reported at
/// `method`, the `outcome`) when the unit does not have `CallOutcome` in scope. `call` is
/// returned as it is when it is not an effect.
fn wrap_in_call_outcome(call: TyId, method: &Ident, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    let Ty::Effect(handled) = *tys.get(call) else {
        return Some(call);
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
        return None;
    };
    let outcome = named_ty_with_args(&decl, vec![handled], tys);
    Some(tys.intern(Ty::Effect(outcome)))
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

/// The agent an `.outcome` receiver's inner expression names, when it is an agent instance:
/// `Agent(key)`, or a local bound to one. The instance of a handler's own `self` is not one: a
/// call to it is part of the same handler (ADR 0781), so it stays a plain call.
fn agent_of_instance(inner: &Expr, ctx: &Ctx) -> Option<String> {
    match &inner.kind {
        ExprKind::Call { name, .. } if ctx.input.agents.contains_key(&name.name) => {
            Some(name.name.clone())
        }
        ExprKind::Ident(id) if id.name != "self" => match &*ctx.tys.get(ctx.lookup(&id.name)?) {
            Ty::Named { name, .. } if ctx.input.agents.contains_key(name) => Some(name.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Check `Agent(key).op.outcome(args)` when that is what the call is, else `None`. The operation
/// call is checked as `Agent(key).op(args)` is; its `Effect[R]` becomes `Effect[CallOutcome[R]]`,
/// the same type a cross-context call yields.
pub(super) fn check_agent_outcome_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    if method.name != "outcome" {
        return None;
    }
    let ExprKind::FieldAccess {
        receiver: inner,
        field: op,
    } = &receiver.kind
    else {
        return None;
    };
    let agent_name = agent_of_instance(inner, ctx)?;
    let tys = ctx.tys;
    let _ = type_of(inner, None, ctx);
    let agent = ctx.input.agents.get(&agent_name).cloned()?;
    let Some(handler) = agent
        .handlers
        .iter()
        .find(|h| h.method_name.as_ref().is_some_and(|n| n.name == op.name))
    else {
        ctx.errors.push(CompileError::new(
            "bynk.agent.handler_not_found",
            op.span,
            format!("agent `{agent_name}` has no handler named `{}`", op.name),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return Some(None);
    };
    ctx.callees.insert(
        expr_id,
        Callee::Agent {
            agent: agent_name.clone(),
            handler: op.name.clone(),
        },
    );
    ctx.refs.record(
        op.span,
        SymbolKind::Handler,
        &format!("{agent_name}.{}", op.name),
    );
    if handler.params.len() != args.len() {
        ctx.errors.push(CompileError::new(
            "bynk.agent.handler_arity",
            op.span,
            format!(
                "agent handler `{agent_name}.{}` expects {} argument(s), but {} were given",
                op.name,
                handler.params.len(),
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return Some(None);
    }
    for (p, arg) in handler.params.iter().zip(args.iter()) {
        let pty = resolve_type_ref(&p.type_ref, &ctx.input.types, tys);
        let arg_ty = type_of(arg, pty, ctx);
        if let (Some(a), Some(p_ty)) = (arg_ty, pty.as_ref())
            && !compatible(a, *p_ty, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                arg.span,
                format!(
                    "argument has type `{}`, but `{agent_name}.{}` expects `{}`",
                    a.display(tys),
                    op.name,
                    p_ty.display(tys)
                ),
            ));
        }
    }
    let call = resolve_type_ref(&handler.return_type, &ctx.input.types, tys)?;
    Some(wrap_in_call_outcome(call, method, ctx))
}
