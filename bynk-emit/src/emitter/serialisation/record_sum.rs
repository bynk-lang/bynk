//! Named records and sums (back-half paydown 2.3b, #4088): the record and sum types and their codecs,
//! with the sum-codec tests. Moved out of `serialisation.rs` byte for byte.

use std::sync::Arc;

use bynk_check::wire::{WireBody, WireField, WireRef, WireSum, WireType, wire_type};
use bynk_check::wire_default::field_default_value;
use bynk_syntax::ast::{BaseType, TypeDecl};
use bynk_ts::{
    TsBinaryOp, TsBindingName, TsDecl, TsExpr, TsLit, TsParam, TsStmt, TsSwitchCase, TsType,
};

use crate::emitter::default_value::wire_default_node;
use crate::emitter::{RuntimeUse, payload_prop};
use crate::ts_build::{
    as_expr, binary, block, call, const_, ident, if_, index, member, or_expr, ret, str_lit,
    strict_eq, strict_neq, switch_stmt, typeof_expr,
};

use super::field::{emit_field_deserialise_wire, serialise_field_expr_wire};
use super::generics::plain_json_test;
use super::refined::emit_field_refinement_checks;
use super::support::{
    DepthCodecs, boundary_result, case_, default_case, depth_for, depth_guard_decode,
    depth_guard_encode, depth_param, deserialise_params, err_structural_mismatch_top,
    index_signature_record_ty, path_field, tag_failure_class,
};
use super::{Qual, qual_provenance, qual_type};

#[cfg(test)]
use bynk_check::wire::{BaseGuard, JsonKind, WireVariant};

/// #855 (Phase 2 step 7): builds the [`WireField`] list via
/// `bynk_check::wire::wire_type` — the same declaration-order shape (field
/// name, [`WireRef`] shape, and raw `(Expr, TypeRef)` default) both the
/// codec and a future peek would derive — rather than re-walking
/// `body.fields` inline. `default_value::wire_default_node` is the
/// one place a default's wire-JSON literal is produced, called from
/// `emit_record_codec` at the same point it always was (Part 1's seam: the
/// IR carries the raw default, the emitter renders it).
///
/// #1443 (Arc E slice 5): returns [`emit_record_codec`]'s own `Vec<TsDecl>`
/// straight through — a plain passthrough, the same shape
/// [`emit_refined`]'s own early return to `emit_bytes_named_codec` already
/// established (#1441).
pub(super) fn emit_record(
    name: &str,
    decl: &TypeDecl,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    qual: &Qual,
    codecs: &DepthCodecs,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    let prov = qual_provenance(qual, name);
    let fields = match wire_type(name, decl, types, prov) {
        Some(WireType {
            body: WireBody::Record { fields },
            ..
        }) => fields,
        _ => {
            unreachable!("emit_record is only ever called for a non-generic Record declaration")
        }
    };
    // #661: a consumed record's TS value type reaches through the type-only
    // namespace (`commerce_payment.Receipt`); the codec function name stays
    // bare and local. Its field codec calls are unqualified too — they resolve
    // to the caller's own locally-generated helpers.
    let ts_type = TsType::named(qual_type(qual, name));
    emit_record_codec(name, &ts_type, &fields, types, codecs, ru)
}

/// #3975: a JSON key as an object-literal property name: bare when it is a plain identifier,
/// otherwise a quoted string, so a wire name such as `content-type` or `@odata.etag` is a valid
/// key.
fn object_key(key: &str) -> String {
    let mut chars = key.chars();
    let plain = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if plain {
        key.to_string()
    } else {
        let mut quoted = String::from("\"");
        for c in key.chars() {
            match c {
                '"' => quoted.push_str("\\\""),
                '\\' => quoted.push_str("\\\\"),
                c if c.is_control() => quoted.push_str(&format!("\\u{:04x}", c as u32)),
                c => quoted.push(c),
            }
        }
        quoted.push('"');
        quoted
    }
}

/// v0.174 (#592): the shared record codec body. `fn_suffix` is the codec name
/// suffix (`Order`, or the monomorphised `Paginated_User`); `ts_type` is the
/// TypeScript value type the codec accepts / returns (`Order`, or the erased
/// generic `Paginated<User>`). The two coincide for a non-generic record and
/// diverge for a generic-record instantiation.
///
/// #855 (Phase 2 step 7): takes `&[WireField]` — the field's shape as a
/// [`WireRef`] (rendered via [`serialise_field_expr_wire`] /
/// [`emit_field_deserialise_wire`], no re-derivation from a raw `TypeRef`)
/// and its default as a raw `(Expr, TypeRef)`, built into its wire-JSON
/// literal right here via `field_default_value` and `wire_default_node` — the same point it was
/// always called from, per Part 1's seam (`bynk-check` carries the boundary
/// fact, `bynk-emit` renders it). Only `deserialise_<fn_suffix>` consults
/// a default; `serialise_<fn_suffix>` is untouched (Decision B, #972). A
/// generic-record instantiation's default arrives typed at the
/// instantiation (#2607, Decision I).
///
/// #1443 (Arc E slice 5): returns the `[serialise, deserialise]` pair as
/// real `bynk_ts::TsDecl` nodes — the same `Vec<TsDecl>` shape
/// `emit_bytes_named_codec`/`emit_refined` already established (#1441), for
/// the same reason (this function's two real callers, `emit_record` above
/// and `emit_generic_helpers_qualified`'s own `RecordInst` arm — as of Arc F
/// slice 1, #1451, both `extend` a `Vec<TsDecl>` directly rather than
/// printing; [`decls_as_stmts`] does the one shared boundary-collect, once,
/// at each caller's own caller). Two internal calls stay unprinted:
/// `serialise_field_expr_wire` (`-> TsExpr` since #1435) and
/// `emit_field_deserialise_wire` (`-> Vec<TsStmt>` since #1439) are both
/// already tree-native, and this function itself is becoming tree-native
/// too — consuming their return values directly (no
/// `bynk_ts::print_expr`/`splice_stmts` boundary-print) is correct, not a
/// gap; that print/splice treatment is only for a caller that is itself
/// still `String`-based (`emit_generic_helpers_qualified`'s own
/// `ResultInst`/`OptionInst`/etc. arms, still out of this slice's scope).
pub(super) fn emit_record_codec(
    fn_suffix: &str,
    ts_type: &TsType,
    fields: &[WireField],
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    codecs: &DepthCodecs,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    // #2656: a record on a type cycle threads the nesting depth.
    let depth = depth_for(codecs, fn_suffix, true);
    let mut serialise_params = vec![TsParam::new("value".to_string(), Some(ts_type.clone()))];
    let mut serialise_body = Vec::new();
    if depth.is_some() {
        serialise_params.push(depth_param());
        serialise_body.push(depth_guard_encode());
    }
    serialise_body.push(ret(TsExpr::multiline_object(
        fields
            .iter()
            .map(|field| {
                (
                    object_key(&field.key),
                    serialise_field_expr_wire(
                        &field.shape,
                        &ident(format!("value.{}", field.name)),
                        "",
                        depth,
                        ru,
                    ),
                )
            })
            .collect(),
    )));
    let serialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__serialise_{fn_suffix}"),
        generics: Vec::new(),
        params: serialise_params,
        return_type: Some(TsType::named("__JsonValue")),
        body: serialise_body,
        is_async: false,
        inline: false,
    }));

    // `typeof json !== "object" || json === null || Array.isArray(json)` —
    // a genuine 3-term `Or` chain (see `or_expr`'s own doc for why it
    // prints flat with no parens).
    let mut body: Vec<TsStmt> = depth.iter().map(|_| depth_guard_decode()).collect();
    body.extend([
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
    ]);
    for field in fields {
        // Events slice 3a (#972): a defaulted field is read through a
        // pre-validated `__d_<field>` binding instead of the raw
        // `obj["<field>"]` access — `"fname" in obj`, not `!== undefined`,
        // is the only test that distinguishes a genuinely absent wire key
        // from one present with an explicit value (Decision D; this is also
        // what makes `Option[T]`'s two absences fall out with no
        // special-casing — a wire `{"kind":"None"}` already passed the `in`
        // test, so it flows through to a real `None`, untouched by the
        // default; so does an explicit `null`, since #1887). Everything
        // downstream (`emit_field_deserialise_wire`) is unchanged either
        // way. An undefaulted field reads `obj["<field>"]` with no presence
        // check, so an absent key reaches its codec as `undefined`, which
        // the lenient `Option` decoder (#1887) reads as `None` and every
        // other codec rejects.
        let default = field
            .default
            .as_ref()
            .and_then(|(e, t)| field_default_value(e, t, types).ok());
        let fname = &field.name;
        // #3975: the JSON key the field is read from; `fname` stays the Bynk name, which names
        // the decoder's locals.
        let wkey = &field.key;
        // #3793: `name: T replaces old` reads the old key when the new one is absent. `"new" in
        // obj` is the test, as for a default, so a key present with an explicit value (or an
        // explicit `null`) is never replaced; the new key wins when both arrive.
        let present = |key: &str| binary(TsBinaryOp::In, str_lit(key.to_string()), ident("obj"));
        let keyed = |key: &str| index(ident("obj"), str_lit(key.to_string()));
        let read_old = field
            .renamed_from
            .iter()
            .rev()
            .fold(None, |rest: Option<TsExpr>, old| {
                Some(match rest {
                    // `"old" in obj ? obj["old"] : <rest>`, the rest being the older names.
                    Some(rest) => TsExpr::Conditional {
                        test: Box::new(present(old)),
                        consequent: Box::new(keyed(old)),
                        alternate: Box::new(rest),
                    },
                    None => keyed(old),
                })
            });
        let access = if let Some(d) = &default {
            // `"<field>" in obj ? obj["<field>"] : <default>`: `d` is the default's tree
            // (`field_default_value`), rendered in its wire form (`wire_default_node`). As a
            // `Conditional`'s own `alternate` it sits in an `AssignmentExpression` grammar
            // position, which (per `bynk-ts/src/printer/expr.rs`'s own `Conditional` render arm)
            // never adds parens around either branch, whatever the branch's shape.
            let d_name = format!("__d_{fname}");
            // With a rename: the older names when present, otherwise the default.
            let fallback =
                field
                    .renamed_from
                    .iter()
                    .rev()
                    .fold(wire_default_node(d), |rest, old| TsExpr::Conditional {
                        test: Box::new(present(old)),
                        consequent: Box::new(keyed(old)),
                        alternate: Box::new(rest),
                    });
            body.push(TsStmt::const_stmt(
                TsBindingName::Ident(d_name.clone()),
                Some(TsType::named("__JsonValue")),
                TsExpr::Conditional {
                    test: Box::new(present(wkey)),
                    consequent: Box::new(keyed(wkey)),
                    alternate: Box::new(fallback),
                },
                None,
            ));
            ident(d_name)
        } else if let Some(old) = read_old {
            // Without a default, an absent key must still reach the codec as `undefined`, so the
            // lenient `Option` decoder reads `None` and every other codec rejects: the old key
            // is read only when it is present.
            TsExpr::Conditional {
                test: Box::new(present(wkey)),
                consequent: Box::new(keyed(wkey)),
                alternate: Box::new(old),
            }
        } else {
            keyed(wkey)
        };
        let sub_path = path_field(&field.path_segment);
        // #1439 (Arc E slice 3): `emit_field_deserialise_wire` returns a
        // real `Vec<bynk_ts::TsStmt>` — extended straight into this
        // function's own body, no `splice_stmts` (that boundary-print
        // helper is only for a caller that still prints into a `String`;
        // this function's own body is a real `Vec<TsStmt>` now).
        body.extend(emit_field_deserialise_wire(
            fname,
            &field.shape,
            &access,
            &sub_path,
            depth,
            ru,
        ));
        // #1969: an inline field refinement (`qty: Int where Positive`) is
        // checked on the decoded value, after the field's own codec, and fails
        // as a `RefinementViolation` at the field's path — the same envelope a
        // named refined type's decode gives.
        body.extend(emit_field_refinement_checks(
            fname,
            &field.predicates,
            &sub_path,
            match &field.shape {
                WireRef::Base { base, .. } => Some(*base),
                WireRef::Decimal => Some(BaseType::Decimal),
                WireRef::Bytes
                | WireRef::Named { .. }
                | WireRef::Inst { .. }
                | WireRef::Unit
                | WireRef::Unchecked { .. } => None,
            },
            ru,
        ));
    }
    // Review of #1444: a *zero-field* record — `type X = {}`, parse-legal
    // Bynk that never reaches a wire boundary in the current corpus, so no
    // fixture exercises this — hits `TsExpr::object`'s own single-line
    // collapse (`{}`), where the pre-conversion `write!(out, "  return Ok({{
    // ")` + empty `parts.join(", ")` + `" }} as {ts_type});"` always built
    // `Ok({  } as T)` (a literal double space, the same "`{  }`, not the
    // tight `{}` shortcut" quirk `workers.rs`'s/`project.rs`'s own
    // `deps`/`{ns}Deps` objects already carry). TS-parse-identical either
    // way — documented rather than silently changed, the same "name a real,
    // unreachable-today formatting hazard rather than fix it inline"
    // precedent P7.9's own `Array`-over-`Union` hazard already set (that one
    // since closed, #1898).
    body.push(ret(call(
        ident("Ok"),
        vec![as_expr(
            TsExpr::object(
                fields
                    .iter()
                    .map(|field| (field.name.clone(), ident(format!("__{}", field.name))))
                    .collect(),
            ),
            ts_type.clone(),
        )],
    )));

    let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__deserialise_{fn_suffix}"),
        generics: Vec::new(),
        params: deserialise_params(depth),
        return_type: Some(boundary_result(ts_type.clone())),
        body,
        is_async: false,
        inline: false,
    }));

    vec![serialise, deserialise]
}

/// #855 (Phase 2 step 8): builds the [`WireSum`] via
/// `bynk_check::wire::wire_type` — the same declaration-order variant/payload
/// shape a peek would derive — instead of re-walking `body.variants` inline.
///
/// #1445 (Arc E slice 6): returns [`emit_sum_codec`]'s own `Vec<TsDecl>`
/// straight through — a plain passthrough, the same shape [`emit_record`]'s
/// own early return to `emit_record_codec` already established (#1443).
pub(super) fn emit_sum(
    name: &str,
    decl: &TypeDecl,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    qual: &Qual,
    codecs: &DepthCodecs,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    // #661: a consumed sum's TS value type is namespace-qualified; the codec
    // function name and its per-variant codec calls stay bare and local. #593:
    // the codec body is the shared `emit_sum_codec` (also reused, unqualified,
    // for a generic-sum instantiation), so the qualified value type is threaded
    // in as its `ts_type`.
    let prov = qual_provenance(qual, name);
    let sum = match wire_type(name, decl, types, prov) {
        Some(WireType {
            body: WireBody::Sum(s),
            ..
        }) => s,
        _ => unreachable!("emit_sum is only ever called for a non-generic Sum declaration"),
    };
    let ty = TsType::named(qual_type(qual, name));
    emit_sum_codec(name, &ty, &sum, codecs, ru)
}

/// The serialise/deserialise pair for a sum type, over an already-resolved
/// [`WireSum`]. `fn_suffix` names the emitted functions (`Opt` / `Opt_Int`),
/// `ts_type` is their value type (`Opt` / `Opt<number>` / a namespace-qualified
/// `shop.Opt`). #593: a generic-sum instantiation reuses this with substituted
/// payload types, exactly as a generic record reuses [`emit_record_codec`].
///
/// #855 (Phase 2 step 8): the wire discriminant (`kind`) and in-memory
/// discriminant (`tag`) are read off `sum.wire_discriminant` /
/// `sum.memory_discriminant` rather than hard-coded string literals — the
/// same two values as before (`wire.rs`'s module doc: `memory_discriminant`
/// is the softest part of the seam, carried beside `wire_discriminant`
/// because this codec's whole job is translating between them), so this is
/// purely reading the fact from the IR, not a behaviour change.
///
/// #1445 (Arc E slice 6): returns the `[serialise, deserialise]` pair as real
/// `bynk_ts::TsDecl` nodes, the same `Vec<TsDecl>` shape #1441/#1443 already
/// established — this function's two real callers ([`emit_sum`] above and
/// `emit_generic_helpers_qualified`'s own `SumInst` arm) — as of Arc F slice
/// 1 (#1451), both `extend` a `Vec<TsDecl>` directly rather than printing;
/// [`decls_as_stmts`] does the one shared boundary-collect, once, at each
/// caller's own caller.
///
/// **The `(value as any).<field>` investigation (#1423's own residual,
/// re-examined per this issue's own explicit instruction):** narrowed, not
/// kept. Empirically confirmed under `tsc --strict` (not assumed): once
/// `value.{tag}` is the `switch` discriminant, TypeScript's own control-flow
/// analysis narrows `value` to the exact variant member inside each `case`
/// block — the same narrowing `if (value.tag === "...")` gets, and `switch`
/// on a discriminant property is documented TS behaviour, not a guess. This
/// is a genuinely different shape from P7.2's own failed attempt (the
/// comment this replaces): that attempt cast `value` itself to `Record<
/// string, unknown>` BEFORE any narrowing, which throws away the literal
/// discriminant entirely and hands `serialise_field_expr_wire`'s per-shape
/// helpers (`(v: number) => ...` for `Float`, etc.) a field typed `unknown`
/// no narrowing could ever recover. Reading the field through the
/// switch-narrowed `value` directly, with NO cast at all, needs no such
/// recovery — confirmed against both a plain-`string` field and a `Float`
/// field (`Number.isFinite` guard intact) under `tsc --strict --lib es2020`,
/// and again against a *generic*-sum instantiation (`ApiResult<User>`,
/// `value.value` narrowed to the concrete `User` argument, not just the
/// erased type parameter). Net effect: `ts_any` drops by exactly this one
/// Rust-source site (31 → 30) — the opposite of slice 3's own "don't claim
/// a false win" trap, a real, verified win. The fixture corpus's own
/// `(value as any).<field>` bytes (`212_json_codec`,
/// `407_workers_generic_sum_boundary`, et al.) change to a bare
/// `value.<field>` as a direct, intended consequence — reblessed, not a
/// zero-diff violation (this one narrow site is the explicitly-authorised
/// exception to this slice's own zero-diff mandate, not an accidental
/// formatting drift).
pub(super) fn emit_sum_codec(
    fn_suffix: &str,
    ts_type: &TsType,
    sum: &WireSum,
    codecs: &DepthCodecs,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    // #3975: the JSON key the variant travels under (`@discriminator`, else `kind`). The decoder's
    // local that holds its value keeps the name `kind` whatever the key, since the key may not be
    // a TypeScript identifier.
    let dkey = sum.wire_discriminant.as_str();
    let kind = "kind";
    // #2656: a sum on a type cycle threads the nesting depth.
    let depth = depth_for(codecs, fn_suffix, true);
    let tag = sum.memory_discriminant;
    // #3974: a sum with no payload variant also reads as a bare string.
    let enum_like = !sum.variants.is_empty() && sum.variants.iter().all(|v| v.payload.is_empty());

    let serialise_cases: Vec<TsSwitchCase> = sum
        .variants
        .iter()
        .map(|variant| {
            let vname = &variant.name;
            // #3975: the value the discriminator takes on the wire.
            let wname = &variant.wire_name;
            if variant.payload.is_empty() {
                let tagged = TsExpr::object(vec![(object_key(dkey), str_lit(wname.clone()))]);
                // #3974: in plain mode a payload-less sum is its variant's name.
                let written = if enum_like && ru.plain_json() {
                    TsExpr::Conditional {
                        test: Box::new(plain_json_test()),
                        consequent: Box::new(str_lit(wname.clone())),
                        alternate: Box::new(tagged),
                    }
                } else {
                    tagged
                };
                case_(str_lit(vname.clone()), vec![ret(written)], false)
            } else {
                let mut entries = vec![(object_key(dkey), str_lit(wname.clone()))];
                for field in &variant.payload {
                    // Narrowed (see this function's own doc): `value` is
                    // already the exact variant member inside this `case`
                    // block (TypeScript's own discriminated-union narrowing
                    // through the `switch (value.{tag})` above), so the
                    // field reads through it directly — no `as any`, no
                    // recovery cast.
                    entries.push((
                        object_key(&field.key),
                        serialise_field_expr_wire(
                            &field.shape,
                            &ident(format!("value.{}", payload_prop(&field.name))),
                            "",
                            depth,
                            ru,
                        ),
                    ));
                }
                case_(
                    str_lit(vname.clone()),
                    vec![ret(TsExpr::object(entries))],
                    true,
                )
            }
        })
        .collect();
    let mut serialise_params = vec![TsParam::new("value".to_string(), Some(ts_type.clone()))];
    let mut serialise_body = Vec::new();
    if depth.is_some() {
        serialise_params.push(depth_param());
        serialise_body.push(depth_guard_encode());
    }
    serialise_body.push(switch_stmt(member(ident("value"), tag), serialise_cases));
    let serialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__serialise_{fn_suffix}"),
        generics: Vec::new(),
        params: serialise_params,
        return_type: Some(TsType::named("__JsonValue")),
        body: serialise_body,
        is_async: false,
        inline: false,
    }));

    // The top-level structural guard + `obj` cast — the same shape
    // `emit_record_codec`'s own deserialise body opens with (#1443), kept
    // as a second, file-local copy rather than shared (this file's own
    // "own builder set" convention, `index_signature_record_ty`'s own doc).
    let mut deserialise_body: Vec<TsStmt> = depth.iter().map(|_| depth_guard_decode()).collect();
    if enum_like {
        deserialise_body.push(bare_string_variant(sum, ts_type, tag));
    }
    deserialise_body.extend([
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
        const_(
            kind.to_string(),
            index(ident("obj"), str_lit(dkey.to_string())),
        ),
    ]);
    let mut deserialise_cases: Vec<TsSwitchCase> = sum
        .variants
        .iter()
        .map(|variant| {
            let vname = &variant.name;
            // #3975: the case matches the wire name; the value's tag is the Bynk name.
            let wname = &variant.wire_name;
            if variant.payload.is_empty() {
                case_(
                    str_lit(wname.clone()),
                    vec![ret(call(
                        ident("Ok"),
                        vec![as_expr(
                            TsExpr::object(vec![(tag.to_string(), str_lit(vname.clone()))]),
                            ts_type.clone(),
                        )],
                    ))],
                    false,
                )
            } else {
                let mut stmts = Vec::new();
                for field in &variant.payload {
                    let access = index(ident("obj"), str_lit(field.key.clone()));
                    let sub_path = path_field(&field.path_segment);
                    stmts.extend(emit_field_deserialise_wire(
                        &field.name,
                        &field.shape,
                        &access,
                        &sub_path,
                        depth,
                        ru,
                    ));
                }
                let mut entries = vec![(tag.to_string(), str_lit(vname.clone()))];
                for field in &variant.payload {
                    entries.push((
                        payload_prop(&field.name),
                        ident(format!("__{}", field.name)),
                    ));
                }
                stmts.push(ret(call(
                    ident("Ok"),
                    vec![as_expr(TsExpr::object(entries), ts_type.clone())],
                )));
                case_(str_lit(wname.clone()), stmts, true)
            }
        })
        .collect();
    deserialise_cases.push(default_case(vec![ret(err_structural_mismatch_top(
        "sum variant kind",
        tag_failure_class(&ident(kind)),
    ))]));
    deserialise_body.push(switch_stmt(ident(kind), deserialise_cases));

    let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__deserialise_{fn_suffix}"),
        generics: Vec::new(),
        params: deserialise_params(depth),
        return_type: Some(boundary_result(ts_type.clone())),
        body: deserialise_body,
        is_async: false,
        inline: false,
    }));

    vec![serialise, deserialise]
}

/// #3974: `if (typeof json === "string") { switch (json) { case "A": return Ok({ tag: "A" }); … } }`,
/// the bare-string form of a payload-less sum, tried before the object form. A string that
/// names no variant is the same refusal the object form gives an unknown `kind`.
fn bare_string_variant(sum: &WireSum, ts_type: &TsType, tag: &str) -> TsStmt {
    let mut cases: Vec<TsSwitchCase> = sum
        .variants
        .iter()
        .map(|variant| {
            case_(
                str_lit(variant.wire_name.clone()),
                vec![ret(call(
                    ident("Ok"),
                    vec![as_expr(
                        TsExpr::object(vec![(tag.to_string(), str_lit(variant.name.clone()))]),
                        ts_type.clone(),
                    )],
                ))],
                false,
            )
        })
        .collect();
    cases.push(default_case(vec![ret(err_structural_mismatch_top(
        "sum variant kind",
        tag_failure_class(&ident("json")),
    ))]));
    if_(
        strict_eq(typeof_expr(ident("json")), str_lit("string")),
        block(vec![switch_stmt(ident("json"), cases)]),
    )
}

#[cfg(test)]
mod emit_sum_codec_tests {
    use super::*;
    use bynk_check::wire::Expected;

    /// #3975: a wire name is a bare property name when it is an identifier, a quoted string otherwise.
    #[test]
    fn a_wire_key_is_quoted_unless_it_is_an_identifier() {
        assert_eq!(object_key("amount"), "amount");
        assert_eq!(object_key("_id$"), "_id$");
        assert_eq!(object_key("content-type"), "\"content-type\"");
        assert_eq!(object_key("@odata.etag"), "\"@odata.etag\"");
        assert_eq!(object_key("1st"), "\"1st\"");
        assert_eq!(object_key("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(object_key(""), "\"\"");
    }

    fn render_decls(decls: Vec<TsDecl>) -> String {
        decls
            .into_iter()
            .map(|d| bynk_ts::print_stmt(&TsStmt::decl(d, None), 0))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Direct unit coverage for the one real shape no fixture in the current
    /// corpus reaches (confirmed by grep — a Python scan over every emitted
    /// `.ts` file in the tree for a `case "<variant>": { return { kind:
    /// "...", f1: ..., f2: ..., ... }` shape with 2+ payload fields found
    /// zero matches — not assumed): a payload-carrying variant with TWO OR
    /// MORE fields. The one real multi-field sum in the fixture corpus
    /// (`bynk-tests/tests/fixtures/positive/474_or_patterns`'s own
    /// `BookingState`) never crosses the wire boundary, so `serialise_
    /// BookingState`/`deserialise_BookingState` are never generated; every
    /// sum type that DOES cross the boundary (`Status`, `MessageArg`,
    /// `ApiResult<T>`, ...) happens to carry exactly one field per
    /// payload-carrying variant. This test also re-pins, directly rather
    /// than only end-to-end: the mixed payload-free/payload-carrying
    /// `case_braced` shape (`212_json_codec`'s own real fixture, `Status`),
    /// and the narrowed (no `as any`) field access this slice's own
    /// investigation established (this function's own doc).
    #[test]
    fn multi_field_variant_and_payload_free_variant_share_one_switch() {
        let ru = RuntimeUse::default();
        let sum = WireSum {
            wire_discriminant: "kind".to_string(),
            memory_discriminant: "tag",
            variants: vec![
                WireVariant {
                    name: "Pending".to_string(),
                    wire_name: "Pending".to_string(),
                    payload: vec![],
                },
                WireVariant {
                    name: "Shipped".to_string(),
                    wire_name: "Shipped".to_string(),
                    payload: vec![
                        WireField {
                            name: "tracking".to_string(),
                            key: "tracking".to_string(),
                            shape: WireRef::Base {
                                base: BaseType::String,
                                json: JsonKind::String,
                                guards: vec![],
                                expected: Expected::Json(JsonKind::String),
                            },
                            path_segment: "tracking".to_string(),
                            default: None,
                            predicates: Vec::new(),
                            renamed_from: Vec::new(),
                        },
                        WireField {
                            name: "weight".to_string(),
                            key: "weight".to_string(),
                            shape: WireRef::Base {
                                base: BaseType::Int,
                                json: JsonKind::Number,
                                guards: vec![BaseGuard::Integral],
                                expected: Expected::Json(JsonKind::Number),
                            },
                            path_segment: "weight".to_string(),
                            default: None,
                            predicates: Vec::new(),
                            renamed_from: Vec::new(),
                        },
                    ],
                },
            ],
        };
        let decls = emit_sum_codec(
            "Parcel",
            &TsType::named("Parcel"),
            &sum,
            &DepthCodecs::new(),
            &ru,
        );
        assert_eq!(
            render_decls(decls),
            "export function __serialise_Parcel(value: Parcel): __JsonValue {\n  switch (value.tag) {\n    case \"Pending\":\n      return { kind: \"Pending\" };\n    case \"Shipped\": {\n      return { kind: \"Shipped\", tracking: value.tracking as __JsonValue, weight: ((v: number) => { if (!globalThis.Number.isSafeInteger(v)) throw new globalThis.Error(\"Int outside the safe-integer range at boundary\"); return v as __JsonValue; })(value.weight) };\n    }\n  }\n}\n\nexport function __deserialise_Parcel(json: __JsonValue, path: string = \"$\"): Result<Parcel, __BoundaryError> {\n  if (typeof json !== \"object\" || json === null || globalThis.Array.isArray(json)) {\n    return Err({ kind: \"StructuralMismatch\", path, expected: \"object\", actual: typeof json });\n  }\n  const obj = json as { [k: string]: __JsonValue };\n  const kind = obj[\"kind\"];\n  switch (kind) {\n    case \"Pending\":\n      return Ok({ tag: \"Pending\" } as Parcel);\n    case \"Shipped\": {\n      if (typeof obj[\"tracking\"] !== \"string\") {\n        return Err({ kind: \"StructuralMismatch\", path: `${path}.tracking`, expected: \"string\", actual: typeof obj[\"tracking\"] });\n      }\n      const __tracking = obj[\"tracking\"];\n      if (typeof obj[\"weight\"] !== \"number\") {\n        return Err({ kind: \"StructuralMismatch\", path: `${path}.weight`, expected: \"number\", actual: typeof obj[\"weight\"] });\n      }\n      if (!globalThis.Number.isSafeInteger(obj[\"weight\"])) {\n        return Err({ kind: \"StructuralMismatch\", path: `${path}.weight`, expected: \"safe integer\", actual: globalThis.Number.isInteger(obj[\"weight\"]) ? \"integer outside the safe range\" : \"non-integer number\" });\n      }\n      const __weight = obj[\"weight\"];\n      return Ok({ tag: \"Shipped\", tracking: __tracking, weight: __weight } as Parcel);\n    }\n    default:\n      return Err({ kind: \"StructuralMismatch\", path, expected: \"sum variant kind\", actual: typeof kind === \"string\" ? \"unrecognised variant\" : typeof kind });\n  }\n}\n"
        );
    }
}
