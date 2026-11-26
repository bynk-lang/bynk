//! #3902: the Durable Object's `webSocketClose` and `webSocketError` for a `from websocket` service.
//!
//! Both run the service's `on close` through the runtime's `__webSocketClosing` (once per socket:
//! it marks the attachment closed, so an error followed by a close does not run the handler twice)
//! and complete the close handshake after it, even when the handler faults. `webSocketClose` hands
//! the handler the peer's `CloseInfo`; `webSocketError` logs the error's class name, never a frame,
//! and runs it with `CloseInfo { code: 1011, reason: "error", clean: false }`.

use super::websocket::WsOpenHost;
use crate::ts_build::{
    await_expr, block, block_arrow, bool_lit, call, expr_stmt, ident, if_, member, num_lit,
    return_, str_lit,
};
use bynk_syntax::ast::{Handler, TypeDecl};
use bynk_ts::{TsClassMethod, TsExpr, TsParam, TsStmt, TsType};
use std::collections::HashMap;
use std::sync::Arc;

/// The firing socket's minimal structural surface, widened with `serializeAttachment` so the
/// runtime can mark the attachment closed.
pub(super) const WS_CLOSE_TY: &str = "{ deserializeAttachment(): unknown; serializeAttachment(value: unknown): void; send(data: string): void; close(code?: number, reason?: string): void }";

/// Add `webSocketClose` and `webSocketError` for the service's `on close` handler.
pub(super) fn emit_close_methods(
    methods: &mut Vec<TsClassMethod>,
    host: &WsOpenHost<'_>,
    close: &Handler,
    types: &HashMap<String, Arc<TypeDecl>>,
    att_guard: &dyn Fn(&mut Vec<TsStmt>),
    deps_arg: &TsExpr,
) {
    let promise_void = TsType::named_with_args("globalThis.Promise", vec![TsType::named("void")]);
    let ws_param = || TsParam::new("ws".to_string(), Some(TsType::named(WS_CLOSE_TY)));
    let run = run_close_arrow(host, close, types, deps_arg);

    let mut on_close = Vec::new();
    att_guard(&mut on_close);
    on_close.extend(host.rebuild.stmts());
    on_close.push(expr_stmt(await_expr(call(
        ident("__webSocketClosing"),
        vec![
            ident("ws"),
            ident("__att"),
            TsExpr::object(vec![
                ("code".to_string(), ident("code")),
                ("reason".to_string(), ident("reason")),
                ("clean".to_string(), ident("wasClean")),
            ]),
            run.clone(),
        ],
    ))));
    methods.push(TsClassMethod::new(
        "webSocketClose",
        true,
        vec![
            ws_param(),
            TsParam::new("code".to_string(), Some(TsType::named("number"))),
            TsParam::new("reason".to_string(), Some(TsType::named("string"))),
            TsParam::new("wasClean".to_string(), Some(TsType::named("boolean"))),
        ],
        Some(promise_void.clone()),
        on_close,
    ));

    let mut on_error = Vec::new();
    att_guard(&mut on_error);
    on_error.extend(host.rebuild.stmts());
    on_error.push(expr_stmt(await_expr(call(
        ident("__webSocketErrored"),
        vec![ident("ws"), ident("__att"), ident("error"), run],
    ))));
    methods.push(TsClassMethod::new(
        "webSocketError",
        true,
        vec![
            ws_param(),
            TsParam::new("error".to_string(), Some(TsType::named("unknown"))),
        ],
        Some(promise_void),
        on_error,
    ));
}

/// `async (__info) => { await this.__wsClose_<service>(connection, <route values>, __info?, deps); }`:
/// the handler call, with the route values recovered positionally from the attachment and the
/// close details where the handler declares a trailing `close: CloseInfo`.
fn run_close_arrow(
    host: &WsOpenHost<'_>,
    close: &Handler,
    types: &HashMap<String, Arc<TypeDecl>>,
    deps_arg: &TsExpr,
) -> TsExpr {
    let route = bynk_check::context_checks::close_route_params(close, types);
    let mut args = vec![ident("connection")];
    for (i, p) in route.iter().enumerate() {
        args.push(super::websocket::ws_arg_cast("__att", i, &p.type_ref));
    }
    if route.len() < close.params.len() {
        args.push(ident("__info"));
    }
    args.push(deps_arg.clone());
    let method = super::websocket::ws_close_do_method_name(host.service);
    block_arrow(
        vec![TsParam::new("__info".to_string(), None)],
        true,
        vec![expr_stmt(await_expr(call(
            member(ident("this"), method),
            args,
        )))],
    )
}

/// #3903: `if (__webSocketAuthExpired(__att)) { await __webSocketClosing(ws, __att, { code: 1008,
/// reason: "authentication expired", clean: false }, <on close>); return; }`: a frame that arrives after
/// the connection's credential expired closes it with `1008` and runs `on close` (once), without running
/// `on message`. The close goes out first: the release `on close` performs closes a held connection with
/// no code, which would otherwise reach the client in place of `1008`. Checked per frame, not by a timer,
/// so an idle connection costs nothing.
pub(super) fn expiry_check(
    host: &WsOpenHost<'_>,
    close: &Handler,
    types: &HashMap<String, Arc<TypeDecl>>,
    deps_arg: &TsExpr,
) -> TsStmt {
    let closing = expr_stmt(await_expr(call(
        ident("__webSocketClosing"),
        vec![
            ident("ws"),
            ident("__att"),
            TsExpr::object(vec![
                ("code".to_string(), num_lit(1008)),
                ("reason".to_string(), str_lit("authentication expired")),
                ("clean".to_string(), bool_lit(false)),
            ]),
            run_close_arrow(host, close, types, deps_arg),
        ],
    )));
    if_(
        call(ident("__webSocketAuthExpired"), vec![ident("__att")]),
        block(vec![closing, return_(None)]),
    )
}
