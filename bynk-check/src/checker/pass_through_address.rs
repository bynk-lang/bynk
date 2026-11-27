//! #3752 (pass-through HTTP, slice 3c, D20): a suite case's address of an `ANY`
//! route. The case names the method sent, then the route's pattern and its
//! parameters (`proxy.GET("/blog/*path", "/posts/x")`). `HEAD` and `OPTIONS`
//! address only an `ANY` route. The handler's `request` is built from the method,
//! the path and the inbound headers, so it takes no argument, and a trailing
//! `RequestHeaders` sets those headers below `system`, an `Authorization` among
//! them (the case shows `forward` never sends it on). The outcome is the handler's
//! `Effect[PassThrough]`.

use super::calls::{check_address_args, handler_outcome};
use super::{Callee, Ctx, HandlerAddress, TestServiceSig, Ty, TyId, type_of};
use crate::checker::ExprId;
use bynk_syntax::ast::{Expr, ExprKind, HandlerKind, HttpMethod, Ident};
use bynk_syntax::error::CompileError;

/// The methods an `ANY` route forwards (D7), and so the addresses it takes.
const SENT_METHODS: [&str; 7] = ["DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT"];

/// `svc.<METHOD>("<pattern>", …)` addressing an `ANY` route: `Some(outcome)` once it
/// is checked, `None` when the call is not one (an ordinary route at the pattern
/// answers the method first, as the router's D5 has it).
pub(super) fn check_any_address(
    sig: &TestServiceSig,
    id: &Ident,
    method: &Ident,
    args: &[Expr],
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    let Some(ExprKind::StrLit(path)) = args.first().map(|a| &a.kind) else {
        return None;
    };
    if method.name == "ANY" {
        return Some(any_refusal(id, method, path, args, ctx));
    }
    if !SENT_METHODS.contains(&method.name.as_str()) {
        return None;
    }
    let at = |any: bool| {
        sig.handlers.iter().find(|h| match &h.kind {
            HandlerKind::Http { method: m, path: p } if p == path => {
                if any {
                    *m == HttpMethod::Any
                } else {
                    *m != HttpMethod::Any && m.as_str() == method.name
                }
            }
            _ => false,
        })
    };
    if at(false).is_some() {
        return None;
    }
    let Some(handler) = at(true) else {
        if HttpMethod::from_ident(&method.name).is_some() {
            return None;
        }
        return Some(head_or_options_refusal(id, method, path, args, ctx));
    };
    let handler = handler.clone();
    let params: Vec<_> = handler
        .params
        .iter()
        .filter(|p| p.name.name != "request")
        .cloned()
        .collect();
    let _ = type_of(&args[0], None, ctx);
    let mut positional = &args[1..];
    crate::http_rest::check_rest_arguments(path, &params, positional, ctx.errors);
    let mut extra_headers = false;
    if positional.len() == params.len() + 1
        && let Some((last, rest)) = positional.split_last()
        && type_of(last, None, ctx).is_some_and(|t| matches!(&*ctx.tys.get(t), Ty::RequestHeaders))
    {
        positional = rest;
        extra_headers = true;
    }
    ctx.callees.insert(
        expr_id,
        Callee::TestService {
            service: id.name.clone(),
            address: HandlerAddress::Http {
                verb: HttpMethod::Any,
                path: Some(path.clone()),
                request: handler.params.iter().position(|p| p.name.name == "request"),
            },
            extra_headers,
        },
    );
    check_address_args(
        &id.name,
        &method.name,
        &params,
        positional,
        method.span,
        ctx,
    );
    Some(handler_outcome(&handler, ctx))
}

/// `HEAD` or `OPTIONS` on a path no `ANY` route matches: the router answers those
/// itself on an ordinary route (ADR 0162, ADR 0159), so a case can't send one.
fn head_or_options_refusal(
    id: &Ident,
    method: &Ident,
    path: &str,
    args: &[Expr],
    ctx: &mut Ctx,
) -> Option<TyId> {
    ctx.errors.push(
        CompileError::new(
            "bynk.test.service_bad_address",
            method.span,
            format!(
                "`{}.{}(\"{path}\")` sends `{}`, which only a pass-through route (`on ANY(\"{path}\")`) is addressed with",
                id.name, method.name, method.name
            ),
        )
        .with_note("the router answers `HEAD` and `OPTIONS` itself on an ordinary route; drive its `GET` instead"),
    );
    for a in args {
        let _ = type_of(a, None, ctx);
    }
    None
}

/// `svc.ANY(…)`: an `ANY` route answers whatever method the client sent, so a case
/// names that method.
fn any_refusal(
    id: &Ident,
    method: &Ident,
    path: &str,
    args: &[Expr],
    ctx: &mut Ctx,
) -> Option<TyId> {
    ctx.errors.push(
        CompileError::new(
            "bynk.test.service_bad_address",
            method.span,
            format!(
                "`{}.ANY(\"{path}\")` names no method: a case addresses a pass-through route by the method it sends",
                id.name
            ),
        )
        .with_note(format!(
            "send one of the seven it forwards, e.g. `{}.GET(\"{path}\", …)` or `{}.HEAD(\"{path}\", …)`",
            id.name, id.name
        )),
    );
    for a in args {
        let _ = type_of(a, None, ctx);
    }
    None
}
