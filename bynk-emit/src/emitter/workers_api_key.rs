//! #3939 (the API keys track, D3): the edge seam of a route an `ApiKey` actor guards.
//!
//! Before the body runs, in this order (track §8):
//!
//! 1. read the key from the actor's `header`, or `Authorization: Bearer`; absent or repeated is
//!    `401`;
//! 2. parse it (length, prefix, the three parts, base62, checksum); any failure is `401` with no
//!    lookup;
//! 3. digest the secret under the pepper (`HMAC-SHA-256`); a missing or bad pepper is a thrown
//!    `ApiKeyPepperFault`, a faulted request, never a quiet accept;
//! 4. ask `<Actor>Keys(handle).verify`, which compares exactly once, in constant time, against the
//!    record's digest or a dummy (D3 B), and answers the identity or `None`;
//! 5. mint the identity through the identity type's `of`; a violation is `401`;
//! 6. run the handler with the key removed from its `RequestHeaders` view (the router does that,
//!    from `consumed_credential_headers`).
//!
//! Every refusal from step 2 on is the same `HttpResult.Unauthorized`. The store call is awaited
//! with no `catch`: an unreachable agent faults the request (`500`), and `AgentBusy` answers `503`
//! with `Retry-After: 1`, as on any HTTP route, so a store fault is never an admit and never a
//! `401` (D3 C). Nothing is cached (D3 D).

use bynk_check::api_key::ApiKeySeam;
use bynk_syntax::ast::Handler;
use bynk_ts::{TsExpr, TsObjectEntry, TsParam, TsStmt, TsType};

use super::workers::{
    Failure, SeamPrincipal, handler_return, qualified_ts_type_ref, refused, secret_probe_expr,
};
use super::workers_entry::wire_ident;
use crate::emitter::http_handler_method_name_ir;
use crate::ts_build::{
    await_expr, call, const_, ident, if_, member, method_call, null_lit, return_, str_lit,
    strict_eq, strict_neq,
};

/// The seam's statements, every refusal through `failure`, and what it leaves the handler.
pub(super) fn api_key_seam_stmts(
    seam: &ApiKeySeam,
    failure: Failure,
) -> (Vec<TsStmt>, SeamPrincipal) {
    let esc = crate::emitter::escape_ts_string;
    let header = match &seam.header {
        Some(h) => str_lit(esc(h)),
        None => null_lit(),
    };
    let mut stmts = vec![
        // 1. The credential.
        const_(
            "__rawKey",
            call(
                ident("__apiKeyCredentialOf"),
                vec![ident("request"), header],
            ),
        ),
        if_(
            strict_eq(ident("__rawKey"), null_lit()),
            refused(failure, "Unauthorized"),
        ),
        // 2. Parse, with no lookup on a failure.
        const_(
            "__key",
            call(
                ident("__parseApiKey"),
                vec![ident("__rawKey"), str_lit(esc(&seam.prefix))],
            ),
        ),
        if_(
            strict_eq(ident("__key"), null_lit()),
            refused(failure, "Unauthorized"),
        ),
        // 3. The digests, under the pepper the seam alone reads.
        const_(
            "__digests",
            await_expr(call(
                ident("__apiKeyDigests"),
                vec![
                    await_expr(call(
                        ident("__loadApiKeyPeppers"),
                        vec![
                            TsExpr::object(vec![(
                                "secret".to_string(),
                                secret_probe_expr(&esc(&seam.secret)),
                            )]),
                            TsExpr::object(vec![(
                                "secret".to_string(),
                                str_lit(esc(&seam.secret)),
                            )]),
                        ],
                    )),
                    member(ident("__key"), "secret"),
                ],
            )),
        ),
    ];
    // 4. The store: one call, one constant-time comparison.
    let agent = bynk_check::api_key::keys_agent_name(&seam.actor);
    let factory = member(
        ident("handlers"),
        crate::emitter::agent_factory_name(&agent),
    );
    stmts.push(const_(
        "__owner",
        await_expr(method_call(
            call(
                factory,
                vec![member(ident("__key"), "handle"), ident("env")],
            ),
            "verify",
            vec![
                member(ident("__key"), "keyNo"),
                call(ident("__apiKeyPresented"), vec![ident("__digests")]),
                call(ident("__apiKeyFirstDigest"), vec![ident("__digests")]),
                call(ident("__apiKeyDummyDigest"), vec![]),
                ident("deps"),
            ],
        )),
    ));
    stmts.push(if_(
        strict_neq(member(ident("__owner"), "tag"), str_lit("Some")),
        refused(failure, "Unauthorized"),
    ));
    // 5. The identity, minted through its own `of`.
    stmts.push(const_(
        "__id",
        method_call(
            member(ident("handlers"), seam.identity_type.clone()),
            "of",
            vec![call(
                ident("String"),
                vec![member(ident("__owner"), "value")],
            )],
        ),
    ));
    stmts.push(if_(
        strict_eq(member(ident("__id"), "tag"), str_lit("Err")),
        refused(failure, "Unauthorized"),
    ));
    let mut extras = Vec::new();
    if seam.binder.is_some() {
        extras.push(TsObjectEntry::prop(
            "identity".to_string(),
            member(ident("__id"), "value"),
        ));
    }
    (
        stmts,
        SeamPrincipal {
            key: call(ident("String"), vec![member(ident("__id"), "value")]),
            extras,
        },
    )
}

/// #3939 (slice 2, track §12): the verify/run pair of an identity-limited `ApiKey` route. The seam's
/// own statements run in `verify`, the key store's lookup included, so a fabricated key is refused
/// there and never reaches the limiter; a verified caller is counted as `identity:<Actor>:<sub>`.
pub(super) fn api_key_identity_split(
    sname: &str,
    h: &Handler,
    method: bynk_model::IrHttpMethod,
    path: &str,
    seam: &ApiKeySeam,
) -> Vec<TsObjectEntry> {
    crate::emitter::workers::identity_split_wrapper(
        sname,
        h,
        method,
        path,
        api_key_seam_stmts(seam, &crate::emitter::workers::err_failure),
    )
}

/// The compose wrapper for a single-actor `ApiKey` HTTP handler: the seam, then the handler.
pub(super) fn emit_http_api_key_wrapper(
    sname: &str,
    h: &Handler,
    method: bynk_model::IrHttpMethod,
    path: &str,
    seam: &ApiKeySeam,
) -> TsObjectEntry {
    let method_key = http_handler_method_name_ir(method, path);
    let param_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(wire_ident(&p.name.name)))
        .collect();
    let mut params = vec![TsParam::new(
        "request".to_string(),
        Some(TsType::named("globalThis.Request")),
    )];
    params.extend(h.params.iter().map(|p| {
        TsParam::new(
            wire_ident(&p.name.name),
            Some(qualified_ts_type_ref(&p.type_ref)),
        )
    }));
    let (mut stmts, principal) = api_key_seam_stmts(seam, &|result| return_(Some(result)));
    stmts.push(handler_return(
        sname,
        &method_key,
        param_args,
        principal.extras,
    ));
    TsObjectEntry::Method {
        name: method_key,
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: None,
        doc: None,
        inline: false,
        body: stmts,
        span: None,
    }
}

/// `deps.__apiKey`: the peppers of this context's `ApiKey` actors by name, read from the
/// environment, for a handler's `Actor.mint`. `None` when it has none. `callDurableObjectMethod`
/// drops the entry from the `deps` an agent call sends, so a pepper never reaches an agent.
pub(super) fn api_key_deps_entry(table: &bynk_check::symbols::UnitTable) -> Option<TsObjectEntry> {
    let mut seams: Vec<ApiKeySeam> = table
        .actors
        .values()
        .filter_map(|a| bynk_check::api_key::api_key_seam_of(a, None))
        .collect();
    if seams.is_empty() {
        return None;
    }
    seams.sort_by(|a, b| a.actor.cmp(&b.actor));
    let esc = crate::emitter::escape_ts_string;
    let peppers = seams
        .iter()
        .map(|s| {
            (
                esc(&s.actor),
                TsExpr::object(vec![(
                    "secret".to_string(),
                    secret_probe_expr(&esc(&s.secret)),
                )]),
            )
        })
        .collect();
    Some(TsObjectEntry::prop(
        "__apiKey".to_string(),
        TsExpr::object(peppers),
    ))
}
