//! TypeScript type rendering: `ts_type_ref*`, `ts_ty`, and the `TsType` builders.

use super::*;

/// Unchecked construction of a branded value in emitted TypeScript.
///
/// ADR 0182: an **opaque** type exposes a runtime `.unsafe(value)` constructor
/// (source-callable within its defining commons, and the target of its internal
/// uses), so opaque construction stays `T.unsafe(value)`. A **refined** or
/// **alias** type has **no** public `.unsafe`: exposing one let hand-written host
/// or adapter code bypass the refinement predicate, the credibility hole #545
/// closed. Its admitted / generated values are branded with an inline `as` cast
/// — byte-for-byte the old `.unsafe` body (`return value as T`) at the call site,
/// but not a callable API surface a consumer can reach.
pub(crate) fn unchecked_construct(name: &str, value: &str, is_opaque: bool) -> String {
    if is_opaque {
        format!("{name}.unsafe({value})")
    } else {
        format!("({value} as {name})")
    }
}

/// Unchecked construction inside GENERATED TEST scaffolding (`tests/*.test.ts`).
///
/// There a branded type is in scope only as an `any`-typed value binding
/// (`const {{ T }} = ns as any`) — never as a type — so the production
/// `(value as T)` form fails to resolve `T`. Opaque still constructs through its
/// `.unsafe` value method (kept, ADR 0182); a refined/alias value brands to `any`,
/// which is exactly the type the pre-0182 `T.unsafe(value)` already produced here
/// (`T` being `any`) and erases to the raw value at runtime — without
/// reintroducing a callable refined `.unsafe`.
pub fn unchecked_construct_test(name: &str, value: &str, is_opaque: bool) -> String {
    if is_opaque {
        format!("{name}.unsafe({value})")
    } else {
        // P7.2: deferred, not narrowed — checked, not skipped. This doc comment's
        // own explanation still holds: `name` genuinely isn't resolvable as a
        // type at this call site (only as an `any`-typed value binding), so
        // there is no real type to cast to here. Fixing this needs restructuring
        // what the test scaffold imports, not a same-line text change.
        format!("({value} as any)")
    }
}

/// [`unchecked_construct_test`] as a node (1A.17): `Name.unsafe(value)` for an
/// opaque type, `(value as any)` otherwise (the cast stays: see the note there).
pub(super) fn unchecked_construct_test_node(
    name: &str,
    value: bynk_ts::TsExpr,
    is_opaque: bool,
) -> bynk_ts::TsExpr {
    if is_opaque {
        bynk_ts::TsExpr::Call {
            callee: Box::new(super::lower::member_node(name, "unsafe")),
            args: vec![value],
        }
    } else {
        bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
            expr: Box::new(value),
            ty: bynk_ts::TsType::named("any"),
        }))
    }
}

pub(super) fn ts_base(b: BaseType) -> &'static str {
    match b {
        BaseType::Int => "number",
        BaseType::String => "string",
        BaseType::Bool => "boolean",
        BaseType::Float => "number",
        BaseType::Duration | BaseType::Instant => "number",
        // v0.110 (ADR 0142): `Bytes` is the one base type that does NOT erase
        // to `number` — it lowers to an immutable octet sequence, `Uint8Array`.
        BaseType::Bytes => "globalThis.Uint8Array",
        // #2622: a `Decimal` is a frozen `{ coefficient, scale }` over `bigint`,
        // named structurally so no module needs a type import for it.
        BaseType::Decimal => DECIMAL_TS_TYPE,
    }
}

/// #2622: the TypeScript type of a `Decimal` (see `runtime/src/decimal.ts`).
pub(crate) const DECIMAL_TS_TYPE: &str = "{ readonly coefficient: bigint; readonly scale: number }";

/// #2622: a `Decimal` literal as a TypeScript object literal - no runtime
/// import. `coef` is the signed coefficient digits.
pub(crate) fn decimal_ts_literal(coef: &str, scale: u32) -> String {
    format!(
        "{{ coefficient: {}n, scale: {scale} }}",
        decimal_signed_coef(coef)
    )
}

pub fn ts_type_ref(r: &TypeRef) -> String {
    ts_type_ref_with(r, None)
}

/// [`TsType`]-returning sibling of `ts_type_ref`, qualifying named types that
/// live in `scope` with the namespace `ns` (`Order` → `Ns.Order`). Used by
/// `observation_call_record_types` (`project/tests_emit.rs`) for mock method
/// signatures that sit outside the destructuring that brings a namespace's
/// value-side names into local scope, so the types must be referenced fully
/// qualified. Qualification recurses through generic arguments; base/unit
/// types are unaffected.
///
/// Originally paired with a `String`-returning `ts_type_ref_qualified` (Decision
/// B, #1321) — `workers.rs`'s own type-position needs (Arc C slice 3) wanted
/// the structured `TsType` this function returns, added *alongside* rather
/// than replacing, since `ts_type_ref_qualified` itself was still needed by a
/// `String`-based caller. Arc C, slice 32 (`tests_emit.rs` slice B, #1399)
/// converted that one remaining caller (`observation_call_record_types`) to
/// call this function directly instead — `ts_type_ref_qualified` had no
/// other production caller left, so it was deleted rather than kept as dead
/// code (the same "close it, don't leave a substitute-around orphan" call
/// slice 6's `ts_string_literal` deletion already made); its own direct unit
/// tests below now pin this function's output through `bynk_ts::print_type`
/// instead.
///
/// Not a new structural walk — `ts_type_ref_to_ts_type` (below) already
/// builds a real `TsType` from every `TypeRef` variant; this function is
/// that same build, just with the qualifying closure threaded through.
pub fn ts_type_ref_qualified_ts_type(r: &TypeRef, scope: &HashSet<String>, ns: &str) -> TsType {
    ts_type_ref_to_ts_type(
        r,
        Some(&|name| scope.contains(name).then(|| ns.to_string())),
    )
}

/// [`TsType`]-returning sibling of `ts_type_ref`, like `ts_type_ref_qualified`
/// but with each in-scope name carrying its *own* namespace via `type_ns`
/// rather than one shared `ns` — needed when a signature mixes names owned
/// by the target unit with names reached only through a `uses`d commons
/// (e.g. a stub class implementing an adapter-sourced capability whose
/// return type lives in a commons the capability's own unit `uses`, never
/// in the target context itself — Locale capability track, slice 1, #844).
/// Qualifying such a name under the target's own namespace would reference
/// an export `emit_context_rebrands` never emits (it only rebrands names
/// the target's *own* lowered body references), so each name is qualified
/// under the namespace that actually exports it. Used by `emit_stub_class`
/// (`project/tests_emit.rs`) for its own method params/return-type.
///
/// Originally paired with a `String`-returning `ts_type_ref_qualified_multi`
/// — Arc C slice 33 (`tests_emit.rs` slice C, #1401) converted `emit_stub_
/// class`'s own two real call sites (params/return-type) to call this
/// function directly instead, the same cleanup Arc C slice 32/#1399 already
/// made for `ts_type_ref_qualified`'s own identical pairing — `ts_type_ref_
/// qualified_multi` had no other production caller left, so it was deleted
/// rather than kept as dead code, with its own direct unit test rerouted
/// through `bynk_ts::print_type` instead.
pub fn ts_type_ref_qualified_multi_ts_type(
    r: &TypeRef,
    type_ns: &HashMap<String, String>,
) -> TsType {
    ts_type_ref_to_ts_type(r, Some(&|name| type_ns.get(name).cloned()))
}

/// A name → owning-namespace lookup for `ts_type_ref_with`'s `qualify` arm.
pub(super) type QualifyFn<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Shared renderer behind `ts_type_ref` (`qualify = None`) and the two
/// `ts_type_ref_qualified*` helpers above (`qualify = Some(name -> namespace)`).
/// With `None` it is output-identical to the historic `ts_type_ref`; the only
/// divergence is the `Named`/`App` arms, which qualify in-scope names when
/// `qualify` is set.
/// P7.9 (#1315, R7.2): renders `r` by building a real [`bynk_ts::TsType`]
/// and printing it through [`bynk_ts::print_type`], instead of `format!`-ing
/// the type text by hand — this function's own signature and every one of
/// its ~110 real callers (via `ts_type_ref`/`ts_type_ref_qualified*`) are
/// unchanged; only the internal construction moved. `TypeRef` has no shape
/// this crate's `TsType` (`Named`/`Array`/`Object`/`Fn`, all extended for
/// this slice's own grounded gaps — a `readonly` modifier, and `Fn` itself)
/// can't represent — confirmed by matching every `TypeRef` variant below.
pub(super) fn ts_type_ref_with(r: &TypeRef, qualify: Option<QualifyFn<'_>>) -> String {
    bynk_ts::print_type(&ts_type_ref_to_ts_type(r, qualify))
}

/// Whether `ty` is a bare (no type arguments) `Named` type whose name is
/// exactly `target` — used by the two arms below that special-case a
/// `Promise`/function return of `void`, checking the *structure* of the
/// already-built `TsType` instead of the pre-P7.9 code's own `inner == "()"
/// || inner == "void"` text comparison. No arm in [`ts_type_ref_to_ts_type`]
/// ever builds a literal `"()"`-named type, so that half of the check is
/// dead in practice today, same as it was before this slice — inherited
/// unchanged rather than dropped, since removing "impossible" defensive
/// code isn't this slice's job.
pub(super) fn is_bare_named(ty: &TsType, target: &str) -> bool {
    matches!(ty, TsType::Named { name, type_args } if type_args.is_empty() && name == target)
}

/// #1883, slice 1a: the TypeScript shape of a Bynk `Asset` —
/// `{ readonly url: string }`, structural, so a program that only *names* the
/// type (`fn link(a: Asset) -> String`) compiles with no runtime support.
/// Each declared asset is emitted as a constant of this shape (`emit_project`).
/// #2341 (decision A): `RequestHeaders` as TypeScript — structural, so no
/// module needs a new runtime import for it: `get`, `bearer` and `clientIp`,
/// each answering an `Option<string>`. The runtime's `requestHeaders(request)`
/// builds one.
pub(crate) fn request_headers_ts_type() -> TsType {
    // `Option<string>` spelled out: a composition root imports no `Option`,
    // and the bare name there is the DOM's `Option` constructor.
    let opt = || {
        TsType::union(vec![
            TsType::Object(vec![
                bynk_ts::TsTypeMember::readonly_prop("tag", TsType::named("\"Some\"")),
                bynk_ts::TsTypeMember::readonly_prop("value", TsType::named("string")),
            ]),
            TsType::Object(vec![bynk_ts::TsTypeMember::readonly_prop(
                "tag",
                TsType::named("\"None\""),
            )]),
        ])
    };
    let name = bynk_ts::TsParam {
        name: "name".to_string(),
        ty: Some(TsType::named("string")),
        optional: false,
    };
    TsType::Object(vec![
        bynk_ts::TsTypeMember::method("get", vec![name], opt()),
        bynk_ts::TsTypeMember::method("bearer", vec![], opt()),
        bynk_ts::TsTypeMember::method("clientIp", vec![], opt()),
    ])
}

/// #2380 (L5): `TokenClaims` as TypeScript — structural, like
/// `RequestHeaders`: `get(name)` for a string claim, `has(name)` for any.
/// #2854 (decision B): `RequestQuery` as TypeScript — structural, like
/// `RequestHeaders`: `get`, `all` and `params`. The runtime's
/// `requestQuery(request)` builds one.
pub(crate) fn request_query_ts_type() -> TsType {
    let name = || bynk_ts::TsParam {
        name: "name".to_string(),
        ty: Some(TsType::named("string")),
        optional: false,
    };
    let opt = TsType::union(vec![
        TsType::Object(vec![
            bynk_ts::TsTypeMember::readonly_prop("tag", TsType::named("\"Some\"")),
            bynk_ts::TsTypeMember::readonly_prop("value", TsType::named("string")),
        ]),
        TsType::Object(vec![bynk_ts::TsTypeMember::readonly_prop(
            "tag",
            TsType::named("\"None\""),
        )]),
    ]);
    TsType::Object(vec![
        bynk_ts::TsTypeMember::method("get", vec![name()], opt),
        bynk_ts::TsTypeMember::method("all", vec![name()], TsType::named("readonly string[]")),
        bynk_ts::TsTypeMember::method(
            "params",
            vec![],
            TsType::named("globalThis.ReadonlyMap<string, readonly string[]>"),
        ),
    ])
}

/// #3277: `RequestCookies` as TypeScript — structural, like `RequestQuery`:
/// `get`, `all` and `names`. The runtime's `requestCookies(request)` builds one.
pub(crate) fn request_cookies_ts_type() -> TsType {
    let name = || bynk_ts::TsParam {
        name: "name".to_string(),
        ty: Some(TsType::named("string")),
        optional: false,
    };
    let opt = TsType::union(vec![
        TsType::Object(vec![
            bynk_ts::TsTypeMember::readonly_prop("tag", TsType::named("\"Some\"")),
            bynk_ts::TsTypeMember::readonly_prop("value", TsType::named("string")),
        ]),
        TsType::Object(vec![bynk_ts::TsTypeMember::readonly_prop(
            "tag",
            TsType::named("\"None\""),
        )]),
    ]);
    TsType::Object(vec![
        bynk_ts::TsTypeMember::method("get", vec![name()], opt),
        bynk_ts::TsTypeMember::method("all", vec![name()], TsType::named("readonly string[]")),
        bynk_ts::TsTypeMember::method("names", vec![], TsType::named("readonly string[]")),
    ])
}

pub(crate) fn token_claims_ts_type() -> TsType {
    let opt = TsType::union(vec![
        TsType::Object(vec![
            bynk_ts::TsTypeMember::readonly_prop("tag", TsType::named("\"Some\"")),
            bynk_ts::TsTypeMember::readonly_prop("value", TsType::named("string")),
        ]),
        TsType::Object(vec![bynk_ts::TsTypeMember::readonly_prop(
            "tag",
            TsType::named("\"None\""),
        )]),
    ]);
    let name = || bynk_ts::TsParam {
        name: "name".to_string(),
        ty: Some(TsType::named("string")),
        optional: false,
    };
    TsType::Object(vec![
        bynk_ts::TsTypeMember::method("get", vec![name()], opt),
        bynk_ts::TsTypeMember::method("has", vec![name()], TsType::named("boolean")),
    ])
}

pub(crate) fn asset_ts_type() -> TsType {
    TsType::Object(vec![bynk_ts::TsTypeMember::readonly_prop(
        "url",
        TsType::named("string"),
    )])
}

pub(super) fn ts_type_ref_to_ts_type(r: &TypeRef, qualify: Option<QualifyFn<'_>>) -> TsType {
    match r {
        TypeRef::Base(b, _) => TsType::named(ts_base(*b)),
        TypeRef::Named(id) => {
            let name = if let Some(f) = qualify
                && let Some(ns) = f(&id.name)
            {
                format!("{ns}.{}", id.name)
            } else {
                id.name.clone()
            };
            TsType::named(name)
        }
        TypeRef::Result(t, e, _) => TsType::named_with_args(
            "Result",
            vec![
                ts_type_ref_to_ts_type(t, qualify),
                ts_type_ref_to_ts_type(e, qualify),
            ],
        ),
        TypeRef::Option(t, _) => {
            TsType::named_with_args("Option", vec![ts_type_ref_to_ts_type(t, qualify)])
        }
        TypeRef::Effect(t, _) => {
            let inner = ts_type_ref_to_ts_type(t, qualify);
            if is_bare_named(&inner, "()") || is_bare_named(&inner, "void") {
                TsType::named_with_args("globalThis.Promise", vec![TsType::named("void")])
            } else {
                TsType::named_with_args("globalThis.Promise", vec![inner])
            }
        }
        TypeRef::HttpResult(t, _) => {
            TsType::named_with_args("HttpResult", vec![ts_type_ref_to_ts_type(t, qualify)])
        }
        // v0.20b: collections lower to immutable TS shapes.
        TypeRef::List(t, _) => TsType::readonly_array(ts_type_ref_to_ts_type(t, qualify)),
        // K6a (#2596): an immutable, insertion-ordered JS `Set`.
        TypeRef::Set(t, _) => {
            TsType::named_with_args("ReadonlySet", vec![ts_type_ref_to_ts_type(t, qualify)])
        }
        // A `Query[T]`'s own real shape is `(() => readonly T[])` — an
        // *outer* paren pair around the whole function type (found by the
        // zero-diff check this slice's own "Done when" requires: the
        // general `TsType::Fn` rendering, correctly matching the real
        // parametered-function-type case, has no such wrap, but every
        // fixture's own Query annotation does — e.g.
        // `bynk-tests/tests/fixtures/positive/302_query_annotated_let/expected/
        // probe.ts:46`). `TsType` has no general parenthesisation wrapper
        // (adding one for this single real use would be inventing a new
        // variant beyond this slice's own authorised gap list — `readonly`
        // and `Fn`, nothing else); pre-rendering the `Fn` type immediately
        // and wrapping the *text* in parens, carried as an opaque `Named`
        // with no type arguments (which prints its `name` verbatim,
        // byte-for-byte, whatever it contains), reproduces the real shape
        // exactly without widening the algebra. Worth reconsidering if a
        // second real case ever needs the same wrap.
        TypeRef::Query(t, _) => {
            let fn_ty = TsType::Fn {
                names: vec![],
                params: vec![],
                ret: Box::new(query_result_ty(ts_type_ref_to_ts_type(t, qualify))),
            };
            TsType::named(format!("({})", bynk_ts::print_type(&fn_ty)))
        }
        // v0.100: `Stream[T]` lowers to a host async iterable.
        TypeRef::Stream(t, _) => {
            TsType::named_with_args("AsyncIterable", vec![ts_type_ref_to_ts_type(t, qualify)])
        }
        // v0.102: a `Connection[F]` lowers to the runtime `Connection<F>`
        // interface (the concrete implementation arrives with the protocol).
        TypeRef::Connection(t, _) => {
            TsType::named_with_args("Connection", vec![ts_type_ref_to_ts_type(t, qualify)])
        }
        // v0.119: `History[Agent]` is a test-only generator with no emitted TS
        // type — it never reaches a signature/field position (the property runner
        // binds the driven history as an ordinary array). Rendered defensively.
        TypeRef::History(_, _) => TsType::named("never"),
        TypeRef::Map(k, v, _) => TsType::named_with_args(
            "ReadonlyMap",
            vec![
                ts_type_ref_to_ts_type(k, qualify),
                ts_type_ref_to_ts_type(v, qualify),
            ],
        ),
        TypeRef::QueueResult(_) => TsType::named("QueueResult"),
        TypeRef::Asset(_) => asset_ts_type(),
        TypeRef::RequestHeaders(_) => request_headers_ts_type(),
        TypeRef::RequestQuery(_) => request_query_ts_type(),
        TypeRef::RequestCookies(_) => request_cookies_ts_type(),
        TypeRef::TokenClaims(_) => token_claims_ts_type(),
        TypeRef::BoundaryRejection(_) => {
            TsType::named("{ readonly tag: string; readonly detail: string }")
        }
        TypeRef::ValidationError(_) => TsType::named("ValidationError"),
        TypeRef::DecodeError(_) => TsType::named("DecodeError"),
        TypeRef::Unit(_) => TsType::named("void"),
        // v0.157 (ADR 0183): `Name[Arg, …]` lowers to the erased TS generic
        // `Name<Arg, …>` — the generic record's interface is emitted with the
        // same type parameters (like a generic function's erased `<A, B>`).
        TypeRef::App { name, args, .. } => {
            // Review of #1315/#1316: pre-slice this arm interpolated `head`
            // unconditionally, so a hypothetical empty-args `App` rendered
            // `Head<>` — `TsType::named_with_args(head, vec![])` instead
            // renders bare `Head` (`TsType::Named`'s own printer only opens
            // `<...>` when `type_args` is non-empty), a real behaviour
            // change *if* this arm is ever reached with empty `args`. It
            // isn't today: `ty_to_type_ref` only ever builds `App` with a
            // non-empty argument list (this file, `ty_to_type_ref`'s own
            // `Ty::Named` arm), and the resolver arity-checks a
            // `Name[Arg, …]` application before either side ever sees one.
            // Asserted, not silently relied on, so a future caller that
            // *can* reach here with no args fails loudly instead of
            // silently changing bytes.
            debug_assert!(
                !args.is_empty(),
                "TypeRef::App is only ever built with a non-empty argument list"
            );
            let head = if let Some(f) = qualify
                && let Some(ns) = f(&name.name)
            {
                format!("{ns}.{}", name.name)
            } else {
                name.name.clone()
            };
            let rendered: Vec<TsType> = args
                .iter()
                .map(|a| ts_type_ref_to_ts_type(a, qualify))
                .collect();
            TsType::named_with_args(head, rendered)
        }
        // v0.20a: a function type lowers to a TS function type. Positional
        // parameter names (`a0`, `a1`, …) are the printer's own job now
        // (`bynk_ts::print_type`'s `TsType::Fn` rendering) — TS requires
        // names in function type syntax; an Effect return is already
        // Promise via recursion.
        TypeRef::Fn(params, ret, _) => {
            let params: Vec<TsType> = params
                .iter()
                .map(|p| ts_type_ref_to_ts_type(p, qualify))
                .collect();
            let ret = ts_type_ref_to_ts_type(ret, qualify);
            let ret = if is_bare_named(&ret, "()") {
                TsType::named("void")
            } else {
                ret
            };
            TsType::Fn {
                names: vec![],
                params,
                ret: Box::new(ret),
            }
        }
    }
}

/// P7.9 (#1315): pins `ts_type_ref`'s exact pre-slice text for a
/// representative `TypeRef` from every real shape category, the same
/// discipline this track's own history names for "prove the test would
/// actually catch it" (P7.2's `brand_assertion` gap, P7.3's escaping gap —
/// both invisible to the byte-golden fixture corpus alone). These pin the
/// literal strings `ts_type_ref_with` built by hand before this slice
/// rebuilt it to construct a real `bynk_ts::TsType` and print that instead
/// — a regression here is a regression in every type-position string the
/// compiler emits, not caught by any one fixture necessarily exercising
/// this exact shape.
#[cfg(test)]
mod ts_type_ref_tests {
    use super::*;

    fn sp() -> bynk_syntax::span::Span {
        bynk_syntax::span::Span::new(0, 0)
    }

    fn base(b: BaseType) -> TypeRef {
        TypeRef::Base(b, sp())
    }

    fn named(name: &str) -> TypeRef {
        TypeRef::Named(Ident {
            name: name.to_string(),
            span: sp(),
        })
    }

    #[test]
    fn bare_named_type() {
        assert_eq!(ts_type_ref(&named("Order")), "Order");
    }

    #[test]
    fn result_type() {
        let r = TypeRef::Result(
            Box::new(base(BaseType::Int)),
            Box::new(named("MyError")),
            sp(),
        );
        assert_eq!(ts_type_ref(&r), "Result<number, MyError>");
    }

    #[test]
    fn option_type() {
        let r = TypeRef::Option(Box::new(base(BaseType::String)), sp());
        assert_eq!(ts_type_ref(&r), "Option<string>");
    }

    #[test]
    fn effect_of_a_non_unit_type_becomes_promise() {
        let r = TypeRef::Effect(Box::new(base(BaseType::Int)), sp());
        assert_eq!(ts_type_ref(&r), "globalThis.Promise<number>");
    }

    #[test]
    fn effect_of_unit_becomes_promise_void() {
        let r = TypeRef::Effect(Box::new(TypeRef::Unit(sp())), sp());
        assert_eq!(ts_type_ref(&r), "globalThis.Promise<void>");
    }

    #[test]
    fn http_result_type() {
        let r = TypeRef::HttpResult(Box::new(named("Order")), sp());
        assert_eq!(ts_type_ref(&r), "HttpResult<Order>");
    }

    #[test]
    fn list_is_a_readonly_array() {
        let r = TypeRef::List(Box::new(named("Order")), sp());
        assert_eq!(ts_type_ref(&r), "readonly Order[]");
    }

    /// The one shape this slice's own gap analysis found: `Query`'s real
    /// text wraps the whole function type in an *extra* outer paren pair
    /// (`(() => readonly T[])`), distinct from a bare `Fn` type's own
    /// convention (no outer wrap) — caught only by this zero-diff check
    /// against `bynk-tests/tests/fixtures/positive/302_query_annotated_let/
    /// expected/probe.ts`, not by reasoning about the algebra alone.
    #[test]
    fn query_wraps_the_whole_function_type_in_parens() {
        let r = TypeRef::Query(Box::new(named("Order")), sp());
        assert_eq!(
            ts_type_ref(&r),
            "(() => readonly Order[] | globalThis.Promise<readonly Order[]>)"
        );
    }

    #[test]
    fn stream_is_an_async_iterable() {
        let r = TypeRef::Stream(Box::new(named("Order")), sp());
        assert_eq!(ts_type_ref(&r), "AsyncIterable<Order>");
    }

    #[test]
    fn connection_type() {
        let r = TypeRef::Connection(Box::new(named("Frame")), sp());
        assert_eq!(ts_type_ref(&r), "Connection<Frame>");
    }

    #[test]
    fn history_renders_defensively_as_never() {
        let r = TypeRef::History(Box::new(named("Agent")), sp());
        assert_eq!(ts_type_ref(&r), "never");
    }

    #[test]
    fn map_is_a_readonly_map() {
        let r = TypeRef::Map(
            Box::new(base(BaseType::String)),
            Box::new(named("Order")),
            sp(),
        );
        assert_eq!(ts_type_ref(&r), "ReadonlyMap<string, Order>");
    }

    #[test]
    fn the_four_bare_named_types() {
        assert_eq!(ts_type_ref(&TypeRef::QueueResult(sp())), "QueueResult");
        assert_eq!(
            ts_type_ref(&TypeRef::ValidationError(sp())),
            "ValidationError"
        );
        assert_eq!(ts_type_ref(&TypeRef::DecodeError(sp())), "DecodeError");
        assert_eq!(ts_type_ref(&TypeRef::Unit(sp())), "void");
    }

    #[test]
    fn app_is_a_generic_instantiation() {
        let r = TypeRef::App {
            name: Ident {
                name: "MyGeneric".to_string(),
                span: sp(),
            },
            args: vec![base(BaseType::Int), named("Order")],
            span: sp(),
        };
        assert_eq!(ts_type_ref(&r), "MyGeneric<number, Order>");
    }

    #[test]
    fn real_function_type_uses_positional_parameter_names() {
        let r = TypeRef::Fn(
            vec![base(BaseType::String), base(BaseType::Int)],
            Box::new(named("Order")),
            sp(),
        );
        assert_eq!(ts_type_ref(&r), "(a0: string, a1: number) => Order");
    }

    #[test]
    fn base_types() {
        assert_eq!(ts_type_ref(&base(BaseType::Int)), "number");
        assert_eq!(ts_type_ref(&base(BaseType::Float)), "number");
        assert_eq!(ts_type_ref(&base(BaseType::Duration)), "number");
        assert_eq!(ts_type_ref(&base(BaseType::Instant)), "number");
        assert_eq!(ts_type_ref(&base(BaseType::String)), "string");
        assert_eq!(ts_type_ref(&base(BaseType::Bool)), "boolean");
        assert_eq!(ts_type_ref(&base(BaseType::Bytes)), "globalThis.Uint8Array");
    }

    /// Coverage gap named in review of #1315/#1316: every test above goes
    /// through `ts_type_ref` (`qualify = None`) — the qualifying branches
    /// this slice's own refactor most directly restructured (`App`'s `head`
    /// now flows through `TsType::named_with_args` instead of being
    /// interpolated) were unpinned. Asserts both that the head qualifies
    /// *and* that qualification recurses into the type arguments.
    #[test]
    fn qualified_generic_instantiation_qualifies_head_and_recurses_into_args() {
        let r = TypeRef::App {
            name: Ident {
                name: "MyGeneric".to_string(),
                span: sp(),
            },
            args: vec![base(BaseType::Int), named("Order")],
            span: sp(),
        };
        let mut scope = HashSet::new();
        scope.insert("MyGeneric".to_string());
        scope.insert("Order".to_string());
        assert_eq!(
            bynk_ts::print_type(&ts_type_ref_qualified_ts_type(&r, &scope, "Ns")),
            "Ns.MyGeneric<number, Ns.Order>"
        );
    }

    /// Same shape as above, through `ts_type_ref_qualified_multi_ts_type` —
    /// no direct test existed for this function at all before this fix.
    #[test]
    fn qualified_multi_generic_instantiation_qualifies_head_and_recurses_into_args() {
        let r = TypeRef::App {
            name: Ident {
                name: "MyGeneric".to_string(),
                span: sp(),
            },
            args: vec![base(BaseType::Int), named("Order")],
            span: sp(),
        };
        let mut type_ns = HashMap::new();
        type_ns.insert("MyGeneric".to_string(), "A".to_string());
        type_ns.insert("Order".to_string(), "B".to_string());
        assert_eq!(
            bynk_ts::print_type(&ts_type_ref_qualified_multi_ts_type(&r, &type_ns)),
            "A.MyGeneric<number, B.Order>"
        );
    }
}

/// v0.20b: render a checker `Ty` as a TypeScript type. Used by the inline
/// kernel-method lowerings, whose IIFE parameters must be annotated
/// (`noImplicitAny`). Rigid type variables render as themselves — inside an
/// emitted generic function they are in scope as TS type parameters.
/// P7.9 (#1315, R7.2): renders `t` by building a real [`bynk_ts::TsType`]
/// and printing it through [`bynk_ts::print_type`], instead of `format!`-ing
/// the type text by hand — this function's own signature and every one of
/// its ~45 real callers are unchanged; only the internal construction moved.
/// `Ty::ActorSum` needed a new gap closed beyond the accepted proposal's own
/// `readonly`/`Fn` list — a real, grounded one (found during implementation
/// review, not speculative): a resolved multi-actor sum lowers to a genuine
/// type-position union no existing `TsType` variant could represent, closed
/// by adding `TsType::Union` (see its own doc).
pub(super) fn ts_ty(t: TyId, tys: TyRender<'_>) -> String {
    bynk_ts::print_type(&ts_ty_to_ts_type(t, tys))
}

/// `elem_ts` (a [`ts_ty`] rendering) made safe to suffix with `[]`, for the
/// kernel lowerings that `format!` an array type around an element's text
/// rather than building a [`TsType::Array`]. `[]` binds tighter than a prefix
/// `readonly`, a function arrow, `|` and `&`, so `readonly readonly T[][]`
/// (TS1354) or `(a0: T) => T[]` would be misread; such an element is
/// parenthesised. The check is on text, so it is conservative — an operator
/// nested inside a generic argument parenthesises too, which is redundant but
/// still valid TypeScript. The printer's own `TsType::Array` arm applies the
/// exact structural rule.
pub(super) fn ts_array_elem(elem_ts: &str) -> String {
    if elem_ts.starts_with("readonly ")
        || elem_ts.contains("=>")
        || elem_ts.contains('|')
        || elem_ts.contains('&')
    {
        format!("({elem_ts})")
    } else {
        elem_ts.to_string()
    }
}

/// #1927: how the module being emitted spells a type it names. A unit's own
/// value (a function, asset, service, agent or provider) can hide a
/// `uses`-imported type of its name. The unit's source may not write that
/// type, but the lowering still names the types of checked expressions (#1778:
/// a kernel's `(__x: Name) => …`, an `if` slot, `List.empty`), and in this
/// module `Name` is the local value: a `const`, or for an agent or provider a
/// `class`, whose instance type would capture the annotation. So while such a
/// module is emitted, each hidden type is spelled as an inline import type,
/// `import("<module>").Name`, which needs no import statement and names the
/// declaring module's type whatever this module declares. Agents are never
/// `Ty::Named`, so within the unit a hidden name always means the imported
/// type.
///
/// Scoped per module and carried by [`TyRender`]: the spellings are a field of
/// the unit's [`EmitProjectCtx`] (`type_spellings`), built by the driver from
/// `shadowed_imports`, and every renderer reaches them through the `TyRender`
/// it is handed instead of a thread-local.
#[derive(Clone, Copy)]
pub(crate) struct TyRender<'a> {
    tys: &'a Arc<Types>,
    spellings: &'a HashMap<String, String>,
}

impl<'a> TyRender<'a> {
    /// The checker's interner plus the unit's hidden-type spellings.
    pub(crate) fn new(tys: &'a Arc<Types>, spellings: &'a HashMap<String, String>) -> Self {
        Self { tys, spellings }
    }

    /// The interner, for a caller that wants `Ty` structure and not text.
    pub(crate) fn types(&self) -> &'a Arc<Types> {
        self.tys
    }

    /// How the unit spells the type `name`: its rename, if any.
    pub(crate) fn spelling(&self, name: &str) -> String {
        self.spellings
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }
}

/// The spellings of a unit that hides no imported type (and of a lowering with
/// no unit behind it): every name is spelled as itself.
pub(crate) fn no_spellings() -> &'static HashMap<String, String> {
    static NONE: std::sync::LazyLock<HashMap<String, String>> =
        std::sync::LazyLock::new(HashMap::new);
    &NONE
}

impl std::ops::Deref for TyRender<'_> {
    type Target = Arc<Types>;
    fn deref(&self) -> &Arc<Types> {
        self.tys
    }
}

/// What a store `Query[T]`'s thunk produces: its elements, or a promise of them (#3189). A
/// collection backed by storage reads asynchronously; one held in memory does not, and the
/// query algebra chains through `__bynkThen` so it serves either.
pub(super) fn query_result_ty(elem: TsType) -> TsType {
    let items = TsType::readonly_array(elem);
    TsType::union(vec![
        items.clone(),
        TsType::named_with_args("globalThis.Promise", vec![items]),
    ])
}

pub(super) fn ts_ty_to_ts_type(t: TyId, tys: TyRender<'_>) -> TsType {
    match &*tys.get(t) {
        // bynk internal error (finding #28, R4.3): `Ty::Error` records a
        // resolution failure, which per R4.3 is always accompanied by a
        // pushed diagnostic — the check that produced it should have failed
        // the whole program and never reached emission. A loud failure here
        // beats silently emitting a type for a node the checker gave up on.
        Ty::Error => panic!(
            "bynk internal error (finding #28): emitter asked to render `Ty::Error` as a \
             TypeScript type — a checked program should never contain one"
        ),
        // #2620: the bottom type renders as TypeScript's own `never`.
        Ty::Never => TsType::named("never"),
        Ty::Base(b) => TsType::named(ts_base(*b)),
        // #1977: the compiler-known `MapEntry[K, V]` (ADR 0184) has no
        // `TypeDecl`, so no module declares or imports a `MapEntry` type to
        // name. It renders as its own shape, the `{ key, value }` record the
        // `.entries` lowering builds. (A user type of the same name, which the
        // checker prefers for field access, interns to the same `Ty` and so
        // renders inline too; no program declares one today.)
        Ty::Named {
            name,
            kind: NamedKind::Record,
            args,
        } if name == MAP_ENTRY && args.len() == 2 => TsType::Object(vec![
            bynk_ts::TsTypeMember::readonly_prop("key", ts_ty_to_ts_type(args[0], tys)),
            bynk_ts::TsTypeMember::readonly_prop("value", ts_ty_to_ts_type(args[1], tys)),
        ]),
        // v0.157 (ADR 0183): a generic record instantiation renders as the
        // erased TS generic `Name<Arg, …>`; a non-generic named type is bare.
        Ty::Named { name, args, .. } if args.is_empty() => TsType::named(tys.spelling(name)),
        Ty::Named { name, args, .. } => TsType::named_with_args(
            tys.spelling(name),
            args.iter().map(|a| ts_ty_to_ts_type(*a, tys)).collect(),
        ),
        Ty::Result(t, e) => TsType::named_with_args(
            "Result",
            vec![ts_ty_to_ts_type(*t, tys), ts_ty_to_ts_type(*e, tys)],
        ),
        Ty::Option(t) => TsType::named_with_args("Option", vec![ts_ty_to_ts_type(*t, tys)]),
        Ty::Effect(t) => match &*tys.get(*t) {
            Ty::Unit => TsType::named_with_args("globalThis.Promise", vec![TsType::named("void")]),
            _ => TsType::named_with_args("globalThis.Promise", vec![ts_ty_to_ts_type(*t, tys)]),
        },
        Ty::HttpResult(t) => TsType::named_with_args("HttpResult", vec![ts_ty_to_ts_type(*t, tys)]),
        Ty::List(t) => TsType::readonly_array(ts_ty_to_ts_type(*t, tys)),
        Ty::Set(t) => TsType::named_with_args("ReadonlySet", vec![ts_ty_to_ts_type(*t, tys)]),
        // v0.91 (ADR 0119): a `Query[T]` lowers to a deferred producer of its
        // elements — a thunk run by the terminal. Same outer-paren shape as
        // `TypeRef::Query` (`ts_type_ref_to_ts_type`'s own comment explains
        // the opaque-`Named` wrap this reuses, unchanged reasoning).
        Ty::Query(t) => {
            let fn_ty = TsType::Fn {
                names: vec![],
                params: vec![],
                ret: Box::new(query_result_ty(ts_ty_to_ts_type(*t, tys))),
            };
            TsType::named(format!("({})", bynk_ts::print_type(&fn_ty)))
        }
        // v0.100: a `Stream[T]` lowers to a host async iterable.
        Ty::Stream(t) => TsType::named_with_args("AsyncIterable", vec![ts_ty_to_ts_type(*t, tys)]),
        // v0.102: a `Connection[F]` lowers to the runtime `Connection<F>` interface.
        Ty::Connection(t) => TsType::named_with_args("Connection", vec![ts_ty_to_ts_type(*t, tys)]),
        Ty::TestConnection(t) => {
            TsType::named_with_args("TestConnection", vec![ts_ty_to_ts_type(*t, tys)])
        }
        Ty::Map(k, v) => TsType::named_with_args(
            "ReadonlyMap",
            vec![ts_ty_to_ts_type(*k, tys), ts_ty_to_ts_type(*v, tys)],
        ),
        Ty::QueueResult => TsType::named("QueueResult"),
        // #1934: produced only by a `system`-tier test-service address, so
        // only a test module meets them: the runtime's `HttpOutcome<T>`, and
        // the shape of its `Rejected` payload.
        Ty::HttpOutcome(t) => {
            TsType::named_with_args("HttpOutcome", vec![ts_ty_to_ts_type(*t, tys)])
        }
        Ty::BoundaryRejection => TsType::named("{ readonly tag: string; readonly detail: string }"),
        Ty::Asset => asset_ts_type(),
        Ty::RequestHeaders => request_headers_ts_type(),
        Ty::RequestQuery => request_query_ts_type(),
        Ty::RequestCookies => request_cookies_ts_type(),
        Ty::TokenClaims => token_claims_ts_type(),
        Ty::ValidationError => TsType::named("ValidationError"),
        Ty::DecodeError => TsType::named("DecodeError"),
        Ty::Unit => TsType::named("void"),
        Ty::Fn { params, ret } => TsType::Fn {
            names: vec![],
            params: params.iter().map(|p| ts_ty_to_ts_type(*p, tys)).collect(),
            ret: Box::new(ts_ty_to_ts_type(*ret, tys)),
        },
        Ty::Var(n) => TsType::named(n.clone()),
        // The identity type the actor binding yields (`name.identity`).
        Ty::Actor(id) => ts_ty_to_ts_type(*id, tys),
        // v0.52: a resolved multi-actor sum lowers to a discriminated union
        // tagged by actor name; non-unit members carry their identity.
        // Each member's own real text is `, `-separated (`{ tag: "x",
        // identity: T }`) — `TsType::Object`'s own renderer is `; `-
        // separated (the ordinary TS object-*type* convention, correct for
        // every other real caller, e.g. `events_fanout.rs`'s own interface
        // members), so building each member as an `Object` would silently
        // change this one shape's separator and fail the zero-diff check
        // (caught exactly that way — a fixture-corpus-adjacent direct test
        // failure, not reasoning about the algebra). Each member is instead
        // an opaque `Named`-carries-verbatim-text node, the same convention
        // `ts_type_ref_to_ts_type`'s own `Query` arm already established for
        // a single real shape `TsType`'s general renderers don't reproduce
        // byte-for-byte — not a new pattern, the second real use of one.
        Ty::ActorSum(members) => TsType::union(
            members
                .iter()
                .map(|(name, id)| match &*tys.get(*id) {
                    Ty::Unit => TsType::named(format!("{{ tag: \"{name}\" }}")),
                    _ => TsType::named(format!(
                        "{{ tag: \"{name}\", identity: {} }}",
                        bynk_ts::print_type(&ts_ty_to_ts_type(*id, tys))
                    )),
                })
                .collect(),
        ),
    }
}

/// P7.9 (#1315): pins `ts_ty`'s exact pre-slice text for a representative
/// `Ty` from every real shape category — the same "prove the test would
/// actually catch it" discipline `ts_type_ref_tests` above already applies
/// to `ts_type_ref`, and doubly important here since `ts_ty` is where
/// `TsType::Union`/`ActorSum` (added in review of this same PR, beyond the
/// accepted proposal's own gap list) is the one real, exercised caller.
#[cfg(test)]
mod ts_ty_tests {
    use super::*;
    use bynk_check::checker::{NamedKind, Types};

    #[test]
    fn base_types() {
        let tys = Arc::new(Types::new());
        assert_eq!(
            ts_ty(
                tys.intern(Ty::Base(BaseType::Int)),
                TyRender::new(&tys, no_spellings())
            ),
            "number"
        );
    }

    #[test]
    fn named_type_bare_and_generic() {
        let tys = Arc::new(Types::new());
        let bare = tys.intern(Ty::Named {
            name: "Order".to_string(),
            kind: NamedKind::Record,
            args: vec![],
        });
        assert_eq!(ts_ty(bare, TyRender::new(&tys, no_spellings())), "Order");

        let arg = tys.intern(Ty::Base(BaseType::String));
        let generic = tys.intern(Ty::Named {
            name: "Paginated".to_string(),
            kind: NamedKind::Record,
            args: vec![arg],
        });
        assert_eq!(
            ts_ty(generic, TyRender::new(&tys, no_spellings())),
            "Paginated<string>"
        );
    }

    #[test]
    fn map_entry_renders_its_own_shape() {
        // #1977: no module declares `MapEntry`, so it is never named.
        let tys = Arc::new(Types::new());
        let k = tys.intern(Ty::Base(BaseType::String));
        let v = tys.intern(Ty::Base(BaseType::Int));
        let entry = bynk_check::checker::map_entry_ty(k, v, &tys);
        assert_eq!(
            ts_ty(entry, TyRender::new(&tys, no_spellings())),
            "{ readonly key: string; readonly value: number }"
        );
        let list = tys.intern(Ty::List(entry));
        assert_eq!(
            ts_ty(list, TyRender::new(&tys, no_spellings())),
            "readonly { readonly key: string; readonly value: number }[]"
        );
    }

    #[test]
    fn result_option_effect_http_result() {
        let tys = Arc::new(Types::new());
        let int = tys.intern(Ty::Base(BaseType::Int));
        let err = tys.intern(Ty::Named {
            name: "MyError".to_string(),
            kind: NamedKind::Record,
            args: vec![],
        });
        let result = tys.intern(Ty::Result(int, err));
        assert_eq!(
            ts_ty(result, TyRender::new(&tys, no_spellings())),
            "Result<number, MyError>"
        );

        let option = tys.intern(Ty::Option(int));
        assert_eq!(
            ts_ty(option, TyRender::new(&tys, no_spellings())),
            "Option<number>"
        );

        let effect = tys.intern(Ty::Effect(int));
        assert_eq!(
            ts_ty(effect, TyRender::new(&tys, no_spellings())),
            "globalThis.Promise<number>"
        );

        let unit = tys.intern(Ty::Unit);
        let effect_unit = tys.intern(Ty::Effect(unit));
        assert_eq!(
            ts_ty(effect_unit, TyRender::new(&tys, no_spellings())),
            "globalThis.Promise<void>"
        );

        let http_result = tys.intern(Ty::HttpResult(int));
        assert_eq!(
            ts_ty(http_result, TyRender::new(&tys, no_spellings())),
            "HttpResult<number>"
        );
    }

    #[test]
    fn list_map_stream_connection() {
        let tys = Arc::new(Types::new());
        let order = tys.intern(Ty::Named {
            name: "Order".to_string(),
            kind: NamedKind::Record,
            args: vec![],
        });
        assert_eq!(
            ts_ty(
                tys.intern(Ty::List(order)),
                TyRender::new(&tys, no_spellings())
            ),
            "readonly Order[]"
        );
        // A nested list's inner readonly array is parenthesised (TS1354).
        let orders = tys.intern(Ty::List(order));
        assert_eq!(
            ts_ty(
                tys.intern(Ty::List(orders)),
                TyRender::new(&tys, no_spellings())
            ),
            "readonly (readonly Order[])[]"
        );
        assert_eq!(
            ts_array_elem(&ts_ty(orders, TyRender::new(&tys, no_spellings()))),
            "(readonly Order[])"
        );
        assert_eq!(
            ts_array_elem(&ts_ty(order, TyRender::new(&tys, no_spellings()))),
            "Order"
        );

        let s = tys.intern(Ty::Base(BaseType::String));
        assert_eq!(
            ts_ty(
                tys.intern(Ty::Map(s, order)),
                TyRender::new(&tys, no_spellings())
            ),
            "ReadonlyMap<string, Order>"
        );
        assert_eq!(
            ts_ty(
                tys.intern(Ty::Stream(order)),
                TyRender::new(&tys, no_spellings())
            ),
            "AsyncIterable<Order>"
        );
        assert_eq!(
            ts_ty(
                tys.intern(Ty::Connection(order)),
                TyRender::new(&tys, no_spellings())
            ),
            "Connection<Order>"
        );
    }

    /// The same shared shape as `TypeRef::Query` — pins that `ts_ty`'s own
    /// `Query` arm reproduces the identical outer-paren wrap after
    /// conversion, not just `ts_type_ref`'s.
    #[test]
    fn query_wraps_the_whole_function_type_in_parens() {
        let tys = Arc::new(Types::new());
        let order = tys.intern(Ty::Named {
            name: "Order".to_string(),
            kind: NamedKind::Record,
            args: vec![],
        });
        assert_eq!(
            ts_ty(
                tys.intern(Ty::Query(order)),
                TyRender::new(&tys, no_spellings())
            ),
            "(() => readonly Order[] | globalThis.Promise<readonly Order[]>)"
        );
    }

    #[test]
    fn bare_types_and_fn() {
        let tys = Arc::new(Types::new());
        assert_eq!(
            ts_ty(
                tys.intern(Ty::QueueResult),
                TyRender::new(&tys, no_spellings())
            ),
            "QueueResult"
        );
        assert_eq!(
            ts_ty(
                tys.intern(Ty::ValidationError),
                TyRender::new(&tys, no_spellings())
            ),
            "ValidationError"
        );
        assert_eq!(
            ts_ty(
                tys.intern(Ty::DecodeError),
                TyRender::new(&tys, no_spellings())
            ),
            "DecodeError"
        );
        assert_eq!(
            ts_ty(tys.intern(Ty::Unit), TyRender::new(&tys, no_spellings())),
            "void"
        );

        let int = tys.intern(Ty::Base(BaseType::Int));
        let s = tys.intern(Ty::Base(BaseType::String));
        let f = tys.intern(Ty::Fn {
            params: vec![int, s],
            ret: int,
        });
        assert_eq!(
            ts_ty(f, TyRender::new(&tys, no_spellings())),
            "(a0: number, a1: string) => number"
        );
    }

    /// The real gap found during implementation review: a resolved
    /// multi-actor sum lowers to a genuine type-position union, tagged by
    /// actor name, non-unit members carrying their identity — the shape
    /// that motivated adding `TsType::Union` beyond the accepted proposal's
    /// own `readonly`/`Fn` gap list.
    #[test]
    fn actor_sum_is_a_tagged_union() {
        let tys = Arc::new(Types::new());
        let unit = tys.intern(Ty::Unit);
        let identity = tys.intern(Ty::Named {
            name: "AdminIdentity".to_string(),
            kind: NamedKind::Record,
            args: vec![],
        });
        let sum = tys.intern(Ty::ActorSum(vec![
            ("Guest".to_string(), unit),
            ("Admin".to_string(), identity),
        ]));
        assert_eq!(
            ts_ty(sum, TyRender::new(&tys, no_spellings())),
            "{ tag: \"Guest\" } | { tag: \"Admin\", identity: AdminIdentity }"
        );
    }

    /// Coverage gap named in review of #1315/#1316: `Ty::Var` (a type
    /// variable's own bare name) and `Ty::Actor` (delegates to its
    /// identity type) were unpinned.
    #[test]
    fn var_and_actor() {
        let tys = Arc::new(Types::new());
        assert_eq!(
            ts_ty(
                tys.intern(Ty::Var("T".to_string())),
                TyRender::new(&tys, no_spellings())
            ),
            "T"
        );

        let identity = tys.intern(Ty::Named {
            name: "AdminIdentity".to_string(),
            kind: NamedKind::Record,
            args: vec![],
        });
        let actor = tys.intern(Ty::Actor(identity));
        assert_eq!(
            ts_ty(actor, TyRender::new(&tys, no_spellings())),
            "AdminIdentity"
        );
    }
}
