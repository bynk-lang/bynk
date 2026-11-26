//! #3939 (the API keys track, §9): lower `Actor.mint(identity, label)` on an `ApiKey` actor.
//!
//! The call becomes `__mintApiKey(peppers, …, store)`: the runtime draws the key and its digest
//! under the pepper (from `deps.__apiKey`, which the Worker's compose reads from the environment
//! and an agent call never forwards), then `store` asks the identity's `<Actor>KeyOwner` to file
//! the digest under its handle, and the key is answered once as an `ApiKeySecret`.

use bynk_ts::{TsExpr, TsParam, TsType};

use super::super::lower::Pre;
use super::super::lower_ctx::LowerCtx;
use super::super::Expr;
use crate::ts_build::{arrow_with, await_expr, call, ident, member, method_call, num_lit, paren, str_lit};

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
    let now = await_expr(call(member(member(deps_node.clone(), "Clock"), "now"), vec![]));
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
