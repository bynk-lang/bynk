//! The shared codec vocabulary (back-half paydown 2.3a, #4088): the switch-case and property builders, the
//! path builders of 1A.40, the structural-mismatch error, and the depth-threading set every codec
//! shares. It reads no AST. Moved out of `serialisation.rs` byte for byte.

use bynk_check::wire::{JsonKind, MAX_RECURSIVE_NESTING};
use bynk_ts::{
    TsBinaryOp, TsBindingName, TsExpr, TsLit, TsObjectEntry, TsParam, TsStmt, TsSwitchCase, TsType,
    TsTypeMember,
};

use crate::ts_build::{
    as_expr, binary, block, call, ident, if_, index, member, not_expr, or_expr, ret, str_lit,
    strict_eq, strict_neq, typeof_expr,
};

/// `case <test>: <body>` — unlike `workers_entry.rs`'s own identically-named
/// helper (which always braces, since every one of its real cases wants a
/// `{ }` block), `emit_sum_codec` has BOTH real shapes side by side in the
/// same switch: a payload-free variant's case is unbraced, a
/// payload-carrying sibling's is braced (`212_json_codec`'s own mixed
/// `Status` fixture: `case "Pending": return ...;` beside `case "Shipped":
/// { ... }`). `braced` is the caller's own explicit choice rather than an
/// inferred one, so this stays a thin, honest wrapper over
/// [`TsSwitchCase::case_braced`] (`bynk-ts/src/program/stmt.rs`, #1445) instead
/// of a heuristic derived from `body`'s own shape.
pub(super) fn case_(test: TsExpr, body: Vec<TsStmt>, braced: bool) -> TsSwitchCase {
    TsSwitchCase {
        labels: vec![test],
        body,
        default_braced: false,
        case_braced: braced,
        span: None,
    }
}

/// `default: <body>` — unbraced, the same convention `workers_entry.rs`'s
/// own `default_case` helper already established for every real `default`
/// arm this file's own sum codec has (a `StructuralMismatch` one-liner).
pub(super) fn default_case(body: Vec<TsStmt>) -> TsSwitchCase {
    TsSwitchCase {
        labels: vec![],
        body,
        default_braced: false,
        case_braced: true,
        span: None,
    }
}

/// `value.<prop>`: a property of the `Option` or `Result` value a generic serialiser holds.
pub(super) fn value_prop(prop: &str) -> TsExpr {
    member(ident("value"), prop)
}

/// `obj["<key>"]`: a key of the decoded JSON object a generic deserialiser holds.
pub(super) fn obj_prop(key: &str) -> TsExpr {
    index(ident("obj"), str_lit(key))
}

/// `` `${path}.<segment>` ``: the path of a record or sum field, a template literal over the
/// codec's own `path` parameter. Paths never nest across codecs (each codec has its own `path`
/// parameter), so this and the two below cover every sub-path the serialise family builds.
pub(super) fn path_field(segment: &str) -> TsExpr {
    TsExpr::template_lit(
        vec![String::new(), [".", segment].concat()],
        vec![ident("path")],
    )
}

/// `` `${path}[${i}]` ``: the path of a `List` or `Set` element, or of a `Map` entry.
pub(super) fn path_elem() -> TsExpr {
    TsExpr::template_lit(
        vec![String::new(), "[".to_string(), "]".to_string()],
        vec![ident("path"), ident("i")],
    )
}

/// `` `${path}[${i}][<n>]` ``: the key (`0`) or the value (`1`) of a `Map` entry.
pub(super) fn path_entry(n: usize) -> TsExpr {
    TsExpr::template_lit(
        vec![
            String::new(),
            "[".to_string(),
            ["][", &n.to_string(), "]"].concat(),
        ],
        vec![ident("path"), ident("i")],
    )
}

/// `Err({ kind: "StructuralMismatch", path: <path>, expected: <expected>, actual: <actual> })`:
/// the one wire-guard failure shape every real arm of [`emit_field_deserialise_wire`] returns.
/// Distinct from `deserialise_expr_via`'s own `structural_mismatch` closure further down this
/// file in two ways that matter: `path` here is spliced in as the expression it is (a bare
/// `path` identifier, or a template literal like `` `${path}.value` ``), not a string literal to
/// quote; and the object is never cast `as BoundaryError` the way that closure's is (only
/// `deserialise_expr_via`'s own callers need the cast, since they build a whole
/// `Result<T, BoundaryError>` return value directly).
pub(super) fn err_structural_mismatch(path: &TsExpr, expected: &str, actual: TsExpr) -> TsExpr {
    call(
        ident("Err"),
        vec![TsExpr::object(vec![
            ("kind".to_string(), str_lit("StructuralMismatch")),
            ("path".to_string(), path.clone()),
            ("expected".to_string(), str_lit(expected)),
            ("actual".to_string(), actual),
        ])],
    )
}

/// `Err({ kind: "StructuralMismatch", path, expected: <expected>, actual:
/// <actual> })` — [`emit_bytes_named_codec`]/[`emit_refined`]'s own
/// top-level boundary-guard failure shape (#1441, Arc E slice 4).
///
/// Deliberately a *second*, distinct helper rather than a call to
/// [`err_structural_mismatch`] above: that one's own `path` parameter
/// is real call-site-supplied TS source text (a bare `path` identifier or a
/// template literal like `` `${path}.value` ``), rendered as an ordinary
/// `path: <path>` property because every one of its real callers
/// (`emit_field_deserialise_wire`'s own field-level guards) passes a
/// rendered *sub*-path template, never the bare top-level `path` parameter
/// itself — confirmed empirically (no fixture anywhere renders the literal
/// text `path: path`). Every real call site *here*, by contrast, validates
/// a function's own top-level `path` parameter directly, which TypeScript's
/// object-literal shorthand prints as bare `path,` — confirmed byte-for-byte
/// against `bynk-tests/tests/fixtures/positive/254_multi_file_commons_workers_
/// codec/expected/money/cents.ts:26` (`emit_refined`'s own pre-conversion
/// text). [`TsExpr::object`]'s convenience constructor only builds
/// [`TsObjectEntry::Prop`] entries, which would print `path: path` instead
/// — a different, wrong byte sequence — so this builds the mixed
/// `Prop`/[`TsObjectEntry::Shorthand`] entry list directly.
pub(super) fn err_structural_mismatch_top(expected: &str, actual: TsExpr) -> TsExpr {
    call(
        ident("Err"),
        // Review of #1442, finding 3: `TsExpr::object_entries` already
        // builds exactly this (a single-line object from a
        // `Vec<TsObjectEntry>`) — `TsExpr::object`'s own doc only rules out
        // the *other* convenience constructor, which takes `(String,
        // TsExpr)` pairs and can't represent `Shorthand`.
        vec![TsExpr::object_entries(vec![
            TsObjectEntry::prop("kind".to_string(), str_lit("StructuralMismatch")),
            TsObjectEntry::Shorthand("path".to_string()),
            TsObjectEntry::prop("expected".to_string(), str_lit(expected)),
            TsObjectEntry::prop("actual".to_string(), actual),
        ])],
    )
}

/// #4604: the `actual` of a number that passed its `typeof` check and failed its integrality
/// (`integral`) or finiteness check: a class from a closed set, never the number itself (it can be
/// personal data, and a `DecodeError` is logged).
pub(super) fn number_failure_class(integral: bool, v: &TsExpr) -> TsExpr {
    if !integral {
        return str_lit("non-finite number");
    }
    TsExpr::Conditional {
        test: Box::new(call(
            member(ident("globalThis.Number"), "isInteger"),
            vec![v.clone()],
        )),
        consequent: Box::new(str_lit("integer outside the safe range")),
        alternate: Box::new(str_lit("non-integer number")),
    }
}

/// #4604: the `actual` of a variant tag that names no variant: `unrecognised variant` for a
/// string (the stored text is never echoed), the `typeof` for anything else.
pub(super) fn tag_failure_class(v: &TsExpr) -> TsExpr {
    TsExpr::Conditional {
        test: Box::new(strict_eq(typeof_expr(v.clone()), str_lit("string"))),
        consequent: Box::new(str_lit("unrecognised variant")),
        alternate: Box::new(typeof_expr(v.clone())),
    }
}

/// The TS token a [`JsonKind`] spells as. `json_kind_of` (`bynk_check::wire`)
/// replaces `ts_base_for_serialisation`'s *classification* of a `BaseType`;
/// this is the TS-spelling half the wire.rs seam keeps in `bynk-emit`. Also
/// doubles as the boundary `typeof` check string — for every `BaseType` the
/// two coincide (a bare `Int`/`Float`/`Duration`/`Instant`/`String`/`Bytes`/
/// `Bool` field is validated against exactly the JSON `typeof` its kind
/// implies), so a single call replaces what used to be two identical `match
/// base` blocks.
pub(super) fn json_kind_ts(k: JsonKind) -> &'static str {
    match k {
        JsonKind::Number => "number",
        JsonKind::String => "string",
        JsonKind::Boolean => "boolean",
        JsonKind::Object => "object",
        JsonKind::Array => "array",
        JsonKind::Null => "null",
    }
}

/// `path: string = "$"`: the JSON path every generated decoder starts from, a typed parameter
/// with a default value (#3996 moved it off a raw-text name onto [`TsParam::with_default`]).
pub(super) fn deserialise_path_param() -> TsParam {
    TsParam::new("path", Some(TsType::named("string"))).with_default(str_lit("$"))
}

/// #2656: the set of codec suffixes that carry a nesting depth
/// ([`bynk_check::wire::recursive_codec_suffixes`]), computed once per
/// emission call from the type table. A codec on a type cycle takes a trailing
/// `depth: number = 0` parameter and passes it on to every callee in the set;
/// every other codec is emitted exactly as before.
pub(super) type DepthCodecs = std::collections::HashSet<String>;

/// #2656: how one codec body passes the nesting depth to its callees. `None`
/// for a codec off every type cycle, which has no `depth` in scope and calls
/// every codec with the default.
#[derive(Clone, Copy)]
pub(super) struct Depth<'a> {
    codecs: &'a DepthCodecs,
    /// Whether the depth argument a callee in `codecs` receives is `depth + 1` (a named type's
    /// codec, which is one level of nesting) or `depth` (a container's, `List_Dir`, which is not).
    level: bool,
}

/// #2656: the `depth` a codec passes on, or `None` when `depth` is not in
/// scope or `suffix` does not take one.
pub(super) fn depth_arg(depth: Option<Depth<'_>>, suffix: &str) -> Option<TsExpr> {
    depth.filter(|d| d.codecs.contains(suffix)).map(|d| {
        if d.level {
            binary(
                TsBinaryOp::Add,
                ident("depth"),
                TsExpr::Lit(TsLit::Num("1".to_string())),
            )
        } else {
            ident("depth")
        }
    })
}

/// #2656: the [`Depth`] context for the body of the codec named `suffix`:
/// `Some` only when that codec is on a type cycle. `level` is true for a named
/// type's codec (record or sum, generic instantiations included), which counts
/// one level, and false for a container's.
pub(super) fn depth_for<'a>(
    codecs: &'a DepthCodecs,
    suffix: &str,
    level: bool,
) -> Option<Depth<'a>> {
    codecs.contains(suffix).then_some(Depth { codecs, level })
}

/// #2656: `depth: number = 0`, the trailing parameter of a codec on a type
/// cycle — a typed parameter with a default, as [`deserialise_path_param`] is.
pub(super) fn depth_param() -> TsParam {
    TsParam::new("depth", Some(TsType::named("number")))
        .with_default(TsExpr::Lit(TsLit::Num("0".to_string())))
}

/// `Result<ok, __BoundaryError>`: what every generated decoder returns.
pub(super) fn boundary_result(ok: TsType) -> TsType {
    TsType::named_with_args("Result", vec![ok, TsType::named("__BoundaryError")])
}

/// `json: __JsonValue, path: string = "$"`, plus [`depth_param`] for a codec
/// on a type cycle (#2656): the parameters of every generated decoder.
pub(super) fn deserialise_params(depth: Option<Depth<'_>>) -> Vec<TsParam> {
    let mut params = vec![
        TsParam::new("json".to_string(), Some(TsType::named("__JsonValue"))),
        deserialise_path_param(),
    ];
    if depth.is_some() {
        params.push(depth_param());
    }
    params
}

/// `value: <ty>`, plus [`depth_param`] for a codec on a type cycle (#2656):
/// the parameters of every generated encoder.
pub(super) fn serialise_params(ty: TsType, depth: Option<Depth<'_>>) -> Vec<TsParam> {
    let mut params = vec![TsParam::new("value".to_string(), Some(ty))];
    if depth.is_some() {
        params.push(depth_param());
    }
    params
}

/// #2656: the nesting bound's test, `depth >= <bound>`.
fn depth_exceeded() -> TsExpr {
    binary(
        TsBinaryOp::GreaterThanEq,
        ident("depth"),
        TsExpr::Lit(TsLit::Num(MAX_RECURSIVE_NESTING.to_string())),
    )
}

/// #2656: the `expected` of the nesting bound's `StructuralMismatch`.
pub(super) fn depth_expected() -> String {
    format!("a value nested at most {MAX_RECURSIVE_NESTING} levels deep")
}

/// #2656: the first statement of a recursive type's decoder: a value nested
/// past the bound is a `StructuralMismatch` at its own path, so the boundary
/// answers it as it answers any other malformed value (an HTTP `400`, an
/// `Err` from `Json.decode`), rather than recursing until the host's stack
/// overflows.
pub(super) fn depth_guard_decode() -> TsStmt {
    if_(
        depth_exceeded(),
        block(vec![ret(err_structural_mismatch_top(
            &depth_expected(),
            str_lit("a value nested more deeply"),
        ))]),
    )
}

/// #2656: the first statement of a recursive type's encoder: a value nested
/// past the bound is a contract violation and throws, as a non-finite `Float`
/// does, so a context never sends or stores a value its peer, or its own
/// rehydration, must refuse.
pub(super) fn depth_guard_encode() -> TsStmt {
    if_(
        depth_exceeded(),
        TsStmt::throw_stmt(
            TsExpr::New {
                callee: Box::new(ident("globalThis.Error")),
                args: vec![str_lit(format!(
                    "value nested more than {MAX_RECURSIVE_NESTING} levels deep at boundary"
                ))],
            },
            None,
        ),
    )
}

/// `{ [k: string]: JsonValue }` — the structural type
/// `emit_record_codec`'s own `const obj = json as { ... };` cast needs
/// before indexing it by field name. #1443 (Arc E slice 5): the identical
/// shape `workers_entry.rs`'s own `index_signature_record_ty` already
/// builds for its multi-param `on call` dispatch — kept as a second, local
/// copy rather than shared cross-file, matching this file's own "private
/// builder set" convention (#1435's own doc) rather than promoting a
/// two-caller shape into `bynk-ts` itself.
pub(super) fn index_signature_record_ty() -> TsType {
    TsType::Object(vec![TsTypeMember::index(
        "k",
        TsType::named("string"),
        TsType::named("__JsonValue"),
    )])
}

/// `if (typeof json !== "object" || json === null || Array.isArray(json)) {
/// return Err(...); } const obj = json as { [k: string]: JsonValue };` — the
/// exact 2-statement object-shape guard [`emit_record_codec`] already builds
/// inline (#1443), needed verbatim by [`emit_generic_helpers_qualified`]'s
/// own `ResultInst` arm (Arc E slice 7, #1447; the `OptionInst` arm used it
/// too until #1887 made `Option` decoding lenient, which must accept `null`
/// and non-object bare values this guard rejects). A local helper
/// here rather than a shared export, or a call into `emit_record_codec`
/// itself: matches this file's own "private builder set, not promoted
/// cross-function" precedent (#1435's own doc) — two real call sites inside
/// this one function is exactly the shape that precedent already covers,
/// and `emit_record_codec` has no reason to know this function exists.
pub(super) fn object_shape_guard() -> Vec<TsStmt> {
    vec![
        if_(
            or_expr(
                or_expr(
                    strict_neq(typeof_expr(ident("json")), str_lit("object")),
                    strict_eq(ident("json"), TsExpr::Lit(TsLit::Null)),
                ),
                call(
                    member(ident("globalThis.Array"), "isArray"),
                    vec![ident("json")],
                ),
            ),
            block(vec![ret(err_structural_mismatch_top(
                "object",
                typeof_expr(ident("json")),
            ))]),
        ),
        TsStmt::const_stmt(
            TsBindingName::Ident("obj".to_string()),
            None,
            as_expr(ident("json"), index_signature_record_ty()),
            None,
        ),
    ]
}

/// `if (!Array.isArray(json)) { return Err(...); }` — `ListInst`/`MapInst`'s
/// own shared array-shape guard (Arc E slice 7, #1447), the array-typed
/// sibling of [`object_shape_guard`] just above.
pub(super) fn array_shape_guard() -> TsStmt {
    if_(
        not_expr(call(
            member(ident("globalThis.Array"), "isArray"),
            vec![ident("json")],
        )),
        block(vec![ret(err_structural_mismatch_top(
            "array",
            typeof_expr(ident("json")),
        ))]),
    )
}
