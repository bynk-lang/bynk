//! #3939 (the API keys track, §9): `Actor.mint(identity, label)` on an `ApiKey` actor.
//!
//! `mint` makes a key for `identity`, stores its peppered digest in the identity's key store and
//! answers it once, as an `ApiKeySecret`, or `TooManyKeys` when the identity holds the most live
//! keys it may. It hashes the key under the pepper, which reaches a service handler's
//! dependencies and nothing else, so it is legal only in a service handler
//! (`bynk.actor.api_key_mint_outside_handler`, D2 B). It stamps the key's creation time from the
//! `Clock`, which the handler lists in `given`. It takes the actor's own identity type, so a
//! handler can only mint a key for a value that is already a valid, sealed identity.

use super::*;

/// Type a member call on a local `ApiKey` actor, or `None` when `receiver` is not one.
pub(crate) fn check_api_key_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    let ExprKind::Ident(actor) = &receiver.kind else {
        return None;
    };
    if ctx.lookup(actor.name.as_str()).is_some() {
        return None;
    }
    if actor.name == "ApiKey" {
        return Some(check_internal_equal(method, args, span, expr_id, ctx));
    }
    let decl = api_key_actor(&actor.name, ctx)?;
    ctx.refs.record(actor.span, SymbolKind::Actor, &actor.name);
    if method.name != "mint" {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_not_found",
            method.span,
            format!(
                "an `ApiKey` actor has no member `{}`: it has `mint(identity, label)`",
                method.name
            ),
        ));
        return give_up(args, ctx);
    }
    if args.len() != 2 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`{}.mint` takes 2 arguments (the identity and a label), got {}",
                actor.name,
                args.len()
            ),
        ));
        return give_up(args, ctx);
    }
    let tys = ctx.tys;
    let (Some(secret), Some(too_many), Some(label)) = (
        std_named(ctx, "ApiKeySecret"),
        std_named(ctx, "TooManyKeys"),
        std_named(ctx, "ApiKeyLabel"),
    ) else {
        return give_up(args, ctx);
    };
    ctx.callees.insert(
        expr_id,
        Callee::ApiKeyMint {
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
            &format!("the `{}.mint` identity", actor.name),
            ctx,
        );
    } else {
        let _ = type_of(&args[0], None, ctx);
    }
    check_arg(
        &args[1],
        label,
        &format!("the `{}.mint` label", actor.name),
        ctx,
    );
    if !ctx.effectful {
        ctx.errors.push(CompileError::new(
            "bynk.effect.capability_in_pure_context",
            span,
            format!(
                "`{}.mint` stores a key in its key store, so it can only be called inside an effectful body (one returning `Effect[T]`)",
                actor.name
            ),
        ));
    }
    if !ctx.in_service_handler && !ctx.in_test_body {
        ctx.errors.push(
            CompileError::new(
                "bynk.actor.api_key_mint_outside_handler",
                span,
                format!("`{}.mint` can only be called in a service handler", actor.name),
            )
            .with_note(
                "the pepper a key is hashed with reaches a service handler, not a provider, an agent or a function: mint the key in the handler and return it",
            ),
        );
    }
    if ctx.caps.capabilities.contains_key("Clock") {
        ctx.caps.given_used.insert("Clock".to_string());
    } else {
        ctx.errors.push(
            CompileError::new(
                "bynk.given.undeclared_capability",
                span,
                format!(
                    "`{}.mint` reads the clock for the key's creation time, but the handler does not list `Clock` in `given`",
                    actor.name
                ),
            )
            .with_note(
                "add `consumes bynk { Clock }` to the context and `given Clock` to the handler, so the dependency is visible and a test can stub it",
            ),
        );
    }
    let result = tys.intern(Ty::Result(secret, too_many));
    Some(Some(tys.intern(Ty::Effect(result))))
}

/// #3939: `ApiKey.equal(a, b)`, the key store's constant-time comparison of two digests. It is
/// internal: only the agents the compiler generates for an `ApiKey` actor reach it, so no
/// capability is put in the author's scope for code they did not write. Anywhere else it is
/// `bynk.actor.api_key_internal`. It lowers to the runtime's one constant-time comparison, the
/// function `bynk.crypto`'s `Crypto.equal` calls.
fn check_internal_equal(
    method: &Ident,
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    if method.name != "equal" || !crate::api_key::in_generated_agent(span, &ctx.input.commons.items)
    {
        ctx.errors.push(
            CompileError::new(
                "bynk.actor.api_key_internal",
                span,
                "`ApiKey` names an actor scheme, which has no members a program can call",
            )
            .with_note(
                "the key store's comparison is internal to the agents the compiler generates for an `ApiKey` actor; compare secrets with `Secret.matches` or `bynk.crypto`'s `Crypto.equal`",
            ),
        );
        let _ = give_up(args, ctx);
        return None;
    }
    let bytes = tys.intern(Ty::Base(BaseType::Bytes));
    for a in args {
        check_arg(a, bytes, "an `ApiKey.equal` digest", ctx);
    }
    ctx.callees.insert(expr_id, Callee::ApiKeyEqual);
    Some(tys.intern(Ty::Effect(tys.intern(Ty::Base(BaseType::Bool)))))
}

/// Type the arguments for their errors alone, and give the call no type.
fn give_up(args: &[Expr], ctx: &mut Ctx) -> Option<Option<TyId>> {
    for a in args {
        let _ = type_of(a, None, ctx);
    }
    Some(None)
}

/// A `bynk` type by name: the project's, when it is the first-party one, else the embedded one.
fn std_named(ctx: &Ctx, name: &str) -> Option<TyId> {
    let found = ctx
        .input
        .types
        .get(name)
        .filter(|d| crate::context_checks::is_std_bynk_type(name, &ctx.input.types, d))
        .cloned()
        .or_else(|| super::kernels::std_decl(name))?;
    Some(crate::checker::named_ty(&found, ctx.tys))
}

/// The local `ApiKey` actor named `name`.
fn api_key_actor<'a>(name: &str, ctx: &Ctx<'a>) -> Option<&'a bynk_syntax::ast::ActorDecl> {
    ctx.input
        .commons
        .items
        .iter()
        .filter_map(|i| i.as_actor())
        .find(|a| a.name.name == name && crate::api_key::is_api_key_actor(a))
}
