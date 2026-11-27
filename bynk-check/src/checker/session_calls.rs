//! #3938: `Actor.revokeAll(identity)` on a `SignedCookie` actor with `sessions = true`: end every
//! session of one identity in its session agent. It is an `Effect[()]` that runs in a service
//! handler, where the session agent is reached, and takes the actor's own identity type, so a
//! handler can only revoke the sessions of a value that is already a valid, sealed identity.

use super::*;

/// Type `Actor.revokeAll(identity)`. `decl` is the actor and `actor` its name as written. A
/// stateless actor has no sessions to revoke, so the call is refused there.
pub(super) fn check_revoke_all(
    decl: &bynk_syntax::ast::ActorDecl,
    actor: &Ident,
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    let tys = ctx.tys;
    if !crate::actors::signed_cookie_seam_of(decl, None).is_some_and(|s| s.sessions) {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.method_not_found",
                span,
                format!(
                    "`{}` has no member `revokeAll`: its cookies are stateless, so there are no sessions to revoke",
                    actor.name
                ),
            )
            .with_note(
                "declare `sessions = true` on the actor to keep each session in a generated agent, which `revokeAll` then empties",
            ),
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return Some(None);
    }
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`{}.revokeAll` takes 1 argument, got {}",
                actor.name,
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return Some(None);
    }
    ctx.callees.insert(
        expr_id,
        Callee::SessionRevokeAll {
            actor: actor.name.clone(),
        },
    );
    if let Some(bynk_syntax::ast::TypeRef::Named(id)) = &decl.identity
        && let Some(t) = ctx.input.types.get(&id.name).cloned()
    {
        let identity = crate::checker::named_ty(&t, tys);
        check_arg(
            &args[0],
            identity,
            &format!("the `{}.revokeAll` identity", actor.name),
            ctx,
        );
    } else {
        let _ = type_of(&args[0], None, ctx);
    }
    if !ctx.effectful {
        ctx.errors.push(CompileError::new(
            "bynk.effect.capability_in_pure_context",
            span,
            format!(
                "`{}.revokeAll` ends sessions in its session agent, so it can only be called inside an effectful body (one returning `Effect[T]`)",
                actor.name
            ),
        ));
    }
    if !ctx.in_service_handler && !ctx.in_test_body {
        ctx.errors.push(CompileError::new(
            "bynk.actor.signed_cookie_issue_outside_handler",
            span,
            format!(
                "`{}.revokeAll` can only be called in a service handler",
                actor.name
            ),
        ));
    }
    Some(Some(tys.intern(Ty::Effect(tys.intern(Ty::Unit)))))
}

/// `by Revoked` on a route whose seam keeps no sessions (#3938) and is no `ApiKey` seam (#3939):
/// `bynk.test.revoked_needs_sessions_route`.
pub(super) fn revoked_needs_sessions(span: Span) -> CompileError {
    CompileError::new(
        "bynk.test.revoked_needs_sessions_route",
        span,
        "`by Revoked` sends a revoked session's cookie or a revoked API key, but this handler's route has no `SignedCookie` actor with `sessions = true` and no `ApiKey` actor to refuse it",
    )
    .with_note(
        "use `by Revoked` on a route guarded by a `SignedCookie` actor declared with `sessions = true`, or by an `ApiKey` actor; a stateless cookie cannot be revoked, and `by Forged` or `by Expired` test its signature and expiry",
    )
}
