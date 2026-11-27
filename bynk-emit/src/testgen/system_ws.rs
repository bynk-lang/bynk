//! The WebSocket drivers of a `system` suite's target (#3901).
//!
//! A `system` case drives a `from websocket` service by address (`svc.open(tc, …)`, `svc.message(tc, frame)`,
//! `svc.close(tc)`) through a real upgrade against the participant Worker, so the handshake, the Worker-side
//! authentication, the routing to the Durable Object, the attachment and the frame decoding are all exercised.
//! The client half is the test-runtime fragment `ws_client.ts` (`__bynkWsOpen` and its two siblings); this
//! module emits, per service, the thin drivers that name what is specific to it: the route's parameters, the
//! credential the service's actor reads, and the codecs of its `in` and `out` frames.

use super::*;
use crate::emitter::ctx::SystemWsService;
use crate::emitter::emit::test_scaffold_ns as test_ns;
use bynk_ts::{TsExpr, TsParam, TsStmt, TsType};

/// The drivers of one WebSocket service, and the secret its actor signs with (to be set in the test process).
pub(super) struct WsDrivers {
    pub(super) code: Vec<TsStmt>,
    pub(super) service: SystemWsService,
    pub(super) secret: Option<String>,
}

fn is_plain_bytes(t: &bynk_syntax::ast::TypeRef) -> bool {
    matches!(
        t,
        bynk_syntax::ast::TypeRef::Base(bynk_syntax::ast::BaseType::Bytes, _)
    )
}

/// `__sysdrive_ws_<what>_<service>`.
fn driver_name(what: &str, service: &str) -> String {
    ["__sysdrive_ws_", what, "_", service].concat()
}

/// The drivers of the `from websocket` service `sname`, or `None` when it has no `on open` handler.
///
/// - `__sysdrive_ws_open_<S>(tc, ...route, sub)` performs the upgrade with a valid credential.
/// - `__sysdrive_ws_<variant>_<S>(tc, ...route, sub)` performs it with the bad one `variant` names; `noauth`
///   offers none.
/// - `__sysdrive_ws_message_<S>(tc, frame)` and `__sysdrive_ws_close_<S>(tc, info)` write on the open connection.
///
/// An actor this driver cannot yet present (`Oidc`, `SignedCookie`, an actor sum) gets an `open` driver that
/// fails the case with a message saying so, never an upgrade the Worker would refuse for the wrong reason.
pub(super) fn ws_service_drivers(
    sname: &str,
    decl: &bynk_syntax::ast::ServiceDecl,
    target: &str,
    table: &UnitTable,
    runtime_use: &crate::emitter::RuntimeUse,
    secret_read_of: &dyn Fn(&str) -> TsExpr,
) -> Option<WsDrivers> {
    use bynk_model::{IrHandlerKind, lower_handler_kind_ir};
    let bynk_syntax::ast::ServiceProtocol::WebSocket { in_type, out_type } = &decl.protocol else {
        return None;
    };
    let open = decl
        .handlers
        .iter()
        .find(|h| matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Open))?;
    let ns = test_ns(target);
    let type_ns = ns_qualifier(&ns);
    let binding = crate::emitter::bindings::service_binding(target).name;
    let mut code: Vec<TsStmt> = Vec::new();
    let mut service = SystemWsService::default();
    let mut secret = None;

    let params: Vec<TsParam> = std::iter::once(TsParam::new("tc", Some(any_type())))
        .chain(open.params.iter().map(|p| {
            TsParam::new(
                crate::emitter::ts_ident(&p.name.name),
                Some(driver_param_type(&p.type_ref)),
            )
        }))
        .chain([TsParam::new("__sub", Some(TsType::named("string")))])
        .collect();
    // `[["roomId", roomId], …]`: the route values by the parameter names the Worker reads from the query.
    let route = TsExpr::array(
        open.params
            .iter()
            .map(|p| {
                TsExpr::array(vec![
                    str_lit(p.name.name.clone()),
                    ident(crate::emitter::ts_ident(&p.name.name)),
                ])
            })
            .collect(),
    );
    let decode: TsExpr = if is_plain_bytes(out_type) {
        TsExpr::Arrow {
            params: vec![TsParam::new("w", Some(TsType::named("unknown")))],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(TsExpr::Paren(
                Box::new(TsExpr::object(vec![
                    ("tag".to_string(), str_lit("Ok")),
                    (
                        "value".to_string(),
                        TsExpr::As {
                            expr: Box::new(ident("w")),
                            ty: TsType::named("never"),
                        },
                    ),
                ])),
            )))),
        }
    } else {
        crate::emitter::serialisation::deserialise_ref_via(out_type, &type_ns, runtime_use)
    };
    // The upgrade, handed to `__bynkWsOpen`: `<binding>.fetch(request)` is the Worker's own `fetch`.
    let open_call = |protocols: TsExpr| {
        ret(call(
            ident("__bynkWsOpen"),
            vec![
                ident("tc"),
                TsExpr::Arrow {
                    params: vec![TsParam::new("__req", None)],
                    is_async: false,
                    generics: Vec::new(),
                    return_type: None,
                    body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(method_call(
                        member(member(ident("__h"), "env"), binding.clone()),
                        "fetch",
                        vec![ident("__req")],
                    )))),
                },
                TsExpr::object(vec![
                    ("service".to_string(), str_lit(sname.to_string())),
                    ("params".to_string(), route.clone()),
                    ("protocols".to_string(), protocols),
                    ("origin".to_string(), ident("undefined")),
                ]),
                decode.clone(),
            ],
        ))
    };
    let pair = |token: TsExpr| {
        TsExpr::template_lit(
            vec!["bynk.bearer, ".to_string(), String::new()],
            vec![token],
        )
    };

    match bynk_check::actors::bearer_seam_for(open, &table.actors) {
        Some(seam) => {
            secret = Some(seam.secret.clone());
            let secret_read = secret_read_of(&seam.secret);
            let mint = system_request::BearerMint::of(seam.claims.clone(), secret_read_of);
            let mut claim_entries = vec![
                bynk_ts::TsObjectEntry::prop("sub", ident("__sub")),
                bynk_ts::TsObjectEntry::prop(
                    "exp",
                    binary(bynk_ts::TsBinaryOp::Add, now_secs(), num_lit("3600")),
                ),
            ];
            claim_entries.extend(system_request::bearer_claim_entries(&mint));
            let token = |claims: TsExpr, secret: TsExpr| {
                const_(
                    "__token",
                    await_expr(call(ident("__bynkSignHs256"), vec![claims, secret])),
                )
            };
            code.push(async_driver(
                driver_name("open", sname),
                params.clone(),
                vec![
                    harness_const(),
                    token(TsExpr::object_entries(claim_entries), secret_read.clone()),
                    open_call(pair(ident("__token"))),
                ],
            ));
            // `by Nobody`: no `Sec-WebSocket-Protocol` at all, so the Worker answers `401`.
            code.push(async_driver(
                driver_name("noauth", sname),
                params.clone(),
                vec![harness_const(), open_call(ident("undefined"))],
            ));
            service.variants.push("noauth");
            for (name, secret, sign) in
                system_request::bearer_variants(&secret_read, &seam.claims, true)
            {
                let claims = system_request::bearer_variant_claims(
                    name,
                    binary(sign, now_secs(), num_lit("3600")),
                    &mint,
                );
                code.push(async_driver(
                    driver_name(name, sname),
                    params.clone(),
                    vec![
                        harness_const(),
                        token(claims, secret),
                        open_call(pair(ident("__token"))),
                    ],
                ));
                service.variants.push(name);
            }
        }
        None if open.by_clause.as_ref().is_none() || is_public(open, &table.actors) => {
            // A public service: the upgrade offers no credential.
            code.push(async_driver(
                driver_name("open", sname),
                params.clone(),
                vec![harness_const(), open_call(ident("undefined"))],
            ));
        }
        None => {
            code.push(async_driver(
                driver_name("open", sname),
                params.clone(),
                vec![TsStmt::throw_stmt(
                    TsExpr::New {
                        callee: Box::new(ident("globalThis.Error")),
                        args: vec![str_lit(
                            [
                                "`",
                                sname,
                                ".open` at `system`: this service's actor is not one a `system` case presents yet (only `Bearer` and public services are); drive it at `unit` or `integration`",
                            ]
                            .concat(),
                        )],
                    },
                    None,
                )],
            ));
        }
    }

    let frame_in = if is_plain_bytes(in_type) {
        ident("frame")
    } else {
        crate::emitter::serialisation::serialise_expr_via(
            in_type,
            &TsExpr::Ident("frame".to_string()),
            &type_ns,
            runtime_use,
        )
    };
    code.push(async_driver(
        driver_name("message", sname),
        vec![
            TsParam::new("tc", Some(any_type())),
            TsParam::new("frame", Some(any_type())),
        ],
        vec![ret(call(
            ident("__bynkWsMessage"),
            vec![ident("tc"), str_lit(sname.to_string()), frame_in],
        ))],
    ));
    code.push(async_driver(
        driver_name("close", sname),
        vec![
            TsParam::new("tc", Some(any_type())),
            TsParam::new("info", Some(any_type())),
        ],
        vec![ret(call(
            ident("__bynkWsClose"),
            vec![ident("tc"), str_lit(sname.to_string()), ident("info")],
        ))],
    ));
    Some(WsDrivers {
        code,
        service,
        secret,
    })
}

/// Whether the handler's actor is a public one (`auth = None`), whose upgrade offers no credential.
fn is_public(
    h: &bynk_syntax::ast::Handler,
    actors: &HashMap<String, bynk_syntax::ast::ActorDecl>,
) -> bool {
    h.by_clause
        .as_ref()
        .and_then(|by| actors.get(&by.primary().name))
        .and_then(|a| a.auth.as_ref())
        .is_some_and(|auth| auth.name == "None")
}

/// The drivers of every WebSocket service of the target, appended to `routes`, and the secrets their actors
/// sign with added to `secrets`; the services driven are returned, for the lowering of a case's address.
pub(super) fn emit_ws_drivers(
    (target, table): (&str, &UnitTable),
    runtime_use: &crate::emitter::RuntimeUse,
    secret_read_of: &dyn Fn(&str) -> TsExpr,
    (secrets, routes): (&mut std::collections::BTreeSet<String>, &mut Vec<TsStmt>),
) -> HashMap<String, SystemWsService> {
    let mut driven = HashMap::new();
    let mut names: Vec<&String> = table.services.keys().collect();
    names.sort();
    for sname in names {
        let Some(drivers) = ws_service_drivers(
            sname,
            &table.services[sname],
            target,
            table,
            runtime_use,
            secret_read_of,
        ) else {
            continue;
        };
        secrets.extend(drivers.secret);
        routes.extend(drivers.code);
        driven.insert(sname.clone(), drivers.service);
    }
    driven
}

/// The client half of a WebSocket address (`ws_client.ts`), for a target with a service to drive.
pub(super) fn ws_client_splice(driven: &HashMap<String, SystemWsService>) -> Option<TsStmt> {
    if driven.is_empty() {
        return None;
    }
    let mut client = TestRuntimeModule::WsClient.splice();
    client.no_blank_before = true;
    Some(client)
}
