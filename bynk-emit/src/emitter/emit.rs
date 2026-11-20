//! Per-declaration emission — the functions `emit_project` drives to render
//! each top-level Bynk declaration into TypeScript: type/refined/record/sum
//! declarations and their checks, attached methods and free functions,
//! capabilities, providers, services, contexts, and agents (plus the
//! worker-dispatch lowering helpers those emitters use). Split out of
//! `emitter.rs` (ADR 0060); the codec/reference/import/header helpers and the
//! `ts_*`/`LowerCtx` core stay in the parent and are reached via `use super::*`.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use super::zero_value::{render_zero_value, zero_expr as zero_value_expr};
use crate::emit_model::EmitProjectCtx;
use bynk_check::actors::ActorDecl;
use bynk_check::checker::{TyId, TypedCommons, Types};
use bynk_check::firstparty::FirstPartyCap;
use bynk_syntax::ast::{
    AgentDecl, BaseType, CapabilityDecl, CommonsItem, ConstDecl, Expr, ExprKind, FnDecl, FnName,
    Handler, Ident, MessageEntry, MessagesDecl, Param, PredKind, ProviderDecl, RecordField,
    Refinement, ServiceDecl, StoreField, TypeBody, TypeDecl, TypeParam, TypeRef,
};

use bynk_model::{
    ActorSeamIr, FnSig, IrHandlerKind, IrHttpMethod, OpSig, ProtocolIr, SchemaVersionIr,
    StoreFieldIr, StoreKindIr, TypeShape,
};
use bynk_model::{
    HandlerSignatureIr, body_writes_state, is_effectful_return, lower_actor_seam_ir,
    lower_handler_kind_ir, lower_protocol_ir_from_commons,
};

use super::ctx::{AgentStoreState, BodyMode, HandlerShared, ModuleCtx};
use super::lower_ctx::LowerCtx;
use super::lower_stmt::{Body, Item, block_body_with_return, claim_scoped, function_body_stmts};
use super::ts_types::{ts_base, ts_ty, ts_ty_to_ts_type, ts_type_ref, ts_type_ref_to_ts_type};
use super::*;

/// P6.x (#1188): reads `shape` — `t`'s already-lowered `bynk-emit::ir::TypeShape`
/// — instead of walking `t.body` (the AST's own `TypeBody`) directly. Doesn't
/// spell that AST module's path literally in this comment on purpose: this
/// file is otherwise invisible to the `ast_importers` probe
/// (`xtask/src/greenfield_status.rs`), which matches on the literal string,
/// comments included — see `design/tracks/the-ir.md` §5's own note on this
/// exact blind spot.
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

/// The TypeScript zero-value expression for a state field, or `None` if its
/// type is not zeroable (#3165: the checker decides, `zero_value` renders).
fn zero_value_ts(
    type_ref: &TypeRef,
    inline: Option<&Refinement>,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<String> {
    bynk_check::checker::zero_value(type_ref, inline, types).map(|z| render_zero_value(&z))
}

/// The runtime test a `via schema(...)` guard negates (events slice 4b, #990):
/// `===` for a literal, `>=` and/or `<=` for a range, and none for `_`, which
/// only forwards the envelope. Bounds are inclusive.
fn schema_version_test(env_binder: &str, version: SchemaVersionIr) -> Option<String> {
    let v = format!("{env_binder}.schemaVersion");
    match version {
        SchemaVersionIr::Exact(n) => Some(format!("{v} === {n}")),
        SchemaVersionIr::Range {
            lo: Some(lo),
            hi: Some(hi),
        } => Some(format!("{v} >= {lo} && {v} <= {hi}")),
        SchemaVersionIr::Range {
            lo: Some(lo),
            hi: None,
        } => Some(format!("{v} >= {lo}")),
        SchemaVersionIr::Range {
            lo: None,
            hi: Some(hi),
        } => Some(format!("{v} <= {hi}")),
        SchemaVersionIr::Range { lo: None, hi: None } | SchemaVersionIr::Any => None,
    }
}

/// The parts of a refined-or-opaque `TypeBody` that its lowering reads.
///
/// Honest about what this is: `emit_refined_type` reached eight arguments when it
/// gained `runtime_use`, and grouping three of them keeps it under
/// `clippy::too_many_arguments` without another `#[allow]`. The three do cohere —
/// all come out of the same `t.body` the caller already matched on — but the
/// function still takes six parameters, so this is a lint fix that reads well,
/// not a decomposition.
struct RefinedShape<'a> {
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

    let checks = {
        let checks = emit_refined_checks(t, base, refinement);
        // #2622: a `Decimal` violation reports its canonical text, since the
        // `bigint` coefficient cannot be JSON-stringified.
        if base == BaseType::Decimal && refinement.is_some() {
            runtime_use.note_decimal("__bynkDecimalToString");
        }
        for p in refinement.iter().flat_map(|r| &r.predicates) {
            runtime_use.note_predicate(&p.kind);
        }
        checks
    };
    let cast_to_name = bynk_ts::TsExpr::As {
        expr: Box::new(bynk_ts::TsExpr::Ident("value".to_string())),
        ty: bynk_ts::TsType::named(name.clone()),
    };
    let mut of_body = checks;
    of_body.push(bynk_ts::TsStmt::return_stmt(
        Some(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Ident("Ok".to_string())),
            args: vec![cast_to_name.clone()],
        }),
        None,
    ));
    let mut entries: Vec<bynk_ts::TsObjectEntry> = vec![bynk_ts::TsObjectEntry::Method {
        name: "of".to_string(),
        is_async: false,
        generics: Vec::new(),
        params: vec![bynk_ts::TsParam::new(
            "value".to_string(),
            Some(base_ty.clone()),
        )],
        return_type: Some(bynk_ts::TsType::named_with_args(
            "Result",
            vec![
                bynk_ts::TsType::named(name.clone()),
                bynk_ts::TsType::named("ValidationError"),
            ],
        )),
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
    decimal: bool,
) -> bynk_ts::TsStmt {
    let value_entry = if decimal {
        bynk_ts::TsObjectEntry::prop(
            "value".to_string(),
            bynk_ts::TsExpr::Ident("__bynkDecimalToString(value)".to_string()),
        )
    } else {
        bynk_ts::TsObjectEntry::Shorthand("value".to_string())
    };
    let err_obj = bynk_ts::TsExpr::object_entries(vec![
        bynk_ts::TsObjectEntry::prop("field".to_string(), field),
        bynk_ts::TsObjectEntry::prop("message".to_string(), message),
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
        base == BaseType::Decimal,
    )
}

/// v0.157 (ADR 0183): the erased TS type-parameter list for a generic
/// declaration (`<A, B>`), or `""` when non-generic. The same erasure used by
/// generic functions.
///
/// #1333 (R7.1): each name prints through a real [`bynk_ts::TsType::named`]
/// via [`bynk_ts::print_type`] — P7.9's own established entry point for one
/// type fragment — instead of splicing the raw `&str` directly; this
/// function's own `-> String` signature and every one of its 5 real callers
/// are unchanged.
pub(crate) fn ts_type_params(params: &[TypeParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let names: Vec<String> = params
        .iter()
        .map(|p| bynk_ts::print_type(&bynk_ts::TsType::named(p.name.name.as_str())))
        .collect();
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

/// #1337: returns real `bynk_ts::TsObjectEntry`s (was `out: &mut String`) —
/// #1478's own `emit_refined_type`/`emit_record_type`/`emit_sum_type`
/// conversion appends these directly into a real `TsExpr::Object`'s own
/// entries, with no opaque-entries carrier needed at all (the question this
/// slice's own accepted proposal was asked to resolve).
fn emit_attached_methods(
    type_name: &str,
    type_params: &[TypeParam],
    commons: &TypedCommons,
    runtime_use: &RuntimeUse,
    unit: UnitFacts<'_>,
) -> Vec<bynk_ts::TsObjectEntry> {
    let mut entries = Vec::new();
    for item in &commons.commons.items {
        let CommonsItem::Fn(f) = item else { continue };
        let FnName::Method {
            type_name: t,
            method_name,
        } = &f.name
        else {
            continue;
        };
        if t.name != type_name {
            continue;
        }
        entries.push(emit_method(
            f,
            type_name,
            type_params,
            method_name,
            commons,
            runtime_use,
            unit,
        ));
    }
    entries
}

/// v0.132.1 (#481): the consumer-context rebrand of a `uses`-imported refined
/// type carries a value-side const (`.of` / `.unsafe`, emitted inline in
/// `emit_context_rebrands`). A commons may *also* attach user-defined methods
/// to the type (`fn Cents.fromInt(...)`, merged into the const by
/// [`emit_attached_methods`] in the commons' *own* output). Those methods must
/// reach the consumer too, or a call like `Cents.fromInt(n)` — which the
/// checker accepts — fails `tsc` on the rebranded const. Forward each attached
/// method by delegating to the value-imported `__Commons{type_name}` (a value
/// import, so the delegate target exists at runtime); the `as unknown as`
/// return cast bridges the commons brand to the context brand, exactly like the
/// inline `.of` / `.unsafe` forwarders. Covers static *and* instance methods
/// (a consumer-branded `self` is a structural subtype of `__Commons{name}`, so
/// no argument cast is needed).
///
/// `methods` are the imported type's attached method signatures, threaded in
/// via [`EmitProjectCtx::imported_methods`] (the consumer context's own
/// `TypedCommons` merges the imported *types* but not their fn items). They
/// arrive pre-sorted by name so emission is deterministic.
///
/// P6.18: reads [`FnSig`] (the declaring unit's own resolved `params`/
/// `return_ty`) rather than a raw `FnDecl`'s `TypeRef`s — `self`'s own type
/// still comes from `type_name` directly, the *consumer* context's own
/// rebranded name, never resolved through `FnSig` at all (a method's own
/// generic receiver plays no part in what gets forwarded here).
///
/// #1337: fully self-contained, no `emitter/lower.rs` dependency at all
/// (unlike [`emit_method`]'s own opaque body) — the body here is one real,
/// statically-known statement shape, so it converts to a real
/// `bynk_ts::TsStmt::Return`, no opaque carrier needed. `out: &mut String`
/// kept (the P7.9/step-1 pattern): `emitter.rs`'s own real caller
/// (`emit_context_rebrands`) is still unconverted, so this stays a "build a
/// real node internally, print just that fragment" helper rather than
/// returning entries the caller isn't ready to consume structurally.
/// Arc C, slice 30 (#1392): returns real `bynk_ts::TsObjectEntry::Method`
/// values (was `out: &mut String`) — the one real call site
/// (`emit_context_rebrands`) now appends them directly into its own
/// `Vec<TsObjectEntry>` instead of splicing printed text into a shared
/// buffer, the same `emit_attached_methods` precedent (#1337) already
/// established for its own sibling per-entry builder.
pub(crate) fn emit_forwarded_methods(
    type_name: &str,
    methods: &[FnSig],
    tys: TyRender<'_>,
) -> Vec<bynk_ts::TsObjectEntry> {
    let mut entries = Vec::new();
    for f in methods {
        let mut params: Vec<bynk_ts::TsParam> = Vec::new();
        let mut args: Vec<bynk_ts::TsExpr> = Vec::new();
        if f.has_self {
            params.push(bynk_ts::TsParam::new(
                "self".to_string(),
                Some(bynk_ts::TsType::named(type_name)),
            ));
            args.push(bynk_ts::TsExpr::Ident("self".to_string()));
        }
        for (name, ty) in &f.params {
            let ident = ts_ident(name);
            params.push(bynk_ts::TsParam::new(
                ident.clone(),
                Some(bynk_ts::TsType::named(ts_ty(*ty, tys))),
            ));
            args.push(bynk_ts::TsExpr::Ident(ident));
        }
        let ret = ts_ty(f.return_ty, tys);
        let call = bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident(format!("__Commons{type_name}"))),
                property: f.name.clone(),
            }),
            args,
        };
        // #1704: no cast. The commons method speaks the unbranded types,
        // which fit the context's optionally-branded ones directly.
        let body = vec![bynk_ts::TsStmt::return_stmt(Some(call), None)];
        let entry = bynk_ts::TsObjectEntry::Method {
            name: f.name.clone(),
            is_async: false,
            // `FnSig` (P6.18's own note) never carries generics here — a
            // forwarded method's own receiver is always the consumer
            // context's own concrete rebranded name, never resolved
            // through a generic receiver.
            generics: Vec::new(),
            params,
            return_type: Some(bynk_ts::TsType::named(ret)),
            // `FnSig` carries no doc text either — nothing forwards it.
            doc: None,
            // #1337's own one real `inline: true` site: the pre-conversion
            // `writeln!` always built the whole entry — signature and
            // one-statement body alike — on one physical line, unlike
            // every other real `Method` entry in this tree.
            inline: true,
            body,
            span: None,
        };
        entries.push(entry);
    }
    entries
}

/// #2719: the type of the hidden execution-context parameter a function in the
/// needs-context set takes last; the same shape a handler's `deps.__exec` has.
fn exec_param(name: &str) -> bynk_ts::TsParam {
    bynk_ts::TsParam {
        name: name.to_string(),
        ty: Some(bynk_ts::TsType::Object(vec![
            bynk_ts::TsTypeMember::method(
                "waitUntil",
                vec![bynk_ts::TsParam {
                    name: "promise".to_string(),
                    ty: Some(bynk_ts::TsType::named_with_args(
                        "globalThis.Promise",
                        vec![bynk_ts::TsType::named("unknown")],
                    )),
                    optional: false,
                    rest: false,
                }],
                bynk_ts::TsType::named("void"),
            ),
        ])),
        optional: false,
        rest: false,
    }
}

/// #1337: returns one real `bynk_ts::TsObjectEntry::Method` (was
/// `out: &mut String`) — the per-entry builder [`emit_attached_methods`]
/// calls once per matching attached method.
///
/// The method's own body is NOT built here at all — it's delegated
/// wholesale to `emit_block_as_function_body_with_return`
/// (`emitter/lower.rs:201`), the one splice boundary ADR
/// `arc-c-lower-rs-permanent-exclusion` names as a *permanent*, deliberate
/// exclusion from this tree (`lower.rs` is the compiler's own second
/// code-generation pass, comprehensive language-surface work Arc C was
/// never scoped to cover) — captured into a `String` at the exact same
/// absolute indent the pre-conversion code always passed
/// (`INDENT_STEP * 2`), then carried as one opaque `bynk_ts::TsStmt::raw`
/// statement, printed exactly as given with no reinterpretation.
fn emit_method(
    f: &FnDecl,
    type_name: &str,
    type_params: &[TypeParam],
    method_name: &Ident,
    commons: &TypedCommons,
    runtime_use: &RuntimeUse,
    unit: UnitFacts<'_>,
) -> bynk_ts::TsObjectEntry {
    // #1337: a real gap the accepted proposal's own citation missed (it
    // searched for `///`-style doc markers; this language's own doc block
    // is `---`-delimited, per the lexer — `Timestamp.diff`/`Timestamp.add`
    // in `65_money_uses_time` both carry one) — caught by the zero-diff
    // fixture check, not reasoned about in the abstract.
    let doc = f.documentation.as_deref().map(str::to_owned);
    // #594: a method on a generic type erases to a generic namespace-object
    // member. The namespace `const` cannot itself carry `<T>`, so the type's own
    // parameters are threaded onto *each* method alongside the method's own
    // (`map<T, U>(self: Box<T>, …)`). `ts_type_params` is empty for a
    // non-generic type, keeping the pre-#594 output byte-identical.
    let self_ty_args = ts_type_params(type_params);
    let mut method_generics: Vec<TypeParam> = type_params.to_vec();
    method_generics.extend(f.type_params.iter().cloned());
    // #1337: a real gap the accepted proposal's own citation missed (it
    // searched the project-form fixture corpus only; the one real site,
    // `402_generic_instance_method`, is single-file form) — caught by the
    // zero-diff fixture check, not reasoned about in the abstract. Bare
    // names only, matching `ts_type_params`'s own rendering exactly (see
    // `TsObjectEntry::Method::generics`'s own doc for why a full
    // `Vec<TsParam>` would be the wrong shape here).
    //
    // Review of #1338: must be a bare, UNESCAPED clone, not `ts_ident`.
    // `ts_ident` is for value identifiers and renames reserved words like
    // `deps`/`static`/`package` — all legal Bynk *type*-parameter names
    // (`parse_optional_type_params` accepts any identifier). `self_ty_args`
    // (via `ts_type_params`, a few lines above) and every `ts_type_ref`
    // param/return type reference these same names unescaped, so escaping
    // only the declaration site here would desync it from its own uses:
    // `map<__id_deps>(self: Box<deps>): Box<deps>` — `deps` undeclared,
    // `__id_deps` unused, a real tsc error the pre-conversion code never
    // had (it built `map<deps>(self: Box<deps>)`, self-consistent).
    let generics: Vec<String> = method_generics
        .iter()
        .map(|tp| tp.name.name.clone())
        .collect();
    let mut params: Vec<bynk_ts::TsParam> = Vec::new();
    if f.has_self {
        params.push(bynk_ts::TsParam::new(
            "self".to_string(),
            Some(bynk_ts::TsType::named(format!("{type_name}{self_ty_args}"))),
        ));
    }
    for p in &f.params {
        params.push(bynk_ts::TsParam::new(
            ts_ident(&p.name.name),
            Some(bynk_ts::TsType::named(ts_type_ref(&p.type_ref))),
        ));
    }
    let empty = bynk_check::resolver::CrossContextInfo::default();
    let mut cx = LowerCtx::new(
        ModuleCtx::new(commons, &empty, runtime_use, unit),
        BodyMode::Method,
    );
    // Constrained generics slice 1: one trailing comparator parameter per
    // `Ord`-bounded type parameter (the receiver type's first).
    params.extend(cx.declare_witnesses(f));
    // #2719: the execution context, last.
    if commons.exec_plan.fn_needs(f) {
        params.push(exec_param("__exec"));
    }
    // A pure method is a plain member; an effectful one is `async` (below).
    // #1653: parameters are bindings of the body's own scope, so a `let` that
    // re-binds one gets a fresh name instead of redeclaring it.
    for p in &f.params {
        cx.declare_binder(&p.name.name);
    }
    // #2725: a method is `async` exactly as a free fn is (`-> Effect[T]`), so
    // an effectful method that awaits is valid TS; #2719's exec-context need
    // also forces it (a defensive union: such a method is effectful anyway).
    let is_async = is_effectful_return(&f.return_type) || commons.exec_plan.fn_needs(f);
    // #3193: a direct self tail call runs as a loop, in constant stack. A method
    // carries no contract clauses (`emit_method` guards none), so there are no
    // `requires` to re-check per iteration; if methods gain them, they belong in
    // the guards closure below.
    let body = if let Some(plan) = super::tco::plan(f, commons, unit.render(commons)) {
        super::tco::loop_body_stmts(f, &plan, &mut cx, is_async, INDENT_STEP * 2, |_| {
            Body::new()
        })
    } else {
        function_body_stmts(
            &f.body,
            &mut cx,
            INDENT_STEP * 2,
            is_async,
            Some(&f.return_type),
        )
    };
    bynk_ts::TsObjectEntry::Method {
        name: method_name.name.clone(),
        is_async,
        generics,
        params,
        return_type: Some(bynk_ts::TsType::named(ts_type_ref(&f.return_type))),
        doc,
        inline: false,
        body,
        span: None,
    }
}

/// #1351 (R7.1): `export {async}function {name}{generics}({params}): {ret} {
/// <body> }` is a real [`bynk_ts::TsDecl::Function`] — `params`/`return_type`
/// route through the already-real [`ts_type_ref_to_ts_type`] (P7.9, #1315;
/// this file's own directly-callable private-sibling-visibility precedent,
/// #1339) instead of the opaque pre-printed `String` `ts_type_ref` returns;
/// `generics` is `TsDecl::Function`'s own real gap this proposal closes
/// (bare names, matching every other real generics-list precedent in this
/// crate). The function's own BODY — whichever of the two still-unconverted
/// sources built it (the ordinary lowered block, or
/// `emit_contract_guarded_body`'s own guard-wrapped one, NEITHER converted
/// by this proposal) — is captured into a fresh buffer and carried as ONE
/// opaque [`bynk_ts::TsStmtKind::Raw`] statement, unchanged text, the exact
/// precedent #1337 established for `emit_method`'s own wholesale-delegated
/// body. This function's own exact signature is unchanged, the P7.9/step-1
/// pattern — it never owned a `Verbatim` construction site.
///
/// Review-caught-before-merge, #1351: the body can no longer lower directly
/// into `out` the way the pre-conversion code did (source-map checkpoints
/// were correct there only because the body's own text landed at its real,
/// final position in `out` as it was written). Once the body is captured
/// into its own local `body_text` buffer for embedding as a `Raw`
/// statement, any `record_span` call made *during* that lowering would
/// record an offset relative to `body_text`'s own 0-based length — wrong
/// once spliced elsewhere. Uses the same local-sub-builder-then-`merge`
/// pattern `emit_service`'s own handler-body lowering already established
/// (`LowerCtx::record_span`'s own doc: "a caller building ... into its own
/// local `String` ... before splicing it elsewhere must not call this with
/// that buffer's own length") — `body_smb` collects checkpoints relative to
/// `body_text`, then `merge`s into the real `source_map`, rebased at
/// `body_text`'s own splice offset within the fully-printed declaration.
/// #1480: returns real [`bynk_ts::TsStmt`]s (was `out: &mut String` plus a
/// hand-rolled `merge` call) — `#1477` (`print_stmt_and_merge`) now recovers
/// the `Raw` body's own print-time splice offset itself, so this function no
/// longer needs to recover it by exact arithmetic over its own printed text
/// (the `debug_assert!`-guarded computation this replaces). The body's own
/// `body_smb` is attached directly to the `Raw` body statement's own
/// `nested_map` field instead; every caller now prints (and, if it has a
/// real module map to merge into, merges) through `print_stmt_and_merge`
/// rather than a plain `print_stmt`/`push_str`.
pub(crate) fn emit_free_fn(
    f: &FnDecl,
    commons: &TypedCommons,
    // v0.115: emit the contract call-site guard (dev/test profile). Stripped
    // (false) in the deploy build for zero runtime cost (DECISION J).
    contracts: bool,
    runtime_use: &RuntimeUse,
    cross_context: &bynk_check::resolver::CrossContextInfo,
    unit: UnitFacts<'_>,
) -> Vec<bynk_ts::TsStmt> {
    emit_free_fn_in(
        f,
        commons,
        contracts,
        runtime_use,
        cross_context,
        true,
        false,
        unit,
    )
}

/// #2834: a suite-local helper `fn` — `emit_free_fn` without the `export`,
/// since it is declared inside a test runner and called from its body.
pub fn emit_suite_fn(
    f: &FnDecl,
    commons: &TypedCommons,
    runtime_use: &RuntimeUse,
) -> Vec<bynk_ts::TsStmt> {
    // A suite is only ever built for `bynk test`, which guards every contract.
    // A suite helper cannot declare `given`, so it has no capability types to name.
    let empty = bynk_check::resolver::CrossContextInfo::default();
    emit_free_fn_in(
        f,
        commons,
        true,
        runtime_use,
        &empty,
        false,
        true,
        UnitFacts::detached(),
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_free_fn_in(
    f: &FnDecl,
    commons: &TypedCommons,
    contracts: bool,
    runtime_use: &RuntimeUse,
    cross_context: &bynk_check::resolver::CrossContextInfo,
    export: bool,
    suite_fn: bool,
    unit: UnitFacts<'_>,
) -> Vec<bynk_ts::TsStmt> {
    let FnName::Free(name) = &f.name else {
        return Vec::new();
    };
    let mut stmts = Vec::new();
    if let Some(doc) = f.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let mut params: Vec<bynk_ts::TsParam> = f
        .params
        .iter()
        .map(|p| {
            bynk_ts::TsParam::new(
                ts_ident(&p.name.name),
                Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
            )
        })
        .collect();
    let is_async = is_effectful_return(&f.return_type);
    // v0.20a: erased TS generics — the type parameters print verbatim and
    // exist only at TS type-check time (no runtime dispatch).
    let generics: Vec<String> = f
        .type_params
        .iter()
        .map(|tp| tp.name.name.clone())
        .collect();

    let empty = bynk_check::resolver::CrossContextInfo::default();
    let given_ir: Vec<bynk_model::CapRefIr> =
        f.given.iter().map(bynk_model::lower_cap_ref_ir).collect();
    let cx = if given_ir.is_empty() {
        LowerCtx::new(
            ModuleCtx::new(commons, &empty, runtime_use, unit),
            BodyMode::FreeFn,
        )
    } else {
        LowerCtx::new(
            ModuleCtx::new(commons, cross_context, runtime_use, unit),
            BodyMode::GivenFn {
                handler: HandlerShared {
                    capabilities: given_ir.iter().map(|c| c.name.clone()).collect(),
                    handler_scope: Some(format!(
                        "{}.fn.{}",
                        commons.commons.name.joined(),
                        name.name
                    )),
                    owning_context: commons.commons.name.joined(),
                    ..HandlerShared::default()
                },
            },
        )
    };
    // #2990: a suite helper records its `Json` codec roots for the test module
    // that splices it in.
    let mut cx = if suite_fn { cx.with_suite_fn() } else { cx };
    // Constrained generics slice 1: one trailing comparator parameter per
    // `Ord`-bounded type parameter.
    params.extend(cx.declare_witnesses(f));
    // #2868: the capabilities a `given` function names, as a hidden `deps`
    // parameter (before the execution context): exactly the declared ones, so
    // a caller hands over its own `deps`, which has them.
    if !given_ir.is_empty() {
        params.push(bynk_ts::TsParam::new(
            "deps".to_string(),
            Some(bynk_ts::TsType::Object(
                given_ir
                    .iter()
                    .map(|c| {
                        bynk_ts::TsTypeMember::prop(c.name.clone(), cap_ref_ty(c, cross_context))
                    })
                    .collect(),
            )),
        ));
    }
    // #2719: the execution context, last.
    if commons.exec_plan.fn_needs(f) {
        params.push(exec_param("__exec"));
    }
    let async_tail = is_effectful_return(&f.return_type);
    let guarded = contracts && (!f.requires.is_empty() || !f.ensures.is_empty());
    // #1653: parameters are bindings of the body's own scope, so a `let` that
    // re-binds one gets a fresh name instead of redeclaring it.
    for p in &f.params {
        cx.declare_binder(&p.name.name);
    }
    // #3193: a direct self tail call runs as a loop, in constant stack.
    let body = if let Some(plan) = super::tco::plan(f, commons, unit.render(commons)) {
        let check_requires = contracts && !f.requires.is_empty();
        let guards = |cx: &mut LowerCtx| {
            let mut guards = Body::new();
            if check_requires {
                requires_guards(&mut guards, f, cx);
            }
            guards
        };
        super::tco::loop_body_stmts(f, &plan, &mut cx, async_tail, INDENT_STEP, guards)
    } else if guarded {
        contract_guarded_body_stmts(f, &mut cx, async_tail)
    } else {
        function_body_stmts(
            &f.body,
            &mut cx,
            INDENT_STEP,
            async_tail,
            Some(&f.return_type),
        )
    };

    // #1486: `no_blank_before` when this function's own doc comment is the
    // preceding statement — `bynk_ts::print` itself now supplies the one
    // blank line between adjacent real statements (a fresh call site's own
    // preceding item, or nothing at all for the very first top-level
    // statement), but a doc comment sits flush above the declaration it
    // documents, no blank between them.
    let function = bynk_ts::TsDecl::Function {
        name: ts_ident(&name.name),
        generics,
        params,
        return_type: Some(ts_type_ref_to_ts_type(&f.return_type, None)),
        body,
        is_async,
        inline: false,
    };
    let mut fn_stmt = bynk_ts::TsStmt::decl(
        if export {
            bynk_ts::TsDecl::Export(Box::new(function))
        } else {
            function
        },
        None,
    );
    fn_stmt.no_blank_before = f.documentation.is_some();
    stmts.push(fn_stmt);
    stmts
}

/// #2617: this file's constants, in declaration order, each a hoisted,
/// cached function rather than an exported `const`:
///
/// ```ts
/// var __const_MaxRetries: { v: number } | undefined;
/// export function MaxRetries(): number { return (__const_MaxRetries ??= { v: 3 }).v; }
/// ```
///
/// A function declaration is hoisted and its cache a `var` (which reads as
/// `undefined` before its statement runs, where a `let` would throw), so a
/// constant can be read before its module has evaluated: when sibling files
/// of a unit import each other in a cycle, through a function, a type or
/// another constant, ES module evaluation order cannot reach a constant
/// before it is initialised. The initialiser runs once, on the first read,
/// and the `{ v }` box keeps a falsy or `null` value cached. Every reference
/// to a constant lowers to a call (`lower_ident`, through `Callee::Const`).
/// The initialiser is lowered as a static expression (the agent field
/// initialiser's lowering, `BodyMode::StaticInit`). Empty when the file
/// declares none, so a module without constants is unchanged.
pub(crate) fn emit_consts(commons: &TypedCommons, ctx: &EmitProjectCtx) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    for item in &commons.commons.items {
        let CommonsItem::Const(c) = item else {
            continue;
        };
        let mut module = ModuleCtx::for_unit(commons, ctx);
        module.target = ctx.target;
        module.consumed_type_ns = consumed_type_ns(ctx);
        module.consumed_type_declared = ctx.consumed_type_aliases.clone();
        module.consumed_context_types = ctx.consumed_context_types();
        stmts.extend(emit_const_in(c, module));
    }
    stmts
}

/// One constant's cached-function declaration (see [`emit_consts`]), lowered in
/// the module context `module`.
fn emit_const_in(c: &ConstDecl, module: ModuleCtx<'_>) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    {
        let mut pre = Pre::new();
        let mut icx = LowerCtx::new(module, BodyMode::StaticInit);
        let init = pre.lower(&c.init, &mut icx);
        // The closed static set (`consts::static_error`) holds no `if`,
        // `match`, block or `?`, the only shapes the lowering hoists setup
        // statements for, so an initialiser lowers to one expression.
        assert!(
            pre.is_empty(),
            "bynk internal error: constant `{}`'s static initialiser hoisted statements",
            c.name.name
        );
        let name = ts_ident(&c.name.name);
        let cache = format!("__const_{}", c.name.name);
        let ty = ts_type_ref_to_ts_type(&c.ty, None);
        // `var __const_Name: { v: T } | undefined;`
        let var = bynk_ts::TsStmt::var_stmt(
            cache.clone(),
            bynk_ts::TsType::Union {
                members: vec![
                    bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::Prop {
                        name: "v".to_string(),
                        ty: ty.clone(),
                        optional: false,
                        readonly: false,
                    }]),
                    bynk_ts::TsType::named("undefined"),
                ],
                multiline: false,
            },
            None,
        );
        stmts.push(var);
        // The documentation goes on the function, which is what a consumer
        // imports and an editor shows.
        if let Some(doc) = c.documentation.as_deref() {
            let mut doc = bynk_ts::TsStmt::doc_comment(doc, None);
            doc.no_blank_before = true;
            stmts.push(doc);
        }
        // `export function Name(): T { return (__const_Name ??= { v: <init> }).v; }`
        let mut func = bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Function {
                name,
                generics: Vec::new(),
                params: Vec::new(),
                return_type: Some(ty),
                body: vec![bynk_ts::TsStmt::return_stmt(
                    Some(bynk_ts::TsExpr::Ident(format!(
                        "({cache} ??= {{ v: {init} }}).v"
                    ))),
                    None,
                )],
                is_async: false,
                inline: true,
            })),
            Some(c.span),
        );
        func.no_blank_before = true;
        stmts.push(func);
    }
    stmts
}

/// A `${…}` interpolation of each in-scope value for a contract failure report.
fn contract_param_dump(f: &FnDecl, extra_result: bool) -> String {
    let mut parts: Vec<String> = f
        .params
        .iter()
        .filter(|p| p.name.name != "_")
        .map(|p| format!("{n}=${{{v}}}", n = p.name.name, v = ts_ident(&p.name.name)))
        .collect();
    if extra_result {
        parts.push("result=${result}".to_string());
    }
    parts.join(", ")
}

/// `requires` guards of a contracted free function, parameters in scope. Also
/// re-run at the top of each iteration of a self-tail-call loop (#3193).
fn requires_guards(body: &mut Body, f: &FnDecl, cx: &mut LowerCtx) {
    let FnName::Free(name) = &f.name else {
        return;
    };
    let fn_name = name.name.clone();
    // Precondition guards — parameters are the TS function params, in scope.
    for c in &f.requires {
        let mut pre = Pre::new();
        let pred = pre.lower(&c.predicate, cx);
        body.extend(pre.items());
        let msg = format!(
            "contract violated: precondition \\`{clause}\\` of {fn_name} ({dump})",
            clause = c.name.name,
            dump = contract_param_dump(f, false),
        );
        body.push(Item::Node(Box::new(contract_guard_if_stmt(&pred, &msg))));
    }
}

/// #1353 (R7.1): closes step (4) of the design pass's own decomposition
/// order — `emit_free_fn`'s own outer wrapper landed in #1352; this converts
/// `emit_contract_guarded_body`'s own real remainder. Each precondition/
/// postcondition guard (`if (!(pred)) { const __e = new Error(`...`);
/// __e.name = "BynkContractError"; throw __e; }`) is a real
/// [`bynk_ts::TsStmt::if_stmt`] over a real [`bynk_ts::TsStmt::inline_block`]
/// of 3 real statements (`const_stmt`/`assign`/[`bynk_ts::TsStmt::throw_stmt`]
/// — `Throw` is this proposal's own real gap, `bynk_ts::TsStmtKind` had no
/// bare `throw` before now), printed through [`bynk_ts::print_stmt`]; the
/// trailing `return result;` is a real [`bynk_ts::TsStmt::return_stmt`].
///
/// `pred` (`Pre::lower`'s own return) and each hoisted `pre.stmts()` line
/// stay opaque — carried the same way #1335's own `pred_check_stmt` used to
/// carry `pred_condition_and_message`'s `cond` (an `Ident` wrapped in this
/// crate's real `Unary::Not`/`Paren`) before #1471 converted it to a real
/// `TsExpr`, but for a DIFFERENT, stronger, and still-standing reason: `Pre`
/// lives in `emitter/lower.rs` (`pre.lower` calls `lower_expr`, the general
/// expression lowerer) — the one splice boundary ADR
/// `arc-c-lower-rs-permanent-exclusion` names a *permanent* Arc C exclusion,
/// not the temporarily-unconverted-but-convertible-later machinery
/// `pred_condition_and_message` (`emitter.rs` proper) turned out to be. The
/// error message (backtick-quoted, with its own already-escaped `\``
/// clause-name delimiters and `${param}=...`-style interpolations already
/// baked in as literal text by `param_dump`) is carried the same way as one
/// opaque [`bynk_ts::TsExpr::template_lit`] part with zero real `exprs` —
/// `parts` prints verbatim, so this reproduces the exact byte output with no
/// decomposition needed.
///
/// The result-capturing IIFE (`const result = (async () => { <body> }
/// )();`/`const result = (() => { <body> })();`) stays ONE opaque
/// [`bynk_ts::TsStmt::raw`] statement — not decomposed into a real
/// `Const`/`Arrow`/`Call` — matching #1352's own identical treatment of
/// `emit_free_fn`'s unguarded body one level up: #1327's own analogous
/// single site found a real block-body `Arrow` "would have needed new
/// flatten-every-nested-statement ... printer machinery, disproportionate";
/// #1435 later added `TsArrowBody::Block` for a genuinely different real
/// site (`serialisation.rs`'s `Float` guard), but that variant's own
/// printer is still the same one-physical-line compact-statement renderer,
/// which cannot represent this IIFE's own genuine multi-statement block
/// body either. `emit_block_as_function_body_with_return`'s own output
/// (still unconverted, out of scope) is captured into a local buffer inside
/// that one opaque statement, the same way it always has been.
///
/// This function's own exact `out: &mut String` signature is unchanged, the
/// P7.9/step-1 pattern one level deeper than #1352's own conversion — it
/// never owned a `Verbatim` construction site (still spliced into
/// `emit_free_fn`'s own `body_text` buffer, itself carried as `emit_free_
/// fn`'s own `Raw` statement).
///
/// v0.115: emit a contracted free function's body behind the dev/test call-site
/// guard (DECISION J). Preconditions (`requires`) are checked on entry; the body
/// runs into a captured `result`; postconditions (`ensures`) are checked over
/// `result` before it is returned. Each violation throws with the clause name
/// and the offending argument/`result` values. This wrapper is emitted only in
/// the dev/test profile and is O(1) in code size (one guard, not per call site).
///
/// 1A.15: the body is the statements of the function body, not text — the
/// original body runs inside the `(() => { ... })()` text, whose lowered
/// statements nest one step deeper, so they reach the printer through
/// [`Body::into_stmts`] with their checkpoints.
fn contract_guarded_body_stmts(
    f: &FnDecl,
    cx: &mut LowerCtx,
    async_tail: bool,
) -> Vec<bynk_ts::TsStmt> {
    let FnName::Free(name) = &f.name else {
        return Vec::new();
    };
    let fn_name = name.name.clone();
    let mut body = Body::new();
    requires_guards(&mut body, f, cx);
    // Run the original body, capturing its value as `result` for the `ensures`.
    body.push(Item::line(if async_tail {
        "const result = await (async () => {"
    } else {
        "const result = (() => {"
    }));
    body.push(Item::Indent(block_body_with_return(
        &f.body,
        cx,
        INDENT_STEP * 2,
        async_tail,
        Some(&f.return_type),
    )));
    body.push(Item::line("})();"));
    // Postcondition guards — `result` (and the parameters) are in scope.
    for c in &f.ensures {
        let mut pre = Pre::new();
        let pred = pre.lower(&c.predicate, cx);
        body.extend(pre.items());
        let msg = format!(
            "contract violated: postcondition \\`{clause}\\` of {fn_name} ({dump})",
            clause = c.name.name,
            dump = contract_param_dump(f, true),
        );
        body.push(Item::Node(Box::new(contract_guard_if_stmt(&pred, &msg))));
    }
    body.push(Item::Node(Box::new(bynk_ts::TsStmt::return_stmt(
        Some(bynk_ts::TsExpr::Ident("result".to_string())),
        None,
    ))));
    body.into_stmts(INDENT_STEP)
}

/// Shared builder for the contract call-site guard's own real `if (!(pred))
/// { const __e = new Error(`msg`); __e.name = "BynkContractError"; throw
/// __e; }` shape — identical for a `requires`/`ensures` clause, differing
/// only in `pred`/`msg`, both still-opaque text (see `emit_contract_guarded_
/// body`'s own doc for why).
fn contract_guard_if_stmt(pred: &str, msg: &str) -> bynk_ts::TsStmt {
    let cond = bynk_ts::TsExpr::Unary {
        op: bynk_ts::TsUnaryOp::Not,
        expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Ident(
            pred.to_string(),
        )))),
    };
    let new_error = bynk_ts::TsExpr::New {
        callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.Error".to_string())),
        args: vec![bynk_ts::TsExpr::template_lit(
            vec![msg.to_string()],
            Vec::new(),
        )],
    };
    let const_e = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__e".to_string()),
        None,
        new_error,
        None,
    );
    let assign_name = bynk_ts::TsStmt::assign(
        bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("__e".to_string())),
            property: "name".to_string(),
        },
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str("BynkContractError".to_string())),
        None,
    );
    let throw_e = bynk_ts::TsStmt::throw_stmt(bynk_ts::TsExpr::Ident("__e".to_string()), None);
    let then_branch = bynk_ts::TsStmt::inline_block(vec![const_e, assign_name, throw_e], None);
    bynk_ts::TsStmt::if_stmt(cond, then_branch, None)
}

#[cfg(test)]
mod emit_free_fn_tests {
    use crate::testkit::emit_source;

    /// #1351: pins `emit_free_fn`'s own real generic/non-generic output
    /// byte-for-byte against `198_generic_identity_compose`'s own real
    /// `expected.ts` (`bynk-tests/tests/fixtures/positive/198_generic_identity_compose`)
    /// — a generic single-param function, a multi-generic multi-param one
    /// (function-typed params, exercising `TsParam.ty`'s own `Fn` arm), and
    /// a plain non-generic one, transcribed directly from that fixture.
    #[test]
    fn generic_and_plain_free_functions_match_the_real_fixtures_own_bytes() {
        let ts = emit_source(
            r#"
commons generic

fn identity[A](x: A) -> A {
  x
}

fn compose[A, B, C](f: A -> B, g: B -> C, x: A) -> C {
  g(f(x))
}

fn demo() -> Int {
  identity(5)
}
"#,
        );
        assert!(
            ts.contains("export function identity<A>(x: A): A {\n  return x;\n}\n"),
            "{ts}"
        );
        assert!(
            ts.contains(
                "export function compose<A, B, C>(f: (a0: A) => B, g: (a0: B) => C, x: A): C {\n  \
                 return g(f(x));\n}\n"
            ),
            "{ts}"
        );
        assert!(
            ts.contains("export function demo(): number {\n  return identity(5);\n}\n"),
            "{ts}"
        );
    }

    /// #1351: pins the `async`/effectful-return case — `is_effectful_return`
    /// gates both the header's own `async` keyword and `async_tail`'s body
    /// lowering, unchanged by this conversion.
    #[test]
    fn an_effectful_free_function_emits_async_and_no_generics() {
        let ts = emit_source(
            r#"
commons effectful

fn compute() -> Effect[Int] {
  1
}
"#,
        );
        assert!(
            ts.contains("export async function compute(): globalThis.Promise<number> {\n"),
            "{ts}"
        );
    }
}

/// Synthesise a TypeScript-safe method name for an `on http METHOD path`
/// handler. The result is used both as the key on the service object and
/// as the identifier the Worker fetch handler invokes. Path parameter
/// segments (`:name`) become `Param_name` to remain distinct from literal
/// segments. (v0.9 §5.3)
///
/// Slice 1 of `#1542` (the IR cutover, `design/archive/retired-tracks.md`): this used to
/// have an AST-typed `HttpMethod` twin, `http_handler_method_name` — deleted
/// once its last production caller converted to this `IrHttpMethod` version,
/// which every call site now holds already-resolved (`IrHandlerKind::Http`'s
/// own payload) rather than re-deriving from `HandlerKind::Http`.
pub fn http_handler_method_name_ir(method: IrHttpMethod, path: &str) -> String {
    http_handler_method_name_from_str(method.as_str(), path)
}

fn http_handler_method_name_from_str(method: &str, path: &str) -> String {
    let mut s = format!("http_{method}");
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        s.push('_');
        if let Some(rest) = seg.strip_prefix(':') {
            s.push_str("Param_");
            s.push_str(&sanitise_path_segment(rest));
        } else {
            s.push_str(&sanitise_path_segment(seg));
        }
    }
    s
}

/// Slice 3 (semantic-debugging track, ADR 0105): the debug-metadata sidecar's
/// `{ fn → label }` map — each emitted handler function paired with its Bynk
/// operation label, so the debugger can name a stack frame `GET "/"` rather than
/// `http_GET`. Built by re-walking the unit's handlers with the *same* naming
/// functions the emitter uses (so the keys match the emitted function names);
/// serialised as a JSON object (manual, like the source map — no serde). Returns
/// `None` when the unit declares no handlers.
pub fn collect_handler_labels(commons: &TypedCommons) -> Option<String> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for item in &commons.commons.items {
        match item {
            CommonsItem::Service(s) => {
                let (mut cron_idx, mut queue_idx) = (0usize, 0usize);
                for h in &s.handlers {
                    let pair = match lower_handler_kind_ir(&h.kind) {
                        IrHandlerKind::Http { method, path } => (
                            http_handler_method_name_ir(method, &path),
                            format!("{} \"{}\"", method.as_str(), path),
                        ),
                        IrHandlerKind::Cron { expr } => {
                            let n = cron_handler_method_name(&s.name.name, cron_idx);
                            cron_idx += 1;
                            (n, format!("cron \"{expr}\""))
                        }
                        IrHandlerKind::Message
                            if matches!(
                                lower_protocol_ir_from_commons(&s.protocol, commons),
                                ProtocolIr::WebSocket { .. }
                            ) =>
                        {
                            ("message".to_string(), "WebSocket message".to_string())
                        }
                        IrHandlerKind::Message => {
                            let n = queue_handler_method_name(&s.name.name, queue_idx);
                            queue_idx += 1;
                            (n, "message".to_string())
                        }
                        IrHandlerKind::Call => {
                            ("call".to_string(), handler_op_label("call", &h.params))
                        }
                        IrHandlerKind::Open => ("open".to_string(), "WebSocket open".to_string()),
                        IrHandlerKind::Close => {
                            ("close".to_string(), "WebSocket close".to_string())
                        }
                        // Events track, slice 0 (spine #936): exactly one
                        // `on event` per `from Events(E)` service (no
                        // pattern-refinement fan-out yet, so no index).
                        IrHandlerKind::Event => ("event".to_string(), "event".to_string()),
                        IrHandlerKind::Rejected => ("rejected".to_string(), "rejected".to_string()),
                    };
                    entries.push(pair);
                }
            }
            CommonsItem::Agent(a) => {
                for h in &a.handlers {
                    if let Some(name) = &h.method_name {
                        entries.push((name.name.clone(), handler_op_label(&name.name, &h.params)));
                    }
                }
            }
            _ => {}
        }
    }
    if entries.is_empty() {
        return None;
    }
    // Dedup by key (e.g. two services each with an `on call` emit a `call` method in
    // their own object — distinct in the emitted code, but one key here); keep the
    // first so the JSON object is well-formed.
    let mut seen = std::collections::HashSet::new();
    let mut out = String::from("{");
    let mut first = true;
    for (k, v) in &entries {
        if !seen.insert(k.clone()) {
            continue;
        }
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&bynk_project::json_string(k));
        out.push(':');
        out.push_str(&bynk_project::json_string(v));
    }
    out.push('}');
    Some(out)
}

/// `name(p1, p2)` from a handler's parameters — the operation label for `call` and
/// agent handlers (HTTP handlers use method + path instead).
fn handler_op_label(name: &str, params: &[Param]) -> String {
    let ps: Vec<String> = params.iter().map(|p| p.name.name.clone()).collect();
    format!("{}({})", name, ps.join(", "))
}

/// Synthesise a TypeScript-safe method name for an `on cron` handler (v0.10a):
/// `cron_<service>_<index>`, where `index` is the handler's position among the
/// service's cron handlers in declaration order. The same key is computed at
/// each emission site (the `handlers` method, the `compose` surface wrapper,
/// and the `scheduled` dispatcher) by walking handlers in the same order, so it
/// is collision-free and stable without encoding the schedule expression.
pub fn cron_handler_method_name(service: &str, index: usize) -> String {
    format!("cron_{service}_{index}")
}

/// v0.12: order a context's providers so each appears after the providers of
/// the capabilities it depends on (its `given`). Used by the composition root
/// to emit `const <Cap> = new <Provider>({ deps })` bindings in dependency
/// order. Cycles are rejected by the checker, so this terminates; the marker is
/// set before recursing as a defensive guard. Keyed by capability name.
pub(crate) fn topo_order_providers(
    providers: &std::collections::HashMap<String, ProviderDecl>,
) -> Vec<String> {
    fn visit(
        node: &str,
        providers: &std::collections::HashMap<String, ProviderDecl>,
        visited: &mut HashSet<String>,
        order: &mut Vec<String>,
    ) {
        if visited.contains(node) {
            return;
        }
        visited.insert(node.to_string());
        if let Some(p) = providers.get(node) {
            let given = bynk_model::lower_provider_given_ir(p);
            let mut deps: Vec<&str> = given
                .iter()
                .filter(|d| d.context.is_none())
                .map(|d| d.name.as_str())
                .filter(|n| providers.contains_key(*n))
                .collect();
            deps.sort_unstable();
            for d in deps {
                visit(d, providers, visited, order);
            }
        }
        order.push(node.to_string());
    }
    let mut order = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut keys: Vec<&String> = providers.keys().collect();
    keys.sort();
    for k in keys {
        visit(k, providers, &mut visited, &mut order);
    }
    order
}

/// Method name for an `on queue` handler (v0.10b): `queue_<service>_<index>`,
/// by the handler's position among the service's queue handlers. Computed
/// identically at the `handlers` method, the `compose` surface wrapper, and the
/// `queue` dispatcher (queue names are unique context-wide, but the index keeps
/// the key identifier-safe without sanitising the name).
pub fn queue_handler_method_name(service: &str, index: usize) -> String {
    format!("queue_{service}_{index}")
}

fn sanitise_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

/// Left-folds `parts` into one `+`-chained `bynk_ts::TsExpr::Binary` tree,
/// matching `.join(" + ")`'s own left-to-right text join exactly —
/// `TsBinaryOp::Add`'s own real, dominant site in this whole cluster (#1388).
/// Panics on an empty `parts`; every real caller below already special-cases
/// the empty-template case before reaching here.
fn join_plus(mut parts: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    let first = parts.remove(0);
    parts
        .into_iter()
        .fold(first, |acc, next| bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::Add,
            left: Box::new(acc),
            right: Box::new(next),
        })
}

/// One message entry's TS renderer: `(params) => <expr>`. A literal-only
/// template collapses to a plain string; a template with placeholders becomes
/// a `+`-concatenation, each placeholder substituted from `params` via
/// `renderArg` (imported from `bynk.locale`, never duplicated here) when
/// present, else left as the literal `{name}` token — total, never throws,
/// matching `render`'s own totality contract.
///
/// message-bundles slice 3 (#878): an ICU-dispatch placeholder (`inner`
/// contains a `,`) instead becomes `emit_icu_placeholder`'s codegen, which
/// needs `locale_tag` to construct `Intl.*` at runtime. `Err(_)` (a
/// malformed ICU construct) is unreachable in practice — the checker
/// (`bynk.messages.malformed_icu_syntax`) already rejects it before emission
/// runs on an error-free project — but is handled totally, matching this
/// function's own "never throws" contract, rather than `unreachable!()`.
///
/// Arc C, step (11) (#1388): returns a real `bynk_ts::TsExpr::Arrow` instead
/// of writing text — the caller (`emit_messages_bundle`, #1355) drops its
/// own opaque `TsExpr::Ident(renderer_text)` wrap and uses this value
/// directly as the object entry's own value.
fn emit_message_entry_renderer(
    entry: &MessageEntry,
    locale_tag: &str,
    runtime_use: &RuntimeUse,
) -> bynk_ts::TsExpr {
    let segments = split_template(&entry.template);
    let parts: Vec<bynk_ts::TsExpr> = segments
        .iter()
        .map(|s| match s {
            TemplateSegment::Literal(lit) => {
                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str((*lit).to_string()))
            }
            TemplateSegment::Placeholder { inner, .. } => match inner.find(',') {
                None => {
                    // `(params.get(key) !== undefined ? renderArg(params.get(key) as MessageArg) : fallback)`
                    // — unconditionally parenthesised, matching the
                    // pre-conversion text exactly even when this is the
                    // template's own only segment (no `+`-join to borrow
                    // parens from in that case).
                    let get_call = bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Member {
                            object: Box::new(bynk_ts::TsExpr::Ident("params".to_string())),
                            property: "get".to_string(),
                        }),
                        args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            (*inner).to_string(),
                        ))],
                    };
                    bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Conditional {
                        test: Box::new(bynk_ts::TsExpr::Binary {
                            op: bynk_ts::TsBinaryOp::StrictNotEq,
                            left: Box::new(get_call.clone()),
                            right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                        }),
                        consequent: Box::new(bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Ident("__bynkRenderArg".to_string())),
                            args: vec![bynk_ts::TsExpr::As {
                                expr: Box::new(get_call),
                                ty: bynk_ts::TsType::named("__bynkMessageArg"),
                            }],
                        }),
                        alternate: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(format!(
                            "{{{inner}}}"
                        )))),
                    }))
                }
                Some(_) => match icu::parse_icu_placeholder(inner) {
                    Ok(p) => emit_icu_placeholder(&p, locale_tag, runtime_use),
                    Err(_) => {
                        let name = inner.split(',').next().unwrap_or(inner).trim();
                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(format!("{{{name}}}")))
                    }
                },
            },
        })
        .collect();
    let body = if parts.is_empty() {
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(String::new()))
    } else {
        join_plus(parts)
    };
    bynk_ts::TsExpr::Arrow {
        params: vec![bynk_ts::TsParam::new(
            "params".to_string(),
            Some(bynk_ts::TsType::named_with_args(
                "ReadonlyMap",
                vec![
                    bynk_ts::TsType::named("string"),
                    bynk_ts::TsType::named("__bynkMessageArg"),
                ],
            )),
        )],
        is_async: false,
        generics: Vec::new(),
        return_type: Some(bynk_ts::TsType::named("string")),
        body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(body))),
    }
}

/// message-bundles slice 3 (#878): codegen for one ICU-dispatch placeholder.
/// Each kind emits as an IIFE (matching this file's own "non-tail dispatch
/// becomes an IIFE" idiom) guarding on the looked-up `MessageArg`'s `.tag`
/// before touching `.value` — TS narrows `__arg` to the matching variant
/// after the guard, so no cast is needed. Falls back to the literal
/// `"{name}"` text when the param is missing or the wrong `MessageArg`
/// variant, matching the plain-placeholder fallback's own convention.
///
/// Arc C, step (11) (#1388): `Plural`/`Number`/`Date` convert fully to real
/// `bynk_ts::TsExpr` nodes (`Call` over an `Arrow` — `Arrow` is already in
/// the printer's own `needs_parens_as_operand`, so the IIFE's own
/// `(...)(...)` wrapping comes free, no explicit `Paren`). `Select` stays
/// one opaque `TsExpr::Ident` — the only one of the four real shapes with a
/// BLOCK-bodied IIFE (at the time, `TsExpr::Arrow` had no block-body variant
/// at all, and extending it for this one real site was rejected as
/// disproportionate, the same "odd, one-off shape stays opaque text"
/// posture this track uses repeatedly), though its own arm VALUES are still
/// real `emit_sub_message` results, stringified back into that opaque host
/// text via the new `bynk_ts::print_expr` (#1388's own second real gap).
/// #1435 (Arc E slice 1) later gave `TsExpr::Arrow` a real block-body shape
/// (`TsArrowBody::Block`) for a different real site; this `Select` arm was
/// not reconverted along with it (out of that slice's own scope) and stays
/// exactly as it was.
fn emit_icu_placeholder(
    p: &icu::IcuPlaceholder<'_>,
    locale_tag: &str,
    runtime_use: &RuntimeUse,
) -> bynk_ts::TsExpr {
    // Recorded per-arm, not once up front: a `select` placeholder emits
    // `Object.hasOwn` over an arm table and no formatter at all, so noting here
    // would import the three helpers into a select-only bundle that never calls
    // them. The three are imported as a group, so one note per helper-emitting
    // arm is enough.
    let tag_lit_str = || bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(locale_tag.to_string()));
    let fallback = || bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(format!("{{{}}}", p.name)));
    let arg_expr = || bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("params".to_string())),
            property: "get".to_string(),
        }),
        args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
            p.name.to_string(),
        ))],
    };
    let arg_tag = |property: &str| bynk_ts::TsExpr::Member {
        object: Box::new(bynk_ts::TsExpr::Ident("__arg".to_string())),
        property: property.to_string(),
    };
    let iife = |arrow_body: bynk_ts::TsExpr| bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Arrow {
            params: vec![bynk_ts::TsParam::new("__arg".to_string(), None)],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(arrow_body))),
        }),
        args: vec![arg_expr()],
    };
    match &p.kind {
        icu::PlaceholderKind::Plural { arms } => {
            runtime_use.note_icu();
            let arms_obj: Vec<(String, bynk_ts::TsExpr)> = arms
                .iter()
                .map(|(cat, segs)| {
                    (
                        format!("\"{}\"", cat.as_str()),
                        emit_sub_message(segs, locale_tag, runtime_use),
                    )
                })
                .collect();
            // `__arg === undefined || (__arg.tag !== "Whole" && __arg.tag !== "Num")`
            // — the `&&` sub-expression is explicitly parenthesised in the
            // pre-conversion text even though `&&` already binds tighter
            // than `||` and needs no parens grammatically; reproduced via
            // an explicit `Paren`, matching the real bytes rather than the
            // precedence table's own (grammatically equivalent) choice.
            let test = bynk_ts::TsExpr::Binary {
                op: bynk_ts::TsBinaryOp::Or,
                left: Box::new(bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::StrictEq,
                    left: Box::new(bynk_ts::TsExpr::Ident("__arg".to_string())),
                    right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                }),
                right: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::And,
                    left: Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::StrictNotEq,
                        left: Box::new(arg_tag("tag")),
                        right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            "Whole".to_string(),
                        ))),
                    }),
                    right: Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::StrictNotEq,
                        left: Box::new(arg_tag("tag")),
                        right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            "Num".to_string(),
                        ))),
                    }),
                }))),
            };
            let call = bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__selectPluralArm".to_string())),
                args: vec![
                    tag_lit_str(),
                    arg_tag("value"),
                    bynk_ts::TsExpr::object(arms_obj),
                ],
            };
            iife(bynk_ts::TsExpr::Conditional {
                test: Box::new(test),
                consequent: Box::new(fallback()),
                alternate: Box::new(call),
            })
        }
        icu::PlaceholderKind::Select { arms } => {
            let arms_obj: Vec<String> = arms
                .iter()
                .map(|(k, segs)| {
                    format!(
                        "\"{}\": {}",
                        escape_ts_string(k),
                        bynk_ts::print_expr(&emit_sub_message(segs, locale_tag, runtime_use))
                    )
                })
                .collect();
            let key = format!("\"{}\"", escape_ts_string(p.name));
            let fallback_text = format!("\"{{{}}}\"", escape_ts_string(p.name));
            let arg = format!("params.get({key})");
            // `Object.hasOwn` (not `?? __arms["other"]`): the arm table is an
            // object literal, so a runtime `__arg.value` naming an
            // `Object.prototype` member (`"constructor"`, `"toString"`,
            // `"__proto__"`) would resolve off the prototype chain and never
            // reach the mandatory `other` arm — `??` only fires on
            // null/undefined, and an inherited method is neither (#900). The
            // dispatch's real question is own-property presence. `Object.hasOwn`
            // is ES2022, which `emit_tsconfig` already targets.
            //
            // Stays one opaque leaf — the only one of this cluster's 4 real
            // shapes with a BLOCK-bodied IIFE (see this function's own doc:
            // `TsArrowBody::Block` exists since #1435, but this arm was not
            // reconverted along with it) — now tagged `VerbatimExpr`, not
            // `Ident` (#1539).
            bynk_ts::TsExpr::VerbatimExpr(
                format!(
                    "((__arg) => {{ if (__arg === undefined || __arg.tag !== \"Text\") {{ return {fallback_text}; }} const __arms: globalThis.Record<string, string> = {{ {} }}; return globalThis.Object.hasOwn(__arms, __arg.value) ? __arms[__arg.value] : __arms[\"other\"]; }})({arg})",
                    arms_obj.join(", "),
                ),
                bynk_ts::VerbatimOrigin::Emit,
            )
        }
        icu::PlaceholderKind::Number { style } => {
            runtime_use.note_icu();
            let mut args = vec![tag_lit_str(), arg_tag("value")];
            if let Some(s) = style {
                args.push(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    s.as_str().to_string(),
                )));
            }
            iife(bynk_ts::TsExpr::Conditional {
                test: Box::new(bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::And,
                    left: Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::StrictNotEq,
                        left: Box::new(bynk_ts::TsExpr::Ident("__arg".to_string())),
                        right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                    }),
                    right: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::Or,
                        left: Box::new(bynk_ts::TsExpr::Binary {
                            op: bynk_ts::TsBinaryOp::StrictEq,
                            left: Box::new(arg_tag("tag")),
                            right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                "Whole".to_string(),
                            ))),
                        }),
                        right: Box::new(bynk_ts::TsExpr::Binary {
                            op: bynk_ts::TsBinaryOp::StrictEq,
                            left: Box::new(arg_tag("tag")),
                            right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                "Num".to_string(),
                            ))),
                        }),
                    }))),
                }),
                consequent: Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Ident("__formatIcuNumber".to_string())),
                    args,
                }),
                alternate: Box::new(fallback()),
            })
        }
        icu::PlaceholderKind::Date { style } => {
            runtime_use.note_icu();
            let mut args = vec![tag_lit_str(), arg_tag("value")];
            if let Some(s) = style {
                args.push(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    s.as_str().to_string(),
                )));
            }
            iife(bynk_ts::TsExpr::Conditional {
                test: Box::new(bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::And,
                    left: Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::StrictNotEq,
                        left: Box::new(bynk_ts::TsExpr::Ident("__arg".to_string())),
                        right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                    }),
                    right: Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::StrictEq,
                        left: Box::new(arg_tag("tag")),
                        right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            "Moment".to_string(),
                        ))),
                    }),
                }),
                consequent: Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Ident("__formatIcuDate".to_string())),
                    args,
                }),
                alternate: Box::new(fallback()),
            })
        }
    }
}

/// Turns one `plural`/`select` arm's sub-message into a `+`-joined TS
/// expression, evaluated inside `emit_icu_placeholder`'s IIFE where `__arg`
/// is already in scope and narrowed. `Hash` only ever occurs in a `plural`
/// arm (the parser's `allow_hash` guarantees a `select` arm never contains
/// one), where `__arg.value: number` after narrowing.
///
/// Arc C, step (11) (#1388): returns a real `bynk_ts::TsExpr` — `locale_tag`
/// (review of #1389: renamed from the pre-conversion `tag_lit`, which
/// pointed the wrong way once this stopped taking pre-quoted text) is now
/// the RAW locale tag, not pre-quoted/pre-escaped text, since `TsLit::Str`'s
/// own renderer applies the identical escaping itself (documented
/// byte-for-byte match with `escape_ts_string`).
fn emit_sub_message(
    segs: &[icu::SubSegment],
    locale_tag: &str,
    runtime_use: &RuntimeUse,
) -> bynk_ts::TsExpr {
    if segs.is_empty() {
        return bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(String::new()));
    }
    let parts: Vec<bynk_ts::TsExpr> = segs
        .iter()
        .map(|seg| match seg {
            icu::SubSegment::Literal(s) => bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(s.clone())),
            // A bare `#` renders the argument as a number. Recorded here even
            // though the only arm that can contain one is `plural` (see this
            // function's doc), which records for itself: the note belongs with
            // the emission, so this stays correct if that invariant ever moves.
            icu::SubSegment::Hash => {
                runtime_use.note_icu();
                bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Ident("__formatIcuNumber".to_string())),
                    args: vec![
                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(locale_tag.to_string())),
                        bynk_ts::TsExpr::Member {
                            object: Box::new(bynk_ts::TsExpr::Ident("__arg".to_string())),
                            property: "value".to_string(),
                        },
                    ],
                }
            }
        })
        .collect();
    join_plus(parts)
}

/// Compiles every `messages` block in one commons into one `code ->
/// renderer` lookup per locale, a shared dispatch object keyed by locale tag,
/// and one bundle-scoped `render(tag, msg)` that looks up the resolved
/// `tag`'s own table, else the `@reference` locale's, else falls back to
/// `bynk.locale`'s own `render` (imported under the private alias
/// `__bynkLocaleRender` — `emit_unit`, project.rs — since this file's own
/// `render` would otherwise collide with the imported name) for any code no
/// declared locale covers. Also exports the bundle's declared-locale set and
/// reference tag (message-bundles slice 2, #874, ADR 0273) — the
/// precondition Locale's own slice 2 (negotiation) needs before it can
/// start.
///
/// `blocks` is every `CommonsItem::Messages` in the commons, in source
/// order; `reference` is the one block among them carrying `@reference` —
/// the caller (`emit_project`) already found it to decide *whether* to call
/// this function at all, so it's passed in rather than re-derived here (PR
/// #875 review — a harmless but needless second scan).
/// #1355 (R7.1): converts this function's own outer construction to real
/// `bynk_ts` nodes, with two deliberate, named opaque carve-outs.
///
/// **`__messagesByLocale`'s own header/type-annotation/closing-brace stay
/// hand-written text** — `Record<string, Record<string, (params: ReadonlyMap
/// <string, MessageArg>) => string>>` needs its inner function type's one
/// parameter named `params`; `bynk_ts::TsType::Fn`'s own `params` are
/// deliberately anonymous, printer-numbered positionally (`a0`, `a1`, …) —
/// the same "an odd, one-off type shape stays opaque text" precedent P7.9
/// (#1315) already used for `Query[T]`'s own extra-paren-wrapped shape, not
/// a general `TsType::Fn` redesign for this one call site. Each LOCALE's own
/// entry (and each per-code entry nested inside it) IS a real
/// [`bynk_ts::TsObjectEntry`], though — printed through the shared
/// [`bynk_ts::print_object_entry`] fragment entry point directly into the
/// hand-written wrapper, the same "return/print real entries into a
/// still-hand-written enclosing literal" shape `emit_attached_methods`'s own
/// callers already use (#1337) — chosen here specifically because
/// `emit_doc_block`'s own per-locale doc comment has no real node shape to
/// intersperse with today (`TsObjectEntry::Prop` carries no `doc` field the
/// way `Method`'s own does; adding one for this single narrow need would be
/// disproportionate). Each per-code entry's own VALUE —
/// `emit_message_entry_renderer`'s output, one of step (11)'s own named,
/// not-yet-proposed ICU-formatting cluster — stays opaque, carried as a
/// [`bynk_ts::TsExpr::Ident`] wrapping already-formed JS, the established
/// "call an unconverted sibling helper, carry its text opaquely" pattern
/// this whole track uses.
///
/// **`__messagesReferenceLocale`/`__messagesLocales`/`render` convert fully** —
/// no opaque carve-outs, every shape they need (`TsExpr::As`, `TsExpr::
/// Index`, `TsExpr::Conditional`, `TsBinaryOp::NullishCoalescing`, `TsStmt::
/// If`/`Return`) already exists.
/// #1478: returns real [`bynk_ts::TsStmt`]s (was `out: &mut String`). The
/// `__messagesByLocale` header/entries/closing-brace block stays exactly the
/// opaque, hand-assembled text #1355's own doc above already carves out —
/// built into a local buffer instead of the caller's `out`, then carried as
/// one [`bynk_ts::TsStmtKind::Raw`] statement, the same "already real-node-
/// printed internally, still `String`-typed at its own call boundary"
/// carrier `emit_refined_type`'s own `checks_text` uses (#1478).
pub(crate) fn emit_messages_bundle(
    blocks: &[&MessagesDecl],
    reference: &MessagesDecl,
    runtime_use: &RuntimeUse,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();

    // One `code -> renderer` table per locale, inlined into a single
    // `__messagesByLocale` object literal keyed by tag. No per-locale `const
    // __messages_<tag>` binding: a locale tag can be `"pt-BR"`, which is not a
    // valid TS identifier, so a named binding would be a syntax error — the
    // object is keyed by the tag *string* and needs no binding of its own.
    let record = |value: bynk_ts::TsType| {
        bynk_ts::TsType::named_with_args(
            "globalThis.Record",
            vec![bynk_ts::TsType::named("string"), value],
        )
    };
    let renderer_ty = bynk_ts::TsType::Fn {
        params: vec![bynk_ts::TsType::named_with_args(
            "ReadonlyMap",
            vec![
                bynk_ts::TsType::named("string"),
                bynk_ts::TsType::named("__bynkMessageArg"),
            ],
        )],
        names: vec!["params".to_string()],
        ret: Box::new(bynk_ts::TsType::named("string")),
    };
    let locale_entries: Vec<bynk_ts::TsObjectEntry> = blocks
        .iter()
        .map(|m| {
            let code_entries: Vec<bynk_ts::TsObjectEntry> = m
                .entries
                .iter()
                .map(|entry| {
                    bynk_ts::TsObjectEntry::prop(
                        format!("\"{}\"", escape_ts_string(&entry.code)),
                        emit_message_entry_renderer(entry, &m.tag, runtime_use),
                    )
                })
                .collect();
            let locale_entry = bynk_ts::TsObjectEntry::prop(
                format!("\"{}\"", escape_ts_string(&m.tag)),
                bynk_ts::TsExpr::multiline_object_entries(code_entries),
            );
            match m.documentation.as_deref() {
                Some(doc) => locale_entry.with_doc(doc),
                None => locale_entry,
            }
        })
        .collect();
    stmts.push(bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::ConstDecl {
            name: "__messagesByLocale".to_string(),
            ty: Some(record(record(renderer_ty))),
            init: bynk_ts::TsExpr::multiline_object_entries(locale_entries),
        },
        None,
    ));

    // `("tag" as string) as LocaleTag` — the inner `as` is wrapped in an
    // explicit `Paren`: `as` is left-associative, so the un-parenthesised
    // `"tag" as string as LocaleTag` is the identical cast grammatically,
    // but the pre-conversion text always parenthesised it, and `As`'s own
    // renderer does not auto-add parens around a nested `As` operand. The
    // same "explicit `Paren` always prints its own literal parens"
    // precedent #1323 established.
    //
    // Review of #1356, finding 1: `tag` is the RAW locale tag, not
    // `escape_ts_string`-escaped — `TsLit::Str`'s own renderer already
    // escapes (byte-identical to `escape_ts_string`, deliberately, P7.8),
    // so pre-escaping here would double-escape any backslash/quote a tag
    // ever contains, corrupting the literal's own decoded value. Not
    // reachable today (`LocaleTag`'s own `Matches(...)` refinement rejects
    // anything but letters/digits/hyphens) but a real, latent bug had the
    // caller pre-escaped, the same class of hazard #1335's own `msg`
    // deviation was written to avoid. `__messagesByLocale`'s own `Prop` KEY a
    // few lines up is the opposite, correct case: a key is spliced
    // verbatim, never run through a printer escaper, so it needs the
    // explicit `escape_ts_string` call to be safe at all.
    let tag_cast = |tag: &str| bynk_ts::TsExpr::As {
        expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
            expr: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(tag.to_string()))),
            ty: bynk_ts::TsType::named("string"),
        }))),
        ty: bynk_ts::TsType::named("__bynkLocaleTag"),
    };
    let ref_locale_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: "__messagesReferenceLocale".to_string(),
            ty: Some(bynk_ts::TsType::named("__bynkLocaleTag")),
            init: tag_cast(&reference.tag),
        })),
        None,
    );
    stmts.push(ref_locale_decl);

    let locale_list: Vec<bynk_ts::TsExpr> = blocks.iter().map(|m| tag_cast(&m.tag)).collect();
    // #1486: no blank wanted between `ref_locale_decl` and `locales_decl` —
    // matches the pre-conversion hand-written text this crate's own
    // zero-diff discipline requires preserving exactly, not expressible
    // through `bynk_ts::print`'s own uniform "always one blank" policy
    // without this escape hatch (see `TsStmt::no_blank_before`'s own doc).
    let mut locales_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: "__messagesLocales".to_string(),
            ty: Some(bynk_ts::TsType::readonly_array(bynk_ts::TsType::named(
                "__bynkLocaleTag",
            ))),
            init: bynk_ts::TsExpr::array(locale_list),
        })),
        None,
    );
    locales_decl.no_blank_before = true;
    stmts.push(locales_decl);

    let ident = |s: &str| bynk_ts::TsExpr::Ident(s.to_string());
    let member = |object: bynk_ts::TsExpr, property: &str| bynk_ts::TsExpr::Member {
        object: Box::new(object),
        property: property.to_string(),
    };
    let index = |object: bynk_ts::TsExpr, index: bynk_ts::TsExpr| bynk_ts::TsExpr::Index {
        object: Box::new(object),
        index: Box::new(index),
    };
    let not_undefined = |e: bynk_ts::TsExpr| bynk_ts::TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::StrictNotEq,
        left: Box::new(e),
        right: Box::new(ident("undefined")),
    };

    let local_table_decl = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__localeTable".to_string()),
        None,
        index(ident("__messagesByLocale"), ident("tag")),
        None,
    );
    let reference_table_decl = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__referenceTable".to_string()),
        None,
        index(
            ident("__messagesByLocale"),
            ident("__messagesReferenceLocale"),
        ),
        None,
    );
    let entry_ternary = bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Conditional {
        test: Box::new(not_undefined(ident("__localeTable"))),
        consequent: Box::new(index(ident("__localeTable"), member(ident("msg"), "code"))),
        alternate: Box::new(ident("undefined")),
    }));
    let entry_decl = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__entry".to_string()),
        None,
        bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::NullishCoalescing,
            left: Box::new(entry_ternary),
            right: Box::new(index(
                ident("__referenceTable"),
                member(ident("msg"), "code"),
            )),
        },
        None,
    );
    let if_entry = bynk_ts::TsStmt::if_stmt(
        not_undefined(ident("__entry")),
        bynk_ts::TsStmt::block(
            vec![bynk_ts::TsStmt::return_stmt(
                Some(bynk_ts::TsExpr::Call {
                    callee: Box::new(ident("__entry")),
                    args: vec![member(ident("msg"), "params")],
                }),
                None,
            )],
            None,
        ),
        None,
    );
    let fallback_return = bynk_ts::TsStmt::return_stmt(
        Some(bynk_ts::TsExpr::Call {
            callee: Box::new(ident("__bynkLocaleRender")),
            args: vec![ident("tag"), ident("msg")],
        }),
        None,
    );

    let render_fn = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Function {
            name: "render".to_string(),
            generics: Vec::new(),
            params: vec![
                bynk_ts::TsParam::new(
                    "tag".to_string(),
                    Some(bynk_ts::TsType::named("__bynkLocaleTag")),
                ),
                bynk_ts::TsParam::new(
                    "msg".to_string(),
                    Some(bynk_ts::TsType::named("__bynkMessage")),
                ),
            ],
            return_type: Some(bynk_ts::TsType::named("string")),
            body: vec![
                local_table_decl,
                reference_table_decl,
                entry_decl,
                if_entry,
                fallback_return,
            ],
            is_async: false,
            inline: false,
        })),
        None,
    );
    stmts.push(render_fn);
    stmts
}

// -- v0.5 emission --

/// P6.x (#1193, slice 3 of #1187): reads each op's resolved
/// `params`/`return_ty` off `ops` ([`bynk_model::OpSig`], from
/// `bynk_model::lower_capability_ops_ir`) through `ts_ty`, instead of
/// walking `CapabilityOp::params`/`return_type` `TypeRef`s through
/// `ts_type_ref` directly. `c` is still needed alongside `ops` — an `OpSig`
/// carries neither `c.documentation`/`op.documentation` nor the `Arc` back
/// to the declaration #1188 could rely on for `Type` ([DECISION A]).
/// `ops`/`c.ops` are zipped by index: both are built in declaration order
/// (`lower_capability_ops_ir` maps `cap.ops` in place), never reordered by
/// either side.
///
/// Accepted divergence (review of #1194, widening #1193's own disclosure):
/// `resolve_type_ref` returns `None` — falling back to `Unit`/`void` here —
/// for two shapes `ts_type_ref` rendered directly before this slice: an
/// unresolvable type name (`void` instead of the raw bad name) and
/// `TypeRef::History` (`void` instead of `ts_type_ref`'s own `"never"`,
/// `emitter.rs:4127`). Both are reachable only through a capability op,
/// since the resolver skips `CommonsItem::Capability` outright — no program
/// the checker actually validates can reach either.
/// #1357 (R7.1): both declarations this function builds are now real
/// `bynk_ts` nodes. `export interface {Name} { op<T>(params): ret; ... }` is
/// a real [`bynk_ts::TsDecl::Interface`] over real, per-op
/// [`bynk_ts::TsTypeMember::Method`] entries — `generics`/`doc` are this
/// slice's own real gap (bare names for `generics`, matching every other
/// real generics-list precedent in this crate; `doc` mirrors
/// [`bynk_ts::TsObjectEntry::Method.doc`]'s own identical field, #1337) —
/// params route through the already-real [`ts_ty_to_ts_type`] (P7.9,
/// #1315) instead of the opaque pre-printed `String` `ts_ty` returns. The
/// injection token (`export const __{Name}Token: symbol =
/// globalThis.Symbol("{Name}");`, #1653) is a real
/// [`bynk_ts::TsDecl::ConstDecl`]. This function's own exact signature is
/// unchanged, the P7.9/step-1 pattern — it never owned a `Verbatim`
/// construction site.
/// #1478: real-node-internally already — returns its two real declarations
/// (an interface, an injection-token const) directly instead of writing into
/// `out: &mut String`.
pub(crate) fn emit_capability(
    c: &CapabilityDecl,
    ops: &[OpSig],
    commons: &TypedCommons,
    unit: UnitFacts<'_>,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    if let Some(doc) = c.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    // Review of #1194: `zip` silently truncates to the shorter side — this
    // guards the by-index pairing invariant `ops`/`c.ops` are documented
    // (not enforced) to share, so a future second caller that passes a
    // mismatched `ops` loses trailing interface methods loudly instead of
    // silently.
    debug_assert_eq!(
        c.ops.len(),
        ops.len(),
        "`ops` is zipped with `c.ops` by index — the two must be the same lowering"
    );
    let members: Vec<bynk_ts::TsTypeMember> = c
        .ops
        .iter()
        .zip(ops)
        .map(|(op, sig)| {
            // Constrained generics slice 3 (DECISION D): a parameter whose type
            // mentions a `Codable` type parameter is encoded whole at the call
            // site, so the provider receives a JSON value, never a witness.
            let codable = bynk_check::checker::codable::codable_op_params(op);
            let encoded: Vec<bool> = op
                .params
                .iter()
                .map(|p| bynk_check::checker::codable::mentions_any(&p.type_ref, &codable))
                .collect();
            let params: Vec<bynk_ts::TsParam> = sig
                .params
                .iter()
                .zip(&encoded)
                .map(|((name, ty), encoded)| {
                    bynk_ts::TsParam::new(
                        ts_ident(name),
                        Some(if *encoded {
                            bynk_ts::TsType::named("unknown")
                        } else {
                            ts_ty_to_ts_type(*ty, unit.render(commons))
                        }),
                    )
                })
                .collect();
            // #3458: a typed read's `Effect[P]` payload mentions a `Codable`
            // parameter, so the provider replies with plain JSON that the call
            // site decodes: the interface returns `unknown` there.
            let typed_read = matches!(
                &op.return_type,
                bynk_syntax::ast::TypeRef::Effect(payload, _)
                    if bynk_check::checker::codable::mentions_any(payload, &codable)
            );
            // A type parameter that only occurs in encoded parameters is not in
            // the interface (nothing would use it).
            let kept: Vec<String> = op
                .type_params
                .iter()
                .map(|tp| tp.name.name.clone())
                .filter(|tp| {
                    codable.is_empty()
                        || op
                            .params
                            .iter()
                            .zip(&encoded)
                            .filter(|(_, enc)| !**enc)
                            .any(|(p, _)| {
                                bynk_check::checker::codable::mentions_any(
                                    &p.type_ref,
                                    std::slice::from_ref(tp),
                                )
                            })
                        || (!typed_read
                            && bynk_check::checker::codable::mentions_any(
                                &op.return_type,
                                std::slice::from_ref(tp),
                            ))
                })
                .collect();
            // #926 (Decision C): a genuine generic TS interface method, no
            // monomorphisation/erasure. Reads `op.type_params` (not
            // `sig.type_params`): it only extracts each `TypeParam`'s own
            // name, no type resolution, and `OpSig::type_params` exists for
            // `lower_op_sig_ir`'s own rigid-variable scoping, not for this
            // to render from (Decision A/B, #1193).
            bynk_ts::TsTypeMember::Method {
                name: op.name.name.clone(),
                generics: kept,
                params,
                ret: if typed_read {
                    bynk_ts::TsType::named_with_args(
                        "globalThis.Promise",
                        vec![bynk_ts::TsType::named("unknown")],
                    )
                } else {
                    ts_ty_to_ts_type(sig.return_ty, unit.render(commons))
                },
                doc: op.documentation.as_deref().map(str::to_owned),
            }
        })
        .collect();
    // #1486: `no_blank_before` when this capability's own doc comment is the
    // preceding statement — see `emit_free_fn`'s own identical note. The two
    // explicit `TsStmt::blank(None)` pushes this function used to carry
    // (after `interface`, after `token_decl`) are gone for the same reason
    // `emit_refined_type`'s own note gives.
    let mut interface = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Interface {
            name: c.name.name.clone(),
            type_params: Vec::new(),
            members,
        })),
        None,
    );
    interface.no_blank_before = c.documentation.is_some();
    stmts.push(interface);

    // Injection token (symbol carrying the interface type). #1653: typed
    // `symbol`, not `unique symbol` — TypeScript only gives a bare `Symbol(…)`
    // call a unique-symbol type, and the call is qualified as
    // `globalThis.Symbol` so a user type named `Symbol` cannot hide it.
    let token_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: format!("__{}Token", c.name.name),
            ty: Some(bynk_ts::TsType::named("symbol")),
            init: bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.Symbol".to_string())),
                args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    c.name.name.clone(),
                ))],
            },
        })),
        None,
    );
    stmts.push(token_decl);
    stmts
}

/// S4a (#1966): a derived capability (`capability Sessions = Kv`) is its
/// base's interface under its own name, `export type Sessions =
/// bynk_cloudflare.Kv;`, so everything typed by the capability (a handler's
/// `deps`, the context's deps interface, a test double) has the base's ops,
/// and compose's base provider satisfies it. The injection token is the same
/// as a declared capability's.
pub(crate) fn emit_derived_capability(
    c: &CapabilityDecl,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    let Some(derived) = &c.derived else {
        return Vec::new();
    };
    let base = &derived.base.name;
    // The checker resolved the base among the flattened capabilities
    // (`bynk.capability.unknown_base`), so a miss is a broken build already
    // reported; the bare name keeps the output printable.
    let ty = match ctx.cross_context.flattened_caps.get(base) {
        Some(unit) => format!("{}.{base}", qualified_to_ns(unit)),
        None => base.clone(),
    };
    let mut stmts = Vec::new();
    if let Some(doc) = c.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let mut alias = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::TypeAlias {
            name: c.name.name.clone(),
            type_params: Vec::new(),
            ty: bynk_ts::TsType::named(ty),
        })),
        None,
    );
    alias.no_blank_before = c.documentation.is_some();
    stmts.push(alias);
    stmts.push(bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: format!("__{}Token", c.name.name),
            ty: Some(bynk_ts::TsType::named("symbol")),
            init: bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.Symbol".to_string())),
                args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    c.name.name.clone(),
                ))],
            },
        })),
        None,
    ));
    stmts
}

#[cfg(test)]
mod emit_capability_tests {
    /// Review of #1358, finding 2: the printer-side test
    /// (`bynk-ts::printer::tests::prints_a_generic_documented_interface_method`)
    /// proves `TsTypeMember::Method.doc` renders correctly, but not that
    /// `emit_capability`'s own `doc: op.documentation.clone()` wiring
    /// actually threads a real op's doc comment there — the one piece of
    /// this slice's own conversion with no real fixture in the corpus
    /// exercising it. Closes that gap directly: a real op-level doc
    /// comment, generic op, through the real emission pipeline.
    #[test]
    fn a_generic_capability_ops_own_doc_comment_reaches_the_interface() {
        // `capability` only parses inside a `context` (`emit_source`'s own
        // single-file path is `commons`-only) — drives `compile_in_memory`
        // directly instead, the same fs-free seam `emit_bundle` uses one
        // layer up.
        let src = r#"
context demo

capability Clock {
  ---
  Returns the current time.
  ---
  fn now[T](unit: T) -> T
}
"#;
        let out = match bynk_driver::project::compile_in_memory(
            src,
            bynk_check::project_model::BuildTarget::Bundle,
            Default::default(),
        ) {
            Ok(out) => out,
            Err(_) => panic!("fixture should compile:\n{src}"),
        };
        let ts = out
            .artefacts
            .docs
            .iter()
            .find(|(path, _)| path.to_string_lossy().contains("demo"))
            .map(|(_, doc)| doc.text())
            .expect("the context's own module should be in the output");
        assert!(
            ts.contains(
                "export interface Clock {\n  \
                 /**\n   \
                 * Returns the current time.\n   \
                 */\n  \
                 now<T>(unit: T): T;\n\
                 }\n"
            ),
            "{ts}"
        );
    }
}

/// A provider's declarations (1A.32a, Decision A of #3733): its doc comment, if any, then
/// `export class P implements Cap { .. }` as a [`bynk_ts::TsDecl::Class`] over typed members, then
/// the `export const __PProvider = { token, factory }` registration. `None` for an external
/// (bodiless) provider, which the adapter's binding supplies.
///
/// Source maps come from the printer alone: each op's [`bynk_ts::TsClassMethod::span`] anchors its
/// header and prologue to its own `fn` line (#2280), the class's `end_span` anchors the closing
/// brace to the provider, and the registration carries the provider's span. The caller gives the
/// first statement the provider's span, as for every other item.
pub(crate) fn emit_provider(
    p: &ProviderDecl,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Option<Vec<bynk_ts::TsStmt>> {
    // v0.17: an external (bodiless) provider inside an adapter is supplied by
    // the adapter's binding — the compiler emits no class for it. Its symbol is
    // imported and constructed by the consumer's compose (§6.1, Phase 2).
    if p.external {
        return None;
    }
    // v0.12: a provider with `given` receives its dependencies through a
    // constructor; its bodies call them as `this.deps.<cap>`. The deps object
    // type lists exactly the provider's `given` capabilities.
    //
    // P7.2 (review of #1300): computed once here and reused below (the
    // capability-scope collection in each op, and the factory's own `deps` type).
    let given_ir = bynk_model::lower_provider_given_ir(p);
    // #2355: a provider whose ops construct a local agent hands `this.deps` to
    // the agent's methods, so it takes a `deps` even with no `given`; on
    // `workers` that `deps` also carries the Worker's `env`, which the agent
    // factory reads its Durable Object namespace from.
    let constructs_agent =
        bynk_check::project_model::provider_constructs_agent(p, |n| ctx.local_agents.contains(n));
    let takes_deps = !p.given.is_empty() || constructs_agent;
    let mut deps_members: Vec<bynk_ts::TsTypeMember> = given_ir
        .iter()
        .map(|c| bynk_ts::TsTypeMember::prop(c.name.clone(), cap_ref_ty(c, &ctx.cross_context)))
        .collect();
    if constructs_agent && matches!(ctx.target, BuildTarget::Workers) {
        let mut agents: Vec<String> = ctx.local_agents.iter().cloned().collect();
        agents.sort();
        deps_members.push(bynk_ts::TsTypeMember::prop(
            "env",
            workers_env_ty(&[], &agents),
        ));
    }
    let deps_ty = bynk_ts::TsType::Object(deps_members);
    let prov = p.provider_name.name.as_str();
    let mut class = bynk_ts::TsDecl::class(
        prov,
        Vec::new(),
        None,
        p.ops
            .iter()
            .map(|op| provider_op_method(p, op, &given_ir, takes_deps, commons, ctx))
            .collect(),
    )
    .implementing(vec![bynk_ts::TsType::named(p.capability.name.as_str())])
    .ending_at(p.span);
    if takes_deps
        && let bynk_ts::TsDecl::Class {
            fields,
            constructor,
            ..
        } = &mut class
    {
        // The field is declared separately and assigned in the constructor body
        // rather than written as a parameter property (`constructor(private deps:
        // …)`). A parameter property is the one type-directed construct pure
        // type-stripping cannot erase — Node's `--experimental-strip-types` throws
        // `ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX` on it — so this de-sugared form keeps
        // the emitter's whole surface strip-removable (the strip-only emission
        // invariant, ADR 0136). The field stays typed for the `tsc`/strict path,
        // and under ES2022 `useDefineForClassFields` the declaration defines
        // `this.deps` before the body assigns the real value; the end state is
        // correct and strips to `constructor(deps) { this.deps = deps; }`.
        fields.push(bynk_ts::TsClassField::new("deps", deps_ty.clone()).private());
        *constructor = Some(bynk_ts::TsClassCtor {
            params: vec![deps_param(&deps_ty)],
            body: vec![bynk_ts::TsStmt::assign(
                bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
                    property: "deps".to_string(),
                },
                bynk_ts::TsExpr::Ident("deps".to_string()),
                None,
            )],
        });
    }
    let mut stmts = Vec::new();
    let has_doc = p.documentation.is_some();
    if let Some(doc) = p.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let mut class_stmt = bynk_ts::TsStmt::decl(bynk_ts::TsDecl::Export(Box::new(class)), None);
    // A leading doc comment sits directly above the declaration it documents.
    class_stmt.no_blank_before = has_doc;
    stmts.push(class_stmt);
    // The registration: `token` names the capability, `factory` builds the class from
    // its dependencies. Its own line belongs to the provider, not to the last op.
    let construct = |args: Vec<bynk_ts::TsExpr>| bynk_ts::TsExpr::New {
        callee: Box::new(bynk_ts::TsExpr::Ident(prov.to_string())),
        args,
    };
    let factory = bynk_ts::TsExpr::Arrow {
        params: if takes_deps {
            vec![deps_param(&deps_ty)]
        } else {
            Vec::new()
        },
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(construct(
            if takes_deps {
                vec![bynk_ts::TsExpr::Ident("deps".to_string())]
            } else {
                Vec::new()
            },
        )))),
    };
    let token = format!("__{}Token", p.capability.name);
    stmts.push(bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: format!("__{prov}Provider"),
            ty: None,
            init: bynk_ts::TsExpr::object(vec![
                ("token".to_string(), bynk_ts::TsExpr::Ident(token)),
                ("factory".to_string(), factory),
            ]),
        })),
        Some(p.span),
    ));
    Some(stmts)
}

/// `deps: <deps type>`: the one parameter of a provider's constructor and of its factory.
fn deps_param(deps_ty: &bynk_ts::TsType) -> bynk_ts::TsParam {
    bynk_ts::TsParam {
        name: "deps".to_string(),
        ty: Some(deps_ty.clone()),
        optional: false,
        rest: false,
    }
}

/// One provider op as a class method, its body lowered through
/// [`function_body_stmts`]. #2280: the method's span anchors its header and prologue to its
/// own `fn` line rather than the previous op's last body line.
fn provider_op_method(
    p: &ProviderDecl,
    op: &bynk_syntax::ast::ProviderOp,
    given_ir: &[bynk_model::CapRefIr],
    takes_deps: bool,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> bynk_ts::TsClassMethod {
    let params: Vec<bynk_ts::TsParam> = op
        .params
        .iter()
        .map(|p| {
            bynk_ts::TsParam::new(
                ts_ident(&p.name.name),
                Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
            )
        })
        .collect();
    // P6.55 (design/tracks/the-ir.md §6b): computed once, not twice.
    let effectful = is_effectful_return(&op.return_type);
    let mut module = ModuleCtx::for_unit(commons, ctx);
    module.agent_method_givens = ctx.agent_method_givens.clone();
    module.agent_called_contexts = ctx.agent_called_contexts.clone();
    module.event_schema_versions = ctx.event_schema_versions.clone();
    module.consumed_type_ns = consumed_type_ns(ctx);
    module.consumed_type_declared = ctx.consumed_type_aliases.clone();
    module.consumed_context_types = ctx.consumed_context_types();
    module.target = ctx.target;
    module.note_unit(ctx);
    let mut cx = LowerCtx::new(
        module,
        BodyMode::ProviderOp {
            handler: HandlerShared {
                // The provider's `given` capabilities are in scope in its
                // bodies, and resolve against the injected `this.deps`.
                capabilities: given_ir.iter().map(|c| c.name.clone()).collect(),
                deps_on_this: takes_deps,
                // #934: a provider's own op body can itself call a `given`
                // capability (e.g. a provider that dedups its own work via
                // `Idempotency`).
                handler_scope: Some(format!(
                    "{}.provides.{}.{}",
                    ctx.commons_name, p.capability.name, op.name.name
                )),
                owning_context: ctx.commons_name.clone(),
                ..HandlerShared::default()
            },
        },
    );
    cx.local_agents = ctx.local_agents.clone();
    // #1653: parameters are bindings of the body's own scope, so a `let` that
    // re-binds one gets a fresh name instead of redeclaring it.
    for p in &op.params {
        cx.declare_binder(&p.name.name);
    }
    let body = function_body_stmts(
        &op.body,
        &mut cx,
        INDENT_STEP * 2,
        effectful,
        Some(&op.return_type),
    );
    bynk_ts::TsClassMethod {
        span: Some(op.span),
        ..bynk_ts::TsClassMethod::new(
            op.name.name.clone(),
            effectful,
            params,
            Some(ts_type_ref_to_ts_type(&op.return_type, None)),
            body,
        )
    }
}

/// Append one field to a `deps` object type under construction —
/// `emit_service`'s own actor-seam/`__exec`/events-dispatch widening all
/// need this. Review of #1209: gathering the actor-seam widening into one
/// `match` (below) made this splice's own repetition newly visible — six
/// copies in `emit_service` alone before this helper.
///
/// Arc F, slice 4 (#1463): widening is now a plain `Vec::push`, replacing the
/// `trim_end_matches('}')` string splice this helper used before — genuinely
/// safer, not just tidier (review of #1469): `trim_end_matches` strips *all*
/// trailing braces, so the old splice only produced the right answer because
/// a nested object field always happened to end with a space before its own
/// closing brace — `__exec`'s own `{ waitUntil(...): void }` shape was one
/// stray space away from silently losing a brace. Takes the member list
/// directly (review of #1469) rather than a `TsType` that would only ever be
/// the `Object` variant — no runtime check, and no panic path, for a shape
/// mismatch that can't happen.
fn append_deps_field(
    deps_members: &mut Vec<bynk_ts::TsTypeMember>,
    name: impl Into<String>,
    ty: bynk_ts::TsType,
) {
    deps_members.push(bynk_ts::TsTypeMember::prop(name, ty));
}

/// The type of `deps.identity` for a `Caller` or `Sender` handler. For `Sender`
/// (#3328) it is the structural type of the minted `SenderIdentity`. Structural, so the handler module needs no import of the
/// `bynk.cloudflare` declaration the checker typed `s.identity` by, and the
/// declared interface is assignable to it.
fn seam_identity_ts_type(seam: &ActorSeamIr) -> bynk_ts::TsType {
    if !matches!(seam, ActorSeamIr::Sender(_)) {
        // `Caller`: the calling context's name.
        return bynk_ts::TsType::named("string");
    }
    bynk_ts::TsType::Object(vec![
        bynk_ts::TsTypeMember::readonly_prop("address", bynk_ts::TsType::named("string")),
        bynk_ts::TsTypeMember::readonly_prop(
            "dmarc",
            // #3397: the `DmarcVerdict` union itself, not `{ tag: string }`,
            // so `s.identity.dmarc` passes where a `DmarcVerdict` is expected.
            // Keep the variants in step with `DmarcVerdict` in
            // `bynk-std/lib/cloudflare/bynk.cloudflare.bynk` and the runtime's
            // `email.ts`.
            bynk_ts::TsType::union(
                ["DmarcPass", "DmarcFail", "DmarcNone"]
                    .into_iter()
                    .map(|v| {
                        bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::readonly_prop(
                            "tag",
                            bynk_ts::TsType::named(format!("\"{v}\"")),
                        )])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// A service's declarations (1A.32a, Decision A of #3733): its doc comment, if any, then
/// `export const {name} = { .. };` as a [`bynk_ts::TsDecl::ConstDecl`] over a multiline object
/// with one [`bynk_ts::TsObjectEntry::Method`] per handler.
///
/// Source maps come from the printer alone: each handler's entry span anchors its header and
/// prologue to its own `on call` line (#2280), and the object's `end_span` anchors the closing
/// `};` to the service. The caller gives the first statement the service's span, as for every
/// other item.
pub(crate) fn emit_service(
    s: &ServiceDecl,
    protocol: &ProtocolIr,
    signatures: &[HandlerSignatureIr],
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    let tys = UnitFacts::of(ctx).render(commons);
    let mut entries: Vec<bynk_ts::TsObjectEntry> = Vec::new();
    let mut cron_idx = 0usize;
    let mut queue_idx = 0usize;
    let ws_proto = matches!(protocol, ProtocolIr::WebSocket { .. });
    // #1187's slice 5: `s.handlers`/`signatures` are the same list in the
    // same declaration order — `signatures` is built by mapping `s.handlers`
    // 1:1 at the one call site (`emitter.rs`) — the same zip-by-index
    // precedent `emit_capability`'s own `c.ops`/`ops` pairing already
    // established (#1193).
    for (handler, (ir_params, _ir_given, ir_ret, ir_effectful)) in s.handlers.iter().zip(signatures)
    {
        // v0.104/v0.106 (real-time track slice 3b): on Workers a `from websocket`
        // lifecycle handler (`on open`/`on message`/`on close`) does not emit a
        // service-surface method — its body runs inside the hosting Durable Object
        // (`__wsOpen`/`__wsMessage`/`__wsClose`, DECISION A), driven by the edge
        // upgrade / `webSocketMessage` / `webSocketClose`, not from here. (On bundle
        // these are callable surface methods a `TestConnection` test drives.)
        let handler_kind_ir = lower_handler_kind_ir(&handler.kind);
        let is_ws_handler = matches!(handler_kind_ir, IrHandlerKind::Open | IrHandlerKind::Close)
            || (ws_proto && matches!(handler_kind_ir, IrHandlerKind::Message));
        // Events track, slice 0 (spine #936): unlike the WebSocket lifecycle
        // handlers above, `on event` has no *other* place its body could go
        // on Workers — a WS lifecycle handler's real body lives in the
        // hosting agent's DO (`emit_ws_do_method`); a subscriber service has
        // no such alternate home. So the method is still emitted here on
        // every target — real delivery (the fan-out DO calling in) reaches
        // it directly, not through `compose.ts`'s HTTP-routable surface,
        // which never exposes it regardless (no HTTP method/route to route
        // from).
        if is_ws_handler && matches!(ctx.target, BuildTarget::Workers) {
            continue;
        }
        // #1361: the doc block is no longer written directly here — it now
        // lives on the real `TsObjectEntry::Method` built below (`doc:
        // handler.documentation.clone()`), rendered by `render_multiline_
        // object_entry`'s own `Method` arm at the right depth/position.
        let kind_name = match &handler_kind_ir {
            IrHandlerKind::Call => "call".to_string(),
            IrHandlerKind::Http { method, path } => http_handler_method_name_ir(*method, path),
            IrHandlerKind::Cron { .. } => {
                let name = cron_handler_method_name(&s.name.name, cron_idx);
                cron_idx += 1;
                name
            }
            // v0.106: a `from websocket` `on message` is the inbound surface method.
            IrHandlerKind::Message if ws_proto => "message".to_string(),
            IrHandlerKind::Message => {
                let name = queue_handler_method_name(&s.name.name, queue_idx);
                queue_idx += 1;
                name
            }
            // v0.103/v0.106: the WebSocket lifecycle surface methods.
            IrHandlerKind::Open => "open".to_string(),
            IrHandlerKind::Close => "close".to_string(),
            // Events track, slice 0 (spine #936): exactly one `on event` per
            // `from Events(E)` service.
            IrHandlerKind::Event => "event".to_string(),
            // #2341 (decision C): the router's rejection renderer.
            IrHandlerKind::Rejected => "rejected".to_string(),
        };
        // For service handlers the operation name is the handler kind
        // (e.g. `call`). v0.5 has only one handler kind, so the service is a
        // single-operation object literal.
        let mut params: Vec<bynk_ts::TsParam> = ir_params
            .iter()
            .map(|(name, ty)| {
                bynk_ts::TsParam::new(ts_ident(name), Some(ts_ty_to_ts_type(*ty, tys)))
            })
            .collect();
        // v0.103/v0.106: a `from websocket` lifecycle handler receives the
        // `connection` as its first parameter (the synthetic binding the checker
        // added — the fresh socket for `on open`, the firing socket for `on
        // message`/`on close`); emit it so the lowered body's `connection` resolves.
        if is_ws_handler && let ProtocolIr::WebSocket { out_ty, .. } = protocol {
            params.insert(
                0,
                bynk_ts::TsParam::new(
                    "connection".to_string(),
                    Some(bynk_ts::TsType::named_with_args(
                        "Connection",
                        vec![ts_ty_to_ts_type(*out_ty, tys)],
                    )),
                ),
            );
        }
        // Events track, slice 4 (spine #936): a `via schema(N)` guard needs
        // `env.schemaVersion` regardless of whether the subscriber declared
        // the optional `env: EventEnvelope` parameter (slice 2) — a bare
        // `on event(e: E)` handler has no envelope in scope otherwise. When
        // missing, insert a synthetic parameter in the same position a real
        // `env` would occupy, under a name distinct from anything a user
        // could write, so it can never collide with a declared one. The two
        // envelope-forwarding call sites (`workers.rs`, `project.rs`) widen
        // their own condition to match, so the value actually arrives here.
        let schema_dispatch_env_binder = if handler_kind_ir == IrHandlerKind::Event
            && let ProtocolIr::Events {
                schema_dispatch: Some(_),
                ..
            } = protocol
        {
            match ir_params.get(1) {
                Some((env_param_name, _)) => Some(ts_ident(env_param_name)),
                None => {
                    params.insert(
                        1,
                        bynk_ts::TsParam::new(
                            "__bynkSchemaEnv".to_string(),
                            Some(bynk_ts::TsType::named("EventEnvelope")),
                        ),
                    );
                    Some("__bynkSchemaEnv".to_string())
                }
            }
        } else {
            None
        };
        // Lower the body first so we can detect cross-context usage and
        // adjust the deps shape accordingly.
        // v0.52: a multi-actor sum handler's resolved actor is threaded through
        // `deps.who`; the binder ident lowers to it so the body can `match`. A
        // sum supersedes the single-actor Bearer identity path (the per-arm
        // identity comes from the match, not a single `deps.identity`).
        // v0.47/v0.151/v0.52/v0.54: a handler's own actor-verification seam —
        // Bearer/Oidc/Caller thread their identity through `deps.identity`,
        // a sum threads the resolved-actor tagged union through `deps.who`
        // (the body `match`es it). `lower_actor_seam_ir`'s own doc comment
        // (`bynk_model::ActorSeamIr`) has the full grounding for why sum is
        // tried first (it can otherwise collide with Bearer) and why the
        // other three are mutually exclusive by construction.
        let seam = lower_actor_seam_ir(handler, &ctx.actors);
        let deps_identity_binder = match &seam {
            ActorSeamIr::Caller(binder) | ActorSeamIr::Sender(binder) => Some(binder.clone()),
            ActorSeamIr::Bearer(s) => s.binder.clone(),
            ActorSeamIr::Oidc(s) => s.binder.clone(),
            ActorSeamIr::SignedCookie(s) => s.binder.clone(),
            ActorSeamIr::Sum(_) | ActorSeamIr::CallSum(_) | ActorSeamIr::None => None,
        };
        let actor_sum_binder = if matches!(seam, ActorSeamIr::Sum(_) | ActorSeamIr::CallSum(_)) {
            handler
                .by_clause
                .as_ref()
                .and_then(|by| by.binder.as_ref())
                .map(|binder| binder.name.clone())
        } else {
            None
        };
        let mut module = ModuleCtx::for_unit(commons, ctx);
        module.note_unit(ctx);
        module.agent_method_givens = ctx.agent_method_givens.clone();
        module.agent_called_contexts = ctx.agent_called_contexts.clone();
        module.event_schema_versions = ctx.event_schema_versions.clone();
        module.consumed_type_ns = consumed_type_ns(ctx);
        module.consumed_type_declared = ctx.consumed_type_aliases.clone();
        module.consumed_context_types = ctx.consumed_context_types();
        module.target = ctx.target;
        let mut cx = LowerCtx::new(
            module,
            BodyMode::ServiceHandler {
                handler: HandlerShared {
                    capabilities: bynk_model::lower_handler_given_ir(handler)
                        .into_iter()
                        .map(|c| c.name)
                        .collect::<HashSet<_>>(),
                    // #934: the qualified handler name `Idempotency.dedup`/
                    // `remember` key scoping prefixes onto the developer-supplied
                    // key.
                    handler_scope: Some(format!(
                        "{}.{}.{}",
                        ctx.commons_name, s.name.name, kind_name
                    )),
                    owning_context: ctx.commons_name.clone(),
                    ..HandlerShared::default()
                },
                deps_identity_binder,
                actor_sum_binder,
            },
        );
        cx.local_agents = ctx.local_agents.clone();
        // #1653: parameters are bindings of the body's own scope, so a `let` that
        // re-binds one gets a fresh name instead of redeclaring it.
        for p in &handler.params {
            cx.declare_binder(&p.name.name);
        }
        let async_tail = *ir_effectful;
        // 1A.15: the body is items, not text: the prologue guards, the lowered
        // body and the events wrapper reach the printer as statements, which
        // records their checkpoints.
        let lowered_body = block_body_with_return(
            &handler.body,
            &mut cx,
            INDENT_STEP * 2,
            async_tail,
            Some(&handler.return_type),
        );
        let mut guards = Body::new();
        // Events track, slice 1 (spine #936): a `from Events(E { ... })`
        // subscription filter is deliver-and-filter (ADR 0286, unchanged by
        // this slice) — the fan-out mechanism still delivers every emission
        // of `E` to every subscriber; this subscriber's own generated
        // handler evaluates the pattern as a guard and no-ops if it doesn't
        // match. Prologue, not a wrapper edit, so it covers all three
        // delivery paths (Cloudflare Workers, Bundle/node, Bundle/browser)
        // in one place, since they all call into this same generated method.
        // `handler.params[0]`'s type is guaranteed to equal the header's
        // event type by `check_service_protocols`'s param-type-agreement
        // check, so testing its own name (read off `ir_params`, the
        // already-lowered signature `emit_service`'s own loop already
        // holds — P6.54) here is sound.
        if handler_kind_ir == IrHandlerKind::Event
            && let ProtocolIr::Events {
                pattern: Some(pattern),
                ..
            } = protocol
            && let Some((param_name, _)) = ir_params.first()
            && let Some(guard) = event_pattern_guard_ir(&ts_ident(param_name), Some(pattern))
        {
            guards.push(Item::line(format!("if (!({guard})) return undefined;")));
        }
        // Events track, slice 4 (spine #936): a `via schema(N)` guard,
        // independent of and in addition to the payload-pattern guard above
        // — a service may carry either, both, or neither. Same
        // prologue technique, same three-delivery-path coverage. The
        // envelope binder is either the user's own declared `env` name or
        // the synthetic one inserted above.
        if let ProtocolIr::Events {
            schema_dispatch: Some(version),
            ..
        } = protocol
            && let Some(env_binder) = &schema_dispatch_env_binder
            && let Some(test) = schema_version_test(env_binder, *version)
        {
            // Inserted ahead of the subscriber-filter guard, as it always was.
            let mut schema_guard = Body::new();
            schema_guard.push(Item::line(format!("if (!({test})) return undefined;")));
            schema_guard.append(std::mem::replace(&mut guards, Body::new()));
            guards = schema_guard;
        }
        // Append the deps parameter (may include surface field if the body
        // made cross-context calls). v0.47: a Bearer handler's deps also carries
        // the seam-minted `identity` — but only when a binder captures it
        // (v0.50: a binder-less Bearer handler verifies but mints no identity).
        let mut deps_members = build_deps_object_ty_with_surface(
            &effective_given(&bynk_model::lower_handler_given_ir(handler), &cx),
            &cx,
            &ctx.cross_context,
            ctx.target,
        );
        // v0.47/v0.151/v0.52/v0.54: widen `deps` for whichever actor seam
        // this handler resolved to — at most one arm ever fires, `seam`
        // being an enum rather than four independent optionals (a stronger
        // guarantee than the four-`if`-in-a-row shape this replaced, whose
        // mutual exclusion depended on the resolver-priority `if`s above
        // rather than the type system).
        match &seam {
            // #3386: a `SignedCookie` seam mints its identity from `sub` just the same.
            ActorSeamIr::Bearer(_) | ActorSeamIr::SignedCookie(_) => {
                if let Some(id) = seam.captured_identity_type() {
                    append_deps_field(&mut deps_members, "identity", bynk_ts::TsType::named(id));
                }
            }
            // v0.151: an `Oidc`-binding handler threads its `sub`-minted
            // identity into deps exactly like Bearer.
            ActorSeamIr::Oidc(s) if s.binder.is_some() => {
                append_deps_field(
                    &mut deps_members,
                    "identity",
                    bynk_ts::TsType::named(s.identity_type.clone()),
                );
                // #2380 (L5): and the verified token's claims.
                append_deps_field(
                    &mut deps_members,
                    "claims",
                    crate::emitter::ts_types::token_claims_ts_type(),
                );
            }
            // v0.54: a Caller-binding call handler's deps carries the
            // caller's context name as its `CallerId` identity (a `string`).
            // #3328: and a `Sender`-binding email handler's carries the minted
            // `SenderIdentity` (see [`seam_identity_ts_type`]).
            ActorSeamIr::Caller(_) | ActorSeamIr::Sender(_) => {
                append_deps_field(&mut deps_members, "identity", seam_identity_ts_type(&seam));
            }
            // v0.52: a sum handler's deps carries the resolved-actor tagged
            // union (`who`), which the body `match`es. A binder-less sum is
            // rejected by the checker, so a sum handler always captures `who`.
            //
            // Arc F, slice 4 (#1463): review of #1463 found this can't build
            // as a real `TsType::Object`/`TsType::union` — each member's own
            // `{ tag: "Name", identity: T }` shape is comma-separated
            // (matching the pre-existing hand-written text), but
            // `TsType::Object`'s own printer always joins members with `"; "`
            // (`bynk-ts/src/printer.rs`'s own hardcoded convention) —
            // confirmed by a real fixture byte-diff, not assumed. Rebuilding
            // it would silently change the punctuation, not just the
            // construction. Stays the pre-rendered text it already was,
            // wrapped as opaque `Named` — the same "an odd, one-off shape
            // stays opaque text" precedent this track uses elsewhere, not a
            // gap this slice leaves unclosed.
            ActorSeamIr::Sum(members) => {
                append_deps_field(
                    &mut deps_members,
                    "who",
                    bynk_ts::TsType::named(actor_sum_ts_union(members)),
                );
            }
            // I3 (#1971): a `call` sum. `Caller` carries the calling context's
            // name; a foreign caller is its actor alone.
            ActorSeamIr::CallSum(members) => {
                append_deps_field(&mut deps_members, "who", call_sum_ts_type(members));
            }
            // A binder-less Bearer/Oidc, or no seam at all — nothing to widen.
            ActorSeamIr::Oidc(_) | ActorSeamIr::None => {}
        }
        // v0.79: a handler whose body uses `detach` receives the execution context
        // (`__exec`) in its deps, so the fire-and-forget send can hand its promise
        // to `waitUntil`. Gated on the body so non-sending handlers are unchanged.
        if commons.exec_plan.handler_needs(handler) {
            append_deps_field(
                &mut deps_members,
                "__exec",
                bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::method(
                    "waitUntil",
                    vec![bynk_ts::TsParam::new(
                        "promise".to_string(),
                        Some(bynk_ts::TsType::named_with_args(
                            "globalThis.Promise",
                            vec![bynk_ts::TsType::named("unknown")],
                        )),
                    )],
                    bynk_ts::TsType::named("void"),
                )]),
            );
        }
        // Events track, slice 0 (spine #936): a handler whose body uses
        // `Events.emit` receives a compose-supplied `__eventsDispatch`
        // callback in its deps — the actual fanout delivery mechanism
        // (a Cloudflare Durable Object per publishing context, or an
        // in-process dispatch on Bundle/node) lives at the composition
        // layer, which already knows how to build any capability's
        // provider; the handler itself only needs to hand its buffered
        // `__events` to whatever composed it. Mirrors `__exec`'s threading
        // for `detach` exactly. Gated on more than this handler's own body: a
        // handler with no direct `Events.emit` call but that invokes a
        // *local agent* method which itself emits (`Ledger(id).bump(...)`)
        // still needs `__eventsDispatch` threaded through, to hand onward —
        // `cx.agent_given_caps_used` is exactly the existing mechanism that
        // already propagates an invoked agent method's other `given`
        // capabilities up to the caller (`effective_given`, just below);
        // `Events` rides the same path since it's declared as an ordinary
        // `given Events` on the agent handler.
        let needs_events_dispatch = cx.is_first_party(FirstPartyCap::Events)
            && (bynk_model::block_uses_emit(&handler.body, &commons.callees)
                || cx
                    .agent_given_caps_used()
                    .is_some_and(|m| m.contains_key("Events")));
        if needs_events_dispatch {
            // Arc F, slice 4 (#1463): a bare function type has no way to name
            // its own parameter (`TsType::Fn`'s params are anonymous,
            // positional `a0`/`a1`/… — its own doc), the same real algebra
            // gap `emit_context_deps_interface`'s own identical field already
            // named (#1453) — stays opaque `Named` text, not a new pattern.
            append_deps_field(
                &mut deps_members,
                "__eventsDispatch",
                bynk_ts::TsType::named(format!(
                    "(events: globalThis.Array<{}>) => globalThis.Promise<void>",
                    crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE
                )),
            );
        }
        // Arc F, slice 4 (#1463): `deps`'s own type is now the real
        // `TsType::Object` `build_deps_object_ty_with_surface` builds,
        // widened in place by every `append_deps_field` call above — closes
        // #1361's own named residual ("deps's own type stays one opaque
        // `TsType::named` string... real, separate, much larger work than
        // this slice's own scope").
        params.push(bynk_ts::TsParam::new(
            "deps".to_string(),
            Some(bynk_ts::TsType::Object(deps_members)),
        ));
        // Events track, slice 0 (spine #936): release-at-commit (the retired
        // events track's §3.0, `design/archive/retired-tracks.md`) needs a
        // completion boundary services never had before — the
        // body runs inside an IIFE so its own `return`s resolve `__result`
        // rather than the outer method, and only a handler that completes
        // without throwing reaches past it, then flushes `__events` to
        // whatever composed it (`deps.__eventsDispatch`, threaded above).
        // Gated on `block_uses_emit` specifically (not the broader
        // `needs_events_dispatch` above) — a handler that only *forwards*
        // `__eventsDispatch` to an agent it calls has nothing of its own to
        // buffer or flush, so it keeps byte-identical output, mirroring
        // `__exec`'s gate on `block_uses_send`.
        let body_emits_directly = bynk_model::block_uses_emit(&handler.body, &commons.callees);
        // #1361: the events-IIFE wrapper (if any) surrounds the guards and the
        // lowered body. 1A.15: the whole method body is statements now; the
        // wrapper's own lines are nodes where the printer would lay them out
        // the same way, and text where it would not (the body sits at the
        // IIFE's own indentation, which a nested arrow body would not).
        let mut body = guards;
        body.append(lowered_body);
        let body = if body_emits_directly {
            events_release_body(body)
        } else {
            body
        };
        let body = body.into_stmts(INDENT_STEP * 2);

        // #2280: anchor the method header and prologue to the handler's own
        // `on call` line rather than the previous statement's.
        entries.push(bynk_ts::TsObjectEntry::Method {
            name: kind_name,
            is_async: *ir_effectful,
            generics: Vec::new(),
            params,
            return_type: Some(ts_ty_to_ts_type(*ir_ret, tys)),
            doc: handler.documentation.as_deref().map(str::to_owned),
            inline: false,
            body,
            span: Some(handler.span),
        });
    }
    // #2280: what follows the last handler belongs to the service, not to that
    // handler's final body line.
    let mut stmts = Vec::new();
    let has_doc = s.documentation.is_some();
    if let Some(doc) = s.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let mut object = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: s.name.name.clone(),
            ty: None,
            init: bynk_ts::TsExpr::multiline_object_ending_at(entries, s.span),
        })),
        None,
    );
    // A leading doc comment sits directly above the declaration it documents.
    object.no_blank_before = has_doc;
    stmts.push(object);
    stmts
}

/// v0.15: the TypeScript deps-field type for a `given` capability reference.
/// A local capability uses its bare interface name; a cross-context one is
/// qualified with the providing context's import namespace
/// (`platform_time.Clock`).
///
/// Arc F, slice 4 (#1463): a qualified capability name (`ns.Cap`) has no
/// dedicated `bynk_ts::TsType` shape (no member-access-on-a-type variant in
/// the algebra) — `Named` carrying the whole dotted string is the same
/// honest-opaque representation `workers_env_ty`'s own `ServiceBinding`/
/// `DurableObjectNamespace` entries already use for library-provided names.
fn cap_ref_ty(
    c: &bynk_model::CapRefIr,
    info: &bynk_check::resolver::CrossContextInfo,
) -> bynk_ts::TsType {
    match c.context.as_deref().and_then(|p| info.resolve_prefix(p)) {
        Some(consumed) => {
            bynk_ts::TsType::named(format!("{}.{}", qualified_to_ns(&consumed), c.name))
        }
        // v0.17: a bare flattened capability (`consumes U { Cap }`) keeps its
        // interface in the consumed unit's module — qualify the type there.
        None => match info.flattened_caps.get(&c.name) {
            Some(unit) => bynk_ts::TsType::named(format!("{}.{}", qualified_to_ns(unit), c.name)),
            None => bynk_ts::TsType::named(c.name.clone()),
        },
    }
}

/// Events track, slice 0 (spine #936): does any service or agent handler in
/// this commons emit — the same predicate `block_uses_emit` answers per
/// handler body, rolled up to "does the context-level `__eventsDispatch`
/// deps field need to exist at all." Syntactic, like its sibling: a false
/// positive here just adds an unused interface field, not a miscompile.
pub(crate) fn commons_uses_emit(commons: &TypedCommons) -> bool {
    commons.commons.items.iter().any(|item| match item {
        CommonsItem::Service(s) => s
            .handlers
            .iter()
            .any(|h| bynk_model::block_uses_emit(&h.body, &commons.callees)),
        CommonsItem::Agent(a) => a
            .handlers
            .iter()
            .any(|h| bynk_model::block_uses_emit(&h.body, &commons.callees)),
        _ => false,
    })
}

/// v0.15: collect the cross-context capabilities a context's **handlers**
/// (service + agent) reference via `given B.Cap`, as `(deps_key,
/// consumed_context)` pairs, deduplicated by key and sorted. These become
/// top-level deps fields (handlers access them as `deps.<key>`). Capabilities
/// used only by a provider are injected into that provider's constructor
/// instead, so they are excluded here.
pub(crate) fn cross_context_caps_used(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<(String, String)> {
    let info = &ctx.cross_context;
    let mut seen: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let givens = commons
        .commons
        .items
        .iter()
        .flat_map(|item| match item {
            CommonsItem::Service(s) => s.handlers.as_slice(),
            CommonsItem::Agent(a) => a.handlers.as_slice(),
            _ => &[],
        })
        .map(bynk_model::lower_handler_given_ir)
        .chain(std::iter::once(sibling_agent_givens(commons, ctx)));
    for given in givens {
        for c in given {
            // Events track, slice 0 (spine #936): `Events.emit` is
            // intercepted entirely at the call site (release-at-commit
            // buffering) and never calls through a constructed provider
            // — unlike every other capability, there is no
            // `EventsProvider` for compose to build, so the first-party
            // `Events` must not appear in any context's deps interface.
            if bynk_check::firstparty::resolve(&c.name, &ctx.commons_name, &info.flattened_caps)
                == Some(FirstPartyCap::Events)
            {
                continue;
            }
            if let Some(prefix) = &c.context {
                if let Some(consumed) = info.resolve_prefix(prefix) {
                    seen.entry(c.name.clone()).or_insert(consumed);
                }
            } else if let Some(unit) = info.flattened_caps.get(&c.name) {
                // v0.17: a bare flattened capability is a cross-unit dep too.
                seen.entry(c.name.clone()).or_insert_with(|| unit.clone());
            }
        }
    }
    seen.into_iter().collect()
}

/// v0.15: the set of consumed contexts whose capabilities this context
/// references anywhere (handlers *or* providers), so their namespaces are
/// imported for the capability interface types.
pub(crate) fn cross_context_cap_namespaces(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> std::collections::BTreeSet<String> {
    let info = &ctx.cross_context;
    let mut out = std::collections::BTreeSet::new();
    let mut collect = |given: Vec<bynk_model::CapRefIr>| {
        for c in given {
            if let Some(prefix) = &c.context
                && let Some(consumed) = info.resolve_prefix(prefix)
            {
                out.insert(consumed);
            } else if c.context.is_none()
                // v0.17: a bare flattened capability imports its interface from
                // the consumed unit's module.
                && let Some(unit) = info.flattened_caps.get(&c.name)
            {
                out.insert(unit.clone());
            }
        }
    };
    for item in &commons.commons.items {
        match item {
            CommonsItem::Service(s) => s
                .handlers
                .iter()
                .for_each(|h| collect(bynk_model::lower_handler_given_ir(h))),
            CommonsItem::Agent(a) => a
                .handlers
                .iter()
                .for_each(|h| collect(bynk_model::lower_handler_given_ir(h))),
            CommonsItem::Provider(p) => collect(bynk_model::lower_provider_given_ir(p)),
            // #2912: a function's `given` names its capability in `deps` too.
            CommonsItem::Fn(f) => collect(bynk_model::lower_fn_given_ir(f)),
            _ => {}
        }
    }
    collect(sibling_agent_givens(commons, ctx));
    // S4a (#1966): a derived capability's alias names its base's interface in
    // the base's module.
    out.extend(
        commons
            .commons
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Capability(c) => c.derived.as_ref(),
                _ => None,
            })
            .filter_map(|d| info.flattened_caps.get(&d.base.name).cloned()),
    );
    out
}

/// #1820: the `given` capabilities of every agent this file constructs that
/// a sibling file of its context declares. A handler here forwards its `deps`
/// to that agent (`effective_given`), so its `deps` name them although no
/// declaration in this file does. Empty unless the context is split across
/// files: an agent this file declares is walked with the file's own items.
fn sibling_agent_givens(commons: &TypedCommons, ctx: &EmitProjectCtx) -> Vec<bynk_model::CapRefIr> {
    let declared_here: HashSet<&str> = commons
        .commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Agent(a) => Some(a.name.name.as_str()),
            _ => None,
        })
        .collect();
    let constructed: std::collections::BTreeSet<&str> = commons
        .callees
        .values()
        .filter_map(|c| match c {
            bynk_check::checker::Callee::AgentInit(a) if !declared_here.contains(a.as_str()) => {
                Some(a.as_str())
            }
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    for agent in constructed {
        let Some(methods) = ctx.agent_method_givens.get(agent) else {
            continue;
        };
        let mut methods: Vec<_> = methods.iter().collect();
        methods.sort_by_key(|(m, _)| *m);
        for (_, caps) in methods {
            out.extend(caps.iter().cloned());
        }
    }
    out
}

/// #527: a handler's *effective* `given` list — its declared capabilities
/// plus those required by the local-agent methods its body calls (recorded
/// during lowering). Compose always instantiates the full capability union
/// into the runtime deps value, so this widening only brings the deps *type*
/// in line with the value; without it, forwarding `deps` to an agent method
/// with `given` failed `tsc --strict` (and under-documented the dependency).
fn effective_given(
    declared: &[bynk_model::CapRefIr],
    cx: &LowerCtx<'_>,
) -> Vec<bynk_model::CapRefIr> {
    let mut out = declared.to_vec();
    let have: HashSet<String> = declared.iter().map(|c| c.name.clone()).collect();
    for (key, cap) in cx.agent_given_caps_used().into_iter().flatten() {
        if !have.contains(key) {
            out.push(cap.clone());
        }
    }
    // Events track, slice 0 (spine #936): `Events.emit` is intercepted
    // entirely at the call site (release-at-commit buffering; see the
    // `Events`/`emit` special case in `lower.rs`) and never calls through a
    // constructed provider — unlike every other capability, there is no
    // `EventsProvider` for compose to build. So `Events` must not appear in
    // `deps` at all, or compose would need to construct a provider that
    // doesn't exist. Filtered the same way #934 distinguishes a genuine
    // first-party `Events` from an unrelated same-named capability.
    out.retain(|c| cx.first_party_cap(&c.name) != Some(FirstPartyCap::Events));
    out
}

/// Review of #1469: returns the bare member list, not `TsType::Object(members)`
/// — every real caller immediately widens it further (`append_deps_field`)
/// before wrapping it as the `deps` parameter's own type, and threading
/// `Vec<TsTypeMember>` end to end lets that widening take `&mut Vec<...>`
/// directly, with no `TsType::Object` match arm (and no panic path for the
/// "found something else" case that can't actually happen) in between.
fn build_deps_object_ty_with_surface(
    given: &[bynk_model::CapRefIr],
    cx: &LowerCtx<'_>,
    cross_context: &bynk_check::resolver::CrossContextInfo,
    target: BuildTarget,
) -> Vec<bynk_ts::TsTypeMember> {
    let mut members: Vec<bynk_ts::TsTypeMember> = given
        .iter()
        .map(|c| bynk_ts::TsTypeMember::prop(c.name.clone(), cap_ref_ty(c, cross_context)))
        .collect();
    match target {
        BuildTarget::Bundle => {
            // #1960: an agent method this body calls may itself call a
            // consumed service, reading the `surface` this body hands it —
            // the bundle counterpart of the whole `env` a Workers body
            // threads whenever it constructs an agent.
            if cx.cross_context_used()
                || (cx.agents_instantiated() && has_consumed_service(cross_context))
            {
                members.push(bynk_ts::TsTypeMember::prop(
                    "surface",
                    surface_ty(cross_context),
                ));
            }
        }
        BuildTarget::Workers => {
            // v0.9.2: in workers mode `env` carries both consumed-context
            // Service Bindings and the local agents' Durable Object namespaces.
            // It is threaded into deps whenever the handler makes a
            // cross-context call or instantiates an agent.
            if cx.cross_context_used() || cx.agents_instantiated() {
                let agents = if cx.agents_instantiated() {
                    sorted_local_agents(cx)
                } else {
                    Vec::new()
                };
                members.push(bynk_ts::TsTypeMember::prop(
                    "env",
                    workers_env_ty(&cx.cross_contexts_called(), &agents),
                ));
            }
        }
    }
    members
}

/// Local agent names in this commons, sorted — the DO bindings `env` exposes
/// in workers mode. #1820: every file's of the context (`cx.local_agents`),
/// not only this file's: a handler constructing an agent declared in a
/// sibling file passes the Worker's `env` to that agent's factory.
fn sorted_local_agents(cx: &LowerCtx<'_>) -> Vec<String> {
    let names: std::collections::BTreeSet<String> = cx
        .commons()
        .commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Agent(a) => Some(a.name.name.clone()),
            _ => None,
        })
        .chain(cx.local_agents.iter().cloned())
        .collect();
    names.into_iter().collect()
}

/// Workers-mode deps.env shape: the bindings handlers reach
/// ([`crate::emitter::wrangler::handler_bindings`], #1950) — one Service
/// Binding per consumed context the handler calls and one Durable Object
/// namespace per local agent.
fn workers_env_ty(called: &[String], agents: &[String]) -> bynk_ts::TsType {
    // #1961: only a consumed context this handler **calls**
    // (`lower_workers_cross_context_call`), the one thing a handler reaches
    // through `env`. The Worker binds exactly the contexts some body of it
    // calls (`worker_bindings::context_bindings`), a superset of any one
    // handler's, so `compose`'s `env` always satisfies this type. A unit
    // consumed for capabilities, types or events only has no binding at all —
    // an adapter is not a Worker, and a context whose services go uncalled
    // need not be bound (binding it made an event subscriber and its
    // publisher bind each other, which no deploy order satisfies).
    let members = crate::emitter::wrangler::handler_bindings(called, agents)
        .into_iter()
        .map(|b| {
            let ty = crate::emitter::wrangler::binding_ts_type(&b.source);
            bynk_ts::TsTypeMember::prop(b.name, bynk_ts::TsType::named(ty))
        })
        .collect();
    bynk_ts::TsType::Object(members)
}

/// v0.15: true when at least one consumed context exposes services (and thus
/// a `__makeSurface`). A context may now consume another purely for its
/// capabilities, in which case there is no surface to thread.
fn has_consumed_service(cross_context: &bynk_check::resolver::CrossContextInfo) -> bool {
    cross_context
        .consumed_services
        .values()
        .any(|svcs| !svcs.is_empty())
}

/// Build the TS type for the `surface` field in deps, naming each consumed
/// context by its surface key plus the consumed context's __makeSurface type.
/// Only service-bearing consumed contexts contribute (a capability-only
/// consumed context has no `__makeSurface`).
///
/// Arc F, slice 4 (#1463): `ReturnType<T>` itself is representable generically
/// (`TsType::named_with_args`, review of #1469) — the real gap is its own
/// type argument here, `typeof {ns}.__makeSurface`, whose `typeof` operand
/// isn't an ordinary type at all and has no algebra member of its own.
/// `Named` carries only that opaque operand text now, one level narrower
/// than the whole expression this used to wrap as one string.
fn surface_ty(cross_context: &bynk_check::resolver::CrossContextInfo) -> bynk_ts::TsType {
    let mut entries: Vec<(String, String)> = Vec::new();
    // Use alias if present, else the last segment of the qualified name.
    // Order: stable (sorted) so the diff is deterministic.
    let mut consumed_sorted: Vec<String> = cross_context
        .consumed_services
        .iter()
        .filter(|(_, svcs)| !svcs.is_empty())
        .map(|(q, _)| q.clone())
        .collect();
    consumed_sorted.sort();
    // Reverse lookup: consumed-context qualified name → alias.
    let mut alias_for: HashMap<String, String> = HashMap::new();
    for (alias, target) in &cross_context.aliases {
        alias_for.insert(target.clone(), alias.clone());
    }
    for q in &consumed_sorted {
        let key = alias_for
            .get(q)
            .cloned()
            .unwrap_or_else(|| q.rsplit('.').next().unwrap_or(q.as_str()).to_string());
        let ns = qualified_to_ns(q);
        entries.push((key, format!("typeof {ns}.__makeSurface")));
    }
    bynk_ts::TsType::Object(
        entries
            .into_iter()
            .map(|(k, v)| {
                bynk_ts::TsTypeMember::prop(
                    k,
                    bynk_ts::TsType::named_with_args(
                        "globalThis.ReturnType",
                        vec![bynk_ts::TsType::named(v)],
                    ),
                )
            })
            .collect(),
    )
}

/// #2058: each type name this unit reaches from a consumed *context*, mapped
/// to the namespace the module names that context's types under
/// (`ev_store`). A literal admitted to such a refined type is cast to
/// `ev_store.Code`, not a bare `Code` nothing imports.
pub(crate) fn consumed_type_ns(ctx: &EmitProjectCtx) -> HashMap<String, String> {
    ctx.imported_from_kind
        .iter()
        .filter(|(_, kind)| **kind == UnitKind::Context)
        .filter_map(|(name, _)| {
            let owner = ctx.imported_from.get(name)?;
            Some((name.clone(), qualified_to_ns(owner)))
        })
        .collect()
}

/// Turn a qualified context name (e.g. `commerce.payment`) into the JS
/// namespace ident used in `import * as <ns>` (`commerce_payment`).
pub(crate) fn qualified_to_ns(q: &str) -> String {
    q.replace('.', "_")
}

/// #1759: the identifier a generated **test** module imports a unit as
/// (`import * as __ns_greet from …`). A test module destructures a unit's
/// values out of this namespace (`const { greet } = __ns_greet;`), so the
/// namespace can't take `qualified_to_ns`'s bare name: a context `greet`
/// declaring `fn greet` would shadow its own import and read `greet` in its
/// temporal dead zone. The reserved `__ns_` prefix can't be a user identifier.
/// Every test-module namespace reference goes through this, in `tests_emit`
/// and in lowering that runs `in_test_scaffold()`.
pub fn test_scaffold_ns(q: &str) -> String {
    format!("__ns_{}", qualified_to_ns(q))
}

/// The PascalCase name a context uses for its generated `Deps` interface:
/// `shortener.links` → `ShortenerLinks`.
pub fn context_pascal(name: &str) -> String {
    name.split('.')
        .map(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Emit the per-context `<Ctx>Deps` interface (v0.9.2 §4): the providers the
/// context contributes plus the surfaces of any consumed contexts. Replaces
/// the fragile `Parameters<typeof svc.call>[1]` indexing, which only resolved
/// correctly for single-argument service operations.
/// Arc F slice 3 (#1453): builds a real `bynk_ts::TsDecl::Interface`, the
/// same `readonly`-member shape `emit_agent`'s own state interface already
/// establishes (`emit_agent.rs:4436-4444`) — return contract stays
/// the interface's own name too (used as a plain `TsType::named(deps_name)`
/// type reference by `emit_make_surface`, its one caller); since 1A.32c-1 it
/// returns the declaration as a node, not printed text.
///
/// Two fields stay opaque `TsType::named(...)` text, deliberately, not
/// guessed: `surface`'s type comes from `surface_ty`, itself still
/// `String`-returning (#1449's own bucket-4 residual, out of this slice's
/// scope) — wrapping its output opaquely is the established "carry an
/// unconverted sibling's own text" pattern. `__eventsDispatch`'s function
/// type needs a *named* parameter (`events`); `bynk_ts::TsType::Fn` only
/// supports positional parameter names (`a0`, `a1`, …, confirmed against its
/// own printer tests) — the same real, already-named algebra gap
/// `ts_type_ref_to_ts_type`'s own `TypeRef::Query` arm (`emitter.rs`)
/// resolved by pre-rendering and wrapping as opaque `Named` text rather than
/// widening `TsType::Fn` for one call site.
fn emit_context_deps_interface(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> (bynk_ts::TsStmt, String) {
    let deps_name = format!("__{}Deps", context_pascal(&commons.commons.name.joined()));
    let mut members: Vec<bynk_ts::TsTypeMember> = commons
        .commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Capability(c) => Some(bynk_ts::TsTypeMember::readonly_prop(
                c.name.name.clone(),
                bynk_ts::TsType::named(c.name.name.clone()),
            )),
            _ => None,
        })
        .collect();
    // #1820: and the capabilities a sibling file of a split context declares.
    // A service here may call an agent there whose methods are `given` one,
    // and its handler's `deps` then names it (`effective_given`).
    let mut sibling_caps: Vec<&String> = ctx
        .file_decl_index
        .capabilities
        .iter()
        .filter(|(_, path)| **path != ctx.source_path)
        .map(|(cap, _)| cap)
        .collect();
    sibling_caps.sort();
    for cap in sibling_caps {
        members.push(bynk_ts::TsTypeMember::readonly_prop(
            cap.clone(),
            bynk_ts::TsType::named(cap.clone()),
        ));
    }
    // v0.15: cross-context capabilities the context consumes appear in deps,
    // typed against the providing context's namespace.
    for (key, consumed) in cross_context_caps_used(commons, ctx) {
        let ty = bynk_ts::TsType::named(format!("{}.{key}", qualified_to_ns(&consumed)));
        members.push(bynk_ts::TsTypeMember::readonly_prop(key, ty));
    }
    if !ctx.cross_context.consumed_contexts.is_empty() && has_consumed_service(&ctx.cross_context) {
        // Arc F, slice 4 (#1463): `surface_ty` now returns a real `TsType`
        // directly — closes the residual Arc F slice 3 (#1453) named here
        // ("kept as opaque `TsType::named(...)` text... because its own
        // source, `surface_ty`, is still `String`-returning").
        members.push(bynk_ts::TsTypeMember::readonly_prop(
            "surface",
            surface_ty(&ctx.cross_context),
        ));
    }
    // Events track, slice 0 (spine #936): a context with any handler that
    // emits needs `__eventsDispatch` threaded all the way from `__makeSurface`
    // down to that handler's own `deps` param (`emit_service`/`emit_agent`'s
    // matching field) — otherwise `svc.call(args, deps)` inside `__makeSurface`
    // fails to typecheck, since `deps`'s static type there is this interface,
    // not whatever compose actually constructs.
    let is_first_party_events = bynk_check::firstparty::resolve(
        FirstPartyCap::Events.name(),
        &ctx.commons_name,
        &ctx.cross_context.flattened_caps,
    ) == Some(FirstPartyCap::Events);
    if is_first_party_events && commons_uses_emit(commons) {
        members.push(bynk_ts::TsTypeMember::readonly_prop(
            "__eventsDispatch",
            bynk_ts::TsType::named(format!(
                "(events: globalThis.Array<{}>) => globalThis.Promise<void>",
                crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE
            )),
        ));
    }
    // #2683: a surface `on call` handler that calls `within` (or sends with
    // `detach`) forwards `deps` to a handler that requires `__exec`, so the
    // surface's `deps` carry it; the compose root supplies it.
    if commons.commons.items.iter().any(|i| match i {
        CommonsItem::Service(s) => s
            .handlers
            .iter()
            .any(|h| crate::emitter::call_handler_needs_exec(&commons.exec_plan, h)),
        _ => false,
    }) {
        members.push(bynk_ts::TsTypeMember::readonly_prop(
            "__exec",
            bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::method(
                "waitUntil",
                vec![bynk_ts::TsParam::new(
                    "promise".to_string(),
                    Some(bynk_ts::TsType::named_with_args(
                        "globalThis.Promise",
                        vec![bynk_ts::TsType::named("unknown")],
                    )),
                )],
                bynk_ts::TsType::named("void"),
            )]),
        ));
    }
    let interface_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Interface {
            name: deps_name.clone(),
            type_params: Vec::new(),
            members,
        })),
        None,
    );
    // #1486: no trailing blank baked in here — the printer's automatic
    // top-level spacing policy supplies the single blank before whatever
    // follows this interface.
    (interface_decl, deps_name)
}

/// v0.54 (#655): whether any of `services` declares an `on call … by c: Caller`
/// handler, whose emitted `deps` carries the calling context's qualified name as
/// its `CallerId` identity (ADR 0092). This is the *single* predicate both
/// bundle caller-identity seams consult — `emit_make_surface` (which adds the
/// `__caller` parameter) and the compose root (which passes the name) — so the
/// two can never disagree on which providers thread a caller. `caller_binder_for`
/// self-guards `HandlerKind::Call`, so no kind filter is needed or wanted here.
pub fn any_service_binds_caller<'a>(
    services: impl IntoIterator<Item = &'a ServiceDecl>,
    actors: &HashMap<String, ActorDecl>,
) -> bool {
    // I3 (#1971): a `call` sum names its caller too (`who`), and in bundle
    // mode every caller is a context.
    services.into_iter().any(|s| {
        s.handlers.iter().any(|h| {
            bynk_check::actors::caller_binder_for(h, actors).is_some()
                || bynk_check::actors::call_admission_for(h, actors)
                    .is_some_and(|a| a.sum.is_some())
        })
    })
}

/// Emit the `__makeSurface(deps)` function for a context that exposes
/// services to other contexts (v0.6 §6.3 / §6.4).
///
/// Arc C, slice 18 (#1364): closes the `emit_make_surface` half of step (8)
/// of the design pass's own decomposition order (#1331) — the "cross-context
/// lowering cluster" the same step also names stays a separate, deferred
/// remainder (a genuinely different, harder shape: real cross-module codec
/// lowering, not top-level structure construction), the same "split when a
/// named remainder is harder" precedent steps (4)/(6) already established.
/// Converts fully — no opaque carve-out: every shape this function needs
/// (`bynk_ts::TsDecl::Function`, `TsObjectEntry::Method`/`Spread`,
/// `TsStmt::Return`, `TsExpr::Call`/`multiline_object_entries`) already
/// exists in `bynk_ts`, and this function never lowers an expression through
/// `LowerCtx` (no `source_map` parameter reaches it), so there is no
/// sub-builder/merge machinery to get right here. `emit_context_deps_interface`
/// converted later (Arc F slice 3, #1453) — it still returns `String` (just
/// the interface's own name, used below as a plain `TsType::named` type
/// reference), but the interface declaration itself is a real
/// `bynk_ts::TsDecl::Interface` now, not `writeln!`-built text.
/// #1478: returns real [`bynk_ts::TsStmt`]s (was `out: &mut String`).
/// `emit_context_deps_interface` returns the interface declaration as a node
/// (1A.32c-1), so it is the surface's first statement.
pub(crate) fn emit_make_surface(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    let services: Vec<&ServiceDecl> = commons
        .commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Service(s) => Some(s),
            _ => None,
        })
        .collect();
    if services.is_empty() {
        return Vec::new();
    }
    let (deps_decl, deps_name) = emit_context_deps_interface(commons, ctx);
    let mut stmts = vec![deps_decl];
    // v0.54 (#655): an `on call … by c: Caller` handler reads a live `CallerId`
    // (the calling context's qualified name) threaded through `deps.identity`.
    // In bundle mode the compose root supplies that name to `__makeSurface` as a
    // second `__caller` argument — the analogue of the `X-Bynk-Caller` header a
    // Worker reads at its entry (ADR 0092). Only a context with such a handler
    // takes the extra parameter, so a caller-free surface is byte-unchanged. The
    // shared `any_service_binds_caller` predicate is what keeps this seam and the
    // compose root in lockstep on which providers get the extra argument.
    let binds_caller =
        |h: &Handler| bynk_check::actors::caller_binder_for(h, &ctx.actors).is_some();
    let any_caller = any_service_binds_caller(services.iter().copied(), &ctx.actors);
    let mut params = vec![bynk_ts::TsParam::new(
        "deps".to_string(),
        Some(bynk_ts::TsType::named(deps_name)),
    )];
    if any_caller {
        params.push(bynk_ts::TsParam::new(
            "__caller".to_string(),
            Some(bynk_ts::TsType::named("string")),
        ));
    }
    let mut entries: Vec<bynk_ts::TsObjectEntry> = Vec::new();
    for s in &services {
        // For each handler kind currently only `call`. We bind it as a
        // method on the surface with the deps captured.
        let handler = s
            .handlers
            .iter()
            .find(|h| matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Call));
        let Some(h) = handler else { continue };
        let is_async = is_effectful_return(&h.return_type);
        // #2696: the method body reads `<service>` after the parameters bind, so a
        // user parameter spelled like the service would shadow it. Rename the
        // user's binder (hygienically), never the generated name, so every other
        // surface stays byte-identical. `deps` needs no check here: `ts_ident`
        // already maps it to `__id_deps`. `__caller` needs none either: a
        // `__`-prefixed name can't be a user identifier.
        let surface_param = |name: &str| -> String {
            let id = ts_ident(name);
            if id == s.name.name {
                format!("__id_{id}")
            } else {
                id
            }
        };
        let method_params: Vec<bynk_ts::TsParam> = h
            .params
            .iter()
            .map(|p| {
                bynk_ts::TsParam::new(
                    surface_param(&p.name.name),
                    Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
                )
            })
            .collect();
        let mut call_args: Vec<bynk_ts::TsExpr> = h
            .params
            .iter()
            .map(|p| bynk_ts::TsExpr::Ident(surface_param(&p.name.name)))
            .collect();
        // A Caller-binding handler's `deps.identity` is the caller name the
        // compose root threaded in; every other handler forwards `deps` verbatim.
        //
        // I3 (#1971): in bundle mode every caller is a context, never a
        // foreign caller, so a `call` sum resolves to `Caller` with that name,
        // and a handler that admits only foreign callers is never admitted.
        let admission = bynk_check::actors::call_admission_for(h, &ctx.actors);
        if admission.as_ref().is_some_and(|a| !a.contexts) {
            entries.push(bynk_ts::TsObjectEntry::Method {
                name: s.name.name.clone(),
                is_async,
                generics: Vec::new(),
                params: method_params,
                return_type: Some(ts_type_ref_to_ts_type(&h.return_type, None)),
                doc: None,
                inline: false,
                body: vec![bynk_ts::TsStmt::throw_stmt(
                    bynk_ts::TsExpr::New {
                        callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.Error".to_string())),
                        args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(format!(
                            "`{}` admits only foreign callers, and a context is not one",
                            s.name.name
                        )))],
                    },
                    None,
                )],
                span: None,
            });
            continue;
        }
        let deps_arg = if admission.is_some_and(|a| a.sum.is_some()) {
            bynk_ts::TsExpr::object_entries(vec![
                bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident("deps".to_string())),
                bynk_ts::TsObjectEntry::prop(
                    "who".to_string(),
                    bynk_ts::TsExpr::object(vec![
                        (
                            "tag".to_string(),
                            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str("Caller".to_string())),
                        ),
                        (
                            "identity".to_string(),
                            bynk_ts::TsExpr::Ident("__caller".to_string()),
                        ),
                    ]),
                ),
            ])
        } else if binds_caller(h) {
            bynk_ts::TsExpr::object_entries(vec![
                bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident("deps".to_string())),
                bynk_ts::TsObjectEntry::prop(
                    "identity".to_string(),
                    bynk_ts::TsExpr::Ident("__caller".to_string()),
                ),
            ])
        } else {
            bynk_ts::TsExpr::Ident("deps".to_string())
        };
        call_args.push(deps_arg);
        let call = bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident(s.name.name.clone())),
                property: "call".to_string(),
            }),
            args: call_args,
        };
        entries.push(bynk_ts::TsObjectEntry::Method {
            name: s.name.name.clone(),
            is_async,
            generics: Vec::new(),
            params: method_params,
            return_type: Some(ts_type_ref_to_ts_type(&h.return_type, None)),
            doc: None,
            inline: false,
            body: vec![bynk_ts::TsStmt::return_stmt(Some(call), None)],
            span: None,
        });
    }
    let func_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Function {
            name: "__makeSurface".to_string(),
            generics: Vec::new(),
            params,
            return_type: None,
            body: vec![bynk_ts::TsStmt::return_stmt(
                Some(bynk_ts::TsExpr::multiline_object_entries(entries)),
                None,
            )],
            is_async: false,
            inline: false,
        })),
        None,
    );
    stmts.push(func_decl);
    // #1486: no explicit trailing blank stmt here — the printer's automatic
    // top-level spacing policy supplies the single blank before whatever
    // follows (never import-kind at this function's own call site, so
    // never exempted from it).
    stmts
}

pub(crate) fn flatten_emit_ident_chain(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(id) => Some(id.name.clone()),
        ExprKind::FieldAccess { receiver, field } => {
            let head = flatten_emit_ident_chain(receiver)?;
            Some(format!("{head}.{}", field.name))
        }
        _ => None,
    }
}

/// True when a type has a boundary deserialiser (ADR 0124 rehydration gate). A
/// non-codec type never legally reaches a `store` position, but a `Cell[()]`
/// could, so the gate skips anything `deserialise_expr` would reject.
fn is_codecable(t: &TypeRef) -> bool {
    matches!(
        t,
        TypeRef::Base(..)
            | TypeRef::Named(..)
            | TypeRef::Option(..)
            | TypeRef::List(..)
            | TypeRef::Result(..)
            | TypeRef::Map(..)
    )
}

/// An agent's `store` fields with the checker's resolved kind (1A.33): the kind is read from
/// the [`StoreFieldIr`], and the declaration's own type references stay the AST ones the
/// codec and key checks below take. `state` is built from `a.store_fields` in order
/// (`agent_state_ir`), so the two zip one to one.
fn store_kinds<'a, 'b>(
    a: &'a AgentDecl,
    state: &'b [StoreFieldIr],
) -> impl Iterator<Item = (&'a StoreField, &'b StoreKindIr)> {
    debug_assert_eq!(a.store_fields.len(), state.len());
    a.store_fields.iter().zip(state.iter().map(|ir| &ir.kind))
}

/// The checker's resolved `store` fields of an agent, in declaration order.
pub(crate) fn agent_state_ir(a: &AgentDecl, program: &CheckedProgram) -> Vec<StoreFieldIr> {
    a.store_fields
        .iter()
        .map(|f| lower_store_field_shape_ir(f, program))
        .collect()
}

/// Whether an agent emits a rehydration gate (and so imports `rehydrationViolation`).
/// Mirrors the per-field validation the gate builds, so the header import and the
/// emitted gate agree exactly (a mismatch is an unused / undefined import).
pub(crate) fn agent_needs_rehydrate(
    a: &AgentDecl,
    state: &[StoreFieldIr],
    types: &HashMap<String, Arc<TypeDecl>>,
) -> bool {
    // v0.105 (slice 3b-ii): a held `Map[K, Connection]` persists `K → connId` and
    // is rehydrated like any map — no value check (the connId is opaque) but the
    // textual `K` key is validated. The `Map` arm already counts that via the key,
    // so held maps need no special handling here.
    store_kinds(a, state).any(|(f, kind)| match kind {
        StoreKindIr::Cell(_) | StoreKindIr::Log(..) => {
            f.kind.args.first().is_some_and(is_codecable)
        }
        StoreKindIr::Map(..) | StoreKindIr::Cache(..) => {
            f.kind.args.get(1).is_some_and(is_codecable)
                || f.kind
                    .args
                    .first()
                    .is_some_and(|k| type_base_is_string(k, types))
        }
        StoreKindIr::Set(_) => f
            .kind
            .args
            .first()
            .is_some_and(|t| type_base_is_string(t, types)),
    })
}

/// Whether any agent of the unit emits a rehydration gate.
pub(crate) fn commons_need_rehydrate(commons: &TypedCommons, program: &CheckedProgram) -> bool {
    commons.commons.items.iter().any(|i| match i {
        CommonsItem::Agent(a) => {
            agent_needs_rehydrate(a, &agent_state_ir(a, program), &commons.types)
        }
        _ => false,
    })
}

/// The runtime names every agent of the unit's state wiring references, sorted and unique.
pub(crate) fn commons_state_runtime_names(
    commons: &TypedCommons,
    program: &CheckedProgram,
) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = commons
        .commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Agent(a) => Some(agent_state_runtime_names(
                a,
                &agent_state_ir(a, program),
                commons.tys(),
                &commons.types,
            )),
            _ => None,
        })
        .flatten()
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Whether any agent of the unit has a held `Map[K, Connection]`.
pub(crate) fn commons_have_held_storage(commons: &TypedCommons, program: &CheckedProgram) -> bool {
    commons.commons.items.iter().any(|i| match i {
        CommonsItem::Agent(a) => {
            agent_has_held_storage(a, &agent_state_ir(a, program), commons.tys())
        }
        _ => false,
    })
}

/// The runtime names an agent's state wiring references (#3254): the handle classes its
/// state is made of, and the helpers that put them into wire form and fill them back.
/// Mirrors what `emit_agent_state_record`/`emit_agent_state_persistence` emit.
pub(crate) fn agent_state_runtime_names(
    a: &AgentDecl,
    state: &[StoreFieldIr],
    tys: &Types,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Vec<&'static str> {
    let held = held_map_fields(a, state, tys);
    let is_held = |f: &StoreField| held.iter().any(|(n, _)| n.name == f.name.name);
    let mut names: Vec<&'static str> = Vec::new();
    let mut textual_key = false;
    for (f, kind) in store_kinds(a, state) {
        match kind {
            StoreKindIr::Map(..) => {
                names.push("__StoreMap");
                if is_held(f) {
                    names.extend(["__wireHeld", "__wireHeldOrder", "__fillHeld"]);
                } else {
                    names.extend(["__wireMap", "__fillMap"]);
                }
                if f.annotations.iter().any(|an| an.name.name == "indexed") {
                    names.push("__wirePostings");
                }
                textual_key |= f
                    .kind
                    .args
                    .first()
                    .is_some_and(|k| type_base_is_string(k, types));
            }
            StoreKindIr::Set(_) => {
                names.extend(["__StoreSet", "__wireSet", "__fillSet"]);
                textual_key |= f
                    .kind
                    .args
                    .first()
                    .is_some_and(|t| type_base_is_string(t, types));
            }
            StoreKindIr::Cache(..) => names.extend(["__StoreCache", "__wireMap", "__fillMap"]),
            StoreKindIr::Log(..) => names.extend(["__StoreLog", "__wireLog", "__fillLog"]),
            StoreKindIr::Cell(_) => {}
        }
    }
    if textual_key {
        names.push("__wireEntries");
    }
    // Store queries chain through it (#3189); an agent with no collection has none.
    if !names.is_empty() {
        names.push("__bynkThen");
    }
    if agent_reads_lazily(state, tys, a) {
        names.extend(["__attachLazy", "__readsLazily"]);
    }
    names
}

/// Whether an agent's collections can be read lazily (#3189): it has a `Map`, `Set`, `Cache` or
/// `Log` field and none is a held `Map` (a held map keeps live connections inline in the record).
/// `loadState` still reads a wholesale record, or one a migration step covers, whole.
fn agent_reads_lazily(state: &[StoreFieldIr], tys: &Types, a: &AgentDecl) -> bool {
    held_map_fields(a, state, tys).is_empty()
        && state
            .iter()
            .any(|f| !matches!(f.kind, StoreKindIr::Cell(_)))
}

/// The `[lo, from]` schema-version ranges the migration steps cover, as the runtime's
/// `__checkStoredSchemaVersion` and `__readStored` take them.
fn step_ranges_expr(step_plans: &[bynk_check::schema_registry::MigrationStep]) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Array {
        items: step_plans
            .iter()
            .map(|s| bynk_ts::TsExpr::Array {
                items: vec![
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(s.lo.to_string())),
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(s.from.to_string())),
                ],
                multiline: false,
            })
            .collect(),
        multiline: false,
    }
}

/// The per-field decode and encode closures the storage-backed handles of a lazily read agent
/// are attached with (#3189): a stored value decodes when an entry is first read (so a corrupt
/// one faults when touched), and a written value encodes as it is staged. Returns the const's
/// name, or `None` when the agent does not read lazily.
fn emit_lazy_field_spec(
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    stmts: &mut Vec<bynk_ts::TsStmt>,
) -> Option<String> {
    if !agent_reads_lazily(layout.state, commons.tys(), a) {
        return None;
    }
    let agent_name = &a.name.name;
    // `value_ts` is the field's value type: the encoder takes `unknown` and narrows it once.
    let codec = |ty: &TypeRef, path: &str, value_ts: &str| -> String {
        if !is_codecable(ty) {
            return String::new();
        }
        let d = bynk_ts::print_expr(&serialisation::deserialise_expr(
            ty,
            "(__w as unknown as __JsonValue)",
            path,
            &ctx.runtime_use,
        ));
        let e = bynk_ts::print_expr(&serialisation::serialise_text(ty, "__v", &ctx.runtime_use));
        format!(
            ", decode: (__w: unknown) => {{ const __r = {d}; if (__r.tag === \"Err\") throw __rehydrationViolation(\"{agent_name}\", __r.error); return __r.value; }}, encode: (__u: unknown) => {{ const __v = __u as {value_ts}; return {e}; }}"
        )
    };
    // A refined textual `Map` key or `Set` element is checked as it is read back, as the whole-record
    // gate checks every one at load (a key the program supplies is typed and is not checked).
    let key_check = |key_ty: Option<&TypeRef>, path: &str| -> String {
        let Some(k) = key_ty.filter(|k| type_base_is_string(k, &commons.types)) else {
            return String::new();
        };
        let d = bynk_ts::print_expr(&serialisation::deserialise_expr(
            k,
            "(__k as unknown as __JsonValue)",
            path,
            &ctx.runtime_use,
        ));
        format!(
            ", checkKey: (__k: string) => {{ const __r = {d}; if (__r.tag === \"Err\") throw __rehydrationViolation(\"{agent_name}\", __r.error); }}"
        )
    };
    let field = |name: &str, kind: &str, tail: String| {
        format!("{{ name: \"{name}\", kind: \"{kind}\"{tail} }}")
    };
    let mut spec: Vec<String> = Vec::new();
    for (name, v) in &layout.store_map_fields {
        let ts = layout
            .map_value_ts
            .get(&name.name)
            .map(bynk_ts::print_type)
            .unwrap_or_default();
        let checked = key_check(store_field_key_type(a, &name.name), &name.name);
        spec.push(field(
            &name.name,
            "map",
            codec(v, &name.name, &ts) + &checked,
        ));
    }
    for (name, elem) in &layout.store_set_fields {
        spec.push(field(&name.name, "set", key_check(Some(elem), &name.name)));
    }
    for (name, v, _) in &layout.store_cache_fields {
        let ts = match layout.store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Cache(_, v_ty, _)) => {
                bynk_ts::print_type(&ts_ty_to_ts_type(*v_ty, UnitFacts::of(ctx).render(commons)))
            }
            _ => ts_type_ref(v),
        };
        spec.push(field(&name.name, "cache", codec(v, &name.name, &ts)));
    }
    for (name, v, _) in &layout.store_log_fields {
        let ts = layout
            .log_value_ts
            .get(&name.name)
            .map(bynk_ts::print_type)
            .unwrap_or_default();
        spec.push(field(&name.name, "log", codec(v, &name.name, &ts)));
    }
    let name = format!("__lazyFieldsOf{agent_name}");
    stmts.push(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(name.clone()),
        None,
        bynk_ts::TsExpr::VerbatimExpr(
            format!("[{}] as const", spec.join(", ")),
            bynk_ts::VerbatimOrigin::Emit,
        ),
        None,
    ));
    Some(name)
}

/// v0.104/v0.105 (real-time track slice 3b): an agent has at least one `store
/// Map[K, V]` whose value `V` is a held resource (a `Connection`). On Workers a
/// live socket cannot serialise, so the durable record persists the **connection
/// id** (`Record<K, connId>`) and each `Connection` is re-resolved from its connId
/// via the hibernatable-socket API — so a stored connection survives DO eviction
/// (§2.9.6). Drives the `resolveConnection`/`connIdOf` imports and the held-map
/// realisation in `emit_agent`. (Bundle keeps held maps in the in-memory test
/// state record of `TestConnection`s — its tested behaviour — so this is
/// Workers-only.)
pub(crate) fn agent_has_held_storage(a: &AgentDecl, state: &[StoreFieldIr], tys: &Types) -> bool {
    !held_map_fields(a, state, tys).is_empty()
}

/// The agent's `store Map[K, V]` fields whose value is a held `Connection`, as
/// `(name, value-type)`. The value type is the held `Connection[F]` itself (so
/// its TS rendering is `Connection<F>`).
pub(crate) fn held_map_fields<'a>(
    a: &'a AgentDecl,
    state: &[StoreFieldIr],
    tys: &Types,
) -> Vec<(&'a Ident, &'a TypeRef)> {
    // 1A.33: a held map is a `Map` whose resolved value type is a held `Connection` (looking
    // through `Option`/`Effect`, as the checker's held rule does), not an AST head-name test.
    store_kinds(a, state)
        .filter_map(|(f, kind)| match kind {
            StoreKindIr::Map(_, v) if held_frame_ty(*v, tys).is_some() => {
                Some((&f.name, &f.kind.args[1]))
            }
            _ => None,
        })
        .collect()
}

/// #1187's Agent state-field slice 2c: a held `Map[K, V]`'s own frame type
/// `F`, resolved at the `TyId` level — `V` may be a bare `Connection[F]`, or
/// `F` may sit behind an `Option`/`Effect` wrapper (`Map[K,
/// Option[Connection[F]]]` is checker-legal: `type_ref_is_held`,
/// `bynk-check/src/context_checks.rs`, recurses through both to decide
/// storage admission). Mirrors that recursion at the `TyId` level rather
/// than reusing `Ty::is_held()`/`Ty::held_inner()` (`bynk-check/src/
/// checker.rs`): neither recurses past a bare `Connection` today (both are
/// otherwise dead code — no caller anywhere in this workspace), so using
/// them as-is here would silently reintroduce the same Option-blind gap
/// this slice fixes.
///
/// Tested at the `Types`/`TyId` level directly ([`held_frame_ty_tests`]),
/// not through a full end-to-end fixture: a real `store x: Map[K,
/// Option[Connection[F]]]` program certifies (`bynk check` exits 0), but
/// `bynk compile` panics before reaching this function at all —
/// `bynk-check::wire::codec_suffix`'s `TypeRef::Connection(..) =>
/// unreachable!(...)` arm, reached because whatever builds this project's
/// wire/codec instantiation table walks every store field's value type
/// looking for boundary-codec names, `Option`-recursing into a held
/// `Connection` it should have excluded. Confirmed pre-existing and
/// unrelated to this slice (reproduces identically on `main`, before this
/// function existed) — a real `bynk-check` bug, out of scope for this
/// emitter-only cutover; worth its own follow-up issue.
fn held_frame_ty(ty: TyId, tys: &Types) -> Option<TyId> {
    match &*tys.get(ty) {
        Ty::Connection(inner) => Some(*inner),
        Ty::Option(inner) | Ty::Effect(inner) => held_frame_ty(*inner, tys),
        _ => None,
    }
}

/// [`held_frame_ty`]'s `TypeRef`-level mirror, and `held_maps_ts`'s own
/// fallback when the frame's `TyId` doesn't resolve (a held field's own
/// type reference is never checker-validated, only its shape — see that
/// call site's own doc comment). Same `Connection`/`Option`/`Effect`
/// recursion as `type_ref_is_held` (`bynk-check/src/context_checks.rs`),
/// applied here at the AST level instead of just to a bare `Connection` the
/// way this function's own pre-#1187 shape did — so the fallback path
/// renders the same frame type the `TyId` path would have, not the whole
/// `Option<Connection<F>>`/`Effect<Connection<F>>` wrapper.
fn held_frame_ty_ref(t: &TypeRef) -> &TypeRef {
    match t {
        TypeRef::Connection(inner, _) | TypeRef::Option(inner, _) | TypeRef::Effect(inner, _) => {
            held_frame_ty_ref(inner)
        }
        _ => t,
    }
}

#[cfg(test)]
mod held_frame_ty_tests {
    use super::*;

    #[test]
    fn unwraps_a_bare_connection() {
        let tys = Types::new();
        let frame = tys.intern(Ty::Base(BaseType::String));
        let conn = tys.intern(Ty::Connection(frame));
        assert!(matches!(held_frame_ty(conn, &tys), Some(f) if f == frame));
    }

    /// The shape a naive, non-recursive port of `Ty::held_inner()` would
    /// get wrong — the same gap this slice found in the pre-existing
    /// `held_maps_ts` construction (a bare `match v { TypeRef::Connection
    /// (inner, _) => …, _ => ts_type_ref(v) }` that rendered the whole
    /// `Option<Connection<F>>` wrapper instead of unwrapping to `F`).
    #[test]
    fn unwraps_through_option_and_effect_like_type_ref_is_held_does() {
        let tys = Types::new();
        let frame = tys.intern(Ty::Base(BaseType::String));
        let conn = tys.intern(Ty::Connection(frame));
        let opt = tys.intern(Ty::Option(conn));
        assert!(matches!(held_frame_ty(opt, &tys), Some(f) if f == frame));
        let eff = tys.intern(Ty::Effect(conn));
        assert!(matches!(held_frame_ty(eff, &tys), Some(f) if f == frame));
        // `Effect[Option[Connection[F]]]` — nested wrapping, both layers unwrap.
        let opt_eff = tys.intern(Ty::Effect(opt));
        assert!(matches!(held_frame_ty(opt_eff, &tys), Some(f) if f == frame));
    }

    #[test]
    fn a_non_held_type_returns_none_even_wrapped() {
        let tys = Types::new();
        let s = tys.intern(Ty::Base(BaseType::String));
        assert!(held_frame_ty(s, &tys).is_none());
        let opt_s = tys.intern(Ty::Option(s));
        assert!(held_frame_ty(opt_s, &tys).is_none());
    }
}

/// The key type (`K`) of a two-argument store field (`Map`/`Cache`) named
/// `field`, used by the rehydration gate to validate textual keys (ADR 0124).
fn store_field_key_type<'a>(a: &'a AgentDecl, field: &str) -> Option<&'a TypeRef> {
    a.store_fields
        .iter()
        .find(|f| f.name.name == field && f.kind.args.len() == 2)
        .map(|f| &f.kind.args[0])
}

/// True when a type's base is `String` — directly, or through a named refined /
/// opaque alias. A textual key persists as its own string in a storage `Record`,
/// so the rehydration gate can validate it; a non-textual key persists as a
/// `String(k)` structural key, whose refinement validation is deferred (ADR 0124).
fn type_base_is_string(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    match t {
        TypeRef::Base(BaseType::String, _) => true,
        TypeRef::Named(id) => matches!(
            types.get(&id.name).map(|d| &d.body),
            Some(TypeBody::Refined {
                base: BaseType::String,
                ..
            }) | Some(TypeBody::Opaque {
                base: BaseType::String,
                ..
            })
        ),
        _ => false,
    }
}

/// True when `t` is `Int` or a refined/opaque type over `Int`.
fn type_base_is_int(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    match t {
        TypeRef::Base(BaseType::Int, _) => true,
        TypeRef::Named(id) => matches!(
            types.get(&id.name).map(|d| &d.body),
            Some(TypeBody::Refined {
                base: BaseType::Int,
                ..
            }) | Some(TypeBody::Opaque {
                base: BaseType::Int,
                ..
            })
        ),
        _ => false,
    }
}

/// v0.119: the `.call` variant tag for a handler — its name with the first letter
/// upper-cased (`spend` → `Spend`). Must match the checker's synthesised sum
/// variant (`tests_emit::history_variant_name`), which the reader matches with
/// `is` / `match`.
fn history_variant_tag(handler: &str) -> String {
    let mut c = handler.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => handler.to_string(),
    }
}

/// #1482: returns a real `Vec<bynk_ts::TsStmt>` (was `out: &mut String` plus
/// a `source_map: Option<&RefCell<SourceMapBuilder>>` parameter) — the
/// state interface/registry/zero-factory/rehydration-gate/construction-
/// factory are each already real, fully-converted top-level nodes (#1367/
/// #1369/#1371/#1377) and now push directly into the returned `Vec` instead
/// of being printed through the removed `out` parameter (per #1462's own
/// confirmed 66-site "already-argued print-and-splice" bucket — no internal
/// construction logic changes here, only the outer plumbing). Two sites stay
/// genuinely opaque, each wrapped as its own `TsStmt::raw`, mirroring
/// `emit_provider`'s own Decision-C precedent (#1480):
///
/// - **The DO class** (`export class {Name} { ... }`) — `emit_agent`'s own
///   instance of `emit_provider`'s Decision C (the class's own header/
///   fields/constructor stay hand-written text, cited in place below); every
///   real per-method/per-handler print-and-splice site inside it (`loadState`/
///   `commitState`/the per-handler loop/the WS-hosted DO methods/`fetch`)
///   still builds and merges exactly as before, just into a local buffer/map
///   pair instead of this function's own former `out`/`source_map`
///   parameters — captured as ONE `TsStmt::raw` carrying a class-wide
///   `nested_map` that combines every one of those merges, the same
///   two-level composition `emit_provider` already established.
/// - **The history-driver** (`__bynkDriveHistory_*`) — #1386's own standing
///   decision, confirmed still-permanent by #1462's own correction (test-
///   support only, stripped from deploy, thinnest fixture coverage in the
///   track, its own per-handler discriminated-union algebra deferred to
///   `any` by design): genuinely raw hand-templated text throughout, zero
///   `bynk_ts` nodes or `LowerCtx` involvement anywhere in it, captured as a
///   second, separate `TsStmt::raw` with no `nested_map` (nothing to merge),
///   not decomposed further, wrapping the whole function's own text output.
///
/// The caller (`emitter.rs`) merges every returned node into its own real
/// module map via `print_stmt_and_merge`, the same composition
/// `emit_free_fn`/`emit_provider`/`emit_service` already use.
/// #1678: the name of an agent's wire table, `__<Agent>Wire`.
/// #1818: whether a handler of `a` reads the agent's own key as `self.<key>`.
/// Only such an agent carries its key (a `__key` field, set by the bundle
/// factory and, on workers, sent with each call), so every other agent's output
/// is unchanged.
pub(crate) fn agent_reads_key(a: &AgentDecl) -> bool {
    a.handlers.iter().any(|h| {
        let mut found = false;
        bynk_model::walk_block_exprs(&h.body, &mut |e| {
            if let ExprKind::FieldAccess { receiver, field } = &e.kind
                && matches!(&receiver.kind, ExprKind::Ident(id) if id.name == "self")
                && field.name == a.key_name.name
            {
                found = true;
            }
        });
        found
    })
}

/// #1818: `this.__key = __decodeAgentKey(<codec>, <json>) as <K>;`, setting the
/// key of an agent that reads it at an entry point: the agent dispatch, a
/// forwarded WebSocket upgrade, or a waking WebSocket message or close.
fn agent_key_assign(codec: &str, key_ts: &str, json: &str) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::assign(
        bynk_ts::TsExpr::Ident("this.__key".to_string()),
        bynk_ts::TsExpr::Ident(format!("__decodeAgentKey({codec}, {json}) as {key_ts}")),
        None,
    )
}

/// #1818: the name of an agent's key codec, emitted on workers for an agent
/// that reads its key.
pub(crate) fn agent_key_codec_name(agent: &str) -> String {
    format!("__{agent}Key")
}

fn agent_wire_name(agent: &str) -> String {
    format!("__{agent}Wire")
}

/// #1678 (runtime-semantics track S11): an agent's wire table on the `workers`
/// target, `const __<Agent>Wire: AgentWire = { <method>: { args, result } }`.
/// Each position's codec is the boundary codec's serialise/deserialise pair for
/// its type, the same helpers a cross-context call uses; the codec closure
/// (`agent_call_boundary_roots`) emits them in this module. A position with no
/// wire form passes through (`AGENT_WIRE_PASS`). The DO stub's proxy and the
/// DO's `fetch` both read it.
/// #1818: `const __<Agent>Key = <codec>`, the key type's wire codec, shared by
/// the caller's proxy (which encodes the key) and the DO (which decodes it).
fn agent_key_codec(a: &AgentDecl, ru: &crate::emitter::runtime_use::RuntimeUse) -> bynk_ts::TsStmt {
    // Exported: a WebSocket edge in `compose.ts` encodes the key it forwards
    // with the upgrade.
    bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: agent_key_codec_name(&a.name.name),
            ty: None,
            init: agent_wire_codec(&a.key_type, ru),
        })),
        None,
    )
}

/// One wire position's codec: `{ enc, dec }` from the type's boundary codec, or
/// the pass-through for a type with no wire form.
fn agent_wire_codec(t: &TypeRef, ru: &crate::emitter::runtime_use::RuntimeUse) -> bynk_ts::TsExpr {
    if serialisation::agent_wire_passes_through(t) {
        bynk_ts::TsExpr::Ident("__AGENT_WIRE_PASS".to_string())
    } else {
        bynk_ts::TsExpr::object(vec![
            (
                "enc".to_string(),
                serialisation::serialise_ref_via(t, "", ru),
            ),
            (
                "dec".to_string(),
                serialisation::deserialise_ref_via(t, "", ru),
            ),
        ])
    }
}

fn agent_wire_table(
    a: &AgentDecl,
    ru: &crate::emitter::runtime_use::RuntimeUse,
) -> bynk_ts::TsStmt {
    let codec = |t: &TypeRef| {
        if serialisation::agent_wire_passes_through(t) {
            bynk_ts::TsExpr::Ident("__AGENT_WIRE_PASS".to_string())
        } else {
            bynk_ts::TsExpr::object(vec![
                (
                    "enc".to_string(),
                    serialisation::serialise_ref_via(t, "", ru),
                ),
                (
                    "dec".to_string(),
                    serialisation::deserialise_ref_via(t, "", ru),
                ),
            ])
        }
    };
    let methods = a
        .handlers
        .iter()
        .map(|h| {
            let name = h
                .method_name
                .as_ref()
                .map(|m| m.name.clone())
                .unwrap_or_else(|| "call".to_string());
            let args =
                bynk_ts::TsExpr::array(h.params.iter().map(|p| codec(&p.type_ref)).collect());
            (
                name,
                bynk_ts::TsExpr::object(vec![
                    ("args".to_string(), args),
                    ("result".to_string(), codec(&h.return_type)),
                ]),
            )
        })
        .collect();
    let mut table = bynk_ts::TsExpr::object(methods);
    if let bynk_ts::TsExpr::Object { multiline, .. } = &mut table {
        *multiline = true;
    }
    bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::ConstDecl {
            name: agent_wire_name(&a.name.name),
            ty: Some(bynk_ts::TsType::named("__AgentWire")),
            init: table,
        },
        None,
    )
}

/// #2272: the `path:line:col` of a declaration's name in its `.bynk` file, as a
/// violation reports it. The path is the project-relative identity the test runner
/// uses, so an editor resolves it the same way it resolves an `expect`.
fn declaration_location(
    ctx: &EmitProjectCtx,
    source_text: &str,
    span: bynk_syntax::span::Span,
) -> String {
    let (line, col) = bynk_syntax::span::line_col(source_text, span.start);
    format!("{}:{line}:{col}", ctx.identity_path.replace('\\', "/"))
}

/// How an agent's `store` fields classify, read once from the agent's
/// declaration and the checker's resolved [`StoreFieldIr`]s (#3167): the `Cell`
/// fields that form the state record, the plain and held `Map`s, the `Set`s,
/// `Cache`s and `Log`s, and the per-field facts (TS types, TTLs, retain windows,
/// secondary indexes) the state record, its load/commit and the handler
/// lowering each need. Computed once so those steps share one classification.
pub(crate) struct AgentStoreLayout<'a> {
    /// The checker's resolved `store` fields, in declaration order (1A.33).
    pub(crate) state: &'a [StoreFieldIr],
    /// Every `store Map` whose value is a held `Connection`, on any target. `held_maps` is
    /// this on Workers and empty elsewhere, where a held map is not persisted by connection id.
    pub(crate) all_held_maps: Vec<(&'a Ident, &'a TypeRef)>,
    pub(crate) store_field_ty: HashMap<&'a str, &'a StoreKindIr>,
    pub(crate) effective_fields: Vec<RecordField>,
    pub(crate) is_workers: bool,
    pub(crate) held_maps: Vec<(&'a Ident, &'a TypeRef)>,
    pub(crate) held_maps_ts: HashMap<String, bynk_ts::TsType>,
    pub(crate) store_map_fields: Vec<(&'a Ident, &'a TypeRef)>,
    pub(crate) map_names: HashSet<String>,
    pub(crate) map_value_ts: HashMap<String, bynk_ts::TsType>,
    pub(crate) store_map_indexes: HashMap<String, Vec<String>>,
    pub(crate) store_set_fields: Vec<(&'a Ident, &'a TypeRef)>,
    pub(crate) set_field_names: Vec<&'a str>,
    pub(crate) set_names: HashSet<String>,
    pub(crate) store_cache_fields: Vec<(&'a Ident, &'a TypeRef, i64)>,
    pub(crate) cache_ttls: HashMap<String, i64>,
    pub(crate) store_log_fields: Vec<(&'a Ident, &'a TypeRef, Option<i64>)>,
    pub(crate) log_retains: HashMap<String, Option<i64>>,
    pub(crate) log_value_ts: HashMap<String, bynk_ts::TsType>,
}

/// Classify an agent's `store` fields (see [`AgentStoreLayout`]).
fn classify_agent_store<'a>(
    a: &'a AgentDecl,
    state: &'a [StoreFieldIr],
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> AgentStoreLayout<'a> {
    let tys = UnitFacts::of(ctx).render(commons);
    // #1187's Agent state-field slice: the checker-resolved TyId for each
    // Cell/Map/Cache/Log field's element type, keyed by field name — read
    // once here so the state-interface block below can render through
    // `ts_ty` (TyId) instead of `ts_type_ref` (raw AST) without repeating
    // this lookup at every one of its four sites.
    let store_field_ty: HashMap<&str, &StoreKindIr> =
        state.iter().map(|f| (f.field.as_str(), &f.kind)).collect();
    let is_store_agent = true;
    // v0.81 (storage track, ADR 0109): an agent's `Cell` fields ARE its state
    // record, so the whole state machinery (interface, zero factory, load/commit,
    // invariant gate) derives the record fields from the cells. Each `Cell[T]`
    // field becomes a `T`-typed record field carrying the cell's initialiser.
    // Handler bodies lower as bare reads / `:=` over `__state`, with an implicit
    // commit at handler end.
    // P6.53 (design/tracks/the-ir.md §6b): membership reads `state`'s own
    // typed `StoreKindIr::Cell` instead of a string comparison against
    // `f.kind.head.name` — a user type genuinely named `Cell` (with a
    // different arity or an unrelated shape) cannot silently pass this
    // check when the field's own already-checked kind says otherwise. The
    // field's own `TypeRef`/`init` stay AST-typed (rendering parameters
    // outside this slice's scope).
    let effective_fields: Vec<RecordField> = a
        .store_fields
        .iter()
        .filter(|f| {
            matches!(
                store_field_ty.get(f.name.name.as_str()),
                Some(StoreKindIr::Cell(_))
            )
        })
        .map(|f| RecordField {
            annotations: Vec::new(),
            documentation: None,
            trivia: Default::default(),
            name: f.name.clone(),
            type_ref: f.kind.args[0].clone(),
            refinement: None,
            init: f.init.clone(),
            span: f.span,
        })
        .collect();
    // v0.82 (ADR 0110): `store Map[K, V]` fields are also state-record fields, but
    // persisted as a JSON-serialisable `Record<string, V>` (the value `Map` is a
    // JS `Map`, which does not serialise). Collected separately from `Cell` fields
    // since their TS type and zero differ; the working record is committed by the
    // same flush. `(name, V)`.
    // v0.104/v0.105 (real-time track slice 3b): on Workers, a `store Map[K,
    // Connection]` holds live sockets that cannot themselves be JSON-persisted.
    // v0.105 (slice 3b-ii) persists the **connection id** instead: the held map is a
    // durable `Record<string(K), connId>` in the state record, and each `Connection`
    // is re-resolved from its connId via the platform's hibernatable-socket API, so a
    // stored connection survives DO eviction (§2.9.6). The held maps are kept *out*
    // of `store_map_fields` (their value type and entry lowering differ from a plain
    // `Map[K, V]`) but are emitted into the state interface / zero / rehydration
    // key-check / load-commit as string records, and they trigger the commit flush
    // like any other persisted field. (Bundle keeps held maps in the in-memory test
    // state record of `TestConnection`s — its tested behaviour — so the connId
    // representation is Workers-only.)
    let is_workers = matches!(ctx.target, BuildTarget::Workers);
    let all_held_maps = held_map_fields(a, state, commons.tys());
    let held_maps: Vec<(&Ident, &TypeRef)> = if is_workers {
        all_held_maps.clone()
    } else {
        Vec::new()
    };
    let held_map_names: HashSet<String> = held_maps.iter().map(|(n, _)| n.name.clone()).collect();
    // The held map's lowering needs the connection's **frame type** `F` (for
    // `resolveConnection<F>`), not the `Connection<F>` wrapper — extract it.
    // #1187's Agent state-field slice 2c: the field's own membership in
    // `held_maps` still comes from `held_map_fields`'s AST-based
    // `type_ref_is_held` (unchanged — its caller, `write_header`
    // (`emitter.rs`), has no `CheckedProgram` in scope, so this and that
    // predicate stay deliberately parallel, not unified), but the frame
    // type itself is now read off `state`'s already-resolved `TyId` via
    // `held_frame_ty`, through `ts_ty` — fixing a real, previously
    // uncovered gap: the old `match v { TypeRef::Connection(inner, _) =>
    // ts_type_ref(inner), _ => ts_type_ref(v) }` only ever unwrapped a
    // *bare* `Connection`; a `Map[K, Option[Connection[F]]]` value — legal
    // per the checker's own `type_ref_is_held`, which recurses through
    // `Option`/`Effect` (`bynk-check/src/context_checks.rs`) — fell into
    // the `_` arm and rendered the whole `Option<Connection<F>>` wrapper,
    // not `F`. No existing fixture exercised this shape to catch it.
    let held_maps_ts: HashMap<String, bynk_ts::TsType> = held_maps
        .iter()
        .map(|(n, v)| {
            // Review of #1187's slice 2c: no checker pass validates a store
            // field's own type reference (only its shape —
            // `resolve_store_field_ty`'s own doc comment, `ir/lower.rs`,
            // names this explicitly), so `store x: Map[K, Connection[Bogus]]`
            // certifies today with an unresolvable frame type; the whole
            // `Connection[Bogus]` then fails to resolve too
            // (`resolve_type_ref_in`'s `?` propagation, `bynk-check/src/
            // checker.rs`), so `StoreKindIr::Map`'s own value `TyId` falls
            // back to `Ty::Unit` — `held_frame_ty` legitimately returns
            // `None` on a certified program here, not an internal
            // inconsistency. Fall back to the AST-level frame type (still
            // Option/Effect-unwrapped, via `held_frame_ty_ref`) instead of
            // panicking, mirroring `resolve_store_field_ty`'s own posture
            // for the identical reason.
            let ts = store_field_ty
                .get(n.name.as_str())
                .and_then(|kind| match kind {
                    StoreKindIr::Map(_, v_ty) => held_frame_ty(*v_ty, tys.types()),
                    _ => None,
                })
                .map(|frame_ty| ts_ty_to_ts_type(frame_ty, tys))
                .unwrap_or_else(|| ts_type_ref_to_ts_type(held_frame_ty_ref(v), None));
            (n.name.clone(), ts)
        })
        .collect();
    // P6.53 (design/tracks/the-ir.md §6b): typed membership, same reasoning
    // as `effective_fields` above.
    let store_map_fields: Vec<(&Ident, &TypeRef)> = if is_store_agent {
        a.store_fields
            .iter()
            .filter(|f| {
                matches!(
                    store_field_ty.get(f.name.name.as_str()),
                    Some(StoreKindIr::Map(..))
                )
            })
            .filter(|f| !held_map_names.contains(&f.name.name))
            .map(|f| (&f.name, &f.kind.args[1]))
            .collect()
    } else {
        Vec::new()
    };
    let map_names: HashSet<String> = store_map_fields
        .iter()
        .map(|(n, _)| n.name.clone())
        .collect();
    // Review of #1460: a plain map's own value type `V`, mirroring
    // `held_maps_ts`'s exact construction (above) — needed so
    // `lower_query_method`'s collection-kernel sites can narrow a plain map's
    // own value scan lifted into the general query vocabulary the same way a
    // real `store Query[T]` field already can.
    let map_value_ts: HashMap<String, bynk_ts::TsType> = store_map_fields
        .iter()
        .map(|(n, v)| {
            let ts = store_field_ty
                .get(n.name.as_str())
                .and_then(|kind| match kind {
                    StoreKindIr::Map(_, v_ty) => Some(ts_ty_to_ts_type(*v_ty, tys)),
                    _ => None,
                })
                .unwrap_or_else(|| ts_type_ref_to_ts_type(v, None));
            (n.name.clone(), ts)
        })
        .collect();
    // v0.93 (ADR 0118): `store Map[K, V] @indexed(by: f, …)` — each `by:` field
    // gets a maintained secondary index. `map name → [field, …]` (a deduped,
    // declaration-ordered list). The keys are validated against `V` in
    // `project::validate`; here we only read the surface to drive emission.
    // #1187's Agent state-field slice 2b: which `Map` fields have an
    // `@indexed(by: …)` key list, IR-sourced — `StoreFieldIr::indexed` is
    // already the deduped, declaration-order key list
    // `store_field_kind_and_indexed` built from the same annotations this
    // used to re-walk here. Deliberately still keyed and filtered exactly
    // like the AST walk it replaces: no held-map exclusion (a held
    // `Map[K, Connection]` is an ordinary `StoreKindIr::Map` as far as the
    // checker/IR are concerned — "held" is purely this function's own
    // downstream Workers-mode concern, applied separately to `store_map_fields`
    // above), so a held map's own indexes are still emitted here exactly as
    // before.
    let store_map_indexes: HashMap<String, Vec<String>> = if is_store_agent {
        state
            .iter()
            .filter(|f| matches!(f.kind, StoreKindIr::Map(..)) && !f.indexed.is_empty())
            .map(|f| (f.field.clone(), f.indexed.clone()))
            .collect()
    } else {
        HashMap::new()
    };
    // v0.83 (ADR 0110): `store Set[T]` fields are state-record fields too,
    // persisted as a JSON-serialisable `Record<string, boolean>` (a JS `Set`
    // does not serialise). `(name, T)`; the element type is unused in the TS
    // representation but kept for symmetry with maps — needed only for the
    // rehydration check below, which still validates against a raw `TypeRef`
    // (`serialisation.rs`'s own `TypeRef`-driven boundary, out of this
    // slice's scope).
    // P6.53 (design/tracks/the-ir.md §6b): typed membership, same reasoning
    // as `effective_fields` above.
    let store_set_fields: Vec<(&Ident, &TypeRef)> = if is_store_agent {
        a.store_fields
            .iter()
            .filter(|f| {
                matches!(
                    store_field_ty.get(f.name.name.as_str()),
                    Some(StoreKindIr::Set(_))
                )
            })
            .map(|f| (&f.name, &f.kind.args[0]))
            .collect()
    } else {
        Vec::new()
    };
    // #1187's Agent state-field slice 2b: which fields are Sets, IR-sourced
    // (declaration order preserved — `state` is built from `a.store_fields`
    // in order) — every consumer below except the rehydration check, which
    // alone needs `store_set_fields`'s own `TypeRef`.
    let set_field_names: Vec<&str> = state
        .iter()
        .filter(|f| matches!(f.kind, StoreKindIr::Set(_)))
        .map(|f| f.field.as_str())
        .collect();
    let set_names: HashSet<String> = set_field_names.iter().map(|s| s.to_string()).collect();
    // v0.87 (ADR 0113): `store Cache[K, V] @ttl(d)` fields — a value record plus
    // a per-entry expiry instant. `(name, V, ttl-millis)`; the ttl is the field's
    // `@ttl` Duration literal (validated by the checker).
    // P6.53 (design/tracks/the-ir.md §6b): `ttl` reads `state`'s own
    // already-computed `StoreKindIr::Cache(_, _, ttl)` (`store_field_ty`,
    // built above from `lower_store_field_shape_ir` →
    // `store_field_kind_and_indexed` → `duration_millis_annotation`)
    // instead of re-walking `f.annotations`/`ExprKind::DurationLit` a
    // second time — the two walks could diverge and emit a wrong TTL with
    // nothing to catch it. `?` on a miss preserves the original's own
    // defensive drop (an internal-consistency signal, not a real-world
    // case: every `Cache`-kind field in `a.store_fields` has a matching
    // `StoreKindIr::Cache` entry by construction).
    let store_cache_fields: Vec<(&Ident, &TypeRef, i64)> = if is_store_agent {
        a.store_fields
            .iter()
            .filter_map(|f| {
                let StoreKindIr::Cache(_, _, ttl) = store_field_ty.get(f.name.name.as_str())?
                else {
                    return None;
                };
                Some((&f.name, &f.kind.args[1], *ttl))
            })
            .collect()
    } else {
        Vec::new()
    };
    let cache_ttls: HashMap<String, i64> = store_cache_fields
        .iter()
        .map(|(n, _, ttl)| (n.name.clone(), *ttl))
        .collect();
    // v0.95 (ADR 0121): `store Log[T] [@retain(d)]` fields — an ordered array of
    // `{ t, v }` entries. `(name, T, optional retain-millis)`; the retain (from
    // `@retain`) prunes on append.
    // P6.53 (design/tracks/the-ir.md §6b): same dedup as `store_cache_fields`
    // above — `retain` reads `StoreKindIr::Log(_, retain)` instead of
    // re-walking `f.annotations` a second time. Every `Log`-kind field in
    // `a.store_fields` has a matching `StoreKindIr::Log` entry by
    // construction; `.and_then` (not `?`) preserves the original's own
    // per-field presence (every filtered field stays, `retain` is simply
    // `None` when the field has no `@retain`).
    let store_log_fields: Vec<(&Ident, &TypeRef, Option<i64>)> = if is_store_agent {
        a.store_fields
            .iter()
            .filter_map(|f| {
                let StoreKindIr::Log(_, retain) = store_field_ty.get(f.name.name.as_str())? else {
                    return None;
                };
                Some((&f.name, &f.kind.args[0], *retain))
            })
            .collect()
    } else {
        Vec::new()
    };
    let log_retains: HashMap<String, Option<i64>> = store_log_fields
        .iter()
        .map(|(n, _, r)| (n.name.clone(), *r))
        .collect();
    // Review of #1460: a Log's own element type `T`, same reason and
    // construction as `map_value_ts` above.
    let log_value_ts: HashMap<String, bynk_ts::TsType> = store_log_fields
        .iter()
        .map(|(n, v, _)| {
            let ts = store_field_ty
                .get(n.name.as_str())
                .and_then(|kind| match kind {
                    StoreKindIr::Log(elem_ty, _) => Some(ts_ty_to_ts_type(*elem_ty, tys)),
                    _ => None,
                })
                .unwrap_or_else(|| ts_type_ref_to_ts_type(v, None));
            (n.name.clone(), ts)
        })
        .collect();
    AgentStoreLayout {
        state,
        all_held_maps,
        store_field_ty,
        effective_fields,
        is_workers,
        held_maps,
        held_maps_ts,
        store_map_fields,
        map_names,
        map_value_ts,
        store_map_indexes,
        store_set_fields,
        set_field_names,
        set_names,
        store_cache_fields,
        cache_ttls,
        store_log_fields,
        log_retains,
        log_value_ts,
    }
}

/// The names and facts about an agent's state that `loadState` refers to: the
/// fresh-state factory, the rehydration/migration functions, and the schema
/// versions the stored layout is checked against (#3167).
struct AgentLoadFns<'a> {
    zero_fn: &'a String,
    fields_const: &'a String,
    rehydrate_fn: &'a String,
    encode_fn: &'a String,
    migrate_fn: &'a String,
    /// The function that fills a loaded state's collection handles from the stored
    /// record, when the agent has any (#3254).
    hydrate_fn: &'a String,
    has_rehydrate: bool,
    /// Whether the agent has a collection field (#3254).
    has_collections: bool,
    /// Whether `commitState` writes through the encoder: the agent has a value that
    /// needs the codec, or a collection handle that must be put in wire form.
    has_encode: bool,
    step_plans: &'a [bynk_check::schema_registry::MigrationStep],
    agent_versions: &'a Option<bynk_check::schema_registry::AgentVersions>,
    /// The decode/encode spec a lazily read agent's storage-backed handles attach with (#3189).
    lazy_fields: Option<&'a String>,
}

/// An agent class's two private state methods, `loadState` and `commitState`
/// (#3167): load reads, rehydrates and migrates the stored record; commit
/// checks the agent's invariants and transitions and writes it back. Kept
/// together as one unit, and written onto `out` in that order, because the two
/// are the agent's whole storage contract with the platform.
#[allow(clippy::too_many_arguments)]
fn emit_agent_load_commit_state(
    methods: &mut Vec<bynk_ts::TsClassMethod>,
    a: &AgentDecl,
    effective_fields: &[RecordField],
    fns: &AgentLoadFns,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    source_text: &str,
) {
    let state_ty = format!("{}State", a.name.name);
    let AgentLoadFns {
        zero_fn,
        fields_const,
        rehydrate_fn,
        encode_fn,
        migrate_fn,
        hydrate_fn,
        has_rehydrate,
        has_collections,
        has_encode,
        step_plans,
        agent_versions,
        lazy_fields,
    } = *fns;
    // Arc C, slice 21 (#1371): the third of step (9)'s own proposed 5-6
    // sub-slices — the class's own wrapper (header/fields/constructor) stays
    // hand-written text (Decision C, the same boundary #1359's own
    // `emit_provider` already established), but `loadState` converts fully
    // to a real `bynk_ts::TsClassMethod` fragment, printed through
    // `print_class_method` (the same fragment entry point #1359 added).
    // `commitState`, this class's OTHER private method, stays deferred as
    // its own separate, later sub-slice — a real, harder remainder found
    // only once actually read (its own invariant/transition predicate
    // lowering writes directly into `out` today, the same "direct write,
    // real position" shape `emit_free_fn`/`emit_contract_guarded_body` had
    // *before* their own conversions needed a sub-builder for `Raw`-
    // embedding, #1352/#1353), not silently folded into "step (9), slice 3
    // done." `this.state.storage.get<{state_ty}>(...)`'s own generic method
    // call has no representation in `TsExpr::Call` (`type_args` was never
    // added — 41 real construction sites across the workspace, far more
    // than a single narrow need like this one justifies touching) — carried
    // as one opaque callee leaf, the same "an odd, one-off shape stays
    // opaque text" precedent P7.9's own `Query[T]` and #1357's own `unique
    // symbol` already established, not a new pattern; now tagged
    // `VerbatimExpr`, not `Ident` (#1539).
    // `TsClassMethod.private` is a real, new, small gap the grounding pass
    // itself predicted (#1366) — `loadState`/`commitState` are the first
    // `private` method sites this whole track has hit (`emit_provider`'s
    // own ops, #1359, were all public).
    let load_state_body = {
        let mut stmts = vec![
            bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("stored".to_string()),
                None,
                bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::VerbatimExpr(
                        format!("this.state.storage.get<{state_ty}>"),
                        bynk_ts::VerbatimOrigin::Emit,
                    )),
                    args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                        "state".to_string(),
                    ))],
                })),
                None,
            ),
            // v0.96 (ADR 0124): a fresh key takes its zero (valid by
            // construction). For a stored record, merge zero-then-stored —
            // D4: a `store` field added in a later deploy and absent from
            // the persisted record takes its default, rather than reading
            // `undefined` — then run the rehydration validation gate on the
            // merged state before any handler reads it (D1/D2).
            bynk_ts::TsStmt::if_stmt(
                bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::StrictEq,
                    left: Box::new(bynk_ts::TsExpr::Ident("stored".to_string())),
                    right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                },
                bynk_ts::TsStmt::return_stmt(
                    Some(bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Ident(zero_fn.clone())),
                        args: Vec::new(),
                    }),
                    None,
                ),
                None,
            ),
        ];
        // Per-entry storage slice 0 (#3189): any `__layout` marker was written
        // by a later build, so fault before anything is decoded, merged or
        // written back. Unconditional: a lockless agent has no `__v` check.
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__checkStoredLayout".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(a.name.name.clone())),
                    bynk_ts::TsExpr::Ident("stored".to_string()),
                ],
            },
            None,
        ));
        // State-migrations S1 (#2211, track §3.2, Decision D): with the
        // schema lock, check the raw record's `__v` marker before anything is
        // merged or decoded, so a record from a newer build (or with no path
        // forward) faults without being read or written back.
        if let Some(v) = &agent_versions {
            stmts.push(bynk_ts::TsStmt::expr_stmt(
                bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Ident(
                        "__checkStoredSchemaVersion".to_string(),
                    )),
                    args: vec![
                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(a.name.name.clone())),
                        bynk_ts::TsExpr::Ident("stored".to_string()),
                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(v.version.to_string())),
                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(v.oldest_readable.to_string())),
                    ]
                    .into_iter()
                    // State-migrations S2 (#2224 Decision C): each step's
                    // covered range makes those versions readable too.
                    .chain((!step_plans.is_empty()).then(|| step_ranges_expr(step_plans)))
                    .collect(),
                },
                None,
            ));
        }
        // Per-entry storage (#3189): join the entries a per-entry record keeps
        // outside `"state"` back into the wholesale-shaped record (a wholesale one
        // is returned as it is, and split by its first commit). Everything below
        // reads the joined record, so a migration step and the rehydration gate
        // see what they always saw.
        stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__wholesale".to_string()),
            None,
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__readStored".to_string())),
                args: [
                    bynk_ts::TsExpr::Ident("this".to_string()),
                    bynk_ts::TsExpr::Ident("this.state.storage".to_string()),
                    bynk_ts::TsExpr::Ident("stored".to_string()),
                ]
                .into_iter()
                // #3189: an agent that reads lazily says which stored versions a migration
                // step covers, which `readStored` still reads whole.
                .chain(lazy_fields.map(|_| step_ranges_expr(step_plans)))
                .collect(),
            })),
            None,
        ));
        // State-migrations S2 (#2224 Decision D): a record a step covers runs
        // it first; everything below reads the step's output.
        let src = if step_plans.is_empty() {
            "__wholesale"
        } else {
            let as_ty = |expr: bynk_ts::TsExpr, ty: bynk_ts::TsType| bynk_ts::TsExpr::As {
                expr: Box::new(bynk_ts::TsExpr::As {
                    expr: Box::new(expr),
                    ty: bynk_ts::TsType::named("unknown"),
                }),
                ty,
            };
            stmts.push(bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("__src".to_string()),
                None,
                as_ty(
                    bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Ident(migrate_fn.clone())),
                        args: vec![as_ty(
                            bynk_ts::TsExpr::Ident("__wholesale".to_string()),
                            bynk_ts::TsType::named("globalThis.Record<string, unknown>"),
                        )],
                    },
                    bynk_ts::TsType::named(state_ty.clone()),
                ),
                None,
            ));
            "__src"
        };
        stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__zero".to_string()),
            None,
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident(zero_fn.clone())),
                args: Vec::new(),
            },
            None,
        ));
        // Track §3.4 (Decision E): name, once per isolate, any stored key the
        // definition no longer declares. It stays in `__merged` and so is
        // written back on commit, never deleted.
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident(
                    "__reportUnknownStoredKeys".to_string(),
                )),
                args: vec![
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(a.name.name.clone())),
                    bynk_ts::TsExpr::Ident(src.to_string()),
                    bynk_ts::TsExpr::Ident("__zero".to_string()),
                ],
            },
            None,
        ));
        stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__merged".to_string()),
            None,
            bynk_ts::TsExpr::object_entries(vec![
                bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident("__zero".to_string())),
                bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(src.to_string())),
            ]),
            None,
        ));
        if has_rehydrate {
            stmts.push(bynk_ts::TsStmt::expr_stmt(
                bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Ident(rehydrate_fn.clone())),
                    args: vec![
                        bynk_ts::TsExpr::Ident("__merged".to_string()),
                        bynk_ts::TsExpr::Ident(src.to_string()),
                    ],
                },
                None,
            ));
        }
        // Per-collection handles (#3254): fill each collection the record carries into
        // its handle, in the order the record notes.
        if has_collections {
            stmts.push(hydrate_or_attach_stmt(hydrate_fn, src, lazy_fields));
        }
        stmts.push(bynk_ts::TsStmt::return_stmt(
            Some(bynk_ts::TsExpr::Ident("__merged".to_string())),
            None,
        ));
        stmts
    };
    let load_state_method = bynk_ts::TsClassMethod::new(
        "loadState",
        true,
        Vec::new(),
        Some(bynk_ts::TsType::named_with_args(
            "globalThis.Promise",
            vec![bynk_ts::TsType::named(state_ty.clone())],
        )),
        load_state_body,
    )
    .private();
    let class_depth = 0;
    methods.push(load_state_method);
    // Arc C, slice 22 (#1373): closes step (9)'s own third sub-slice by
    // landing the deferred remainder, `commitState` (#1371's own `loadState`
    // landed the sub-slice's other half). `record_span` is a documented
    // no-op when a `LowerCtx` has no attached builder (`emitter.rs`'s own
    // doc), and neither the invariant nor the transition `LowerCtx` below
    // is ever given one (`LowerCtx::new(...)`, never `.with_source_map(...)`
    // — `emit_agent`'s own local `source_map` binding isn't threaded into
    // either) — confirmed directly, not assumed, before writing this
    // comment. So despite the grounding pass's own earlier caution that
    // `commitState` would need the same sub-builder/merge care #1352/#1353
    // established, it genuinely does not: there is no source map being
    // written to here, today or after this conversion, so there is nothing
    // to preserve. Each invariant's/transition's own hoisted pre-statements
    // and predicate expression stay opaque (`lower.rs`'s own permanently-
    // excluded general expression lowering, ADR
    // `arc-c-lower-rs-permanent-exclusion`) — the hoisted lines as their own
    // `TsStmt::Raw` (indent baked in manually, since `Raw` prints verbatim
    // with none of its own — depth 2 for an invariant's own top-level
    // check, depth 3 for a transition's own check nested inside the
    // `if (__prior !== undefined)` block), the predicate wrapped in a real
    // `Unary`/`Paren` condition — `!({pred})`, `pred` itself still one
    // opaque `Ident` leaf (#1539: the same `Unary{Not, Paren(Ident(pred))}`
    // shape `contract_guard_if_stmt`, above, already builds for its own
    // opaque predicate; this site used to spell the whole thing as one
    // `TsExpr::Ident(format!("!({pred})"))` string instead of the equivalent
    // real node). Everything else — the `if`/`console.error`/`throw`
    // wrapper, the transition prologue — is real: nothing here needs the
    // general expression lowerer at all.
    let commit_state_body = {
        // Review of #1374: the hoisted-statement `Raw` indent is derived
        // from `class_depth` rather than hardcoded — a hoisting predicate
        // is untested by the fixture corpus today (every real `@invariant`/
        // `transition` fixture's own predicate hoists nothing), so an
        // un-derived literal here would have been silently wrong the day a
        // future caller printed this method at a non-zero depth, with
        // nothing but a fixture no one has written yet to catch it.
        let build_violation_check =
            |name: &str,
             declaration: &str,
             location: &str,
             pred: String,
             pre_stmts: Vec<bynk_ts::TsStmt>| {
                let mut stmts = pre_stmts;
                stmts.push(bynk_ts::TsStmt::if_stmt(
                    bynk_ts::TsExpr::Unary {
                        op: bynk_ts::TsUnaryOp::Not,
                        expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Ident(
                            pred,
                        )))),
                    },
                    bynk_ts::TsStmt::block(
                        vec![
                            bynk_ts::TsStmt::expr_stmt(
                                bynk_ts::TsExpr::Call {
                                    callee: Box::new(bynk_ts::TsExpr::Ident(
                                        "globalThis.console.error".to_string(),
                                    )),
                                    args: vec![
                                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(format!(
                                            "InvariantViolation {}.{name}",
                                            a.name.name
                                        ))),
                                        bynk_ts::TsExpr::object(vec![
                                            (
                                                "agent".to_string(),
                                                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                                    a.name.name.clone(),
                                                )),
                                            ),
                                            (
                                                "invariant".to_string(),
                                                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                                    name.to_string(),
                                                )),
                                            ),
                                        ]),
                                    ],
                                },
                                None,
                            ),
                            bynk_ts::TsStmt::throw_stmt(
                                bynk_ts::TsExpr::Call {
                                    callee: Box::new(bynk_ts::TsExpr::Ident(
                                        "__invariantViolation".to_string(),
                                    )),
                                    args: vec![
                                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                            a.name.name.clone(),
                                        )),
                                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(name.to_string())),
                                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                            declaration.to_string(),
                                        )),
                                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                            location.to_string(),
                                        )),
                                    ],
                                },
                                None,
                            ),
                        ],
                        None,
                    ),
                    None,
                ));
                stmts
            };
        let mut stmts: Vec<bynk_ts::TsStmt> = Vec::new();
        // v0.80 (§14): evaluate each invariant against the proposed state `s`
        // before the write. A violation throws `InvariantViolation` *before*
        // `storage.put`, so the offending commit never persists
        // (non-persistence of the offending commit — not whole-handler
        // rollback). The refusal is logged with the agent type and
        // invariant name (never the key — see ADR 0107) so it is
        // distinguishable from a crash in the logs.
        if !a.invariants.is_empty() {
            let field_names: HashSet<String> = effective_fields
                .iter()
                .map(|f| f.name.name.clone())
                .collect();
            for inv in &a.invariants {
                let mut cx = LowerCtx::new(
                    ModuleCtx::for_unit(commons, ctx),
                    BodyMode::Invariant {
                        name: "s".to_string(),
                        fields: field_names.clone(),
                    },
                );
                let mut pre = Pre::new();
                let pred = pre.lower(&inv.predicate, &mut cx);
                stmts.extend(build_violation_check(
                    &inv.name.name,
                    "invariant",
                    &declaration_location(ctx, source_text, inv.name.span),
                    pred,
                    pre.ts_stmts(&"  ".repeat(class_depth + 2)),
                ));
            }
        }
        // v0.116 (testing track slice 4): step invariants — evaluate each
        // `transition` against the pre-/post-commit state pair. The old
        // state is still in storage (this method performs the `put`), so
        // reading it here yields the pre-commit snapshot; `undefined` is the
        // genesis commit, which has no prior state to transition from and is
        // skipped (snapshot invariants above still apply). `old`/`new` are
        // lowered to `__old`/`__new` (`new` is a JS reserved word). A
        // violation throws the same `InvariantViolation`-family fault,
        // before the write.
        if !a.transitions.is_empty() {
            let mut transition_body = vec![
                bynk_ts::TsStmt::const_stmt(
                    bynk_ts::TsBindingName::Ident("__old".to_string()),
                    None,
                    bynk_ts::TsExpr::object_entries(vec![
                        bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Ident(zero_fn.clone())),
                            args: Vec::new(),
                        }),
                        bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(
                            "__prior".to_string(),
                        )),
                    ]),
                    None,
                ),
                bynk_ts::TsStmt::const_stmt(
                    bynk_ts::TsBindingName::Ident("__new".to_string()),
                    None,
                    bynk_ts::TsExpr::Ident("s".to_string()),
                    None,
                ),
            ];
            // #1649: the prior state is stored in the wire shape, so decode it
            // the way `loadState` does before a predicate reads `old`. Before
            // #1649 this read was raw and unvalidated.
            if has_rehydrate {
                transition_body.insert(
                    1,
                    bynk_ts::TsStmt::expr_stmt(
                        bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Ident(rehydrate_fn.clone())),
                            args: vec![
                                bynk_ts::TsExpr::Ident("__old".to_string()),
                                bynk_ts::TsExpr::Ident("__prior".to_string()),
                            ],
                        },
                        None,
                    ),
                );
            }
            for tr in &a.transitions {
                let mut cx = LowerCtx::new(
                    ModuleCtx::for_unit(commons, ctx),
                    BodyMode::Transition {
                        old: "__old".to_string(),
                        new: "__new".to_string(),
                    },
                );
                let mut pre = Pre::new();
                let pred = pre.lower(&tr.predicate, &mut cx);
                transition_body.extend(build_violation_check(
                    &tr.name.name,
                    "transition",
                    &declaration_location(ctx, source_text, tr.name.span),
                    pred,
                    pre.ts_stmts(&"  ".repeat(class_depth + 3)),
                ));
            }
            stmts.push(bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("__prior".to_string()),
                None,
                bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::VerbatimExpr(
                        format!("this.state.storage.get<{state_ty}>"),
                        bynk_ts::VerbatimOrigin::Emit,
                    )),
                    args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                        "state".to_string(),
                    ))],
                })),
                None,
            ));
            stmts.push(bynk_ts::TsStmt::if_stmt(
                bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::StrictNotEq,
                    left: Box::new(bynk_ts::TsExpr::Ident("__prior".to_string())),
                    right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                },
                bynk_ts::TsStmt::block(transition_body, None),
                None,
            ));
        }
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__commitStored".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Ident("this".to_string()),
                    bynk_ts::TsExpr::Ident("this.state.storage".to_string()),
                    // #1649: the wire shape, the same shape `loadState` decodes.
                    // State-migrations S1 (#2211, Decision D): with the schema
                    // lock, stamped with the class's version in the same `put`,
                    // spread on so the state type is untouched. The commit does
                    // not check the stored marker itself: it relies on every
                    // caller having run `loadState` first, which faults a newer
                    // record before any write. A future path that commits
                    // without loading would overwrite a newer record silently,
                    // so it must check `__v` too (#2213 review).
                    {
                        let payload = if has_encode {
                            bynk_ts::TsExpr::Call {
                                callee: Box::new(bynk_ts::TsExpr::Ident(encode_fn.clone())),
                                args: vec![bynk_ts::TsExpr::Ident("s".to_string())],
                            }
                        } else {
                            bynk_ts::TsExpr::Ident("s".to_string())
                        };
                        match &agent_versions {
                            Some(v) => bynk_ts::TsExpr::object_entries(vec![
                                bynk_ts::TsObjectEntry::Spread(payload),
                                bynk_ts::TsObjectEntry::prop(
                                    "__v".to_string(),
                                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(
                                        v.version.to_string(),
                                    )),
                                ),
                            ]),
                            None => payload,
                        }
                    },
                    bynk_ts::TsExpr::Ident(fields_const.clone()),
                    // Cache reaping (#3189): the state object the handler's `Cache` ops
                    // noted their clock against.
                    bynk_ts::TsExpr::Ident("s".to_string()),
                ],
            })),
            None,
        ));
        stmts
    };
    let (commit_state_body, commit_state_params) =
        commit_state_params(commit_state_body, &state_ty, ctx, fields_const);
    let commit_state_method = bynk_ts::TsClassMethod::new(
        "commitState",
        true,
        commit_state_params,
        Some(bynk_ts::TsType::named_with_args(
            "globalThis.Promise",
            vec![bynk_ts::TsType::named("void")],
        )),
        commit_state_body,
    )
    .private();
    methods.push(commit_state_method);
}

/// `commitState`'s parameters and body. #3581: a `bynk dev` build's `commitState` also records the transition. It takes the
/// handler's name as an optional trailing parameter, snapshots what is stored just
/// before the write (the last statement of `body`), and appends `{handler, old, new,
/// at}` to the instance's ring buffer just after it. A commit that throws before the
/// write (an invariant or `transition` refusal) never reaches the append. Returns
/// `body` unchanged, and just the state parameter, for every other build.
fn commit_state_params(
    mut body: Vec<bynk_ts::TsStmt>,
    state_ty: &str,
    ctx: &EmitProjectCtx,
    fields_const: &str,
) -> (Vec<bynk_ts::TsStmt>, Vec<bynk_ts::TsParam>) {
    let mut params = vec![bynk_ts::TsParam::new(
        "s".to_string(),
        Some(bynk_ts::TsType::named(state_ty.to_string())),
    )];
    if !ctx.inspect_agents {
        return (body, params);
    }
    let call = |name: &str, extra: &[&str]| {
        let mut args = vec![
            bynk_ts::TsExpr::Ident("this.state.storage".to_string()),
            bynk_ts::TsExpr::Ident(fields_const.to_string()),
        ];
        args.extend(extra.iter().map(|e| bynk_ts::TsExpr::Ident(e.to_string())));
        bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Ident(name.to_string())),
            args,
        }))
    };
    let before = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__before".to_string()),
        None,
        call("__historyBefore", &[]),
        None,
    );
    let after =
        bynk_ts::TsStmt::expr_stmt(call("__historyRecord", &["__before", "__handler"]), None);
    body.insert(body.len().saturating_sub(1), before);
    body.push(after);
    params.push(bynk_ts::TsParam::optional(
        "__handler".to_string(),
        Some(bynk_ts::TsType::named("string")),
    ));
    (body, params)
}

/// `globalThis.Record<string, unknown>`: the wire shape a migration step reads and returns.
fn wire_record_ty() -> bynk_ts::TsType {
    bynk_ts::TsType::named_with_args(
        "globalThis.Record",
        vec![
            bynk_ts::TsType::named("string"),
            bynk_ts::TsType::named("unknown"),
        ],
    )
}

/// `if (__v >= <lo> && __v <= <from>) return <step_fn>(stored);`: one arm of an agent's
/// migration dispatcher.
fn migrate_step_dispatch(lo: i64, from: i64, step_fn: &str) -> bynk_ts::TsStmt {
    let ident = |name: &str| bynk_ts::TsExpr::Ident(name.to_string());
    let bound = |op: bynk_ts::TsBinaryOp, n: i64| bynk_ts::TsExpr::Binary {
        op,
        left: Box::new(ident("__v")),
        right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(n.to_string()))),
    };
    bynk_ts::TsStmt::if_stmt(
        bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::And,
            left: Box::new(bound(bynk_ts::TsBinaryOp::GreaterThanEq, lo)),
            right: Box::new(bound(bynk_ts::TsBinaryOp::LessThanEq, from)),
        },
        bynk_ts::TsStmt::return_stmt(
            Some(bynk_ts::TsExpr::Call {
                callee: Box::new(ident(step_fn)),
                args: vec![ident("stored")],
            }),
            None,
        ),
        None,
    )
}

/// `function <migrate_fn>(stored): Record<string, unknown>`: reads the stored version (1 when
/// absent), runs the nearest step covering it, else returns the record as it is.
fn migrate_dispatcher_decl(migrate_fn: &str, dispatch: Vec<bynk_ts::TsStmt>) -> bynk_ts::TsStmt {
    let stored = || bynk_ts::TsExpr::Ident("stored".to_string());
    let version = || bynk_ts::TsExpr::Index {
        object: Box::new(stored()),
        index: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str("__v".to_string()))),
    };
    let read_version = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__v".to_string()),
        None,
        bynk_ts::TsExpr::Conditional {
            test: Box::new(bynk_ts::TsExpr::Binary {
                op: bynk_ts::TsBinaryOp::StrictEq,
                left: Box::new(version()),
                right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
            }),
            consequent: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("1".to_string()))),
            alternate: Box::new(bynk_ts::TsExpr::As {
                expr: Box::new(version()),
                ty: bynk_ts::TsType::named("number"),
            }),
        },
        None,
    );
    let mut body = vec![read_version];
    body.extend(dispatch);
    body.push(bynk_ts::TsStmt::return_stmt(Some(stored()), None));
    bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Function {
            name: migrate_fn.to_string(),
            generics: Vec::new(),
            params: vec![bynk_ts::TsParam::new(
                "stored".to_string(),
                Some(wire_record_ty()),
            )],
            return_type: Some(wire_record_ty()),
            body,
            is_async: false,
            inline: false,
        },
        None,
    )
}

/// What `loadState` does with the merged state's collections: fill the in-memory handles from the
/// record, or, for a lazily read record (#3189), attach storage-backed ones in their place.
fn hydrate_or_attach_stmt(
    hydrate_fn: &str,
    src: &str,
    lazy_fields: Option<&String>,
) -> bynk_ts::TsStmt {
    let hydrate = bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Ident(hydrate_fn.to_string())),
        args: vec![
            bynk_ts::TsExpr::Ident("__merged".to_string()),
            bynk_ts::TsExpr::Ident(src.to_string()),
        ],
    };
    let Some(spec) = lazy_fields else {
        return bynk_ts::TsStmt::expr_stmt(hydrate, None);
    };
    let ident = |name: &str| bynk_ts::TsExpr::Ident(name.to_string());
    let attach = bynk_ts::TsExpr::Call {
        callee: Box::new(ident("__attachLazy")),
        args: vec![ident("this.state.storage"), ident("__merged"), ident(spec)],
    };
    let reads_lazily = bynk_ts::TsExpr::Call {
        callee: Box::new(ident("__readsLazily")),
        args: vec![ident("this")],
    };
    bynk_ts::TsStmt::if_else_same_line_stmt(
        reads_lazily,
        bynk_ts::TsStmt::block(vec![bynk_ts::TsStmt::expr_stmt(attach, None)], None),
        bynk_ts::TsStmt::block(vec![bynk_ts::TsStmt::expr_stmt(hydrate, None)], None),
        None,
    )
}

/// What the rest of an agent's emission refers to by name once its state's
/// validation and migration functions are declared (#3167).
struct AgentPersistFns {
    /// The rehydration check `loadState` runs on a stored record.
    rehydrate_fn: String,
    /// The wire-format encoder `commitState` writes through.
    encode_fn: String,
    /// The migration dispatcher `loadState` calls for an older stored layout.
    migrate_fn: String,
    /// Fills a loaded state's collection handles from the stored record (#3254).
    hydrate_fn: String,
    /// Whether the agent's state needs the rehydration check at all.
    has_rehydrate: bool,
    /// Whether the agent has a `Map`, `Set`, `Cache`, `Log` or held `Map` field.
    has_collections: bool,
    /// Whether `commitState` writes through the encoder: a value needs the codec, or a
    /// collection handle must be put in wire form.
    has_encode: bool,
    /// The migration steps from each older schema version, in order.
    step_plans: Vec<bynk_check::schema_registry::MigrationStep>,
    /// The agent's declared schema versions, if it has any.
    agent_versions: Option<bynk_check::schema_registry::AgentVersions>,
    /// The lazy-read field spec's name, when the agent reads its collections lazily (#3189).
    lazy_fields: Option<String>,
}

/// `globalThis.<path>`, a dotted path such as `Object.keys`, as member accesses.
fn persist_global(path: &str) -> bynk_ts::TsExpr {
    path.split('.')
        .fold(hist_id("globalThis"), |object, property| {
            hist_member(object, property)
        })
}

/// `expr as ty`.
fn persist_cast(expr: bynk_ts::TsExpr, ty: bynk_ts::TsType) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::As {
        expr: Box::new(expr),
        ty,
    }
}

/// `expr as unknown as ty`: the double cast that reinterprets a value at another type.
fn persist_recast(expr: bynk_ts::TsExpr, ty: bynk_ts::TsType) -> bynk_ts::TsExpr {
    persist_cast(persist_cast(expr, bynk_ts::TsType::named("unknown")), ty)
}

/// `globalThis.Record<string, unknown>`.
fn persist_record_ty() -> bynk_ts::TsType {
    bynk_ts::TsType::named_with_args(
        "globalThis.Record",
        vec![
            bynk_ts::TsType::named("string"),
            bynk_ts::TsType::named("unknown"),
        ],
    )
}

/// `{ v: unknown }`: the shape of a `Cache` or `Log` entry's value cell.
fn persist_entry_ty() -> bynk_ts::TsType {
    bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::prop(
        "v",
        bynk_ts::TsType::named("unknown"),
    )])
}

/// `{ -readonly [K in keyof source]: source[K] }`: `source` with its `readonly`
/// modifiers removed, so a decode can write through it.
fn persist_writable(source: bynk_ts::TsType) -> bynk_ts::TsType {
    bynk_ts::TsType::mapped(
        "K",
        source.clone(),
        bynk_ts::TsType::indexed(source, bynk_ts::TsType::named("K")),
    )
    .with_mapped_readonly(bynk_ts::TsMappedModifier::Remove)
}

/// `typeof s`.
fn persist_typeof_s() -> bynk_ts::TsType {
    bynk_ts::TsType::type_of(hist_id("s"))
}

/// `(s as unknown as globalThis.Record<string, unknown>)`: the loaded record seen as
/// what it is at runtime, an untyped record. The collection fields are held as
/// handles (#3254), but in the loaded record they are still the stored records and
/// arrays.
fn persist_record_view() -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Paren(Box::new(persist_recast(hist_id("s"), persist_record_ty())))
}

/// `(<view>.<field> as ty)`: a collection field of the record view, cast.
fn persist_field_as(field: &str, ty: bynk_ts::TsType) -> bynk_ts::TsExpr {
    persist_cast(hist_member(persist_record_view(), field), ty)
}

/// `globalThis.Object.prototype.hasOwnProperty.call(stored, "name")`: whether the stored
/// record carries the field.
fn persist_present(name: &str) -> bynk_ts::TsExpr {
    hist_call(
        persist_global("Object.prototype.hasOwnProperty.call"),
        vec![hist_id("stored"), hist_str(name)],
    )
}

/// `<value> as unknown as __JsonValue`, in parentheses, printed as the text the
/// boundary deserialiser takes for its input.
fn persist_json_input(value: bynk_ts::TsExpr) -> String {
    bynk_ts::print_expr(&bynk_ts::TsExpr::Paren(Box::new(persist_recast(
        value,
        bynk_ts::TsType::named("__JsonValue"),
    ))))
}

/// The decode's first two statements: `const __r = <decoded>;` and the throw on an `Err`
/// that disposes of a failure as an internal `RehydrationViolation` fault.
fn persist_decode(decoded: bynk_ts::TsExpr, agent_name: &str) -> Vec<bynk_ts::TsStmt> {
    let result = || hist_id("__r");
    vec![
        hist_const("__r", decoded),
        bynk_ts::TsStmt::if_stmt(
            hist_binary(
                bynk_ts::TsBinaryOp::StrictEq,
                hist_member(result(), "tag"),
                hist_str("Err"),
            ),
            bynk_ts::TsStmt::throw_stmt(
                hist_call(
                    hist_id("__rehydrationViolation"),
                    vec![hist_str(agent_name), hist_member(result(), "error")],
                ),
                None,
            ),
            None,
        ),
    ]
}

/// `if (<present>) { <then> }`, the guard every rehydration check sits behind.
fn persist_when_present(name: &str, then: Vec<bynk_ts::TsStmt>) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::if_stmt(
        persist_present(name),
        bynk_ts::TsStmt::block(then, None),
        None,
    )
}

/// `for (const <binding> of <iter>) { <body> }`.
fn persist_for_each(
    binding: &str,
    iter: bynk_ts::TsExpr,
    body: Vec<bynk_ts::TsStmt>,
) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::for_of(binding, iter, bynk_ts::TsStmt::block(body, None), None)
}

/// The function `loadState` calls to fill a record's collections into the zero state's handles
/// (#3254): one fill per collection field, from the stored record.
fn hydrate_fn_decl(
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    hydrate_fn: &str,
    zero_fn: &str,
) -> bynk_ts::TsStmt {
    let state_ty = format!("{}State", a.name.name);
    let held_names_all: HashSet<&str> = layout
        .all_held_maps
        .iter()
        .map(|(n, _)| n.name.as_str())
        .collect();
    let record_at = |key: &str| {
        persist_cast(
            bynk_ts::TsExpr::Index {
                object: Box::new(hist_id("__r")),
                index: Box::new(hist_str(key)),
            },
            bynk_ts::TsType::named("never"),
        )
    };
    let mut body = vec![
        hist_const("__z", hist_call(hist_id(zero_fn), Vec::new())),
        hist_const(
            "__w",
            persist_cast(
                hist_id("s"),
                persist_writable(bynk_ts::TsType::named(state_ty.clone())),
            ),
        ),
        hist_const(
            "__r",
            persist_recast(hist_id("stored"), persist_record_ty()),
        ),
    ];
    let mut fill = |name: &str, with: &str| {
        let mut args = vec![hist_member(hist_id("__z"), name), record_at(name)];
        // A held map's insertion order is stored beside its record, as `<field>:order`.
        if with == "__fillHeld" {
            args.push(record_at(&format!("{name}:order")));
        }
        body.push(bynk_ts::TsStmt::if_stmt(
            hist_call(
                persist_global("Object.hasOwn"),
                vec![hist_id("__r"), hist_str(name)],
            ),
            bynk_ts::TsStmt::block(
                vec![
                    bynk_ts::TsStmt::expr_stmt(hist_call(hist_id(with), args), None),
                    bynk_ts::TsStmt::assign(
                        hist_member(hist_id("__w"), name),
                        hist_member(hist_id("__z"), name),
                        None,
                    ),
                ],
                None,
            ),
            None,
        ));
    };
    for (name, _) in &layout.store_map_fields {
        let held = held_names_all.contains(name.name.as_str());
        fill(&name.name, if held { "__fillHeld" } else { "__fillMap" });
    }
    for (name, _) in &layout.held_maps {
        fill(&name.name, "__fillHeld");
    }
    for (name, _) in &layout.store_set_fields {
        fill(&name.name, "__fillSet");
    }
    for (name, _, _) in &layout.store_cache_fields {
        fill(&name.name, "__fillMap");
    }
    for (name, _, _) in &layout.store_log_fields {
        fill(&name.name, "__fillLog");
    }
    bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Function {
            name: hydrate_fn.to_string(),
            generics: Vec::new(),
            params: vec![
                bynk_ts::TsParam::new(
                    "s".to_string(),
                    Some(bynk_ts::TsType::named(state_ty.clone())),
                ),
                bynk_ts::TsParam::new("stored".to_string(), Some(bynk_ts::TsType::named(state_ty))),
            ],
            return_type: Some(bynk_ts::TsType::named("void")),
            body,
            is_async: false,
            inline: false,
        },
        None,
    )
}

/// The rehydration checks `loadState` runs on a stored record, and the entries the
/// wire-format encoder writes beside `...s` (#1649, ADR 0124).
fn agent_state_checks_and_entries(
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> (Vec<bynk_ts::TsStmt>, Vec<bynk_ts::TsObjectEntry>) {
    let AgentStoreLayout {
        effective_fields,
        held_maps,
        store_map_fields,
        store_map_indexes,
        store_set_fields,
        store_cache_fields,
        store_log_fields,
        ..
    } = layout;
    let mut encode_entries: Vec<bynk_ts::TsObjectEntry> = Vec::new();
    let agent_name = &a.name.name;
    let mut rehydrate_checks: Vec<bynk_ts::TsStmt> = Vec::new();
    // The loaded record is statically typed (its fields are the agent's types),
    // but at runtime its bytes are untrusted, so each value is laundered to
    // `JsonValue` before the boundary deserialiser decodes it.
    let ser =
        |ty: &TypeRef, value: &str| serialisation::serialise_text(ty, value, &ctx.runtime_use);
    // 1A: `deserialise_expr` still takes its input as text (printed from a node by
    // `persist_json_input`); the input moves onto `TsExpr` in 1A.38 (#3745).
    let deser = |ty: &TypeRef, json: String, path: &str| {
        serialisation::deserialise_expr(ty, &json, path, &ctx.runtime_use)
    };
    // `Cell[T]` — decode the field value as `T`.
    for f in effective_fields {
        let path = f.name.name.as_str();
        // Only codec-able types have a deserialiser; a non-storable type never
        // reaches a `store` position (the checker rejects it), but guard anyway.
        if !is_codecable(&f.type_ref) {
            continue;
        }
        let d = deser(
            &f.type_ref,
            persist_json_input(hist_member(hist_id("s"), path)),
            path,
        );
        // The state interface's fields are `readonly`; the decode writes
        // through a mutable view of the freshly merged record. The cast to
        // the field's own type matters on the workers target, where the
        // state interface re-declares a commons type locally and
        // TypeScript's brands make the two nominally distinct, though
        // they share one runtime shape.
        let mut block = persist_decode(d, agent_name);
        block.push(bynk_ts::TsStmt::assign(
            hist_member(
                bynk_ts::TsExpr::Paren(Box::new(persist_cast(
                    hist_id("s"),
                    persist_writable(persist_typeof_s()),
                ))),
                path,
            ),
            persist_recast(
                hist_member(hist_id("__r"), "value"),
                bynk_ts::TsType::indexed(persist_typeof_s(), bynk_ts::TsType::string_literal(path)),
            ),
            None,
        ));
        rehydrate_checks.push(persist_when_present(path, block));
        encode_entries.push(bynk_ts::TsObjectEntry::prop(
            path,
            ser(&f.type_ref, &format!("s.{path}")),
        ));
    }
    let held_decls_all = &layout.all_held_maps;
    let held_names_all: HashSet<&str> = held_decls_all
        .iter()
        .map(|(n, _)| n.name.as_str())
        .collect();
    // A textual `K` key persists as a `String(k)` Record key, so each entry's key is
    // validated: `for (const [__k] of __wireEntries(<view>.n as never)) { <decode> }`.
    let key_check = |n: &str, k: &TypeRef| {
        let d = deser(k, persist_json_input(hist_id("__k")), n);
        persist_when_present(
            n,
            vec![persist_for_each(
                "[__k]",
                hist_call(
                    hist_id("__wireEntries"),
                    vec![persist_field_as(n, bynk_ts::TsType::named("never"))],
                ),
                persist_decode(d, agent_name),
            )],
        )
    };
    // `__wire<Kind>(s.n, ...)`.
    let wire = |kind: &str, n: &str, extra: Option<bynk_ts::TsExpr>| {
        let mut args = vec![hist_member(hist_id("s"), n)];
        args.extend(extra);
        bynk_ts::TsObjectEntry::prop(n, hist_call(hist_id(kind), args))
    };
    // `(<param>) => <body>`.
    let arrow = |param: &str, body: bynk_ts::TsExpr| bynk_ts::TsExpr::Arrow {
        params: vec![bynk_ts::TsParam::new(param.to_string(), None)],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(body))),
    };
    // `Map[K, V]` — validate each entry value against `V`, and each key against
    // `K` when `K` is textual (the key persists as a `String(k)` Record key).
    for (name, v) in store_map_fields {
        let n = &name.name;
        let held = held_names_all.contains(n.as_str());
        if is_codecable(v) {
            let entry = bynk_ts::TsExpr::Index {
                object: Box::new(bynk_ts::TsExpr::Paren(Box::new(persist_field_as(
                    n,
                    persist_record_ty(),
                )))),
                index: Box::new(hist_id("__k")),
            };
            let d = deser(v, persist_json_input(entry.clone()), n);
            let mut body = persist_decode(d, agent_name);
            body.push(bynk_ts::TsStmt::assign(
                entry,
                hist_member(hist_id("__r"), "value"),
                None,
            ));
            rehydrate_checks.push(persist_when_present(
                n,
                vec![persist_for_each(
                    "__k",
                    hist_call(
                        persist_global("Object.keys"),
                        vec![persist_field_as(n, bynk_ts::TsType::named("object"))],
                    ),
                    body,
                )],
            ));
        }
        if let Some(k) = store_field_key_type(a, n)
            && type_base_is_string(k, &commons.types)
        {
            rehydrate_checks.push(key_check(n, k));
        }
        if held {
            encode_entries.push(wire("__wireHeld", n, None));
            encode_entries.push(bynk_ts::TsObjectEntry::prop(
                format!("\"{n}:order\""),
                hist_call(
                    hist_id("__wireHeldOrder"),
                    vec![hist_member(hist_id("s"), n)],
                ),
            ));
        } else if is_codecable(v) {
            encode_entries.push(wire("__wireMap", n, Some(arrow("__v", ser(v, "__v")))));
        } else {
            encode_entries.push(wire("__wireMap", n, None));
        }
        // An `@indexed` map's posting lists ride in the wire record beside it (v0.93).
        let mut index_fields: Vec<&String> = store_map_indexes
            .get(n)
            .map(|fs| fs.iter().collect())
            .unwrap_or_default();
        index_fields.sort();
        for f in index_fields {
            encode_entries.push(bynk_ts::TsObjectEntry::prop(
                format!("{n}__idx_{f}"),
                hist_call(
                    hist_id("__wirePostings"),
                    vec![hist_member(hist_id("s"), n), hist_str(f)],
                ),
            ));
        }
    }
    // v0.105 (slice 3b-ii): a held `Map[K, Connection]` persists `K → connId`. The
    // connId is an opaque platform string (no value check); validate each textual
    // `K` key, as for a plain map.
    for (name, _) in held_maps {
        let n = &name.name;
        if let Some(k) = store_field_key_type(a, n)
            && type_base_is_string(k, &commons.types)
        {
            rehydrate_checks.push(key_check(n, k));
        }
        encode_entries.push(wire("__wireHeld", n, None));
        encode_entries.push(bynk_ts::TsObjectEntry::prop(
            format!("\"{n}:order\""),
            hist_call(
                hist_id("__wireHeldOrder"),
                vec![hist_member(hist_id("s"), n)],
            ),
        ));
    }
    // `Set[T]` — the elements are the (textual) Record keys; validate when `T`
    // is textual, else defer (structural string key).
    for (name, t) in store_set_fields {
        let n = &name.name;
        if type_base_is_string(t, &commons.types) {
            rehydrate_checks.push(key_check(n, t));
        }
        encode_entries.push(wire("__wireSet", n, None));
    }
    // `Cache[K, V]` — validate each entry's `.v` against `V`.
    for (name, v, _) in store_cache_fields {
        let n = &name.name;
        if is_codecable(v) {
            let entry_value = || hist_member(hist_id("__e"), "v");
            let d = deser(v, persist_json_input(entry_value()), n);
            let mut body = persist_decode(d, agent_name);
            body.push(bynk_ts::TsStmt::assign(
                entry_value(),
                hist_member(hist_id("__r"), "value"),
                None,
            ));
            let cells = persist_field_as(
                n,
                bynk_ts::TsType::named_with_args(
                    "globalThis.Record",
                    vec![bynk_ts::TsType::named("string"), persist_entry_ty()],
                ),
            );
            rehydrate_checks.push(persist_when_present(
                n,
                vec![persist_for_each(
                    "__e",
                    hist_call(persist_global("Object.values"), vec![cells]),
                    body,
                )],
            ));
            encode_entries.push(wire(
                "__wireMap",
                n,
                Some(arrow(
                    "__e",
                    bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::object_entries(vec![
                        bynk_ts::TsObjectEntry::Spread(hist_id("__e")),
                        bynk_ts::TsObjectEntry::prop("v", ser(v, "__e.v")),
                    ]))),
                )),
            ));
        } else {
            encode_entries.push(wire("__wireMap", n, None));
        }
    }
    // `Log[T]` — validate each entry's `.v` against `T`.
    for (name, t, _) in store_log_fields {
        let n = &name.name;
        if is_codecable(t) {
            let entry_value = || hist_member(hist_id("__e"), "v");
            let d = deser(t, persist_json_input(entry_value()), n);
            let mut body = persist_decode(d, agent_name);
            body.push(bynk_ts::TsStmt::assign(
                entry_value(),
                hist_member(hist_id("__r"), "value"),
                None,
            ));
            let cells = bynk_ts::TsExpr::Paren(Box::new(persist_field_as(
                n,
                bynk_ts::TsType::named_with_args("ReadonlyArray", vec![persist_entry_ty()]),
            )));
            rehydrate_checks.push(persist_when_present(
                n,
                vec![persist_for_each("__e", cells, body)],
            ));
            encode_entries.push(wire("__wireLog", n, Some(arrow("__v", ser(t, "__v")))));
        } else {
            encode_entries.push(wire("__wireLog", n, None));
        }
    }
    (rehydrate_checks, encode_entries)
}

/// The functions that guard an agent's stored state across a deploy (#3167): the
/// rehydration check `loadState` runs, the wire-format encoder, and the
/// migration step chain for a stored layout from an older schema version.
/// Pushes those declarations onto `stmts`; returns their names and the facts
/// `loadState` needs.
fn emit_agent_state_persistence(
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    zero_fn: &str,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    stmts: &mut Vec<bynk_ts::TsStmt>,
) -> AgentPersistFns {
    let state_ty = format!("{}State", a.name.name);
    let AgentStoreLayout {
        effective_fields,
        held_maps,
        store_map_fields,
        store_set_fields,
        store_cache_fields,
        store_log_fields,
        ..
    } = layout;
    // v0.96 (ADR 0124): the rehydration validation gate. `loadState` validates a
    // *loaded* (merged) state against the current type definition before any
    // handler reads it — the load-time twin of the commit-time invariant gate.
    // Each value position (a `Cell`'s `T`, a `Map`/`Cache`'s `V`, a `Log`'s `T`,
    // and a textual `Set` element / `Map` key) is run through the same boundary
    // deserialiser the HTTP/queue seams use; a failure is disposed of as an
    // internal `RehydrationViolation` fault (Q6), never a caller-facing 400.
    // (Non-textual `Map`/`Set` keys persist as structural string keys — refined-
    // key rehydration validation is a named follow-on, ADR 0124 D5.)
    let rehydrate_fn = format!("__rehydrate{}State", a.name.name);
    // #1649 (runtime-semantics track §3.2): agent state is stored in the **wire
    // shape**. `commitState` writes `__encode<Agent>State(s)`, which runs each
    // codec-able value through the boundary serialiser (`kind`-tagged sums,
    // base64 `Bytes`, entries-array value `Map`s). `loadState` merges
    // zero-then-stored and calls `__rehydrate<Agent>State(merged, stored)`,
    // which now *decodes* each value present in `stored` back into the in-memory
    // shape, instead of validating it and discarding the result. Before #1649,
    // `commitState` stored the in-memory shape while the gate validated against
    // the wire shape, so every reload of an enum/`Option`/`Result`/`Bytes`/
    // value-`Map` state faulted (`RehydrationViolation … StructuralMismatch`).
    //
    // A field absent from `stored` keeps its in-memory zero (ADR 0124 D4), and is
    // not decoded. An unknown stored key (a renamed or removed field) rides along
    // untouched and is written back, so #539's migrations still find it.
    let encode_fn = format!("__encode{}State", a.name.name);
    let (rehydrate_checks, encode_entries) =
        agent_state_checks_and_entries(a, layout, commons, ctx);
    // Whether the agent has any collection: its handles are encoded on every commit and
    // filled on every load, whether or not its values need the codec.
    let has_collections = !store_map_fields.is_empty()
        || !held_maps.is_empty()
        || !store_set_fields.is_empty()
        || !store_cache_fields.is_empty()
        || !store_log_fields.is_empty();
    let has_rehydrate = agent_needs_rehydrate(a, layout.state, &commons.types);
    let has_encode = has_rehydrate || has_collections;
    // State-migrations S1 (#2211): this class's lock version facts, present
    // only when the build has the schema lock (Decision A).
    let agent_versions = ctx.agent_schema_versions.get(&a.name.name).cloned();
    if has_rehydrate {
        let rehydrate_fn_decl = bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Function {
                name: rehydrate_fn.clone(),
                generics: Vec::new(),
                params: vec![
                    bynk_ts::TsParam::new(
                        "s".to_string(),
                        Some(bynk_ts::TsType::named(state_ty.clone())),
                    ),
                    // #1649: the stored record, so a field absent from it keeps
                    // its in-memory zero instead of being decoded (ADR 0124 D4).
                    bynk_ts::TsParam::new(
                        "stored".to_string(),
                        Some(bynk_ts::TsType::named(state_ty.clone())),
                    ),
                ],
                return_type: Some(bynk_ts::TsType::named("void")),
                body: rehydrate_checks,
                is_async: false,
                inline: false,
            },
            None,
        );
        // #1486: no more explicit trailing `TsStmt::blank(None)` — see
        // `emit_refined_type`'s own identical note.
        stmts.push(rehydrate_fn_decl);
    }
    // Per-collection handles (#3254): `loadState` fills each collection the stored record
    // carries into its handle (the zero state's, so a `Map` keeps its `@indexed` indexers),
    // in the order the record notes. A collection the record lacks stays the empty zero.
    let hydrate_fn = format!("__hydrate{}State", a.name.name);
    if has_collections {
        stmts.push(hydrate_fn_decl(a, layout, &hydrate_fn, zero_fn));
    }
    if has_encode {
        // #1649: the wire-shape encoder `commitState` writes through. Every
        // codec-able value position is serialised; the rest of the record (held
        // maps, `Set`s, `@indexed` posting lists, unknown stored keys) is
        // already wire-shaped and spreads through unchanged.
        let mut entries = vec![bynk_ts::TsObjectEntry::Spread(hist_id("s"))];
        entries.extend(encode_entries);
        stmts.push(bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Function {
                name: encode_fn.clone(),
                generics: Vec::new(),
                params: vec![bynk_ts::TsParam::new(
                    "s".to_string(),
                    Some(bynk_ts::TsType::named(state_ty.clone())),
                )],
                return_type: Some(persist_record_ty()),
                body: vec![bynk_ts::TsStmt::return_stmt(
                    Some(bynk_ts::TsExpr::object_entries(entries)),
                    None,
                )],
                is_async: false,
                inline: false,
            },
            None,
        ));
    }
    // State-migrations S2 (#2224, track §3.3, Decision D): one function per
    // valid `migrate from N` step, carrying a stored (wire-shaped) record from
    // version N to the current shape, and a dispatcher `loadState` calls after
    // the `__v` check. A step's output is wire-shaped again, so `loadState`'s
    // zero-merge and the ADR 0124 gate then validate it like any record.
    let migrate_fn = format!("__migrate{}State", a.name.name);
    let step_plans: Vec<bynk_check::schema_registry::MigrationStep> = agent_versions
        .as_ref()
        .map(|v| v.steps.clone())
        .unwrap_or_default();
    if !step_plans.is_empty() {
        let agent_name = &a.name.name;
        let present = |name: &str| {
            format!("globalThis.Object.prototype.hasOwnProperty.call(stored, \"{name}\")")
        };
        let ser = |ty: &TypeRef, value: &str| {
            bynk_ts::print_expr(&serialisation::serialise_text(ty, value, &ctx.runtime_use))
        };
        let cell_names: HashSet<String> = effective_fields
            .iter()
            .map(|f| f.name.name.clone())
            .collect();
        let map_names: HashSet<String> = store_map_fields
            .iter()
            .map(|(n, _)| n.name.clone())
            .collect();
        let quote_list = |names: &[String]| {
            names
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut dispatch: Vec<bynk_ts::TsStmt> = Vec::new();
        for plan in &step_plans {
            let Some(step) = a.migrations.iter().find(|m| m.from == plan.from) else {
                continue;
            };
            let step_fn = format!("__migrate{agent_name}From{}", plan.from);
            // Names a step binds (`old`, its `let`s) shadow carried fields.
            let mut shadowed: HashSet<String> =
                step.old.iter().map(|o| o.name.name.clone()).collect();
            for item in &step.items {
                if let bynk_syntax::ast::MigrateItem::Let(l) = item {
                    shadowed.insert(l.name.name.clone());
                }
            }
            let mut cx = LowerCtx::new(
                ModuleCtx::for_unit(commons, ctx),
                BodyMode::Invariant {
                    name: "__c".to_string(),
                    fields: cell_names.difference(&shadowed).cloned().collect(),
                },
            );
            let mut body = String::new();
            // 1. Decode the old fields through the old type's own codec. A
            //    field the stored record lacks (an additive predecessor,
            //    Decision C) takes the old type's declared default — the
            //    record decoder applies it — or else its type's zero.
            if let Some(old) = &step.old
                && let TypeRef::Named(id) = &old.type_ref
                && let Some(decl) = commons.types.get(&id.name)
                && let TypeBody::Record(r) = &decl.body
            {
                let mut entries = Vec::new();
                for f in &r.fields {
                    let n = &f.name.name;
                    let raw = match &f.type_ref {
                        // A store `Map` is a `Record<string, V>`; a `Map`
                        // value's wire form is an entries array. A `Cell`
                        // holding a map is already in that wire form.
                        TypeRef::Map(k, _, _) if plan.store_maps.contains(n) => format!(
                            "globalThis.Object.entries(stored[\"{n}\"] as globalThis.Record<string, unknown>).map(([__k, __v]) => [{key}, __v])",
                            key = if type_base_is_string(k, &commons.types) {
                                "__k"
                            } else {
                                "globalThis.Number(__k)"
                            }
                        ),
                        _ => format!("stored[\"{n}\"]"),
                    };
                    if f.init.is_some() {
                        entries.push(format!(
                            "...({p} ? {{ {n}: {raw} }} : {{}})",
                            p = present(n)
                        ));
                    } else {
                        match zero_value_ts(&f.type_ref, f.refinement.as_ref(), &commons.types) {
                            Some(z) => entries.push(format!(
                                "{n}: {p} ? {raw} : {zw}",
                                p = present(n),
                                zw = ser(&f.type_ref, &z)
                            )),
                            None => entries.push(format!("{n}: {raw}")),
                        }
                    }
                }
                let d = bynk_ts::print_expr(&serialisation::deserialise_expr(
                    &old.type_ref,
                    "(__ow as never)",
                    &old.name.name,
                    &ctx.runtime_use,
                ));
                let old_js = cx.bind_local_name(&old.name.name);
                writeln!(body, "  const __ow = {{ {} }};", entries.join(", ")).unwrap();
                writeln!(
                    body,
                    "  const __od = {d}; if (__od.tag === \"Err\") throw __rehydrationViolation(\"{agent_name}\", __od.error);"
                )
                .unwrap();
                writeln!(body, "  const {old_js} = __od.value;").unwrap();
            }
            // 2. The carried state: version N's fields the current shape keeps
            //    unchanged, decoded as today; every other field at its zero.
            writeln!(
                body,
                "  const __picked: globalThis.Record<string, unknown> = {{}}; for (const __k of [{}]) if (globalThis.Object.prototype.hasOwnProperty.call(stored, __k)) __picked[__k] = stored[__k];",
                quote_list(&plan.carried)
            )
            .unwrap();
            writeln!(
                body,
                "  const __c = {{ ...{zero_fn}(), ...__picked }} as {state_ty};"
            )
            .unwrap();
            if has_rehydrate {
                writeln!(
                    body,
                    "  {rehydrate_fn}(__c, __picked as unknown as {state_ty});"
                )
                .unwrap();
            }
            if has_collections {
                writeln!(
                    body,
                    "  {hydrate_fn}(__c, __picked as unknown as {state_ty});"
                )
                .unwrap();
            }
            // 3. The body, with record-literal semantics: every assignment
            //    reads the carried state, never another assignment. A throw
            //    is `MigrationFailed`, never a bare 500 (Decision D).
            writeln!(body, "  let __out: {state_ty};").unwrap();
            writeln!(body, "  try {{").unwrap();
            let mut assigns = Vec::new();
            for item in &step.items {
                match item {
                    bynk_syntax::ast::MigrateItem::Let(l) => {
                        let mut pre = Pre::new();
                        let v = pre.lower(&l.value, &mut cx);
                        for s in &pre.stmts() {
                            writeln!(body, "    {s}").unwrap();
                        }
                        let js = cx.bind_local_name(&l.name.name);
                        writeln!(body, "    const {js} = {v};").unwrap();
                    }
                    bynk_syntax::ast::MigrateItem::Assign(asg) => {
                        let mut pre = Pre::new();
                        let v = pre.lower(&asg.value, &mut cx);
                        for s in &pre.stmts() {
                            writeln!(body, "    {s}").unwrap();
                        }
                        let n = &asg.target.name;
                        let v = if map_names.contains(n) {
                            // A `Map` value is a `ReadonlyMap`; a store `Map` is a
                            // handle keyed by `String(k)`, built fresh from the zero
                            // state so it carries its `@indexed` indexers.
                            format!(
                                "((__h) => {{ for (const [__k, __v] of {v}) __h.put(globalThis.String(__k), __v); return __h; }})({zero_fn}().{n})"
                            )
                        } else {
                            v
                        };
                        assigns.push(format!("{n}: {v}"));
                    }
                    bynk_syntax::ast::MigrateItem::Drop(_) => {}
                }
            }
            let fields_out = if assigns.is_empty() {
                String::new()
            } else {
                format!(", {}", assigns.join(", "))
            };
            writeln!(body, "    __out = {{ ...__c{fields_out} }} as {state_ty};").unwrap();
            writeln!(
                body,
                "  }} catch {{ throw __migrationFailed(\"{agent_name}\", {}); }}",
                plan.from
            )
            .unwrap();
            // 4. Back to the wire, keeping any key version N did not know.
            let enc = if has_encode {
                format!("{encode_fn}(__out)")
            } else {
                "(__out as unknown as globalThis.Record<string, unknown>)".to_string()
            };
            writeln!(
                body,
                "  const __known = new globalThis.Set<string>([{}]); const __keep: globalThis.Record<string, unknown> = {{}}; for (const __k of globalThis.Object.keys(stored)) if (__k !== \"__v\" && !__known.has(__k)) __keep[__k] = stored[__k];",
                quote_list(&plan.known)
            )
            .unwrap();
            writeln!(body, "  return {{ ...__keep, ...{enc} }};").unwrap();
            stmts.push(bynk_ts::TsStmt::raw(
                format!(
                    "function {step_fn}(stored: globalThis.Record<string, unknown>): globalThis.Record<string, unknown> {{\n{body}}}\n"
                ),
                None,
            ));
            dispatch.push(migrate_step_dispatch(plan.lo, plan.from, &step_fn));
        }
        // The nearest step covering the stored version runs; a version no step
        // covers reads by zero-merge (it passed `__checkStoredSchemaVersion`).
        stmts.push(migrate_dispatcher_decl(&migrate_fn, dispatch));
    }
    let lazy_fields = emit_lazy_field_spec(a, layout, commons, ctx, stmts);
    AgentPersistFns {
        rehydrate_fn,
        encode_fn,
        migrate_fn,
        hydrate_fn,
        has_rehydrate,
        has_collections,
        has_encode,
        step_plans,
        agent_versions,
        lazy_fields,
    }
}

/// An agent class's handler methods (#3167): one per `on` handler, each lowered
/// to a class method and written onto `out`, in declaration order.
fn emit_agent_handlers(
    methods: &mut Vec<bynk_ts::TsClassMethod>,
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) {
    let is_store_agent = true;
    let AgentStoreLayout {
        effective_fields,
        held_maps_ts,
        map_names,
        map_value_ts,
        store_map_indexes,
        set_names,
        cache_ttls,
        log_retains,
        log_value_ts,
        ..
    } = layout;
    // 3) Handlers.
    let cell_names: HashSet<String> = effective_fields
        .iter()
        .map(|f| f.name.name.clone())
        .collect();
    for h in &a.handlers {
        // Arc C, slice 23 (#1375): closes step (9)'s own fourth sub-slice —
        // each handler converts to a real `bynk_ts::TsClassMethod` fragment,
        // the same "params/return-type real, whole body one opaque
        // `TsStmt::Raw`, two-level offset merge" shape `emit_service`
        // (#1361) already established for its own near-identical prologue/
        // body/epilogue wrapping — printed through `print_class_method`
        // (#1359) instead of `print_object_entry`, since a handler here is
        // a CLASS method, not an object-literal entry. The old standalone
        // `emit_doc_block` call is removed — `doc` on the method now
        // carries it, the same doc-duplication bug #1361's own review
        // caught fixed proactively here rather than rediscovered.
        let mut params: Vec<bynk_ts::TsParam> = h
            .params
            .iter()
            .map(|p| {
                bynk_ts::TsParam::new(
                    ts_ident(&p.name.name),
                    Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
                )
            })
            .collect();
        // Lower body into a buffer so we can detect cross-context usage and
        // shape the deps type accordingly.
        // v0.81: a store-agent handler reads/writes cells over a mutable working
        // record `__state`; a state-record handler uses `currentState`/`self.state`.
        // A store handler that performs any `:=` wraps its body in a closure so an
        // implicit commit runs at handler end on every (success) return path.
        // #1196/R6.5: write detection reads the checker's own resolved
        // `Callee::Store` classification (`bynk_model::body_writes_state`)
        // rather than matching a bare-identifier receiver name against this
        // agent's own field-name sets — a locally-shadowed field name (a
        // handler param, say) can no longer false-positive into an
        // unnecessary implicit-commit wrapper the way the deleted
        // `block_writes_state` could (`1196_agent_write_detection_via_
        // resolved_callee`'s own fixture pins the fix). A held `Map[K,
        // Connection]`'s own `put`/`remove` still triggers the commit flush
        // (v0.105, slice 3b-ii) with no extra name-set union needed here —
        // a held map is an ordinary `store Map` field as far as the checker's
        // own `Callee::Store` resolution is concerned, "held" being purely
        // this emitter's own downstream connection-resolution concern.
        let writes_state = is_store_agent && body_writes_state(&h.body, commons);
        let store = is_store_agent.then(|| {
            Box::new(AgentStoreState {
                state: ("__state".to_string(), cell_names.clone()),
                maps: map_names.clone(),
                sets: set_names.clone(),
                caches: cache_ttls.clone(),
                logs: log_retains.clone(),
                indexes: store_map_indexes.clone(),
                held_maps: held_maps_ts.clone(),
                map_values: map_value_ts.clone(),
                connection_maps: layout
                    .state
                    .iter()
                    .filter(|f| match &f.kind {
                        StoreKindIr::Map(_, v) => {
                            matches!(*commons.tys().get(*v), Ty::Connection(_))
                        }
                        _ => false,
                    })
                    .map(|f| f.field.clone())
                    .collect(),
                log_values: log_value_ts.clone(),
            })
        });
        // #934: mirrors the `method` name resolved below (all non-`method_name`
        // agent handler kinds resolve to `"call"`), computed here so it's
        // available before the body lowers.
        let scope_method = h
            .method_name
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| "call".to_string());
        let mut module = ModuleCtx::for_unit(commons, ctx);
        module.note_unit(ctx);
        // #1960: the body lowers for the build target, as a service body
        // does, so a cross-context call reads the Service Binding its `deps`
        // type names.
        module.target = ctx.target;
        module.agent_method_givens = ctx.agent_method_givens.clone();
        module.agent_called_contexts = ctx.agent_called_contexts.clone();
        module.event_schema_versions = ctx.event_schema_versions.clone();
        module.consumed_type_ns = consumed_type_ns(ctx);
        module.consumed_type_declared = ctx.consumed_type_aliases.clone();
        module.consumed_context_types = ctx.consumed_context_types();
        let mut cx = LowerCtx::new(
            module,
            BodyMode::AgentHandler {
                handler: HandlerShared {
                    capabilities: bynk_model::lower_handler_given_ir(h)
                        .into_iter()
                        .map(|c| c.name)
                        .collect::<HashSet<_>>(),
                    handler_scope: Some(format!(
                        "{}.{}.{}",
                        ctx.commons_name, a.name.name, scope_method
                    )),
                    owning_context: ctx.commons_name.clone(),
                    ..HandlerShared::default()
                },
                in_agent_handler: true,
                agent_key_field: Some(a.key_name.name.clone()),
                store,
            },
        );
        cx.local_agents = ctx.local_agents.clone();
        // A handler param is emitted at its natural `ts_ident` name (above), so
        // it must be declared into scope the same way: otherwise a param that
        // shares a name with a `store Map`/`Set`/`Cache`/`Log` field is
        // invisible to `LowerCtx::is_local`, and the store-field dispatch
        // below silently wins over the parameter.
        for p in &h.params {
            cx.declare_binder(&p.name.name);
        }
        // P6.54 (design/tracks/the-ir.md §6b): computed once, not twice —
        // `async_kw` below used to call `is_effectful_return(&h.return_type)`
        // again for the identical value.
        let effectful = is_effectful_return(&h.return_type);
        let async_tail = effectful;
        // A writing store handler's body sits one level deeper, inside the
        // implicit-commit closure.
        let body_indent = if writes_state {
            INDENT_STEP * 3
        } else {
            INDENT_STEP * 2
        };
        let lowered_body = block_body_with_return(
            &h.body,
            &mut cx,
            body_indent,
            async_tail,
            Some(&h.return_type),
        );
        let mut deps_members = build_deps_object_ty_with_surface(
            &effective_given(&bynk_model::lower_handler_given_ir(h), &cx),
            &cx,
            &ctx.cross_context,
            ctx.target,
        );
        // Events track, slice 0 (spine #936): see the matching field on the
        // service path (`emit_service`) — an agent handler that emits, or
        // that calls another local agent method which itself emits, gets
        // the same compose-supplied `__eventsDispatch` callback.
        let needs_events_dispatch = cx.is_first_party(FirstPartyCap::Events)
            && (bynk_model::block_uses_emit(&h.body, &commons.callees)
                || cx
                    .agent_given_caps_used()
                    .is_some_and(|m| m.contains_key("Events")));
        if needs_events_dispatch {
            // Arc F, slice 4 (#1463): stays opaque `Named` text — the same
            // `TsType::Fn`-has-no-named-params gap `emit_service`'s own
            // identical field already names.
            append_deps_field(
                &mut deps_members,
                "__eventsDispatch",
                bynk_ts::TsType::named(format!(
                    "(events: globalThis.Array<{}>) => globalThis.Promise<void>",
                    crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE
                )),
            );
        }
        params.push(bynk_ts::TsParam::new(
            "deps".to_string(),
            Some(bynk_ts::TsType::Object(deps_members)),
        ));
        let ret = ts_type_ref_to_ts_type(&h.return_type, None);
        let method = h
            .method_name
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| match lower_handler_kind_ir(&h.kind) {
                IrHandlerKind::Call => "call".to_string(),
                // HTTP/cron/queue/open handlers are service-only (rejected in
                // agents by the parser); these arms are defensive and unreachable
                // here.
                IrHandlerKind::Http { .. }
                | IrHandlerKind::Cron { .. }
                | IrHandlerKind::Message
                | IrHandlerKind::Open
                | IrHandlerKind::Close
                | IrHandlerKind::Event
                | IrHandlerKind::Rejected => "call".to_string(),
            });
        // Load state at entry. A state-record handler binds `currentState` and
        // commits explicitly via `commit`. A store handler binds a mutable
        // working record `__state`; reads/writes go through it, and (if it writes)
        // the body is wrapped so `commitState` runs once at handler end — the
        // implicit, atomic commit (ADR 0109). A fault before that flush persists
        // nothing; the invariant gate inside `commitState` runs before the write.
        //
        // 1A.15: the whole prologue + body + epilogue is statements, which the
        // printer records the checkpoints of (it was one raw text blob with
        // a source map of its own, merged at a computed offset).
        let body_emits_directly = bynk_model::block_uses_emit(&h.body, &commons.callees);
        let mut body = assemble_agent_body(
            lowered_body,
            is_store_agent,
            writes_state,
            body_emits_directly,
            history_handler(ctx, &scope_method),
        );
        // #3465: an agent handler that claimed settles its claims after the state
        // commit and the events flush, so a failure of either releases them.
        if cx.claim_scope_used {
            body = claim_scoped(body, INDENT_STEP * 2);
        }
        // #2270: anchor the method header and prologue to the handler's own
        // `on call` line. Without this they inherit the previous checkpoint,
        // so an uncalled handler's header leaked onto the preceding body's
        // last line in coverage.
        methods.push(bynk_ts::TsClassMethod {
            doc: h.documentation.as_deref().map(str::to_owned),
            span: Some(h.span),
            ..bynk_ts::TsClassMethod::new(
                method,
                effectful,
                params,
                Some(ret),
                body.into_stmts(INDENT_STEP * 2),
            )
        });
    }
}

/// #3581: in a `bynk dev` build the handler's name rides with its commit, so the
/// instance's history entry names it. `None` for every other build.
fn history_handler<'a>(ctx: &EmitProjectCtx, handler: &'a str) -> Option<&'a str> {
    ctx.inspect_agents.then_some(handler)
}

/// `const __events: ... = [];`.
fn events_decl_item() -> Item {
    Item::Node(Box::new(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__events".to_string()),
        Some(bynk_ts::TsType::named_with_args(
            "globalThis.Array",
            vec![bynk_ts::TsType::named(
                crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE,
            )],
        )),
        bynk_ts::TsExpr::array(Vec::new()),
        None,
    )))
}

/// The line that opens the closure a handler body runs in so that its own
/// `return`s resolve `__result`. Text, because the body sits at this line's own
/// indentation, where a nested arrow body would sit one step deeper.
fn result_closure_open() -> Item {
    Item::line("const __result = await (async () => {")
}

fn result_closure_close() -> Item {
    Item::line("})();")
}

/// `if (__events.length > 0) { await deps.__eventsDispatch(__events); }`, with
/// the call on its own lines in a braced block, or `inline` on one.
fn events_flush_item(inline: bool) -> Item {
    let ident = |name: &str| bynk_ts::TsExpr::Ident(name.to_string());
    let dispatch = bynk_ts::TsStmt::expr_stmt(
        bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(ident("deps")),
                property: "__eventsDispatch".to_string(),
            }),
            args: vec![ident("__events")],
        })),
        None,
    );
    let then = if inline {
        bynk_ts::TsStmt::inline_block(vec![dispatch], None)
    } else {
        bynk_ts::TsStmt::block(vec![dispatch], None)
    };
    let cond = bynk_ts::TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::GreaterThan,
        left: Box::new(bynk_ts::TsExpr::Member {
            object: Box::new(ident("__events")),
            property: "length".to_string(),
        }),
        right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("0".to_string()))),
    };
    Item::Node(Box::new(bynk_ts::TsStmt::if_stmt(cond, then, None)))
}

fn return_result_item() -> Item {
    Item::Node(Box::new(bynk_ts::TsStmt::return_stmt(
        Some(bynk_ts::TsExpr::Ident("__result".to_string())),
        None,
    )))
}

/// A service handler's body when it emits events directly: the buffered
/// `__events`, the body inside the closure that resolves `__result`, then the
/// flush to whatever composed the handler, and the result.
fn events_release_body(body: Body) -> Body {
    let mut wrapped = Body::new();
    wrapped.push(events_decl_item());
    wrapped.push(result_closure_open());
    wrapped.append(body);
    wrapped.push(result_closure_close());
    wrapped.push(events_flush_item(false));
    wrapped.push(return_result_item());
    wrapped
}

/// `await this.<method>(<args>)`.
fn this_await(method: &str, args: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
            property: method.to_string(),
        }),
        args,
    }))
}

/// One agent handler method's prologue + lowered body + epilogue (#3167). A
/// store handler that writes wraps its body in the implicit-commit closure; one
/// that emits events directly flushes them after it. The body items are
/// `lowered`, lowered at one step deeper when the closure wraps them.
fn assemble_agent_body(
    lowered: Body,
    is_store_agent: bool,
    writes_state: bool,
    body_emits_directly: bool,
    history_handler: Option<&str>,
) -> Body {
    let mut body = Body::new();
    let ident = |name: &str| bynk_ts::TsBindingName::Ident(name.to_string());
    let state_decl = if !is_store_agent {
        bynk_ts::TsStmt::const_stmt(
            ident("currentState"),
            None,
            this_await("loadState", Vec::new()),
            None,
        )
    } else if writes_state {
        bynk_ts::TsStmt::const_stmt(
            ident("__state"),
            None,
            bynk_ts::TsExpr::object_entries(vec![bynk_ts::TsObjectEntry::Spread(
                bynk_ts::TsExpr::Paren(Box::new(this_await("loadState", Vec::new()))),
            )]),
            None,
        )
    } else {
        bynk_ts::TsStmt::const_stmt(
            ident("__state"),
            None,
            this_await("loadState", Vec::new()),
            None,
        )
    };
    body.push(Item::Node(Box::new(state_decl)));
    // Only a handler that wraps its body (a commit, or direct events) needs the closure.
    if !writes_state && !body_emits_directly {
        body.append(lowered);
        return body;
    }
    if body_emits_directly {
        body.push(events_decl_item());
    }
    body.push(result_closure_open());
    // A writing handler's body sits one step deeper, inside the closure.
    if writes_state {
        body.push(Item::Indent(lowered));
    } else {
        body.append(lowered);
    }
    body.push(result_closure_close());
    if writes_state {
        let mut args = vec![bynk_ts::TsExpr::Ident("__state".to_string())];
        if let Some(h) = history_handler {
            args.push(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(h.to_string())));
        }
        body.push(Item::Node(Box::new(bynk_ts::TsStmt::expr_stmt(
            this_await("commitState", args),
            None,
        ))));
    }
    if body_emits_directly {
        body.push(events_flush_item(true));
    }
    body.push(return_result_item());
    body
}

/// What an agent class's dispatch methods depend on, decided while the class
/// header was written (#3167).
struct AgentDispatchFacts<'a> {
    /// The handler reads the agent's key (`self.key`).
    reads_key: bool,
    /// The `given` capability dependencies the RPC branch rebuilds, if any.
    given_deps_expr: &'a Option<String>,
    /// Whether the RPC branch rebuilds the agent's environment at all.
    rpc_rebuilds_env: bool,
    /// Whether a handler body emits events directly.
    agent_uses_emit: bool,
    /// Whether a handler body reaches the Worker's own `env`.
    agent_reaches_env: bool,
    /// The agent's declared schema versions, if it has any.
    agent_versions: &'a Option<bynk_check::schema_registry::AgentVersions>,
}

/// The tail of an agent class (#3167): the Workers `fetch` dispatch that turns
/// an RPC request into a method call, the dispatch for each `from websocket`
/// `on open` host, and the class's closing brace. Written onto `out`.
fn emit_agent_dispatch(
    methods: &mut Vec<bynk_ts::TsClassMethod>,
    a: &AgentDecl,
    facts: &AgentDispatchFacts,
    ws_open_hosts: &[WsOpenHost<'_>],
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) {
    let tys = UnitFacts::of(ctx).render(commons);
    let AgentDispatchFacts {
        reads_key,
        given_deps_expr,
        rpc_rebuilds_env,
        agent_uses_emit,
        agent_reaches_env,
        agent_versions,
    } = *facts;
    // v0.9.2: workers-mode DO dispatch. Method calls arrive as `fetch` requests
    // under `/_bynk/agent/<method>`; decode `{ args, deps }`, invoke the
    // handler with deps as the trailing argument, and serialise the result.
    if matches!(ctx.target, BuildTarget::Workers) {
        // Arc C, slice 27 (#1384): the third and final of step (9) sub-slice
        // (5)'s own 3 independent slices — converts to a real
        // `bynk_ts::TsClassMethod`, printed through `print_class_method`, the
        // same zero-`LowerCtx`/no-merge shape #1382 (`emit_ws_dispatch_
        // handlers`) established for this cluster's other Rust-side-only
        // emitter. `(this as unknown as Record<string, (...bynkArgs:
        // unknown[]) => unknown>)` stays one opaque `TsExpr::Ident` (a
        // nested `As`-under-`As` chain the printer's own `needs_parens_as_
        // operand` would otherwise wrap in an extra, wrong parenthesis pair
        // — a real, previously-unpredicted algebra gap, not a style choice);
        // `...args` stays one opaque `TsExpr::Ident` call argument (the
        // spread-call-argument shape the grounding pass DID predict for this
        // slice, #1382's own PR confirming it belongs here, not there).
        let mut fetch_stmts = vec![bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("url".to_string()),
            None,
            bynk_ts::TsExpr::New {
                callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.URL".to_string())),
                args: vec![bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("request".to_string())),
                    property: "url".to_string(),
                }],
            },
            None,
        )];
        // v0.104 (slice 3b): a forwarded WebSocket upgrade. The edge has already
        // authenticated the actor (the body never runs unverified); accept the
        // socket here, run the on-open body, and return the `101` carrying the
        // client end. The verified identity and route arguments ride in a trusted
        // internal header (the DO is only reachable through the Worker).
        for host in ws_open_hosts {
            fetch_stmts.push(ws_open_fetch_branch_stmt(host, tys));
        }
        // State-migrations S4 (#2259 Decision D): `bynk migrate sweep`'s wake,
        // a branch of its own before the method dispatch. It carries no key
        // (steps are key-free), loads the state (running any step), and
        // commits only a record older than this build's version. Reachable
        // only through this Worker's signed `/_bynk/migrate/` route.
        fetch_stmts.extend(wake_and_inspect_branches(a, agent_versions, ctx));
        let dispatch_callee = bynk_ts::TsExpr::Index {
            object: Box::new(bynk_ts::TsExpr::Ident(
                "(this as unknown as globalThis.Record<string, (...bynkArgs: unknown[]) => unknown>)"
                    .to_string(),
            )),
            index: Box::new(bynk_ts::TsExpr::Ident("methodName".to_string())),
        };
        let mut agent_dispatch_stmts = vec![
            bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("methodName".to_string()),
                None,
                bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Member {
                            object: Box::new(bynk_ts::TsExpr::Ident("url".to_string())),
                            property: "pathname".to_string(),
                        }),
                        property: "slice".to_string(),
                    }),
                    args: vec![bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            "/_bynk/agent/".to_string(),
                        ))),
                        property: "length".to_string(),
                    }],
                },
                None,
            ),
            bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::ObjectPattern(if reads_key {
                    vec!["args".to_string(), "deps".to_string(), "key".to_string()]
                } else {
                    vec!["args".to_string(), "deps".to_string()]
                }),
                None,
                bynk_ts::TsExpr::As {
                    expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Await(
                        Box::new(bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Member {
                                object: Box::new(bynk_ts::TsExpr::Ident("request".to_string())),
                                property: "json".to_string(),
                            }),
                            args: Vec::new(),
                        }),
                    )))),
                    ty: bynk_ts::TsType::Object(
                        vec![
                            bynk_ts::TsTypeMember::prop(
                                "args",
                                bynk_ts::TsType::array(bynk_ts::TsType::named("unknown")),
                            ),
                            bynk_ts::TsTypeMember::prop("deps", bynk_ts::TsType::named("unknown")),
                        ]
                        .into_iter()
                        .chain(reads_key.then(|| {
                            bynk_ts::TsTypeMember::prop("key", bynk_ts::TsType::named("unknown"))
                        }))
                        .collect(),
                    ),
                },
                None,
            ),
        ];
        // #1818: an agent that reads its key learns it from the call, decoded
        // by its key codec, before the handler runs.
        if reads_key {
            agent_dispatch_stmts.push(agent_key_assign(
                &agent_key_codec_name(&a.name.name),
                &crate::emitter::ts_types::ts_type_ref(&a.key_type),
                "key",
            ));
        }
        // #1678: the wire arguments are decoded through this agent's table
        // before the handler sees them (`decodeAgentArgs` throws on a decode
        // failure, an internal fault), and the result is encoded on the way out.
        let decoded_args = bynk_ts::TsExpr::Spread(Box::new(hist_call(
            hist_id("__decodeAgentArgs"),
            vec![
                hist_id(&agent_wire_name(&a.name.name)),
                hist_id("methodName"),
                hist_id("args"),
            ],
        )));
        let result_expr = if rpc_rebuilds_env {
            // #527: the wire deps are JSON — any capability provider in them
            // is a dead plain object (its methods did not survive
            // serialisation). Rebuild this agent's `given` deps in-process,
            // exactly as compose wires them, and let them win the merge.
            //
            // Events track, slice 0: `deps.__eventsDispatch` is a function
            // too, and does not survive the wire any better than a provider
            // — rebuilt from this Worker's own `env.EVENTS_FANOUT` binding
            // (shared by every DO class this script hosts) rather than
            // trusting whatever the caller's JSON carried.
            // P7.2: matches `emitter/workers.rs`'s own established idiom for a
            // multi-binding env whose exact accessed keys vary by call site
            // (`given_deps_expr` may reference other bindings besides
            // `EVENTS_FANOUT`, computed elsewhere and not fully traced here) —
            // a generic string-keyed record, not a precise structural type.
            //
            // `this.__env as unknown as Record<string, unknown>` stays one
            // opaque `TsExpr::Ident` — the same nested-`As`-chain gap the
            // dispatch callee above hits.
            agent_dispatch_stmts.push(agent_env_const_stmt());
            let mut entries = vec![bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Paren(
                Box::new(bynk_ts::TsExpr::As {
                    expr: Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::NullishCoalescing,
                        left: Box::new(bynk_ts::TsExpr::Ident("deps".to_string())),
                        right: Box::new(bynk_ts::TsExpr::object(Vec::new())),
                    }),
                    ty: bynk_ts::TsType::named_with_args(
                        "globalThis.Record",
                        vec![
                            bynk_ts::TsType::named("string"),
                            bynk_ts::TsType::named("unknown"),
                        ],
                    ),
                }),
            ))];
            if let Some(expr) = &given_deps_expr {
                agent_dispatch_stmts.push(given_deps_const_stmt(expr));
                entries.push(bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(
                    "__givenDeps".to_string(),
                )));
            }
            if agent_uses_emit {
                agent_dispatch_stmts.push(events_deps_const_stmt());
                entries.push(bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(
                    "__eventsDeps".to_string(),
                )));
            }
            if agent_reaches_env {
                entries.push(bynk_ts::TsObjectEntry::Shorthand("env".to_string()));
            }
            // P7.2: `methodName` is read from `url.pathname` at runtime, so no
            // static type can name which handler this is — a callable
            // dictionary is what's actually known, and `result` flows only into
            // a generic `JSON.stringify` below, so `unknown` costs nothing there.
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(dispatch_callee),
                args: vec![
                    decoded_args.clone(),
                    bynk_ts::TsExpr::object_entries(entries),
                ],
            }))
        } else {
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(dispatch_callee),
                args: vec![decoded_args, bynk_ts::TsExpr::Ident("deps".to_string())],
            }))
        };
        agent_dispatch_stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("result".to_string()),
            None,
            result_expr,
            None,
        ));
        // #1678: the result is encoded through the wire table. A void method's
        // `()` codec encodes `null`; before #1678 `?? null` did that here,
        // since `JSON.stringify(undefined)` is the *string* `undefined` — not
        // JSON — which the calling proxy's `response.json()` rejects (#527).
        agent_dispatch_stmts.push(bynk_ts::TsStmt::return_stmt(
            Some(bynk_ts::TsExpr::New {
                callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.Response".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Member {
                            object: Box::new(bynk_ts::TsExpr::Ident("globalThis.JSON".to_string())),
                            property: "stringify".to_string(),
                        }),
                        args: vec![bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Ident(
                                "__encodeAgentResult".to_string(),
                            )),
                            args: vec![
                                bynk_ts::TsExpr::Ident(agent_wire_name(&a.name.name)),
                                bynk_ts::TsExpr::Ident("methodName".to_string()),
                                bynk_ts::TsExpr::Ident("result".to_string()),
                            ],
                        }],
                    },
                    bynk_ts::TsExpr::object(vec![(
                        "headers".to_string(),
                        bynk_ts::TsExpr::object(vec![(
                            "\"content-type\"".to_string(),
                            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                "application/json".to_string(),
                            )),
                        )]),
                    )]),
                ],
            }),
            None,
        ));
        fetch_stmts.push(bynk_ts::TsStmt::if_stmt(
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("url".to_string())),
                        property: "pathname".to_string(),
                    }),
                    property: "startsWith".to_string(),
                }),
                args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    "/_bynk/agent/".to_string(),
                ))],
            },
            bynk_ts::TsStmt::block(agent_dispatch_stmts, None),
            None,
        ));
        fetch_stmts.push(bynk_ts::TsStmt::return_stmt(
            Some(bynk_ts::TsExpr::New {
                callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.Response".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str("Not Found".to_string())),
                    bynk_ts::TsExpr::object(vec![(
                        "status".to_string(),
                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("404".to_string())),
                    )]),
                ],
            }),
            None,
        ));
        // #2280: the dispatch methods belong to the agent, not to the last
        // hosted lifecycle method's final body line.
        let fetch_method = bynk_ts::TsClassMethod::new(
            "fetch",
            true,
            vec![bynk_ts::TsParam::new(
                "request".to_string(),
                Some(bynk_ts::TsType::named("globalThis.Request")),
            )],
            Some(bynk_ts::TsType::named_with_args(
                "globalThis.Promise",
                vec![bynk_ts::TsType::named("globalThis.Response")],
            )),
            fetch_stmts,
        );
        methods.push(bynk_ts::TsClassMethod {
            span: Some(a.span),
            ..fetch_method
        });
        // v0.106 (slice 3b-iii): the inbound/close dispatch — Cloudflare calls
        // `webSocketMessage`/`webSocketClose` on a hibernatable socket. Decode the
        // frame against `in:` (reject-and-close on failure), recover the sender
        // identity + route args from the socket attachment, and run the body.
        for host in ws_open_hosts {
            emit_ws_dispatch_handlers(methods, host, &ctx.runtime_use, &commons.types, tys);
        }
    }
}

/// #3581: `__noteAgentInstance(env?.__BYNK_INSPECT_REGISTRY, env?.<binding>,
/// "<Agent>", key, <key codec>, <make>)`: the factory's agent, recorded in the
/// Worker's dev registry on first address and returned as it was.
fn inspect_noted(a: &AgentDecl, binding: &str, make: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    let env_prop = |property: &str| bynk_ts::TsExpr::OptionalMember {
        object: Box::new(bynk_ts::TsExpr::Ident("env".to_string())),
        property: property.to_string(),
    };
    bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Ident("__noteAgentInstance".to_string())),
        args: vec![
            env_prop("__BYNK_INSPECT_REGISTRY"),
            env_prop(binding),
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(a.name.name.clone())),
            bynk_ts::TsExpr::Ident("key".to_string()),
            bynk_ts::TsExpr::Ident(agent_key_codec_name(&a.name.name)),
            make,
        ],
    }
}

/// The agent-construction factory (#3167): `AgentName(key)` lowers to a call of
/// this, which on the bundle target builds the agent over its registered state
/// and on Workers hands back a proxy over the Durable Object stub. Pushes the
/// factory declaration onto `stmts`.
fn emit_agent_factory(
    a: &AgentDecl,
    ctx: &EmitProjectCtx,
    reads_key: bool,
    registry: &str,
    stmts: &mut Vec<bynk_ts::TsStmt>,
) {
    // v0.9.2: agent-construction factory. Lowering of `AgentName(key)` calls
    // this. A present DO binding (workers) routes through `makeWorkersAgent`;
    // otherwise the bundle registry path is taken. The single `makeAgent`
    // helper keeps the call site target-agnostic.
    //
    // Arc C, slice 24 (#1377): closes step (9)'s own sixth sub-slice, the
    // factory function half — the smaller, self-contained half of the
    // originally-lumped "(6) the factory function plus the history-driver"
    // item, converted independently of the history-driver (which stays
    // deferred, its own priority undecided). Converts fully, no opaque
    // carve-out at all: every shape needed (`TsDecl::Function`,
    // `TsExpr::OptionalMember`, `TsExpr::Arrow`, `TsExpr::New`) already
    // existed. The WS-hosted DO methods/`fetch` dispatcher/
    // `emit_ws_dispatch_handlers` cluster (originally sub-slice (5)) stays
    // deferred here — as of this slice, not yet grounded; its own dedicated
    // grounding pass (post-#1378, no issue number) has since found it
    // genuinely lower source-map risk than this comment's own caution
    // implied, decomposing into 3 independent slices — see
    // `design/tracks/the-typescript-tree.md` §6 for the current state,
    // since a comment landed at one slice's own commit is a snapshot, not
    // a live pointer.
    let workers = matches!(ctx.target, BuildTarget::Workers);
    // #3581: a `bynk dev` build records the key each instance is addressed by, so it
    // needs the key codec even for an agent that never reads its key.
    let inspect = workers && ctx.inspect_agents;
    if workers {
        stmts.push(agent_wire_table(a, &ctx.runtime_use));
        if reads_key || inspect {
            stmts.push(agent_key_codec(a, &ctx.runtime_use));
        }
    }
    let key_ts = ts_type_ref_to_ts_type(&a.key_type, None);
    let bind = crate::emitter::wrangler::agent_binding(&a.name.name).name;
    let mut make_agent_args = vec![
        bynk_ts::TsExpr::Ident(registry.to_string()),
        bynk_ts::TsExpr::OptionalMember {
            object: Box::new(bynk_ts::TsExpr::Ident("env".to_string())),
            property: bind.clone(),
        },
        bynk_ts::TsExpr::Ident("key".to_string()),
        bynk_ts::TsExpr::Arrow {
            params: vec![bynk_ts::TsParam::new("state".to_string(), None)],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(if reads_key {
                // #1818: the bundle instance learns its key here.
                bynk_ts::TsExpr::Ident(format!(
                    "globalThis.Object.assign(new {}(state), {{ __key: key }})",
                    a.name.name
                ))
            } else {
                bynk_ts::TsExpr::New {
                    callee: Box::new(bynk_ts::TsExpr::Ident(a.name.name.clone())),
                    args: vec![bynk_ts::TsExpr::Ident("state".to_string())],
                }
            }))),
        },
    ];
    // #1678: on `workers` the proxy over the DO stub encodes arguments and
    // decodes the result through this agent's wire table.
    if workers {
        make_agent_args.push(bynk_ts::TsExpr::Ident(agent_wire_name(&a.name.name)));
        if reads_key {
            make_agent_args.push(bynk_ts::TsExpr::Ident(agent_key_codec_name(&a.name.name)));
        }
    }
    // #3581: the factory hands the agent back through `__noteAgentInstance`, which
    // records the instance in the dev registry and returns it unchanged.
    let make_agent = bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Ident("__makeAgent".to_string())),
        args: make_agent_args,
    };
    let make_agent = if inspect {
        inspect_noted(a, &bind, make_agent)
    } else {
        make_agent
    };
    let mut env_props = vec![bynk_ts::TsTypeMember::Prop {
        name: bind.clone(),
        ty: bynk_ts::TsType::named("__DurableObjectNamespace"),
        optional: true,
        readonly: false,
    }];
    if inspect {
        env_props.push(bynk_ts::TsTypeMember::Prop {
            name: "__BYNK_INSPECT_REGISTRY".to_string(),
            ty: bynk_ts::TsType::named("__DurableObjectNamespace"),
            optional: true,
            readonly: false,
        });
    }
    let factory_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Function {
            name: agent_factory_name(&a.name.name),
            generics: Vec::new(),
            params: vec![
                bynk_ts::TsParam::new("key".to_string(), Some(key_ts)),
                bynk_ts::TsParam::optional(
                    "env".to_string(),
                    Some(bynk_ts::TsType::Object(env_props)),
                ),
            ],
            return_type: Some(bynk_ts::TsType::named(a.name.name.clone())),
            body: vec![bynk_ts::TsStmt::return_stmt(Some(make_agent), None)],
            is_async: false,
            inline: false,
        })),
        None,
    );
    stmts.push(factory_decl);
    // #1486: no explicit blank stmt here — the printer's automatic top-level
    // spacing policy supplies the single blank before whatever follows.
}

/// The history-property driver (#3167): an exported, test-only function for each
/// agent a `for all run: History[Agent]` property targets, so every other agent's
/// emission is unchanged. Pushes it onto `stmts` after the agent's factory.
fn emit_agent_history_driver(
    a: &AgentDecl,
    fns: &AgentLoadFns,
    registry: &str,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    stmts: &mut Vec<bynk_ts::TsStmt>,
) {
    let state_ty = format!("{}State", a.name.name);
    // v0.119 (testing track slice 7, ADR 0155): the history-property driver. Only
    // agents a `for all run: History[Agent]` property targets get this exported
    // test-support function (gated on `history_target_agents`), so every other
    // agent's emission is byte-for-byte unchanged. It drives a generated call
    // sequence through the *real* handlers from a fresh instance, recording each
    // reached step — its call, whether it committed (an invariant/`transition`
    // refusal throws `invariantViolation`, leaving state uncommitted), and the
    // committed `old` → `new` pair — for the runner's predicate. Test-only, so it
    // is stripped from deploy builds with the rest of the test surface.
    if ctx.history_target_agents.contains(&a.name.name) {
        let hs: Vec<&Handler> = a
            .handlers
            .iter()
            .filter(|h| {
                matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Call)
                    && h.method_name.is_some()
            })
            .collect();
        let driver = format!("__bynkDriveHistory_{}", a.name.name);
        // P7.2: `call`'s real shape is a discriminated union across `hs` (one
        // variant per handler, built further down from `history_variant_tag` +
        // each handler's own fields) and `deps` varies per handler too (each
        // handler computes its own `deps_ty` from its own `given` set,
        // `build_deps_object_ty_with_surface`'s own per-handler call elsewhere
        // in this file) — a single driver-wide type for either needs the
        // intersection/union across every handler this agent's history targets,
        // not a same-line change. `seq`'s own `args` is safe to narrow:
        // it's read back only through `history_arg_expr` call sites below,
        // never given a fixed shape of its own.
        let step_ty = history_step_ty(&state_ty);
        let seq_ty = bynk_ts::TsType::named_with_args(
            "globalThis.Array",
            vec![bynk_ts::TsType::Object(vec![
                bynk_ts::TsTypeMember::prop("h", bynk_ts::TsType::named("number")),
                bynk_ts::TsTypeMember::prop(
                    "args",
                    bynk_ts::TsType::array(bynk_ts::TsType::named("unknown")),
                ),
            ])],
        );
        let mut body = vec![
            bynk_ts::TsStmt::expr_stmt(
                hist_call(hist_member(hist_id(registry), "reset"), vec![]),
                None,
            ),
            history_inst_stmt(a, commons),
            hist_const("__load", history_load_arrow(a, fns, &state_ty)),
            hist_const("__rej", history_rejected_arrow()),
            bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("__steps".to_string()),
                Some(history_steps_ty(&step_ty)),
                bynk_ts::TsExpr::array(Vec::new()),
                None,
            ),
            hist_const("__ce", hist_console_error()),
        ];
        body.push(bynk_ts::TsStmt::assign(
            hist_console_error(),
            history_console_mute_arrow(),
            None,
        ));
        // A driven run deliberately provokes rejected steps (an invariant/
        // `transition` refusal), each of which `console.error`s an
        // `InvariantViolation` line from `commitState` before throwing. Mute just
        // those lines for the duration of the drive so the run isn't drowned in
        // expected noise; every other `console.error` still passes through, and the
        // original is restored in `finally`.
        body.push(bynk_ts::TsStmt::try_finally(
            bynk_ts::TsStmt::block(
                vec![
                    history_drive_loop(&hs, &step_ty, commons),
                    bynk_ts::TsStmt::return_stmt(Some(hist_id("__steps")), None),
                ],
                None,
            ),
            bynk_ts::TsStmt::block(
                vec![bynk_ts::TsStmt::assign(
                    hist_console_error(),
                    hist_id("__ce"),
                    None,
                )],
                None,
            ),
            None,
        ));
        stmts.push(bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Function {
                name: driver,
                generics: Vec::new(),
                params: vec![
                    bynk_ts::TsParam::new("seq", Some(seq_ty)),
                    bynk_ts::TsParam::new("deps", Some(history_any_ty())),
                ],
                return_type: Some(bynk_ts::TsType::named_with_args(
                    "globalThis.Promise",
                    vec![history_steps_ty(&step_ty)],
                )),
                body,
                is_async: true,
                inline: false,
            })),
            None,
        ));
        // #1486: no trailing blank baked in here — the printer's automatic
        // top-level spacing policy supplies the single blank before whatever
        // statement follows.
    }
}

/// `any`, the one escape the history driver keeps (P7.2): `call` and `deps` take a different
/// shape per handler, so a single driver-wide type would need a per-agent union.
fn history_any_ty() -> bynk_ts::TsType {
    bynk_ts::TsType::named("any")
}

fn hist_id(name: &str) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Ident(name.to_string())
}

fn hist_str(text: &str) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(text.to_string()))
}

fn hist_num(n: impl ToString) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(n.to_string()))
}

fn hist_member(object: bynk_ts::TsExpr, property: &str) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Member {
        object: Box::new(object),
        property: property.to_string(),
    }
}

fn hist_call(callee: bynk_ts::TsExpr, args: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Call {
        callee: Box::new(callee),
        args,
    }
}

fn hist_await(expr: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Await(Box::new(expr))
}

fn hist_const(name: &str, init: bynk_ts::TsExpr) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(name.to_string()),
        None,
        init,
        None,
    )
}

fn hist_let(
    name: &str,
    ty: Option<bynk_ts::TsType>,
    init: Option<bynk_ts::TsExpr>,
) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::let_stmt(
        bynk_ts::TsBindingName::Ident(name.to_string()),
        ty,
        init,
        None,
    )
}

/// `globalThis.console.error`.
fn hist_console_error() -> bynk_ts::TsExpr {
    hist_member(hist_member(hist_id("globalThis"), "console"), "error")
}

fn hist_binary(
    op: bynk_ts::TsBinaryOp,
    left: bynk_ts::TsExpr,
    right: bynk_ts::TsExpr,
) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

/// `{ call: any, accepted: boolean, old: S, new: S, refusal?: unknown }`: one reached step.
fn history_step_ty(state_ty: &str) -> bynk_ts::TsType {
    let state = || bynk_ts::TsType::named(state_ty.to_string());
    bynk_ts::TsType::Object(vec![
        bynk_ts::TsTypeMember::prop("call", history_any_ty()),
        bynk_ts::TsTypeMember::prop("accepted", bynk_ts::TsType::named("boolean")),
        bynk_ts::TsTypeMember::prop("old", state()),
        bynk_ts::TsTypeMember::prop("new", state()),
        bynk_ts::TsTypeMember::optional_prop("refusal", bynk_ts::TsType::named("unknown")),
    ])
}

/// `globalThis.Array<step>`.
fn history_steps_ty(step_ty: &bynk_ts::TsType) -> bynk_ts::TsType {
    bynk_ts::TsType::named_with_args("globalThis.Array", vec![step_ty.clone()])
}

/// `{ invariantViolation?: unknown }`: the one field a caught throw is checked for.
fn history_violation_ty() -> bynk_ts::TsType {
    bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::optional_prop(
        "invariantViolation",
        bynk_ts::TsType::named("unknown"),
    )])
}

/// `(e as { invariantViolation?: unknown }).invariantViolation`.
fn history_violation_of(caught: &str) -> bynk_ts::TsExpr {
    hist_member(
        bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
            expr: Box::new(hist_id(caught)),
            ty: history_violation_ty(),
        })),
        "invariantViolation",
    )
}

/// `const __inst = <factory>(<key zero>) as any;`. P7.2: deferred, not narrowed. A first
/// attempt dropped this cast, reasoning the factory already declares its agent and `.state`/
/// `.{method}` are real declared members — true, but it broke real `tsc --strict` fixtures
/// (248/249): `history_arg_expr` produces unbranded plain values (`Number(__st.args[0])`),
/// which don't structurally match a branded handler param type (`Amount`) once `__inst` is
/// genuinely agent-typed instead of `any`. Branding `history_arg_expr`'s own output is the
/// real fix, not a same-line change here.
fn history_inst_stmt(a: &AgentDecl, commons: &TypedCommons) -> bynk_ts::TsStmt {
    let key = bynk_check::checker::zero_value(&a.key_type, None, &commons.types)
        .map(|z| zero_value_expr(&z))
        .unwrap_or_else(|| bynk_ts::TsExpr::As {
            expr: Box::new(hist_id("undefined")),
            ty: bynk_ts::TsType::named("never"),
        });
    hist_const(
        "__inst",
        bynk_ts::TsExpr::As {
            expr: Box::new(hist_call(
                hist_id(&agent_factory_name(&a.name.name)),
                vec![key],
            )),
            ty: history_any_ty(),
        },
    )
}

/// The `async` arrow that reads the instance's stored state the way `loadState` does (#1649,
/// #2211, #2224, #3189): decoded, version-checked, joined and migrated into the state type.
fn history_load_arrow(a: &AgentDecl, fns: &AgentLoadFns, state_ty: &str) -> bynk_ts::TsExpr {
    let AgentLoadFns {
        zero_fn,
        rehydrate_fn,
        migrate_fn,
        hydrate_fn,
        has_rehydrate,
        has_collections,
        step_plans,
        agent_versions,
        ..
    } = *fns;
    let stored = || {
        hist_member(
            hist_member(hist_member(hist_id("__inst"), "state"), "storage"),
            "get",
        )
    };
    let assign_s = |value: bynk_ts::TsExpr| bynk_ts::TsStmt::assign(hist_id("__s"), value, None);
    let zero = || hist_call(hist_id(zero_fn), vec![]);
    let mut body = vec![
        // `let`: the stored record is replaced by its joined, wholesale-shaped form below
        // (#3189).
        hist_let(
            "__s",
            None,
            Some(hist_await(hist_call(stored(), vec![hist_str("state")]))),
        ),
        bynk_ts::TsStmt::if_stmt(
            hist_binary(
                bynk_ts::TsBinaryOp::StrictEq,
                hist_id("__s"),
                hist_id("undefined"),
            ),
            bynk_ts::TsStmt::return_stmt(Some(zero()), None),
            None,
        ),
        bynk_ts::TsStmt::expr_stmt(
            hist_call(
                hist_id("__checkStoredLayout"),
                vec![hist_str(&a.name.name), hist_id("__s")],
            ),
            None,
        ),
    ];
    // State-migrations S1 (#2211): the same `__v` check `loadState` runs.
    if let Some(v) = agent_versions {
        let mut args = vec![
            hist_str(&a.name.name),
            hist_id("__s"),
            hist_num(v.version),
            hist_num(v.oldest_readable),
        ];
        if !step_plans.is_empty() {
            args.push(step_ranges_expr(step_plans));
        }
        body.push(bynk_ts::TsStmt::expr_stmt(
            hist_call(hist_id("__checkStoredSchemaVersion"), args),
            None,
        ));
    }
    body.push(assign_s(hist_await(hist_call(
        hist_id("__readStored"),
        vec![
            hist_id("__inst"),
            hist_member(hist_member(hist_id("__inst"), "state"), "storage"),
            hist_id("__s"),
        ],
    ))));
    // State-migrations S2 (#2224): and the same step dispatch.
    if !step_plans.is_empty() {
        body.push(assign_s(hist_call(
            hist_id(migrate_fn),
            vec![hist_id("__s")],
        )));
    }
    body.push(hist_const(
        "__m",
        bynk_ts::TsExpr::object_entries(vec![
            bynk_ts::TsObjectEntry::Spread(zero()),
            bynk_ts::TsObjectEntry::Spread(hist_id("__s")),
        ]),
    ));
    // #1649: decode the stored (wire-shape) state exactly as `loadState` does.
    for (wanted, func) in [(has_rehydrate, rehydrate_fn), (has_collections, hydrate_fn)] {
        if wanted {
            body.push(bynk_ts::TsStmt::expr_stmt(
                hist_call(hist_id(func), vec![hist_id("__m"), hist_id("__s")]),
                None,
            ));
        }
    }
    body.push(bynk_ts::TsStmt::return_stmt(Some(hist_id("__m")), None));
    bynk_ts::TsExpr::Arrow {
        params: Vec::new(),
        is_async: true,
        generics: Vec::new(),
        return_type: Some(bynk_ts::TsType::named_with_args(
            "globalThis.Promise",
            vec![bynk_ts::TsType::named(state_ty.to_string())],
        )),
        body: Box::new(bynk_ts::TsArrowBody::Block(body)),
    }
}

/// `(e: unknown) => !!e && (e as { invariantViolation?: unknown }).invariantViolation !== undefined`.
/// P7.2: `e` is a caught throw of unknown shape by construction — a marker type stating
/// exactly the one field this checks, not a blanket escape.
fn history_rejected_arrow() -> bynk_ts::TsExpr {
    let not = |e| bynk_ts::TsExpr::Unary {
        op: bynk_ts::TsUnaryOp::Not,
        expr: Box::new(e),
    };
    bynk_ts::TsExpr::Arrow {
        params: vec![bynk_ts::TsParam::new(
            "e",
            Some(bynk_ts::TsType::named("unknown")),
        )],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(hist_binary(
            bynk_ts::TsBinaryOp::And,
            not(not(hist_id("e"))),
            hist_binary(
                bynk_ts::TsBinaryOp::StrictNotEq,
                history_violation_of("e"),
                hist_id("undefined"),
            ),
        )))),
    }
}

/// `(...__a: unknown[]) => { if (typeof __a[0] === "string" && __a[0].startsWith("InvariantViolation")) return; __ce(...__a); }`
/// P7.2: `console.error`'s own real type is `(...data: unknown[]) => void` — `__ce` is already
/// that type by inference, and the replacement matches it exactly, so no cast was ever
/// load-bearing.
fn history_console_mute_arrow() -> bynk_ts::TsExpr {
    let first = || bynk_ts::TsExpr::Index {
        object: Box::new(hist_id("__a")),
        index: Box::new(hist_num(0)),
    };
    let is_violation = hist_binary(
        bynk_ts::TsBinaryOp::And,
        hist_binary(
            bynk_ts::TsBinaryOp::StrictEq,
            bynk_ts::TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Typeof,
                expr: Box::new(first()),
            },
            hist_str("string"),
        ),
        hist_call(
            hist_member(first(), "startsWith"),
            vec![hist_str("InvariantViolation")],
        ),
    );
    bynk_ts::TsExpr::Arrow {
        params: vec![
            bynk_ts::TsParam::new(
                "__a",
                Some(bynk_ts::TsType::array(bynk_ts::TsType::named("unknown"))),
            )
            .rest(),
        ],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Block(vec![
            bynk_ts::TsStmt::if_stmt(is_violation, bynk_ts::TsStmt::return_stmt(None, None), None),
            bynk_ts::TsStmt::expr_stmt(
                hist_call(
                    hist_id("__ce"),
                    vec![bynk_ts::TsExpr::Spread(Box::new(hist_id("__a")))],
                ),
                None,
            ),
        ])),
    }
}

/// v0.119 (ADR 0155): the TS expression for a driven handler argument. An
/// `Int`-based value (bare or refined/opaque) is coerced to `number` — the
/// property generator yields `bigint`, but handler bodies do `number` arithmetic
/// (the same rule the contract attacker follows; the refinement brand is
/// compile-time only, so `Number(...)` is a no-op at runtime). Everything else —
/// `String`/refined-`String`, `Bool` — passes through.
fn history_arg_expr(
    p: &Param,
    i: usize,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> bynk_ts::TsExpr {
    let arg = bynk_ts::TsExpr::Index {
        object: Box::new(hist_member(hist_id("__st"), "args")),
        index: Box::new(hist_num(i)),
    };
    if type_base_is_int(&p.type_ref, types) {
        hist_call(hist_member(hist_id("globalThis"), "Number"), vec![arg])
    } else {
        arg
    }
}

/// `switch (__st.h) { case 0: <body>; break; … }`, one case per handler.
fn history_switch(
    hs: &[&Handler],
    body: impl Fn(&Handler) -> Vec<bynk_ts::TsStmt>,
) -> bynk_ts::TsStmt {
    let cases = hs
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let mut stmts = body(h);
            stmts.push(bynk_ts::TsStmt::break_stmt(None));
            bynk_ts::TsSwitchCase {
                labels: vec![hist_num(i)],
                body: stmts,
                default_braced: false,
                case_braced: false,
                span: None,
            }
        })
        .collect();
    bynk_ts::TsStmt::switch_stmt(hist_member(hist_id("__st"), "h"), cases, None)
}

/// The driver's loop over the generated sequence: run the call, then record the step.
fn history_drive_loop(
    hs: &[&Handler],
    step_ty: &bynk_ts::TsType,
    commons: &TypedCommons,
) -> bynk_ts::TsStmt {
    let run = history_switch(hs, |h| {
        let m = &h.method_name.as_ref().unwrap().name;
        let mut args: Vec<bynk_ts::TsExpr> = h
            .params
            .iter()
            .enumerate()
            .map(|(j, p)| history_arg_expr(p, j, &commons.types))
            .collect();
        args.push(hist_id("deps"));
        vec![bynk_ts::TsStmt::expr_stmt(
            hist_await(hist_call(hist_member(hist_id("__inst"), m), args)),
            None,
        )]
    });
    // An invariant/`transition` refusal throws `invariantViolation`: the step is not accepted,
    // and the refusal is the runner's own to read.
    let rejected = bynk_ts::TsStmt::try_catch(
        bynk_ts::TsStmt::block(vec![run], None),
        Some("__e"),
        bynk_ts::TsStmt::block(
            vec![bynk_ts::TsStmt::if_else_stmt(
                hist_call(hist_id("__rej"), vec![hist_id("__e")]),
                bynk_ts::TsStmt::block(
                    vec![
                        bynk_ts::TsStmt::assign(
                            hist_id("__accepted"),
                            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(false)),
                            None,
                        ),
                        bynk_ts::TsStmt::assign(
                            hist_id("__refusal"),
                            history_violation_of("__e"),
                            None,
                        ),
                    ],
                    None,
                ),
                bynk_ts::TsStmt::block(
                    vec![bynk_ts::TsStmt::throw_stmt(hist_id("__e"), None)],
                    None,
                ),
                None,
            )],
            None,
        ),
        None,
    );
    // P7.2: deferred, same reason as `step_ty`'s own `call` field — a real type here is the
    // same per-handler discriminated union.
    let record = history_switch(hs, |h| {
        let tag = history_variant_tag(&h.method_name.as_ref().unwrap().name);
        let mut entries = vec![bynk_ts::TsObjectEntry::prop(
            "tag".to_string(),
            hist_str(&tag),
        )];
        for (j, p) in h.params.iter().enumerate() {
            entries.push(bynk_ts::TsObjectEntry::prop(
                ts_ident(&p.name.name),
                history_arg_expr(p, j, &commons.types),
            ));
        }
        vec![bynk_ts::TsStmt::assign(
            hist_id("__call"),
            bynk_ts::TsExpr::object_entries(entries),
            None,
        )]
    });
    let new_state = bynk_ts::TsExpr::Conditional {
        test: Box::new(hist_id("__accepted")),
        consequent: Box::new(hist_await(hist_call(hist_id("__load"), vec![]))),
        alternate: Box::new(hist_id("__old")),
    };
    let step = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__step".to_string()),
        Some(step_ty.clone()),
        bynk_ts::TsExpr::object(
            [
                ("call", "__call"),
                ("accepted", "__accepted"),
                ("old", "__old"),
                ("new", "__new"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), hist_id(v)))
            .collect(),
        ),
        None,
    );
    // The refusal is the runner's, not part of the step's Bynk type, so it is
    // defined non-enumerable: `==` and a rendered step never see it.
    let define_refusal = bynk_ts::TsStmt::if_stmt(
        hist_binary(
            bynk_ts::TsBinaryOp::StrictNotEq,
            hist_id("__refusal"),
            hist_id("undefined"),
        ),
        bynk_ts::TsStmt::expr_stmt(
            hist_call(
                hist_member(
                    hist_member(hist_id("globalThis"), "Object"),
                    "defineProperty",
                ),
                vec![
                    hist_id("__step"),
                    hist_str("refusal"),
                    bynk_ts::TsExpr::object(vec![("value".to_string(), hist_id("__refusal"))]),
                ],
            ),
            None,
        ),
        None,
    );
    bynk_ts::TsStmt::for_of(
        "__st",
        hist_id("seq"),
        bynk_ts::TsStmt::block(
            vec![
                hist_const("__old", hist_await(hist_call(hist_id("__load"), vec![]))),
                hist_let(
                    "__accepted",
                    None,
                    Some(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true))),
                ),
                hist_let("__refusal", Some(bynk_ts::TsType::named("unknown")), None),
                rejected,
                hist_const("__new", new_state),
                hist_let("__call", Some(history_any_ty()), None),
                record,
                step,
                define_refusal,
                bynk_ts::TsStmt::expr_stmt(
                    hist_call(
                        hist_member(hist_id("__steps"), "push"),
                        vec![hist_id("__step")],
                    ),
                    None,
                ),
            ],
            None,
        ),
        None,
    )
}

/// The names of an agent's state registry and fresh-state factory.
struct AgentStateNames {
    /// The per-agent state registry (`__agentStates…`).
    registry: String,
    /// The fresh-state factory (`__zeroOf<Agent>State`).
    zero_fn: String,
    /// The description of the agent's collection fields the per-entry storage
    /// codec reads (`__entryFieldsOf<Agent>`, #3189).
    fields_const: String,
}

/// The agent's state record, as the TypeScript the rest of the agent reads: the
/// `<Agent>State` interface, the per-agent state registry, and the fresh-state
/// factory (#3167). Pushes those three declarations, in that order, onto `stmts`.
/// Returns the registry's and the factory's names, which `loadState`, the
/// rehydration step and the test reset refer to.
fn emit_agent_state_record(
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    stmts: &mut Vec<bynk_ts::TsStmt>,
) -> AgentStateNames {
    let tys = UnitFacts::of(ctx).render(commons);
    let state_ty = format!("{}State", a.name.name);
    let AgentStoreLayout {
        store_field_ty,
        effective_fields,
        held_maps,
        store_map_fields,
        store_map_indexes,
        set_field_names,
        store_cache_fields,
        store_log_fields,
        ..
    } = layout;
    // 1) State record type.
    //
    // Arc C, slice 19 (#1367): the first of step (9)'s own proposed 5-6
    // sub-slices (design/tracks/the-typescript-tree.md §6's own grounding
    // pass) — converts this block alone to a real `bynk_ts::TsDecl::Interface`
    // (already exists, gained `type_params`/per-member `readonly` by #1339,
    // no new algebra gap needed). Each field's own `readonly {name}: {ty};`
    // line is a real `TsTypeMember::Prop`; `ts_ty_to_ts_type` (the real-node
    // sibling of the plain-`String` `ts_ty` this block called before) already
    // exists and is already used by `emit_service`/`emit_make_surface` for
    // the identical purpose. Three compound member types, one load-bearing
    // trap among them: `Cache`'s `Record<string, { v: T; exp: number }>` and
    // `Log`'s `Array<{ t: number; v: T }>` are real `TsType::named_with_args`
    // over a real inline `TsType::Object` — no opaque text anywhere in this
    // block. The posting-list's own `Record<string, string[]>` is the third,
    // and it must use `TsType::Array { readonly: false, .. }` (postfix `[]`
    // syntax), NOT `named_with_args("Array", ..)` like `Cache`/`Log`
    // themselves use two blocks down — semantically identical, textually
    // distinct; using the generic form here would silently break zero-diff.
    // See its own construction below for the actual choice, not just this
    // note.
    let mut members: Vec<bynk_ts::TsTypeMember> = Vec::new();
    for f in effective_fields {
        let cell_ty = match store_field_ty.get(f.name.name.as_str()) {
            Some(StoreKindIr::Cell(t)) => *t,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Cell in this \
                 function's own state reader, found {other:?}",
                f.name.name
            ),
        };
        members.push(bynk_ts::TsTypeMember::Prop {
            name: f.name.name.clone(),
            ty: ts_ty_to_ts_type(cell_ty, tys),
            optional: false,
            readonly: true,
        });
    }
    for (name, _) in store_map_fields {
        let value_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Map(_, v)) => *v,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Map in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "__StoreMap",
                vec![ts_ty_to_ts_type(value_ty, tys)],
            ),
            optional: false,
            readonly: true,
        });
    }
    // v0.105 (slice 3b-ii): a held `Map[K, Connection]` persists `K → connId`.
    for (name, _) in held_maps {
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "__StoreMap",
                vec![bynk_ts::TsType::named("string")],
            ),
            optional: false,
            readonly: true,
        });
    }
    for name in set_field_names {
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.to_string(),
            ty: bynk_ts::TsType::named("__StoreSet"),
            optional: false,
            readonly: true,
        });
    }
    // v0.93 (ADR 0118): an `@indexed(by: f)` posting list lives inside the `Map`'s handle
    // (#3254), not in a sibling state field; the wire form still carries it.
    for (name, _, _) in store_cache_fields {
        let value_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Cache(_, v, _)) => *v,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Cache in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "__StoreCache",
                vec![ts_ty_to_ts_type(value_ty, tys)],
            ),
            optional: false,
            readonly: true,
        });
    }
    for (name, _, _) in store_log_fields {
        let elem_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Log(t, _)) => *t,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Log in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "__StoreLog",
                vec![ts_ty_to_ts_type(elem_ty, tys)],
            ),
            optional: false,
            readonly: true,
        });
    }
    // #1486: `no_blank_before` when this agent's own doc comment is the
    // preceding statement — see `emit_free_fn`'s own identical note. The
    // explicit `TsStmt::blank(None)` push this used to carry is gone —
    // `bynk_ts::print` itself now supplies the one blank line between
    // adjacent real statements.
    let mut state_interface = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Interface {
            name: state_ty.clone(),
            type_params: Vec::new(),
            members,
        })),
        None,
    );
    state_interface.no_blank_before = a.documentation.is_some();
    stmts.push(state_interface);
    // v0.9.2: per-agent state registry (bundle mode + `bynk test`) and the
    // zero-value factory used to initialise a fresh key's state.
    //
    // Arc C, slice 20 (#1369): the second of step (9)'s own proposed 5-6
    // sub-slices — the registry `const` and the zero-factory `function`
    // build real nodes; the zero-record's own per-field VALUES stay
    // opaque where they genuinely must (a `Cell` field with an initialiser
    // lowers through `LowerCtx`, `lower.rs`'s own permanently-excluded
    // general expression lowering — the same "carry an unconverted
    // sibling's already-formed JS text as `TsExpr::Ident`" pattern
    // #1355's own `emit_message_entry_renderer` call already established),
    // but every EMPTY-container field (`Map`/`Set`/`Cache`/held-map/
    // posting-list `{}`, `Log`'s `[]`) is fully real — `TsExpr::object(
    // vec![])`/`TsExpr::array(vec![])`, no opacity needed there at all.
    let registry = agent_registry_name(&a.name.name);
    let zero_fn = format!("__zeroOf{}State", a.name.name);
    let registry_const = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::ConstDecl {
            name: registry.clone(),
            ty: None,
            init: bynk_ts::TsExpr::New {
                callee: Box::new(bynk_ts::TsExpr::Ident("__StateRegistry".to_string())),
                args: Vec::new(),
            },
        },
        None,
    );
    stmts.push(registry_const);
    // v0.11: build the fresh-state record. A field with an explicit initialiser
    // lowers its (static) expression; a field without one uses the v0.9.2
    // implicit zero.
    let mut zero_entries: Vec<bynk_ts::TsObjectEntry> = Vec::new();
    // Review of #1370, finding 1: `lower_if`'s own non-`ternary_shaped`
    // branch and `build_match_iife` can both return a genuinely multi-line
    // IIFE as `val` (real embedded `\n` bytes) with `pre` empty, so the
    // `pre.is_empty()` branch above passes it straight through unwrapped —
    // no fixture in the 31-fixture `store` sweep reaches this shape (the
    // only store-init-with-`if` fixture, `1029_agent_static_init_hoist`,
    // takes the `hoist_if_as_statement` path instead, whose own hoisted
    // statements are individually newline-free), so it was invisible to the
    // zero-diff check. Tracked here so the zero-factory's own `inline` flag
    // can be set correctly below — `inline: true` is only safe when every
    // entry's own value is genuinely single-line (`render_compact_stmts`'s
    // own `debug_assert!` panics on an embedded newline, a real
    // debug-build crash for a `store` field initialised with a `match` or a
    // statement-shaped/`is`-binding `if`, not merely a formatting nit).
    let mut zero_record_has_multiline_value = false;
    for f in effective_fields {
        let val = if let Some(init) = &f.init {
            let mut pre = Pre::new();
            let mut module = ModuleCtx::for_unit(commons, ctx);
            module.target = ctx.target;
            module.agent_method_givens = ctx.agent_method_givens.clone();
            module.agent_called_contexts = ctx.agent_called_contexts.clone();
            module.event_schema_versions = ctx.event_schema_versions.clone();
            module.consumed_type_ns = consumed_type_ns(ctx);
            module.consumed_type_declared = ctx.consumed_type_aliases.clone();
            module.consumed_context_types = ctx.consumed_context_types();
            let mut icx = LowerCtx::new(module, BodyMode::StaticInit);
            icx.local_agents = ctx.local_agents.clone();
            let expr = pre.lower(init, &mut icx);
            // A static initialiser lowers to a pure expression (no setup
            // statements). #1029 review: if any appear, wrap them in an IIFE
            // rather than splicing them into a comma sequence. The comma
            // form only ever worked for expression-shaped hoists — a `const`
            // or an `if` operand does not parse — and T2.1 made a
            // statement-shaped hoist reachable here for the first time, since
            // a value-position `if` that hoists now yields `let …; if (…) {…}`
            // where it used to yield a self-contained arrow. An IIFE is
            // sound for this position specifically: a static initialiser has
            // no enclosing function to `return` out of, so the arrow cannot
            // swallow a control transfer the way it would in a handler body.
            if pre.is_empty() {
                expr
            } else {
                format!("(() => {{ {} return {expr}; }})()", pre.stmts().join(" "))
            }
        } else {
            zero_value_ts(&f.type_ref, f.refinement.as_ref(), &commons.types)
                .unwrap_or_else(|| "undefined as never".to_string())
        };
        if val.contains('\n') {
            zero_record_has_multiline_value = true;
        }
        zero_entries.push(bynk_ts::TsObjectEntry::prop(
            f.name.name.clone(),
            bynk_ts::TsExpr::Ident(val),
        ));
    }
    // A fresh `store Map`/`Set`/`Cache`/`Log` is an empty handle (#3254): insertion-ordered
    // whatever its keys. A `Map` carries the indexers of its `@indexed(by:)` fields, so its
    // handle keeps the posting lists itself (v0.93, ADR 0118).
    for (name, _) in store_map_fields {
        let value_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Map(_, v)) => *v,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Map in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        let v_ts = bynk_ts::print_type(&ts_ty_to_ts_type(value_ty, tys));
        let indexers = store_map_indexes
            .get(&name.name)
            .map(|fs| {
                let mut fs: Vec<&String> = fs.iter().collect();
                fs.sort();
                fs.into_iter()
                    .map(|f| format!("{{ index: \"{f}\", of: (__v) => String(__v.{f}) }}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let args = if indexers.is_empty() {
            String::new()
        } else {
            format!("[{indexers}]")
        };
        zero_entries.push(bynk_ts::TsObjectEntry::prop(
            name.name.clone(),
            bynk_ts::TsExpr::Ident(format!("new __StoreMap<{v_ts}>({args})")),
        ));
    }
    // A fresh held `Map[K, Connection]` (a `K → connId` handle) is empty too.
    for (name, _) in held_maps {
        zero_entries.push(bynk_ts::TsObjectEntry::prop(
            name.name.clone(),
            bynk_ts::TsExpr::Ident("new __StoreMap<string>()".to_string()),
        ));
    }
    for name in set_field_names {
        zero_entries.push(bynk_ts::TsObjectEntry::prop(
            name.to_string(),
            bynk_ts::TsExpr::Ident("new __StoreSet()".to_string()),
        ));
    }
    for (name, _, _) in store_cache_fields {
        let value_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Cache(_, v, _)) => *v,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Cache in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        let v_ts = bynk_ts::print_type(&ts_ty_to_ts_type(value_ty, tys));
        zero_entries.push(bynk_ts::TsObjectEntry::prop(
            name.name.clone(),
            bynk_ts::TsExpr::Ident(format!("new __StoreCache<{v_ts}>()")),
        ));
    }
    for (name, _, _) in store_log_fields {
        let elem_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Log(t, _)) => *t,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Log in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        let e_ts = bynk_ts::print_type(&ts_ty_to_ts_type(elem_ty, tys));
        zero_entries.push(bynk_ts::TsObjectEntry::prop(
            name.name.clone(),
            bynk_ts::TsExpr::Ident(format!("new __StoreLog<{e_ts}>()")),
        ));
    }
    // #1486: `no_blank_before: true`, unconditional — no blank line wanted
    // between `registry_const` and this function, matching the
    // pre-conversion hand-written text this crate's own zero-diff
    // discipline requires preserving exactly (see `TsStmt::no_blank_
    // before`'s own doc, added specifically for this real, already-shipped
    // pairing). The explicit `TsStmt::blank(None)` push this used to carry
    // afterward is also gone — `bynk_ts::print` itself now supplies the one
    // blank line before whatever follows.
    let mut zero_fn_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Function {
            name: zero_fn.clone(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: Some(bynk_ts::TsType::named(state_ty.clone())),
            body: vec![bynk_ts::TsStmt::return_stmt(
                Some(bynk_ts::TsExpr::object_entries(zero_entries)),
                None,
            )],
            is_async: false,
            // Single-line only when safe (see the `zero_record_has_multiline_
            // value` note above) — a genuinely multi-line `Cell` initialiser
            // falls back to the ordinary multi-line block form instead of
            // tripping `render_compact_stmts`'s own debug_assert!.
            inline: !zero_record_has_multiline_value,
        },
        None,
    );
    zero_fn_decl.no_blank_before = true;
    stmts.push(zero_fn_decl);
    // Per-entry storage (#3189): which fields are collections, and which index
    // posting records belong to which `Map`. The runtime codec splits a state record
    // by this on commit and joins it back on load.
    let fields_const = format!("__entryFieldsOf{}", a.name.name);
    let mut spec: Vec<String> = Vec::new();
    // A held `Map[K, Connection]` keeps its connection ids inline in the state record:
    // its values are live connections, not stored data, so it is not a per-entry field.
    // (On the bundle target `held_maps` is empty, so ask the declaration.)
    let held_decls = &layout.all_held_maps;
    let held_names: HashSet<&str> = held_decls.iter().map(|(n, _)| n.name.as_str()).collect();
    for (name, _) in store_map_fields
        .iter()
        .filter(|(n, _)| !held_names.contains(n.name.as_str()))
    {
        let indexes: Vec<String> = store_map_indexes
            .get(&name.name)
            .map(|fs| {
                let mut fs: Vec<&String> = fs.iter().collect();
                fs.sort();
                fs.into_iter()
                    .map(|f| {
                        format!(
                            "{{ index: \"{f}\", recordField: \"{}__idx_{f}\" }}",
                            name.name
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if indexes.is_empty() {
            spec.push(format!("{{ name: \"{}\", kind: \"map\" }}", name.name));
        } else {
            spec.push(format!(
                "{{ name: \"{}\", kind: \"map\", indexes: [{}] }}",
                name.name,
                indexes.join(", ")
            ));
        }
    }
    for name in set_field_names {
        spec.push(format!("{{ name: \"{name}\", kind: \"set\" }}"));
    }
    for (name, _, _) in store_cache_fields {
        spec.push(format!("{{ name: \"{}\", kind: \"cache\" }}", name.name));
    }
    for (name, _, _) in store_log_fields {
        spec.push(format!("{{ name: \"{}\", kind: \"log\" }}", name.name));
    }
    stmts.push(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(fields_const.clone()),
        None,
        bynk_ts::TsExpr::VerbatimExpr(
            format!("[{}] as const", spec.join(", ")),
            bynk_ts::VerbatimOrigin::Emit,
        ),
        None,
    ));
    AgentStateNames {
        registry,
        zero_fn,
        fields_const,
    }
}

/// The Durable Object constructor: `state` alone, or with the Worker's `env` when the agent
/// rebuilds deps inside the object (#527), in which case the `private __env` field joins `fields`.
fn agent_constructor(
    needs_env_ctor: bool,
    fields: &mut Vec<bynk_ts::TsClassField>,
) -> bynk_ts::TsClassCtor {
    let this_assign = |prop: &str, value: &str| {
        bynk_ts::TsStmt::assign(
            bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
                property: prop.to_string(),
            },
            bynk_ts::TsExpr::Ident(value.to_string()),
            None,
        )
    };
    let state = bynk_ts::TsParam::new(
        "state",
        Some(bynk_ts::TsType::named("__DurableObjectState")),
    );
    if !needs_env_ctor {
        return bynk_ts::TsClassCtor {
            params: vec![state],
            body: vec![this_assign("state", "state")],
        };
    }
    fields.push(bynk_ts::TsClassField::new("__env", bynk_ts::TsType::named("unknown")).private());
    bynk_ts::TsClassCtor {
        params: vec![
            state,
            bynk_ts::TsParam::optional("env", Some(bynk_ts::TsType::named("unknown"))),
        ],
        body: vec![this_assign("state", "state"), this_assign("__env", "env")],
    }
}

pub(crate) fn emit_agent(
    a: &AgentDecl,
    state: &[StoreFieldIr],
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    source_text: &str,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    if let Some(doc) = a.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let layout = classify_agent_store(a, state, commons, ctx);
    let AgentStateNames {
        registry,
        zero_fn,
        fields_const,
    } = emit_agent_state_record(a, &layout, commons, ctx, &mut stmts);
    let AgentPersistFns {
        rehydrate_fn,
        encode_fn,
        migrate_fn,
        hydrate_fn,
        has_rehydrate,
        has_collections,
        has_encode,
        step_plans,
        agent_versions,
        lazy_fields,
    } = emit_agent_state_persistence(a, &layout, &zero_fn, commons, ctx, &mut stmts);
    let is_workers = layout.is_workers;
    // 2) Durable Object class: fields, constructor and every method are nodes, so the
    // printer alone records the source map (#1482 gave the carrier a map of its own).
    let reads_key = agent_reads_key(a);
    let mut fields = vec![bynk_ts::TsClassField::new(
        "state",
        bynk_ts::TsType::named("__DurableObjectState"),
    )];
    if reads_key {
        // #1818: set before any handler runs, by the bundle factory or the
        // workers `fetch` dispatch.
        fields.push(
            bynk_ts::TsClassField::new("__key", ts_type_ref_to_ts_type(&a.key_type, None))
                .definite(),
        );
    }
    // #527: an agent whose methods take `given` capabilities rebuilds those
    // deps *inside* the DO (providers cannot cross the JSON wire), and some
    // providers take the Worker `env` — workerd passes it as the DO
    // constructor's second argument. Bundle mode constructs with `state`
    // only and never uses the fetch path, so the parameter stays optional.
    let given_deps_expr = ctx.agent_given_deps.get(&a.name.name).cloned();
    // Events track, slice 0 (spine #936): the same wire problem #527 already
    // solved for capability providers hits `deps.__eventsDispatch` too — it is
    // a function, so it does not survive the DO's JSON wire either. An agent
    // whose own handler body emits directly (`body_emits_directly`'s
    // per-handler test, mirrored here at the whole-agent level) needs its
    // fetch dispatch to rebuild it from `env.EVENTS_FANOUT` exactly as a
    // `given` provider is rebuilt, so it takes the same env-carrying
    // constructor.
    let agent_uses_emit = a
        .handlers
        .iter()
        .any(|h| bynk_model::block_uses_emit(&h.body, &commons.callees));
    // #1960: likewise `deps.env` itself — its Service Bindings and DO
    // namespaces are dead plain objects once the caller's deps cross the
    // wire, so an agent whose handlers call a consumed context's service or
    // construct an agent reads them from its own `env` instead.
    let agent_reaches_env = ctx.target == BuildTarget::Workers
        && a.handlers
            .iter()
            .any(|h| bynk_model::block_reaches_worker_env(&h.body, &commons.callees));
    // #3109: likewise the `from websocket` bodies this agent's Durable Object
    // hosts (`on open` / `on message` / `on close`): they run here, so any
    // Service Binding or DO namespace they reach comes from this object's `env`.
    let ws_hosts_reach_env = ctx.target == BuildTarget::Workers
        && ws_open_hosts_for(&a.name.name, commons, &ctx.local_agents, &ctx.actors)
            .iter()
            .any(|h| h.reaches_env(commons));
    // The `/_bynk/agent/` RPC branch rebuilds on its own condition; the ws
    // hosts only widen the constructor (and the ws entry points).
    let rpc_rebuilds_env = given_deps_expr.is_some() || agent_uses_emit || agent_reaches_env;
    let needs_env_ctor = rpc_rebuilds_env || ws_hosts_reach_env;
    let constructor = agent_constructor(needs_env_ctor, &mut fields);
    let mut methods = Vec::new();
    emit_agent_load_commit_state(
        &mut methods,
        a,
        &layout.effective_fields,
        &AgentLoadFns {
            zero_fn: &zero_fn,
            fields_const: &fields_const,
            rehydrate_fn: &rehydrate_fn,
            encode_fn: &encode_fn,
            migrate_fn: &migrate_fn,
            hydrate_fn: &hydrate_fn,
            has_rehydrate,
            has_collections,
            has_encode,
            step_plans: &step_plans,
            agent_versions: &agent_versions,
            lazy_fields: lazy_fields.as_ref(),
        },
        commons,
        ctx,
        source_text,
    );
    emit_agent_handlers(&mut methods, a, &layout, commons, ctx);
    // v0.104 (real-time track slice 3b): the `from websocket` `on open` handlers
    // whose connection transfers to *this* agent are hosted in this Durable Object
    // (DECISION A) — the upgrade is authenticated at the edge then forwarded here,
    // where the socket is accepted and the body runs as a `this`-self-call.
    let mut ws_open_hosts: Vec<WsOpenHost<'_>> = if is_workers {
        ws_open_hosts_for(&a.name.name, commons, &ctx.local_agents, &ctx.actors)
    } else {
        Vec::new()
    };
    if reads_key {
        for host in &mut ws_open_hosts {
            host.agent_key = Some((
                agent_key_codec_name(&a.name.name),
                crate::emitter::ts_types::ts_type_ref(&a.key_type),
            ));
        }
    }
    for host in &mut ws_open_hosts {
        host.rebuild = WsDepsRebuild {
            given: given_deps_expr.clone(),
            uses_emit: agent_uses_emit,
            reaches_env: host.reaches_env(commons),
        };
    }
    for host in &ws_open_hosts {
        emit_ws_do_method(
            &mut methods,
            a,
            host,
            host.handler,
            &ws_open_do_method_name(host.service),
            commons,
            ctx,
        );
        if let Some(m) = host.message {
            emit_ws_do_method(
                &mut methods,
                a,
                host,
                m,
                &ws_message_do_method_name(host.service),
                commons,
                ctx,
            );
        }
        if let Some(c) = host.close {
            emit_ws_do_method(
                &mut methods,
                a,
                host,
                c,
                &ws_close_do_method_name(host.service),
                commons,
                ctx,
            );
        }
    }
    emit_agent_dispatch(
        &mut methods,
        a,
        &AgentDispatchFacts {
            reads_key,
            given_deps_expr: &given_deps_expr,
            rpc_rebuilds_env,
            agent_uses_emit,
            agent_reaches_env,
            agent_versions: &agent_versions,
        },
        &ws_open_hosts,
        commons,
        ctx,
    );
    // #2270: what follows the last method belongs to the agent, not to that method's
    // final body line.
    let class = bynk_ts::TsDecl::class(a.name.name.as_str(), fields, Some(constructor), methods)
        .ending_at(a.span);
    stmts.push(bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(class)),
        None,
    ));
    emit_agent_factory(a, ctx, reads_key, &registry, &mut stmts);
    emit_agent_history_driver(
        a,
        &AgentLoadFns {
            zero_fn: &zero_fn,
            fields_const: &fields_const,
            rehydrate_fn: &rehydrate_fn,
            encode_fn: &encode_fn,
            migrate_fn: &migrate_fn,
            hydrate_fn: &hydrate_fn,
            has_rehydrate,
            has_collections,
            has_encode,
            step_plans: &step_plans,
            agent_versions: &agent_versions,
            // The history driver reads and commits whole.
            lazy_fields: None,
        },
        &registry,
        commons,
        ctx,
        &mut stmts,
    );
    stmts
}

/// v0.104 (real-time track slice 3b): a `from websocket` `on open` handler hosted
/// in a Durable Object (DECISION A) — the service it belongs to, the handler, the
/// protocol's `out` frame type (the `Connection`'s parameter), and the Bearer
/// seam (the edge authenticates with it; the DO method threads the verified
/// identity through `deps`).
struct WsOpenHost<'a> {
    service: &'a str,
    handler: &'a Handler,
    // P6.52 (design/tracks/the-ir.md §6b): `TyId`, not `&'a TypeRef` — read
    // off `ProtocolIr::WebSocket` (`lower_protocol_ir_from_commons`) once at
    // `ws_open_hosts_for`'s own construction site, instead of every renderer
    // below re-resolving the raw `ServiceProtocol::WebSocket` frame types
    // itself.
    out_ty: TyId,
    // v0.106 (slice 3b-iii): the service's inbound `in` frame type and its optional
    // `on message`/`on close` handlers — run in this same DO (`webSocketMessage`/
    // `webSocketClose`), with identity + route args recovered from the socket
    // attachment the `on open` accept wrote.
    in_ty: TyId,
    message: Option<&'a Handler>,
    close: Option<&'a Handler>,
    seam: Option<bynk_check::actors::BearerSeam>,
    /// An actor-sum `by who: A | B`: the members in first-wins order. The edge
    /// forwards which one verified (`{ tag, identity? }`) and the DO rebuilds
    /// `deps.who` from it. Mutually exclusive with `seam`, which a sum never has.
    sum: Option<Vec<bynk_check::actors::SumMember>>,
    // #1818: for a hosting agent that reads its own key, the key codec's name
    // and the key's TS type. A WebSocket body runs through these entry points,
    // not the `/_bynk/agent/` dispatch, so each sets `this.__key` itself.
    agent_key: Option<(String, String)>,
    // #3045: the `given`/events deps every entry point rebuilds from the DO's `env`.
    rebuild: WsDepsRebuild,
}

impl WsOpenHost<'_> {
    /// True if the service has an inbound/close handler — then the `on open` accept
    /// writes the identity + route args into the socket attachment so a waking
    /// `webSocketMessage`/`webSocketClose` can recover them.
    fn has_inbound(&self) -> bool {
        self.message.is_some() || self.close.is_some()
    }

    /// #3109: whether any of the service's hosted lifecycle bodies reaches the
    /// Worker `env` (a consumed context's service or a local agent).
    fn reaches_env(&self, commons: &TypedCommons) -> bool {
        std::iter::once(self.handler)
            .chain(self.message)
            .chain(self.close)
            .any(|h| bynk_model::block_reaches_worker_env(&h.body, &commons.callees))
    }
}

/// The TS type of a resolved actor sum (`deps.who`): each member's tagged
/// shape, `{ tag: "User", identity: UserId }` or `{ tag: "Visitor" }`, joined
/// as a union. The HTTP wrapper builds this value and the WebSocket Durable
/// Object rebuilds it, so both read the one shape.
fn actor_sum_ts_union(members: &[bynk_check::actors::SumMember]) -> String {
    members
        .iter()
        .map(|m| match m.identity_type() {
            Some(id) => format!("{{ tag: \"{}\", identity: {id} }}", m.actor_name),
            None => format!("{{ tag: \"{}\" }}", m.actor_name),
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The DO method name a hosted `on open` lowers to (`__wsOpen_<Service>`). The
/// edge forwards the upgrade to `/_bynk/ws/open/<Service>`, which dispatches here.
fn ws_open_do_method_name(service: &str) -> String {
    format!("__wsOpen_{service}")
}

/// The DO method names a hosted `on message` / `on close` lower to.
fn ws_message_do_method_name(service: &str) -> String {
    format!("__wsMessage_{service}")
}
fn ws_close_do_method_name(service: &str) -> String {
    format!("__wsClose_{service}")
}

/// Collect the `on open` handlers in `commons` whose connection transfers to the
/// agent named `agent` — its statically-routable single-transfer target (D2). The
/// shape constraint (`bynk.ws.open_transfer_shape`) guarantees at most one such
/// transfer per handler, so the host DO is unambiguous.
fn ws_open_hosts_for<'a>(
    agent: &str,
    commons: &'a TypedCommons,
    local_agents: &HashSet<String>,
    actors: &HashMap<String, ActorDecl>,
) -> Vec<WsOpenHost<'a>> {
    let mut hosts = Vec::new();
    for item in &commons.commons.items {
        let CommonsItem::Service(s) = item else {
            continue;
        };
        let ProtocolIr::WebSocket { out_ty, in_ty } =
            lower_protocol_ir_from_commons(&s.protocol, commons)
        else {
            continue;
        };
        for h in &s.handlers {
            if !matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Open) {
                continue;
            }
            if let crate::emitter::websocket::WsOpenShape::One(t) =
                crate::emitter::websocket::analyse_open_shape(&h.body, local_agents)
                && t.agent == agent
            {
                hosts.push(WsOpenHost {
                    service: &s.name.name,
                    handler: h,
                    out_ty,
                    in_ty,
                    message: s
                        .handlers
                        .iter()
                        .find(|h| matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Message)),
                    close: s
                        .handlers
                        .iter()
                        .find(|h| matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Close)),
                    sum: bynk_check::actors::sum_members_for(h, actors),
                    seam: if h.by_clause.as_ref().is_some_and(|by| by.is_sum()) {
                        None
                    } else {
                        bynk_check::actors::bearer_seam_for(h, actors)
                    },
                    agent_key: None,
                    rebuild: WsDepsRebuild::default(),
                });
            }
        }
    }
    hosts
}

/// `typeof message === "string" ? message : new globalThis.TextDecoder().decode(message)`: an
/// incoming frame as text, whichever form the platform delivered.
fn ws_frame_text_expr() -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Conditional {
        test: Box::new(bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::StrictEq,
            left: Box::new(bynk_ts::TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Typeof,
                expr: Box::new(bynk_ts::TsExpr::Ident("message".to_string())),
            }),
            right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                "string".to_string(),
            ))),
        }),
        consequent: Box::new(bynk_ts::TsExpr::Ident("message".to_string())),
        alternate: Box::new(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::New {
                    callee: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("globalThis".to_string())),
                        property: "TextDecoder".to_string(),
                    }),
                    args: Vec::new(),
                }),
                property: "decode".to_string(),
            }),
            args: vec![bynk_ts::TsExpr::Ident("message".to_string())],
        }),
    }
}

/// `globalThis.JSON.parse(__raw) as __JsonValue`: the frame text parsed, before the codec decodes it.
fn ws_frame_json_expr() -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::As {
        expr: Box::new(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("globalThis".to_string())),
                    property: "JSON".to_string(),
                }),
                property: "parse".to_string(),
            }),
            args: vec![bynk_ts::TsExpr::Ident("__raw".to_string())],
        }),
        ty: bynk_ts::TsType::named("__JsonValue"),
    }
}

/// `void code; void reason; void wasClean;`: the close handler's parameters, marked used.
fn ws_close_unused_params() -> Vec<bynk_ts::TsStmt> {
    ["code", "reason", "wasClean"]
        .into_iter()
        .map(|name| {
            bynk_ts::TsStmt::expr_stmt(
                bynk_ts::TsExpr::Unary {
                    op: bynk_ts::TsUnaryOp::Void,
                    expr: Box::new(bynk_ts::TsExpr::Ident(name.to_string())),
                },
                None,
            )
        })
        .collect()
}

/// `try { <target> = <value>; } catch { ws.close(1003, "<reason>"); return; }`:
/// one step of decoding an incoming WebSocket frame, closing the socket and
/// returning when it fails.
fn ws_decode_attempt(target: &str, value: bynk_ts::TsExpr, reason: &str) -> bynk_ts::TsStmt {
    let close = bynk_ts::TsStmt::expr_stmt(
        bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("ws".to_string())),
                property: "close".to_string(),
            }),
            args: vec![
                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("1003".to_string())),
                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(reason.to_string())),
            ],
        },
        None,
    );
    bynk_ts::TsStmt::try_catch(
        bynk_ts::TsStmt::inline_block(
            vec![bynk_ts::TsStmt::assign(
                bynk_ts::TsExpr::Ident(target.to_string()),
                value,
                None,
            )],
            None,
        ),
        None::<String>,
        bynk_ts::TsStmt::inline_block(vec![close, bynk_ts::TsStmt::return_stmt(None, None)], None),
        None,
    )
}

/// Emit the DO method that runs a hosted `from websocket` lifecycle body (`on
/// open`/`on message`/`on close`). The synthetic `connection` arrives as the first
/// parameter (the fresh socket for `on open`, the firing socket for `on message`/
/// `on close`), the handler's own parameters follow (for `on message`, the decoded
/// frame and any route values), and the verified identity rides in `deps`. The
/// body lowers with `ws_self_agent` set, so an agent transfer becomes a `this`
/// self-call rather than a cross-instance RPC.
#[allow(clippy::too_many_arguments)]
// Arc C, slice 25 (#1380): the first of step (9) sub-slice (5)'s own 3
// independent slices (the grounding pass post-#1378, `design/tracks/
// the-typescript-tree.md` §6), and the least risky by that pass's own
// finding — the identical shape `emit_provider`'s own already-landed ops
// (#1359) established: params/return-type real, the whole body one opaque
// `TsStmt::Raw`, a single-level offset (no nesting — this method wraps no
// events-IIFE, no implicit-commit closure, unlike `emit_service`/
// `emit_agent`'s own handler loop). `connection: Connection<T>`'s own
// generic type stays one opaque `TsType::named` string, the same "an
// odd, one-off shape stays opaque text" precedent `this.state.storage.
// get<T>` already set twice (#1371/#1373) — `Connection`'s own type
// argument is exactly this function's own real, single grounded need,
// not a general case for a new `TsType` type-argument mechanism. No
// `doc` — confirmed directly (review of #1379): this function has never
// emitted one, no `doc` parameter, no local, no `emit_doc_block` call.
fn emit_ws_do_method(
    methods: &mut Vec<bynk_ts::TsClassMethod>,
    agent: &AgentDecl,
    host: &WsOpenHost<'_>,
    h: &Handler,
    method: &str,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) {
    let mut params = vec![bynk_ts::TsParam::new(
        "connection".to_string(),
        Some(bynk_ts::TsType::named(format!(
            "Connection<{}>",
            ts_ty(host.out_ty, UnitFacts::of(ctx).render(commons))
        ))),
    )];
    for p in &h.params {
        params.push(bynk_ts::TsParam::new(
            ts_ident(&p.name.name),
            Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
        ));
    }
    let mut module = ModuleCtx::for_unit(commons, ctx);
    module.note_unit(ctx);
    module.agent_method_givens = ctx.agent_method_givens.clone();
    module.agent_called_contexts = ctx.agent_called_contexts.clone();
    module.event_schema_versions = ctx.event_schema_versions.clone();
    module.consumed_type_ns = consumed_type_ns(ctx);
    module.consumed_type_declared = ctx.consumed_type_aliases.clone();
    module.consumed_context_types = ctx.consumed_context_types();
    module.target = ctx.target;
    let mut cx = LowerCtx::new(
        module,
        BodyMode::WsDoMethod {
            handler: HandlerShared {
                capabilities: bynk_model::lower_handler_given_ir(h)
                    .into_iter()
                    .map(|c| c.name)
                    .collect::<HashSet<_>>(),
                // #934: the hosting agent + lifecycle method name
                // (`open`/`message`/`close`).
                handler_scope: Some(format!(
                    "{}.{}.{}",
                    ctx.commons_name, agent.name.name, method
                )),
                owning_context: ctx.commons_name.clone(),
                ..HandlerShared::default()
            },
            // Each lifecycle handler names its own binder: `on message`/`on close`
            // carry their own `by`, and nothing requires the name to match
            // `on open`'s. The principal itself comes from the `on open`
            // verification (the attachment), whichever name a handler gives it.
            deps_identity_binder: host.seam.as_ref().and_then(|_| {
                h.by_clause
                    .as_ref()
                    .and_then(|by| by.binder.as_ref())
                    .map(|b| b.name.clone())
            }),
            actor_sum_binder: host
                .sum
                .as_ref()
                .and(h.by_clause.as_ref())
                .filter(|by| by.is_sum())
                .and_then(|by| by.binder.as_ref())
                .map(|b| b.name.clone()),
            ws_self_agent: Some(agent.name.name.clone()),
        },
    );
    cx.local_agents = ctx.local_agents.clone();
    // #1653: parameters are bindings of the body's own scope, so a `let` that
    // re-binds one gets a fresh name instead of redeclaring it.
    for p in &h.params {
        cx.declare_binder(&p.name.name);
    }
    let async_tail = is_effectful_return(&h.return_type);
    let body = function_body_stmts(
        &h.body,
        &mut cx,
        INDENT_STEP * 2,
        async_tail,
        Some(&h.return_type),
    );
    let mut deps_members = build_deps_object_ty_with_surface(
        &effective_given(&bynk_model::lower_handler_given_ir(h), &cx),
        &cx,
        &ctx.cross_context,
        ctx.target,
    );
    if let Some(seam) = host.seam.as_ref().filter(|s| s.binder.is_some()) {
        append_deps_field(
            &mut deps_members,
            "identity",
            bynk_ts::TsType::named(seam.identity_type.clone()),
        );
    }
    if let Some(members) = &host.sum {
        append_deps_field(
            &mut deps_members,
            "who",
            bynk_ts::TsType::named(actor_sum_ts_union(members)),
        );
    }
    // #3045: the body hands its `deps` on to an agent handler that emits (or emits
    // itself), so `deps` must carry `__eventsDispatch` as an HTTP handler's does.
    // The entry points above spread `__eventsDeps` into it from the DO's `env`.
    if cx.is_first_party(FirstPartyCap::Events)
        && (bynk_model::block_uses_emit(&h.body, &commons.callees)
            || cx
                .agent_given_caps_used()
                .is_some_and(|m| m.contains_key("Events")))
    {
        append_deps_field(
            &mut deps_members,
            "__eventsDispatch",
            bynk_ts::TsType::named(format!(
                "(events: globalThis.Array<{}>) => globalThis.Promise<void>",
                crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE
            )),
        );
    }
    params.push(bynk_ts::TsParam::new(
        "deps".to_string(),
        Some(bynk_ts::TsType::Object(deps_members)),
    ));
    // #2280: anchored to the lifecycle handler's own `on open`/`on message`/
    // `on close` line, not to the previous method's last body line.
    methods.push(bynk_ts::TsClassMethod {
        span: Some(h.span),
        ..bynk_ts::TsClassMethod::new(
            method,
            async_tail,
            params,
            Some(ts_type_ref_to_ts_type(&h.return_type, None)),
            body,
        )
    });
}

/// The `fetch` branches ahead of the method dispatch that need the agent's schema
/// versions: the migration sweep's wake (S4, #2259), and in a `bynk dev` build
/// (#3581) the inspect read.
fn wake_and_inspect_branches(
    a: &AgentDecl,
    agent_versions: &Option<bynk_check::schema_registry::AgentVersions>,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    let mut branches = Vec::new();
    if let Some(v) = agent_versions {
        branches.push(migrate_wake_branch_stmt(v.version));
    }
    if ctx.inspect_agents {
        branches.push(inspect_branch_stmt(a, agent_versions));
    }
    branches
}

/// #3581: the `/_bynk/inspect` branch of an agent class's `fetch` in a `bynk dev`
/// build: `return __inspectAgent(this.state, <collection fields>, <versions>, url)`.
/// It reads the stored record and writes nothing, and is reachable only through
/// the Worker's signed `/_bynk/inspect/` route. A class with no schema lock has no
/// version to hold the record to, so `current` is `null` and the check is skipped.
fn inspect_branch_stmt(
    a: &AgentDecl,
    agent_versions: &Option<bynk_check::schema_registry::AgentVersions>,
) -> bynk_ts::TsStmt {
    let num = |n: i64| bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(n.to_string()));
    let (current, oldest, steps) = match agent_versions {
        Some(v) => (
            num(v.version),
            num(v.oldest_readable),
            step_ranges_expr(&v.steps),
        ),
        None => (
            bynk_ts::TsExpr::Ident("null".to_string()),
            num(1),
            step_ranges_expr(&[]),
        ),
    };
    let info = bynk_ts::TsExpr::object(vec![
        (
            "agent".to_string(),
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(a.name.name.clone())),
        ),
        ("current".to_string(), current),
        ("oldestReadable".to_string(), oldest),
        ("steps".to_string(), steps),
    ]);
    bynk_ts::TsStmt::if_stmt(
        bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::StrictEq,
            left: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("url".to_string())),
                property: "pathname".to_string(),
            }),
            right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                "/_bynk/inspect".to_string(),
            ))),
        },
        bynk_ts::TsStmt::return_stmt(
            Some(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__inspectAgent".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Ident("this.state".to_string()),
                    bynk_ts::TsExpr::Ident(format!("__entryFieldsOf{}", a.name.name)),
                    info,
                    bynk_ts::TsExpr::Ident("url".to_string()),
                ],
            }),
            None,
        ),
        None,
    )
}

/// Build the `fetch` branch that completes a forwarded WebSocket upgrade for a
/// hosted `on open`. The edge has already verified the actor, so the body runs
/// authenticated; this accepts the socket, reconstructs the route arguments and
/// identity from the trusted internal header, runs the on-open body, and returns
/// the `101` handing the client end back.
///
/// Arc C, slice 27 (#1384): a real `TsStmt::If` node (a `Block` then-branch),
/// composed directly into `fetch`'s own body — the last consumer of this
/// `&mut String`-writing shape converted, matching #1382's own "boilerplate
/// converts fully" posture. `__payload.args[i] as T`/the deps-object text/
/// the generic `acceptHibernatableConnection<T>` callee stay opaque
/// `TsExpr::Ident` leaves, the same established escape hatch #1382 used —
/// `__payload.identity` itself is a real `TsExpr::Member`, not opaque
/// (review of #1385, finding 2: this doc previously listed it as one).
/// State-migrations S4 (#2259 Decision D): `if (url.pathname ===
/// "/_bynk/migrate") return await __wakeForMigration(await
/// this.state.storage.get("state"), <version>, () => this.loadState(), (s) =>
/// this.commitState(s));`
fn migrate_wake_branch_stmt(version: i64) -> bynk_ts::TsStmt {
    let arrow = |params: Vec<bynk_ts::TsParam>, body: bynk_ts::TsExpr| bynk_ts::TsExpr::Arrow {
        params,
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(body))),
    };
    let this_call = |method: &str, args: Vec<bynk_ts::TsExpr>| bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
            property: method.to_string(),
        }),
        args,
    };
    let wake = bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Ident("__wakeForMigration".to_string())),
        args: vec![
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("this.state.storage.get".to_string())),
                args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    "state".to_string(),
                ))],
            })),
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(version.to_string())),
            arrow(Vec::new(), this_call("loadState", Vec::new())),
            arrow(
                vec![bynk_ts::TsParam::new("s".to_string(), None)],
                this_call("commitState", vec![bynk_ts::TsExpr::Ident("s".to_string())]),
            ),
        ],
    }));
    bynk_ts::TsStmt::if_stmt(
        bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::StrictEq,
            left: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("url".to_string())),
                property: "pathname".to_string(),
            }),
            right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                "/_bynk/migrate".to_string(),
            ))),
        },
        bynk_ts::TsStmt::return_stmt(Some(wake), None),
        None,
    )
}

/// `const __eventsDeps = { __eventsDispatch: (events) => __dispatchToEventsFanout(env.EVENTS_FANOUT, events) };`
/// — the fan-out callback is a function, so it never survives the DO wire and is
/// rebuilt from the DO's own `env` (which must already be in scope).
fn events_deps_const_stmt() -> bynk_ts::TsStmt {
    let bind = crate::emitter::wrangler::events_fanout_binding().name;
    // P7.2: `env` is now `Record<string, unknown>` (above), so this
    // specific binding needs its own cast to what
    // `dispatchToEventsFanout` actually declares.
    bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__eventsDeps".to_string()),
        None,
        bynk_ts::TsExpr::object(vec![(
            "__eventsDispatch".to_string(),
            bynk_ts::TsExpr::Arrow {
                params: vec![bynk_ts::TsParam::new(
                    "events".to_string(),
                    Some(bynk_ts::TsType::named_with_args(
                        "globalThis.Array",
                        vec![bynk_ts::TsType::named(
                            crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE,
                        )],
                    )),
                )],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(
                    bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Ident(
                            "__dispatchToEventsFanout".to_string(),
                        )),
                        args: vec![
                            bynk_ts::TsExpr::As {
                                expr: Box::new(bynk_ts::TsExpr::Member {
                                    object: Box::new(bynk_ts::TsExpr::Ident("env".to_string())),
                                    property: bind,
                                }),
                                ty: bynk_ts::TsType::named("__DurableObjectNamespace"),
                            },
                            bynk_ts::TsExpr::Ident("events".to_string()),
                        ],
                    },
                ))),
            },
        )]),
        None,
    )
}

/// `const __givenDeps = <expr>;` — the agent's `given` providers rebuilt in-process.
fn given_deps_const_stmt(expr: &str) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__givenDeps".to_string()),
        None,
        bynk_ts::TsExpr::Ident(expr.to_string()),
        None,
    )
}

/// The in-DO deps a WebSocket entry point (`on open`'s fetch branch,
/// `webSocketMessage`, `webSocketClose`) rebuilds from the DO's own `env`, as the
/// `/_bynk/agent/` branch does: the `given` providers and the events fan-out
/// callback. Neither survives the wire or the socket attachment. Returns the
/// statements (`env`, `__givenDeps`, `__eventsDeps`) and the spreads to append
/// last in the deps object so the rebuilt providers win. Spreads, unlike the
/// shorthand `env`, are exempt from excess-property checks, so the body's own
/// `deps` type need not name them.
#[derive(Clone, Default)]
struct WsDepsRebuild {
    given: Option<String>,
    uses_emit: bool,
    /// #3109: a body reaches `deps.env` (Service Bindings, DO namespaces).
    reaches_env: bool,
}

impl WsDepsRebuild {
    fn is_empty(&self) -> bool {
        self.given.is_none() && !self.uses_emit && !self.reaches_env
    }

    fn stmts(&self) -> Vec<bynk_ts::TsStmt> {
        if self.is_empty() {
            return Vec::new();
        }
        let mut stmts = vec![agent_env_const_stmt()];
        if let Some(expr) = &self.given {
            stmts.push(given_deps_const_stmt(expr));
        }
        if self.uses_emit {
            stmts.push(events_deps_const_stmt());
        }
        stmts
    }

    fn spreads(&self) -> Vec<bynk_ts::TsObjectEntry> {
        let mut v = Vec::new();
        if self.given.is_some() {
            v.push(bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(
                "__givenDeps".to_string(),
            )));
        }
        if self.uses_emit {
            v.push(bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(
                "__eventsDeps".to_string(),
            )));
        }
        if self.reaches_env {
            // A spread, not a shorthand `env`: exempt from excess-property
            // checks for a lifecycle body whose own deps type omits `env`. The
            // `never` cast satisfies the body's precise binding-shape type.
            v.push(bynk_ts::TsObjectEntry::Spread(
                bynk_ts::TsExpr::object_entries(vec![bynk_ts::TsObjectEntry::prop(
                    "env",
                    bynk_ts::TsExpr::As {
                        expr: Box::new(bynk_ts::TsExpr::Ident("env".to_string())),
                        ty: bynk_ts::TsType::named("never"),
                    },
                )]),
            ));
        }
        v
    }

    /// `{ <principal>, ...rebuilt }`, or `{}` when both are empty.
    fn deps_object(&self, principal: Option<bynk_ts::TsObjectEntry>) -> bynk_ts::TsExpr {
        let mut entries: Vec<bynk_ts::TsObjectEntry> = principal.into_iter().collect();
        entries.extend(self.spreads());
        bynk_ts::TsExpr::object_entries(entries)
    }
}

/// `const env = this.__env as unknown as Record<string, unknown>;` — the DO's own
/// `env`, which the `given` providers (and the events fan-out) are rebuilt from.
fn agent_env_const_stmt() -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("env".to_string()),
        None,
        bynk_ts::TsExpr::Ident(
            "this.__env as unknown as globalThis.Record<string, unknown>".to_string(),
        ),
        None,
    )
}

fn ws_open_fetch_branch_stmt(host: &WsOpenHost<'_>, tys: TyRender<'_>) -> bynk_ts::TsStmt {
    let h = host.handler;
    let path = format!("/_bynk/ws/open/{}", host.service);
    let method = ws_open_do_method_name(host.service);
    // The trusted internal header carries the route args, and the verified
    // identity only when the actor binds one (a binder-less `by` forwards none).
    let binds_identity = host.seam.as_ref().is_some_and(|s| s.binder.is_some());
    // An actor sum forwards the member that verified (`who`) in place of a bare
    // identity: the DO cannot tell which member it was from an identity alone.
    let id_field = if host.sum.is_some() {
        "; who: unknown"
    } else if binds_identity {
        "; identity: string"
    } else {
        ""
    };
    let key_field = if host.agent_key.is_some() {
        "; key: unknown"
    } else {
        ""
    };
    let payload_ty = format!("{{ args: unknown[]{id_field}{key_field} }}");
    let mut stmts = vec![
        bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__pair".to_string()),
            None,
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__newWebSocketPair".to_string())),
                args: Vec::new(),
            },
            None,
        ),
        bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__payload".to_string()),
            None,
            bynk_ts::TsExpr::As {
                expr: Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("globalThis.JSON".to_string())),
                        property: "parse".to_string(),
                    }),
                    args: vec![bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::NullishCoalescing,
                        left: Box::new(bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Member {
                                object: Box::new(bynk_ts::TsExpr::Member {
                                    object: Box::new(bynk_ts::TsExpr::Ident("request".to_string())),
                                    property: "headers".to_string(),
                                }),
                                property: "get".to_string(),
                            }),
                            args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                "X-Bynk-Ws-Open".to_string(),
                            ))],
                        }),
                        right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            "{}".to_string(),
                        ))),
                    }],
                }),
                ty: bynk_ts::TsType::named(payload_ty),
            },
            None,
        ),
    ];
    // v0.105 (slice 3b-ii): accept the server socket into the DO *hibernatably*
    // (tagged with a fresh connId, attached for wake-time recovery) so a stored
    // connection survives eviction — not `server.accept()`, which is in-memory only.
    // v0.106 (slice 3b-iii): when the service has an inbound/close handler, also
    // attach the sender identity + route args so a waking `webSocketMessage`/
    // `webSocketClose` recovers them without re-authenticating.
    let mut accept_args = vec![
        bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
            property: "state".to_string(),
        },
        bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("__pair".to_string())),
            property: "server".to_string(),
        },
    ];
    if host.has_inbound() {
        let identity = if binds_identity {
            bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("__payload".to_string())),
                property: "identity".to_string(),
            }
        } else {
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(String::new()))
        };
        let mut meta = vec![
            ("identity".to_string(), identity),
            (
                "args".to_string(),
                bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("__payload".to_string())),
                    property: "args".to_string(),
                },
            ),
        ];
        // An actor sum's resolved member rides in the attachment too, so a
        // waking `webSocketMessage`/`webSocketClose` rebuilds `deps.who`.
        if host.sum.is_some() {
            meta.push((
                "who".to_string(),
                bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("__payload".to_string())),
                    property: "who".to_string(),
                },
            ));
        }
        // #1818: a waking `webSocketMessage`/`webSocketClose` recovers the key
        // from the attachment, as it does the identity and route args.
        if host.agent_key.is_some() {
            meta.push((
                "key".to_string(),
                bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("__payload".to_string())),
                    property: "key".to_string(),
                },
            ));
        }
        accept_args.push(bynk_ts::TsExpr::object(meta));
    }
    // #1818: the edge forwards the agent's key with the upgrade.
    if let Some((codec, key_ts)) = &host.agent_key {
        stmts.push(agent_key_assign(codec, key_ts, "__payload.key"));
    }
    stmts.push(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("connection".to_string()),
        None,
        bynk_ts::TsExpr::Call {
            // Generic call, no `type_args` field on `Call` — same "an odd,
            // one-off shape stays opaque text" precedent as `loadState`'s own
            // `this.state.storage.get<T>` above; tagged `VerbatimExpr`, not
            // `Ident` (#1539).
            callee: Box::new(bynk_ts::TsExpr::VerbatimExpr(
                format!(
                    "__acceptHibernatableConnection<{}>",
                    ts_ty(host.out_ty, tys)
                ),
                bynk_ts::VerbatimOrigin::Emit,
            )),
            args: accept_args,
        },
        None,
    ));
    let mut call_args = vec![bynk_ts::TsExpr::Ident("connection".to_string())];
    for (i, p) in h.params.iter().enumerate() {
        // `__payload.args[i] as <type>` — an `Index`-under-`As` cast; tagged
        // `VerbatimExpr`, not `Ident` (#1539), the same "small and local"
        // boundary the rest of this slice draws rather than modeling every
        // remaining opaque shape as real nodes.
        call_args.push(bynk_ts::TsExpr::VerbatimExpr(
            format!("__payload.args[{i}] as {}", ts_type_ref(&p.type_ref)),
            bynk_ts::VerbatimOrigin::Emit,
        ));
    }
    // #3042: the agent handler an `on open` transfers to may declare `given`,
    // and `on open` hands it `deps` as they stand. Providers cannot cross the
    // wire, so rebuild them from this DO's own `env`, exactly as the
    // `/_bynk/agent/` branch does, and let them lead the deps object.
    stmts.extend(host.rebuild.stmts());
    let principal = if let Some(members) = &host.sum {
        Some(principal_entry(
            "who",
            "__payload",
            bynk_ts::TsType::named(actor_sum_ts_union(members)),
        ))
    } else {
        host.seam
            .as_ref()
            .filter(|s| s.binder.is_some())
            .map(|seam| {
                principal_entry(
                    "identity",
                    "__payload",
                    bynk_ts::TsType::named(seam.identity_type.clone()),
                )
            })
    };
    // Rebuilt providers last, as in the `/_bynk/agent/` branch: they win.
    let deps_arg = host.rebuild.deps_object(principal);
    call_args.push(deps_arg);
    let call_expr = bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
            property: method,
        }),
        args: call_args,
    };
    stmts.push(bynk_ts::TsStmt::expr_stmt(
        if is_effectful_return(&h.return_type) {
            bynk_ts::TsExpr::Await(Box::new(call_expr))
        } else {
            call_expr
        },
        None,
    ));
    stmts.push(bynk_ts::TsStmt::return_stmt(
        Some(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Ident(
                "__webSocketUpgradeResponse".to_string(),
            )),
            args: vec![bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("__pair".to_string())),
                property: "client".to_string(),
            }],
        }),
        None,
    ));
    bynk_ts::TsStmt::if_stmt(
        bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::StrictEq,
            left: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("url".to_string())),
                property: "pathname".to_string(),
            }),
            right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(path))),
        },
        bynk_ts::TsStmt::block(stmts, None),
        None,
    )
}

/// v0.106 (slice 3b-iii): the deps argument a hosted `on message`/`on close` body
/// receives — the verified identity recovered from the socket attachment, when the
/// actor binds one (else `{}`).
fn ws_attachment_deps_arg(host: &WsOpenHost<'_>) -> bynk_ts::TsExpr {
    let principal = if let Some(members) = &host.sum {
        Some(principal_entry(
            "who",
            "__att",
            bynk_ts::TsType::named(actor_sum_ts_union(members)),
        ))
    } else {
        host.seam
            .as_ref()
            .filter(|s| s.binder.is_some())
            .map(|seam| {
                principal_entry(
                    "identity",
                    "__att",
                    bynk_ts::TsType::named(seam.identity_type.clone()),
                )
            })
    };
    host.rebuild.deps_object(principal)
}

/// `<field>: <holder>.<field> as <ty>`: the verified principal a hosted body's
/// deps object leads with, read back from the upgrade payload or the socket
/// attachment.
fn principal_entry(field: &str, holder: &str, ty: bynk_ts::TsType) -> bynk_ts::TsObjectEntry {
    bynk_ts::TsObjectEntry::prop(
        field,
        bynk_ts::TsExpr::As {
            expr: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident(holder.to_string())),
                property: field.to_string(),
            }),
            ty,
        },
    )
}

/// v0.106 (slice 3b-iii): emit the hibernatable-WebSocket dispatch handlers
/// Cloudflare invokes on an accepted socket — `webSocketMessage` (an inbound frame)
/// and `webSocketClose`. Each recovers `{ connId, identity, args }` from the socket
/// attachment the `on open` accept wrote, re-wraps the firing socket as the
/// `connection`, and runs the corresponding DO-hosted body. `webSocketMessage`
/// decodes the raw frame against the service's `in:` type first — a malformed frame
/// closes the socket (`1003`/`1008`) and is never dispatched (the client-bytes trust
/// boundary).
///
/// Arc C, slice 26 (#1382): both methods convert to real
/// `bynk_ts::TsClassMethod` fragments, printed through `print_class_method`.
/// Unlike every prior Arc C slice, this function lowers no `.bynk` body at
/// all (it has no `LowerCtx`, no `body_smb`, no `source_map` parameter) — it
/// is pure dispatch plumbing, so there is nothing to merge back into a
/// module source map, and every statement below builds through real
/// `TsStmt`/`TsExpr` nodes rather than one opaque `Raw` blob, mirroring
/// `loadState`'s own (#1371) "pure boilerplate converts fully" precedent.
/// The two `try { ... } catch { ... }` lines and the `void`-suppression
/// line stay opaque `TsStmt::raw` text — see the proposal issue (#1382) for
/// why (`TryCatch`'s own renderer has no single-line form, and no
/// `TsUnaryOp::Void` exists for a single real call site).
fn emit_ws_dispatch_handlers(
    methods: &mut Vec<bynk_ts::TsClassMethod>,
    host: &WsOpenHost<'_>,
    runtime_use: &RuntimeUse,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: TyRender<'_>,
) {
    if !host.has_inbound() {
        return;
    }
    let out_ts = ts_ty(host.out_ty, tys);
    let att_who = if host.sum.is_some() {
        "; who: unknown"
    } else {
        ""
    };
    let att_ty = if host.agent_key.is_some() {
        format!("{{ connId: string; identity: string{att_who}; args: unknown[]; key: unknown }}")
    } else {
        format!("{{ connId: string; identity: string{att_who}; args: unknown[] }}")
    };
    // The firing socket's minimal structural surface (attachment + send/close), so
    // emitted code stays free of `@cloudflare/workers-types`.
    let ws_ty = "{ deserializeAttachment(): unknown; send(data: string): void; close(code?: number, reason?: string): void }";
    let deps_arg = ws_attachment_deps_arg(host);
    let att_or_null = bynk_ts::TsType::union(vec![
        bynk_ts::TsType::named(att_ty.clone()),
        bynk_ts::TsType::named("null"),
    ]);
    // `const __att = ws.deserializeAttachment() as {att_ty} | null;` / the
    // missing-session guard / `const connection = new WorkersConnection<...>`
    // — identical in both branches below (the pre-conversion `writeln!` code
    // duplicated this same text too), factored once so the two call sites
    // can't independently drift.
    let att_guard = |stmts: &mut Vec<bynk_ts::TsStmt>| {
        stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__att".to_string()),
            None,
            bynk_ts::TsExpr::As {
                expr: Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("ws".to_string())),
                        property: "deserializeAttachment".to_string(),
                    }),
                    args: Vec::new(),
                }),
                ty: att_or_null.clone(),
            },
            None,
        ));
        stmts.push(bynk_ts::TsStmt::if_stmt(
            bynk_ts::TsExpr::Binary {
                op: bynk_ts::TsBinaryOp::StrictEq,
                left: Box::new(bynk_ts::TsExpr::Ident("__att".to_string())),
                right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Null)),
            },
            bynk_ts::TsStmt::inline_block(
                vec![
                    bynk_ts::TsStmt::expr_stmt(
                        bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Member {
                                object: Box::new(bynk_ts::TsExpr::Ident("ws".to_string())),
                                property: "close".to_string(),
                            }),
                            args: vec![
                                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("1011".to_string())),
                                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str("no session".to_string())),
                            ],
                        },
                        None,
                    ),
                    bynk_ts::TsStmt::return_stmt(None, None),
                ],
                None,
            ),
            None,
        ));
        stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("connection".to_string()),
            None,
            bynk_ts::TsExpr::New {
                // Generic constructor, no `type_args` field on `New` — same
                // "an odd, one-off shape stays opaque text" precedent as the
                // generic-method-call sites above; tagged `VerbatimExpr`, not
                // `Ident` (#1539).
                callee: Box::new(bynk_ts::TsExpr::VerbatimExpr(
                    format!("__WorkersConnection<{out_ts}>"),
                    bynk_ts::VerbatimOrigin::Emit,
                )),
                args: vec![
                    bynk_ts::TsExpr::Ident("ws".to_string()),
                    bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("__att".to_string())),
                        property: "connId".to_string(),
                    },
                ],
            },
            None,
        ));
        // #1818: restore the key the `on open` attached.
        if let Some((codec, key_ts)) = &host.agent_key {
            stmts.push(agent_key_assign(codec, key_ts, "__att.key"));
        }
    };

    if let Some(m) = host.message {
        let method = ws_message_do_method_name(host.service);
        // P6.52 (design/tracks/the-ir.md §6b): `host.in_ty` round-trips back
        // to a `TypeRef` for `serialisation::deserialise_expr`, the excluded
        // codec renderer's own boundary (P6.33 ruled it phase 7, `TypeRef`-
        // driven by definition). `ty_to_type_ref` only returns `None` for a
        // function/effect/type-variable `Ty` — none of which a `from
        // websocket` service's own `in:` frame type can ever resolve to,
        // since `check_service_protocols` already constrains it to a
        // codec-eligible type before this function ever runs.
        let in_type = ty_to_type_ref(host.in_ty, tys.types()).unwrap_or_else(|| {
            panic!(
                "bynk internal error: a `from websocket` service's own `in:` frame type did \
                 not round-trip to a TypeRef, but check_service_protocols already certified it \
                 codec-eligible"
            )
        });
        let decode = serialisation::deserialise_expr(&in_type, "__json", "frame", runtime_use);
        let mut stmts = Vec::new();
        att_guard(&mut stmts);
        stmts.extend(host.rebuild.stmts());
        stmts.push(bynk_ts::TsStmt::let_stmt(
            bynk_ts::TsBindingName::Ident("__raw".to_string()),
            Some(bynk_ts::TsType::named("string")),
            None,
            None,
        ));
        stmts.push(ws_decode_attempt(
            "__raw",
            ws_frame_text_expr(),
            "unreadable frame",
        ));
        stmts.push(bynk_ts::TsStmt::let_stmt(
            bynk_ts::TsBindingName::Ident("__json".to_string()),
            Some(bynk_ts::TsType::named("__JsonValue")),
            None,
            None,
        ));
        stmts.push(ws_decode_attempt(
            "__json",
            ws_frame_json_expr(),
            "malformed frame",
        ));
        stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__dec".to_string()),
            None,
            decode,
            None,
        ));
        stmts.push(bynk_ts::TsStmt::if_stmt(
            bynk_ts::TsExpr::Binary {
                op: bynk_ts::TsBinaryOp::StrictEq,
                left: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("__dec".to_string())),
                    property: "tag".to_string(),
                }),
                right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str("Err".to_string()))),
            },
            bynk_ts::TsStmt::inline_block(
                vec![
                    bynk_ts::TsStmt::expr_stmt(
                        bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Member {
                                object: Box::new(bynk_ts::TsExpr::Ident("ws".to_string())),
                                property: "close".to_string(),
                            }),
                            args: vec![
                                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("1008".to_string())),
                                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                    "invalid frame".to_string(),
                                )),
                            ],
                        },
                        None,
                    ),
                    bynk_ts::TsStmt::return_stmt(None, None),
                ],
                None,
            ),
            None,
        ));
        // The decoded frame fills the param typed as the service `in`; the rest are
        // route values recovered (positionally) from the attachment args.
        let mut call_args = vec!["connection".to_string()];
        let mut route_idx = 0usize;
        // Resolved-`Ty` equality (not `type_refs_match`'s surface-syntax
        // comparison, which fell through to `_ => false` for a `List`/`Map`/
        // `Query`/… frame type — a false negative here emits every param as
        // a route value, silently wrong for a frame type that shape covers):
        // must agree with `check_service_protocols`'s identical resolved
        // check, or the checker's "exactly one frame param" could accept a
        // program this picks a different (or no) param's argument for.
        let no_vars = HashSet::new();
        let resolve_ty = |t: &TypeRef| {
            bynk_check::checker::resolve_type_ref_in(t, types, &no_vars, tys.types())
                .unwrap_or(tys.intern(bynk_check::checker::Ty::Unit))
        };
        for p in &m.params {
            if resolve_ty(&p.type_ref) == host.in_ty {
                call_args.push("__dec.value".to_string());
            } else {
                call_args.push(format!(
                    "__att.args[{route_idx}] as {}",
                    ts_type_ref(&p.type_ref)
                ));
                route_idx += 1;
            }
        }
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
                    property: method,
                }),
                args: call_args
                    .into_iter()
                    .map(bynk_ts::TsExpr::Ident)
                    .chain([deps_arg.clone()])
                    .collect(),
            })),
            None,
        ));
        let message_method = bynk_ts::TsClassMethod::new(
            "webSocketMessage",
            true,
            vec![
                bynk_ts::TsParam::new("ws".to_string(), Some(bynk_ts::TsType::named(ws_ty))),
                bynk_ts::TsParam::new(
                    "message".to_string(),
                    Some(bynk_ts::TsType::union(vec![
                        bynk_ts::TsType::named("string"),
                        bynk_ts::TsType::named("globalThis.ArrayBuffer"),
                    ])),
                ),
            ],
            Some(bynk_ts::TsType::named_with_args(
                "globalThis.Promise",
                vec![bynk_ts::TsType::named("void")],
            )),
            stmts,
        );
        methods.push(message_method);
    }

    if let Some(c) = host.close {
        let method = ws_close_do_method_name(host.service);
        let mut stmts = ws_close_unused_params();
        att_guard(&mut stmts);
        stmts.extend(host.rebuild.stmts());
        let mut call_args = vec!["connection".to_string()];
        for (i, p) in c.params.iter().enumerate() {
            call_args.push(format!("__att.args[{i}] as {}", ts_type_ref(&p.type_ref)));
        }
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
                    property: method,
                }),
                args: call_args
                    .into_iter()
                    .map(bynk_ts::TsExpr::Ident)
                    .chain([deps_arg])
                    .collect(),
            })),
            None,
        ));
        let close_method = bynk_ts::TsClassMethod::new(
            "webSocketClose",
            true,
            vec![
                bynk_ts::TsParam::new("ws".to_string(), Some(bynk_ts::TsType::named(ws_ty))),
                bynk_ts::TsParam::new("code".to_string(), Some(bynk_ts::TsType::named("number"))),
                bynk_ts::TsParam::new("reason".to_string(), Some(bynk_ts::TsType::named("string"))),
                bynk_ts::TsParam::new(
                    "wasClean".to_string(),
                    Some(bynk_ts::TsType::named("boolean")),
                ),
            ],
            Some(bynk_ts::TsType::named_with_args(
                "globalThis.Promise",
                vec![bynk_ts::TsType::named("void")],
            )),
            stmts,
        );
        methods.push(close_method);
    }
}

#[cfg(test)]
mod doc_block_tests {
    use super::INDENT_STEP;

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
        emit_doc_block(&mut out, Some("a method"), crate::emitter::INDENT_STEP);
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

#[cfg(test)]
mod messages_template_tests {
    use super::{TemplateSegment, placeholder_names, split_template};
    use std::collections::BTreeSet;

    fn kinds(template: &str) -> Vec<(&str, &str)> {
        split_template(template)
            .into_iter()
            .map(|s| match s {
                TemplateSegment::Literal(l) => ("lit", l),
                TemplateSegment::Placeholder { inner, .. } => ("ph", inner),
            })
            .collect()
    }

    #[test]
    fn literal_only_template_is_one_segment() {
        assert_eq!(kinds("Bye"), vec![("lit", "Bye")]);
    }

    #[test]
    fn empty_template_is_one_empty_literal_segment() {
        // emit_message_entry_renderer's `parts.is_empty()` guard exists
        // specifically because split_template never returns zero segments —
        // this pins that invariant.
        assert_eq!(kinds(""), vec![("lit", "")]);
    }

    #[test]
    fn single_placeholder_with_surrounding_literal_text() {
        assert_eq!(
            kinds("Hello, {name}!"),
            vec![("lit", "Hello, "), ("ph", "name"), ("lit", "!")],
        );
    }

    #[test]
    fn placeholder_at_the_very_start_or_end() {
        assert_eq!(kinds("{name}!"), vec![("ph", "name"), ("lit", "!")]);
        assert_eq!(kinds("Hi, {name}"), vec![("lit", "Hi, "), ("ph", "name")]);
    }

    #[test]
    fn back_to_back_placeholders() {
        assert_eq!(kinds("{a}{b}"), vec![("ph", "a"), ("ph", "b")],);
    }

    #[test]
    fn unmatched_brace_and_empty_braces_are_literal_text() {
        // No closing `}` at all.
        assert_eq!(kinds("a {b"), vec![("lit", "a {b")]);
        // Empty `{}` names nothing, so it's not a placeholder either.
        assert_eq!(kinds("a {} b"), vec![("lit", "a {} b")]);
    }

    #[test]
    fn multibyte_literal_text_around_a_placeholder() {
        // A non-ASCII literal must not panic the byte-index scan and must
        // round-trip intact (UTF-8 char-boundary safety, not just byte count).
        assert_eq!(
            kinds("caf\u{e9} {name} \u{1f980}"),
            vec![("lit", "caf\u{e9} "), ("ph", "name"), ("lit", " \u{1f980}")],
        );
    }

    // -- message-bundles slice 3 (#878): ICU-dispatch placeholders --

    #[test]
    fn icu_plural_placeholder_captures_full_inner_text_with_nested_braces() {
        assert_eq!(
            kinds("You have {n, plural, one {# item} other {# items}}."),
            vec![
                ("lit", "You have "),
                ("ph", "n, plural, one {# item} other {# items}"),
                ("lit", "."),
            ],
        );
    }

    #[test]
    fn icu_placeholder_and_plain_placeholder_side_by_side() {
        assert_eq!(
            kinds("{name}: {n, number}"),
            vec![("ph", "name"), ("lit", ": "), ("ph", "n, number")],
        );
    }

    #[test]
    fn icu_placeholder_with_no_true_close_falls_back_to_literal() {
        // A `,` precedes the naive first `}`, so this is ICU-dispatch — but
        // the arm's own `{` is never closed, so `find_icu_close` never
        // reaches depth 0. Same "just literal text" policy as an unmatched
        // plain `{`.
        assert_eq!(
            kinds("{n, plural, one {# item"),
            vec![("lit", "{n, plural, one {# item")],
        );
    }

    #[test]
    fn placeholder_names_trims_icu_names_but_not_plain_ones() {
        assert_eq!(placeholder_names("{ name }"), BTreeSet::from([" name "]));
        assert_eq!(
            placeholder_names("{ n , plural, one {#} other {#}}"),
            BTreeSet::from(["n"])
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
    use crate::testkit::{emit_bundle, emit_source};

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

    /// Exercises `emit_project`'s own Type/Event loops (Decision A/B, #1188) —
    /// a separate call site from `emit_source`'s single-file `emit()` path
    /// above, going through the full multi-file project pipeline instead.
    #[test]
    fn record_and_sum_types_emit_identically_through_the_project_pipeline() {
        let ts = emit_bundle(
            r#"
type Order = { id: String, total: Int }
type PaymentError = enum { Declined, InsufficientFunds }
"#,
        );
        assert!(
            ts.contains(
                "export interface Order {\n  readonly id: string;\n  readonly total: number;\n}\n"
            ),
            "{ts}"
        );
        assert!(
            ts.contains(
                "export type PaymentError =\n    { readonly tag: \"Declined\" }\n  | \
                 { readonly tag: \"InsufficientFunds\" };\n"
            ),
            "{ts}"
        );
    }

    /// Exercises `emit_project`'s **Event** mirror loop specifically (review
    /// on #1190) — the one call site whose correctness this slice's own
    /// `type_shape_for` leans on without a fallback: that `TypedCommons::types`
    /// already holds an entry keyed `e.name.name` for every `CommonsItem::
    /// Event`, the same table an ordinary declared type resolves through
    /// (Decision B, #1188's own doc comment on `type_shape_for`). The prior
    /// test above only reaches the Type loop; `emit_bundle` can't express an
    /// event at all (it wraps its body in `commons app.bundle`, and `event`
    /// outside a `context` is `bynk.event.outside_context`), so this drives
    /// `project::compile_in_memory` directly with a context source instead.
    #[test]
    fn event_type_emits_through_the_project_pipelines_event_mirror_loop() {
        let out = bynk_driver::project::compile_in_memory(
            "context demo {\n  event Notified = { id: String }\n}\n",
            bynk_check::project_model::BuildTarget::Bundle,
            Default::default(),
        )
        .unwrap_or_else(|_| panic!("event-in-context fixture should compile"));
        let ts = out
            .artefacts
            .docs
            .iter()
            .find(|(path, _)| path.to_string_lossy().contains("demo"))
            .map(|(_, doc)| doc.text())
            .unwrap_or_else(|| panic!("the demo context's own module should be in the output"));
        assert!(
            ts.contains("export interface Notified {\n  readonly id: string;\n}\n"),
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

/// Differential coverage for `emit_capability` reading `bynk-emit::ir::OpSig`
/// (built by `lower_capability_item_ir`/`lower_op_sig_ir`) instead of
/// walking `CapabilityOp::params`/`return_type` `TypeRef`s directly (P6.x,
/// #1193, slice 3 of #1187). `STORE_FIXTURE` covers a multi-op capability, a
/// named return/param type (`Order`), and — new relative to #1188's own
/// corpus — a generic op (`get[T]`), whose `Effect[Option[T]]` return type
/// exercises `ts_ty`'s `Ty::Var` arm the way no pre-existing capability
/// fixture's own emitted-TS assertions did. `capability` is a context-only
/// construct (`bynk.capability.outside_context`), so — like the Event mirror
/// test above — this drives `project::compile_in_memory` directly with a
/// context source rather than `emit_source`/`emit_bundle`.
#[cfg(test)]
mod capability_op_sig_emission_tests {
    const STORE_FIXTURE: &str = r#"
context demo {
  type Order = { id: String, total: Int }

  capability Store {
    fn get[T](key: String) -> Effect[Option[T]]
    fn put(key: String, value: Order) -> Effect[()]
    fn count() -> Effect[Int]
  }
}
"#;

    fn emit_store_context(src: &str) -> String {
        let out = bynk_driver::project::compile_in_memory(
            src,
            bynk_check::project_model::BuildTarget::Bundle,
            Default::default(),
        )
        .unwrap_or_else(|_| panic!("capability fixture should compile:\n{src}"));
        out.artefacts
            .docs
            .iter()
            .find(|(path, _)| path.to_string_lossy().contains("demo"))
            .map(|(_, doc)| doc.text())
            .unwrap_or_else(|| panic!("the demo context's own module should be in the output"))
    }

    /// A non-generic op's param/return types (`Order`, a named record type,
    /// and `Effect[()]`/`Effect[Int]`, both base-type-adjacent) render
    /// identically to `ts_type_ref`'s pre-cutover output.
    #[test]
    fn named_and_base_return_types_render_through_ts_ty() {
        let ts = emit_store_context(STORE_FIXTURE);
        assert!(
            ts.contains("put(key: string, value: Order): globalThis.Promise<void>;\n"),
            "{ts}"
        );
        assert!(
            ts.contains("count(): globalThis.Promise<number>;\n"),
            "{ts}"
        );
    }

    /// The generic op: `get[T]`'s own rigid type variable renders bare
    /// (`ts_ty`'s `Ty::Var` arm) inside the `Effect[Option[T]]` return type's
    /// `Option<…>` wrapper — `<T>` on the interface method itself still
    /// comes from `op.type_params` (`ts_type_params`), not `sig.type_params`
    /// (Decision A/B, #1193).
    #[test]
    fn generic_op_renders_its_rigid_type_var_and_type_param_list() {
        let ts = emit_store_context(STORE_FIXTURE);
        assert!(
            ts.contains("get<T>(key: string): globalThis.Promise<Option<T>>;\n"),
            "{ts}"
        );
    }

    #[test]
    fn interface_and_injection_token_frame_the_ops() {
        let ts = emit_store_context(STORE_FIXTURE);
        assert!(ts.contains("export interface Store {\n"), "{ts}");
        assert!(
            ts.contains("export const __StoreToken: symbol = globalThis.Symbol(\"Store\");\n"),
            "{ts}"
        );
    }
}

/// I3 (#1971): the TypeScript type of a `call` handler's resolved actor sum
/// (`deps.who`): `Caller` carries the calling context's name, a foreign
/// caller is its actor alone. Opaque text, as the HTTP sum's union is.
pub(crate) fn call_sum_ts_type(members: &[bynk_check::actors::CallSumMember]) -> bynk_ts::TsType {
    let union = members
        .iter()
        .map(|m| match m {
            bynk_check::actors::CallSumMember::Caller => {
                "{ tag: \"Caller\", identity: string }".to_string()
            }
            bynk_check::actors::CallSumMember::Foreign(f) => format!("{{ tag: \"{}\" }}", f.actor),
        })
        .collect::<Vec<_>>()
        .join(" | ");
    bynk_ts::TsType::named(union)
}

/// #3066: a suite's own `type` and `const` declarations, as TypeScript for the
/// body of one test runner: the types (an interface and its companion object, or
/// a union), their JSON codecs (so `select[T]`, `Json.encode`/`decode` and the
/// boundary paths find `__serialise_T` / `__deserialise_T` in scope), and the
/// constants (cached-function declarations, like a unit's). They are declared
/// local to the runner, so another suite declaring the same name cannot collide
/// and nothing here reaches the deployable. `typed` is the suite's view with the
/// suite's types and constants already merged in; `qual` qualifies a target
/// type a suite type's codec mentions, as the module-level codecs do.
///
/// Returns the printed statements with the `export` modifiers dropped (a
/// function-local declaration cannot carry one), unindented.
pub fn emit_suite_decls(
    types: &[TypeDecl],
    consts: &[ConstDecl],
    typed: &TypedCommons,
    qual: &std::collections::HashMap<String, String>,
    runtime_use: &RuntimeUse,
) -> String {
    let mut stmts: Vec<bynk_ts::TsStmt> = Vec::new();
    for t in types {
        let Some(def) = typed.types.get(&t.name.name) else {
            continue;
        };
        let shape = bynk_model::lower_suite_type_shape_ir(def, typed);
        stmts.extend(emit_type_in(
            t,
            &shape,
            typed,
            "",
            runtime_use,
            UnitFacts::detached(),
        ));
    }
    if !types.is_empty() {
        // The type itself plus the wrappers a test reads one through: a
        // `select[T]` is a `List[T]`, a `firstAs[T]` an `Option[T]`.
        let roots = suite_base_codec_roots(types);
        let (names, insts) = super::serialisation::collect_codec_closure(&roots, &typed.types);
        runtime_use.note_json_codec();
        runtime_use.note_boundary_codec();
        stmts.extend(super::serialisation::decls_as_stmts_block(
            super::serialisation::emit_helpers_for_owner_qualified(
                &names,
                &typed.types,
                "",
                qual,
                runtime_use,
            ),
        ));
        stmts.extend(super::serialisation::decls_as_stmts(
            super::serialisation::emit_generic_helpers_qualified(
                &insts,
                &typed.types,
                qual,
                runtime_use,
            ),
        ));
    }
    let empty = bynk_check::resolver::CrossContextInfo::default();
    for c in consts {
        stmts.extend(emit_const_in(
            c,
            ModuleCtx::new(typed, &empty, runtime_use, UnitFacts::detached()),
        ));
    }
    stmts
        .iter()
        .map(|s| bynk_ts::print_stmt(s, 0))
        .collect::<String>()
        .lines()
        .map(|l| format!("{}\n", l.strip_prefix("export ").unwrap_or(l)))
        .collect()
}

/// The roots a suite's local codecs always cover: each non-generic type plus the
/// `List`/`Option` wrappers a test reads one through (a `select[T]` is a
/// `List[T]`, a `firstAs[T]` an `Option[T]`).
fn suite_base_codec_roots(types: &[TypeDecl]) -> Vec<TypeRef> {
    let mut roots: Vec<TypeRef> = Vec::new();
    for t in types.iter().filter(|t| t.type_params.is_empty()) {
        let named = TypeRef::Named(t.name.clone());
        let span = t.name.span;
        roots.push(TypeRef::List(Box::new(named.clone()), span));
        roots.push(TypeRef::Option(Box::new(named.clone()), span));
        roots.push(named);
    }
    roots
}

/// #3111: the codecs a runner's body reached for beyond the always-present
/// ones ([`emit_suite_decls`]): a suite type under another generic (`Result`,
/// a user generic, nested containers) at a `Json` boundary. `reached` is what
/// the runner's lowering noted; only roots that name a suite type are served
/// here (the module-level closure serves the rest). Returns printed function
/// declarations, unindented, to be spliced ahead of the body; empty when the
/// body reached nothing new.
pub fn emit_suite_late_codecs(
    types: &[TypeDecl],
    typed: &TypedCommons,
    qual: &std::collections::HashMap<String, String>,
    runtime_use: &RuntimeUse,
    reached: &[TyId],
) -> String {
    if types.is_empty() || reached.is_empty() {
        return String::new();
    }
    let names: std::collections::HashSet<String> =
        types.iter().map(|t| t.name.name.clone()).collect();
    let extra: Vec<TypeRef> = reached
        .iter()
        .filter_map(|t| super::ty_to_type_ref(*t, typed.tys()))
        .filter(|r| super::serialisation::root_names_any(r, &names))
        .collect();
    if extra.is_empty() {
        return String::new();
    }
    let (base_names, base_insts) =
        super::serialisation::collect_codec_closure(&suite_base_codec_roots(types), &typed.types);
    let (all_names, all_insts) = super::serialisation::collect_codec_closure(&extra, &typed.types);
    let new_names: Vec<String> = all_names
        .into_iter()
        .filter(|n| !base_names.contains(n))
        .collect();
    let base_inst_names: std::collections::HashSet<String> =
        base_insts.iter().map(|i| i.ts_name()).collect();
    let new_insts: Vec<_> = all_insts
        .into_iter()
        .filter(|i| !base_inst_names.contains(&i.ts_name()))
        .collect();
    let mut stmts: Vec<bynk_ts::TsStmt> = Vec::new();
    stmts.extend(super::serialisation::decls_as_stmts_block(
        super::serialisation::emit_helpers_for_owner_qualified(
            &new_names,
            &typed.types,
            "",
            qual,
            runtime_use,
        ),
    ));
    stmts.extend(super::serialisation::decls_as_stmts(
        super::serialisation::emit_generic_helpers_qualified(
            &new_insts,
            &typed.types,
            qual,
            runtime_use,
        ),
    ));
    stmts
        .iter()
        .map(|s| bynk_ts::print_stmt(s, 0))
        .collect::<String>()
        .lines()
        .map(|l| format!("{}\n", l.strip_prefix("export ").unwrap_or(l)))
        .collect()
}

/// 1A.32c-1 review: no golden fixture reaches an agent's `migrate from N`
/// dispatcher, so this pins the node form against the text the old `writeln!`
/// built (`format!("function {migrate_fn}(stored: ...) {{\n  const __v = ...;\n{dispatch}  return stored;\n}}\n")`),
/// byte for byte, for a dispatcher with two steps.
#[cfg(test)]
mod migrate_dispatcher_tests {
    use super::{migrate_dispatcher_decl, migrate_step_dispatch};

    #[test]
    fn the_migration_dispatcher_prints_as_the_old_text_did() {
        let decl = migrate_dispatcher_decl(
            "__migrateCounterState",
            vec![
                migrate_step_dispatch(1, 1, "__migrateCounterFrom1"),
                migrate_step_dispatch(2, 3, "__migrateCounterFrom2"),
            ],
        );
        let old = "function __migrateCounterState(stored: globalThis.Record<string, unknown>): globalThis.Record<string, unknown> {\n  const __v = stored[\"__v\"] === undefined ? 1 : stored[\"__v\"] as number;\n  if (__v >= 1 && __v <= 1) return __migrateCounterFrom1(stored);\n  if (__v >= 2 && __v <= 3) return __migrateCounterFrom2(stored);\n  return stored;\n}\n";
        assert_eq!(bynk_ts::print_stmt(&decl, 0), old);
    }
}
