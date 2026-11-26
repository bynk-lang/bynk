//! The credential a `from websocket` upgrade verifies in the Worker, before the request reaches the
//! Durable Object (#3900 Bearer, #3904 `Oidc` and `SignedCookie`): fail closed, and the verified
//! identity and expiry are what the Worker forwards.

use super::workers::*;
use crate::ts_build::{
    await_expr, call, const_, ident, if_, member, method_call, not_expr, return_, str_lit,
    strict_eq, strict_neq, typeof_expr,
};
use bynk_check::actors::{BearerSeam, OidcSeam, SignedCookieSeam};
use bynk_syntax::ast::{ActorDecl, Handler, ServiceDecl};
use bynk_ts::{TsExpr, TsStmt};
use std::collections::HashMap;

/// The scheme of a lone `from websocket` actor and what its verification reads.
pub(super) enum WsCredential {
    /// `["bynk.bearer", token]` in `Sec-WebSocket-Protocol`.
    Bearer(BearerSeam),
    /// `["bynk.oidc", token]` in `Sec-WebSocket-Protocol`.
    Oidc(OidcSeam),
    /// The session cookie, behind an exact `Origin` check against the service's `cors` origins.
    Cookie {
        seam: SignedCookieSeam,
        origins: Vec<String>,
    },
}

impl WsCredential {
    /// The credential of `on open`'s lone actor; `None` for a `None`-scheme actor or an actor sum.
    pub(super) fn for_handler(
        h: &Handler,
        service: &ServiceDecl,
        actors: &HashMap<String, ActorDecl>,
    ) -> Option<Self> {
        if h.by_clause.as_ref().is_some_and(|by| by.is_sum()) {
            return None;
        }
        if let Some(seam) = bynk_check::actors::bearer_seam_for(h, actors) {
            return Some(Self::Bearer(seam));
        }
        if let Some(seam) = bynk_check::actors::oidc_seam_for(h, actors) {
            return Some(Self::Oidc(seam));
        }
        bynk_check::actors::signed_cookie_seam_for(h, actors).map(|seam| Self::Cookie {
            seam,
            origins: csrf_origins(service),
        })
    }
}

/// What verifying a credential leaves for the rest of the upgrade.
pub(super) struct WsVerified {
    pub(super) stmts: Vec<TsStmt>,
    /// Whether `__id` holds the identity the actor's binder names.
    pub(super) has_identity: bool,
    /// The credential's expiry in seconds, forwarded so the Durable Object closes at it (#3903).
    pub(super) exp: TsExpr,
}

fn unauthorized() -> TsStmt {
    return_(Some(new_response(vec![
        str_lit("Unauthorized"),
        status_object("401"),
    ])))
}

/// `const __id = handlers.<Identity>.of(<sub>)`, refused `401` on a refinement violation.
fn identity_stmts(identity_type: &str, sub: TsExpr) -> [TsStmt; 2] {
    [
        const_(
            "__id",
            method_call(
                member(ident("handlers"), identity_type.to_string()),
                "of",
                vec![sub],
            ),
        ),
        if_(
            strict_eq(member(ident("__id"), "tag"), str_lit("Err")),
            unauthorized(),
        ),
    ]
}

pub(super) fn ws_credential_stmts(credential: &WsCredential) -> WsVerified {
    match credential {
        WsCredential::Bearer(seam) => bearer_stmts(seam),
        WsCredential::Oidc(seam) => oidc_stmts(seam),
        WsCredential::Cookie { seam, origins } => cookie_stmts(seam, origins),
    }
}

fn bearer_stmts(seam: &BearerSeam) -> WsVerified {
    let secret = crate::emitter::escape_ts_string(&seam.secret);
    // #3900: the token is the subprotocol after `bynk.bearer`; the old `[token]` form is a `400`
    // naming the form that works, and no token at all is a `401`.
    let mut stmts = bearer_offer_stmts(true, "bynk.bearer");
    stmts.push(const_("__token", member(ident("__offer"), "token")));
    // The hosting context's `Env` carries the DO binding (a non-empty object type), so the secret
    // probe casts through `unknown` to index it.
    stmts.push(const_("__secret", secret_probe_expr(&secret)));
    stmts.push(if_(
        strict_neq(typeof_expr(ident("__secret")), str_lit("string")),
        unauthorized(),
    ));
    stmts.push(const_(
        "__claims",
        await_expr(call(
            ident("__verifyBearerJwtHs256"),
            vec![ident("__token"), ident("__secret")],
        )),
    ));
    stmts.push(if_(
        strict_eq(member(ident("__claims"), "tag"), str_lit("Err")),
        unauthorized(),
    ));
    // A refinement actor's authorisation invariant: scheme verified (401 above), a failed claim
    // predicate is 403, checked against verified claims.
    if let Some(pred) = &seam.authorization {
        stmts.push(if_(
            super::claim_predicate::claim_predicate_denied(pred),
            return_(Some(new_response(vec![
                str_lit("Forbidden"),
                status_object("403"),
            ]))),
        ));
    }
    let sub = member(member(ident("__claims"), "value"), "sub");
    let has_identity = seam.binder.is_some();
    if has_identity {
        stmts.extend(identity_stmts(&seam.identity_type, sub));
    }
    WsVerified {
        stmts,
        has_identity,
        exp: claims_exp(),
    }
}

fn oidc_stmts(seam: &OidcSeam) -> WsVerified {
    // #3904: the token is the subprotocol after `bynk.oidc`, offered as `Bearer`'s is.
    let mut stmts = bearer_offer_stmts(true, "bynk.oidc");
    stmts.push(const_("__token", member(ident("__offer"), "token")));
    let (verify, _) = oidc_verify_stmts(seam, ident("__token"), &|_| unauthorized());
    stmts.extend(verify);
    WsVerified {
        stmts,
        has_identity: seam.binder.is_some(),
        exp: claims_exp(),
    }
}

fn cookie_stmts(seam: &SignedCookieSeam, origins: &[String]) -> WsVerified {
    // #3904: a browser attaches the cookie to a cross-site upgrade, and a WebSocket is not bound by
    // the same-origin policy, so the `Origin` must be exactly one of the service's `cors` origins.
    // A missing `Origin`, or one not listed, is `403` before the cookie is read.
    let listed: Vec<TsExpr> = origins
        .iter()
        .map(|o| str_lit(crate::emitter::escape_ts_string(o)))
        .collect();
    let mut stmts = vec![if_(
        not_expr(call(
            ident("__webSocketOriginAllowed"),
            vec![
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit("Origin")],
                ),
                TsExpr::array(listed),
            ],
        )),
        return_(Some(new_response(vec![
            str_lit("Forbidden"),
            status_object("403"),
        ]))),
    )];
    // A `GET` upgrade skips the HTTP cross-site check, which the `Origin` check above replaces.
    let (verify, _) = signed_cookie_seam_stmts(bynk_model::IrHttpMethod::Get, seam, &[], &|_| {
        unauthorized()
    });
    stmts.extend(verify);
    WsVerified {
        stmts,
        has_identity: seam.binder.is_some(),
        exp: member(member(ident("__session"), "value"), "exp"),
    }
}
