//! The HTTP wrapper of a **multi-actor sum** handler (`by who: A | B`): the boundary the entry
//! passes the request to, which tries each member's scheme in declared order (v0.52, #3936).

use super::internal_fault::undefined_node;
use super::workers::*;
use super::workers_entry::wire_ident;
use crate::emitter::RuntimeUse;
use crate::emitter::http_handler_method_name_ir;
use crate::ts_build::{
    and_expr, as_expr, await_expr, block, call, const_, ident, if_, let_typed, member, method_call,
    new, null_lit, num_lit, return_, str_lit, strict_eq, strict_neq, typeof_expr,
};
use bynk_syntax::ast::{Handler, TypeRef};
use bynk_ts::{TsBindingName, TsExpr, TsObjectEntry, TsParam, TsStmt, TsType};

/// v0.52: the compose wrapper for a **multi-actor sum** handler (`by who: A |
/// B`). Unlike the single-actor wrappers, this one owns the *whole* boundary:
/// it reads the raw body once (when any member needs it or the handler takes a
/// `body`), tries each member's scheme in declared order, binds the first that
/// verifies into a tagged `__who`, and — fail-closed → 401 if none verifies —
/// parses the body and dispatches with `who` threaded through `deps`. The body
/// `match`es on `who`. The entry passes `request` (+ any path params); no body
/// read happens in the entry for a sum route.
#[allow(clippy::too_many_arguments, clippy::vec_init_then_push)]
pub(super) fn emit_http_sum_wrapper(
    sname: &str,
    h: &Handler,
    method: bynk_model::IrHttpMethod,
    path: &str,
    members: &[bynk_check::actors::SumMember],
    service_cap: Option<i64>,
    csrf_origins: &[String],
    runtime_use: &RuntimeUse,
) -> TsObjectEntry {
    use bynk_check::actors::SumMemberSeam;
    let method_key = http_handler_method_name_ir(method, path);
    // The wrapper takes the request first (it reads the body / headers), then
    // the path params (parsed in the entry and passed through); the `body`
    // param is parsed here, not passed in.
    // `ts_ident`-renamed so a reserved-word path param forwards as a valid binder
    // (#723); the wrapper's decl and its forwarded arg both read from this list.
    // P7.2: carries each param's own `TypeRef` alongside its name — real
    // declared types, not `unknown`, matching the same correction
    // `emit_http_wrapper`'s own path params needed (`workers_entry.rs`'s
    // dispatch validates each one against its declared type before this
    // wrapper ever sees it).
    let path_params: Vec<(String, TypeRef)> = h
        .params
        .iter()
        .filter(|p| p.name.name != "body")
        .map(|p| (wire_ident(&p.name.name), p.type_ref.clone()))
        .collect();
    let mut params: Vec<TsParam> = vec![TsParam::new(
        "request".to_string(),
        Some(TsType::named("globalThis.Request")),
    )];
    params.extend(
        path_params
            .iter()
            .map(|(n, ty)| TsParam::new(n.clone(), Some(qualified_ts_type_ref(ty)))),
    );
    let mut stmts: Vec<TsStmt> = Vec::new();

    // Read the raw body once if a member verifies over it (Signature) or the
    // handler takes a `body` param (parsed from the same bytes).
    let has_body = h.params.iter().any(|p| p.name.name == "body");
    let needs_raw = has_body || members.iter().any(|m| m.needs_body());
    // A capped route reads through the counting reader (the entry's
    // `Content-Length` fast-reject stays the cheap first check); a `@raw` route
    // keeps the octets as received for the members to verify and the handler to
    // take, and is always capped.
    let cap = h.effective_max_body(service_cap).filter(|_| needs_raw);
    let raw = has_body && h.raw_content_type().is_some();
    let bad_body = || {
        return_(Some(call(
            member(ident("HttpResult"), "BadRequest"),
            vec![str_lit("Invalid request body")],
        )))
    };
    if let Some(cap) = cap {
        stmts.push(let_typed(
            "__bytes",
            TsType::named("globalThis.Uint8Array | undefined"),
        ));
        stmts.push(TsStmt::try_catch(
            block(vec![TsStmt::assign(
                ident("__bytes"),
                await_expr(call(
                    ident("__readBodyCapped"),
                    vec![ident("request"), num_lit(cap.to_string())],
                )),
                None,
            )]),
            None::<String>,
            block(vec![bad_body()]),
            None,
        ));
        stmts.push(if_(
            strict_eq(ident("__bytes"), ident("undefined")),
            return_(Some(call(
                member(ident("HttpResult"), "PayloadTooLarge"),
                vec![str_lit(format!(
                    "request body exceeds {cap} bytes; `@limit(maxBody: …)` raises it"
                ))],
            ))),
        ));
        if !raw {
            stmts.push(const_(
                "__raw",
                method_call(
                    new(ident("globalThis.TextDecoder"), vec![]),
                    "decode",
                    vec![ident("__bytes")],
                ),
            ));
        }
    } else if needs_raw {
        stmts.push(let_typed("__raw", TsType::named("string")));
        stmts.push(TsStmt::try_catch(
            block(vec![TsStmt::assign(
                ident("__raw"),
                await_expr(method_call(ident("request"), "text", vec![])),
                None,
            )]),
            None::<String>,
            block(vec![bad_body()]),
            None,
        ));
    }
    // What a `Signature` member's HMAC covers.
    let signed = if raw { "__bytes" } else { "__raw" };

    // First-wins resolution: try each member in order; the first to verify sets
    // `__who`. A `None` (catch-all) member always succeeds.
    //
    // P7.2: deferred, not narrowed. `__who` is assigned one of two shapes across
    // the loop below — `{ tag }` or `{ tag; identity: <member's own identity type> }`
    // — and is later spread into `deps.who`, which `handlers.ts`'s own generated
    // interface types precisely per member. A correct narrowing needs a real
    // per-member discriminated union built from each `member.seam`'s
    // `identity_type` and checked against what that generated interface actually
    // expects — more than a same-line text change, and risks a `tsc --strict`
    // mismatch if guessed. Left as `any`, named here rather than silently kept.
    stmts.push(TsStmt::let_stmt(
        TsBindingName::Ident("__who".to_string()),
        Some(TsType::named("any")),
        Some(ident("undefined")),
        None,
    ));
    let sessions = members
        .iter()
        .any(|m| matches!(&m.seam, SumMemberSeam::SignedCookie(s) if s.sessions));
    if sessions {
        // #3938: the verified session of a `sessions = true` cookie member, for `Actor.expire()`.
        stmts.push(TsStmt::let_stmt(
            TsBindingName::Ident("__sess".to_string()),
            Some(TsType::named("unknown")),
            Some(ident("undefined")),
            None,
        ));
    }
    if sum_has_csrf_check(members, method) {
        stmts.push(TsStmt::let_stmt(
            TsBindingName::Ident("__csrfRefused".to_string()),
            None,
            Some(TsExpr::Lit(bynk_ts::TsLit::Bool(false))),
            None,
        ));
    }
    for member_seam in members {
        let tag = crate::emitter::escape_ts_string(&member_seam.actor_name);
        let mut inner: Vec<TsStmt> = Vec::new();
        match &member_seam.seam {
            SumMemberSeam::None => {
                inner.push(TsStmt::assign(
                    ident("__who"),
                    TsExpr::object(vec![("tag".to_string(), str_lit(tag.clone()))]),
                    None,
                ));
            }
            SumMemberSeam::Bearer {
                secret,
                identity_type,
            } => inner.extend(sum_bearer_arm(&tag, secret, identity_type)),
            SumMemberSeam::Oidc(seam) => inner.extend(sum_oidc_arm(&tag, seam)),
            SumMemberSeam::SignedCookie(seam) => inner.extend(sum_cookie_arm(
                &tag,
                seam,
                (!matches!(method, bynk_model::IrHttpMethod::Get)).then_some(csrf_origins),
            )),
            SumMemberSeam::Signature(seam) => inner.extend(sum_signature_arm(&tag, seam, signed)),
        }
        stmts.push(if_(
            strict_eq(ident("__who"), ident("undefined")),
            block(inner),
        ));
    }
    // #3936: a cross-site state-changing request that carried a session cookie was refused by
    // the cookie arm's CSRF check, and no other arm resolved: that is a `403`, not a `401`.
    let csrf_checked = sum_has_csrf_check(members, method);
    stmts.push(if_(
        strict_eq(ident("__who"), ident("undefined")),
        return_(Some(if csrf_checked {
            TsExpr::Conditional {
                test: Box::new(ident("__csrfRefused")),
                consequent: Box::new(member(ident("HttpResult"), "Forbidden")),
                alternate: Box::new(member(ident("HttpResult"), "Unauthorized")),
            }
        } else {
            member(ident("HttpResult"), "Unauthorized")
        })),
    ));

    // Parse the body param from the raw bytes already read (fail-closed → 400).
    if raw {
        // The octets, untouched: no JSON, no base64.
        stmts.push(const_("body", ident("__bytes")));
    } else if let Some(body_param) = h.params.iter().find(|p| p.name.name == "body") {
        stmts.push(let_typed("__body_json", TsType::named("__JsonValue")));
        stmts.push(TsStmt::try_catch(
            block(vec![TsStmt::assign(
                ident("__body_json"),
                as_expr(
                    method_call(ident("globalThis.JSON"), "parse", vec![ident("__raw")]),
                    TsType::named("__JsonValue"),
                ),
                None,
            )]),
            None::<String>,
            block(vec![return_(Some(call(
                member(ident("HttpResult"), "BadRequest"),
                vec![str_lit("Invalid request body")],
            )))]),
            None,
        ));
        // #1321: `super::workers_entry::deserialise_call` delegates to
        // `serialisation::deserialise_expr_via` (#1435, Arc E slice 1: a
        // real `bynk_ts::TsExpr` now, not opaque text). Its result feeds the
        // handler directly: since #1704 a `uses`-commons codec's unbranded
        // result fits the context's optionally-branded parameter type.
        let dser = super::workers_entry::deserialise_call(
            &body_param.type_ref,
            &ident("__body_json"),
            "$",
            runtime_use,
        );
        stmts.push(const_("__r_body", dser));
        stmts.push(if_(
            strict_eq(member(ident("__r_body"), "tag"), str_lit("Err")),
            return_(Some(call(
                member(ident("HttpResult"), "BadRequest"),
                vec![str_lit("Invalid request body")],
            ))),
        ));
        stmts.push(const_("body", member(ident("__r_body"), "value")));
    }
    // The handler's arguments in its declaration order: `body` (parsed above)
    // wherever it sits, and every other parameter — the path params, and
    // #2341's `headers` — as the entry passed it.
    let mut call_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| {
            if p.name.name == "body" {
                ident("body")
            } else {
                ident(wire_ident(&p.name.name))
            }
        })
        .collect();
    call_args.push(if sessions {
        session_deps_with_who()
    } else {
        deps_spread_with("who", ident("__who"))
    });
    stmts.push(return_(Some(call(
        member_chain(ident("handlers"), &[sname, &method_key]),
        call_args,
    ))));

    TsObjectEntry::method(method_key, true, params, None, stmts)
}

/// A `Bearer` sum member: an `Authorization: Bearer …` HS256 token whose `sub` constructs the
/// identity resolves `__who`; any failure leaves it unset so the next member is tried.
#[allow(clippy::vec_init_then_push)]
fn sum_bearer_arm(tag: &str, secret: &str, identity_type: &str) -> Vec<TsStmt> {
    let tag = tag.to_string();
    let mut inner: Vec<TsStmt> = Vec::new();
    inner.push(const_(
        "__authz",
        method_call(
            member(ident("request"), "headers"),
            "get",
            vec![str_lit("Authorization")],
        ),
    ));
    let mut authz_body: Vec<TsStmt> = Vec::new();
    authz_body.push(emit_secret_lookup("__secret", secret));
    let mut secret_body: Vec<TsStmt> = Vec::new();
    secret_body.push(const_(
        "__claims",
        await_expr(call(
            ident("__verifyBearerJwtHs256"),
            vec![
                method_call(ident("__authz"), "slice", vec![num_lit("7")]),
                ident("__secret"),
            ],
        )),
    ));
    secret_body.push(if_(
        strict_eq(member(ident("__claims"), "tag"), str_lit("Ok")),
        block(vec![
            const_(
                "__id",
                method_call(
                    member(ident("handlers"), identity_type.to_string()),
                    "of",
                    vec![member(member(ident("__claims"), "value"), "sub")],
                ),
            ),
            if_(
                strict_eq(member(ident("__id"), "tag"), str_lit("Ok")),
                TsStmt::assign(
                    ident("__who"),
                    TsExpr::object(vec![
                        ("tag".to_string(), str_lit(tag.clone())),
                        ("identity".to_string(), member(ident("__id"), "value")),
                        // #4344: and the verified token's claims, which the arm's second binder reads.
                        (
                            "claims".to_string(),
                            call(
                                ident("__tokenClaims"),
                                vec![member(member(ident("__claims"), "value"), "claims")],
                            ),
                        ),
                    ]),
                    None,
                ),
            ),
        ]),
    ));
    authz_body.push(if_(
        strict_eq(typeof_expr(ident("__secret")), str_lit("string")),
        block(secret_body),
    ));
    inner.push(if_(
        and_expr(
            strict_neq(ident("__authz"), null_lit()),
            method_call(ident("__authz"), "startsWith", vec![str_lit("Bearer ")]),
        ),
        block(authz_body),
    ));

    inner
}

/// A `Signature` sum member: the HMAC over the body bytes (`signed`) under the actor's secret.
#[allow(clippy::vec_init_then_push)]
fn sum_signature_arm(
    tag: &str,
    seam: &bynk_check::actors::SignatureSeam,
    signed: &str,
) -> Vec<TsStmt> {
    let tag = tag.to_string();
    let mut inner: Vec<TsStmt> = Vec::new();
    inner.push(emit_secret_lookup("__secret", &seam.secret));
    let mut secret_body: Vec<TsStmt> = Vec::new();
    let ts_expr = match &seam.timestamp_header {
        Some(th) => {
            let th = crate::emitter::escape_ts_string(th);
            secret_body.push(const_(
                "__ts",
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit(th)],
                ),
            ));
            ident("__ts")
        }
        None => null_lit(),
    };
    let tol = match seam.tolerance_secs {
        Some(n) => num_lit(n.to_string()),
        None => null_lit(),
    };
    let header = crate::emitter::escape_ts_string(&seam.header);
    secret_body.push(const_(
        "__sig_ok",
        await_expr(call(
            ident("__verifySignatureHmacSha256"),
            vec![
                ident(signed),
                ident("__secret"),
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit(header)],
                ),
                ts_expr,
                tol,
            ],
        )),
    ));
    secret_body.push(if_(
        ident("__sig_ok"),
        TsStmt::assign(
            ident("__who"),
            TsExpr::object(vec![("tag".to_string(), str_lit(tag.clone()))]),
            None,
        ),
    ));
    inner.push(if_(
        strict_eq(typeof_expr(ident("__secret")), str_lit("string")),
        block(secret_body),
    ));

    inner
}

/// #3936: whether a sum's cookie arm applies the CSRF check on this method (every method but `GET`).
fn sum_has_csrf_check(
    members: &[bynk_check::actors::SumMember],
    method: bynk_model::IrHttpMethod,
) -> bool {
    !matches!(method, bynk_model::IrHttpMethod::Get)
        && members
            .iter()
            .any(|m| matches!(m.seam, bynk_check::actors::SumMemberSeam::SignedCookie(_)))
}

/// #3936: the statements of an `Oidc` sum member: when the request carries `Authorization:
/// Bearer …` that verifies against the actor's JWKS (the same seam as a single-actor route, with
/// `trustFrom` replacing the declared roots), and whose `sub` constructs the identity, the arm
/// resolves `__who` to `{ tag, identity }`. Any failure leaves `__who` unset, so the next member
/// is tried.
fn sum_oidc_arm(tag: &str, seam: &bynk_check::actors::OidcSeam) -> Vec<TsStmt> {
    let issuer = crate::emitter::escape_ts_string(&seam.issuer);
    let audience = crate::emitter::escape_ts_string(&seam.audience);
    let jwks = crate::emitter::escape_ts_string(&seam.jwks);
    let mut body: Vec<TsStmt> = Vec::new();
    let (mut verify, trust): (Vec<TsStmt>, [TsExpr; 3]) = match &seam.trust_from {
        Some(var) => (
            vec![const_(
                "__trust",
                call(
                    ident("__oidcTrust"),
                    vec![
                        secret_probe_expr(&crate::emitter::escape_ts_string(var)),
                        str_lit(issuer),
                        str_lit(audience),
                        str_lit(jwks),
                    ],
                ),
            )],
            [
                member(ident("__trust"), "issuer"),
                member(ident("__trust"), "audience"),
                member(ident("__trust"), "jwks"),
            ],
        ),
        None => (
            Vec::new(),
            [str_lit(issuer), str_lit(audience), str_lit(jwks)],
        ),
    };
    let [issuer, audience, jwks] = trust;
    let mut checked: Vec<TsStmt> = vec![const_(
        "__claims",
        await_expr(call(
            ident("__verifyOidcJwt"),
            vec![
                method_call(ident("__authz"), "slice", vec![num_lit("7")]),
                issuer,
                audience,
                jwks,
            ],
        )),
    )];
    checked.push(if_(
        strict_eq(member(ident("__claims"), "tag"), str_lit("Ok")),
        block(vec![
            const_(
                "__id",
                method_call(
                    member(ident("handlers"), seam.identity_type.clone()),
                    "of",
                    vec![member(member(ident("__claims"), "value"), "sub")],
                ),
            ),
            if_(
                strict_eq(member(ident("__id"), "tag"), str_lit("Ok")),
                TsStmt::assign(
                    ident("__who"),
                    TsExpr::object(vec![
                        ("tag".to_string(), str_lit(tag)),
                        ("identity".to_string(), member(ident("__id"), "value")),
                    ]),
                    None,
                ),
            ),
        ]),
    ));
    if seam.trust_from.is_some() {
        // A malformed `trustFrom` refuses this arm (fail closed), as it refuses a single-actor route.
        verify.push(if_(
            strict_neq(ident("__trust"), null_lit()),
            block(checked),
        ));
    } else {
        verify.extend(checked);
    }
    body.push(const_(
        "__authz",
        method_call(
            member(ident("request"), "headers"),
            "get",
            vec![str_lit("Authorization")],
        ),
    ));
    body.push(if_(
        and_expr(
            strict_neq(ident("__authz"), null_lit()),
            method_call(ident("__authz"), "startsWith", vec![str_lit("Bearer ")]),
        ),
        block(verify),
    ));
    body
}

/// #3938: for a `sessions = true` actor, `ok` and the cookie's session id is live in the identity's
/// session agent (the check the single-actor wrapper makes after the signature); `ok` as it is
/// otherwise. A cookie without a session id, or whose session was ended, does not resolve the arm.
fn session_live_guard(seam: &bynk_check::actors::SignedCookieSeam, ok: TsExpr) -> TsExpr {
    if !seam.sessions {
        return ok;
    }
    let sid = member(member(ident("__session"), "value"), "sid");
    let agent = bynk_check::actors::sessions_agent_name(&seam.actor);
    let factory = member(
        ident("handlers"),
        crate::emitter::agent_factory_name(&agent),
    );
    let live = await_expr(method_call(
        call(factory, vec![member(ident("__id"), "value"), ident("env")]),
        "isLive",
        vec![
            sid.clone(),
            method_call(member(ident("globalThis"), "Date"), "now", vec![]),
            ident("deps"),
        ],
    ));
    and_expr(and_expr(ok, strict_neq(sid, ident("undefined"))), live)
}

/// #3936: the statements of a `SignedCookie` sum member. The arm reads the session cookie first:
/// with none (or a repeated one) it does not resolve, and it raises no refusal, so a token client
/// that sends no cookie never meets the browser's CSRF rule. With one, a state-changing request
/// (`csrf_origins` is `Some`) must pass the CSRF check (fetch metadata, then `Origin`), else the
/// arm sets `__csrfRefused` and does not resolve; then the cookie is verified and its `sub`
/// constructs the identity.
/// The statement that resolves a verified cookie member: `__who = { tag, identity }`, and, for a
/// `sessions = true` actor, `__sess = { sid, sub }` (#3938).
fn session_resolved(seam: &bynk_check::actors::SignedCookieSeam, tag: &str) -> TsStmt {
    let mut out = vec![TsStmt::assign(
        ident("__who"),
        TsExpr::object(vec![
            ("tag".to_string(), str_lit(tag)),
            ("identity".to_string(), member(ident("__id"), "value")),
        ]),
        None,
    )];
    if seam.sessions {
        let verified = member(ident("__session"), "value");
        out.push(TsStmt::assign(
            ident("__sess"),
            TsExpr::object(vec![
                ("sid".to_string(), member(verified.clone(), "sid")),
                ("sub".to_string(), member(verified, "sub")),
            ]),
            None,
        ));
    }
    if out.len() == 1 {
        // Without a session there is the one assignment, as before part B.
        return out.remove(0);
    }
    block(out)
}

/// #3938: `{ ...deps, ...(__sess === undefined ? {} : { __sessionOf: __sess }), who: __who }`, the
/// handler's deps with the verified cookie session threaded in for `Actor.expire()`.
fn session_deps_with_who() -> TsExpr {
    TsExpr::object_entries(vec![
        TsObjectEntry::Spread(ident("deps")),
        TsObjectEntry::Spread(TsExpr::Paren(Box::new(TsExpr::Conditional {
            test: Box::new(strict_eq(ident("__sess"), ident("undefined"))),
            consequent: Box::new(TsExpr::object(Vec::new())),
            alternate: Box::new(TsExpr::object(vec![(
                "__sessionOf".to_string(),
                ident("__sess"),
            )])),
        }))),
        TsObjectEntry::prop("who".to_string(), ident("__who")),
    ])
}

fn sum_cookie_arm(
    tag: &str,
    seam: &bynk_check::actors::SignedCookieSeam,
    csrf_origins: Option<&[String]>,
) -> Vec<TsStmt> {
    let previous = match &seam.previous {
        Some(var) => secret_probe_expr(&crate::emitter::escape_ts_string(var)),
        None => undefined_node(),
    };
    let verified: Vec<TsStmt> = vec![
        const_(
            "__session",
            await_expr(call(
                ident("__verifySignedCookie"),
                vec![
                    ident("__cookie"),
                    TsExpr::object(vec![
                        (
                            "secret".to_string(),
                            secret_probe_expr(&crate::emitter::escape_ts_string(&seam.secret)),
                        ),
                        ("previous".to_string(), previous),
                    ]),
                    str_lit(crate::emitter::escape_ts_string(&seam.actor)),
                    num_lit(seam.ttl_secs.to_string()),
                ],
            )),
        ),
        if_(
            strict_neq(member(ident("__session"), "tag"), str_lit("Err")),
            block(vec![
                const_(
                    "__id",
                    method_call(
                        member(ident("handlers"), seam.identity_type.clone()),
                        "of",
                        vec![member(member(ident("__session"), "value"), "sub")],
                    ),
                ),
                if_(
                    session_live_guard(
                        seam,
                        strict_eq(member(ident("__id"), "tag"), str_lit("Ok")),
                    ),
                    session_resolved(seam, tag),
                ),
            ]),
        ),
    ];
    let present: Vec<TsStmt> = match csrf_origins {
        None => verified,
        Some(origins) => {
            let origins: Vec<TsExpr> = origins
                .iter()
                .map(|o| str_lit(crate::emitter::escape_ts_string(o)))
                .collect();
            vec![TsStmt::if_else_stmt(
                call(
                    ident("__csrfAllowed"),
                    vec![ident("request"), TsExpr::array(origins)],
                ),
                block(verified),
                block(vec![TsStmt::assign(
                    ident("__csrfRefused"),
                    TsExpr::Lit(bynk_ts::TsLit::Bool(true)),
                    None,
                )]),
                None,
            )]
        }
    };
    vec![
        const_(
            "__cookie",
            call(
                ident("__sessionCookieOf"),
                vec![
                    ident("request"),
                    str_lit(crate::emitter::escape_ts_string(&seam.cookie)),
                ],
            ),
        ),
        if_(strict_neq(ident("__cookie"), null_lit()), block(present)),
    ]
}
