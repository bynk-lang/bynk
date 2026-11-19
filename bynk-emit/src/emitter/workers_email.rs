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

use bynk_syntax::ast::ServiceProtocol;
use bynk_ts::{TsExpr, TsParam, TsStmt, TsType};

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
        .filter_map(|(name, s)| match &s.protocol {
            ServiceProtocol::Email { address } => Some(EmailRoute {
                service: name.clone(),
                address: address.clone(),
            }),
            _ => None,
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

/// The runtime names the entry imports.
pub(super) const RUNTIME_IMPORTS: [&str; 2] = ["__handleEmail", "type __InboundMessage"];

/// `async email(message, env, ctx) { … }`: build the surface, then hand the
/// message to the runtime with one route per service.
pub(super) fn emit_email_handler(
    routes: &[EmailRoute],
    exec_params: &[TsParam],
    compose_args: &[TsExpr],
) -> TsObjectEntry {
    let mut params = vec![
        TsParam {
            name: "message".to_string(),
            ty: Some(TsType::named("__InboundMessage")),
            optional: false,
        },
        TsParam {
            name: "env".to_string(),
            ty: Some(TsType::named("Env")),
            optional: false,
        },
    ];
    params.extend(exec_params.iter().cloned());
    // The routes are built inside a thunk, so composing the surface is guarded
    // with everything else: nothing escapes `email()` as a throw.
    let args: Vec<String> = compose_args.iter().map(bynk_ts::print_expr).collect();
    let mut text = format!(
        "    await __handleEmail(message, () => {{\n      const surface = compose({});\n      return [\n",
        args.join(", ")
    );
    for r in routes {
        let method = crate::emitter::queue_handler_method_name(&r.service, 0);
        text.push_str(&format!(
            "        {{ address: {:?}, handle: (mail, sender) => surface.{method}(mail, sender) }},\n",
            r.address
        ));
    }
    text.push_str("      ];\n    });\n");
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
        body: vec![TsStmt::raw(text, None)],
    }
}
