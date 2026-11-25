//! The persisted-store receivers of a method call: held `Map`, `Map`, `Set`, `Cache`, `Log` and
//! `Cell` (back-half paydown 1A.23, #3724), with the `@indexed` filter routing. Moved out of
//! `lower.rs` byte for byte (Decision F), then converted to nodes: each arm builds a node over
//! its arguments.
//!
//! A statement-bodied operation is an immediately invoked arrow with a multi-line block body
//! (`TsArrowBody::Multiline`, Decision C), one statement per line.

use bynk_check::checker::Callee;
use bynk_ts::{TsBinaryOp, TsExpr, TsLit, TsStmt, TsType, TsUnaryOp};

use super::super::lower::{Lowered, Pre, member_node};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{iife_block, string_of, take};
use super::super::{BinOp, Expr, ExprKind, Ident, LambdaExpr};
use super::query::{
    QueryShape, QuerySource, held_values_source, lower_query_by_name, path_values_source,
    query_key_cmp, query_keyed, query_other_elem,
};
use crate::ts_build::{
    arrow, binary, block, block_arrow, call, const_typed, expr_stmt, ident, member, method_call,
    paren, ret, str_lit, strict_neq,
};
use bynk_ts::TsParam;

/// A store field's handle, `<var>.<field>`, as a node (the working-state variable and the
/// field's name are each an identifier).
struct Receiver {
    node: TsExpr,
}

impl Receiver {
    fn new(var: &str, field: &str) -> Self {
        Receiver {
            node: member(ident(var), field),
        }
    }
}

/// `undefined`.
fn undefined() -> TsExpr {
    ident("undefined")
}

/// `return undefined;`, the end of every effectful operation.
fn done() -> TsStmt {
    ret(undefined())
}

/// `await e;`.
fn awaited(e: TsExpr) -> TsStmt {
    expr_stmt(TsExpr::Await(Box::new(e)))
}

/// `recv.method(args)` over a handle.
fn op(recv: &TsExpr, method: &str, args: Vec<TsExpr>) -> TsExpr {
    method_call(recv.clone(), method, args)
}

/// `if (cond) { body }`.
fn if_then(cond: TsExpr, body: Vec<TsStmt>) -> TsStmt {
    TsStmt::if_stmt(cond, block(body), None)
}

/// `(async () => { body })()`: an effectful operation (the surrounding `<-` awaits it).
fn async_iife(body: Vec<TsStmt>) -> TsExpr {
    iife_block(vec![], true, body, vec![])
}

/// `(() => { body })()`.
fn sync_iife(body: Vec<TsStmt>) -> TsExpr {
    iife_block(vec![], false, body, vec![])
}

/// `throw new globalThis.Error("message");`.
fn throw_error(message: &str) -> TsStmt {
    TsStmt::throw_stmt(
        TsExpr::New {
            callee: Box::new(member_node("globalThis", "Error")),
            args: vec![str_lit(message)],
        },
        None,
    )
}

/// `(f)(x)`: a Bynk function value applied.
fn apply(f: TsExpr, x: TsExpr) -> TsExpr {
    call(paren(f), vec![x])
}

/// The operation's arguments, lowered in order.
fn lower_args(args: &[Expr], cx: &mut LowerCtx, pre: &mut Pre) -> Vec<bynk_ts::TsExpr> {
    args.iter().map(|x| pre.lower_node(x, cx)).collect()
}

/// The method call an operation is lowered for.
#[derive(Clone, Copy)]
struct Site<'a> {
    e: &'a Expr,
    method: &'a Ident,
    args: &'a [Expr],
}

/// A query operation over a store's values, or the comment placeholder for an operation the
/// vocabulary does not know.
fn query_op(
    what: &str,
    source: bynk_ts::TsExpr,
    site: Site<'_>,
    a: Vec<bynk_ts::TsExpr>,
    elem: &TsType,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let Site { e, method, args } = site;
    let tys = cx.ty_render();
    let other_elem = query_other_elem(args, cx);
    lower_query_by_name(
        QuerySource::Values(source),
        method,
        a,
        cx.commons().expr_types.get(&e.id).map(|te| te.ty),
        &QueryShape {
            elem,
            other_elem: &other_elem,
            keyed: query_keyed(e, method, args, cx),
            key_cmp: query_key_cmp(method, args, cx),
            runtime_use: cx.runtime_use(),
            span: e.span,
        },
        tys,
    )
    // The checker admits only the operations of the store's kind, so a miss is a compiler
    // fault (1A.34): a missing checker fact, reported at the call.
    .unwrap_or_else(|| {
        cx.missing_fact(
            e.span,
            format_args!("supported `{what}` operation (`{}`)", method.name),
        )
    })
}

/// #4037: the entry key of `k` in the store field `field`. A scalar key (`String`, `Int` or a
/// refined or opaque type over one) is `String(k)`, as it always was; any other `Key` type is its
/// canonical encoding, which faults naming the field when it is over the platform's key size.
fn key_of(cx: &LowerCtx, field: &str, k: TsExpr) -> TsExpr {
    if cx.agent_store_key_is_composite(field) {
        call(ident("__bynkStoreKey"), vec![str_lit(field), k])
    } else {
        string_of(k)
    }
}

/// The persisted-store receivers of a method call: a `store` field of the
/// current agent used as `<field>.<op>(…)`. Each kind has its own function, tried
/// in order; `None` means the receiver is none of them.
pub(in crate::emitter) fn lower_store_method_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    lower_store_held_map_call(e, receiver, method, args, cx, pre)
        .or_else(|| lower_store_map_call(e, receiver, method, args, cx, pre))
        .or_else(|| lower_store_set_call(e, receiver, method, args, cx, pre))
        .or_else(|| lower_store_cache_call(e, receiver, method, args, cx, pre))
        .or_else(|| lower_store_log_call(e, receiver, method, args, cx, pre))
        .or_else(|| lower_store_cell_call(receiver, method, args, cx, pre))
}

/// `__resolveConnection<F>(this.state, id)`.
fn resolve_connection(frame: &TsType, id_: TsExpr) -> TsExpr {
    call(
        TsExpr::TypeApp {
            callee: Box::new(ident("__resolveConnection")),
            type_args: vec![frame.clone()],
        },
        vec![member_node("this", "state"), id_],
    )
}

/// `if (__c.tag === "Some") { await __c.value.close(); }`.
fn close_if_some() -> TsStmt {
    if_then(
        binary(
            TsBinaryOp::StrictEq,
            member(ident("__c"), "tag"),
            str_lit("Some"),
        ),
        vec![awaited(call(
            member(member(ident("__c"), "value"), "close"),
            vec![],
        ))],
    )
}

/// Lower a held `store Map[K, Connection]` operation (Workers: connection ids are persisted), or `None` when the receiver is not that kind of store
/// field.
fn lower_store_held_map_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    // v0.104/v0.105 (real-time track slice 3b): a held-`Map` operation —
    // `<map>.<op>(…)` on a `store Map[K, Connection]` field on Workers. The live
    // socket cannot be persisted, so the durable record stores the **connection id**
    // (`__state.<map>` is a `Record<string(K), connId>`) and each `Connection` is
    // re-resolved from its connId via `resolveConnection` — so a stored connection
    // survives DO eviction (§2.9.6, slice 3b-ii). `put` records the connection's id
    // (`connIdOf`); `get` resolves it (or `None` if the socket has since closed);
    // `remove` resolves-closes-deletes (the §2.9 "removes-and-closes" contract). The
    // record mutation is staged in `__state` and flushed by the same end-of-handler
    // commit as any other persisted field.
    // P6.21 (partial, continued): the receiver test reads the checker's own
    // `Callee::Store`/`Callee::Query` (P6.0): entry ops (`put`/`remove`/`contains`/`size`/
    // `get`) are `Callee::Store`, lifted query ops (e.g. `parForEach`) `Callee::Query`.
    // `agent_held_map_frame` stays: it answers which *kind* of store field this is (held vs
    // ordinary), not the shadowing question `Callee` already settles.
    if let ExprKind::Ident(id_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Store { .. } | Callee::Query { .. })
        )
        && let Some(f_node) = cx.agent_held_map_frame(&id_.name).cloned()
    {
        let var = cx.agent_store_var().to_string();
        let m = Receiver::new(&var, &id_.name);
        let a = lower_args(args, cx, pre);
        return Some(match method.name.as_str() {
            // #3261: `put` over a key that already holds a connection closes the
            // displaced one first, as `remove` does, so a held connection cannot
            // be dropped unclosed. Re-putting the same connection under its own
            // key (the same connection id) leaves it open.
            "put" => {
                let c = a;
                let [k, v] = take(c);
                async_iife(vec![
                    const_typed("__k", None, key_of(cx, &id_.name, k)),
                    const_typed("__new", None, call(ident("__connIdOf"), vec![v])),
                    const_typed(
                        "__old",
                        None,
                        TsExpr::Await(Box::new(op(&m.node, "get", vec![ident("__k")]))),
                    ),
                    if_then(
                        binary(
                            TsBinaryOp::And,
                            strict_neq(ident("__old"), undefined()),
                            strict_neq(ident("__old"), ident("__new")),
                        ),
                        vec![
                            const_typed("__c", None, resolve_connection(&f_node, ident("__old"))),
                            close_if_some(),
                        ],
                    ),
                    awaited(op(&m.node, "put", vec![ident("__k"), ident("__new")])),
                    done(),
                ])
            }
            "remove" => {
                let c = a;
                let [k] = take(c);
                async_iife(vec![
                    const_typed("__k", None, key_of(cx, &id_.name, k)),
                    const_typed(
                        "__cid",
                        None,
                        TsExpr::Await(Box::new(op(&m.node, "get", vec![ident("__k")]))),
                    ),
                    if_then(
                        strict_neq(ident("__cid"), undefined()),
                        vec![
                            const_typed("__c", None, resolve_connection(&f_node, ident("__cid"))),
                            close_if_some(),
                            awaited(op(&m.node, "remove", vec![ident("__k")])),
                        ],
                    ),
                    done(),
                ])
            }
            "contains" => {
                let c = a;
                let [k] = take(c);
                op(&m.node, "has", vec![key_of(cx, &id_.name, k)])
            }
            "size" => {
                let _ = a;
                op(&m.node, "size", vec![])
            }
            "get" => {
                let c = a;
                let [k] = take(c);
                sync_iife(vec![
                    const_typed(
                        "__cid",
                        None,
                        op(&m.node, "get", vec![key_of(cx, &id_.name, k)]),
                    ),
                    ret(TsExpr::Conditional {
                        test: Box::new(strict_neq(ident("__cid"), undefined())),
                        consequent: Box::new(resolve_connection(&f_node, ident("__cid"))),
                        alternate: Box::new(ident("None")),
                    }),
                ])
            }
            // Any non-entry op is a lazy query lifting the map into a scan over its
            // **resolved** connections (the present ones — a connId whose socket has
            // closed drops out).
            _ => query_op(
                "held Map",
                held_values_source(m.node.clone(), &f_node),
                Site { e, method, args },
                a,
                &f_node,
                cx,
            ),
        });
    }
    None
}

/// Whether a plain `store Map`'s value type is a `Connection`: such a map closes the
/// connection a `put` displaces and a `remove` drops (#3261, #3224).
fn map_holds_connections(cx: &LowerCtx, field: &str) -> bool {
    cx.agent_store_map_holds_connections(field)
}

/// Lower a persisted `store Map[K, V]` operation, including the `@indexed` posting-list maintenance, or `None` when the receiver is not that kind of store
/// field.
fn lower_store_map_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    // v0.82 (ADR 0110): a storage-`Map` operation — `<map>.<op>(…)` on a `store
    // Map[K, V]` field. Lowers to an entry op over `__state.<map>` (an
    // insertion-ordered handle): mutating the working record (`put`/`remove`/`update`/
    // `upsert`) or reading it (`get`/`contains`/`size`). The surface op is
    // `Effect`-typed and awaited with `<-`, but the working-record mutation is
    // synchronous (the durable write is the single end-of-handler flush), so an
    // awaited expression suffices. `update` on an absent key throws (a fault →
    // nothing commits).
    //
    // P6.21 (partial, continued): the receiver test reads the checker's own
    // `Callee::Store`/`Callee::Query` (P6.0). `cx.is_agent_store_map` stays: it answers
    // which *kind* of store field this is, not the shadowing question `Callee` settles.
    if let ExprKind::Ident(id_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Store { .. } | Callee::Query { .. })
        )
        && cx.is_agent_store_map(&id_.name)
    {
        let var = cx.agent_store_var().to_string();
        let m = Receiver::new(&var, &id_.name);
        // v0.93 (ADR 0118): the value-record fields this map is `@indexed(by:)` on.
        let idx_fields: Vec<String> = cx.agent_store_index_fields(&id_.name);
        // Route an equality `filter` on an indexed field to a posting-list lookup
        // (`<map>.byIndex(f, v)`) instead of a full scan. The lookup reads the staged
        // handle, so it stays read-your-writes (the index is maintained by every put).
        if method.name == "filter"
            && let [arg] = args
            && let ExprKind::Lambda(lam) = &arg.kind
            && let Some(routed) =
                pre.absorb_built(route_indexed_filter(&m, &id_.name, &idx_fields, lam, cx))
        {
            return Some(routed);
        }
        let a = lower_args(args, cx, pre);
        let holds_connections = map_holds_connections(cx, &id_.name);
        return Some(match method.name.as_str() {
            // #3254: the map is an insertion-ordered handle, and an `@indexed` map's handle
            // keeps its own posting lists exact on every `put`/`remove` (re-indexing on
            // last-write-wins), so the mutators are the plain ones.
            //
            // #3261: on the bundle, `put` over a key that already holds a `Connection`
            // closes the displaced `TestConnection` first, as `remove` does and as the
            // Workers build does; re-putting the same connection under its own key leaves
            // it open (compared by identity).
            "put" if holds_connections => {
                let c = a;
                let [k, v] = take(c);
                async_iife(vec![
                    const_typed("__k", None, key_of(cx, &id_.name, k)),
                    const_typed("__new", None, v),
                    const_typed(
                        "__old",
                        None,
                        TsExpr::Await(Box::new(op(&m.node, "get", vec![ident("__k")]))),
                    ),
                    if_then(
                        binary(
                            TsBinaryOp::And,
                            strict_neq(ident("__old"), undefined()),
                            strict_neq(ident("__old"), ident("__new")),
                        ),
                        vec![awaited(call(member(ident("__old"), "close"), vec![]))],
                    ),
                    awaited(op(&m.node, "put", vec![ident("__k"), ident("__new")])),
                    done(),
                ])
            }
            // #3189: every handle call is awaited, so a handle backed by storage serves the
            // same lowering as the in-memory one (which returns its values at once).
            "put" => {
                let c = a;
                let [k, v] = take(c);
                async_iife(vec![
                    awaited(op(&m.node, "put", vec![key_of(cx, &id_.name, k), v])),
                    done(),
                ])
            }
            // #3224: on the bundle target a held `store Map[K, Connection]` is an
            // ordinary in-memory map of `TestConnection`s (the Workers build
            // persists connection ids instead and takes `lower_store_held_map_call`), so
            // `remove` must still honour the §2.9 contract that it removes *and closes*:
            // close the connection the entry holds, then delete it, as the Workers lowering
            // does.
            "remove" if holds_connections => {
                let c = a;
                let [k] = take(c);
                async_iife(vec![
                    const_typed("__k", None, key_of(cx, &id_.name, k)),
                    const_typed(
                        "__o",
                        None,
                        TsExpr::Await(Box::new(op(&m.node, "get", vec![ident("__k")]))),
                    ),
                    if_then(
                        strict_neq(ident("__o"), undefined()),
                        vec![
                            awaited(call(member(ident("__o"), "close"), vec![])),
                            awaited(op(&m.node, "remove", vec![ident("__k")])),
                        ],
                    ),
                    done(),
                ])
            }
            "remove" => {
                let c = a;
                let [k] = take(c);
                async_iife(vec![
                    awaited(op(&m.node, "remove", vec![key_of(cx, &id_.name, k)])),
                    done(),
                ])
            }
            "contains" => {
                let c = a;
                let [k] = take(c);
                op(&m.node, "has", vec![key_of(cx, &id_.name, k)])
            }
            "size" => {
                let _ = a;
                op(&m.node, "size", vec![])
            }
            "get" => {
                let c = a;
                let [k] = take(c);
                let present = op(&m.node, "get", vec![ident("__k")]);
                let some = arrow(
                    vec![TsParam::new("__v", None)],
                    call(ident("Some"), vec![TsExpr::NonNull(Box::new(ident("__v")))]),
                );
                let then_get = call(ident("__bynkThen"), vec![present, some]);
                let branch = TsExpr::Conditional {
                    test: Box::new(ident("__h")),
                    consequent: Box::new(then_get),
                    alternate: Box::new(ident("None")),
                };
                async_iife(vec![
                    const_typed("__k", None, key_of(cx, &id_.name, k)),
                    ret(call(
                        ident("__bynkThen"),
                        vec![
                            op(&m.node, "has", vec![ident("__k")]),
                            arrow(vec![TsParam::new("__h", None)], branch),
                        ],
                    )),
                ])
            }
            "update" => {
                let c = a;
                let [k, f] = take(c);
                let modify = block_arrow(
                    vec![TsParam::new("__c", None), TsParam::new("__p", None)],
                    false,
                    vec![
                        if_then(
                            TsExpr::Unary {
                                op: TsUnaryOp::Not,
                                expr: Box::new(ident("__p")),
                            },
                            vec![throw_error("Map.update: key absent")],
                        ),
                        ret(apply(f, TsExpr::NonNull(Box::new(ident("__c"))))),
                    ],
                );
                async_iife(vec![
                    awaited(op(
                        &m.node,
                        "modify",
                        vec![key_of(cx, &id_.name, k), modify],
                    )),
                    done(),
                ])
            }
            "upsert" => {
                let c = a;
                let [k, default, f] = take(c);
                let cur = TsExpr::Conditional {
                    test: Box::new(ident("__p")),
                    consequent: Box::new(TsExpr::NonNull(Box::new(ident("__c")))),
                    alternate: Box::new(paren(default)),
                };
                let modify = arrow(
                    vec![TsParam::new("__c", None), TsParam::new("__p", None)],
                    apply(f, cur),
                );
                async_iife(vec![
                    awaited(op(
                        &m.node,
                        "modify",
                        vec![key_of(cx, &id_.name, k), modify],
                    )),
                    done(),
                ])
            }
            // v0.91 (ADR 0119): any non-entry op is a lazy query that lifts the
            // map into a scan over its values (`Object.values`).
            //
            // Review of #1460: `elem_ts` is this map's own value type `V`
            // (`agent_store_map_value_ts`) — absent only for a fact the checker
            // did not record, which is a missing fact (1A.34), not a real case on a
            // certified program: this arm is only reached when
            // `cx.is_agent_store_map` already matched, so `map_values` always
            // has an entry for `id.name` here.
            _ => {
                let elem = cx
                    .agent_store_map_value_ts(&id_.name)
                    .cloned()
                    .unwrap_or_else(|| {
                        cx.missing_fact_ty(e.span, "value type of an agent `Map` store field")
                    });
                query_op(
                    "Map",
                    path_values_source(m.node.clone()),
                    Site { e, method, args },
                    a,
                    &elem,
                    cx,
                )
            }
        });
    }
    None
}

/// Lower a persisted `store Set[T]` operation, or `None` when the receiver is not that kind of store
/// field.
fn lower_store_set_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    // v0.83: a storage-`Set` operation — `<set>.<op>(…)` on a `store Set[T]` field.
    // Lowers to an entry op over `__state.<set>`: `add`/`remove` mutate the working
    // record, `contains`/`size` read it.
    //
    // P6.21 (partial, continued): reads `Callee::Store` (P6.0). `Set` never gets
    // `Callee::Query` (checker.rs's own `StoreField::Set` arm records only `Store`),
    // unlike `Map`/`Log`.
    if let ExprKind::Ident(id_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Store { .. }))
        && cx.is_agent_store_set(&id_.name)
    {
        let var = cx.agent_store_var().to_string();
        let s = Receiver::new(&var, &id_.name);
        let a = lower_args(args, cx, pre);
        return Some(match method.name.as_str() {
            "add" | "remove" => {
                let which = method.name.as_str();
                {
                    let c = a;
                    let [k] = take(c);
                    async_iife(vec![
                        awaited(op(&s.node, which, vec![key_of(cx, &id_.name, k)])),
                        done(),
                    ])
                }
            }
            "contains" => {
                let c = a;
                let [k] = take(c);
                op(&s.node, "has", vec![key_of(cx, &id_.name, k)])
            }
            "size" => {
                let _ = a;
                op(&s.node, "size", vec![])
            }
            // The checker admits only the operations of the store's kind, so a miss is a
            // compiler fault (1A.34): a missing checker fact, reported at the call.
            other => cx.missing_fact(
                e.span,
                format_args!("supported `Set` operation (`{other}`)"),
            ),
        });
    }
    None
}

/// `await <deps>.Clock.now()`.
fn awaited_now(deps: TsExpr) -> TsExpr {
    TsExpr::Await(Box::new(call(member(member(deps, "Clock"), "now"), vec![])))
}

/// `__e !== undefined && __e.exp > __now`: the entry is present and has not expired.
fn is_live() -> TsExpr {
    binary(
        TsBinaryOp::And,
        strict_neq(ident("__e"), undefined()),
        binary(
            TsBinaryOp::GreaterThan,
            member(ident("__e"), "exp"),
            ident("__now"),
        ),
    )
}

/// `{ v: value, exp: __now + <ttl> }`.
fn stamped(value: TsExpr, ttl: i64) -> TsExpr {
    TsExpr::object(vec![
        ("v".to_string(), value),
        (
            "exp".to_string(),
            binary(
                TsBinaryOp::Add,
                ident("__now"),
                TsExpr::Lit(TsLit::Num(ttl.to_string())),
            ),
        ),
    ])
}

/// Lower a persisted `store Cache[K, V]` operation (TTL expiry against the injected `Clock`), or `None` when the receiver is not that kind of store
/// field.
fn lower_store_cache_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    // v0.87 (ADR 0113): a storage-`Cache` operation — `<cache>.<op>(…)` on a
    // `store Cache[K, V]` field. Lowers to an entry op over `__state.<cache>` (entries
    // `{ v, exp }`) that applies TTL expiry against the injected `Clock`: every op but
    // `remove` reads `now()` (an awaited `Effect`), so the op is an async IIFE.
    // `put`/`update`/`upsert` stamp `exp = now() + ttl`; `get`/`contains`/`size` treat an
    // entry past `exp` as absent.
    //
    // P6.21 (partial, continued): reads `Callee::Store`. `Cache` never gets
    // `Callee::Query`, same as `Set`.
    if let ExprKind::Ident(id_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Store { .. }))
        && let Some(ttl) = cx.agent_store_cache_ttl(&id_.name)
    {
        let var = cx.agent_store_var().to_string();
        let c = Receiver::new(&var, &id_.name);
        // Cache reaping (#3189): note the clock the op read against the state the handler
        // commits, so the commit can delete the entries that clock says are expired. Each
        // operation reads it once, in its own `const __now`.
        let deps = cx.cap_deps_node();
        let now = || {
            const_typed(
                "__now",
                None,
                call(
                    ident("__noteClock"),
                    vec![ident(&var), awaited_now(deps.clone())],
                ),
            )
        };
        let a = lower_args(args, cx, pre);
        return Some(match method.name.as_str() {
            "remove" => {
                let c_ = a;
                let [k] = take(c_);
                async_iife(vec![
                    awaited(op(&c.node, "remove", vec![key_of(cx, &id_.name, k)])),
                    done(),
                ])
            }
            "put" => {
                let c_ = a;
                let [k, v] = take(c_);
                async_iife(vec![
                    now(),
                    awaited(op(
                        &c.node,
                        "put",
                        vec![key_of(cx, &id_.name, k), stamped(v, ttl)],
                    )),
                    done(),
                ])
            }
            "get" => {
                let c_ = a;
                let [k] = take(c_);
                async_iife(vec![
                    now(),
                    const_typed(
                        "__e",
                        None,
                        TsExpr::Await(Box::new(op(&c.node, "get", vec![key_of(cx, &id_.name, k)]))),
                    ),
                    ret(TsExpr::Conditional {
                        test: Box::new(paren(is_live())),
                        consequent: Box::new(call(ident("Some"), vec![member(ident("__e"), "v")])),
                        alternate: Box::new(ident("None")),
                    }),
                ])
            }
            "contains" => {
                let c_ = a;
                let [k] = take(c_);
                async_iife(vec![
                    now(),
                    const_typed(
                        "__e",
                        None,
                        TsExpr::Await(Box::new(op(&c.node, "get", vec![key_of(cx, &id_.name, k)]))),
                    ),
                    ret(is_live()),
                ])
            }
            "size" => {
                let _ = a;
                async_iife(vec![
                    now(),
                    ret(TsExpr::Await(Box::new(op(
                        &c.node,
                        "liveSize",
                        vec![ident("__now")],
                    )))),
                ])
            }
            "update" => {
                let c_ = a;
                let [k, f] = take(c_);
                let modify = block_arrow(
                    vec![TsParam::new("__e", None)],
                    false,
                    vec![
                        if_then(
                            TsExpr::Unary {
                                op: TsUnaryOp::Not,
                                expr: Box::new(is_live()),
                            },
                            vec![throw_error("Cache.update: key absent")],
                        ),
                        ret(stamped(apply(f, member(ident("__e"), "v")), ttl)),
                    ],
                );
                async_iife(vec![
                    now(),
                    const_typed("__k", None, key_of(cx, &id_.name, k)),
                    awaited(op(&c.node, "modify", vec![ident("__k"), modify])),
                    done(),
                ])
            }
            "upsert" => {
                let c_ = a;
                let [k, default, f] = take(c_);
                let cur = TsExpr::Conditional {
                    test: Box::new(paren(is_live())),
                    consequent: Box::new(member(ident("__e"), "v")),
                    alternate: Box::new(paren(default)),
                };
                let modify = block_arrow(
                    vec![TsParam::new("__e", None)],
                    false,
                    vec![
                        const_typed("__cur", None, cur),
                        ret(stamped(apply(f, ident("__cur")), ttl)),
                    ],
                );
                async_iife(vec![
                    now(),
                    const_typed("__k", None, key_of(cx, &id_.name, k)),
                    awaited(op(&c.node, "modify", vec![ident("__k"), modify])),
                    done(),
                ])
            }
            // The checker admits only the operations of the store's kind, so a miss is a
            // compiler fault (1A.34): a missing checker fact, reported at the call.
            other => cx.missing_fact(
                e.span,
                format_args!("supported `Cache` operation (`{other}`)"),
            ),
        });
    }
    None
}

/// Lower a persisted `store Log[T]` operation (append, time windows, the query vocabulary), or `None` when the receiver is not that kind of store
/// field.
fn lower_store_log_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    // v0.95 (ADR 0121): a storage-`Log` operation — `<log>.<op>(…)` on a
    // `store Log[T]` field (an array of `{ t, v }`). `append` stamps the clock
    // and pushes (pruning past `@retain`); the time-window roots and the general
    // query vocabulary lower to a lazy pipeline over the entry values.
    //
    // P6.21 (partial, continued): reads `Callee::Store`/`Callee::Query`. `Log` gets both
    // variants, same as `Map` (`append` is `Store`; the window-root/query vocabulary is
    // `Query`, checker.rs's own `StoreField::Log` arm).
    if let ExprKind::Ident(id_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Store { .. } | Callee::Query { .. })
        )
        && let Some(retain) = cx.agent_store_log_retain(&id_.name)
    {
        let var = cx.agent_store_var().to_string();
        let g = Receiver::new(&var, &id_.name);
        let a = lower_args(args, cx, pre);
        if method.name == "append" {
            let now = awaited_now(cx.cap_deps_node());
            return Some({
                let c = a;
                let [v] = take(c);
                let mut body = vec![
                    const_typed("__now", None, now),
                    awaited(op(&g.node, "append", vec![v, ident("__now")])),
                ];
                if let Some(ms) = retain {
                    body.push(awaited(op(
                        &g.node,
                        "prune",
                        vec![binary(
                            TsBinaryOp::Sub,
                            ident("__now"),
                            TsExpr::Lit(TsLit::Num(ms.to_string())),
                        )],
                    )));
                }
                body.push(done());
                async_iife(body)
            });
        }
        // A time-window root: a `Query` thunk over the windowed values, `(() => g.since(x))`.
        let window = |a: Vec<bynk_ts::TsExpr>, name: &'static str| {
            let c = a;
            paren(arrow(vec![], op(&g.node, name, c)))
        };
        return Some(match method.name.as_str() {
            "since" => window(a, "since"),
            "before" => window(a, "before"),
            "between" => window(a, "between"),
            "recent" => window(a, "recent"),
            "reversed" => window(a, "reversed"),
            // The general query vocabulary over the entry values.
            //
            // Review of #1460: `elem_ts` is this log's own element type `T`
            // (`agent_store_log_value_ts`) — same construction and missing-fact
            // posture as the plain-Map scan above.
            _ => {
                let elem = cx
                    .agent_store_log_value_ts(&id_.name)
                    .cloned()
                    .unwrap_or_else(|| {
                        cx.missing_fact_ty(e.span, "element type of an agent `Log` store field")
                    });
                query_op(
                    "Log",
                    path_values_source(g.node.clone()),
                    Site { e, method, args },
                    a,
                    &elem,
                    cx,
                )
            }
        });
    }
    None
}

/// Lower a `store Cell[T]` update, or `None` when the receiver is not that kind of store
/// field.
fn lower_store_cell_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    // v0.98 (ADR 0125): a storage-`Cell` operation — `<cell>.update(f)` on a
    // `store Cell[T]` field. Lowers to a staged read-modify-write over
    // `__state.<cell>` (`Map.update`'s lowering minus the key-absent guard — a
    // cell is always present, so there is no fault path). The mutation is
    // synchronous against the working state (read-your-writes); the single
    // end-of-handler `commitState` flush runs the invariant gate before the
    // durable write, exactly as `:=` does.
    // Note: unlike the other store kinds, a `Cell` field is deliberately bound
    // into checker scope (self_scope) as a bare local of its element type, so
    // the checker always resolves a same-named identifier to the cell — no
    // `is_local` guard belongs here without also changing that scope
    // construction, or the emitter would disagree with what the checker typed.
    if let ExprKind::Ident(id_) = &receiver.kind
        && cx
            .agent_store_cells()
            .is_some_and(|(_, cells)| cells.contains(&id_.name))
    {
        let var = cx.agent_store_var().to_string();
        let n = Receiver::new(&var, &id_.name);
        let a = lower_args(args, cx, pre);
        return Some(match method.name.as_str() {
            "update" => {
                let c = a;
                let [f] = take(c);
                sync_iife(vec![
                    TsStmt::assign(n.node.clone(), apply(f, n.node.clone()), None),
                    done(),
                ])
            }
            // The checker admits only the operations of the store's kind, so a miss is a
            // compiler fault (1A.34): a missing checker fact, reported at the call.
            other => cx.missing_fact(
                method.span,
                format_args!("supported `Cell` operation (`{other}`)"),
            ),
        });
    }
    None
}

// ---- v0.93 (ADR 0118): `@indexed` secondary-index emission ----------------
//
// For a `store Map[K, V] @indexed(by: f)` field, the map's handle keeps a posting list
// per index: a (stringified) field value maps to the primary keys whose value carries
// it, in insertion order (#3254). Its `put`/`remove` maintain it; an equality `filter`
// on `f` reads it through `byIndex` instead of scanning.

/// If `lam` is `(p) => p.<field> == <value>` (either order) where `<field>` is
/// indexed and `<value>` does not mention `p`, lower it to a posting-list lookup
/// thunk `() => map.byIndex(f, v)`: a node when the value is a node (1A.23, #3724). Otherwise
/// `None` (fall back to scan).
fn route_indexed_filter(
    m: &Receiver,
    map_name: &str,
    fields: &[String],
    lam: &LambdaExpr,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    if fields.is_empty() {
        return None;
    }
    let [param] = lam.params.as_slice() else {
        return None;
    };
    let pname = param.name.name.as_str();
    let ExprKind::BinOp(BinOp::Eq, lhs, rhs) = &lam.body.kind else {
        return None;
    };
    // One side must be `p.<field>`; the other is the (param-independent) value.
    let (field, value) = match as_param_field(pname, lhs) {
        Some(f) => (f, rhs.as_ref()),
        None => (as_param_field(pname, rhs)?, lhs.as_ref()),
    };
    if !fields.iter().any(|f| f == field) || !param_independent(value, pname) {
        return None;
    }
    let v = pre.lower_node(value, cx);
    let built = {
        let c = vec![v];
        let [v] = take(c);
        paren(arrow(
            vec![],
            call(
                member(m.node.clone(), "byIndex"),
                vec![
                    str_lit(field),
                    if cx.agent_store_index_is_composite(map_name, field) {
                        call(ident("__bynkKeyEncode"), vec![v])
                    } else {
                        string_of(v)
                    },
                ],
            ),
        ))
    };
    Some(pre.finish(built))
}

/// `e` as `<param>.<field>` → the field name; else `None`.
fn as_param_field<'e>(pname: &str, e: &'e Expr) -> Option<&'e str> {
    if let ExprKind::FieldAccess { receiver, field } = &e.kind
        && let ExprKind::Ident(r) = &receiver.kind
        && r.name == pname
    {
        Some(field.name.as_str())
    } else {
        None
    }
}

/// Whether `e` provably does not reference the lambda parameter `pname` — only
/// then is it safe to hoist it out of the per-row predicate into one lookup key.
/// Conservative: unrecognised shapes return `false` (no routing).
fn param_independent(e: &Expr, pname: &str) -> bool {
    match &e.kind {
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::StrLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::DurationLit { .. } => true,
        ExprKind::Ident(id) => id.name != pname,
        ExprKind::FieldAccess { receiver, .. } => param_independent(receiver, pname),
        ExprKind::DecimalLit { .. }
        | ExprKind::InterpStr(..)
        | ExprKind::Call { .. }
        | ExprKind::Lambda(..)
        | ExprKind::BinOp(..)
        | ExprKind::UnaryOp(..)
        | ExprKind::Paren(..)
        | ExprKind::Block(..)
        | ExprKind::If { .. }
        | ExprKind::Ok(..)
        | ExprKind::Err(..)
        | ExprKind::Question(..)
        | ExprKind::RecordConstruction { .. }
        | ExprKind::MethodCall { .. }
        | ExprKind::Match { .. }
        | ExprKind::Is { .. }
        | ExprKind::Some(..)
        | ExprKind::None
        | ExprKind::UnitLit
        | ExprKind::RecordSpread { .. }
        | ExprKind::EffectPure(..)
        | ExprKind::Expect(..)
        | ExprKind::Val { .. }
        | ExprKind::Wire(..)
        | ExprKind::ListLit(..)
        | ExprKind::Observation(..)
        | ExprKind::Faults(..)
        | ExprKind::Trace { .. } => false,
    }
}
