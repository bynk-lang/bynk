//! The Worker's `email()` export (#3328, slice 2, ADR 0805).
//!
//! Cloudflare delivers inbound mail to `email(message, env, ctx)`. The export is
//! a short call into the runtime's `handleEmail` (`runtime/src/email.ts`), which
//! does the security-bearing work (dispatch by envelope recipient, parsing, the
//! DMARC verdict, applying the handler's verdict); this module only lists the
//! routes: each `from email("…")` service's address and its compose wrapper. A
//! reply's `EmailMessage` (`cloudflare:email`) is loaded by the runtime when one
//! is built.
//!
//! Kept out of `workers_entry.rs`, whose entry builder is near its length
//! ceiling.

use crate::ts_build::ident;
use bynk_syntax::ast::ServiceProtocol;
use bynk_ts::{TsArrowBody, TsBindingName, TsExpr, TsLit, TsParam, TsStmt, TsType};

use bynk_check::symbols::UnitTable;
use bynk_ts::TsObjectEntry;

/// One `from email(…)` service the Worker routes to.
pub(super) struct EmailRoute {
    pub service: String,
    /// The address as declared (`support@shop.example` or `*@shop.example`).
    pub address: String,
}

/// The routes of every `from email` service in `table`, exact addresses before
/// catch-alls and each group in address order, so the output is deterministic.
pub(super) fn collect_routes(table: &UnitTable) -> Vec<EmailRoute> {
    let mut routes: Vec<EmailRoute> = table
        .services
        .iter()
        .filter_map(|(name, s)| {
            let ServiceProtocol::Email { address } = &s.protocol else {
                return None;
            };
            Some(EmailRoute {
                service: name.clone(),
                address: address.clone(),
            })
        })
        .collect();
    routes.sort_by(|a, b| {
        a.address
            .starts_with("*@")
            .cmp(&b.address.starts_with("*@"))
            .then_with(|| a.address.cmp(&b.address))
            .then_with(|| a.service.cmp(&b.service))
    });
    routes
}

/// `async email(message, env, ctx) { … }`: build the surface, then hand the
/// message to the runtime with one route per service.
pub(super) fn emit_email_handler(
    routes: &[EmailRoute],
    exec_params: &[TsParam],
    compose_args: &[TsExpr],
) -> TsObjectEntry {
    let mut params = vec![
        TsParam::new(
            "message".to_string(),
            Some(TsType::named("__InboundMessage")),
        ),
        TsParam::new("env".to_string(), Some(TsType::named("Env"))),
    ];
    params.extend(exec_params.iter().cloned());
    // The routes are built inside a thunk, so composing the surface is guarded
    // with everything else: nothing escapes `email()` as a throw.
    let route = |r: &EmailRoute| {
        let method = crate::emitter::queue_handler_method_name(&r.service, 0);
        let handle = TsExpr::Arrow {
            params: vec![TsParam::new("mail", None), TsParam::new("sender", None)],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Call {
                callee: Box::new(TsExpr::Member {
                    object: Box::new(ident("surface")),
                    property: method,
                }),
                args: vec![ident("mail"), ident("sender")],
            }))),
        };
        TsExpr::object(vec![
            (
                "address".to_string(),
                TsExpr::Lit(TsLit::Str(r.address.clone())),
            ),
            ("handle".to_string(), handle),
        ])
    };
    let thunk = TsExpr::Arrow {
        params: Vec::new(),
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Multiline(vec![
            TsStmt::const_stmt(
                TsBindingName::Ident("surface".to_string()),
                None,
                TsExpr::Call {
                    callee: Box::new(ident("compose")),
                    args: compose_args.to_vec(),
                },
                None,
            ),
            TsStmt::return_stmt(
                Some(TsExpr::multiline_array(routes.iter().map(route).collect())),
                None,
            ),
        ])),
    };
    let handled = TsExpr::Await(Box::new(TsExpr::Call {
        callee: Box::new(ident("__handleEmail")),
        args: vec![ident("message"), thunk],
    }));
    TsObjectEntry::Method {
        name: "email".to_string(),
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: Some(TsType::named_with_args(
            "globalThis.Promise",
            vec![TsType::named("void")],
        )),
        doc: None,
        inline: false,
        body: vec![TsStmt::expr_stmt(handled, None)],
        span: None,
    }
}
