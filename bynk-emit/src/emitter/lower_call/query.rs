//! The `Query` and `Stream` method vocabularies (back-half paydown 1A.22, #3723, Decision E):
//! `lower_query_method` and its shape helpers, which build nodes, and the stream generators,
//! which stay text. Moved out of `lower.rs` byte for byte, then the `Query` vocabulary
//! converted.

use bynk_check::checker::Ty;
use bynk_check::kernel_op::{QueryOp, StreamOp};
use bynk_ts::{TsArrowBody, TsBinaryOp, TsExpr, TsStmt, TsType, print_type};

use super::super::internal_fault::FaultSite;
use super::super::lower::{Built, LoweredExpr, decimal_lit, node_or_text};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{
    JoinSides, applied, array_ts, arrow, arrow_with, assign, awaited_apply, bail_on_err, binary,
    block, call, clamped_at_zero, const_, distinct_by_body, expr_stmt, for_of, group_by_body, id,
    iife_block, join_body, join_other_elem, kernel_key_comparator, key_fn_is_plain, key_is_plain,
    let_, list_ok_elem, member, method_call_bare, method_sort, param, paren, push_value, ret,
    sort_order, sort_plain_by, spread, take, with_comparator, with_comparator_node, zero,
};
use super::super::ts_types::{TyRender, ts_ty_to_ts_type};
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
// 1A: generator and for-await have no bynk_ts node, so the `Stream` generators stay text (#3723,
// Decision A); their caller hands over text and returns `Err`.
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
/// already-lowered arguments: the element types, and whether a key-based
/// method (`distinct`, `distinctBy`, `groupBy`, `joinOn`, `leftJoin`) has a key
/// that is not a JS primitive and so goes through the keyed helpers (#2789).
///
/// The element types are nodes (1A.22, #3723, Decision A): the printer owns the bracketing
/// of `T[]`, and the text fallbacks print the same node.
pub(in crate::emitter) struct QueryShape<'a> {
    /// The receiver's own element type.
    pub(in crate::emitter) elem: &'a TsType,
    /// `joinOn`/`leftJoin`'s `other` side's element type; unused by every other operation.
    pub(in crate::emitter) other_elem: &'a TsType,
    pub(in crate::emitter) keyed: bool,
    /// The comparator expression for `sortBy`/`min`/`max` (#2787); `None` keeps
    /// the bare `<`/`>` operators, for `Int`/`String`/`Duration`/`Instant` keys.
    pub(in crate::emitter) key_cmp: Option<bynk_ts::TsExpr>,
    /// The module's runtime-helper record, so a lowering that needs a helper
    /// imports it (`Query.sum` over a `Decimal` key needs `__bynkDecimalAdd`, #3421).
    pub(in crate::emitter) runtime_use: &'a RuntimeUse,
    /// The operation's `.bynk` span, where a missing checker fact is reported (1A.34).
    pub(in crate::emitter) span: bynk_syntax::span::Span,
}

/// The `other` side's element type of a `joinOn`/`leftJoin`, for the callers' [`QueryShape`]
/// (`unknown` when there is no `other` argument, where it is never read).
pub(in crate::emitter) fn query_other_elem(args: &[Expr], cx: &LowerCtx) -> TsType {
    if args.is_empty() {
        TsType::named("unknown")
    } else {
        join_other_elem(args, cx)
    }
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

/// A store-lift receiver (`Callee::Store`/`Callee::Query`, a `store Map` lifted
/// into a scan) carries its operation as a name, so resolve it to a [`QueryOp`]
/// here; the value-kernel `Query` receiver has a typed op already.
pub(in crate::emitter) fn lower_query_by_name(
    source: QuerySource,
    method: &Ident,
    a: Vec<LoweredExpr>,
    result_ty: Option<TyId>,
    shape: &QueryShape,
    tys: TyRender<'_>,
) -> Option<Built> {
    QueryOp::from_name(&method.name)
        .and_then(|op| lower_query_method(source, op, a, result_ty, shape, tys))
}

// ---- The callers' sources ---------------------------------------------------------------

/// What a query operation reads: an array-valued source, or a `Query` receiver, which is a
/// thunk to invoke (`(recv)()`) before it reads.
pub(in crate::emitter) enum QuerySource {
    Values(LoweredExpr),
    Thunk(LoweredExpr),
}

/// `recv.values()`: a plain `store Map`'s or a `store Log`'s entries, over its handle node.
pub(in crate::emitter) fn path_values_source(recv: TsExpr) -> TsExpr {
    call(member(recv, "values"), vec![])
}

/// A held `store Map[K, Connection[F]]`'s resolved connections: its stored connection ids
/// resolved through the runtime, the present ones kept. The callback is the one-line block
/// the text printed, so the source reads as it did.
pub(in crate::emitter) fn held_values_source(recv: TsExpr, frame: &TsType) -> TsExpr {
    let resolve = call(
        TsExpr::TypeApp {
            callee: Box::new(id("__resolveConnection")),
            type_args: vec![frame.clone()],
        },
        vec![
            super::super::lower::member_node("this", "state"),
            id("__cid"),
        ],
    );
    let present = TsExpr::Conditional {
        test: Box::new(binary(
            TsBinaryOp::StrictEq,
            member(id("__c"), "tag"),
            TsExpr::Lit(bynk_ts::TsLit::Str("Some".to_string())),
        )),
        consequent: Box::new(TsExpr::array(vec![member(id("__c"), "value")])),
        alternate: Box::new(TsExpr::array(vec![])),
    };
    let each_id = TsExpr::Arrow {
        params: vec![param("__cid", None)],
        is_async: false,
        generics: vec![],
        return_type: None,
        body: Box::new(TsArrowBody::Block(vec![
            const_("__c", None, resolve),
            ret(present),
        ])),
    };
    let values = call(member(recv, "values"), vec![]);
    method_call_bare(values, "flatMap", vec![each_id])
}

// ---- The thunk wrappers -----------------------------------------------------------------
//
// #3189: the source may be a promise (a handle backed by storage reads asynchronously), so
// every operation runs on it through `__bynkThen`, as `__q`. An in-memory handle's values
// arrive at once and the callback runs at once. A builder is a deferred thunk (so the
// terminal reads *staged* state when it runs, read-your-writes, ADR 0109/0119 D7); a
// terminal is the unwrapped `__bynkThen`.

fn q() -> TsExpr {
    id("__q")
}

/// `(() => __bynkThen(orig, (__q) => body))`, `async (__q)` when the body awaits.
fn thunk_node(orig: TsExpr, is_async: bool, body: TsArrowBody) -> TsExpr {
    let each = TsExpr::Arrow {
        params: vec![param("__q", None)],
        is_async,
        generics: vec![],
        return_type: None,
        body: Box::new(body),
    };
    paren(arrow(vec![], call(id("__bynkThen"), vec![orig, each])))
}

/// `__bynkThen(orig, (__q) => expr)`.
fn terminal_node(orig: TsExpr, expr: TsExpr) -> TsExpr {
    call(
        id("__bynkThen"),
        vec![orig, arrow(vec![param("__q", None)], expr)],
    )
}

/// [`thunk_node`] over text.
fn thunk_text(orig: &str, is_async: bool, body: &str) -> String {
    let asyncness = if is_async { "async " } else { "" };
    format!("(() => __bynkThen({orig}, {asyncness}(__q) => {body}))")
}

/// [`terminal_node`] over text.
fn terminal_text(orig: &str, expr: &str) -> String {
    format!("__bynkThen({orig}, (__q) => {expr})")
}

/// `(__x) => (f)(__x)`: the callback is wrapped in a one-parameter arrow so the array index
/// never reaches a one-parameter Bynk function.
fn each(f: TsExpr) -> TsExpr {
    arrow(vec![param("__x", None)], applied(f, id("__x")))
}

/// `await (f)()`: a `Query` argument is itself a thunk, invoked to materialise it.
fn awaited_thunk(f: TsExpr) -> TsExpr {
    TsExpr::Await(Box::new(call(paren(f), vec![])))
}

/// `__a[0]`.
fn first_of_a() -> TsExpr {
    TsExpr::Index {
        object: Box::new(id("__a")),
        index: Box::new(zero()),
    }
}

/// `__q.map((__x) => () => (f)(__x))`: one thunk per element, for `__bynkSettleAll`.
fn settle_thunks(f: TsExpr) -> TsExpr {
    method_call_bare(
        q(),
        "map",
        vec![arrow(
            vec![param("__x", None)],
            arrow(vec![], applied(f, id("__x"))),
        )],
    )
}

/// `await __bynkSettleAll(__q.map(…))`.
fn settled(f: TsExpr) -> TsExpr {
    TsExpr::Await(Box::new(call(
        id("__bynkSettleAll"),
        vec![settle_thunks(f)],
    )))
}

/// What one operation is lowered from: the source, its arguments, and the receiver's shape.
struct Arm<'a> {
    source: QuerySource,
    a: Vec<LoweredExpr>,
    result_ty: Option<TyId>,
    shape: &'a QueryShape<'a>,
    tys: TyRender<'a>,
}

impl Arm<'_> {
    /// Where this operation reports a checker fact it needs and does not find.
    fn fault_site(&self) -> FaultSite<'_> {
        FaultSite {
            sink: self.shape.runtime_use,
            span: self.shape.span,
        }
    }

    /// The operation as a node when its source and every argument are nodes, else today's
    /// text (see `node_or_text`). `node` and `text` receive the source, then the arguments.
    fn built_vec(
        self,
        node: impl FnOnce(TsExpr, Vec<TsExpr>) -> TsExpr,
        text: impl FnOnce(String, Vec<String>) -> String,
    ) -> Built {
        let (source, invoke) = match self.source {
            QuerySource::Values(source) => (source, false),
            QuerySource::Thunk(recv) => (recv, true),
        };
        let mut children = vec![source];
        children.extend(self.a);
        node_or_text(
            children,
            |mut n| {
                let args = n.split_off(1);
                let [orig] = take(n);
                let orig = if invoke {
                    call(paren(orig), vec![])
                } else {
                    orig
                };
                node(orig, args)
            },
            |mut t| {
                let args = t.split_off(1);
                let [orig] = take(t);
                let orig = if invoke { format!("({orig})()") } else { orig };
                text(orig, args)
            },
        )
    }

    /// [`Self::built_vec`] for an operation of exactly `N` arguments.
    fn built<const N: usize>(
        self,
        node: impl FnOnce(TsExpr, [TsExpr; N]) -> TsExpr,
        text: impl FnOnce(String, [String; N]) -> String,
    ) -> Built {
        self.built_vec(|o, a| node(o, take(a)), |o, a| text(o, take(a)))
    }
}

/// v0.91 (ADR 0119, query-algebra slice 2): lower a lazy storage-query op over a
/// `source` array expression. **Builders** wrap the source in a deferred thunk
/// `() => …[]` (so the terminal reads *staged* state when it runs —
/// read-your-writes, ADR 0109/0119 D7). **Terminals** read the source array and
/// produce the result; they are `Effect`-typed (awaited with `<-`), but the
/// staged map is in memory, so a synchronous expression suffices (`await` on a
/// non-promise is identity) — except `forEach`, which awaits its effectful fn.
/// Callbacks are wrapped in a single-arg arrow so the array index never reaches a
/// one-param Bynk fn.
///
/// A node when the source and every argument are nodes (1A.22, #3723), else the text it
/// printed before. `shape.elem` is the receiver's own element type and `shape.other_elem`
/// the `other` side of a join, which each of the four callers derives from its own source
/// (a `store Query[T]` expression, a held `Map`'s connection frame, a plain `Map`'s value
/// type, a `Log`'s element type). `None` when the operation takes no such arguments.
pub(in crate::emitter) fn lower_query_method(
    source: QuerySource,
    op: QueryOp,
    a: Vec<LoweredExpr>,
    result_ty: Option<TyId>,
    shape: &QueryShape,
    tys: TyRender<'_>,
) -> Option<Built> {
    let keyed = shape.keyed;
    let n = a.len();
    let arm = Arm {
        source,
        a,
        result_ty,
        shape,
        tys,
    };
    Some(match (op, n) {
        // -- builders → a deferred thunk over the narrowed source --
        (QueryOp::Filter, 1) => arm.builder("filter"),
        (QueryOp::Map, 1) => arm.builder("map"),
        (QueryOp::FlatMap, 1) => arm.flat_map(),
        (QueryOp::SortBy, 1) => arm.sort_by(false),
        (QueryOp::SortByDescending, 1) => arm.sort_by(true),
        (QueryOp::Take, 1) => arm.slice(true),
        (QueryOp::Skip, 1) => arm.slice(false),
        (QueryOp::Distinct, 0) if keyed => arm.keyed_helper("__bynkDistinct", false),
        (QueryOp::DistinctBy, 1) if keyed => arm.keyed_helper("__bynkDistinctBy", false),
        (QueryOp::GroupBy, 2) if keyed => arm.keyed_helper("__bynkGroupBy", false),
        (QueryOp::JoinOn, 4) if keyed => arm.keyed_helper("__bynkJoinOn", true),
        (QueryOp::LeftJoin, 4) if keyed => arm.keyed_helper("__bynkLeftJoin", true),
        (QueryOp::Distinct, 0) => arm.distinct(),
        (QueryOp::DistinctBy, 1) => arm.distinct_by(),
        (QueryOp::JoinOn, 4) => arm.join_on(false),
        (QueryOp::LeftJoin, 4) => arm.join_on(true),
        (QueryOp::Join, 3) => arm.join(),
        (QueryOp::GroupBy, 2) => arm.group_by(),
        // -- terminals → read the source array (awaited at the `<-`) --
        (QueryOp::Collect, 0) => arm.collect(),
        (QueryOp::First, 0) => arm.first(),
        (QueryOp::FirstOrElse, 1) => arm.first_or_else(),
        (QueryOp::Count, 0) => arm.count(),
        (QueryOp::Fold, 2) => arm.fold(),
        (QueryOp::Any, 1) => arm.predicate("some"),
        (QueryOp::All, 1) => arm.predicate("every"),
        (QueryOp::Sum, 1) => arm.sum(),
        (QueryOp::Min | QueryOp::Max, 1) => arm.min_max(op == QueryOp::Min),
        (QueryOp::Average, 1) => arm.average(),
        (QueryOp::ForEach, 1) => arm.for_each(),
        (QueryOp::ParTraverse, 1) => arm.par_traverse(),
        (QueryOp::TraverseAll, 1) => arm.traverse_all(),
        (QueryOp::ParTraverseAll, 1) => arm.par_traverse_all(),
        (QueryOp::TraverseTry, 1) => arm.traverse_try(),
        (QueryOp::ParTraverseTry, 1) => arm.par_traverse_try(),
        _ => return None,
    })
}

// ---- Builders ---------------------------------------------------------------------------

impl Arm<'_> {
    /// `filter` and `map`: `__q.method((__x) => (f)(__x))`.
    fn builder(self, method: &'static str) -> Built {
        self.built(
            |o, [f]| {
                let body = method_call_bare(q(), method, vec![each(f)]);
                thunk_node(o, false, TsArrowBody::Expr(Box::new(body)))
            },
            |o, [f]| thunk_text(&o, false, &format!("__q.{method}((__x) => ({f})(__x))")),
        )
    }

    /// Storage `flatMap`: the fn returns a `Query` (a thunk), invoked on each element.
    fn flat_map(self) -> Built {
        self.built(
            |o, [f]| {
                let parts = TsExpr::Await(Box::new(call(
                    member(member(id("globalThis"), "Promise"), "all"),
                    vec![method_call_bare(
                        q(),
                        "map",
                        vec![arrow(
                            vec![param("__x", None)],
                            call(applied(f, id("__x")), vec![]),
                        )],
                    )],
                )));
                let body = vec![
                    const_("__parts", None, parts),
                    ret(method_call_bare(id("__parts"), "flat", vec![])),
                ];
                thunk_node(o, true, TsArrowBody::Multiline(body))
            },
            |o, [f]| {
                thunk_text(
                    &o,
                    true,
                    &format!(
                        "{{ const __parts = await globalThis.Promise.all(__q.map((__x) => ({f})(__x)())); return __parts.flat(); }}"
                    ),
                )
            },
        )
    }

    /// `sortBy` and `sortByDescending`: a comparator witness when the key type needs one,
    /// else the bare operators. The comparator is bound once, outside the sort.
    fn sort_by(self, descending: bool) -> Built {
        let (first, second) = sort_order(descending);
        let (lo, hi) = if descending { ("1", "-1") } else { ("-1", "1") };
        match self.shape.key_cmp.clone() {
            Some(cmp) => self.built(
                |o, [key]| {
                    let sorted = with_comparator_node(&cmp, |c| {
                        let compare = call(
                            c,
                            vec![applied(key.clone(), id(first)), applied(key, id(second))],
                        );
                        method_sort(
                            TsExpr::array(vec![spread(q())]),
                            arrow(vec![param("__a", None), param("__b", None)], compare),
                        )
                    });
                    thunk_node(o, false, TsArrowBody::Expr(Box::new(sorted)))
                },
                |o, [key]| {
                    let sorted = with_comparator(&cmp, |c| {
                        format!(
                            "[...__q].sort((__a, __b) => {c}(({key})({first}), ({key})({second})))"
                        )
                    });
                    thunk_text(&o, false, &sorted)
                },
            ),
            None => self.built(
                |o, [key]| {
                    let sorted = sort_plain_by(None, descending, q(), key);
                    thunk_node(o, false, TsArrowBody::Expr(Box::new(sorted)))
                },
                |o, [key]| {
                    thunk_text(
                        &o,
                        false,
                        &format!(
                            "[...__q].sort((__a, __b) => {{ const __ka = ({key})(__a), __kb = ({key})(__b); return __ka < __kb ? {lo} : __ka > __kb ? {hi} : 0; }})"
                        ),
                    )
                },
            ),
        }
    }

    /// `take` (`from_start`) and `skip`: a slice with the count clamped at zero.
    fn slice(self, from_start: bool) -> Built {
        self.built(
            |o, [n]| {
                let mut args = vec![clamped_at_zero(n)];
                if from_start {
                    args.insert(0, zero());
                }
                let body = method_call_bare(q(), "slice", args);
                thunk_node(o, false, TsArrowBody::Expr(Box::new(body)))
            },
            |o, [n]| {
                let start = if from_start { "0, " } else { "" };
                thunk_text(
                    &o,
                    false,
                    &format!("__q.slice({start}globalThis.Math.max(0, {n}))"),
                )
            },
        )
    }

    /// A key that is not a JS primitive goes through the `keys` runtime helper, which takes
    /// the source then each argument (a join's `other` thunk invoked and awaited first).
    /// `note_key` fired in `query_keyed`, so only when this form is chosen.
    fn keyed_helper(self, helper: &'static str, joins: bool) -> Built {
        self.built_vec(
            |o, mut args| {
                if joins {
                    let other = args.remove(0);
                    args.insert(0, awaited_thunk(other));
                }
                args.insert(0, q());
                let body = call(id(helper), args);
                thunk_node(o, joins, TsArrowBody::Expr(Box::new(body)))
            },
            |o, mut args| {
                if joins {
                    args[0] = format!("await ({})()", args[0]);
                }
                let mut all = vec!["__q".to_string()];
                all.extend(args);
                thunk_text(&o, joins, &format!("{helper}({})", all.join(", ")))
            },
        )
    }

    fn distinct(self) -> Built {
        self.built(
            |o, []| {
                let set = TsExpr::New {
                    callee: Box::new(member(id("globalThis"), "Set")),
                    args: vec![q()],
                };
                let body = TsExpr::array(vec![spread(set)]);
                thunk_node(o, false, TsArrowBody::Expr(Box::new(body)))
            },
            |o, []| thunk_text(&o, false, "[...new globalThis.Set(__q)]"),
        )
    }

    // Review of #1460: narrowed, not deferred. A first attempt used `unknown[]` here and
    // broke real `tsc --strict` fixtures (228/231): the whole expression's result flows into
    // a context expecting a specific element type, inferred *because* `__out`'s bucket was
    // left untyped for TS to infer through. The real fix is the receiver's own `Query[T]`
    // element type, `shape.elem`.
    fn distinct_by(self) -> Built {
        let elem = self.shape.elem;
        let elem_arr = array_ts(elem);
        self.built(
            |o, [key]| {
                let body = iife_block(vec![], false, distinct_by_body(elem, q(), key), vec![]);
                thunk_node(o, false, TsArrowBody::Expr(Box::new(body)))
            },
            |o, [key]| {
                thunk_text(
                    &o,
                    false,
                    &format!(
                        "(() => {{ const __seen = new globalThis.Set(); const __out: {elem_arr} = []; for (const __x of __q) {{ const __k = ({key})(__x); if (!__seen.has(__k)) {{ __seen.add(__k); __out.push(__x); }} }} return __out; }})()"
                    ),
                )
            },
        )
    }

    // v0.94 (ADR 0116/0120): joins & grouping over storage queries, lazy builders. `other`
    // is itself a `Query` thunk, invoked to materialise the probed side; the result projects
    // through `into` (no pair value). The hash key is stringified (value-keyable);
    // `groupBy`/`into` get the original key, re-derived from a representative row.
    fn join_on(self, left_join: bool) -> Built {
        let (elem, other) = (self.shape.elem, self.shape.other_elem);
        let (elem_ts, other_ts) = (print_type(elem), print_type(other));
        let other_arr = array_ts(other);
        let sides = JoinSides {
            left_join,
            elem,
            other,
        };
        self.built(
            |o, [other_q, left, right, into]| {
                let iter = paren(awaited_thunk(other_q));
                let body = join_body(&sides, iter, [left, right, into], |probe| {
                    method_call_bare(q(), "flatMap", vec![probe])
                });
                thunk_node(o, true, TsArrowBody::Multiline(body))
            },
            |o, [other_q, left, right, into]| {
                let project = if left_join {
                    format!(
                        "return __m.length > 0 ? __m.map((__u: {other_ts}) => ({into})(__t, Some(__u))) : [({into})(__t, None)];"
                    )
                } else {
                    format!("return __m.map((__u: {other_ts}) => ({into})(__t, __u));")
                };
                thunk_text(
                    &o,
                    true,
                    &format!(
                        "{{ const __h: globalThis.Record<string, {other_arr}> = globalThis.Object.create(null); for (const __u of (await ({other_q})())) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return __q.flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; {project} }}); }}"
                    ),
                )
            },
        )
    }

    /// `join`: a nested loop over a predicate.
    fn join(self) -> Built {
        self.built(
            |o, [other, on, into]| {
                let pair = |f: TsExpr| call(paren(f), vec![id("__t"), id("__u")]);
                let each_u = |body: TsExpr| arrow(vec![param("__u", None)], body);
                let matched = method_call_bare(id("__b"), "filter", vec![each_u(pair(on))]);
                let joined = method_call_bare(matched, "map", vec![each_u(pair(into))]);
                let flat = method_call_bare(
                    q(),
                    "flatMap",
                    vec![arrow(vec![param("__t", None)], joined)],
                );
                let body = vec![
                    const_("__b", None, awaited_thunk(other)),
                    ret(flat),
                ];
                thunk_node(o, true, TsArrowBody::Multiline(body))
            },
            |o, [other, on, into]| {
                thunk_text(
                    &o,
                    true,
                    &format!(
                        "{{ const __b = await ({other})(); return __q.flatMap((__t) => __b.filter((__u) => ({on})(__t, __u)).map((__u) => ({into})(__t, __u))); }}"
                    ),
                )
            },
        )
    }

    // Review of #1460: `__h`'s buckets hold the source's own rows (`shape.elem`), the
    // receiver's element type, same as `distinctBy` (`groupBy` has no `other` argument).
    fn group_by(self) -> Built {
        let elem = self.shape.elem;
        let record_ts = print_type(&TsType::named_with_args(
            "globalThis.Record",
            vec![TsType::named("string"), TsType::array(elem.clone())],
        ));
        self.built(
            |o, [key, into]| {
                let body = group_by_body(elem, q(), key, into);
                thunk_node(o, false, TsArrowBody::Multiline(body))
            },
            |o, [key, into]| {
                thunk_text(
                    &o,
                    false,
                    &format!(
                        "{{ const __h: {record_ts} = globalThis.Object.create(null); const __order: string[] = []; for (const __t of __q) {{ const __k = String(({key})(__t)); if (!(__k in __h)) {{ __h[__k] = []; __order.push(__k); }} __h[__k].push(__t); }} return __order.map((__k) => {{ const __rows = __h[__k]; return ({into})(({key})(__rows[0]), __rows); }}); }}"
                    ),
                )
            },
        )
    }
}

// ---- Terminals --------------------------------------------------------------------------

/// `(() => { body })()`, async when the body awaits.
fn iife_over_q(is_async: bool, body: Vec<TsStmt>) -> TsExpr {
    iife_block(vec![], is_async, body, vec![])
}

impl Arm<'_> {
    fn collect(self) -> Built {
        self.built(
            |o, []| terminal_node(o, q()),
            |o, []| terminal_text(&o, "__q"),
        )
    }

    fn first(self) -> Built {
        self.built(
            |o, []| {
                let found = TsExpr::Conditional {
                    test: Box::new(binary(
                        TsBinaryOp::GreaterThan,
                        member(id("__a"), "length"),
                        zero(),
                    )),
                    consequent: Box::new(call(id("Some"), vec![first_of_a()])),
                    alternate: Box::new(id("None")),
                };
                let body = vec![const_("__a", None, q()), ret(found)];
                terminal_node(o, iife_over_q(false, body))
            },
            |o, []| {
                terminal_text(
                    &o,
                    "(() => { const __a = __q; return __a.length > 0 ? Some(__a[0]) : None; })()",
                )
            },
        )
    }

    fn first_or_else(self) -> Built {
        self.built(
            |o, [default]| {
                let found = TsExpr::Conditional {
                    test: Box::new(binary(
                        TsBinaryOp::GreaterThan,
                        member(id("__a"), "length"),
                        zero(),
                    )),
                    consequent: Box::new(first_of_a()),
                    alternate: Box::new(paren(default)),
                };
                let body = vec![const_("__a", None, q()), ret(found)];
                terminal_node(o, iife_over_q(false, body))
            },
            |o, [default]| {
                terminal_text(
                    &o,
                    &format!(
                        "(() => {{ const __a = __q; return __a.length > 0 ? __a[0] : ({default}); }})()"
                    ),
                )
            },
        )
    }

    fn count(self) -> Built {
        self.built(
            |o, []| terminal_node(o, member(q(), "length")),
            |o, []| terminal_text(&o, "__q.length"),
        )
    }

    fn fold(self) -> Built {
        self.built(
            |o, [init, f]| {
                let step = assign(
                    id("__acc"),
                    call(paren(f), vec![id("__acc"), id("__x")]),
                );
                let body = vec![
                    let_("__acc", init),
                    for_of("__x", q(), step),
                    ret(id("__acc")),
                ];
                terminal_node(o, iife_over_q(false, body))
            },
            |o, [init, f]| {
                terminal_text(
                    &o,
                    &format!(
                        "(() => {{ let __acc = {init}; for (const __x of __q) __acc = ({f})(__acc, __x); return __acc; }})()"
                    ),
                )
            },
        )
    }

    /// `any` and `all`.
    fn predicate(self, method: &'static str) -> Built {
        self.built(
            |o, [p]| terminal_node(o, method_call_bare(q(), method, vec![each(p)])),
            |o, [p]| terminal_text(&o, &format!("__q.{method}((__x) => ({p})(__x))")),
        )
    }

    // #3421: a `Decimal` key sums exactly, as `List.sum` does (#2622): from the zero decimal
    // through the runtime's exact add, never a float `+`.
    fn sum(self) -> Built {
        let tys = self.tys;
        let decimal = matches!(
            self.result_ty.map(|t| tys.get(t)).as_deref(),
            Some(Ty::Effect(inner)) if matches!(&*tys.get(*inner), Ty::Base(BaseType::Decimal))
        );
        if decimal {
            self.shape.runtime_use.note_decimal("__bynkDecimalAdd");
        }
        self.built(
            |o, [key]| {
                let (acc_ty, add, init) = if decimal {
                    (
                        TsType::named(crate::emitter::ts_types::DECIMAL_TS_TYPE),
                        call(
                            id("__bynkDecimalAdd"),
                            vec![id("__s"), applied(key, id("__x"))],
                        ),
                        decimal_lit("0", 0),
                    )
                } else {
                    (
                        TsType::named("number"),
                        binary(TsBinaryOp::Add, id("__s"), applied(key, id("__x"))),
                        zero(),
                    )
                };
                let step = arrow(vec![param("__s", Some(acc_ty)), param("__x", None)], add);
                terminal_node(o, method_call_bare(q(), "reduce", vec![step, init]))
            },
            |o, [key]| {
                if decimal {
                    terminal_text(
                        &o,
                        &format!(
                            "__q.reduce((__s: {}, __x) => __bynkDecimalAdd(__s, ({key})(__x)), {})",
                            crate::emitter::ts_types::DECIMAL_TS_TYPE,
                            crate::emitter::ts_types::decimal_ts_literal("0", 0)
                        ),
                    )
                } else {
                    terminal_text(
                        &o,
                        &format!("__q.reduce((__s: number, __x) => __s + ({key})(__x), 0)"),
                    )
                }
            },
        )
    }

    /// `min` and `max`: the first key, then each key that orders before (`<`) or after (`>`)
    /// the running one. A comparator witness is bound once around the whole terminal.
    fn min_max(self, is_min: bool) -> Built {
        let cmp_op = if is_min {
            TsBinaryOp::LessThan
        } else {
            TsBinaryOp::GreaterThan
        };
        let cmp = if is_min { "<" } else { ">" };
        let key_cmp = self.shape.key_cmp.clone();
        self.built(
            |o, [key]| {
                let kernel = |better: TsExpr| {
                    let take_key = TsStmt::if_stmt(better, assign(id("__m"), id("__k")), None);
                    let scan = block(vec![
                        const_("__k", None, applied(key.clone(), id("__x"))),
                        take_key,
                    ]);
                    iife_over_q(
                        false,
                        vec![
                            const_("__a", None, q()),
                            TsStmt::if_stmt(
                                binary(TsBinaryOp::StrictEq, member(id("__a"), "length"), zero()),
                                ret(id("None")),
                                None,
                            ),
                            let_("__m", applied(key.clone(), first_of_a())),
                            for_of("__x", id("__a"), scan),
                            ret(call(id("Some"), vec![id("__m")])),
                        ],
                    )
                };
                match &key_cmp {
                    None => terminal_node(o, kernel(binary(cmp_op, id("__k"), id("__m")))),
                    Some(key_cmp) => with_comparator_node(key_cmp, |c| {
                        let better = binary(
                            cmp_op,
                            call(c, vec![id("__k"), id("__m")]),
                            zero(),
                        );
                        terminal_node(o, kernel(better))
                    }),
                }
            },
            |o, [key]| match &key_cmp {
                None => terminal_text(
                    &o,
                    &format!(
                        "(() => {{ const __a = __q; if (__a.length === 0) return None; let __m = ({key})(__a[0]); for (const __x of __a) {{ const __k = ({key})(__x); if (__k {cmp} __m) __m = __k; }} return Some(__m); }})()"
                    ),
                ),
                Some(key_cmp) => with_comparator(key_cmp, |c| {
                    terminal_text(
                        &o,
                        &format!(
                            "(() => {{ const __a = __q; if (__a.length === 0) return None; let __m = ({key})(__a[0]); for (const __x of __a) {{ const __k = ({key})(__x); if ({c}(__k, __m) {cmp} 0) __m = __k; }} return Some(__m); }})()"
                        ),
                    )
                }),
            },
        )
    }

    fn average(self) -> Built {
        let tys = self.tys;
        // Duration averages round to integer millis (checker result decides).
        let round = matches!(
            self.result_ty.map(|t| tys.get(t)).as_deref(),
            Some(Ty::Effect(inner))
                if matches!(&*tys.get(*inner), Ty::Option(o)
                    if matches!(&*tys.get(*o), Ty::Base(BaseType::Duration)))
        );
        let mean = if round {
            "globalThis.Math.round(__s / __a.length)"
        } else {
            "__s / __a.length"
        };
        self.built(
            |o, [key]| {
                let ratio = binary(TsBinaryOp::Div, id("__s"), member(id("__a"), "length"));
                let mean = if round {
                    call(
                        member(member(id("globalThis"), "Math"), "round"),
                        vec![ratio],
                    )
                } else {
                    ratio
                };
                let add = expr_stmt(TsExpr::AddAssign {
                    target: Box::new(id("__s")),
                    value: Box::new(applied(key, id("__x"))),
                });
                let body = vec![
                    const_("__a", None, q()),
                    TsStmt::if_stmt(
                        binary(TsBinaryOp::StrictEq, member(id("__a"), "length"), zero()),
                        ret(id("None")),
                        None,
                    ),
                    let_("__s", zero()),
                    for_of("__x", id("__a"), add),
                    ret(call(id("Some"), vec![mean])),
                ];
                terminal_node(o, iife_over_q(false, body))
            },
            |o, [key]| {
                terminal_text(
                    &o,
                    &format!(
                        "(() => {{ const __a = __q; if (__a.length === 0) return None; let __s = 0; for (const __x of __a) __s += ({key})(__x); return Some({mean}); }})()"
                    ),
                )
            },
        )
    }

    fn for_each(self) -> Built {
        self.built(
            |o, [f]| {
                let each_x = block(vec![expr_stmt(awaited_apply(f))]);
                let body = vec![for_of("__x", q(), each_x)];
                terminal_node(o, iife_over_q(true, body))
            },
            |o, [f]| {
                terminal_text(
                    &o,
                    &format!(
                        "(async () => {{ for (const __x of __q) {{ await ({f})(__x); }} }})()"
                    ),
                )
            },
        )
    }

    // v0.107 (slice 4): the parallel broadcast form — issue the effectful fn over every
    // element concurrently and await them together, so one slow element does not
    // head-of-line-block the rest.
    fn par_traverse(self) -> Built {
        self.built(
            |o, [f]| {
                let body = vec![expr_stmt(settled(f))];
                terminal_node(o, iife_over_q(true, body))
            },
            |o, [f]| {
                terminal_text(
                    &o,
                    &format!(
                        "(async () => {{ await __bynkSettleAll(__q.map((__x) => () => ({f})(__x))); }})()"
                    ),
                )
            },
        )
    }

    // v0.149 (ADR 0173): the collect-all terminals over a query/broadcast — gather every
    // `Result` outcome (an `Err` is a value, so nothing rejects).
    fn traverse_all(self) -> Built {
        let tys = self.tys;
        let res = match self.result_ty.map(|t| tys.get(t)).as_deref() {
            Some(Ty::Effect(inner)) => match &*tys.get(*inner) {
                Ty::List(el) => ts_ty_to_ts_type(*el, tys),
                _ => ts_ty_to_ts_type(*inner, tys),
            },
            _ => self
                .fault_site()
                .missing_fact_ty("`Effect[List[_]]` type for a `traverseAll` call"),
        };
        let res_arr = array_ts(&res);
        self.built(
            |o, [f]| {
                let push = expr_stmt(method_call_bare(
                    id("__out"),
                    "push",
                    vec![awaited_apply(f)],
                ));
                let body = vec![
                    const_("__out", Some(TsType::array(res.clone())), TsExpr::array(vec![])),
                    for_of("__x", q(), block(vec![push])),
                    ret(id("__out")),
                ];
                terminal_node(o, iife_over_q(true, body))
            },
            |o, [f]| {
                terminal_text(
                    &o,
                    &format!(
                        "(async () => {{ const __out: {res_arr} = []; for (const __x of __q) {{ __out.push(await ({f})(__x)); }} return __out; }})()"
                    ),
                )
            },
        )
    }

    fn par_traverse_all(self) -> Built {
        self.built(
            |o, [f]| {
                let body = arrow_with(vec![], true, settled(f));
                terminal_node(o, call(body, vec![]))
            },
            |o, [f]| {
                terminal_text(
                    &o,
                    &format!(
                        "(async () => await __bynkSettleAll(__q.map((__x) => () => ({f})(__x))))()"
                    ),
                )
            },
        )
    }

    // v0.150 (ADR 0174): the short-circuit collect terminals — stop at the first `Err`,
    // returning `Result[List[U], E]`.
    fn traverse_try(self) -> Built {
        let u = list_ok_elem(self.result_ty, self.tys, self.fault_site());
        let u_arr = array_ts(&u);
        self.built(
            |o, [f]| {
                let step = vec![
                    const_("__r", None, awaited_apply(f)),
                    bail_on_err(),
                    push_value(),
                ];
                let body = vec![
                    const_("__out", Some(TsType::array(u.clone())), TsExpr::array(vec![])),
                    for_of("__x", q(), block(step)),
                    ret(call(id("Ok"), vec![id("__out")])),
                ];
                terminal_node(o, iife_over_q(true, body))
            },
            |o, [f]| {
                terminal_text(
                    &o,
                    &format!(
                        "(async () => {{ const __out: {u_arr} = []; for (const __x of __q) {{ const __r = await ({f})(__x); if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})()"
                    ),
                )
            },
        )
    }

    fn par_traverse_try(self) -> Built {
        let u = list_ok_elem(self.result_ty, self.tys, self.fault_site());
        let u_arr = array_ts(&u);
        self.built(
            |o, [f]| {
                let scan = vec![bail_on_err(), push_value()];
                let body = vec![
                    const_("__rs", None, settled(f)),
                    const_("__out", Some(TsType::array(u.clone())), TsExpr::array(vec![])),
                    for_of("__r", id("__rs"), block(scan)),
                    ret(call(id("Ok"), vec![id("__out")])),
                ];
                terminal_node(o, iife_over_q(true, body))
            },
            |o, [f]| {
                terminal_text(
                    &o,
                    &format!(
                        "(async () => {{ const __rs = await __bynkSettleAll(__q.map((__x) => () => ({f})(__x))); const __out: {u_arr} = []; for (const __r of __rs) {{ if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})()"
                    ),
                )
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bynk_check::checker::Types;

    use super::*;
    use crate::emitter::ts_types::no_spellings;

    /// `(op, arguments, keyed)`: every operation the vocabulary lowers, over identifier
    /// arguments, and each keyed form beside its plain one.
    const CASES: &[(QueryOp, &[&str], bool)] = &[
        (QueryOp::Filter, &["p"], false),
        (QueryOp::Map, &["f"], false),
        (QueryOp::FlatMap, &["f"], false),
        (QueryOp::SortBy, &["key"], false),
        (QueryOp::SortByDescending, &["key"], false),
        (QueryOp::Take, &["n"], false),
        (QueryOp::Skip, &["n"], false),
        (QueryOp::Distinct, &[], false),
        (QueryOp::Distinct, &[], true),
        (QueryOp::DistinctBy, &["key"], false),
        (QueryOp::DistinctBy, &["key"], true),
        (QueryOp::GroupBy, &["key", "into"], false),
        (QueryOp::GroupBy, &["key", "into"], true),
        (QueryOp::JoinOn, &["other", "left", "right", "into"], false),
        (QueryOp::JoinOn, &["other", "left", "right", "into"], true),
        (
            QueryOp::LeftJoin,
            &["other", "left", "right", "into"],
            false,
        ),
        (QueryOp::LeftJoin, &["other", "left", "right", "into"], true),
        (QueryOp::Join, &["other", "on", "into"], false),
        (QueryOp::Collect, &[], false),
        (QueryOp::First, &[], false),
        (QueryOp::FirstOrElse, &["d"], false),
        (QueryOp::Count, &[], false),
        (QueryOp::Fold, &["init", "f"], false),
        (QueryOp::Any, &["p"], false),
        (QueryOp::All, &["p"], false),
        (QueryOp::Sum, &["key"], false),
        (QueryOp::Min, &["key"], false),
        (QueryOp::Max, &["key"], false),
        (QueryOp::Average, &["key"], false),
        (QueryOp::ForEach, &["f"], false),
        (QueryOp::ParTraverse, &["f"], false),
        (QueryOp::TraverseAll, &["f"], false),
        (QueryOp::ParTraverseAll, &["f"], false),
        (QueryOp::TraverseTry, &["f"], false),
        (QueryOp::ParTraverseTry, &["f"], false),
    ];

    /// What every case prints: `=== <op> keyed=<bool> cmp=<plain|bare|built>`, the node's
    /// print, then `--- text` and the one-line text the arm printed before it lowered to
    /// nodes. The text is the contract the fallback keeps byte for byte.
    const PRINTS: &str = include_str!("query_cases.txt");

    fn values(source: LoweredExpr) -> QuerySource {
        QuerySource::Values(source)
    }

    fn named(n: &str) -> TsType {
        TsType::named(n)
    }

    fn arg_nodes(args: &[&str]) -> Vec<LoweredExpr> {
        args.iter().map(|a| LoweredExpr::Node(id(a))).collect()
    }

    fn arg_texts(args: &[&str]) -> Vec<LoweredExpr> {
        args.iter()
            .map(|a| LoweredExpr::Text((*a).to_string()))
            .collect()
    }

    /// A comparator that is a bare identifier (substituted), and one that is built (bound
    /// once as `__c`).
    fn comparator(label: &str) -> Option<TsExpr> {
        match label {
            "plain" => None,
            "bare" => Some(id("__bynkCmpFloat")),
            "built" => Some(call(
                id("__bynkCmpSum"),
                vec![TsExpr::array(vec![]), id("w")],
            )),
            other => panic!("unknown comparator `{other}`"),
        }
    }

    struct Env {
        tys: Arc<Types>,
        runtime_use: RuntimeUse,
    }

    fn env() -> Env {
        Env {
            tys: Arc::new(Types::new()),
            runtime_use: RuntimeUse::default(),
        }
    }

    /// The call's checked type the collect operations read: `Effect[List[Int]]` for `traverseAll`,
    /// `Effect[Result[List[Int], Int]]` for the short-circuit ones, and none for the rest.
    fn result_ty(env: &Env, op: QueryOp) -> Option<TyId> {
        let int = env.tys.intern(Ty::Base(BaseType::Int));
        let list = env.tys.intern(Ty::List(int));
        match op {
            QueryOp::TraverseAll => Some(env.tys.intern(Ty::Effect(list))),
            QueryOp::TraverseTry | QueryOp::ParTraverseTry => {
                let result = env.tys.intern(Ty::Result(list, int));
                Some(env.tys.intern(Ty::Effect(result)))
            }
            _ => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lower(
        env: &Env,
        op: QueryOp,
        source: QuerySource,
        args: Vec<LoweredExpr>,
        elems: (&TsType, &TsType),
        keyed: bool,
        key_cmp: Option<TsExpr>,
        result_ty: Option<TyId>,
    ) -> Option<Built> {
        let shape = QueryShape {
            elem: elems.0,
            other_elem: elems.1,
            keyed,
            key_cmp,
            runtime_use: &env.runtime_use,
            span: bynk_syntax::span::Span::new(3, 8),
        };
        lower_query_method(
            source,
            op,
            args,
            result_ty,
            &shape,
            TyRender::new(&env.tys, no_spellings()),
        )
    }

    fn node_print(built: Option<Built>) -> String {
        match built {
            Some(Ok(node)) => bynk_ts::print_expr(&node),
            other => panic!("a node was expected, found {:?}", other.map(|b| b.err())),
        }
    }

    fn text_of(built: Option<Built>) -> String {
        match built {
            Some(Err(text)) => text,
            other => panic!("text was expected, found {:?}", other.map(|b| b.is_ok())),
        }
    }

    /// One recorded case: the operation over identifier arguments, how its key is handled,
    /// the node's print and the text it printed before it lowered to nodes.
    struct Recorded {
        op: QueryOp,
        args: &'static [&'static str],
        keyed: bool,
        cmp: &'static str,
        node: &'static str,
        text: &'static str,
    }

    fn recorded() -> Vec<Recorded> {
        PRINTS
            .split("\n=== ")
            .map(|block| block.trim_start_matches("=== "))
            .filter(|b| !b.is_empty())
            .map(|block| {
                let (head, rest) = block.split_once('\n').expect("a header");
                let (node, text) = rest.split_once("\n--- text\n").expect("a text");
                let mut words = head.split(' ');
                let name = words.next().expect("an operation");
                let keyed = words.next().expect("keyed") == "keyed=true";
                let cmp = words.next().expect("cmp").trim_start_matches("cmp=");
                let (op, args, _) = CASES
                    .iter()
                    .find(|(op, _, k)| format!("{op:?}") == name && *k == keyed)
                    .expect("a known case");
                Recorded {
                    op: *op,
                    args,
                    keyed,
                    cmp,
                    node: node.trim_end(),
                    text: text.trim_end(),
                }
            })
            .collect()
    }

    /// Every operation, over a node source and node arguments, prints exactly as recorded,
    /// with no text exit (an `Ok` node), for each way a comparator is bound.
    #[test]
    fn every_operation_over_nodes_prints_as_recorded() {
        let env = env();
        let (elem, other) = (named("Item"), named("Other"));
        let cases = recorded();
        assert_eq!(cases.len(), 41);
        for Recorded {
            op,
            args,
            keyed,
            cmp,
            node,
            ..
        } in cases
        {
            let built = lower(
                &env,
                op,
                values(LoweredExpr::Node(id("src"))),
                arg_nodes(args),
                (&elem, &other),
                keyed,
                comparator(cmp),
                result_ty(&env, op),
            );
            assert_eq!(node_print(built), node, "{op:?} keyed={keyed} cmp={cmp}");
        }
    }

    /// A text source, or any text argument, keeps today's one-line text byte for byte, for
    /// every arm: the mixed-children rule of `node_or_text`.
    #[test]
    fn a_text_source_or_argument_keeps_the_text_byte_for_byte() {
        let env = env();
        let (elem, other) = (named("Item"), named("Other"));
        for Recorded {
            op,
            args,
            keyed,
            cmp,
            text,
            ..
        } in recorded()
        {
            let from_source = lower(
                &env,
                op,
                values(LoweredExpr::Text("src".to_string())),
                arg_nodes(args),
                (&elem, &other),
                keyed,
                comparator(cmp),
                result_ty(&env, op),
            );
            assert_eq!(text_of(from_source), text, "{op:?} keyed={keyed} cmp={cmp}");
            if !args.is_empty() {
                // Each argument in turn is the one text child.
                for at in 0..args.len() {
                    let mut mixed = arg_nodes(args);
                    mixed[at] = LoweredExpr::Text(args[at].to_string());
                    let built = lower(
                        &env,
                        op,
                        values(LoweredExpr::Node(id("src"))),
                        mixed,
                        (&elem, &other),
                        keyed,
                        comparator(cmp),
                        result_ty(&env, op),
                    );
                    assert_eq!(
                        text_of(built),
                        text,
                        "{op:?} keyed={keyed} cmp={cmp} arg {at}"
                    );
                }
                let all_text = lower(
                    &env,
                    op,
                    values(LoweredExpr::Node(id("src"))),
                    arg_texts(args),
                    (&elem, &other),
                    keyed,
                    comparator(cmp),
                    result_ty(&env, op),
                );
                assert_eq!(text_of(all_text), text, "{op:?} text arguments");
            }
        }
    }

    /// A collect whose call the checker recorded no `Effect[..]` type for is a missing type fact
    /// (1A.34), recorded at the operation's span; the operation still lowers.
    #[test]
    fn a_collect_without_a_result_type_is_a_missing_fact() {
        let (elem, other) = (named("Item"), named("Other"));
        for op in [
            QueryOp::TraverseAll,
            QueryOp::TraverseTry,
            QueryOp::ParTraverseTry,
        ] {
            let env = env();
            let built = lower(
                &env,
                op,
                values(LoweredExpr::Node(id("src"))),
                arg_nodes(&["f"]),
                (&elem, &other),
                false,
                None,
                None,
            );
            assert!(built.is_some(), "{op:?} still lowers");
            let faults = env.runtime_use.take_faults();
            assert_eq!(faults.len(), 1, "{op:?}: {faults:?}");
            assert_eq!(faults[0].span, bynk_syntax::span::Span::new(3, 8), "{op:?}");
        }
    }

    /// A wrong arity, or an operation the vocabulary does not own, is declined with nothing
    /// built.
    #[test]
    fn an_operation_of_the_wrong_arity_is_declined() {
        let env = env();
        let e = named("Item");
        assert!(
            lower(
                &env,
                QueryOp::Filter,
                values(LoweredExpr::Node(id("src"))),
                vec![],
                (&e, &e),
                false,
                None,
                None
            )
            .is_none()
        );
    }

    /// The element type is a node: the printer brackets `T[]` exactly where a bare `[]` would
    /// bind wrongly (a union, an intersection, a function, a `readonly` array), and not for an
    /// operator that is only nested in a generic argument. The text fallback prints the same
    /// type the same way.
    #[test]
    fn the_element_type_is_bracketed_by_the_printer() {
        let env = env();
        let ab = || TsType::union(vec![named("A"), named("B")]);
        let cases: Vec<(&str, TsType)> = vec![
            ("(A | B)[]", ab()),
            (
                "(A & B)[]",
                TsType::intersection(vec![named("A"), named("B")]),
            ),
            (
                "((a0: number) => string)[]",
                TsType::Fn {
                    params: vec![named("number")],
                    names: vec![],
                    ret: Box::new(named("string")),
                },
            ),
            ("(readonly A[])[]", TsType::readonly_array(named("A"))),
            // Redundant before this slice: the operator is inside a generic argument.
            (
                "Option<A | B>[]",
                TsType::named_with_args("Option", vec![ab()]),
            ),
            ("Item[]", named("Item")),
        ];
        for (want, elem) in cases {
            let want = format!("const __out: {want} = [];");
            let built = |source: QuerySource, key: LoweredExpr| {
                lower(
                    &env,
                    QueryOp::DistinctBy,
                    source,
                    vec![key],
                    (&elem, &elem),
                    false,
                    None,
                    None,
                )
            };
            let node = node_print(built(
                values(LoweredExpr::Node(id("src"))),
                LoweredExpr::Node(id("key")),
            ));
            assert!(node.contains(&want), "{want}\n{node}");
            let text = text_of(built(
                values(LoweredExpr::Text("src".into())),
                LoweredExpr::Node(id("key")),
            ));
            assert!(text.contains(&want), "{want}\n{text}");
        }
    }

    /// A `Decimal` key sums from the zero decimal through the runtime's exact add, and notes
    /// that helper; any other key sums as a number.
    #[test]
    fn a_decimal_sum_notes_its_helper() {
        let env = env();
        let decimal = env.tys.intern(Ty::Base(BaseType::Decimal));
        let effect = env.tys.intern(Ty::Effect(decimal));
        let e = named("Item");
        let node = node_print(lower(
            &env,
            QueryOp::Sum,
            values(LoweredExpr::Node(id("src"))),
            arg_nodes(&["key"]),
            (&e, &e),
            false,
            None,
            Some(effect),
        ));
        assert!(
            node.contains("__bynkDecimalAdd(__s, (key)(__x)), { coefficient: 0n, scale: 0 })"),
            "{node}"
        );
        assert!(env.runtime_use.cmp_imports().contains("__bynkDecimalAdd"));
        let plain = self::env();
        lower(
            &plain,
            QueryOp::Sum,
            values(LoweredExpr::Node(id("src"))),
            arg_nodes(&["key"]),
            (&e, &e),
            false,
            None,
            None,
        );
        assert!(plain.runtime_use.cmp_imports().is_empty());
    }

    /// A `Duration` average rounds to whole milliseconds, the checker's result type deciding.
    #[test]
    fn a_duration_average_rounds() {
        let env = env();
        let duration = env.tys.intern(Ty::Base(BaseType::Duration));
        let option = env.tys.intern(Ty::Option(duration));
        let effect = env.tys.intern(Ty::Effect(option));
        let e = named("Item");
        for (result, want) in [
            (
                Some(effect),
                "Some(globalThis.Math.round(__s / __a.length))",
            ),
            (None, "Some(__s / __a.length)"),
        ] {
            let node = node_print(lower(
                &env,
                QueryOp::Average,
                values(LoweredExpr::Node(id("src"))),
                arg_nodes(&["key"]),
                (&e, &e),
                false,
                None,
                result,
            ));
            assert!(node.contains(want), "{node}");
        }
    }

    /// `Stream` generators have no node (no `yield` or `for await` in `bynk_ts`): they stay
    /// text, byte for byte.
    #[test]
    fn the_stream_generators_stay_text() {
        let claim = STREAM_CLAIM;
        assert_eq!(
            lower_stream_method("src".into(), StreamOp::Map, &["f".to_string()]),
            Some(format!(
                "(async function* (__s) {{ {claim} for await (const __e of __s) {{ yield (f)(__e); }} }})(src)"
            ))
        );
        assert_eq!(
            lower_stream_method("src".into(), StreamOp::Take, &["n".to_string()]),
            Some(format!(
                "(async function* (__s) {{ {claim} const __n = n; if (__n <= 0) {{ return; }} let __i = 0; for await (const __e of __s) {{ yield __e; if (++__i >= __n) {{ return; }} }} }})(src)"
            ))
        );
        assert_eq!(
            lower_stream_method("src".into(), StreamOp::Collect, &[]),
            Some(format!(
                "(async (__s) => {{ {claim} const __r = []; for await (const __e of __s) {{ __r.push(__e); }} return __r; }})(src)"
            ))
        );
    }

    /// A `Query` receiver is a thunk, invoked as `(recv)()` before the operation reads it: a
    /// node over a node receiver, `(recv)()` as text over a text one.
    #[test]
    fn a_query_receiver_is_invoked() {
        let env = env();
        let e = named("Item");
        let over = |recv: LoweredExpr| {
            lower(
                &env,
                QueryOp::Count,
                QuerySource::Thunk(recv),
                vec![],
                (&e, &e),
                false,
                None,
                None,
            )
        };
        assert_eq!(
            node_print(over(LoweredExpr::Node(id("recv")))),
            "__bynkThen((recv)(), (__q) => __q.length)"
        );
        assert_eq!(
            text_of(over(LoweredExpr::Text("a ? b : c".into()))),
            "__bynkThen((a ? b : c)(), (__q) => __q.length)"
        );
    }

    /// The caller-side sources: a store field's entries, and a
    /// held `Map`'s resolved connections, each a node over identifier
    /// paths (a text receiver is invoked as text).
    #[test]
    fn the_callers_sources_are_nodes_over_plain_paths() {
        let print = |e: TsExpr| bynk_ts::print_expr(&e);
        assert_eq!(
            print(path_values_source(member(id("__state"), "items"))),
            "__state.items.values()"
        );
        assert_eq!(
            print(path_values_source(member(
                super::super::super::lower::member_node("this", "state"),
                "log"
            ))),
            "this.state.log.values()"
        );
        let frame = named("Frame");
        let held = "__state.conns.values().flatMap((__cid) => { const __c = __resolveConnection<Frame>(this.state, __cid); return __c.tag === \"Some\" ? [__c.value] : []; })";
        assert_eq!(
            print(held_values_source(member(id("__state"), "conns"), &frame)),
            held
        );
    }

    const BAG: &str = "context demo.bag

type Blob = opaque Bytes

agent Bag {
  key id: String
  store nums: Map[String, Int]
  store blobs: Map[String, Bytes]

  on call plain() -> Effect[List[Int]] {
    nums.distinct().collect()
  }
  on call keyed() -> Effect[List[Bytes]] {
    blobs.distinct().collect()
  }
}
";

    const ROOM: &str = "context demo.room

type Frame = { text: String }

agent Room {
  key id: String
  store conns: Map[String, Connection[Frame]]

  on call announce(text: String) -> Effect[()] {
    conns.parTraverse((c: Connection[Frame]) => c.send(Frame { text: text }))
  }
}
";

    fn emitted(
        file: &str,
        source: &str,
        target: bynk_check::project_model::BuildTarget,
        suffix: &str,
    ) -> String {
        let out = crate::testkit::emit_project(&[(file, source)], target);
        let (_, ts) = out
            .docs
            .iter()
            .find(|(path, _)| path.to_string_lossy().ends_with(suffix))
            .expect("the module is emitted");
        ts.clone()
    }

    /// A non-primitive key goes through the keyed helper and imports it, only where it is
    /// used (`note_key` fires on the keyed branch alone); a primitive key stays a `Set`.
    #[test]
    fn a_keyed_helper_is_imported_only_where_it_is_chosen() {
        let ts = emitted(
            "demo/bag.bynk",
            BAG,
            bynk_check::project_model::BuildTarget::Bundle,
            "demo/bag.ts",
        );
        assert!(ts.contains("(__q) => [...new globalThis.Set(__q)]"), "{ts}");
        assert!(ts.contains("(__q) => __bynkDistinct(__q)"), "{ts}");
        let imports = ts
            .lines()
            .filter(|l| l.starts_with("import "))
            .collect::<String>();
        assert!(imports.contains("__bynkDistinct"), "{imports}");
        assert!(!imports.contains("__bynkDistinctBy"), "{imports}");
        let plain = BAG
            .replace("blobs.distinct()", "nums.distinct()")
            .replace("Effect[List[Bytes]]", "Effect[List[Int]]");
        let ts = emitted(
            "demo/bag.bynk",
            &plain,
            bynk_check::project_model::BuildTarget::Bundle,
            "demo/bag.ts",
        );
        assert!(!ts.contains("__bynkDistinct"), "{ts}");
    }

    /// A held `Map`'s resolved connections are the source, and `parTraverse` joins through
    /// `__bynkSettleAll`, which the module then imports.
    #[test]
    fn a_held_map_broadcast_resolves_its_connections_and_imports_the_settle_helper() {
        let ts = emitted(
            "demo/room.bynk",
            ROOM,
            bynk_check::project_model::BuildTarget::Workers,
            "demo-room/handlers.ts",
        );
        assert!(
            ts.contains("__resolveConnection<Frame>(this.state, __cid)"),
            "{ts}"
        );
        assert!(
            ts.contains("await __bynkSettleAll(__q.map((__x) => () =>"),
            "{ts}"
        );
        assert!(ts.contains("__bynkSettleAll"), "{ts}");
    }
}
