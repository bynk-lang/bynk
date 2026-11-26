//! A record field at a boundary (back-half paydown 2.3a, #4088): field decode and encode, wire forms and
//! defaults, with their tests. Moved out of `serialisation.rs` byte for byte.

use bynk_check::wire::{BaseGuard, UncheckedReason, WireRef};
use bynk_syntax::ast::{BaseType, TypeRef};
use bynk_ts::{TsArrowBody, TsExpr, TsLit, TsParam, TsStmt, TsType, TsUnaryOp};

use crate::emitter::RuntimeUse;
use crate::ts_build::{
    as_expr, block, call, const_, ident, if_, member, not_expr, ret, str_lit, strict_eq,
    strict_neq, typeof_expr,
};

use super::support::{Depth, depth_arg, err_structural_mismatch, json_kind_ts};
use super::wire_ref_of;

#[cfg(test)]
use super::support::{path_elem, path_entry, path_field};
#[cfg(test)]
use crate::emitter::default_value::wire_default_node;
#[cfg(test)]
use bynk_check::wire::JsonKind;
#[cfg(test)]
use bynk_check::wire_default::field_default_value;
#[cfg(test)]
use bynk_syntax::ast::{TypeBody, TypeDecl};
#[cfg(test)]
use std::sync::Arc;

/// Emit a let binding `__<field>` after destructuring & validating a
/// nested field.
///
/// #855 (Phase 2 step 6): dispatches on [`WireRef`] (via [`wire_ref_of`])
/// rather than matching `TypeRef` directly — the same resolution
/// `wire_ref` documents itself as mirroring one-for-one, **including** its
/// `Effect`/`HttpResult`/etc. field-position arm folding into the same
/// unchecked cast as `ValidationError`/`DecodeError`/`QueueResult`. This is
/// the deserialise-side function `wire_ref`'s own doc names as the one it
/// agrees with exactly; contrast [`serialise_field_expr_via`], which does
/// not.
///
/// #855 (Phase 2 step 7): a thin `TypeRef` → [`WireRef`] wrapper over
/// [`emit_field_deserialise_wire`] — a record/sum field built from the IR
/// already carries its [`WireRef`] shape (`WireField::shape`) and calls that
/// directly, with no `TypeRef` to convert back from.
///
/// #1439 (Arc E slice 3): returns the same `Vec<bynk_ts::TsStmt>`
/// [`emit_field_deserialise_wire`] now does, rather than writing into an
/// `out: &mut String` — this function's only job is resolving `t` to a
/// [`WireRef`] before delegating, so it carries the signature change with
/// zero body change beyond the return.
pub(super) fn emit_field_deserialise(
    name: &str,
    t: &TypeRef,
    json: &TsExpr,
    path: &TsExpr,
    depth: Option<Depth<'_>>,
    ru: &RuntimeUse,
) -> Vec<TsStmt> {
    emit_field_deserialise_wire(name, &wire_ref_of(t), json, path, depth, ru)
}

/// The [`WireRef`]-driven body [`emit_field_deserialise`] delegates to,
/// exposed directly for a caller that already holds a [`WireRef`] (a
/// [`WireField`]'s `shape`) rather than the `TypeRef` it was resolved from.
///
/// #1439 (Arc E slice 3): returns a real `Vec<bynk_ts::TsStmt>` — a
/// *sequence* of statements (one or more guard `if`s, then a `const`
/// binding), not one expression, unlike slices 1/2's own `-> TsExpr`
/// conversions (#1436/#1438) — hence `Vec`, not a single node. Of this
/// function's 8 real call sites, 7 (`emit_sum_codec`,
/// `emit_generic_helpers_qualified`) are still `write!`-based, so each
/// splices the result back in via `splice_stmts` (since removed, 1A.38a) at the same one-level
/// indent the original `writeln!` calls always used, confirmed
/// byte-identical against the fixture corpus. The 8th, `emit_record_codec`,
/// became tree-native itself (#1443, Arc E slice 5) and now `extend`s the
/// returned statements directly into its own `Vec<TsStmt>` body — no print,
/// no splice, since both sides of that call are real nodes.
pub(super) fn emit_field_deserialise_wire(
    name: &str,
    wire: &WireRef,
    json: &TsExpr,
    path: &TsExpr,
    depth: Option<Depth<'_>>,
    ru: &RuntimeUse,
) -> Vec<TsStmt> {
    match wire {
        // v0.110 (ADR 0142 D5): a bare `Bytes` field is a base64 JSON string —
        // require a string, then decode (rejecting invalid base64), binding the
        // decoded `Uint8Array`. This is the one base type whose wire value is
        // not a direct cast of its erased representation.
        WireRef::Bytes => {
            ru.note_bytes();
            vec![
                if_(
                    strict_neq(typeof_expr(json.clone()), str_lit("string")),
                    block(vec![ret(err_structural_mismatch(
                        path,
                        "base64 string",
                        typeof_expr(json.clone()),
                    ))]),
                ),
                const_(
                    format!("__b_{name}"),
                    call(ident("__bynkBytesFromBase64"), vec![json.clone()]),
                ),
                if_(
                    strict_eq(member(ident(format!("__b_{name}")), "tag"), str_lit("None")),
                    block(vec![ret(err_structural_mismatch(
                        path,
                        "base64 string",
                        str_lit("invalid base64"),
                    ))]),
                ),
                const_(
                    format!("__{name}"),
                    member(ident(format!("__b_{name}")), "value"),
                ),
            ]
        }
        // #2622: a bare `Decimal` field is a canonical decimal JSON string -
        // require a string, then parse it (rejecting any other spelling).
        WireRef::Decimal => {
            ru.note_decimal("__bynkDecimalParseWire");
            vec![
                if_(
                    strict_neq(typeof_expr(json.clone()), str_lit("string")),
                    block(vec![ret(err_structural_mismatch(
                        path,
                        "decimal string",
                        typeof_expr(json.clone()),
                    ))]),
                ),
                const_(
                    format!("__b_{name}"),
                    call(ident("__bynkDecimalParseWire"), vec![json.clone()]),
                ),
                if_(
                    strict_eq(member(ident(format!("__b_{name}")), "tag"), str_lit("None")),
                    block(vec![ret(err_structural_mismatch(
                        path,
                        "decimal string",
                        str_lit("invalid decimal"),
                    ))]),
                ),
                const_(
                    format!("__{name}"),
                    member(ident(format!("__b_{name}")), "value"),
                ),
            ]
        }
        WireRef::Base {
            json: kind,
            guards,
            base,
            ..
        } => {
            let typeof_str = json_kind_ts(*kind);
            // #3974: an `Instant` also reads as an RFC 3339 string, tried after the
            // tagged epoch-millisecond form: the helper passes a number (or anything it
            // cannot read) through, so the guards below see exactly what they did.
            let mut stmts = Vec::new();
            let instant_json;
            let json = if *base == BaseType::Instant {
                ru.note_time("__bynkInstantFromWire");
                let tmp = format!("__{name}$ms");
                stmts.push(const_(
                    tmp.clone(),
                    call(ident("__bynkInstantFromWire"), vec![json.clone()]),
                ));
                instant_json = ident(tmp);
                &instant_json
            } else {
                json
            };
            stmts.push(if_(
                strict_neq(typeof_expr(json.clone()), str_lit(typeof_str)),
                block(vec![ret(err_structural_mismatch(
                    path,
                    typeof_str,
                    typeof_expr(json.clone()),
                ))]),
            ));
            // v0.22b: bare `Int` fields validate integrality (ADR 0049) —
            // with `Float` in the language there is no excuse for a
            // fractional `Int` from the wire. v0.90 (ADR 0114 D7): an `Instant`
            // is whole epoch milliseconds, so it validates integrality too.
            // v0.21: boundary `Float` values are finite (ADR 0040) —
            // `JSON.parse("1e999")` yields `Infinity`, which must not be
            // admitted from the wire.
            for guard in guards {
                let (method, expected) = match guard {
                    // #1657: whole *and* within ±(2^53 − 1).
                    BaseGuard::Integral => ("isSafeInteger", "safe integer"),
                    BaseGuard::Finite => ("isFinite", "finite number"),
                };
                stmts.push(if_(
                    not_expr(call(
                        member(ident("globalThis.Number"), method),
                        vec![json.clone()],
                    )),
                    block(vec![ret(err_structural_mismatch(
                        path,
                        expected,
                        call(ident("String"), vec![json.clone()]),
                    ))]),
                ));
            }
            stmts.push(const_(format!("__{name}"), json.clone()));
            stmts
        }
        // Named type (own module or generic instantiation, both keyed the
        // same way — `Named` for a declared type, `Inst` for `Result` /
        // `Option` / `List` / `Map` / a generic `App`): defer to its own
        // `deserialise_<key>`. Assumes it exists in scope (imported or
        // declared locally).
        // Review of #1440: `Named`/`Inst` differ only in which callee they
        // name (`type_name`/`key`, both a bare `String` — `bynk-check/src/
        // wire.rs`'s own `WireRef` fields) — collapsed via an or-pattern
        // rather than kept as two ~identical arms, the same "one shape, one
        // arm" discipline this tree's own exhaustiveness rules already
        // enforce for the printer side.
        WireRef::Named { name: type_name } | WireRef::Inst { key: type_name } => vec![
            const_(
                format!("__r_{name}"),
                call(
                    ident(format!(
                        "__deserialise_{}",
                        bynk_check::emitted_names::codec_key(type_name)
                    )),
                    [json.clone(), path.clone()]
                        .into_iter()
                        .chain(depth_arg(depth, type_name))
                        .collect(),
                ),
            ),
            if_(
                strict_eq(member(ident(format!("__r_{name}")), "tag"), str_lit("Err")),
                ret(ident(format!("__r_{name}"))),
            ),
            const_(
                format!("__{name}"),
                member(ident(format!("__r_{name}")), "value"),
            ),
        ],
        WireRef::Unit => vec![const_(format!("__{name}"), ident("undefined"))],
        // The runtime-owned error family, plus a stray field-position
        // `Effect` (see this function's doc): no generated codec to name, so
        // the value is cast through unchecked.
        //
        // #1319: closes the `ts_any` residual for the four runtime-owned
        // error types — each now casts through its own real, exported
        // runtime type (`bynk-emit/runtime/src/errors.ts`/`queue.ts`/
        // `http.ts`), and imported because the module's tree names it (the
        // header derives its imports from the tree, 1A.36b-1). `Effect` stays `any`: a stray field-position
        // `Effect[T]` genuinely has no shape to derive one from (this
        // function's own doc explains why it reaches here at all), and
        // narrowing it is not part of this residual — `unknown` would
        // compile but silently drop the fact that it's still open, which is
        // worse than leaving `any` visible.
        //
        // #1439 (Arc E slice 3): the cast is now a real `TsExpr::As` node
        // (nested `As`-under-`As` for the four bridged-through-`unknown`
        // reasons, mirroring `serialise_field_expr_wire`'s own
        // `WireRef::Unchecked` arm's identical shape) rather than a raw
        // `"as any"`/`"as unknown as X"` string glued onto `{json} {cast}`.
        // Per the accepted proposal's own explicit note: this moves the
        // *construction* of the `Effect` arm's `any` from a string to a
        // real node, but does NOT close the `ts_any` residual — `xtask`'s
        // probe was deliberately widened (#1322's review) to also match
        // `named("any"` in Rust source, so the count is expected to stay at
        // 31, not drop to 30.
        WireRef::Unchecked { reason } => {
            let value = match reason {
                UncheckedReason::Effect => as_expr(json.clone(), TsType::named("any")),
                UncheckedReason::ValidationError => as_expr(
                    as_expr(json.clone(), TsType::named("unknown")),
                    TsType::named("ValidationError"),
                ),
                UncheckedReason::DecodeError => as_expr(
                    as_expr(json.clone(), TsType::named("unknown")),
                    TsType::named("DecodeError"),
                ),
                UncheckedReason::HttpResult => as_expr(
                    as_expr(json.clone(), TsType::named("unknown")),
                    TsType::named_with_args("HttpResult", vec![TsType::named("unknown")]),
                ),
                UncheckedReason::QueueResult => as_expr(
                    as_expr(json.clone(), TsType::named("unknown")),
                    TsType::named("QueueResult"),
                ),
                // #1883, slice 1a: unreachable in a checked program
                // (`bynk.types.asset_at_boundary`); total for the model.
                UncheckedReason::Asset => as_expr(
                    as_expr(json.clone(), TsType::named("unknown")),
                    crate::emitter::ts_types::asset_ts_type(),
                ),
            };
            vec![const_(format!("__{name}"), value)]
        }
    }
}

pub(super) fn serialise_field_expr(
    t: &TypeRef,
    value: &TsExpr,
    depth: Option<Depth<'_>>,
    ru: &RuntimeUse,
) -> TsExpr {
    if let TypeRef::Effect(inner, _) = t {
        return serialise_field_expr(inner, value, depth, ru);
    }
    serialise_field_expr_wire(&wire_ref_of(t), value, "", depth, ru)
}

/// The same dispatch, reaching its helpers through `ns` — `""` for a
/// module-local call, `"handlers."` from a Worker entry point that imports the
/// context's handlers as a namespace. Threading the prefix (rather than each
/// caller owning a parallel dispatch) is what keeps the boundary to **one**
/// codec path.
///
/// #855 (Phase 2 step 6): dispatches on [`WireRef`] (via [`wire_ref_of`]),
/// **except** `Effect`, which this function peels itself before consulting
/// the IR. `wire_ref`'s own doc names this exact asymmetry: a field-position
/// `Effect` is `Unchecked` under `wire_ref` (matching
/// [`emit_field_deserialise`], the resolver's one faithful consumer), but
/// this function has always *recursed* into the wrapped type instead — an
/// `Effect`-typed field serialises as its payload's codec, not as an opaque
/// cast. Routing `Effect` through `wire_ref` here would silently change that
/// to an unchecked cast, so it stays a manual peel rather than becoming a
/// second `WireRef` arm with no second consumer.
pub(super) fn serialise_field_expr_via(
    t: &TypeRef,
    value: &TsExpr,
    ns: &str,
    ru: &RuntimeUse,
) -> TsExpr {
    if let TypeRef::Effect(inner, _) = t {
        return serialise_field_expr_via(inner, value, ns, ru);
    }
    serialise_field_expr_wire(&wire_ref_of(t), value, ns, None, ru)
}

/// A number crossing the boundary, checked with `Number.<check>` first: a
/// self-contained IIFE that throws `message` on failure, so the module needs no
/// extra runtime import.
///
/// #1435 (Arc E slice 1): the first real [`TsArrowBody::Block`] site in this
/// tree — a statement body (`if`/`throw`, then `return`), not reducible to one
/// expression the way every other arm here is. See that type's own doc
/// (`bynk-ts/src/program/expr.rs`) for why this widens the existing `Arrow.body`
/// field rather than adding a new `TsExpr` variant.
fn guarded_number(value: &TsExpr, check: &str, message: &str) -> TsExpr {
    call(
        TsExpr::Arrow {
            params: vec![TsParam::new("v".to_string(), Some(TsType::named("number")))],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Block(vec![
                TsStmt::if_stmt(
                    TsExpr::Unary {
                        op: TsUnaryOp::Not,
                        expr: Box::new(call(
                            member(ident("globalThis.Number"), check),
                            vec![ident("v")],
                        )),
                    },
                    TsStmt::throw_stmt(
                        TsExpr::New {
                            callee: Box::new(ident("globalThis.Error")),
                            args: vec![str_lit(message)],
                        },
                        None,
                    ),
                    None,
                ),
                TsStmt::return_stmt(
                    Some(as_expr(ident("v"), TsType::named("__JsonValue"))),
                    None,
                ),
            ])),
        },
        vec![value.clone()],
    )
}

/// The [`WireRef`]-driven body [`serialise_field_expr_via`] delegates to
/// (after its manual `Effect` peel — see that function's doc), exposed
/// directly for a caller that already holds a [`WireRef`] (a [`WireField`]'s
/// `shape`) rather than the `TypeRef` it was resolved from. #855 (Phase 2
/// step 7): a record/sum field's shape can never legally be a field-position
/// `Effect` (non-storable, non-boundary — rejected before it could reach a
/// declared field), so calling this directly for such a field, skipping the
/// `Effect` peel above, is not a second disagreement with `wire_ref` — it is
/// the same dispatch on a shape the peel could never have matched anyway.
pub(super) fn serialise_field_expr_wire(
    wire: &WireRef,
    value: &TsExpr,
    ns: &str,
    depth: Option<Depth<'_>>,
    ru: &RuntimeUse,
) -> TsExpr {
    match wire {
        // Named type or generic instantiation (`Result`/`Option`/`List`/`Map`/
        // a generic `App` all key the same way — see `wire_ref`'s doc):
        // serialise through its own `serialise_<key>`. #2656: a codec on a
        // type cycle passes the nesting depth on to a callee on one.
        WireRef::Named { name: key } | WireRef::Inst { key } => call(
            ident(format!(
                "{ns}__serialise_{}",
                bynk_check::emitted_names::codec_key(key)
            )),
            std::iter::once(value.clone())
                .chain(depth_arg(depth, key))
                .collect(),
        ),
        // v0.21: serialising a non-finite `Float` is a contract violation
        // (`JSON.stringify(NaN)` would silently produce `null`); the guard is
        // a self-contained IIFE (`guarded_number`).
        WireRef::Base {
            base: BaseType::Float,
            ..
        } => guarded_number(value, "isFinite", "non-finite Float at boundary"),
        // #1657 (Decision C): an `Int` outside the safe-integer range (`Infinity`
        // from `5 / 0` before the division trap, or a value past 2^53 from
        // arithmetic) would write `null` or an imprecise number, a silent
        // wire-contract violation the consumer's decoder then rejects. Fault
        // here instead, as a non-finite `Float` does.
        WireRef::Base {
            base: BaseType::Int,
            ..
        } => guarded_number(
            value,
            "isSafeInteger",
            "Int outside the safe-integer range at boundary",
        ),
        // #1969: a `Duration` or `Instant` is whole milliseconds in the `Int`
        // domain, and the receiver rejects anything else (`field_base_guards`),
        // so a fractional or unsafe one faults here, as an `Int` does, rather
        // than reaching the other side as a 400.
        WireRef::Base {
            base: b @ (BaseType::Duration | BaseType::Instant),
            ..
        } => {
            let ms = guarded_number(
                value,
                "isSafeInteger",
                &format!("{} outside the safe-integer range at boundary", b.name()),
            );
            // #3974: in plain mode an `Instant` is an RFC 3339 string in UTC with
            // exactly three fraction digits; `Duration` stays milliseconds.
            if *b == BaseType::Instant && ru.plain_json() {
                ru.note_time("__bynkInstantToIso");
                TsExpr::Conditional {
                    test: Box::new(super::generics::plain_json_test()),
                    consequent: Box::new(call(ident("__bynkInstantToIso"), vec![value.clone()])),
                    alternate: Box::new(ms),
                }
            } else {
                ms
            }
        }
        // v0.110 (ADR 0142 D5): a `Bytes` is base64-encoded on the wire — the
        // one base type whose serialise is an encode, not a bare cast.
        WireRef::Bytes => {
            ru.note_bytes();
            as_expr(
                call(ident("__bynkBytesToBase64"), vec![value.clone()]),
                TsType::named("__JsonValue"),
            )
        }
        // #2622: a `Decimal` is written as its canonical text, never as a
        // `bigint` (which `JSON.stringify` rejects).
        WireRef::Decimal => {
            ru.note_decimal("__bynkDecimalToString");
            as_expr(
                call(ident("__bynkDecimalToString"), vec![value.clone()]),
                TsType::named("__JsonValue"),
            )
        }
        WireRef::Base { .. } => as_expr(value.clone(), TsType::named("__JsonValue")),
        // The runtime-owned error types have no *generated* codec — they are
        // declared by the runtime, not by a `TypeDecl` this emitter can walk, so
        // there is no `serialise_ValidationError` to name. They keep the
        // pass-through the whole boundary used before this increment; unifying
        // the user-type paths does not reach them. Their JSON shape is fixed by
        // the runtime (`errors.ts`), so the cast is not *wrong* — it is simply
        // unchecked, and it is the one remaining unchecked arm at the boundary.
        // (`Effect` never lands here via `serialise_field_expr_via` — it is
        // peeled above; a record/sum field can never legally be `Effect` in
        // the first place — see this function's doc.)
        //
        // Review of #1319/#1320, finding 1's own tsc run (a real fixture with
        // one of these types finally reached this arm — none had before):
        // `value as JsonValue` fails `tsc --strict` (TS2352) for
        // `ValidationError`/`DecodeError` specifically — real interfaces, not
        // structurally close enough to `JsonValue`'s own union for a direct
        // cast; `HttpResult<T>`/`QueueResult`'s own union shapes happen to
        // pass today, but that is a heuristic, not a guarantee this residual
        // should depend on. Bridging through `unknown` (the fix `tsc` itself
        // names) is safe and uniform for all five reasons, `Effect` included.
        //
        // A nested `As`-under-`As` (not one opaque double-cast string): safe
        // here because this function's own result is never rendered through
        // `render_operand` (nothing downstream uses it as a further Member/
        // Unary/Binary/Index operand) — every real caller either splices it
        // into a `Call`/`Object`/`Const` position via `render_expr`'s own
        // plain recursion, or stringifies the whole tree at a print boundary
        // (`bynk_ts::print_expr`), and `render_expr`'s own `As` arm only
        // parenthesises a `Binary`/`Arrow`/`Conditional` inner expression —
        // never a nested `As` — so this renders as the intended, parenless
        // `value as unknown as JsonValue`.
        WireRef::Unchecked { .. } => as_expr(
            as_expr(value.clone(), TsType::named("unknown")),
            TsType::named("__JsonValue"),
        ),
        WireRef::Unit => TsExpr::Lit(TsLit::Null),
    }
}

/// #1439 (Arc E slice 3): direct unit coverage for
/// [`emit_field_deserialise_wire`], byte-checked against the exact strings
/// its pre-conversion `writeln!` body used to produce. The project-form
/// fixture corpus reaches most of these arms already (`bless_positive_
/// fixtures`/`positive_fixtures` passing with zero fixture diff is the
/// stronger, end-to-end proof for those), but a real gap check (per this
/// issue's own "confirm this at implementation time" instruction) found
/// `WireRef::Unchecked { reason: UncheckedReason::Effect }` is NOT reached
/// by any real fixture — `1319_runtime_error_type_fields/expected/
/// errortypes.ts` exercises the other four `Unchecked` reasons (via a
/// record with one field of each runtime-owned error type), but a
/// field-position `Effect` never appears in that fixture or any other in
/// the corpus (a record/sum field can never legally *be* `Effect` per
/// `wire_ref`'s own doc — this arm is reached only via the `TypeRef` path
/// this file's callers don't exercise for that specific shape). This
/// module closes that gap directly rather than leaving the arm asserted
/// only by inspection.
#[cfg(test)]
mod emit_field_deserialise_wire_tests {
    use super::*;
    use bynk_check::wire::Expected;

    fn render(stmts: Vec<TsStmt>) -> String {
        stmts
            .iter()
            .map(|s| bynk_ts::print_stmt(s, 1))
            .collect::<Vec<_>>()
            .join("")
    }

    /// The three path builders are the template literals the text spelled, and print as them.
    #[test]
    fn the_path_builders_are_template_literals_over_path_and_i() {
        assert_eq!(
            path_field("name"),
            TsExpr::template_lit(
                vec![String::new(), ".name".to_string()],
                vec![ident("path")]
            )
        );
        assert_eq!(
            path_elem(),
            TsExpr::template_lit(
                vec![String::new(), "[".to_string(), "]".to_string()],
                vec![ident("path"), ident("i")]
            )
        );
        assert_eq!(
            path_entry(1),
            TsExpr::template_lit(
                vec![String::new(), "[".to_string(), "][1]".to_string()],
                vec![ident("path"), ident("i")]
            )
        );
        assert_eq!(bynk_ts::print_expr(&path_field("name")), "`${path}.name`");
        assert_eq!(bynk_ts::print_expr(&path_elem()), "`${path}[${i}]`");
        assert_eq!(bynk_ts::print_expr(&path_entry(0)), "`${path}[${i}][0]`");
    }

    #[test]
    fn unchecked_effect_casts_through_any() {
        let ru = RuntimeUse::default();
        let stmts = emit_field_deserialise_wire(
            "name",
            &WireRef::Unchecked {
                reason: UncheckedReason::Effect,
            },
            &ident("json"),
            &ident("path"),
            None,
            &ru,
        );
        assert_eq!(render(stmts), "  const __name = json as any;\n");
    }

    #[test]
    fn unchecked_validation_error_casts_through_unknown() {
        let ru = RuntimeUse::default();
        let stmts = emit_field_deserialise_wire(
            "name",
            &WireRef::Unchecked {
                reason: UncheckedReason::ValidationError,
            },
            &ident("json"),
            &ident("path"),
            None,
            &ru,
        );
        assert_eq!(
            render(stmts),
            "  const __name = json as unknown as ValidationError;\n"
        );
    }

    #[test]
    fn unchecked_http_result_names_unknown_element_type() {
        let ru = RuntimeUse::default();
        let stmts = emit_field_deserialise_wire(
            "name",
            &WireRef::Unchecked {
                reason: UncheckedReason::HttpResult,
            },
            &ident("json"),
            &ident("path"),
            None,
            &ru,
        );
        assert_eq!(
            render(stmts),
            "  const __name = json as unknown as HttpResult<unknown>;\n"
        );
    }

    #[test]
    fn unit_arm_binds_undefined() {
        let ru = RuntimeUse::default();
        let stmts = emit_field_deserialise_wire(
            "name",
            &WireRef::Unit,
            &ident("json"),
            &ident("path"),
            None,
            &ru,
        );
        assert_eq!(render(stmts), "  const __name = undefined;\n");
    }

    #[test]
    fn named_arm_propagates_the_error_branch() {
        let ru = RuntimeUse::default();
        let stmts = emit_field_deserialise_wire(
            "name",
            &WireRef::Named {
                name: "Foo".to_string(),
            },
            &ident("json"),
            &ident("path"),
            None,
            &ru,
        );
        assert_eq!(
            render(stmts),
            "  const __r_name = __deserialise_Foo(json, path);\n  if (__r_name.tag === \"Err\") return __r_name;\n  const __name = __r_name.value;\n"
        );
    }

    #[test]
    fn bytes_arm_decodes_base64_with_two_guards() {
        let ru = RuntimeUse::default();
        let stmts = emit_field_deserialise_wire(
            "name",
            &WireRef::Bytes,
            &ident("json"),
            &ident("path"),
            None,
            &ru,
        );
        assert_eq!(
            render(stmts),
            "  if (typeof json !== \"string\") {\n    return Err({ kind: \"StructuralMismatch\", path: path, expected: \"base64 string\", actual: typeof json });\n  }\n  const __b_name = __bynkBytesFromBase64(json);\n  if (__b_name.tag === \"None\") {\n    return Err({ kind: \"StructuralMismatch\", path: path, expected: \"base64 string\", actual: \"invalid base64\" });\n  }\n  const __name = __b_name.value;\n"
        );
    }

    #[test]
    fn base_arm_with_integral_and_finite_guards() {
        let ru = RuntimeUse::default();
        let wire = WireRef::Base {
            base: BaseType::Float,
            json: JsonKind::Number,
            guards: vec![BaseGuard::Integral, BaseGuard::Finite],
            expected: Expected::Json(JsonKind::Number),
        };
        let stmts =
            emit_field_deserialise_wire("name", &wire, &ident("json"), &ident("path"), None, &ru);
        assert_eq!(
            render(stmts),
            "  if (typeof json !== \"number\") {\n    return Err({ kind: \"StructuralMismatch\", path: path, expected: \"number\", actual: typeof json });\n  }\n  if (!globalThis.Number.isSafeInteger(json)) {\n    return Err({ kind: \"StructuralMismatch\", path: path, expected: \"safe integer\", actual: String(json) });\n  }\n  if (!globalThis.Number.isFinite(json)) {\n    return Err({ kind: \"StructuralMismatch\", path: path, expected: \"finite number\", actual: String(json) });\n  }\n  const __name = json;\n"
        );
    }
}

#[cfg(test)]
mod default_lowering_tests {
    use super::*;
    use bynk_syntax::ast::{CommonsItem, Expr, RecordField};
    use std::collections::HashMap;

    /// Parses `src` as a single-file commons and returns its `types` table
    /// plus the field list of the `event`/`type` decl named `subject`, so
    /// each test can feed a real, parsed `(init, type_ref)` pair into
    /// [`field_default_value`] rather than hand-building AST nodes.
    fn parse_fields(
        src: &str,
        subject: &str,
    ) -> (HashMap<String, Arc<TypeDecl>>, Vec<RecordField>) {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("tokenize");
        let unit = bynk_syntax::parser::parse_unit(&tokens, src).expect("parse");
        let items: Vec<CommonsItem> = match unit {
            bynk_syntax::ast::SourceUnit::Context(ctx) => ctx.items,
            bynk_syntax::ast::SourceUnit::Commons(commons) => commons.items,
            bynk_syntax::ast::SourceUnit::Suite(_) | bynk_syntax::ast::SourceUnit::Adapter(_) => {
                panic!("expected a context or commons unit")
            }
        };
        let mut types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        let mut fields = None;
        for item in &items {
            match item {
                CommonsItem::Type(t) => {
                    types.insert(t.name.name.clone(), Arc::new(t.clone()));
                    if t.name.name == subject
                        && let TypeBody::Record(r) = &t.body
                    {
                        fields = Some(r.fields.clone());
                    }
                }
                CommonsItem::Event(e) => {
                    if e.name.name == subject {
                        fields = Some(e.body.fields.clone());
                    }
                    types.insert(e.name.name.clone(), Arc::new(e.as_type_decl()));
                }
                CommonsItem::Fn(_)
                | CommonsItem::Capability(_)
                | CommonsItem::Provider(_)
                | CommonsItem::Service(_)
                | CommonsItem::Agent(_)
                | CommonsItem::Actor(_)
                | CommonsItem::Messages(_)
                | CommonsItem::Asset(_)
                | CommonsItem::Embed(_)
                | CommonsItem::Const(_) => {}
            }
        }
        (
            types,
            fields.unwrap_or_else(|| panic!("no decl named `{subject}`")),
        )
    }

    /// The wire form of a default, printed: the tree [`field_default_value`] builds, rendered by
    /// [`wire_default_node`], as the text the codec splices in.
    fn lower_field_default_wire(
        init: &Expr,
        ty: &TypeRef,
        types: &HashMap<String, Arc<TypeDecl>>,
    ) -> Result<String, bynk_check::wire_default::DefaultReason> {
        field_default_value(init, ty, types)
            .map(|value| bynk_ts::print_expr(&wire_default_node(&value)))
    }

    fn default_of<'a>(fields: &'a [RecordField], name: &str) -> (&'a Expr, &'a TypeRef) {
        let f = fields
            .iter()
            .find(|f| f.name.name == name)
            .unwrap_or_else(|| panic!("no field `{name}`"));
        (
            f.init
                .as_ref()
                .unwrap_or_else(|| panic!("field `{name}` has no default")),
            &f.type_ref,
        )
    }

    #[test]
    fn base_literals_lower_to_their_raw_wire_form() {
        let src = r#"
context test

event E = {
  a: Int = 5,
  b: Int = -5,
  c: String = "hi",
  d: Bool = true,
  e: Float = 1.5,
  f: Duration = 5.minutes,
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let cases = [
            ("a", "5"),
            ("b", "-5"),
            ("c", "\"hi\""),
            ("d", "true"),
            ("e", "1.5"),
            ("f", "300000"),
        ];
        for (name, expected) in cases {
            let (init, ty) = default_of(&fields, name);
            assert_eq!(
                lower_field_default_wire(init, ty, &types),
                Ok(expected.to_string()),
                "field `{name}`"
            );
        }
    }

    #[test]
    fn sum_variant_defaults_lower_to_a_bare_kind_object_not_a_qualified_reference() {
        let src = r#"
context test

type Region = enum { Domestic, International }

event E = {
  bare: Region = Domestic,
  qualified: Region = Region.Domestic,
}
"#;
        let (types, fields) = parse_fields(src, "E");
        for name in ["bare", "qualified"] {
            let (init, ty) = default_of(&fields, name);
            let got = lower_field_default_wire(init, ty, &types).unwrap();
            assert_eq!(got, "{ kind: \"Domestic\" }", "field `{name}`");
            assert!(
                !got.contains('.'),
                "field `{name}` must not contain a qualified reference: {got}"
            );
        }
    }

    #[test]
    fn payload_variant_default_recurses_into_declared_field_types() {
        let src = r#"
context test

type Outcome = | Won(prize: Int) | Lost

event E = {
  o: Outcome = Won(100),
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let (init, ty) = default_of(&fields, "o");
        assert_eq!(
            lower_field_default_wire(init, ty, &types),
            Ok("{ kind: \"Won\", prize: 100 }".to_string())
        );
    }

    #[test]
    fn qualified_payload_variant_call_lowers_the_same_as_the_bare_call() {
        // Regression: `Outcome.Won(100)` parses to `ExprKind::MethodCall`
        // with an `Ident` receiver — a match that missed that shape silently
        // fell through to "expected a variant" for this qualified spelling.
        let src = r#"
context test

type Outcome = | Won(prize: Int) | Lost

event E = {
  o: Outcome = Outcome.Won(100),
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let (init, ty) = default_of(&fields, "o");
        assert_eq!(
            lower_field_default_wire(init, ty, &types),
            Ok("{ kind: \"Won\", prize: 100 }".to_string())
        );
    }

    #[test]
    fn opaque_unsafe_default_lowers_to_the_raw_literal() {
        // Regression: `OrderId.unsafe("x")` also parses to `MethodCall`, the
        // same shape as the qualified-variant case above.
        let src = r#"
context test

type OrderId = opaque String where MinLength(1)

event E = {
  id: OrderId = OrderId.unsafe("abc"),
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let (init, ty) = default_of(&fields, "id");
        assert_eq!(
            lower_field_default_wire(init, ty, &types),
            Ok("\"abc\"".to_string())
        );
    }

    #[test]
    fn option_and_result_defaults_use_the_wire_kind_discriminant() {
        let src = r#"
context test

event E = {
  a: Option[Int] = Some(1),
  b: Option[Int] = None,
  c: Result[Int, String] = Ok(1),
  d: Result[Int, String] = Err("nope"),
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let cases = [
            ("a", "{ kind: \"Some\", value: 1 }"),
            ("b", "{ kind: \"None\" }"),
            ("c", "{ kind: \"Ok\", value: 1 }"),
            ("d", "{ kind: \"Err\", error: \"nope\" }"),
        ];
        for (name, expected) in cases {
            let (init, ty) = default_of(&fields, name);
            assert_eq!(
                lower_field_default_wire(init, ty, &types),
                Ok(expected.to_string()),
                "field `{name}`"
            );
        }
    }

    #[test]
    fn record_literal_default_lowers_to_a_plain_object() {
        let src = r#"
context test

type Region = enum { Domestic, International }
type Meta = { region: Region, note: String }

event E = {
  m: Meta = Meta { region: Region.Domestic, note: "x" },
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let (init, ty) = default_of(&fields, "m");
        assert_eq!(
            lower_field_default_wire(init, ty, &types),
            Ok("{ region: { kind: \"Domestic\" }, note: \"x\" }".to_string())
        );
    }

    #[test]
    fn list_literal_default_recurses_per_element() {
        let src = r#"
context test

event E = {
  xs: List[Int] = [1, 2, 3],
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let (init, ty) = default_of(&fields, "xs");
        assert_eq!(
            lower_field_default_wire(init, ty, &types),
            Ok("[1, 2, 3]".to_string())
        );
    }

    #[test]
    fn mismatched_shapes_return_err_not_panic() {
        let src = r#"
context test

type Region = enum { Domestic, International }

event E = {
  a: Int = "wrong",
  b: Region = International,
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let (init, ty) = default_of(&fields, "a");
        assert!(lower_field_default_wire(init, ty, &types).is_err());
        // Sanity: a *valid* shape for the same sum still succeeds, proving
        // the harness itself is sound.
        let (init, ty) = default_of(&fields, "b");
        assert_eq!(
            lower_field_default_wire(init, ty, &types),
            Ok("{ kind: \"International\" }".to_string())
        );
    }
}
