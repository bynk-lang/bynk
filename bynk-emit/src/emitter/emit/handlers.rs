//! Handler method names and labels, and the handler-body items that services, agents and free
//! functions share: the `events` declaration, the result closure, the flush and release steps.

use crate::emitter::lower_stmt::{Body, Item};
use crate::ts_build::ident;
use bynk_check::checker::TypedCommons;
use bynk_model::{
    IrHandlerKind, IrHttpMethod, ProtocolIr, lower_handler_kind_ir, lower_protocol_ir_from_commons,
};
use bynk_syntax::ast::{CommonsItem, Param};

/// Synthesise a TypeScript-safe method name for an `on http METHOD path`
/// handler. The result is used both as the key on the service object and
/// as the identifier the Worker fetch handler invokes. Path parameter
/// segments (`:name`) become `Param_name` to remain distinct from literal
/// segments. (v0.9 §5.3)
///
/// Slice 1 of `#1542` (the IR cutover, `design/archive/retired-tracks.md`): this used to
/// have an AST-typed `HttpMethod` twin, `http_handler_method_name` — deleted
/// once its last production caller converted to this `IrHttpMethod` version,
/// which every call site now holds already-resolved (`IrHandlerKind::Http`'s
/// own payload) rather than re-deriving from `HandlerKind::Http`.
pub fn http_handler_method_name_ir(method: IrHttpMethod, path: &str) -> String {
    http_handler_method_name_from_str(method.as_str(), path)
}

fn http_handler_method_name_from_str(method: &str, path: &str) -> String {
    let mut s = format!("http_{method}");
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        s.push('_');
        if let Some(rest) = seg.strip_prefix(':') {
            s.push_str("Param_");
            s.push_str(&sanitise_path_segment(rest));
        } else {
            s.push_str(&sanitise_path_segment(seg));
        }
    }
    s
}

/// Slice 3 (semantic-debugging track, ADR 0105): the debug-metadata sidecar's
/// `{ fn → label }` map — each emitted handler function paired with its Bynk
/// operation label, so the debugger can name a stack frame `GET "/"` rather than
/// `http_GET`. bynk_ts::TsExpr by re-walking the unit's handlers with the *same* naming
/// functions the emitter uses (so the keys match the emitted function names);
/// serialised as a JSON object (manual, like the source map — no serde). Returns
/// `None` when the unit declares no handlers.
pub fn collect_handler_labels(commons: &TypedCommons) -> Option<String> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for item in &commons.commons.items {
        match item {
            CommonsItem::Service(s) => {
                let (mut cron_idx, mut queue_idx) = (0usize, 0usize);
                for h in &s.handlers {
                    let pair = match lower_handler_kind_ir(&h.kind) {
                        IrHandlerKind::Http { method, path } => (
                            http_handler_method_name_ir(method, &path),
                            format!("{} \"{}\"", method.as_str(), path),
                        ),
                        IrHandlerKind::Cron { expr } => {
                            let n = cron_handler_method_name(&s.name.name, cron_idx);
                            cron_idx += 1;
                            (n, format!("cron \"{expr}\""))
                        }
                        IrHandlerKind::Message
                            if matches!(
                                lower_protocol_ir_from_commons(&s.protocol, commons),
                                ProtocolIr::WebSocket { .. }
                            ) =>
                        {
                            ("message".to_string(), "WebSocket message".to_string())
                        }
                        IrHandlerKind::Message => {
                            let n = queue_handler_method_name(&s.name.name, queue_idx);
                            queue_idx += 1;
                            (n, "message".to_string())
                        }
                        IrHandlerKind::Call => {
                            ("call".to_string(), handler_op_label("call", &h.params))
                        }
                        IrHandlerKind::Open => ("open".to_string(), "WebSocket open".to_string()),
                        IrHandlerKind::Close => {
                            ("close".to_string(), "WebSocket close".to_string())
                        }
                        // Events track, slice 0 (spine #936): exactly one
                        // `on event` per `from Events(E)` service (no
                        // pattern-refinement fan-out yet, so no index).
                        IrHandlerKind::Event => ("event".to_string(), "event".to_string()),
                        IrHandlerKind::Rejected => ("rejected".to_string(), "rejected".to_string()),
                    };
                    entries.push(pair);
                }
            }
            CommonsItem::Agent(a) => {
                for h in &a.handlers {
                    if let Some(name) = &h.method_name {
                        entries.push((name.name.clone(), handler_op_label(&name.name, &h.params)));
                    }
                }
            }
            CommonsItem::Type(_)
            | CommonsItem::Fn(_)
            | CommonsItem::Capability(_)
            | CommonsItem::Provider(_)
            | CommonsItem::Actor(_)
            | CommonsItem::Messages(_)
            | CommonsItem::Event(_)
            | CommonsItem::Asset(_)
            | CommonsItem::Embed(_)
            | CommonsItem::Const(_) => {}
        }
    }
    if entries.is_empty() {
        return None;
    }
    // Dedup by key (e.g. two services each with an `on call` emit a `call` method in
    // their own object — distinct in the emitted code, but one key here); keep the
    // first so the JSON object is well-formed.
    let mut seen = std::collections::HashSet::new();
    let mut out = String::from("{");
    let mut first = true;
    for (k, v) in &entries {
        if !seen.insert(k.clone()) {
            continue;
        }
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&bynk_project::json_string(k));
        out.push(':');
        out.push_str(&bynk_project::json_string(v));
    }
    out.push('}');
    Some(out)
}

/// `name(p1, p2)` from a handler's parameters — the operation label for `call` and
/// agent handlers (HTTP handlers use method + path instead).
fn handler_op_label(name: &str, params: &[Param]) -> String {
    let ps: Vec<String> = params.iter().map(|p| p.name.name.clone()).collect();
    format!("{}({})", name, ps.join(", "))
}

/// Synthesise a TypeScript-safe method name for an `on cron` handler (v0.10a):
/// `cron_<service>_<index>`, where `index` is the handler's position among the
/// service's cron handlers in declaration order. The same key is computed at
/// each emission site (the `handlers` method, the `compose` surface wrapper,
/// and the `scheduled` dispatcher) by walking handlers in the same order, so it
/// is collision-free and stable without encoding the schedule expression.
pub fn cron_handler_method_name(service: &str, index: usize) -> String {
    format!("cron_{service}_{index}")
}

/// Method name for an `on queue` handler (v0.10b): `queue_<service>_<index>`,
/// by the handler's position among the service's queue handlers. Computed
/// identically at the `handlers` method, the `compose` surface wrapper, and the
/// `queue` dispatcher (queue names are unique context-wide, but the index keeps
/// the key identifier-safe without sanitising the name).
pub fn queue_handler_method_name(service: &str, index: usize) -> String {
    format!("queue_{service}_{index}")
}

fn sanitise_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

/// Left-folds `parts` into one `+`-chained `bynk_ts::TsExpr::Binary` tree,
/// matching `.join(" + ")`'s own left-to-right text join exactly —
/// `TsBinaryOp::Add`'s own real, dominant site in this whole cluster (#1388).
/// Panics on an empty `parts`; every real caller below already special-cases
/// the empty-template case before reaching here.
pub(super) fn join_plus(mut parts: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    let first = parts.remove(0);
    parts
        .into_iter()
        .fold(first, |acc, next| bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::Add,
            left: Box::new(acc),
            right: Box::new(next),
        })
}

/// `const __events: ... = [];`.
pub(super) fn events_decl_item() -> Item {
    Item::Node(Box::new(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__events".to_string()),
        Some(bynk_ts::TsType::named_with_args(
            "globalThis.Array",
            vec![bynk_ts::TsType::named(
                crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE,
            )],
        )),
        bynk_ts::TsExpr::array(Vec::new()),
        None,
    )))
}

/// `const <name> = <await >(<async >() => { <body> })();`: `body` run in a closure of its own, so
/// that its `return`s resolve `name` rather than the enclosing function. The body sits inside the
/// arrow, one step deeper than the declaration, where the printer lays it out and records its
/// checkpoints.
pub(super) fn iife_const_item(name: &str, awaited: bool, body: Body) -> Item {
    let closure = bynk_ts::TsExpr::Arrow {
        params: Vec::new(),
        is_async: awaited,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Multiline(body.into_stmts())),
    };
    let call = bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Paren(Box::new(closure))),
        args: Vec::new(),
    };
    let init = if awaited {
        bynk_ts::TsExpr::Await(Box::new(call))
    } else {
        call
    };
    Item::Node(Box::new(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(name.to_string()),
        None,
        init,
        None,
    )))
}

/// The closure a handler body runs in so that its own `return`s resolve `__result`.
pub(super) fn result_closure_item(body: Body) -> Item {
    iife_const_item("__result", true, body)
}

/// `if (!(<test>)) return undefined;`: a handler that the guard `test` does not admit does nothing.
pub(super) fn return_undefined_unless(test: bynk_ts::TsExpr) -> Item {
    Item::Node(Box::new(crate::ts_build::if_(
        crate::ts_build::not_expr(crate::ts_build::paren(test)),
        crate::ts_build::return_(Some(crate::ts_build::ident("undefined"))),
    )))
}

/// `if (__events.length > 0) { await deps.__eventsDispatch(__events); }`, with
/// the call on its own lines in a braced block, or `inline` on one.
pub(super) fn events_flush_item(inline: bool) -> Item {
    let dispatch = bynk_ts::TsStmt::expr_stmt(
        bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(ident("deps")),
                property: "__eventsDispatch".to_string(),
            }),
            args: vec![ident("__events")],
        })),
        None,
    );
    let then = if inline {
        bynk_ts::TsStmt::inline_block(vec![dispatch], None)
    } else {
        bynk_ts::TsStmt::block(vec![dispatch], None)
    };
    let cond = bynk_ts::TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::GreaterThan,
        left: Box::new(bynk_ts::TsExpr::Member {
            object: Box::new(ident("__events")),
            property: "length".to_string(),
        }),
        right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("0".to_string()))),
    };
    Item::Node(Box::new(bynk_ts::TsStmt::if_stmt(cond, then, None)))
}

pub(super) fn return_result_item() -> Item {
    Item::Node(Box::new(bynk_ts::TsStmt::return_stmt(
        Some(bynk_ts::TsExpr::Ident("__result".to_string())),
        None,
    )))
}

/// A service handler's body when it emits events directly: the buffered
/// `__events`, the body inside the closure that resolves `__result`, then the
/// flush to whatever composed the handler, and the result.
pub(super) fn events_release_body(body: Body) -> Body {
    let mut wrapped = Body::new();
    wrapped.push(events_decl_item());
    wrapped.push(result_closure_item(body));
    wrapped.push(events_flush_item(false));
    wrapped.push(return_result_item());
    wrapped
}
