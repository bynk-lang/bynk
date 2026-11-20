//! The `Json`, `Yaml` and `Form` codec statics of a method call and the typed decoders
//! (`Json.decode`/`decodeSql`/`encode`/`encodePlain`, `Yaml.decode`, `Form.decode`/`encode`),
//! with the decoder the typed `D1` and `Postgres` reads share, and the `bynk eval` tail report.
//! Moved out of `lower.rs` (back-half paydown 1A.21, #3722, Decision E) before they convert to
//! nodes.

use bynk_check::checker::{Callee, Ty};
use bynk_check::kernel_op::{FormStatic, JsonStatic, StaticOp, YamlStatic};
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsExpr, TsLit, TsParam, TsStmt, TsType, TsTypeMember,
    TsUnaryOp,
};

use super::super::lower::{
    Built, LoweredExpr, Pre, ident_node, member_node, node_or_text, str_node, text_value,
};
use super::super::lower_ctx::LowerCtx;
use super::super::ts_types::ts_ty_to_ts_type;
use super::super::{BaseType, Expr, TypeBody, TypeRef, ty_to_type_ref};

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
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<Built> {
    // The codec operation the checker resolved for this call (1A.33). Every form below takes
    // exactly one argument.
    let Some(Callee::Intrinsic { op, .. }) = cx.commons().callees.get(&e.id) else {
        return None;
    };
    let op = *op;
    if args.len() != 1 {
        return None;
    }
    match op {
        // #3214: `Yaml.decode[T](text)` is `Json.decode[T]` with the YAML parser in
        // place of `JSON.parse`; the typed decode and its errors are the same.
        StaticOp::Yaml(YamlStatic::Decode) => {
            if let Some(Ty::Result(t, _)) = cx.commons().expr_ty(e.id).as_deref().cloned()
                && let Some(decoder) = json_decoder_fn(t, DecodeSource::Yaml, cx)
            {
                let arg = pre.lower_node(&args[0], cx);
                return Some(apply_decoder(decoder, arg));
            }
        }
        // #3276 (decision G): `Form.decode[T](text)` reads the text as the router reads
        // a `@form` body, then decodes through `T`'s codec.
        StaticOp::Form(FormStatic::Decode) => {
            if let Some(Ty::Result(t, _)) = cx.commons().expr_ty(e.id).as_deref().cloned()
                && let Some(decoder) = json_decoder_fn(t, DecodeSource::Form, cx)
            {
                let arg = pre.lower_node(&args[0], cx);
                return Some(apply_decoder(decoder, arg));
            }
        }
        // `Form.encode(v)` writes the form text such a route reads.
        StaticOp::Form(FormStatic::Encode) => return Some(lower_form_encode(&args[0], cx, pre)),
        StaticOp::Json(JsonStatic::Encode | JsonStatic::EncodePlain) => {
            if let Some(encoded) = json_encode(
                pre,
                &args[0],
                cx,
                op == StaticOp::Json(JsonStatic::EncodePlain),
            ) {
                return Some(encoded);
            }
        }
        StaticOp::Json(JsonStatic::Decode | JsonStatic::DecodeSql) => {
            if let Some(Ty::Result(t, _)) = cx.commons().expr_ty(e.id).as_deref().cloned()
                && let Some(decoder) = json_decoder_fn(
                    t,
                    if op == StaticOp::Json(JsonStatic::DecodeSql) {
                        DecodeSource::Sql
                    } else {
                        DecodeSource::Json
                    },
                    cx,
                )
            {
                let arg = pre.lower_node(&args[0], cx);
                return Some(apply_decoder(decoder, arg));
            }
        }
        StaticOp::List(_)
        | StaticOp::Map(_)
        | StaticOp::Set(_)
        | StaticOp::Int(_)
        | StaticOp::Float(_)
        | StaticOp::Decimal(_)
        | StaticOp::Duration(_)
        | StaticOp::Instant(_)
        | StaticOp::Bytes(_)
        | StaticOp::TokenClaims(_)
        | StaticOp::RequestHeaders(_)
        | StaticOp::RequestQuery(_)
        | StaticOp::RequestCookies(_)
        | StaticOp::Stream(_)
        | StaticOp::HttpResult(_)
        | StaticOp::QueueResult(_)
        | StaticOp::TestConnection(_)
        | StaticOp::TestRun(_) => {}
    }
    None
}

/// `Form.encode(v)`: the form text a `@form` route reads.
fn lower_form_encode(arg: &Expr, cx: &mut LowerCtx, pre: &mut Pre) -> Built {
    cx.runtime_use().note_form_codec();
    // The checker admits only a concrete record of form fields, so the wire
    // value is always available; a miss is a compiler fault, not a user error
    // (1A.34: a missing checker fact, reported at the argument).
    let wire = form_encode_wire(pre, arg, cx).unwrap_or_else(|| {
        Ok(cx.missing_fact(arg.span, "wire value for a `Form.encode` argument"))
    });
    // #3343: a field renamed by `@name` is written under its wire key.
    let table = cx
        .commons()
        .expr_types
        .get(&arg.id)
        .map(|te| te.ty)
        .and_then(|ty| ty_to_type_ref(ty, cx.commons().tys()))
        .and_then(|tref| {
            super::super::workers_entry::renamed_specs_source(&tref, &cx.commons().types)
        })
        .map(|t| format!(", {t}"));
    // 1A: field-spec table is workers_entry source text
    match table {
        None => match wire {
            Ok(wire) => Ok(call(ident_node("__formString"), vec![wire])),
            Err(text) => Err(format!("__formString({text})")),
        },
        Some(table) => Err(format!(
            "__formString({}{table})",
            LoweredExpr::from(wire).into_text()
        )),
    }
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

/// `a.b`.
fn member(object: TsExpr, property: &str) -> TsExpr {
    TsExpr::Member {
        object: Box::new(object),
        property: property.to_string(),
    }
}

/// `callee(args)`.
fn call(callee: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::Call {
        callee: Box::new(callee),
        args,
    }
}

/// `left === right`.
fn strict_eq(left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op: TsBinaryOp::StrictEq,
        left: Box::new(left),
        right: Box::new(right),
    }
}

/// `left || right`.
fn or(left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op: TsBinaryOp::Or,
        left: Box::new(left),
        right: Box::new(right),
    }
}

/// `object[index]`.
fn index(object: TsExpr, index: TsExpr) -> TsExpr {
    TsExpr::Index {
        object: Box::new(object),
        index: Box::new(index),
    }
}

/// `globalThis.<name>`.
fn global(name: &str) -> TsExpr {
    member_node("globalThis", name)
}

/// `globalThis.Array.isArray(<value>)`.
fn is_array(value: TsExpr) -> TsExpr {
    call(member(global("Array"), "isArray"), vec![value])
}

/// The `__JsonValue` type the decoder reads parse results and rows as.
fn json_value_ty() -> TsType {
    TsType::named("__JsonValue")
}

/// A typed parameter.
fn param(name: &str, ty: TsType) -> TsParam {
    TsParam {
        name: name.to_string(),
        ty: Some(ty),
        optional: false,
        rest: false,
        array_pattern: None,
    }
}

/// `{ stmts }` as a block statement.
fn block(stmts: Vec<TsStmt>) -> TsStmt {
    TsStmt::block(stmts, None)
}

/// `const <name>[: <ty>] = <init>;`.
fn const_(name: &str, ty: Option<TsType>, init: TsExpr) -> TsStmt {
    TsStmt::const_stmt(TsBindingName::Ident(name.to_string()), ty, init, None)
}

/// `return Err(<value>);`.
fn return_err(value: TsExpr) -> TsStmt {
    TsStmt::return_stmt(Some(call(ident_node("Err"), vec![value])), None)
}

/// `if (<test>) return Err(<value>);`.
fn if_err(test: TsExpr, value: TsExpr) -> TsStmt {
    TsStmt::if_stmt(test, return_err(value), None)
}

/// Apply a decoder to the text it reads: `(decoder)(arg)`. A decoder that stayed text (a
/// `Form.decode`, whose field table is text) gives text, and so does a text argument.
fn apply_decoder(decoder: Built, arg: LoweredExpr) -> Built {
    match decoder {
        Ok(node) => {
            let printed = bynk_ts::print_expr(&node);
            node_or_text(
                vec![arg],
                |c| call(TsExpr::Paren(Box::new(node)), c),
                |c| format!("({printed})({})", c.join(", ")),
            )
        }
        Err(text) => Err(format!("({text})({})", arg.into_text())),
    }
}

/// The typed decode arrow:
/// `((__s: string): Result<T, DecodeError> => { … })`, for `Json.decode` /
/// `Json.decodeSql` / `Yaml.decode` (`source`) and for the typed `D1` reads
/// (#2865), which apply it to the rows `all`/`first` return. A node, except for a
/// `Form.decode`, whose field-spec table is `workers_entry` source text, so its
/// decoder stays text (`Err`). `None` when `t` has no TS rendering.
pub(in super::super) fn json_decoder_fn(
    t: bynk_check::checker::TyId,
    source: DecodeSource,
    cx: &mut LowerCtx,
) -> Option<Built> {
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
        super::super::serialisation::qualified_ts_type(&tref, &qual)
    } else {
        ts_ty_to_ts_type(t, tys)
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
        call(
            member(cx.codec(t), "decode"),
            vec![ident_node("__j"), str_node("$")],
        )
    } else {
        super::super::serialisation::deserialise_expr(&tref, "__j", "$", cx.runtime_use())
    };
    let row_pass = if sql {
        sql_bool_pass(t, cx)
    } else if source == DecodeSource::Pg {
        pg_row_pass(t, cx)
    } else {
        Vec::new()
    };
    // The parse step: `JSON.parse`, or (#3214) the YAML parser, whose failures
    // are already positioned `DecodeError`s.
    let parse = if source == DecodeSource::Form {
        cx.runtime_use().note_form_codec();
        let specs = form_specs_literal(t, cx)?;
        // 1A: field-spec table is workers_entry source text
        return Some(Err(form_decoder_text(&ts, &specs, &des)));
    } else if source == DecodeSource::Yaml {
        cx.runtime_use().note_yaml();
        yaml_parse_step()
    } else {
        json_parse_step()
    };
    let mut body = parse;
    body.extend(row_pass);
    body.push(const_("__r", None, des));
    body.extend(decode_tail(&ts));
    Some(Ok(TsExpr::Arrow {
        params: vec![param("__s", TsType::named("string"))],
        is_async: false,
        generics: Vec::new(),
        return_type: Some(decoder_result_ty(&ts)),
        body: Box::new(TsArrowBody::Multiline(body)),
    }))
}

/// `Result<T, DecodeError>`.
fn decoder_result_ty(ts: &TsType) -> TsType {
    TsType::named_with_args("Result", vec![ts.clone(), TsType::named("DecodeError")])
}

/// `let __j: __JsonValue; try { __j = JSON.parse(__s) as __JsonValue; } catch (__e) { return
/// Err({ kind: "Malformed", path: "$", message: String(__e) }); }`.
fn json_parse_step() -> Vec<TsStmt> {
    let malformed = TsExpr::object(vec![
        ("kind".to_string(), str_node("Malformed")),
        ("path".to_string(), str_node("$")),
        (
            "message".to_string(),
            call(ident_node("String"), vec![ident_node("__e")]),
        ),
    ]);
    let parsed = TsExpr::As {
        expr: Box::new(call(
            member(member_node("globalThis", "JSON"), "parse"),
            vec![ident_node("__s")],
        )),
        ty: json_value_ty(),
    };
    vec![
        TsStmt::let_stmt(
            TsBindingName::Ident("__j".to_string()),
            Some(json_value_ty()),
            None,
            None,
        ),
        TsStmt::try_catch(
            block(vec![TsStmt::assign(ident_node("__j"), parsed, None)]),
            Some("__e"),
            block(vec![return_err(malformed)]),
            None,
        ),
    ]
}

/// The step that reads `__s` with `parser` (a runtime helper taking the text, and for a form
/// its field table), whose failures are already `DecodeError`s:
/// `const __p = <parse>; if (__p.tag === "Err") return Err(__p.error); const __j: __JsonValue =
/// __p.value;`.
fn parsed_result_step(parse: TsExpr) -> Vec<TsStmt> {
    vec![
        const_("__p", None, parse),
        if_err(
            strict_eq(member(ident_node("__p"), "tag"), str_node("Err")),
            member(ident_node("__p"), "error"),
        ),
        const_(
            "__j",
            Some(json_value_ty()),
            member(ident_node("__p"), "value"),
        ),
    ]
}

/// #3214: `Yaml.decode`'s parse step.
fn yaml_parse_step() -> Vec<TsStmt> {
    parsed_result_step(call(ident_node("__bynkYamlParse"), vec![ident_node("__s")]))
}

/// What the decoder does with the codec's result: read `Ok`, else map the `BoundaryError` to the
/// uniform `kind`/`path`/`message` record (ADR 0047).
fn decode_tail(ts: &TsType) -> Vec<TsStmt> {
    let be = |property: &str| member(ident_node("__be"), property);
    let r = |property: &str| member(ident_node("__r"), property);
    let kind_is = |kind: &str| strict_eq(be("kind"), str_node(kind));
    let structural = kind_is("StructuralMismatch");
    let refinement = kind_is("RefinementViolation");
    let path = TsExpr::Conditional {
        test: Box::new(TsExpr::Paren(Box::new(or(
            structural.clone(),
            refinement.clone(),
        )))),
        consequent: Box::new(be("path")),
        alternate: Box::new(str_node("$")),
    };
    // The one message chain: a structural mismatch reads what it expected and what it found, a
    // refinement violation its own message, anything else its details.
    let message = TsExpr::Conditional {
        test: Box::new(structural),
        consequent: Box::new(TsExpr::template_lit(
            vec!["expected ".to_string(), ", got ".to_string(), String::new()],
            vec![
                be("expected"),
                call(ident_node("String"), vec![be("actual")]),
            ],
        )),
        alternate: Box::new(TsExpr::Conditional {
            test: Box::new(refinement),
            consequent: Box::new(member(be("violation"), "message")),
            alternate: Box::new(be("details")),
        }),
    };
    let mapped = TsExpr::object(vec![
        ("kind".to_string(), be("kind")),
        ("path".to_string(), path),
        ("message".to_string(), message),
    ]);
    vec![
        TsStmt::if_stmt(
            strict_eq(r("tag"), str_node("Ok")),
            TsStmt::return_stmt(
                Some(call(
                    ident_node("Ok"),
                    vec![TsExpr::As {
                        expr: Box::new(r("value")),
                        ty: ts.clone(),
                    }],
                )),
                None,
            ),
            None,
        ),
        const_("__be", None, r("error")),
        return_err(mapped),
    ]
}

/// A `Form.decode`'s decoder, as text: its field-spec table is `workers_entry` source text, which
/// no node can hold (1A: field-spec table is workers_entry source text).
fn form_decoder_text(ts: &TsType, specs: &str, des: &TsExpr) -> String {
    let ts = bynk_ts::print_type(ts);
    let des = bynk_ts::print_expr(des);
    format!(
        "(__s: string): Result<{ts}, DecodeError> => {{ \
         const __p = __formDecode(__s, {specs}); \
         if (__p.tag === \"Err\") return Err(__p.error); \
         const __j: __JsonValue = __p.value; \
         const __r = {des}; \
         if (__r.tag === \"Ok\") return Ok(__r.value as {ts}); \
         const __be = __r.error; \
         return Err({{ kind: __be.kind, \
         path: (__be.kind === \"StructuralMismatch\" || __be.kind === \"RefinementViolation\") ? __be.path : \"$\", \
         message: __be.kind === \"StructuralMismatch\" ? `expected ${{__be.expected}}, got ${{String(__be.actual)}}` : __be.kind === \"RefinementViolation\" ? __be.violation.message : __be.details }}); }}"
    )
}

/// #3276: the field table `Form.decode[T]` hands the form reader, as the TS array
/// literal of `{ name, kind, list, option }` the router's `@form` path builds. `None`
/// when `t` is not a record of the module's types.
fn form_specs_literal(t: bynk_check::checker::TyId, cx: &LowerCtx) -> Option<String> {
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
fn form_encode_wire(pre: &mut Pre, arg: &Expr, cx: &mut LowerCtx) -> Option<Built> {
    let tys = cx.ty_render();
    let arg_ty = cx.commons().expr_types.get(&arg.id).map(|te| te.ty)?;
    let tref = ty_to_type_ref(arg_ty, tys.types())?;
    if cx.records_codec_roots() {
        cx.runtime_use().note_json_codec_root(arg_ty);
    }
    let v = pre.lower_node(arg, cx);
    Some(serialise_value(&tref, v, cx))
}

/// `value` serialised under `tref`: a node over a node value. A value that is still text keeps
/// the serialiser's text, built over it (1A: serialiser value is text).
fn serialise_value(tref: &TypeRef, value: LoweredExpr, cx: &LowerCtx) -> Built {
    match value {
        LoweredExpr::Node(node) => Ok(super::super::serialisation::serialise_expr(
            tref,
            &node,
            cx.runtime_use(),
        )),
        LoweredExpr::Text(text) => Err(bynk_ts::print_expr(
            &super::super::serialisation::serialise_expr(tref, &text_value(text), cx.runtime_use()),
        )),
    }
}

/// `globalThis.JSON.stringify(<value>)`: a node over a node, else its text.
fn json_stringify(value: Built) -> Built {
    match value {
        Ok(node) => Ok(call(member(global("JSON"), "stringify"), vec![node])),
        Err(text) => Err(format!("globalThis.JSON.stringify({text})")),
    }
}

/// A literal printed exactly as given: one of the fixed pieces of the `bynk eval` line.
fn raw(text: &str) -> TsExpr {
    TsExpr::Lit(TsLit::Raw(text.to_string()))
}

/// `left + right`.
fn add(left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op: TsBinaryOp::Add,
        left: Box::new(left),
        right: Box::new(right),
    }
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
    // `console.log('{"type":"eval-value","valueType":' + JSON.stringify(<type>)` …
    let head = add(
        raw("'{\"type\":\"eval-value\",\"valueType\":'"),
        call(member(global("JSON"), "stringify"), vec![str_node(shown)]),
    );
    let log = |arg: TsExpr| {
        format!(
            "{};",
            bynk_ts::print_expr(&call(member(global("console"), "log"), vec![arg]))
        )
    };
    // A value with no wire shape (a function, say) still reports its type, and
    // no value.
    let Some(json) = json_encode(pre, tail, cx, false) else {
        let _ = pre.lower(tail, cx);
        return Some(log(add(head, raw("'}'"))));
    };
    // The encoder names `JsonValue` and `BoundaryError`, whether or not the
    // value's type needs a closure of codecs of its own.
    cx.runtime_use().note_json_codec();
    cx.runtime_use().note_boundary_codec();
    let json = match json {
        Ok(node) => node,
        // 1A: the value is text, so the line is too.
        Err(text) => {
            let head = bynk_ts::print_expr(&head);
            return Some(format!(
                "globalThis.console.log({head} + ',\"value\":' + {text} + '}}');"
            ));
        }
    };
    Some(log(add(
        add(add(head, raw("',\"value\":'")), json),
        raw("'}'"),
    )))
}

/// `Json.encode(arg)`: `globalThis.JSON.stringify(<serialised arg>)`, with
/// the codec roots the module needs recorded. Shared by the `Json.encode` call,
/// `Json.encodePlain` and `bynk eval`'s tail report, so each encodes a value one
/// way. A node when the argument lowers to one. `None` when the argument's type
/// is unknown to the lowering.
pub(in super::super) fn json_encode(
    pre: &mut Pre,
    arg: &Expr,
    cx: &mut LowerCtx,
    plain: bool,
) -> Option<Built> {
    let tys = cx.ty_render();
    // Constrained generics slice 3: a value whose type mentions a type
    // parameter encodes through the codec witness of that type.
    let arg_ty = cx.commons().expr_types.get(&arg.id).map(|te| te.ty)?;
    if super::super::codec::has_var(arg_ty, cx.commons()) {
        if cx.records_codec_roots() {
            note_ground_roots(arg_ty, cx);
        }
        let codec = cx.codec(arg_ty);
        let v = pre.lower_node(arg, cx);
        // `plain` never reaches a generic argument: the checker refused it.
        return Some(json_stringify(match v {
            LoweredExpr::Node(node) => Ok(call(member(codec, "encode"), vec![node])),
            LoweredExpr::Text(text) => {
                Err(format!("{}.encode({text})", bynk_ts::print_expr(&codec)))
            }
        }));
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
    let v = pre.lower_node(arg, cx);
    if plain {
        return Some(encode_plain(pre, arg, &tref, v, cx));
    }
    Some(json_stringify(serialise_value(&tref, v, cx)))
}

/// #2991: `Json.encodePlain`. The one `Option` encoder reads `__bynkPlainJson`, so raise it
/// for this one synchronous serialisation and restore it in a `finally` (the encoder throws on a
/// non-finite `Float`). The checker has refused a generic argument, so only monomorphic
/// encoders (which read the flag) run here. Inlined rather than a runtime helper, so no module
/// needs a new import.
///
/// The argument must be evaluated *before* the flag is raised, or a tagged `Json.encode` (or a
/// function that calls one) inside it would come out plain. An argument that awaits is hoisted to
/// a temporary; any other is the default of the arrow's one parameter, which JS evaluates at call
/// time, before the body raises the flag (its type is inferred). A text argument keeps the
/// default-parameter form it always had.
fn encode_plain(pre: &mut Pre, arg: &Expr, tref: &TypeRef, v: LoweredExpr, cx: &LowerCtx) -> Built {
    let value = match v {
        LoweredExpr::Node(node) => node,
        // 1A: the argument is text, so the IIFE is too.
        LoweredExpr::Text(text) => {
            let param = if text.contains("await") {
                let tmp = format!("__pj{}", arg.id.0);
                pre.push_text(format!("const {tmp} = {text};"));
                format!("__pv = {tmp}")
            } else {
                format!("__pv = {text}")
            };
            let ser = bynk_ts::print_expr(&super::super::serialisation::serialise_expr(
                tref,
                &ident_node("__pv"),
                cx.runtime_use(),
            ));
            return Err(format!(
                "(({param}): string => {{ const __g = globalThis as {{ __bynkPlainJson?: boolean }}; \
                 const __was = __g.__bynkPlainJson; __g.__bynkPlainJson = true; \
                 try {{ return globalThis.JSON.stringify({ser}); }} \
                 finally {{ __g.__bynkPlainJson = __was; }} }})()"
            ));
        }
    };
    let ser =
        super::super::serialisation::serialise_expr(tref, &ident_node("__pv"), cx.runtime_use());
    Ok(plain_iife(pre, arg.id.0, value, ser))
}

/// The `encodePlain` arrow over an argument node `value` and the serialiser `ser` of `__pv`: an
/// argument that awaits is first hoisted to `const __pj<id> = <value>;`, and the parameter
/// default is then that temporary; any other argument is the default itself.
fn plain_iife(pre: &mut Pre, id: u32, value: TsExpr, ser: TsExpr) -> TsExpr {
    let argument = if bynk_ts::expr_contains_await(&value) {
        let tmp = format!("__pj{id}");
        pre.push_node(TsStmt::const_stmt(
            TsBindingName::Ident(tmp.clone()),
            None,
            value,
            None,
        ));
        ident_node(&tmp)
    } else {
        value
    };
    let g = || ident_node("__g");
    let flag = || member(g(), "__bynkPlainJson");
    let body = vec![
        const_(
            "__g",
            None,
            TsExpr::As {
                expr: Box::new(ident_node("globalThis")),
                ty: TsType::Object(vec![TsTypeMember::optional_prop(
                    "__bynkPlainJson",
                    TsType::named("boolean"),
                )]),
            },
        ),
        const_("__was", None, flag()),
        TsStmt::assign(flag(), TsExpr::Lit(TsLit::Bool(true)), None),
        TsStmt::try_finally(
            block(vec![TsStmt::return_stmt(
                Some(call(member(global("JSON"), "stringify"), vec![ser])),
                None,
            )]),
            block(vec![TsStmt::assign(flag(), ident_node("__was"), None)]),
            None,
        ),
    ];
    TsExpr::DefaultIife {
        name: "__pv".to_string(),
        default: Box::new(argument),
        return_type: TsType::named("string"),
        body,
    }
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
fn sql_bool_pass(t: bynk_check::checker::TyId, cx: &LowerCtx) -> Vec<TsStmt> {
    let tys = cx.ty_render();
    let row = match &*tys.get(t) {
        Ty::List(r) => *r,
        _ => t,
    };
    let Ty::Named { name, .. } = &*tys.get(row) else {
        return Vec::new();
    };
    // The checker resolved this row in the same table, so a miss is a
    // compiler bug that would silently skip the pass.
    debug_assert!(
        cx.commons().types.contains_key(name.as_str()),
        "Json.decodeSql row `{name}` is not in the module's type table"
    );
    let fields = bynk_check::checker::row_bool_fields(name, &cx.commons().types);
    if fields.is_empty() {
        return Vec::new();
    }
    let o = || ident_node("__o");
    let k = || ident_node("__k");
    let row_k = || index(ident_node("__row"), k());
    let number = |n: &str| TsExpr::Lit(TsLit::Num(n.to_string()));
    // `const __flags = (__o: __JsonValue): void => { … };`
    let skip = TsStmt::if_stmt(
        or(
            or(
                strict_eq(o(), TsExpr::Lit(TsLit::Null)),
                TsExpr::Binary {
                    op: TsBinaryOp::StrictNotEq,
                    left: Box::new(TsExpr::Unary {
                        op: TsUnaryOp::Typeof,
                        expr: Box::new(o()),
                    }),
                    right: Box::new(str_node("object")),
                },
            ),
            is_array(o()),
        ),
        TsStmt::return_stmt(None, None),
        None,
    );
    let as_row = TsExpr::As {
        expr: Box::new(o()),
        ty: TsType::Object(vec![TsTypeMember::index(
            "k",
            TsType::named("string"),
            json_value_ty(),
        )]),
    };
    let read_flags = TsStmt::for_of(
        "__k",
        TsExpr::array(fields.iter().map(|f| str_node(f.as_str())).collect()),
        block(vec![TsStmt::if_stmt(
            or(
                strict_eq(row_k(), number("0")),
                strict_eq(row_k(), number("1")),
            ),
            TsStmt::assign(row_k(), strict_eq(row_k(), number("1")), None),
            None,
        )]),
        None,
    );
    let flags = TsExpr::Arrow {
        params: vec![param("__o", json_value_ty())],
        is_async: false,
        generics: Vec::new(),
        return_type: Some(TsType::named("void")),
        body: Box::new(TsArrowBody::Multiline(vec![
            skip,
            const_("__row", None, as_row),
            read_flags,
        ])),
    };
    let mut stmts = vec![const_("__flags", None, flags)];
    stmts.push(apply_to_rows("__flags"));
    stmts
}

/// `if (globalThis.Array.isArray(__j)) __j.forEach(<pass>); else <pass>(__j);`: a row pass over
/// the parsed rows, or the one row.
fn apply_to_rows(pass: &str) -> TsStmt {
    TsStmt::if_else_stmt(
        is_array(ident_node("__j")),
        TsStmt::expr_stmt(
            call(member(ident_node("__j"), "forEach"), vec![ident_node(pass)]),
            None,
        ),
        TsStmt::expr_stmt(call(ident_node(pass), vec![ident_node("__j")]), None),
        None,
    )
}

/// Hyperdrive H3 (#3320): a typed `Postgres` read's pass over the parsed rows
/// before the ordinary codec runs, the counterpart of [`sql_bool_pass`]. The
/// codec reads an `Instant` as epoch milliseconds and an `Int` as a number; a
/// Postgres row carries a `timestamptz` as ISO-8601 text and an `int8` as text
/// (ADR 0799 F), so the runtime's `__bynkPgRow` converts exactly the row's
/// `Instant` and `Int` fields, and leaves anything it cannot convert for the
/// codec to refuse. Empty when the row has neither.
fn pg_row_pass(t: bynk_check::checker::TyId, cx: &LowerCtx) -> Vec<TsStmt> {
    let tys = cx.ty_render();
    let row = match &*tys.get(t) {
        Ty::List(r) => *r,
        _ => t,
    };
    let Ty::Named { name, .. } = &*tys.get(row) else {
        return Vec::new();
    };
    let types = &cx.commons().types;
    let instants = bynk_check::checker::row_base_fields(name, types, BaseType::Instant);
    let ints = bynk_check::checker::row_base_fields(name, types, BaseType::Int);
    if instants.is_empty() && ints.is_empty() {
        return Vec::new();
    }
    cx.runtime_use().note_pg_rows();
    let names =
        |names: &[String]| TsExpr::array(names.iter().map(|f| str_node(f.as_str())).collect());
    // `const __fix = (__o: __JsonValue): void => __bynkPgRow(__o, [instants], [ints]);`
    let fix = TsExpr::Arrow {
        params: vec![param("__o", json_value_ty())],
        is_async: false,
        generics: Vec::new(),
        return_type: Some(TsType::named("void")),
        body: Box::new(TsArrowBody::Expr(Box::new(call(
            ident_node("__bynkPgRow"),
            vec![ident_node("__o"), names(&instants), names(&ints)],
        )))),
    };
    vec![const_("__fix", None, fix), apply_to_rows("__fix")]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn awaiting() -> TsExpr {
        TsExpr::Await(Box::new(call(ident_node("f"), vec![])))
    }

    /// An awaiting argument is hoisted ahead of the IIFE, and the parameter default is the
    /// temporary (no source produces one today, so the builder is driven directly).
    #[test]
    fn an_awaiting_plain_argument_is_hoisted() {
        let mut pre = Pre::new();
        let iife = plain_iife(&mut pre, 7, awaiting(), ident_node("ser"));
        assert_eq!(pre.stmts(), vec!["const __pj7 = await f();".to_string()]);
        let text = bynk_ts::print_expr(&iife);
        assert!(text.starts_with("((__pv = __pj7): string => {"), "{text}");
    }

    #[test]
    fn a_plain_argument_that_does_not_await_is_not_hoisted() {
        let mut pre = Pre::new();
        let iife = plain_iife(
            &mut pre,
            7,
            call(ident_node("g"), vec![]),
            ident_node("ser"),
        );
        assert!(pre.stmts().is_empty());
        let text = bynk_ts::print_expr(&iife);
        assert!(text.starts_with("((__pv = g()): string => {"), "{text}");
    }
}
