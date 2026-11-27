//! #3939 (the API keys track, §9): lower `Actor.mint(identity, label)` on an `ApiKey` actor.
//!
//! The call becomes `__mintApiKey(peppers, …, store)`: the runtime draws the key and its digest
//! under the pepper (from `deps.__apiKey`, which the Worker's compose reads from the environment
//! and an agent call never forwards), then `store` asks the identity's `<Actor>KeyOwner` to file
//! the digest under its handle, and the key is answered once as an `ApiKeySecret`.

use bynk_ts::{TsExpr, TsParam, TsType};

use super::super::Expr;
use super::super::lower::Pre;
use super::super::lower_ctx::LowerCtx;
use crate::ts_build::{
    arrow_with, await_expr, call, ident, member, method_call, num_lit, paren, str_lit,
};

/// `mint`'s lowering, or `None` when the call is not an `ApiKey` actor's `mint`.
pub(in crate::emitter) fn lower_api_key_mint(
    e: &Expr,
    actor: &str,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<TsExpr> {
    let seam = cx.commons().commons.items.iter().find_map(|i| {
        i.as_actor()
            .filter(|a| a.name.name == actor)
            .and_then(|a| bynk_check::api_key::api_key_seam_of(a, None))
    })?;
    // The call's checked `Effect[Result[ApiKeySecret, TooManyKeys]]`, peeled to the `Result`.
    let result_ty = match cx.commons().expr_ty(e.id).as_deref() {
        Some(bynk_check::checker::Ty::Effect(inner)) => *inner,
        _ => return None,
    };
    let result_ts = super::super::ts_types::ts_ty_to_ts_type(result_ty, cx.ty_render());
    let identity = pre.lower_node(args.first()?, cx);
    let label = pre.lower_node(args.get(1)?, cx);
    let deps_node = if cx.in_test_case() {
        ident("deps")
    } else {
        cx.cap_deps_node()
    };
    let owner = bynk_check::api_key::key_owner_agent_name(actor);
    cx.record_agent_call(&owner, "mint");
    let instance = cx.agent_construct_node(&owner, ident("__sub"));
    let now = await_expr(call(
        member(member(deps_node.clone(), "Clock"), "now"),
        vec![],
    ));
    // The store step: `KeyOwner(identity).mint(handle, keyNo, digest, kid, label, now, cap)`.
    let req = |f: &str| member(ident("__req"), f);
    let store = arrow_with(
        vec![TsParam::new("__req".to_string(), None)],
        true,
        method_call(
            instance,
            "mint",
            vec![
                req("handle"),
                req("keyNo"),
                req("digest"),
                req("kid"),
                ident("__label"),
                ident("__now"),
                req("until"),
                req("live"),
                num_lit(bynk_project::api_key_agents::KEY_CAP),
                deps_node.clone(),
            ],
        ),
    );
    let esc = crate::emitter::escape_ts_string;
    let mint = call(
        crate::ts_build::type_app(ident("__mintApiKey"), vec![result_ts]),
        vec![
            call(
                ident("__apiKeyPeppersOf"),
                vec![deps_node, str_lit(esc(actor))],
            ),
            str_lit(esc(&seam.secret)),
            str_lit(esc(&seam.prefix)),
            ident("__label"),
            ident("__now"),
            num_lit(bynk_project::api_key_agents::KEY_CAP),
            store,
            mint_options(&seam),
        ],
    );
    Some(call(
        paren(arrow_with(
            vec![
                TsParam::new(
                    "__sub".to_string(),
                    Some(TsType::named(seam.identity_type.clone())),
                ),
                TsParam::new("__label".to_string(), Some(TsType::named("string"))),
                TsParam::new("__now".to_string(), Some(TsType::named("number"))),
            ],
            true,
            mint,
        )),
        vec![identity, label, now],
    ))
}

/// `ApiKey.equal(a, b)` in a generated key store: `__apiKeyEqual(a, b)`, the runtime's one
/// constant-time comparison (the function `bynk.crypto`'s `Crypto.equal` calls).
pub(in crate::emitter) fn lower_api_key_equal(
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<TsExpr> {
    let a = pre.lower_node(args.first()?, cx);
    let b = pre.lower_node(args.get(1)?, cx);
    Some(call(ident("__apiKeyEqual"), vec![a, b]))
}

/// #3939 (slice 2): `list`, `revoke` or `revokeAll` on an `ApiKey` actor, as calls to the identity's
/// `<Actor>KeyOwner` (and, for `list`, its current key store). None needs the pepper.
///
/// - `list(identity)`: `__apiKeyList(prefix, await owner.current(), (h) => keys(h).labelsOf(),
///   (h) => keys(h).createdOf())`;
/// - `revoke(identity, id)`: `__apiKeyRevoke(prefix, id, (h, k) => owner.revoke(h, k))`, `false`
///   for an id that is not this actor's;
/// - `revokeAll(identity)`: `owner.revokeAll(__newApiKeyHandle())`, which empties and retires the
///   current store and takes the fresh handle.
pub(in crate::emitter) fn lower_api_key_member(
    e: &Expr,
    (actor, member): (&str, &str),
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<TsExpr> {
    let seam = cx.commons().commons.items.iter().find_map(|i| {
        i.as_actor()
            .filter(|a| a.name.name == actor)
            .and_then(|a| bynk_check::api_key::api_key_seam_of(a, None))
    })?;
    let identity = pre.lower_node(args.first()?, cx);
    let deps_node = if cx.in_test_case() {
        ident("deps")
    } else {
        cx.cap_deps_node()
    };
    let owner_agent = bynk_check::api_key::key_owner_agent_name(actor);
    let keys_agent = bynk_check::api_key::keys_agent_name(actor);
    let owner = cx.agent_construct_node(&owner_agent, ident("__sub"));
    let prefix = str_lit(crate::emitter::escape_ts_string(&seam.prefix));
    let body = match member {
        "list" => {
            cx.record_agent_call(&owner_agent, "current");
            cx.record_agent_call(&keys_agent, "labelsOf");
            cx.record_agent_call(&keys_agent, "createdOf");
            let read = |cx: &mut LowerCtx, op: &str| {
                let store = cx.agent_construct_node(&keys_agent, ident("__h"));
                arrow_with(
                    vec![TsParam::new("__h".to_string(), None)],
                    false,
                    method_call(store, op, vec![deps_node.clone()]),
                )
            };
            let list_ty = match cx.commons().expr_ty(e.id).as_deref() {
                Some(bynk_check::checker::Ty::Effect(inner)) => *inner,
                _ => return None,
            };
            let list_ts = super::super::ts_types::ts_ty_to_ts_type(list_ty, cx.ty_render());
            let labels = read(cx, "labelsOf");
            let created = read(cx, "createdOf");
            call(
                crate::ts_build::type_app(ident("__apiKeyList"), vec![list_ts]),
                vec![
                    prefix,
                    await_expr(method_call(owner, "current", vec![deps_node.clone()])),
                    labels,
                    created,
                ],
            )
        }
        "revoke" => {
            cx.record_agent_call(&owner_agent, "revoke");
            let id = pre.lower_node(args.get(1)?, cx);
            call(
                ident("__apiKeyRevoke"),
                vec![
                    prefix,
                    id,
                    arrow_with(
                        vec![
                            TsParam::new("__h".to_string(), None),
                            TsParam::new("__k".to_string(), None),
                        ],
                        false,
                        method_call(
                            owner,
                            "revoke",
                            vec![ident("__h"), ident("__k"), deps_node.clone()],
                        ),
                    ),
                ],
            )
        }
        "revokeAll" => {
            cx.record_agent_call(&owner_agent, "revokeAll");
            method_call(
                owner,
                "revokeAll",
                vec![call(ident("__newApiKeyHandle"), vec![]), deps_node.clone()],
            )
        }
        _ => return None,
    };
    Some(call(
        paren(arrow_with(
            vec![TsParam::new(
                "__sub".to_string(),
                Some(TsType::named(seam.identity_type.clone())),
            )],
            true,
            body,
        )),
        vec![identity],
    ))
}

/// #3939 (slice 3): `{ ttlMs, previous }`, what `__mintApiKey` reads beyond the pepper: the key's
/// lifetime, and the `previous` variable's name when the actor rotates.
pub(crate) fn mint_options(seam: &bynk_check::api_key::ApiKeySeam) -> TsExpr {
    let esc = crate::emitter::escape_ts_string;
    let mut entries = vec![(
        "ttlMs".to_string(),
        seam.ttl_ms
            .map_or_else(crate::ts_build::null_lit, |ms| num_lit(ms)),
    )];
    if let Some(previous) = &seam.previous {
        entries.push(("previous".to_string(), str_lit(esc(previous))));
    }
    TsExpr::object(entries)
}
