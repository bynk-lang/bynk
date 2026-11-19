//! The persisted-store receivers of a method call: held `Map`, `Map`, `Set`, `Cache`, `Log` and
//! `Cell` (back-half paydown 1A.23, #3724, Decision F), with the `@indexed` filter routing.
//! Moved out of `lower.rs` byte for byte before they convert to nodes.

use bynk_check::checker::Callee;

use super::super::lower::{Lowered, LoweredExpr, Pre};
use super::super::lower_ctx::LowerCtx;
use super::query::{
    QueryShape, QuerySource, held_values_source, lower_query_by_name, note_par_fan_out,
    path_values_source, query_key_cmp, query_keyed, query_other_elem,
};
use bynk_syntax::ast::{BinOp, Expr, ExprKind, Ident, LambdaExpr};

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
) -> Option<String> {
    lower_store_held_map_call(e, receiver, method, args, cx, pre)
        .or_else(|| lower_store_map_call(e, receiver, method, args, cx, pre))
        .or_else(|| lower_store_set_call(e, receiver, method, args, cx, pre))
        .or_else(|| lower_store_cache_call(e, receiver, method, args, cx, pre))
        .or_else(|| lower_store_log_call(e, receiver, method, args, cx, pre))
        .or_else(|| lower_store_cell_call(receiver, method, args, cx, pre))
}

/// Lower a held `store Map[K, Connection]` operation (Workers: connection ids are persisted), or `None` when the receiver is not that kind of store
/// field. Moved out of `lower_method_call` (#3167) unchanged.
fn lower_store_held_map_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    let tys = cx.ty_render();
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
    // `Callee::Store`/`Callee::Query` (P6.0) instead of
    // `!cx.is_local(&id.name)` — a real instance of R6.5's name-matched-
    // receiver defect class. A held `store Map[K, Connection]` field's
    // method calls resolve through the same `StoreField::Map` checker
    // dispatch as any other storage map — entry ops (`put`/`remove`/
    // `contains`/`size`/`get`) as `Callee::Store`, lifted query ops (e.g.
    // `parTraverse`) as `Callee::Query` — so both variants are matched here,
    // mirroring `lower_store_map_call`'s own guard.
    // `agent_held_map_frame` stays: it answers which *kind* of store field
    // this is (held vs ordinary), not the shadowing question `Callee`
    // already settles.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Store { .. } | Callee::Query { .. })
        )
        && let Some(f_node) = cx.agent_held_map_frame(&id.name).cloned()
    {
        let f_ts = bynk_ts::print_type(&f_node);
        let var = cx.agent_store_var().to_string();
        let m = format!("{var}.{}", id.name);
        let a_nodes: Vec<LoweredExpr> = args.iter().map(|x| pre.lower_node(x, cx)).collect();
        let a: Vec<String> = a_nodes
            .iter()
            .cloned()
            .map(LoweredExpr::into_text)
            .collect();
        return Some(match method.name.as_str() {
            // #3261: `put` over a key that already holds a connection closes the
            // displaced one first, as `remove` does, so a held connection cannot
            // be dropped unclosed. Re-putting the same connection under its own
            // key (the same connection id) leaves it open.
            "put" => format!(
                "(async () => {{ const __k = String({0}); const __new = __connIdOf({1}); const __old = await {m}.get(__k); if (__old !== undefined && __old !== __new) {{ const __c = __resolveConnection<{f_ts}>(this.state, __old); if (__c.tag === \"Some\") {{ await __c.value.close(); }} }} await {m}.put(__k, __new); return undefined; }})()",
                a[0], a[1]
            ),
            "remove" => format!(
                "(async () => {{ const __k = String({0}); const __cid = await {m}.get(__k); if (__cid !== undefined) {{ const __c = __resolveConnection<{f_ts}>(this.state, __cid); if (__c.tag === \"Some\") {{ await __c.value.close(); }} await {m}.remove(__k); }} return undefined; }})()",
                a[0]
            ),
            "contains" => format!("{m}.has(String({}))", a[0]),
            "size" => format!("{m}.size()"),
            "get" => format!(
                "(() => {{ const __cid = {m}.get(String({0})); return __cid !== undefined ? __resolveConnection<{f_ts}>(this.state, __cid) : None; }})()",
                a[0]
            ),
            // Any non-entry op is a lazy query lifting the map into a scan over its
            // **resolved** connections (the present ones — a connId whose socket has
            // closed drops out).
            _ => {
                note_par_fan_out(cx, method);
                let other_elem = query_other_elem(args, cx);
                lower_query_by_name(
                    QuerySource::Values(held_values_source(&m, &f_node)),
                    method,
                    a_nodes,
                    cx.commons().expr_types.get(&e.id).map(|te| te.ty),
                    &QueryShape {
                        elem: &f_node,
                        other_elem: &other_elem,
                        keyed: query_keyed(e, method, args, cx),
                        key_cmp: query_key_cmp(method, args, cx),
                        runtime_use: cx.runtime_use(),
                    },
                    tys,
                )
                .map_or_else(
                    || format!("(/* unsupported held Map op {} */ undefined)", method.name),
                    |built| LoweredExpr::from(built).into_text(),
                )
            }
        });
    }
    None
}

/// Lower a persisted `store Map[K, V]` operation, including the `@indexed` posting-list maintenance, or `None` when the receiver is not that kind of store
/// field. Moved out of `lower_method_call` (#3167) unchanged.
fn lower_store_map_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    let tys = cx.ty_render();
    // v0.82 (ADR 0110): a storage-`Map` operation — `<map>.<op>(…)` on a `store
    // Map[K, V]` field. Lowers to an entry op over `__state.<map>` (a
    // `Record<string, V>`): mutating the working record (`put`/`remove`/`update`/
    // `upsert`) or reading it (`get`/`contains`/`size`). The surface op is
    // `Effect`-typed and awaited with `<-`, but the working-record mutation is
    // synchronous (the durable write is the single end-of-handler flush), so an
    // awaited expression suffices. `update` on an absent key throws (a fault →
    // nothing commits).
    //
    // P6.21 (partial, continued): the receiver test reads the checker's own
    // `Callee::Store`/`Callee::Query` (P6.0) instead of `!cx.is_local(&id.name)`
    // — a real instance of R6.5's name-matched-receiver defect class, the same
    // one `body_writes_state`/#1196 already closed for write-detection.
    // `cx.is_agent_store_map` stays: it answers a different question (which
    // *kind* of store field this is, needed to pick this branch over
    // Set/Cache/Log/Cell's own), not the shadowing question `Callee` already
    // settles.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Store { .. } | Callee::Query { .. })
        )
        && cx.is_agent_store_map(&id.name)
    {
        let var = cx.agent_store_var().to_string();
        let m = format!("{var}.{}", id.name);
        // v0.93 (ADR 0118): the value-record fields this map is `@indexed(by:)` on.
        let idx_fields: Vec<String> = cx.agent_store_index_fields(&id.name);
        // Route an equality `filter` on an indexed field to a posting-list lookup
        // (`<map>.byIndex(f, v)`) instead of a full scan. The lookup reads the staged
        // handle, so it stays read-your-writes (the index is maintained by every put).
        if method.name == "filter"
            && let [arg] = args
            && let ExprKind::Lambda(lam) = &arg.kind
            && let Some(routed) = pre.absorb_opt(route_indexed_filter(&m, &idx_fields, lam, cx))
        {
            return Some(routed);
        }
        let a_nodes: Vec<LoweredExpr> = args.iter().map(|x| pre.lower_node(x, cx)).collect();
        let a: Vec<String> = a_nodes
            .iter()
            .cloned()
            .map(LoweredExpr::into_text)
            .collect();
        // A key may be a number or a branded value: the handle is keyed by its string.
        let k = a
            .first()
            .map(|x| format!("String({x})"))
            .unwrap_or_default();
        return Some(match method.name.as_str() {
            // #3254: the map is an insertion-ordered handle, and an `@indexed` map's handle
            // keeps its own posting lists exact on every `put`/`remove` (re-indexing on
            // last-write-wins), so the mutators are the plain ones.
            //
            // #3261: on the bundle, `put` over a key that already holds a `Connection`
            // closes the displaced `TestConnection` first, as `remove` does and as the
            // Workers build does; re-putting the same connection under its own key leaves
            // it open (compared by identity).
            "put"
                if cx.agent_store_map_value_ts(&id.name).is_some_and(
                    |v| matches!(v, bynk_ts::TsType::Named { name, .. } if name == "Connection"),
                ) =>
            {
                format!(
                    "(async () => {{ const __k = {0}; const __new = {1}; const __old = await {m}.get(__k); if (__old !== undefined && __old !== __new) {{ await __old.close(); }} await {m}.put(__k, __new); return undefined; }})()",
                    k, a[1]
                )
            }
            // #3189: every handle call is awaited, so a handle backed by storage serves the
            // same lowering as the in-memory one (which returns its values at once).
            "put" => format!(
                "(async () => {{ await {m}.put({}, {}); return undefined; }})()",
                k, a[1]
            ),
            // #3224: on the bundle target a held `store Map[K, Connection]` is an
            // ordinary in-memory map of `TestConnection`s (the Workers build
            // persists connection ids instead and takes `lower_store_held_map_call`), so `remove` must still honour the §2.9 contract that it
            // removes *and closes*: close the connection the entry holds, then
            // delete it, as the Workers lowering does.
            "remove"
                if cx.agent_store_map_value_ts(&id.name).is_some_and(
                    |v| matches!(v, bynk_ts::TsType::Named { name, .. } if name == "Connection"),
                ) =>
            {
                format!(
                    "(async () => {{ const __k = {0}; const __o = await {m}.get(__k); if (__o !== undefined) {{ await __o.close(); await {m}.remove(__k); }} return undefined; }})()",
                    k
                )
            }
            "remove" => format!(
                "(async () => {{ await {m}.remove({}); return undefined; }})()",
                k
            ),
            "contains" => format!("{m}.has({})", k),
            "size" => format!("{m}.size()"),
            "get" => format!(
                "(async () => {{ const __k = {0}; return __bynkThen({m}.has(__k), (__h) => __h ? __bynkThen({m}.get(__k), (__v) => Some(__v!)) : None); }})()",
                k
            ),
            "update" => format!(
                "(async () => {{ await {m}.modify({0}, (__c, __p) => {{ if (!__p) {{ throw new globalThis.Error(\"Map.update: key absent\"); }} return ({1})(__c!); }}); return undefined; }})()",
                k, a[1]
            ),
            "upsert" => format!(
                "(async () => {{ await {m}.modify({0}, (__c, __p) => ({2})(__p ? __c! : ({1}))); return undefined; }})()",
                k, a[1], a[2]
            ),
            // v0.91 (ADR 0119): any non-entry op is a lazy query that lifts the
            // map into a scan over its values (`Object.values`).
            //
            // Review of #1460: `elem_ts` is this map's own value type `V`
            // (`agent_store_map_value_ts`, mirroring `f_ts` above's held-map
            // equivalent) — falls back to `unknown` only for an
            // already-diagnosed program the checker recorded no type for
            // (`join_other_elem_ts`'s own identical fallback posture), not a
            // real case on a certified one: this arm is only reached when
            // `cx.is_agent_store_map` already matched, so `map_values` always
            // has an entry for `id.name` here.
            _ => {
                let elem = cx
                    .agent_store_map_value_ts(&id.name)
                    .cloned()
                    .unwrap_or_else(|| bynk_ts::TsType::named("unknown"));
                note_par_fan_out(cx, method);
                let other_elem = query_other_elem(args, cx);
                lower_query_by_name(
                    QuerySource::Values(path_values_source(&m)),
                    method,
                    a_nodes,
                    cx.commons().expr_types.get(&e.id).map(|te| te.ty),
                    &QueryShape {
                        elem: &elem,
                        other_elem: &other_elem,
                        keyed: query_keyed(e, method, args, cx),
                        key_cmp: query_key_cmp(method, args, cx),
                        runtime_use: cx.runtime_use(),
                    },
                    tys,
                )
                .map_or_else(
                    || format!("(/* unsupported Map op {} */ undefined)", method.name),
                    |built| LoweredExpr::from(built).into_text(),
                )
            }
        });
    }
    None
}

/// Lower a persisted `store Set[T]` operation, or `None` when the receiver is not that kind of store
/// field. Moved out of `lower_method_call` (#3167) unchanged.
fn lower_store_set_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // v0.83: a storage-`Set` operation — `<set>.<op>(…)` on a `store Set[T]` field.
    // Lowers to an entry op over `__state.<set>` (a `Record<string, boolean>`):
    // `add`/`remove` mutate the working record, `contains`/`size` read it.
    //
    // P6.21 (partial, continued): reads `Callee::Store` (P6.0) instead of
    // `!cx.is_local(&id.name)` — see `lower_store_map_call` for the
    // full reasoning. `Set` never gets `Callee::Query` (checker.rs's own
    // `StoreField::Set` arm records only `Store`), unlike `Map`/`Log`.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Store { .. }))
        && cx.is_agent_store_set(&id.name)
    {
        let var = cx.agent_store_var().to_string();
        let s = format!("{var}.{}", id.name);
        let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
        let k = a
            .first()
            .map(|x| format!("String({x})"))
            .unwrap_or_default();
        return Some(match method.name.as_str() {
            "add" => format!(
                "(async () => {{ await {s}.add({}); return undefined; }})()",
                k
            ),
            "remove" => format!(
                "(async () => {{ await {s}.remove({}); return undefined; }})()",
                k
            ),
            "contains" => format!("{s}.has({})", k),
            "size" => format!("{s}.size()"),
            other => format!("(/* unsupported Set op {other} */ undefined)"),
        });
    }
    None
}

/// Lower a persisted `store Cache[K, V]` operation (TTL expiry against the injected `Clock`), or `None` when the receiver is not that kind of store
/// field. Moved out of `lower_method_call` (#3167) unchanged.
fn lower_store_cache_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // v0.87 (ADR 0113): a storage-`Cache` operation — `<cache>.<op>(…)` on a
    // `store Cache[K, V]` field. Lowers to an entry op over `__state.<cache>` (a
    // `Record<string, { v, exp }>`) that applies TTL expiry against the injected
    // `Clock`: every op but `remove` reads `now()` (an awaited `Effect`), so the
    // op is an async IIFE. `put`/`update`/`upsert` stamp `exp = now() + ttl`;
    // `get`/`contains`/`size` treat an entry past `exp` as absent.
    //
    // P6.21 (partial, continued): reads `Callee::Store` instead of
    // `!cx.is_local(&id.name)` — see `lower_store_map_call`. `Cache`
    // never gets `Callee::Query`, same as `Set`.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Store { .. }))
        && let Some(ttl) = cx.agent_store_cache_ttl(&id.name)
    {
        let var = cx.agent_store_var().to_string();
        let c = format!("{var}.{}", id.name);
        // Cache reaping (#3189): note the clock the op read against the state the handler
        // commits, so the commit can delete the entries that clock says are expired.
        let now = format!(
            "__noteClock({var}, await {}.Clock.now())",
            cx.cap_deps_expr()
        );
        let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
        let k = a
            .first()
            .map(|x| format!("String({x})"))
            .unwrap_or_default();
        return Some(match method.name.as_str() {
            "remove" => format!(
                "(async () => {{ await {c}.remove({}); return undefined; }})()",
                k
            ),
            "put" => format!(
                "(async () => {{ const __now = {now}; await {c}.put({0}, {{ v: {1}, exp: __now + {ttl} }}); return undefined; }})()",
                k, a[1]
            ),
            "get" => format!(
                "(async () => {{ const __now = {now}; const __e = await {c}.get({0}); return (__e !== undefined && __e.exp > __now) ? Some(__e.v) : None; }})()",
                k
            ),
            "contains" => format!(
                "(async () => {{ const __now = {now}; const __e = await {c}.get({0}); return __e !== undefined && __e.exp > __now; }})()",
                k
            ),
            "size" => {
                format!(
                    "(async () => {{ const __now = {now}; return await {c}.liveSize(__now); }})()"
                )
            }
            "update" => format!(
                "(async () => {{ const __now = {now}; const __k = {0}; await {c}.modify(__k, (__e) => {{ if (!(__e !== undefined && __e.exp > __now)) {{ throw new globalThis.Error(\"Cache.update: key absent\"); }} return {{ v: ({1})(__e.v), exp: __now + {ttl} }}; }}); return undefined; }})()",
                k, a[1]
            ),
            "upsert" => format!(
                "(async () => {{ const __now = {now}; const __k = {0}; await {c}.modify(__k, (__e) => {{ const __cur = (__e !== undefined && __e.exp > __now) ? __e.v : ({1}); return {{ v: ({2})(__cur), exp: __now + {ttl} }}; }}); return undefined; }})()",
                k, a[1], a[2]
            ),
            other => format!("(/* unsupported Cache op {other} */ undefined)"),
        });
    }
    None
}

/// Lower a persisted `store Log[T]` operation (append, time windows, the query vocabulary), or `None` when the receiver is not that kind of store
/// field. Moved out of `lower_method_call` (#3167) unchanged.
fn lower_store_log_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    let tys = cx.ty_render();
    // v0.95 (ADR 0121): a storage-`Log` operation — `<log>.<op>(…)` on a
    // `store Log[T]` field (an array of `{ t, v }`). `append` stamps the clock
    // and pushes (pruning past `@retain`); the time-window roots and the general
    // query vocabulary lower to a lazy pipeline over the entry values.
    //
    // P6.21 (partial, continued): reads `Callee::Store`/`Callee::Query`
    // instead of `!cx.is_local(&id.name)` — see the storage-`Map` branch
    // above. `Log` gets both variants, same as `Map` (`append` is `Store`;
    // the window-root/query vocabulary is `Query`, checker.rs's own
    // `StoreField::Log` arm).
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Store { .. } | Callee::Query { .. })
        )
        && let Some(retain) = cx.agent_store_log_retain(&id.name)
    {
        let var = cx.agent_store_var().to_string();
        let g = format!("{var}.{}", id.name);
        let a_nodes: Vec<LoweredExpr> = args.iter().map(|x| pre.lower_node(x, cx)).collect();
        let a: Vec<String> = a_nodes
            .iter()
            .cloned()
            .map(LoweredExpr::into_text)
            .collect();
        if method.name == "append" {
            let now = format!("await {}.Clock.now()", cx.cap_deps_expr());
            let prune = match retain {
                Some(ms) => format!(" await {g}.prune(__now - {ms});"),
                None => String::new(),
            };
            return Some(format!(
                "(async () => {{ const __now = {now}; await {g}.append({0}, __now);{prune} return undefined; }})()",
                a[0]
            ));
        }
        let thunk = |body: String| format!("(() => {body})");
        return Some(match method.name.as_str() {
            // Time-window roots → a `Query` thunk over the windowed values.
            "since" => thunk(format!("{g}.since({0})", a[0])),
            "before" => thunk(format!("{g}.before({0})", a[0])),
            "between" => thunk(format!("{g}.between({0}, {1})", a[0], a[1])),
            "recent" => thunk(format!("{g}.recent({0})", a[0])),
            "reversed" => thunk(format!("{g}.reversed()")),
            // The general query vocabulary over the entry values.
            //
            // Review of #1460: `elem_ts` is this log's own element type `T`
            // (`agent_store_log_value_ts`) — same construction and fallback
            // posture as the plain-Map scan above.
            _ => {
                let elem = cx
                    .agent_store_log_value_ts(&id.name)
                    .cloned()
                    .unwrap_or_else(|| bynk_ts::TsType::named("unknown"));
                note_par_fan_out(cx, method);
                let other_elem = query_other_elem(args, cx);
                // The values feeding the general query pipeline.
                lower_query_by_name(
                    QuerySource::Values(path_values_source(&g)),
                    method,
                    a_nodes,
                    cx.commons().expr_types.get(&e.id).map(|te| te.ty),
                    &QueryShape {
                        elem: &elem,
                        other_elem: &other_elem,
                        keyed: query_keyed(e, method, args, cx),
                        key_cmp: query_key_cmp(method, args, cx),
                        runtime_use: cx.runtime_use(),
                    },
                    tys,
                )
                .map_or_else(
                    || format!("(/* unsupported Log op {} */ undefined)", method.name),
                    |built| LoweredExpr::from(built).into_text(),
                )
            }
        });
    }
    None
}

/// Lower a `store Cell[T]` update, or `None` when the receiver is not that kind of store
/// field. Moved out of `lower_method_call` (#3167) unchanged.
fn lower_store_cell_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
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
    if let ExprKind::Ident(id) = &receiver.kind
        && cx
            .agent_store_cells()
            .is_some_and(|(_, cells)| cells.contains(&id.name))
    {
        let var = cx.agent_store_var().to_string();
        let n = format!("{var}.{}", id.name);
        let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
        return Some(match method.name.as_str() {
            "update" => format!("(() => {{ {n} = ({0})({n}); return undefined; }})()", a[0]),
            other => format!("(/* unsupported Cell op {other} */ undefined)"),
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
/// thunk `() => map.byIndex(f, v)`. Otherwise `None` (fall back to scan).
fn route_indexed_filter(
    m: &str,
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
    let v = pre.lower(value, cx);
    let text: Option<String> = Some(format!("(() => {m}.byIndex(\"{field}\", String({v})))"));
    text.map(|expr| pre.finish_text(expr))
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
        _ => false,
    }
}
