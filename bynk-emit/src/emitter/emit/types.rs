//! Type declarations: `type` aliases, refined types and their checks, records and sum types
//! (`emit_type` is the entry point), with the tests that pin their emitted shape.

use super::emit_attached_methods;
use crate::emit_model::EmitProjectCtx;
use crate::emitter::ts_types::{ts_base, ts_ty_to_ts_type};
use crate::emitter::{RuntimeUse, UnitFacts, payload_prop, ts_ident};
use crate::ts_build::{call, ident};
use bynk_check::checker::{TyId, TypedCommons};
use bynk_model::TypeShape;
use bynk_syntax::ast::{BaseType, PredKind, Refinement, TypeDecl, TypeParam};

/// P6.x (#1188): reads `shape` — `t`'s already-lowered `bynk-emit::ir::TypeShape`
/// — instead of walking `t.body` (the AST's own `TypeBody`) directly. (This file
/// does import `bynk_syntax::ast`, for `BaseType` and the like, so the
/// `ast_importers` probe in `xtask/src/greenfield_status.rs` counts it, once,
/// under `emitter/emit.rs` in `AST_IMPORTER_ORIGINS`.)
/// `t` itself is still read for what `TypeShape` deliberately doesn't carry:
/// `.documentation`/`.name`/`.type_params` (header/namespace-object emission,
/// including this type's own attached methods via `emit_attached_methods`,
/// which stays on the untouched, unconverted body-lowering path — see that
/// function's own doc comment).
/// #1478: returns real [`bynk_ts::TsStmt`]s (was `out: &mut String`) — the
/// leading doc comment is now built inline (matching every other converted
/// caller's own way of replacing an `emit_doc_block` call) since
/// `emit_doc_block` itself stays `out`-writing (14 other callers, out of
/// scope here).
pub(crate) fn emit_type(
    t: &TypeDecl,
    shape: &TypeShape,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    // For contexts, the per-type brand string is qualified by the context's
    // name (so two contexts' locally-declared `Order` types have distinct
    // brands at the TS level).
    let brand_prefix = ctx
        .owning_context
        .as_deref()
        .map(|c| format!("{c}."))
        .unwrap_or_default();
    emit_type_in(
        t,
        shape,
        commons,
        &brand_prefix,
        &ctx.runtime_use,
        UnitFacts::of(ctx),
    )
}

/// [`emit_type`] with the brand prefix and runtime-use accumulator supplied
/// directly (#3066: a suite type is emitted into a test module, which has no
/// `EmitProjectCtx` of its own).
pub(crate) fn emit_type_in(
    t: &TypeDecl,
    shape: &TypeShape,
    commons: &TypedCommons,
    brand_prefix: &str,
    runtime_use: &RuntimeUse,
    unit: UnitFacts<'_>,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    let has_doc = t.documentation.is_some();
    if let Some(doc) = t.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let mut shape_stmts = match shape {
        // `Opaque` and `Refined` lower almost identically: a branded base type
        // alias plus an `of` constructor object. The one difference (ADR 0182)
        // is `unsafe`: opaque exposes it (its defining commons needs a
        // representation-level constructor), a refined/alias type does not —
        // `TypeShape::Refined::opaque` already carries that distinction
        // (P6.6's own Decision A unified the AST's two `TypeBody` variants
        // into this one `TypeShape` variant).
        TypeShape::Refined {
            base,
            refinement,
            opaque,
        } => emit_refined_type(
            t,
            RefinedShape {
                base: *base,
                refinement: refinement.as_ref(),
                is_opaque: *opaque,
            },
            commons,
            brand_prefix,
            runtime_use,
            unit,
        ),
        TypeShape::Record { fields } => emit_record_type(t, fields, commons, runtime_use, unit),
        // `embeds` is not read here — today's emitter has no reader for
        // `SumBody::embeds` anywhere (confirmed by grep; `embeds` is checker-
        // enforced construction-time only), so `TypeShape::Sum::embeds` stays
        // unread by this slice too, not a gap introduced by it.
        TypeShape::Sum { variants, .. } => emit_sum_type(t, variants, commons, runtime_use, unit),
    };
    // #1486: a leading doc comment sits directly above the declaration it
    // documents, no blank between them — unlike `Comment`, `DocComment` is
    // never exempted from the printer's automatic top-level spacing policy
    // on its own, so the statement right after it needs `no_blank_before`
    // explicitly (the same pattern `emit_free_fn`/`emit_capability` already
    // use for their own leading doc comments).
    if has_doc && let Some(first) = shape_stmts.first_mut() {
        first.no_blank_before = true;
    }
    stmts.extend(shape_stmts);
    stmts
}

/// The parts of a refined-or-opaque `TypeBody` that its lowering reads.
///
/// Honest about what this is: `emit_refined_type` reached eight arguments when it
/// gained `runtime_use`, and grouping three of them keeps it under
/// `clippy::too_many_arguments` without another `#[allow]`. The three do cohere —
/// all come out of the same `t.body` the caller already matched on — but the
/// function still takes six parameters, so this is a lint fix that reads well,
/// not a decomposition.
pub(in crate::emitter) struct RefinedShape<'a> {
    base: BaseType,
    refinement: Option<&'a Refinement>,
    /// ADR 0182: opaque and refined lower identically apart from `unsafe`, which
    /// only opaque exposes (its defining commons needs a representation-level
    /// constructor).
    is_opaque: bool,
}

/// #1339 (R7.1): `export type {name} = {base} & { readonly __brand: "..."
/// };` is a real [`bynk_ts::TsDecl::TypeAlias`] over a new
/// [`bynk_ts::TsType::Intersection`] (this file's own first real need for
/// it — `TsType` had `Named`/`Array`/`Object`/`Fn`/`Union`, nothing for
/// `A & B`); `export const {name} = { of(...) {...}, unsafe?(...) {...},
/// ...attachedMethods }` is a real [`bynk_ts::TsDecl::ConstDecl`] whose
/// `init` is one [`bynk_ts::TsExpr::multiline_object_entries`] — `of`'s own
/// signature/return-statement are real nodes; `emit_refined_checks`'s own
/// output (still `out: &mut String`, unaffected by this slice, the P7.9/
/// step-1 pattern applied one level down) is captured and carried as one
/// [`bynk_ts::TsStmtKind::Raw`] statement — the same carrier #1337 added for
/// `lower.rs`'s own permanently-excluded body text, reused here for a
/// different, narrower reason: this is legitimately real-node-shaped
/// content (every guard is already built and printed via real
/// `bynk_ts::TsStmt`/`print_stmt` calls inside `emit_refined_checks`
/// itself), just still `String`-typed at its own call boundary because that
/// function's own signature wasn't part of this slice's scope. `emit_
/// attached_methods`'s own real `Vec<TsObjectEntry>` (#1337) is appended
/// directly into the SAME entries list — no more per-entry `print_object_
/// entry` splice loop for this function, now that it owns the whole
/// object's own construction. This function's own exact signature is
/// unchanged, the same P7.9/step-1 pattern.
///
/// #1486: the two explicit `TsStmt::blank(None)` pushes this function used
/// to carry (after `type_alias`, after `const_decl`) are gone — `emit_
/// project`'s own top-level assembly now prints via `bynk_ts::print`
/// itself, whose own top-level loop already inserts exactly one blank line
/// between adjacent real statements; keeping both would have doubled every
/// blank line this function's own two-statement output produces.
fn emit_refined_type(
    t: &TypeDecl,
    shape: RefinedShape<'_>,
    commons: &TypedCommons,
    brand_prefix: &str,
    runtime_use: &RuntimeUse,
    unit: UnitFacts<'_>,
) -> Vec<bynk_ts::TsStmt> {
    let RefinedShape {
        base,
        refinement,
        is_opaque,
    } = shape;
    let name = t.name.name.clone();
    let base_ty = bynk_ts::TsType::named(ts_base(base));

    let brand_ty = bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::readonly_prop(
        "__brand",
        bynk_ts::TsType::named(format!("\"{brand_prefix}{name}\"")),
    )]);
    let type_alias = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::TypeAlias {
            name: name.clone(),
            type_params: Vec::new(),
            ty: bynk_ts::TsType::intersection(vec![base_ty.clone(), brand_ty]),
        })),
        None,
    );
    let mut stmts = vec![type_alias];

    let cast_to_name = bynk_ts::TsExpr::As {
        expr: Box::new(bynk_ts::TsExpr::Ident("value".to_string())),
        ty: bynk_ts::TsType::named(name.clone()),
    };
    let total = bynk_syntax::ast::of_is_total(base, refinement, is_opaque);
    let of_body = refined_of_body(
        t,
        &name,
        shape_parts(base, refinement, is_opaque),
        runtime_use,
    );
    let mut entries: Vec<bynk_ts::TsObjectEntry> = vec![bynk_ts::TsObjectEntry::Method {
        name: "of".to_string(),
        is_async: false,
        generics: Vec::new(),
        params: vec![bynk_ts::TsParam::new(
            "value".to_string(),
            Some(base_ty.clone()),
        )],
        return_type: Some(if total {
            bynk_ts::TsType::named(name.clone())
        } else {
            bynk_ts::TsType::named_with_args(
                "Result",
                vec![
                    bynk_ts::TsType::named(name.clone()),
                    bynk_ts::TsType::named("ValidationError"),
                ],
            )
        }),
        doc: None,
        inline: false,
        body: of_body,
        span: None,
    }];
    // ADR 0182: only opaque types expose a public `unsafe` constructor. A refined
    // or alias type omits it — host code cannot bypass the predicate — and its
    // admitted/generated values brand via an inline `as` cast instead (see
    // `unchecked_construct`).
    if is_opaque {
        entries.push(bynk_ts::TsObjectEntry::Method {
            name: "unsafe".to_string(),
            is_async: false,
            generics: Vec::new(),
            params: vec![bynk_ts::TsParam::new("value".to_string(), Some(base_ty))],
            return_type: Some(bynk_ts::TsType::named(name.clone())),
            doc: None,
            inline: false,
            body: vec![bynk_ts::TsStmt::return_stmt(Some(cast_to_name), None)],
            span: None,
        });
    }
    entries.extend(emit_attached_methods(
        &t.name.name,
        &t.type_params,
        commons,
        runtime_use,
        unit,
    ));
    let const_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name,
            ty: None,
            init: bynk_ts::TsExpr::multiline_object_entries(entries),
        })),
        None,
    );
    stmts.push(const_decl);
    stmts
}

/// The statements of a refined or opaque type's `of(value)`: its guards, then the
/// branded return (`Ok(value as T)`, or `value as T` when the type has no
/// predicate to check, #3946). Shared by the owner's companion and by a consumer
/// that builds the type itself (#4213).
///
/// `t` names the type as its owner declares it (the `field` of a violation);
/// `cast_name` is what the value is branded as in this module, which differs from
/// it for a consumed type the module renames (`consumes c { T as U }`).
pub(in crate::emitter) fn refined_of_body(
    t: &TypeDecl,
    cast_name: &str,
    shape: RefinedShape<'_>,
    runtime_use: &RuntimeUse,
) -> Vec<bynk_ts::TsStmt> {
    let RefinedShape {
        base,
        refinement,
        is_opaque,
    } = shape;
    let total = bynk_syntax::ast::of_is_total(base, refinement, is_opaque);
    let mut body = if total {
        Vec::new()
    } else {
        emit_refined_checks(t, base, refinement)
    };
    // #2622: a `Decimal` violation reports its canonical text, since the
    // `bigint` coefficient cannot be JSON-stringified.
    if !total && base == BaseType::Decimal && refinement.is_some() {
        runtime_use.note_decimal("__bynkDecimalToString");
    }
    let cast = bynk_ts::TsExpr::As {
        expr: Box::new(bynk_ts::TsExpr::Ident("value".to_string())),
        ty: bynk_ts::TsType::named(cast_name.to_string()),
    };
    body.push(bynk_ts::TsStmt::return_stmt(
        Some(if total {
            cast
        } else {
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("Ok".to_string())),
                args: vec![cast],
            }
        }),
        None,
    ));
    body
}

/// The parts of a refined or opaque body [`refined_of_body`] reads.
pub(in crate::emitter) fn shape_parts(
    base: BaseType,
    refinement: Option<&Refinement>,
    is_opaque: bool,
) -> RefinedShape<'_> {
    RefinedShape {
        base,
        refinement,
        is_opaque,
    }
}

/// #1335 (R7.1): each guard is a real [`bynk_ts::TsStmt::If`] wrapping a
/// [`bynk_ts::TsStmt::Return`]. Since 1A.32c-1 the guards are returned as
/// nodes and sit directly in `of`'s body, so the printer lays them out.
fn emit_refined_checks(
    t: &TypeDecl,
    base: BaseType,
    refinement: Option<&Refinement>,
) -> Vec<bynk_ts::TsStmt> {
    let name = &t.name.name;
    let mut out = Vec::new();
    if base == BaseType::Int {
        // #1657: an `Int` is a JS safe integer.
        out.push(numeric_guard_stmt(
            name,
            "isSafeInteger",
            "must be a safe integer",
        ));
    }
    // v0.21: validated `Float` values are finite — `.of` and the boundary
    // codec agree (ADR 0040); only in-language arithmetic is host-defined.
    if base == BaseType::Float {
        out.push(numeric_guard_stmt(
            name,
            "isFinite",
            "must be a finite number",
        ));
    }
    if let Some(r) = refinement {
        for pred in bynk_syntax::ast::in_check_order(&r.predicates, |p| &p.kind) {
            out.push(pred_check_stmt(name, &pred.kind, base));
        }
    }
    out
}

/// `if (!Number.{method}(value)) { return Err({ field: "{name}", message:
/// "{message}", value }); }` — the `Int`/`Float` base-type guard shared by
/// both `emit_refined_checks` call sites, `field`/`message` both real
/// string literals with nothing to escape (a Bynk type name; a fixed,
/// hand-written English message).
fn numeric_guard_stmt(name: &str, method: &str, message: &str) -> bynk_ts::TsStmt {
    let cond = bynk_ts::TsExpr::Unary {
        op: bynk_ts::TsUnaryOp::Not,
        expr: Box::new(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("globalThis.Number".to_string())),
                property: method.to_string(),
            }),
            args: vec![bynk_ts::TsExpr::Ident("value".to_string())],
        }),
    };
    guard_if_stmt(
        cond,
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(name.to_string())),
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(message.to_string())),
        bynk_ts::well_formed_rule_expr(),
        false,
    )
}

/// `if (<cond>) { return Err({ field: <field>, message: <message>, value
/// }); }` — the one real shape both `numeric_guard_stmt` and
/// `pred_check_stmt` build, differing only in `cond`/`field`/`message`.
fn guard_if_stmt(
    cond: bynk_ts::TsExpr,
    field: bynk_ts::TsExpr,
    message: bynk_ts::TsExpr,
    rule: bynk_ts::TsExpr,
    decimal: bool,
) -> bynk_ts::TsStmt {
    let value_entry = if decimal {
        bynk_ts::TsObjectEntry::prop(
            "value".to_string(),
            call(ident("__bynkDecimalToString"), vec![ident("value")]),
        )
    } else {
        bynk_ts::TsObjectEntry::Shorthand("value".to_string())
    };
    let err_obj = bynk_ts::TsExpr::object_entries(vec![
        bynk_ts::TsObjectEntry::prop("field".to_string(), field),
        bynk_ts::TsObjectEntry::prop("message".to_string(), message),
        bynk_ts::TsObjectEntry::prop("rule".to_string(), rule),
        value_entry,
    ]);
    let return_stmt = bynk_ts::TsStmt::return_stmt(
        Some(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Ident("Err".to_string())),
            args: vec![err_obj],
        }),
        None,
    );
    bynk_ts::TsStmt::if_stmt(cond, bynk_ts::TsStmt::block(vec![return_stmt], None), None)
}

fn pred_check_stmt(type_name: &str, pred: &PredKind, base: BaseType) -> bynk_ts::TsStmt {
    let (cond, msg) = crate::emitter::pred_condition_and_message_for(pred, "value", Some(base));
    // #1471: `cond` is now a real `TsExpr` (a `Binary`/`Call` node, not
    // opaque text) — the explicit `Paren` stays regardless, matching this
    // function's own pre-#1471 output byte-for-byte: it was already always
    // applied unconditionally (never derived from `cond`'s own shape), and
    // still is, just wrapping a real node instead of an `Ident`. `msg` stays
    // opaque, pre-quoted text — see `pred_condition_and_message`'s own doc
    // for why (the `Matches` arm's message embeds already-`escape_ts_string`-
    // escaped pattern text that a second `TsLit::Str` escaping pass would
    // double-escape) — but is now tagged `VerbatimExpr`, not `Ident` (#1539:
    // review of `design/reviews/2026-08-30-post-restructuring-review.md`
    // named this exact site as a quoted string literal smuggled through
    // `Ident`; `TsLit::Str` is the real node for that shape everywhere else,
    // but would double-escape `msg` here specifically, so this site routes
    // through the tagged escape hatch instead, per the same review's own
    // fallback).
    let cond_expr = bynk_ts::TsExpr::Unary {
        op: bynk_ts::TsUnaryOp::Not,
        expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(cond))),
    };
    guard_if_stmt(
        cond_expr,
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(type_name.to_string())),
        bynk_ts::TsExpr::VerbatimExpr(format!("\"{msg}\""), bynk_ts::VerbatimOrigin::Emit),
        bynk_ts::pred_rule_expr(pred),
        base == BaseType::Decimal,
    )
}

/// v0.157 (ADR 0183): the erased TS type-parameter list for a generic
/// declaration (`<A, B>`), or `""` when non-generic. The same erasure used by
/// generic functions.
pub(crate) fn ts_type_params(params: &[TypeParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let names: Vec<String> = params.iter().map(|p| p.name.name.clone()).collect();
    format!("<{}>", names.join(", "))
}

/// #1339 (R7.1): `export interface {name}{params} { readonly {field}: {ty};
/// ... }` is a real [`bynk_ts::TsDecl::Interface`] (`type_params` bare
/// names, matching `ts_type_params`'s own convention exactly; each field a
/// [`bynk_ts::TsTypeMember::readonly_prop`], reusing [`TsTypeMember`]'s own
/// existing `readonly` field rather than a bespoke one — every real field
/// here is `readonly`), printed through [`bynk_ts::print_stmt`] at depth 0.
/// Field types route through `ts_ty_to_ts_type` (the real-node sibling
/// `ts_ty` itself already wraps, P7.9) instead of the opaque pre-printed
/// `String` `ts_ty` returns — a real node, not text. This function's own
/// exact signature is unchanged, the same P7.9/step-1 pattern.
///
/// #1486: no more explicit `TsStmt::blank(None)` entries — `bynk_ts::print`
/// itself now supplies the one blank line between adjacent real statements
/// (see `emit_refined_type`'s own identical note).
fn emit_record_type(
    t: &TypeDecl,
    fields: &[(String, TyId)],
    commons: &TypedCommons,
    runtime_use: &RuntimeUse,
    unit: UnitFacts<'_>,
) -> Vec<bynk_ts::TsStmt> {
    let type_params: Vec<String> = t.type_params.iter().map(|p| p.name.name.clone()).collect();
    let members: Vec<bynk_ts::TsTypeMember> = fields
        .iter()
        .map(|(name, ty)| {
            bynk_ts::TsTypeMember::readonly_prop(
                name.clone(),
                ts_ty_to_ts_type(*ty, unit.render(commons)),
            )
        })
        .collect();
    let interface = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Interface {
            name: t.name.name.clone(),
            type_params,
            members,
        })),
        None,
    );
    let entries = emit_attached_methods(&t.name.name, &t.type_params, commons, runtime_use, unit);
    let const_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: t.name.name.clone(),
            ty: None,
            init: bynk_ts::TsExpr::multiline_object_entries(entries),
        })),
        None,
    );
    vec![interface, const_decl]
}

/// #1339 (R7.1): `export type {name}{params} =\n  | {...}\n  | {...};` is a
/// real [`bynk_ts::TsDecl::TypeAlias`] over a new
/// [`bynk_ts::TsType::multiline_union`] (see its own doc for the exact
/// leading-pipe/spacing rules this reproduces byte-for-byte); `export const
/// {name} = { {tag}: ..., ... }` is a real [`bynk_ts::TsDecl::ConstDecl`]
/// whose `init` is one [`bynk_ts::TsExpr::multiline_object_entries`] — a
/// nullary variant is a real cast expression, a payload variant a real
/// [`bynk_ts::TsExpr::Arrow`] (`generics`/`return_type`, #1339's own two
/// gaps beyond the accepted proposal's own three) whose object-literal body
/// is wrapped in an explicit [`bynk_ts::TsExpr::Paren`] — `Arrow`'s own
/// renderer does not auto-parenthesise an object-literal body the way real
/// JS/TS syntax requires to disambiguate it from a block, so this call site
/// must ask for the parens itself, the same "explicit `Paren` always prints
/// its own literal parens" precedent #1323 already established. `emit_
/// attached_methods`'s own real `Vec<TsObjectEntry>` (#1337) is appended
/// directly into the SAME entries list, the same "own the whole object's
/// construction, drop the per-entry splice loop" pattern `emit_refined_
/// type`'s own conversion just used. This function's own exact signature
/// is unchanged, the same P7.9/step-1 pattern.
///
/// #1486: no more explicit `TsStmt::blank(None)` entries — `bynk_ts::print`
/// itself now supplies the one blank line between adjacent real statements
/// (see `emit_refined_type`'s own identical note).
fn emit_sum_type(
    t: &TypeDecl,
    variants: &[(String, Vec<(String, TyId)>)],
    commons: &TypedCommons,
    runtime_use: &RuntimeUse,
    unit: UnitFacts<'_>,
) -> Vec<bynk_ts::TsStmt> {
    // #593: a generic sum erases to a TypeScript generic discriminated union,
    // exactly as a generic record erases to `interface Name<T>`. The type
    // parameters ride the `export type` header and each payload constructor
    // (`Some: <T>(v: T): Opt<T> => …`); a payload-less constructor stays a
    // constant, cast to the all-`never` instantiation (`Opt<never>`), which is
    // assignable to every `Opt<X>` since the nullary arm names no parameter.
    // Both are empty for a non-generic sum, keeping its output identical.
    let name = t.name.name.clone();
    let type_params: Vec<String> = t.type_params.iter().map(|p| p.name.name.clone()).collect();
    let never_ty = if type_params.is_empty() {
        bynk_ts::TsType::named(name.clone())
    } else {
        bynk_ts::TsType::named_with_args(
            name.clone(),
            type_params
                .iter()
                .map(|_| bynk_ts::TsType::named("never"))
                .collect(),
        )
    };
    let self_ty = if type_params.is_empty() {
        bynk_ts::TsType::named(name.clone())
    } else {
        bynk_ts::TsType::named_with_args(
            name.clone(),
            type_params
                .iter()
                .map(|p| bynk_ts::TsType::named(p.clone()))
                .collect(),
        )
    };

    let variant_types: Vec<bynk_ts::TsType> = variants
        .iter()
        .map(|(tag, payload)| {
            let mut members = vec![bynk_ts::TsTypeMember::readonly_prop(
                "tag",
                bynk_ts::TsType::named(format!("\"{tag}\"")),
            )];
            members.extend(payload.iter().map(|(field, ty)| {
                bynk_ts::TsTypeMember::readonly_prop(
                    payload_prop(field),
                    ts_ty_to_ts_type(*ty, unit.render(commons)),
                )
            }));
            bynk_ts::TsType::Object(members)
        })
        .collect();
    let type_alias = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::TypeAlias {
            name: name.clone(),
            type_params: type_params.clone(),
            ty: bynk_ts::TsType::multiline_union(variant_types),
        })),
        None,
    );
    let mut stmts = vec![type_alias];

    let mut entries: Vec<bynk_ts::TsObjectEntry> = Vec::new();
    for (tag, payload) in variants {
        if payload.is_empty() {
            let tag_obj = bynk_ts::TsExpr::object_entries(vec![bynk_ts::TsObjectEntry::prop(
                "tag".to_string(),
                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(tag.clone())),
            )]);
            entries.push(bynk_ts::TsObjectEntry::prop(
                tag.clone(),
                bynk_ts::TsExpr::As {
                    expr: Box::new(tag_obj),
                    ty: never_ty.clone(),
                },
            ));
        } else {
            let ctor_params: Vec<bynk_ts::TsParam> = payload
                .iter()
                .map(|(field, ty)| {
                    bynk_ts::TsParam::new(
                        ts_ident(&payload_prop(field)),
                        Some(ts_ty_to_ts_type(*ty, unit.render(commons))),
                    )
                })
                .collect();
            let mut obj_entries = vec![bynk_ts::TsObjectEntry::prop(
                "tag".to_string(),
                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(tag.clone())),
            )];
            // #1653: the parameter is a binding, so a field named like a
            // reserved word (`class`, `arguments`, `deps`) or `globalThis` is
            // renamed there by `ts_ident`; the property keeps its own name.
            obj_entries.extend(payload.iter().map(|(field, _)| {
                let prop = payload_prop(field);
                let param = ts_ident(&prop);
                if param == prop {
                    bynk_ts::TsObjectEntry::Shorthand(prop)
                } else {
                    bynk_ts::TsObjectEntry::prop(prop, bynk_ts::TsExpr::Ident(param))
                }
            }));
            let ctor_body =
                bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::object_entries(obj_entries)));
            entries.push(bynk_ts::TsObjectEntry::prop(
                tag.clone(),
                bynk_ts::TsExpr::Arrow {
                    params: ctor_params,
                    is_async: false,
                    generics: type_params.clone(),
                    return_type: Some(self_ty.clone()),
                    body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(ctor_body))),
                },
            ));
        }
    }
    entries.extend(emit_attached_methods(
        &t.name.name,
        &t.type_params,
        commons,
        runtime_use,
        unit,
    ));
    let const_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name,
            ty: None,
            init: bynk_ts::TsExpr::multiline_object_entries(entries),
        })),
        None,
    );
    stmts.push(const_decl);
    stmts
}

#[cfg(test)]
mod doc_block_tests {
    /// The printer's indentation step, in columns.
    const INDENT_STEP: usize = 2;

    /// A doc block printed as a JSDoc comment at `indent` columns, the way the
    /// printer lays out a `DocComment` statement.
    fn emit_doc_block(out: &mut String, doc: Option<&str>, indent: usize) {
        let Some(doc) = doc else { return };
        let stmt = bynk_ts::TsStmt::doc_comment(doc, None);
        out.push_str(&bynk_ts::print_stmt(&stmt, indent / INDENT_STEP));
    }

    /// A `*/` in a doc body must not close the JSDoc comment early — otherwise
    /// the trailing text lands as executable top-level TypeScript (issue #720).
    #[test]
    fn escapes_comment_terminator() {
        let mut out = String::new();
        emit_doc_block(
            &mut out,
            Some("docs */ ; (globalThis as any).PWNED = true; /*"),
            0,
        );
        assert!(
            !out.contains("*/ ;"),
            "unescaped comment terminator leaked into output: {out}"
        );
        assert!(out.contains("*\\/ ;"), "expected escaped terminator: {out}");
        // The single legitimate closer is the one we emit last.
        assert_eq!(
            out.matches("*/").count(),
            1,
            "exactly one real closer: {out}"
        );
        assert!(out.trim_end().ends_with("*/"));
    }

    /// Overlapping/adjacent terminators must not survive or re-form a `*/`
    /// under the non-overlapping left-to-right `str::replace`. Pins the
    /// behaviour against a future refactor away from `str::replace`.
    #[test]
    fn escapes_pathological_terminators() {
        for body in ["*/*/", "**/", "*/*", "*/ */ */"] {
            let mut out = String::new();
            emit_doc_block(&mut out, Some(body), 0);
            // The only surviving `*/` is the emitter's own trailing closer.
            assert_eq!(
                out.matches("*/").count(),
                1,
                "input {body:?} left a stray closer: {out}"
            );
            assert!(out.trim_end().ends_with("*/"), "input {body:?}: {out}");
        }
    }

    /// #1333: the real, converted `emit_doc_block` matches
    /// `137_agent_instantiation_workers`'s own real header comment
    /// byte-for-byte — not a synthetic string, the real fixture content
    /// this function's own conversion is verified against.
    #[test]
    fn matches_the_real_fixtures_own_header_comment_byte_for_byte() {
        let mut out = String::new();
        emit_doc_block(
            &mut out,
            Some(
                "A minimal stateful agent in the bundle target: instantiation lowers through the\ngenerated factory, the method call is a direct call, and state persists per key\nacross calls within a session.",
            ),
            0,
        );
        assert_eq!(
            out,
            "/**\n \
             * A minimal stateful agent in the bundle target: instantiation lowers through the\n \
             * generated factory, the method call is a direct call, and state persists per key\n \
             * across calls within a session.\n \
             */\n"
        );
    }

    /// #1333: `indent` is always an exact multiple of `INDENT_STEP` in
    /// every real call site (`0` or `INDENT_STEP`) — pins that the
    /// `indent / INDENT_STEP` conversion to the printer's own depth unit
    /// produces the real, historic indentation exactly.
    #[test]
    fn indents_at_the_nested_indent_step() {
        let mut out = String::new();
        emit_doc_block(&mut out, Some("a method"), INDENT_STEP);
        assert_eq!(out, "  /**\n   * a method\n   */\n");
    }
}

#[cfg(test)]
mod ts_type_params_tests {
    use super::ts_type_params;
    use bynk_syntax::ast::{Ident, TypeParam};
    use bynk_syntax::span::Span;

    fn type_param(name: &str) -> TypeParam {
        TypeParam {
            name: Ident {
                name: name.to_string(),
                span: Span::new(0, name.len()),
            },
            bounds: Vec::new(),
            span: Span::new(0, name.len()),
        }
    }

    #[test]
    fn empty_params_render_as_the_empty_string() {
        assert_eq!(ts_type_params(&[]), "");
    }

    /// #1333: the real, converted `ts_type_params` matches
    /// `816_locale_negotiation_no_bundle_regression`'s own real
    /// `export function map<A, B>(...)` generic list byte-for-byte.
    #[test]
    fn matches_the_real_fixtures_own_two_param_generic_list() {
        let params = [type_param("A"), type_param("B")];
        assert_eq!(ts_type_params(&params), "<A, B>");
    }

    #[test]
    fn single_param_renders_with_no_separator() {
        assert_eq!(ts_type_params(&[type_param("T")]), "<T>");
    }
}

#[cfg(test)]
mod refined_checks_tests {
    use super::{numeric_guard_stmt, pred_check_stmt};
    use bynk_syntax::ast::{BaseType, PredKind};

    /// #1335: the real, converted `numeric_guard_stmt` matches
    /// `254_multi_file_commons_workers_codec`'s own real `Cents.of`
    /// `Int`-base guard byte-for-byte.
    #[test]
    fn numeric_guard_matches_the_real_fixtures_own_int_guard_byte_for_byte() {
        assert_eq!(
            bynk_ts::print_stmt(
                &numeric_guard_stmt("Cents", "isInteger", "must be an integer"),
                2
            ),
            "    if (!globalThis.Number.isInteger(value)) {\n      \
             return Err({ field: \"Cents\", message: \"must be an integer\", value });\n    \
             }\n"
        );
    }

    /// #1335: the real, converted `pred_check_stmt` matches
    /// `254_multi_file_commons_workers_codec`'s own real `Cents.of`
    /// `NonNegative` predicate check byte-for-byte.
    #[test]
    fn pred_check_matches_the_real_fixtures_own_non_negative_check_byte_for_byte() {
        let out = bynk_ts::print_stmt(
            &pred_check_stmt("Cents", &PredKind::NonNegative, BaseType::Int),
            2,
        );
        assert_eq!(
            out,
            "    if (!(value >= 0)) {\n      \
             return Err({ field: \"Cents\", message: \"must be non-negative\", value });\n    \
             }\n"
        );
    }

    /// #1335's own real deviation from the accepted proposal: `msg` is
    /// carried as opaque, pre-quoted text (not `TsLit::Str`) specifically
    /// because `PredKind::Matches`'s own message embeds
    /// `escape_ts_string`-escaped pattern text directly. A source pattern
    /// with one real backslash (`\d+`) comes back from `escape_ts_string`
    /// already doubled to two backslash *characters* (the escaped form —
    /// what a TS string literal must contain for the runtime string to hold
    /// one literal backslash), and both the condition's own regex-source
    /// string and the message embed that same already-escaped text raw,
    /// matching the pre-conversion `writeln!` code exactly. If `msg` were
    /// instead run through `TsLit::Str`'s own escaper (as the accepted
    /// proposal's own Decision B originally called for), those two already-
    /// doubled backslash characters would each be escaped AGAIN, quadrupling
    /// the original single backslash — this test pins the correct,
    /// once-escaped form and would fail under that double-escaping bug. Not
    /// reachable by any fixture today (no `Matches` predicate in the corpus
    /// uses a backslash), so this is the only proof this real, latent bug
    /// class stays closed.
    #[test]
    fn pred_check_does_not_double_escape_a_matches_patterns_own_backslash() {
        let out = bynk_ts::print_stmt(
            &pred_check_stmt(
                "Code",
                &PredKind::Matches(r"\d+".to_string()),
                BaseType::String,
            ),
            2,
        );
        // `escape_ts_string` doubles the pattern's one real backslash to two
        // backslash *characters* — the correctly-escaped form, not a bug.
        // Both the regex-source string and the message embed that same
        // twice-backslash text raw (one escaping pass, not two).
        assert!(
            out.contains("\"^(?:\" + \"\\\\d+\" + \")$\""),
            "condition should carry the once-escaped two-backslash form: {out}"
        );
        assert!(
            out.contains("must match /\\\\d+/"),
            "message should carry the same once-escaped two-backslash form: {out}"
        );
        // A double-escaping bug would quadruple the original single
        // backslash to four backslash characters in the message.
        assert!(
            !out.contains("must match /\\\\\\\\d+/"),
            "message must not be double-escaped to four backslashes: {out}"
        );
    }
}

/// Differential coverage for `emit_type`/`emit_record_type`/`emit_sum_type`/
/// `emit_refined_type` reading `bynk-emit::ir::TypeShape` instead of walking
/// `TypeDecl`/`TypeBody` directly (P6.x, #1188). Every expected string below
/// was captured from this slice's own converted output and cross-checked by
/// hand against `ts_type_ref_with`'s formatting (the pre-existing `TypeRef`
/// renderer `ts_ty`, `emitter.rs:4130`, replaces for record/sum field types) —
/// the two produce identical text for every shape exercised here (base types
/// including `Bytes`, a bare generic type variable, a bare named cross-type
/// reference, a `List`/`Option` wrapper, a generic application (`Box[Int]`),
/// and a refined type's brand surviving as a field type rather than
/// collapsing to its base).
#[cfg(test)]
mod type_shape_emission_tests {
    use crate::testkit::emit_source;

    const TYPES_FIXTURE: &str = r#"
commons demo {
  type Order = { id: String, total: Int }
  type Box[T] = { value: T }
  type PaymentError = enum { Declined, InsufficientFunds }
  type OrderError =
    | OutOfStock(sku: String, qty: Int)
    | Payment(reason: PaymentError)
    embeds PaymentError as Payment
  type Age = Int where Positive
  type UserId = opaque Int
  type Extras = { tags: List[String], blob: Bytes, note: Option[String], boxed: Box[Int], age: Age }
}
"#;

    /// Pins the `ts_type_ref` → `ts_ty` equivalence claim past the four
    /// shapes `Order`/`Box`/`OrderError` already exercise (review on #1190):
    /// a collection wrapper (`List`), a non-`number` base (`Bytes`), another
    /// wrapper (`Option`), a *generic application* (`Box[Int]` — the
    /// `TypeRef::App`/`Ty::Named{args: non-empty}` arm, distinct from the
    /// bare `Named` arm `OrderError`'s `reason: PaymentError` field already
    /// covers), and a refined field (`Age`) to assert its brand survives
    /// rather than collapsing to its base `number`.
    #[test]
    fn record_field_types_cover_wrappers_generics_and_refined_brands() {
        let ts = emit_source(TYPES_FIXTURE);
        assert!(
            ts.contains(
                "export interface Extras {\n  \
                 readonly tags: readonly string[];\n  \
                 readonly blob: globalThis.Uint8Array;\n  \
                 readonly note: Option<string>;\n  \
                 readonly boxed: Box<number>;\n  \
                 readonly age: Age;\n}\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn record_type_emits_interface_and_namespace_object() {
        let ts = emit_source(TYPES_FIXTURE);
        assert!(
            ts.contains(
                "export interface Order {\n  readonly id: string;\n  readonly total: number;\n}\n"
            ),
            "{ts}"
        );
        assert!(ts.contains("export const Order = {\n};\n"), "{ts}");
    }

    /// #1339: pins `emit_sum_type`'s own real multi-line, leading-pipe
    /// union shape and its generic payload-constructor arrows byte-for-byte
    /// — the expected strings below are transcribed directly from
    /// `406_generic_sum_envelope`'s own real `expected.ts`
    /// (`bynk-tests/tests/fixtures/positive/406_generic_sum_envelope`), confirmed
    /// to match it exactly, not independently invented text that merely
    /// looks plausible.
    #[test]
    fn generic_sum_type_matches_the_real_fixtures_own_multiline_union_byte_for_byte() {
        let ts = emit_source(
            r#"
commons envelope {
  type ApiResult[T] =
    | Loaded(value: T)
    | Failed(message: String)
}
"#,
        );
        assert!(
            ts.contains(
                "export type ApiResult<T> =\n    \
                 { readonly tag: \"Loaded\"; readonly value: T }\n  \
                 | { readonly tag: \"Failed\"; readonly message: string };\n"
            ),
            "{ts}"
        );
        assert!(
            ts.contains(
                "export const ApiResult = {\n  \
                 Loaded: <T>(value: T): ApiResult<T> => ({ tag: \"Loaded\", value }),\n  \
                 Failed: <T>(message: string): ApiResult<T> => ({ tag: \"Failed\", message }),\n\
                 };\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn generic_record_type_erases_its_type_parameter() {
        let ts = emit_source(TYPES_FIXTURE);
        assert!(
            ts.contains("export interface Box<T> {\n  readonly value: T;\n}\n"),
            "{ts}"
        );
    }

    #[test]
    fn sum_type_emits_discriminated_union_and_nullary_constants() {
        let ts = emit_source(TYPES_FIXTURE);
        assert!(
            ts.contains(
                "export type PaymentError =\n    { readonly tag: \"Declined\" }\n  | \
                 { readonly tag: \"InsufficientFunds\" };\n"
            ),
            "{ts}"
        );
        assert!(
            ts.contains(
                "export const PaymentError = {\n  \
                 Declined: { tag: \"Declined\" } as PaymentError,\n  \
                 InsufficientFunds: { tag: \"InsufficientFunds\" } as PaymentError,\n};\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn sum_type_payload_variants_emit_typed_fields_and_constructors() {
        let ts = emit_source(TYPES_FIXTURE);
        assert!(
            ts.contains(
                "export type OrderError =\n    \
                 { readonly tag: \"OutOfStock\"; readonly sku: string; readonly qty: number }\n  | \
                 { readonly tag: \"Payment\"; readonly reason: PaymentError };\n"
            ),
            "{ts}"
        );
        assert!(
            ts.contains(
                "export const OrderError = {\n  \
                 OutOfStock: (sku: string, qty: number): OrderError => ({ tag: \"OutOfStock\", sku, qty }),\n  \
                 Payment: (reason: PaymentError): OrderError => ({ tag: \"Payment\", reason }),\n};\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn refined_type_emits_branded_alias_and_of_constructor_without_unsafe() {
        let ts = emit_source(TYPES_FIXTURE);
        assert!(
            ts.contains("export type Age = number & { readonly __brand: \"Age\" };\n"),
            "{ts}"
        );
        assert!(
            ts.contains(
                "export const Age = {\n  of(value: number): Result<Age, ValidationError> {\n    \
                 if (!globalThis.Number.isSafeInteger(value)) {\n      \
                 return Err({ field: \"Age\", message: \"must be a safe integer\", value });\n    }\n    \
                 if (!(value > 0)) {\n      \
                 return Err({ field: \"Age\", message: \"must be positive\", value });\n    }\n    \
                 return Ok(value as Age);\n  },\n};\n"
            ),
            "{ts}"
        );
        assert!(!ts.contains("unsafe(value: number): Age"), "{ts}");
    }

    #[test]
    fn opaque_type_emits_branded_alias_and_unsafe_constructor() {
        let ts = emit_source(TYPES_FIXTURE);
        assert!(
            ts.contains("export type UserId = number & { readonly __brand: \"UserId\" };\n"),
            "{ts}"
        );
        assert!(
            ts.contains("  unsafe(value: number): UserId {\n    return value as UserId;\n  },\n"),
            "{ts}"
        );
    }

    /// Review of #1338, finding 1: a type parameter literally named `deps`
    /// (a value-identifier reserved word, but a perfectly legal Bynk type
    /// parameter — `parse_optional_type_params` accepts any identifier) must
    /// render UNESCAPED at an attached method's own generic-declaration
    /// site, matching every other reference to that same parameter
    /// (`self_ty_args`, the method's own param/return types). Before the
    /// fix, `generics` ran the name through `ts_ident` (value-identifier
    /// escaping), producing `get<__id_deps>(self: Box<deps>): deps { ... }`
    /// — `deps` undeclared, `__id_deps` unused, a real `tsc` error; no
    /// fixture in the corpus covered a reserved-word type param, so the
    /// zero-diff check alone could not have caught this.
    #[test]
    fn a_reserved_word_type_parameter_renders_unescaped_on_an_attached_method() {
        let ts = emit_source(
            r#"
commons demo {
  type Box[deps] = { value: deps }

  fn Box.get(self) -> deps {
    self.value
  }
}
"#,
        );
        assert!(
            ts.contains("get<deps>(self: Box<deps>): deps {"),
            "expected the bare, unescaped `deps` type parameter at every \
             reference (declaration, receiver, return type); got:\n{ts}"
        );
        assert!(
            !ts.contains("__id_deps"),
            "the type parameter's own declaration site must not be escaped \
             via ts_ident (that renaming is for value identifiers only); \
             got:\n{ts}"
        );
    }
}
