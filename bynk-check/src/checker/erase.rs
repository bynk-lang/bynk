//! #3931: `self.erase()` in an agent `on call` handler.
//!
//! `self` is the agent's key (a synthetic record, see `context_checks`); `erase` is the one method it
//! has. The call answers `Effect[()]` and marks the handler: when it commits, the instance's
//! stored state is deleted instead of the handler's other writes, and the next call to the key
//! starts from the initial state, as a never-seen key does.

use super::*;

/// Type `self.erase()`. Returns `None` when the call is not `self.erase` in an agent handler,
/// leaving it to the other rules (which name the agent in the refusal, #4291).
pub(crate) fn check_erase_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    let ExprKind::Ident(id) = &receiver.kind else {
        return None;
    };
    if id.name != "self" || method.name != "erase" || !ctx.in_agent_handler {
        return None;
    }
    let tys = ctx.tys;
    let own_self = ctx.lookup("self").is_some_and(|t| {
        matches!(&*tys.get(t), Ty::Named { name, .. } if super::expressions::self_record_agent(name).is_some())
    });
    if !own_self {
        return None;
    }
    if !args.is_empty() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!("`self.erase` takes 0 arguments, got {}", args.len()),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return Some(None);
    }
    if !ctx.effectful {
        ctx.errors.push(CompileError::new(
            "bynk.effect.capability_in_pure_context",
            span,
            "`self.erase` deletes stored state, so it can only be called inside an effectful body (one returning `Effect[T]`)",
        ));
    }
    ctx.callees.insert(expr_id, Callee::AgentErase);
    let unit = tys.intern(Ty::Unit);
    Some(Some(tys.intern(Ty::Effect(unit))))
}

/// Whether `e` is a call the checker classified as `self.erase()`.
fn is_erase(e: &Expr, ctx: &Ctx) -> bool {
    matches!(ctx.callees.get(&e.id), Some(Callee::AgentErase))
}

/// Whether `s` runs `self.erase()` as its effect: `do self.erase()` or `let _ <- self.erase()`.
fn statement_erases(s: &Statement, ctx: &Ctx) -> bool {
    match s {
        Statement::Do(d) => is_erase(&d.value, ctx),
        Statement::EffectLet(l) => is_erase(&l.value, ctx),
        Statement::Let(_)
        | Statement::Expect(_)
        | Statement::Assign(_)
        | Statement::Advance(_)
        | Statement::Par(_) => false,
    }
}

/// The mutating store operation `e` runs, if it is one (`do orders.put(k, v)`), as the field name.
fn store_write(e: &Expr, ctx: &Ctx) -> Option<String> {
    match ctx.callees.get(&e.id) {
        Some(Callee::Store { field, op }) if op.mutates() => Some(field.clone()),
        _ => None,
    }
}

/// #3931: a write after `self.erase()` has no effect, since the commit deletes the instance rather
/// than writing the handler's state. Warned (`bynk.agent.write_after_erase`) at the write: a `:=` or
/// a mutating store operation, in the same block or in a block nested after the erase (`erased` is
/// the outer block's state). A write before the erase is discarded the same way, but is the natural
/// order of a handler that records something and then erases (an audit entry is an event, not a
/// write), so it is not warned.
pub(crate) fn warn_writes_after_erase(block: &Block, erased: bool, ctx: &mut Ctx) {
    let mut erased = erased;
    for s in &block.statements {
        if erased {
            let target = match s {
                Statement::Assign(a) => Some((a.span, a.target.name.clone())),
                Statement::Do(d) => store_write(&d.value, ctx).map(|f| (d.value.span, f)),
                Statement::EffectLet(l) => store_write(&l.value, ctx).map(|f| (l.value.span, f)),
                Statement::Let(_)
                | Statement::Expect(_)
                | Statement::Advance(_)
                | Statement::Par(_) => None,
            };
            if let Some((span, name)) = target {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.agent.write_after_erase",
                        span,
                        format!(
                            "the write to `{name}` has no effect: `self.erase()` above deletes this instance's state when the handler commits"
                        ),
                    )
                    .with_note("an erased instance keeps none of the handler's writes, whether they come before or after the call"),
                );
            }
        }
        erased = erased || statement_erases(s, ctx);
        for e in statement_exprs(s) {
            warn_in_expr(e, erased, ctx);
        }
    }
    warn_in_expr(&block.tail, erased, ctx);
}

fn statement_exprs(s: &Statement) -> Vec<&Expr> {
    match s {
        Statement::Let(l) | Statement::EffectLet(l) => vec![&l.value],
        Statement::Do(d) => vec![&d.value],
        Statement::Assign(a) => vec![&a.value],
        Statement::Expect(_) | Statement::Advance(_) | Statement::Par(_) => Vec::new(),
    }
}

fn warn_in_expr(e: &Expr, erased: bool, ctx: &mut Ctx) {
    if let ExprKind::Block(b) = &e.kind {
        warn_writes_after_erase(b, erased, ctx);
    } else if let ExprKind::If {
        then_block,
        else_block,
        ..
    } = &e.kind
    {
        warn_writes_after_erase(then_block, erased, ctx);
        warn_writes_after_erase(else_block, erased, ctx);
    } else if let ExprKind::Match { arms, .. } = &e.kind {
        for arm in arms {
            match &arm.body {
                MatchBody::Block(b) => warn_writes_after_erase(b, erased, ctx),
                MatchBody::Expr(x) => warn_in_expr(x, erased, ctx),
            }
        }
    }
}
