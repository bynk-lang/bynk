//! The `Query` and `Stream` method vocabularies (back-half paydown 1A.22, #3723, Decision E):
//! `lower_query_method`, its shape helpers and the stream generators. Moved out of `lower.rs`
//! byte for byte before the `Query` vocabulary converts to nodes.

use bynk_check::checker::Ty;
use bynk_check::kernel_op::{QueryOp, StreamOp};

use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{
    kernel_key_comparator, key_fn_is_plain, key_is_plain, list_ok_elem_ts, with_comparator,
};
use super::super::ts_types::TyRender;
use super::super::ts_types::{ts_array_elem, ts_ty};
use super::super::{BaseType, Expr, Ident, RuntimeUse, TyId};

/// Slice 0b (#2662): the `parTraverse` family joins through `__bynkSettleAll`,
/// so a module using it needs the import even with no `par` block.
/// `lower_query_method` has no `cx`, so its callers note it here.
pub(in crate::emitter) fn note_par_fan_out(cx: &LowerCtx, method: &Ident) {
    if matches!(
        method.name.as_str(),
        "parTraverse" | "parTraverseAll" | "parTraverseTry"
    ) {
        cx.note_settle_all();
    }
}

/// v0.100 (real-time track slice 0): lower a `Stream[T]` op over a `source`
/// async-iterable expression. **Builders** (`map`/`take`) wrap the source in a
/// new async generator — still lazy, still an `AsyncIterable`. The **terminal**
/// `collect` drains the source into an array (an `Effect`, awaited with `<-`).
/// Emitted inline, like the collection kernels, so non-stream files are
/// untouched. Callbacks are wrapped in a single-arg arrow so no extra iterator
/// argument reaches a one-param Bynk fn.
///
/// K-streams (#2623): a stream is single-pass. Each op claims its source when
/// it first reads it ([`STREAM_CLAIM`]): a builder when its generator is first
/// pulled, so building is free and draining claims; `collect` when it runs.
pub(in crate::emitter) fn lower_stream_method(
    source: String,
    op: StreamOp,
    a: &[String],
) -> Option<String> {
    let claim = STREAM_CLAIM;
    match (op, a) {
        // -- builders (return an AsyncIterable) --
        (StreamOp::Map, [f]) => Some(format!(
            "(async function* (__s) {{ {claim} for await (const __e of __s) {{ yield ({f})(__e); }} }})({source})"
        )),
        (StreamOp::Take, [n]) => Some(format!(
            "(async function* (__s) {{ {claim} const __n = {n}; if (__n <= 0) {{ return; }} let __i = 0; for await (const __e of __s) {{ yield __e; if (++__i >= __n) {{ return; }} }} }})({source})"
        )),
        // -- terminal (drains to a list; Effect-typed) --
        (StreamOp::Collect, []) => Some(format!(
            "(async (__s) => {{ {claim} const __r = []; for await (const __e of __s) {{ __r.push(__e); }} return __r; }})({source})"
        )),
        _ => None,
    }
}

/// K-streams (#2623): mark the stream `__s` as read, or fault if it already
/// was. The mark lives on the stream object under a registered symbol, so it
/// holds across modules, and the runtime's response bodies set the same one
/// (`claimStream` in `bynk-emit/runtime/src/http.ts`). Inline, so a file that
/// uses streams still needs no runtime import (ADR 0128 D6).
const STREAM_CLAIM: &str = "const __m = __s as unknown as { [k: symbol]: boolean }; const __k = globalThis.Symbol.for(\"bynk.stream.read\"); if (__m[__k]) { throw new globalThis.Error(\"bynk: a stream can be read only once\"); } __m[__k] = true;";

/// What `lower_query_method` needs to know about the receiver beyond its
/// already-lowered arguments: the rendered element types, and whether a key-based
/// method (`distinct`, `distinctBy`, `groupBy`, `joinOn`, `leftJoin`) has a key
/// that is not a JS primitive and so goes through the keyed helpers (#2789).
pub(in crate::emitter) struct QueryShape<'a> {
    pub(in crate::emitter) elem_ts: &'a str,
    pub(in crate::emitter) other_elem_ts: &'a str,
    pub(in crate::emitter) keyed: bool,
    /// The comparator expression for `sortBy`/`min`/`max` (#2787); `None` keeps
    /// the bare `<`/`>` operators, for `Int`/`String`/`Duration`/`Instant` keys.
    pub(in crate::emitter) key_cmp: Option<bynk_ts::TsExpr>,
    /// The module's runtime-helper record, so a lowering that needs a helper
    /// imports it (`Query.sum` over a `Decimal` key needs `__bynkDecimalAdd`, #3421).
    pub(in crate::emitter) runtime_use: &'a RuntimeUse,
}

/// The comparator for a `Query` `sortBy`/`min`/`max` key, as `lower_list_kernel`
/// picks it: `None` for a plain base key, otherwise the key type's comparator.
pub(in crate::emitter) fn query_key_cmp(
    method: &Ident,
    args: &[Expr],
    cx: &LowerCtx,
) -> Option<bynk_ts::TsExpr> {
    match (method.name.as_str(), args) {
        ("sortBy" | "min" | "max", [key]) => kernel_key_comparator(key, cx),
        ("sortByDescending", [key]) => kernel_key_comparator(key, cx),
        _ => None,
    }
}

/// Whether this `Query` method's key is a non-primitive `Key` type, noting the
/// `keys` runtime helper it then calls. Mirrors `lower_list_kernel`'s
/// `key_is_plain` / `key_fn_is_plain` choice.
pub(in crate::emitter) fn query_keyed(
    e: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &LowerCtx,
) -> bool {
    let tys = cx.ty_render();
    let (helper, plain) = match (method.name.as_str(), args) {
        ("distinct", []) => (
            "__bynkDistinct",
            cx.commons().expr_types.get(&e.id).is_some_and(
                |te| matches!(&*tys.get(te.ty), Ty::Query(el) if key_is_plain(*el, tys.types())),
            ),
        ),
        ("distinctBy", [key]) => ("__bynkDistinctBy", key_fn_is_plain(key, cx)),
        ("groupBy", [key, _]) => ("__bynkGroupBy", key_fn_is_plain(key, cx)),
        ("joinOn", [_, left, ..]) => ("__bynkJoinOn", key_fn_is_plain(left, cx)),
        ("leftJoin", [_, left, ..]) => ("__bynkLeftJoin", key_fn_is_plain(left, cx)),
        _ => return false,
    };
    if !plain {
        cx.runtime_use().note_key(helper);
    }
    !plain
}

/// v0.91 (ADR 0119, query-algebra slice 2): lower a lazy storage-query op over a
/// `source` array expression. **Builders** wrap the source in a deferred thunk
/// `() => …[]` (so the terminal reads *staged* state when it runs —
/// read-your-writes, ADR 0109/0119 D7). **Terminals** read the source array and
/// produce the result; they are `Effect`-typed (awaited with `<-`), but the
/// staged map is in memory, so a synchronous expression suffices (`await` on a
/// non-promise is identity) — except `forEach`, which awaits its effectful fn.
/// The element type is inferred from the typed source (`Record`'s values), so no
/// `__x` annotations are needed. Callbacks are wrapped in a single-arg arrow so
/// the array index never reaches a one-param Bynk fn.
///
/// `shape.elem_ts` is the receiver's own element type, already rendered — resolved
/// by review of #1460 (the `List[T]` sibling, `lower_list_kernel`, already
/// threads the equivalent `elem: TyId` through for its own `distinctBy`/
/// `joinOn`/`leftJoin`/`groupBy` arms; this function's own four collection-
/// kernel sites just never reused it). A pre-rendered string, not a `TyId`,
/// because this function has four real callers with four different sources
/// for it: a genuine `store Query[T]` field/expression (`Ty::Query`'s own
/// element `TyId`, via `ts_ty`), a held `store Map[K, Connection]`'s
/// connection frame type (already a `String`, `agent_held_map_frame`), a
/// plain `store Map[K, V]`'s value type (`agent_store_map_value_ts`, added by
/// this review), and a `store Log[T]`'s element type
/// (`agent_store_log_value_ts`, ditto) — the latter two mirror
/// `held_maps`/`held_maps_ts`'s own established construction exactly, just
/// for the non-held case.
///
/// `shape.other_elem_ts` is `joinOn`/`leftJoin`'s own `other`-side bucket type,
/// computed by every caller via the same `join_other_elem_ts` helper
/// `lower_list_kernel`'s own `joinOn`/`leftJoin`/`join` arms already call —
/// pushed to the call site (rather than this function taking `args`/`cx`
/// itself, `join_other_elem_ts`'s own `args[0]` indexing); every caller
/// already has `args`/`cx` in scope and guards the empty-args case
/// (`distinct`/`collect`/etc. have no `other` to derive one from —
/// `other_elem_ts` is simply unused there). Both ride in [`QueryShape`], with
/// `keyed`, to stay under clippy's argument-count lint.
/// A store-lift receiver (`Callee::Store`/`Callee::Query`, a `store Map` lifted
/// into a scan) carries its operation as a name, so resolve it to a [`QueryOp`]
/// here; the value-kernel `Query` receiver has a typed op already.
pub(in crate::emitter) fn lower_query_by_name(
    source: String,
    method: &Ident,
    a: &[String],
    result_ty: Option<TyId>,
    shape: &QueryShape,
    tys: TyRender<'_>,
) -> Option<String> {
    QueryOp::from_name(&method.name)
        .and_then(|op| lower_query_method(source, op, a, result_ty, shape, tys))
}

pub(in crate::emitter) fn lower_query_method(
    source: String,
    op: QueryOp,
    a: &[String],
    result_ty: Option<TyId>,
    shape: &QueryShape,
    tys: TyRender<'_>,
) -> Option<String> {
    let QueryShape {
        elem_ts,
        other_elem_ts,
        keyed,
        key_cmp,
        runtime_use,
    } = shape;
    let keyed = *keyed;
    let elem_el = ts_array_elem(elem_ts);
    let other_elem_el = ts_array_elem(other_elem_ts);
    // #3189: the source may be a promise (a handle backed by storage reads asynchronously), so
    // every operation runs on it through `__bynkThen`, as `__q`. An in-memory handle's values
    // arrive at once and the callback runs at once.
    let orig = source;
    let source = "__q".to_string();
    let thunk = |body: String| format!("(() => __bynkThen({orig}, (__q) => {body}))");
    let thunk_async = |body: String| format!("(() => __bynkThen({orig}, async (__q) => {body}))");
    let terminal = |expr: String| format!("__bynkThen({orig}, (__q) => {expr})");
    Some(match (op, a) {
        // -- builders → a deferred thunk over the narrowed source --
        (QueryOp::Filter, [p]) => thunk(format!("{source}.filter((__x) => ({p})(__x))")),
        (QueryOp::Map, [f]) => thunk(format!("{source}.map((__x) => ({f})(__x))")),
        // storage flatMap: the fn returns a `Query` (a thunk) — invoke each.
        (QueryOp::FlatMap, [f]) => thunk_async(format!(
            "{{ const __parts = await globalThis.Promise.all({source}.map((__x) => ({f})(__x)())); return __parts.flat(); }}"
        )),
        (QueryOp::SortBy, [key]) if let Some(cmp) = key_cmp => thunk(with_comparator(cmp, |c| {
            format!("[...{source}].sort((__a, __b) => {c}(({key})(__a), ({key})(__b)))")
        })),
        (QueryOp::SortBy, [key]) => thunk(format!(
            "[...{source}].sort((__a, __b) => {{ const __ka = ({key})(__a), __kb = ({key})(__b); return __ka < __kb ? -1 : __ka > __kb ? 1 : 0; }})"
        )),
        (QueryOp::SortByDescending, [key]) if let Some(cmp) = key_cmp => {
            thunk(with_comparator(cmp, |c| {
                format!("[...{source}].sort((__a, __b) => {c}(({key})(__b), ({key})(__a)))")
            }))
        }
        (QueryOp::SortByDescending, [key]) => thunk(format!(
            "[...{source}].sort((__a, __b) => {{ const __ka = ({key})(__a), __kb = ({key})(__b); return __ka < __kb ? 1 : __ka > __kb ? -1 : 0; }})"
        )),
        (QueryOp::Take, [n]) => thunk(format!("{source}.slice(0, globalThis.Math.max(0, {n}))")),
        (QueryOp::Skip, [n]) => thunk(format!("{source}.slice(globalThis.Math.max(0, {n}))")),
        (QueryOp::Distinct, []) if keyed => thunk(format!("__bynkDistinct({source})")),
        (QueryOp::DistinctBy, [key]) if keyed => {
            thunk(format!("__bynkDistinctBy({source}, {key})"))
        }
        (QueryOp::GroupBy, [key, into]) if keyed => {
            thunk(format!("__bynkGroupBy({source}, {key}, {into})"))
        }
        (QueryOp::JoinOn, [other, left, right, into]) if keyed => thunk_async(format!(
            "__bynkJoinOn({source}, await ({other})(), {left}, {right}, {into})"
        )),
        (QueryOp::LeftJoin, [other, left, right, into]) if keyed => thunk_async(format!(
            "__bynkLeftJoin({source}, await ({other})(), {left}, {right}, {into})"
        )),
        (QueryOp::Distinct, []) => thunk(format!("[...new globalThis.Set({source})]")),
        // Review of #1460: narrowed, not deferred. A first attempt used
        // `unknown[]` here and broke real `tsc --strict` fixtures (228/231) —
        // the whole expression's result flows into a context expecting a
        // specific element type (e.g. `readonly Reservation[]`), inferred
        // *because* `__out`/`__h`'s buckets were left untyped for TS to infer
        // through; pinning them to `unknown[]` blocked that inference rather
        // than improving on it. The real fix is the receiver's own `Query[T]`
        // element type, `elem_ts` above — already resolved at every call site
        // via `Ty::Query(elem)`, just never threaded into this function before
        // (the `List[T]` sibling, `lower_list_kernel`, already does this for
        // its own `distinctBy`/`joinOn`/`leftJoin`/`groupBy`).
        (QueryOp::DistinctBy, [key]) => thunk(format!(
            "(() => {{ const __seen = new globalThis.Set(); const __out: {elem_el}[] = []; for (const __x of {source}) {{ const __k = ({key})(__x); if (!__seen.has(__k)) {{ __seen.add(__k); __out.push(__x); }} }} return __out; }})()"
        )),
        // v0.94 (ADR 0116/0120): joins & grouping over storage queries — lazy
        // builders. `other` is itself a `Query` thunk, invoked to materialise the
        // probed side; the result projects through `into` (no pair value). The
        // hash key is stringified (value-keyable); `groupBy`/`into` get the
        // original key, re-derived from a representative row.
        //
        // Review of #1460: `other`'s own element type is `other_elem_ts`,
        // computed at the call site via `join_other_elem_ts` (the same
        // helper `lower_list_kernel`'s own `joinOn`/`leftJoin`/`join` arms
        // already call over `args[0]`'s checked type) — reused, not
        // re-derived, and threaded as a param rather than called here to
        // keep this function's own argument count under clippy's lint.
        (QueryOp::JoinOn, [other, left, right, into]) => thunk_async(format!(
            "{{ const __h: globalThis.Record<string, {other_elem_el}[]> = globalThis.Object.create(null); for (const __u of (await ({other})())) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return {source}.flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; return __m.map((__u: {other_elem_ts}) => ({into})(__t, __u)); }}); }}"
        )),
        (QueryOp::LeftJoin, [other, left, right, into]) => thunk_async(format!(
            "{{ const __h: globalThis.Record<string, {other_elem_el}[]> = globalThis.Object.create(null); for (const __u of (await ({other})())) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return {source}.flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; return __m.length > 0 ? __m.map((__u: {other_elem_ts}) => ({into})(__t, Some(__u))) : [({into})(__t, None)]; }}); }}"
        )),
        (QueryOp::Join, [other, on, into]) => thunk_async(format!(
            "{{ const __b = await ({other})(); return {source}.flatMap((__t) => __b.filter((__u) => ({on})(__t, __u)).map((__u) => ({into})(__t, __u))); }}"
        )),
        // Review of #1460: `__h`'s buckets hold `{source}`'s own rows
        // (`elem_ts`), the receiver's element type — same as `distinctBy`
        // above, not `other`-derived (`groupBy` has no `other` argument).
        (QueryOp::GroupBy, [key, into]) => thunk(format!(
            "{{ const __h: globalThis.Record<string, {elem_el}[]> = globalThis.Object.create(null); const __order: string[] = []; for (const __t of {source}) {{ const __k = String(({key})(__t)); if (!(__k in __h)) {{ __h[__k] = []; __order.push(__k); }} __h[__k].push(__t); }} return __order.map((__k) => {{ const __rows = __h[__k]; return ({into})(({key})(__rows[0]), __rows); }}); }}"
        )),
        // -- terminals → read the source array (awaited at the `<-`) --
        (QueryOp::Collect, []) => terminal(source),
        (QueryOp::First, []) => terminal(format!(
            "(() => {{ const __a = {source}; return __a.length > 0 ? Some(__a[0]) : None; }})()"
        )),
        (QueryOp::FirstOrElse, [default]) => terminal(format!(
            "(() => {{ const __a = {source}; return __a.length > 0 ? __a[0] : ({default}); }})()"
        )),
        (QueryOp::Count, []) => terminal(format!("{source}.length")),
        (QueryOp::Fold, [init, f]) => terminal(format!(
            "(() => {{ let __acc = {init}; for (const __x of {source}) __acc = ({f})(__acc, __x); return __acc; }})()"
        )),
        (QueryOp::Any, [p]) => terminal(format!("{source}.some((__x) => ({p})(__x))")),
        (QueryOp::All, [p]) => terminal(format!("{source}.every((__x) => ({p})(__x))")),
        // #3421: a `Decimal` key sums exactly, as `List.sum` does (#2622): from the
        // zero decimal through the runtime's exact add, never a float `+`.
        (QueryOp::Sum, [key])
            if matches!(
                result_ty.map(|t| tys.get(t)).as_deref(),
                Some(Ty::Effect(inner)) if matches!(&*tys.get(*inner), Ty::Base(BaseType::Decimal))
            ) =>
        {
            runtime_use.note_decimal("__bynkDecimalAdd");
            terminal(format!(
                "{source}.reduce((__s: {}, __x) => __bynkDecimalAdd(__s, ({key})(__x)), {})",
                crate::emitter::ts_types::DECIMAL_TS_TYPE,
                crate::emitter::ts_types::decimal_ts_literal("0", 0)
            ))
        }
        (QueryOp::Sum, [key]) => terminal(format!(
            "{source}.reduce((__s: number, __x) => __s + ({key})(__x), 0)"
        )),
        (QueryOp::Min | QueryOp::Max, [key]) if let Some(key_cmp) = key_cmp => {
            let op = if op == QueryOp::Min { "<" } else { ">" };
            with_comparator(key_cmp, |c| {
                terminal(format!(
                    "(() => {{ const __a = {source}; if (__a.length === 0) return None; let __m = ({key})(__a[0]); for (const __x of __a) {{ const __k = ({key})(__x); if ({c}(__k, __m) {op} 0) __m = __k; }} return Some(__m); }})()"
                ))
            })
        }
        (QueryOp::Min | QueryOp::Max, [key]) => {
            let cmp = if op == QueryOp::Min { "<" } else { ">" };
            terminal(format!(
                "(() => {{ const __a = {source}; if (__a.length === 0) return None; let __m = ({key})(__a[0]); for (const __x of __a) {{ const __k = ({key})(__x); if (__k {cmp} __m) __m = __k; }} return Some(__m); }})()"
            ))
        }
        (QueryOp::Average, [key]) => {
            // Duration averages round to integer millis (checker result decides).
            let round = matches!(
                result_ty.map(|t| tys.get(t)).as_deref(),
                Some(Ty::Effect(inner))
                    if matches!(&*tys.get(*inner), Ty::Option(o)
                        if matches!(&*tys.get(*o), Ty::Base(BaseType::Duration)))
            );
            let mean = if round {
                "globalThis.Math.round(__s / __a.length)"
            } else {
                "__s / __a.length"
            };
            terminal(format!(
                "(() => {{ const __a = {source}; if (__a.length === 0) return None; let __s = 0; for (const __x of __a) __s += ({key})(__x); return Some({mean}); }})()"
            ))
        }
        (QueryOp::ForEach, [f]) => terminal(format!(
            "(async () => {{ for (const __x of {source}) {{ await ({f})(__x); }} }})()"
        )),
        // v0.107 (slice 4): the parallel broadcast form — issue the effectful fn over
        // every element concurrently and await them together, so one slow element
        // does not head-of-line-block the rest.
        (QueryOp::ParTraverse, [f]) => terminal(format!(
            "(async () => {{ await __bynkSettleAll({source}.map((__x) => () => ({f})(__x))); }})()"
        )),
        // v0.149 (ADR 0173): the collect-all terminals over a query/broadcast —
        // gather every `Result` outcome (an `Err` is a value, so nothing rejects).
        (QueryOp::TraverseAll, [f]) => {
            let res_ts = match result_ty.map(|t| tys.get(t)).as_deref() {
                Some(Ty::Effect(inner)) => match &*tys.get(*inner) {
                    Ty::List(el) => ts_ty(*el, tys),
                    _ => ts_ty(*inner, tys),
                },
                _ => "any".to_string(),
            };
            let res_el = ts_array_elem(&res_ts);
            terminal(format!(
                "(async () => {{ const __out: {res_el}[] = []; for (const __x of {source}) {{ __out.push(await ({f})(__x)); }} return __out; }})()"
            ))
        }
        (QueryOp::ParTraverseAll, [f]) => terminal(format!(
            "(async () => await __bynkSettleAll({source}.map((__x) => () => ({f})(__x))))()"
        )),
        // v0.150 (ADR 0174): the short-circuit collect terminals — stop at the
        // first `Err`, returning `Result[List[U], E]`.
        (QueryOp::TraverseTry, [f]) => {
            let u_ts = list_ok_elem_ts(result_ty, tys);
            let u_el = ts_array_elem(&u_ts);
            terminal(format!(
                "(async () => {{ const __out: {u_el}[] = []; for (const __x of {source}) {{ const __r = await ({f})(__x); if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})()"
            ))
        }
        (QueryOp::ParTraverseTry, [f]) => {
            let u_ts = list_ok_elem_ts(result_ty, tys);
            let u_el = ts_array_elem(&u_ts);
            terminal(format!(
                "(async () => {{ const __rs = await __bynkSettleAll({source}.map((__x) => () => ({f})(__x))); const __out: {u_el}[] = []; for (const __r of __rs) {{ if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})()"
            ))
        }
        _ => return None,
    })
}
