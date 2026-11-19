//! The `Json`, `Yaml` and `Form` codec statics of a method call and the typed decoders
//! (`Json.decode`/`decodeSql`/`encode`/`encodePlain`, `Yaml.decode`, `Form.decode`/`encode`),
//! with the decoder the typed `D1` and `Postgres` reads share, and the `bynk eval` tail report.
//! Moved out of `lower.rs` (back-half paydown 1A.21, #3722, Decision E) before they convert to
//! nodes.

use bynk_check::builtin_names::types::*;
use bynk_check::checker::{Callee, Ty};
use bynk_syntax::ast::BaseType;

use super::super::lower::{Lowered, Pre};
use super::super::lower_ctx::LowerCtx;
use super::super::ts_types::ts_ty;
use super::super::{Expr, Ident, TypeRef, ty_to_type_ref};

/// v0.22b: the typed JSON codec (ADR 0045). `encode` dispatches to the
/// module-local `serialise_*` helpers + `JSON.stringify`; `decode[T]` to
/// `JSON.parse` + `deserialise_*`, mapping a parse failure to a `Malformed`
/// DecodeError and a BoundaryError to the uniform `kind`/`path`/`message`
/// record (ADR 0047). Returns `None` when the receiver is not `Json` or the
/// shape does not match, so the dispatcher falls through.
///
/// R8.14 (Arc D, P7.15 review): reads the same `Callee::Intrinsic { ns: JSON,
/// .. }` this call's own root gets classified under in
/// `collect_json_codec_roots` (`bynk-emit/src/emitter.rs`) — not the bare
/// `id.name == JSON` receiver-name match this function used before. The two
/// must agree on what a codec call *is*: `collect_json_codec_roots` decides
/// whether a `serialise_T`/`deserialise_T` helper gets emitted at all, this
/// function decides whether a call site references one. Read using the
/// syntactic match alone, a `Json` shadowed by a local value/type would have
/// disagreed with the (already `Callee`-based) seed collector — no helper
/// emitted, but a reference to one generated anyway, a `tsc` "cannot find
/// name" strictly worse than the syntactic match's own pre-existing
/// wrong-but-well-formed output.
pub(in super::super) fn lower_json_codec_call(
    e: &Expr,
    // Unused now that the guard below reads the checker's own resolved
    // `Callee::Intrinsic` instead of matching the receiver's own shape —
    // kept in the signature for uniformity with this dispatcher's sibling
    // `lower_*_call` functions, which all share this exact parameter list.
    _receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    // #3214: `Yaml.decode[T](text)` is `Json.decode[T]` with the YAML parser in
    // place of `JSON.parse`; the typed decode and its errors are the same.
    if matches!(
        cx.commons().callees.get(&e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == YAML
    ) && args.len() == 1
        && method.name == "decode"
        && let Some(Ty::Result(t, _)) = cx.commons().expr_ty(e.id).as_deref().cloned()
        && let Some(decoder) = json_decoder_fn(t, DecodeSource::Yaml, cx)
    {
        let arg = pre.lower(&args[0], cx);
        return Some(pre.finish_text(format!("({decoder})({arg})")));
    }
    // #3276 (decision G): `Form.decode[T](text)` reads the text as the router reads
    // a `@form` body, then decodes through `T`'s codec; `Form.encode(v)` writes the
    // form text such a route reads.
    if matches!(
        cx.commons().callees.get(&e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == FORM
    ) && args.len() == 1
    {
        if method.name == "decode"
            && let Some(Ty::Result(t, _)) = cx.commons().expr_ty(e.id).as_deref().cloned()
            && let Some(decoder) = json_decoder_fn(t, DecodeSource::Form, cx)
        {
            let arg = pre.lower(&args[0], cx);
            return Some(pre.finish_text(format!("({decoder})({arg})")));
        }
        if method.name == "encode" {
            cx.runtime_use().note_form_codec();
            // The checker admits only a concrete record of form fields, so the wire
            // value is always available; a miss is a compiler fault, not a user error.
            let wire = form_encode_wire(&mut pre, &args[0], cx)
                .expect("`Form.encode` checked a concrete record of form fields");
            // #3343: a field renamed by `@name` is written under its wire key.
            let table = cx
                .commons()
                .expr_types
                .get(&args[0].id)
                .map(|te| te.ty)
                .and_then(|ty| ty_to_type_ref(ty, cx.commons().tys()))
                .and_then(|tref| {
                    super::super::workers_entry::renamed_specs_source(&tref, &cx.commons().types)
                })
                .map(|t| format!(", {t}"))
                .unwrap_or_default();
            return Some(pre.finish_text(format!("__formString({wire}{table})")));
        }
    }
    if matches!(
        cx.commons().callees.get(&e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == JSON
    ) && args.len() == 1
    {
        if matches!(method.name.as_str(), "encode" | "encodePlain")
            && let Some(text) =
                json_encode_text(&mut pre, &args[0], cx, method.name == "encodePlain")
        {
            return Some(pre.finish_text(text));
        }
        if matches!(method.name.as_str(), "decode" | "decodeSql")
            && let Some(Ty::Result(t, _)) = cx.commons().expr_ty(e.id).as_deref().cloned()
            && let Some(decoder) = json_decoder_fn(
                t,
                if method.name == "decodeSql" {
                    DecodeSource::Sql
                } else {
                    DecodeSource::Json
                },
                cx,
            )
        {
            let arg = pre.lower(&args[0], cx);
            return Some(pre.finish_text(format!("({decoder})({arg})")));
        }
    }
    let text: Option<String> = None;
    text.map(|expr| pre.finish_text(expr))
}

/// Which text front end feeds a typed decode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in super::super) enum DecodeSource {
    /// `Json.decode`: the text is JSON.
    Json,
    /// `Json.decodeSql` and the typed `D1` reads: JSON, with SQLite's `0`/`1` flags read as `Bool`.
    Sql,
    /// Hyperdrive H3 (#3320), the typed `Postgres` reads: JSON, with an `int8`'s
    /// text read at an `Int` field and a `timestamptz`'s text at an `Instant`.
    Pg,
    /// #3214, `Yaml.decode`: the text is YAML, parsed to the same JSON-shaped tree.
    Yaml,
    /// #3276, `Form.decode`: the text is a form body, read by the router's own rules.
    Form,
}

/// The arrow function that decodes text into `t`:
/// `((__s: string): Result<T, DecodeError> => …)`, for `Json.decode` /
/// `Json.decodeSql` / `Yaml.decode` (`source`) and for the typed `D1` reads
/// (#2865), which apply it to the rows `all`/`first` return. `None` when `t`
/// has no TS rendering.
pub(in super::super) fn json_decoder_fn(
    t: bynk_check::checker::TyId,
    source: DecodeSource,
    cx: &mut LowerCtx,
) -> Option<String> {
    let sql = source == DecodeSource::Sql;
    let tys = cx.ty_render();
    let rigid = super::super::codec::has_var(t, cx.commons());
    let tref = if rigid {
        // A target that mentions a type parameter decodes through the
        // codec witness; `tref` is then only a placeholder.
        Some(TypeRef::Unit(bynk_syntax::span::Span::default()))
    } else {
        ty_to_type_ref(t, tys.types())
    }?;
    // #917: in a test-scaffold module `T` may be foreign (owned by the
    // suite target or one of its `uses`, never declared locally), so its
    // TS type positions reach through the module's namespace qualifier —
    // empty everywhere else, where this renders identically to `ts_ty`.
    let ts = if cx.in_test_scaffold() && !rigid {
        let qual = cx.runtime_use().json_codec_qual();
        bynk_ts::print_type(&super::super::serialisation::qualified_ts_type(
            &tref, &qual,
        ))
    } else {
        ts_ty(t, tys)
    };
    if cx.records_codec_roots() {
        note_ground_roots(t, cx);
    }
    // #914: the wrapper below names `Result`, `JsonValue` and `DecodeError`
    // in its own signature and body, whichever arm the inner deserialiser
    // takes — including the delegating ones, which record nothing. A
    // module that curates its import list (the test-scaffold modules)
    // otherwise emits all three unimported.
    cx.runtime_use().note_json_codec();
    let des = if rigid {
        // 1A: witness printed for a text caller
        format!("{}.decode(__j, \"$\")", bynk_ts::print_expr(&cx.codec(t)))
    } else {
        bynk_ts::print_expr(&super::super::serialisation::deserialise_expr(
            &tref,
            "__j",
            "$",
            cx.runtime_use(),
        ))
    };
    let sql_bools = if sql {
        sql_bool_pass(t, cx)
    } else if source == DecodeSource::Pg {
        pg_row_pass(t, cx)
    } else {
        String::new()
    };
    // The parse step: `JSON.parse`, or (#3214) the YAML parser, whose failures
    // are already positioned `DecodeError`s.
    let parse = if source == DecodeSource::Form {
        cx.runtime_use().note_form_codec();
        let specs = form_specs_literal(t, cx)?;
        format!(
            "const __p = __formDecode(__s, {specs}); \
             if (__p.tag === \"Err\") return Err(__p.error); \
             const __j: __JsonValue = __p.value; "
        )
    } else if source == DecodeSource::Yaml {
        cx.runtime_use().note_yaml();
        "const __p = __bynkYamlParse(__s); \
         if (__p.tag === \"Err\") return Err(__p.error); \
         const __j: __JsonValue = __p.value; "
            .to_string()
    } else {
        "let __j: __JsonValue; \
         try { __j = globalThis.JSON.parse(__s) as __JsonValue; } \
         catch (__e) { return Err({ kind: \"Malformed\", path: \"$\", message: String(__e) }); } "
            .to_string()
    };
    Some(format!(
        "(__s: string): Result<{ts}, DecodeError> => {{ \
         {parse}\
         {sql_bools}const __r = {des}; \
         if (__r.tag === \"Ok\") return Ok(__r.value as {ts}); \
         const __be = __r.error; \
         return Err({{ kind: __be.kind, \
         path: (__be.kind === \"StructuralMismatch\" || __be.kind === \"RefinementViolation\") ? __be.path : \"$\", \
         message: __be.kind === \"StructuralMismatch\" ? `expected ${{__be.expected}}, got ${{String(__be.actual)}}` : __be.kind === \"RefinementViolation\" ? __be.violation.message : __be.details }}); }}"
    ))
}

/// #3276: the field table `Form.decode[T]` hands the form reader, as the TS array
/// literal of `{ name, kind, list, option }` the router's `@form` path builds. `None`
/// when `t` is not a record of the module's types.
fn form_specs_literal(t: bynk_check::checker::TyId, cx: &LowerCtx) -> Option<String> {
    use bynk_syntax::ast::TypeBody;
    let tys = cx.ty_render();
    let Ty::Named { name, .. } = &*tys.get(t) else {
        return None;
    };
    let decl = cx.commons().types.get(name.as_str())?;
    let TypeBody::Record(rec) = &decl.body else {
        return None;
    };
    let specs = super::super::workers_entry::record_field_specs(rec, &cx.commons().types, false);
    let items: Vec<String> = specs
        .iter()
        .map(super::super::workers_entry::field_spec_source)
        .collect();
    Some(format!("[{}]", items.join(", ")))
}

/// #3276: the wire value `Form.encode(arg)` writes flat: `arg` serialised exactly as
/// `Json.encode` serialises it, before the JSON text step. `None` only for a type
/// the lowering cannot render, which the checker has already refused for `Form`.
fn form_encode_wire(pre: &mut Pre, arg: &Expr, cx: &mut LowerCtx) -> Option<String> {
    let tys = cx.ty_render();
    let arg_ty = cx.commons().expr_types.get(&arg.id).map(|te| te.ty)?;
    let tref = ty_to_type_ref(arg_ty, tys.types())?;
    if cx.records_codec_roots() {
        cx.runtime_use().note_json_codec_root(arg_ty);
    }
    let v = pre.lower(arg, cx);
    Some(bynk_ts::print_expr(
        &super::super::serialisation::serialise_expr(
            &tref,
            &bynk_ts::TsExpr::Ident(v),
            cx.runtime_use(),
        ),
    ))
}

/// #3585: the statement that reports a `bynk eval` case's tail value: its
/// `Json.encode` text and its type, as one NDJSON line. `None` when the value's
/// type is unknown to the lowering, so the case runs without reporting.
pub(in super::super) fn eval_value_line(
    pre: &mut Pre,
    tail: &Expr,
    cx: &mut LowerCtx,
) -> Option<String> {
    let ty = cx.commons().expr_types.get(&tail.id).map(|te| te.ty)?;
    let shown = bynk_check::symbols::display_type_names_in(&cx.commons().tys().display(ty));
    let shown = bynk_project::json_string(&shown);
    let head = format!(
        "globalThis.console.log('{{\"type\":\"eval-value\",\"valueType\":' + globalThis.JSON.stringify({shown})"
    );
    // A value with no wire shape (a function, say) still reports its type, and
    // no value.
    let Some(json) = json_encode_text(pre, tail, cx, false) else {
        let _ = pre.lower(tail, cx);
        return Some(format!("{head} + '}}');"));
    };
    // The encoder names `JsonValue` and `BoundaryError`, whether or not the
    // value's type needs a closure of codecs of its own.
    cx.runtime_use().note_json_codec();
    cx.runtime_use().note_boundary_codec();
    Some(format!("{head} + ',\"value\":' + {json} + '}}');"))
}

/// `Json.encode(arg)`'s text: `globalThis.JSON.stringify(<serialised arg>)`, with
/// the codec roots the module needs recorded. Shared by the `Json.encode` call
/// and the snapshot `expect` (#2842), so both encode a value one way. `None`
/// when the argument's type is unknown to the lowering.
pub(in super::super) fn json_encode_text(
    pre: &mut Pre,
    arg: &Expr,
    cx: &mut LowerCtx,
    plain: bool,
) -> Option<String> {
    let tys = cx.ty_render();
    // Constrained generics slice 3: a value whose type mentions a type
    // parameter encodes through the codec witness of that type.
    let arg_ty = cx.commons().expr_types.get(&arg.id).map(|te| te.ty)?;
    if super::super::codec::has_var(arg_ty, cx.commons()) {
        if cx.records_codec_roots() {
            note_ground_roots(arg_ty, cx);
        }
        // 1A: witness printed for a text caller
        let codec = bynk_ts::print_expr(&cx.codec(arg_ty));
        let v = pre.lower(arg, cx);
        // `plain` never reaches a generic argument: the checker refused it.
        return Some(format!("globalThis.JSON.stringify({codec}.encode({v}))"));
    }
    let tref = ty_to_type_ref(arg_ty, tys.types())?;
    // #917: a test-scaffold module's target/`uses` types export no codec
    // of their own — record the root so the module that spliced this
    // body in can generate its own `serialise_*`/`deserialise_*` closure,
    // the same way a workers cross-context caller generates one for a
    // consumed context's boundary types (#661). P6.28: pushes the `TyId`
    // already in hand, not a re-derived `TypeRef` — `tref` below still
    // exists for this call site's own, separate codec-rendering use.
    if cx.records_codec_roots() {
        cx.runtime_use().note_json_codec_root(arg_ty);
    }
    let v = pre.lower(arg, cx);
    // #1435 (Arc E slice 1): boundary-print, same treatment as this
    // file's own `serialise_expr_via` call site above.
    if plain {
        // #2991: `Json.encodePlain`. The one `Option` encoder reads
        // `__bynkPlainJson`, so raise it for this one synchronous serialisation
        // and restore it in a `finally` (the encoder throws on a non-finite
        // `Float`). The checker has refused a generic argument, so only
        // monomorphic encoders (which read the flag) run here. Inlined rather
        // than a runtime helper, so no module needs a new import.
        // The argument must be evaluated *before* the flag is raised, or a tagged
        // `Json.encode` (or a function that calls one) inside it would come out
        // plain. An argument that awaits is hoisted to a temporary; any other
        // is the default of the arrow's one parameter, which JS evaluates at
        // call time, before the body raises the flag (its type is inferred).
        let (param, tmp) = if v.contains("await") {
            let tmp = format!("__pj{}", arg.id.0);
            pre.push_text(format!("const {tmp} = {v};"));
            (format!("__pv = {tmp}"), "__pv".to_string())
        } else {
            (format!("__pv = {v}"), "__pv".to_string())
        };
        let ser = bynk_ts::print_expr(&super::super::serialisation::serialise_expr(
            &tref,
            &bynk_ts::TsExpr::Ident(tmp.clone()),
            cx.runtime_use(),
        ));
        return Some(format!(
            "(({param}): string => {{ const __g = globalThis as {{ __bynkPlainJson?: boolean }}; \
             const __was = __g.__bynkPlainJson; __g.__bynkPlainJson = true; \
             try {{ return globalThis.JSON.stringify({ser}); }} \
             finally {{ __g.__bynkPlainJson = __was; }} }})()"
        ));
    }
    let ser = bynk_ts::print_expr(&super::super::serialisation::serialise_expr(
        &tref,
        &bynk_ts::TsExpr::Ident(v),
        cx.runtime_use(),
    ));
    Some(format!("globalThis.JSON.stringify({ser})"))
}

/// #917 and constrained generics slice 3: record the variable-free parts of a
/// codec target as roots, so the test-scaffold module that spliced the body in
/// generates the codecs its witnesses name.
fn note_ground_roots(t: bynk_check::checker::TyId, cx: &LowerCtx) {
    let mut parts = Vec::new();
    super::super::codec::ground_parts(t, cx.commons(), &mut parts);
    for p in parts {
        cx.runtime_use().note_json_codec_root(p);
    }
}

/// P4c (#2628): `Json.decodeSql[T]`'s pass over the parsed rows before the
/// ordinary codec runs. SQLite has no boolean, so a flag column arrives as
/// `0`/`1`; at each `Bool` field of the row record (the checker has confined
/// `T` to a row or a `List` of one) exactly those two numbers become
/// `false`/`true`. Anything else is left for the codec to accept or refuse,
/// so every generated codec stays the one `Json.decode` uses. Empty when the
/// row has no `Bool` field.
fn sql_bool_pass(t: bynk_check::checker::TyId, cx: &LowerCtx) -> String {
    let tys = cx.ty_render();
    let row = match &*tys.get(t) {
        Ty::List(r) => *r,
        _ => t,
    };
    let Ty::Named { name, .. } = &*tys.get(row) else {
        return String::new();
    };
    // The checker resolved this row in the same table, so a miss is a
    // compiler bug that would silently skip the pass.
    debug_assert!(
        cx.commons().types.contains_key(name.as_str()),
        "Json.decodeSql row `{name}` is not in the module's type table"
    );
    let fields = bynk_check::checker::row_bool_fields(name, &cx.commons().types);
    if fields.is_empty() {
        return String::new();
    }
    let keys = fields
        .iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "const __flags = (__o: __JsonValue): void => {{ \
         if (__o === null || typeof __o !== \"object\" || globalThis.Array.isArray(__o)) return; \
         const __row = __o as {{ [k: string]: __JsonValue }}; \
         for (const __k of [{keys}]) {{ if (__row[__k] === 0 || __row[__k] === 1) __row[__k] = __row[__k] === 1; }} }}; \
         if (globalThis.Array.isArray(__j)) __j.forEach(__flags); else __flags(__j); "
    )
}

/// Hyperdrive H3 (#3320): a typed `Postgres` read's pass over the parsed rows
/// before the ordinary codec runs, the counterpart of [`sql_bool_pass`]. The
/// codec reads an `Instant` as epoch milliseconds and an `Int` as a number; a
/// Postgres row carries a `timestamptz` as ISO-8601 text and an `int8` as text
/// (ADR 0799 F), so the runtime's `__bynkPgRow` converts exactly the row's
/// `Instant` and `Int` fields, and leaves anything it cannot convert for the
/// codec to refuse. Empty when the row has neither.
fn pg_row_pass(t: bynk_check::checker::TyId, cx: &LowerCtx) -> String {
    let tys = cx.ty_render();
    let row = match &*tys.get(t) {
        Ty::List(r) => *r,
        _ => t,
    };
    let Ty::Named { name, .. } = &*tys.get(row) else {
        return String::new();
    };
    let types = &cx.commons().types;
    let instants = bynk_check::checker::row_base_fields(name, types, BaseType::Instant);
    let ints = bynk_check::checker::row_base_fields(name, types, BaseType::Int);
    if instants.is_empty() && ints.is_empty() {
        return String::new();
    }
    cx.runtime_use().note_pg_rows();
    let list = |names: &[String]| {
        names
            .iter()
            .map(|f| format!("\"{f}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (instants, ints) = (list(&instants), list(&ints));
    format!(
        "const __fix = (__o: __JsonValue): void => __bynkPgRow(__o, [{instants}], [{ints}]); \
         if (globalThis.Array.isArray(__j)) __j.forEach(__fix); else __fix(__j); "
    )
}
