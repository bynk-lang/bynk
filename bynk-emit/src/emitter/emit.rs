//! Per-declaration emission — the functions `emit_project` drives to render
//! each top-level Bynk declaration into TypeScript: type/refined/record/sum
//! declarations and their checks, attached methods and free functions,
//! capabilities, providers, services, contexts, and agents (plus the
//! worker-dispatch lowering helpers those emitters use). Split out of
//! `emitter.rs` (ADR 0060); the codec/reference/import/header helpers and the
//! `ts_*`/`LowerCtx` core stay in the parent and are reached via `use super::*`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use crate::project::EmitProjectCtx;
use bynk_check::actors::ActorDecl;
use bynk_check::checker::{TyId, TypedCommons, Types};
use bynk_syntax::ast::{
    AgentDecl, BaseType, CapabilityDecl, CommonsItem, Expr, ExprKind, FnDecl, FnName, Handler,
    Ident, MessageEntry, MessagesDecl, Param, PredKind, ProviderDecl, RecordField, Refinement,
    ServiceDecl, StoreField, TypeBody, TypeDecl, TypeParam, TypeRef,
};

use bynk_ir::{
    ActorSeamIr, FnSig, IrHandlerKind, IrHttpMethod, OpSig, ProtocolIr, StoreFieldIr, StoreKindIr,
    TypeShape,
};
use bynk_lower::{
    HandlerSignatureIr, body_writes_state, is_effectful_return, lower_actor_seam_ir,
    lower_handler_kind_ir, lower_protocol_ir_from_commons,
};

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
    let mut stmts = Vec::new();
    let has_doc = t.documentation.is_some();
    if let Some(doc) = t.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    // For contexts, the per-type brand string is qualified by the context's
    // name (so two contexts' locally-declared `Order` types have distinct
    // brands at the TS level).
    let brand_prefix = ctx
        .owning_context
        .as_deref()
        .map(|c| format!("{c}."))
        .unwrap_or_default();
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
            &brand_prefix,
            &ctx.runtime_use,
        ),
        TypeShape::Record { fields } => emit_record_type(t, fields, commons, &ctx.runtime_use),
        // `embeds` is not read here — today's emitter has no reader for
        // `SumBody::embeds` anywhere (confirmed by grep; `embeds` is checker-
        // enforced construction-time only), so `TypeShape::Sum::embeds` stays
        // unread by this slice too, not a gap introduced by it.
        TypeShape::Sum { variants, .. } => emit_sum_type(t, variants, commons, &ctx.runtime_use),
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

/// Emit a doc block as a JSDoc-style comment at the given indent. Each line
/// of the doc body is prefixed with ` * `; empty lines become ` *`.
///
/// Doc-block bodies are copied verbatim from source, so a stray `*/` would
/// otherwise close the JSDoc comment early and let the trailing text land as
/// executable top-level TypeScript (issue #720). Escape it to `*\/`, which
/// renders identically but can no longer terminate the comment.
///
/// #1333 (R7.1): builds a real [`bynk_ts::TsStmt`] (`TsStmtKind::DocComment`)
/// and prints it through [`bynk_ts::print_stmt`], instead of `writeln!`-ing
/// the JSDoc text by hand — the escaping/blank-line rules above now live on
/// that variant's own doc and its printer arm; this function's own signature
/// and every one of its 14 real callers are unchanged, the same P7.9 pattern
/// `ts_type_ref`/`ts_ty` already used (`indent` is always an exact multiple
/// of `INDENT_STEP` in every real call, so `indent / INDENT_STEP` is a
/// lossless conversion to the printer's own 2-space-per-depth unit).
pub(crate) fn emit_doc_block(out: &mut String, doc: Option<&str>, indent: usize) {
    let Some(doc) = doc else { return };
    debug_assert!(
        indent.is_multiple_of(INDENT_STEP),
        "emit_doc_block: indent {indent} is not a multiple of INDENT_STEP"
    );
    let stmt = bynk_ts::TsStmt::doc_comment(doc, None);
    out.push_str(&bynk_ts::print_stmt(&stmt, indent / INDENT_STEP));
}

/// v0.93 (ADR 0118): deterministically order the `@indexed` map → fields entries
/// (by map name; fields keep their declaration order). `HashMap` iteration is
/// unordered, so emitted state fields would otherwise drift between runs.
fn sorted_index_fields(indexes: &HashMap<String, Vec<String>>) -> Vec<(&String, &Vec<String>)> {
    let mut entries: Vec<(&String, &Vec<String>)> = indexes.iter().collect();
    entries.sort_by_key(|(name, _)| name.to_string());
    entries
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

    let checks_text = {
        let mut checks = String::new();
        emit_refined_checks(&mut checks, t, base, refinement);
        checks
    };
    let cast_to_name = bynk_ts::TsExpr::As {
        expr: Box::new(bynk_ts::TsExpr::Ident("value".to_string())),
        ty: bynk_ts::TsType::named(name.clone()),
    };
    let mut entries: Vec<bynk_ts::TsObjectEntry> = vec![bynk_ts::TsObjectEntry::Method {
        name: "of".to_string(),
        is_async: false,
        generics: Vec::new(),
        params: vec![bynk_ts::TsParam {
            name: "value".to_string(),
            ty: Some(base_ty.clone()),
            optional: false,
        }],
        return_type: Some(bynk_ts::TsType::named_with_args(
            "Result",
            vec![
                bynk_ts::TsType::named(name.clone()),
                bynk_ts::TsType::named("ValidationError"),
            ],
        )),
        doc: None,
        inline: false,
        body: vec![
            bynk_ts::TsStmt::raw(checks_text, None),
            bynk_ts::TsStmt::return_stmt(
                Some(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Ident("Ok".to_string())),
                    args: vec![cast_to_name.clone()],
                }),
                None,
            ),
        ],
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
            params: vec![bynk_ts::TsParam {
                name: "value".to_string(),
                ty: Some(base_ty),
                optional: false,
            }],
            return_type: Some(bynk_ts::TsType::named(name.clone())),
            doc: None,
            inline: false,
            body: vec![bynk_ts::TsStmt::return_stmt(Some(cast_to_name), None)],
        });
    }
    entries.extend(emit_attached_methods(
        &t.name.name,
        &t.type_params,
        commons,
        runtime_use,
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
/// [`bynk_ts::TsStmt::Return`], printed at depth 2 (`out`'s own 4-space
/// convention here) through [`bynk_ts::print_stmt`], instead of hand-rolled
/// `writeln!`s — `emit_refined_checks` and `emit_pred_check`'s own exact
/// signatures are unchanged, the same P7.9/step-1 (#1333) pattern.
fn emit_refined_checks(
    out: &mut String,
    t: &TypeDecl,
    base: BaseType,
    refinement: Option<&Refinement>,
) {
    let name = &t.name.name;
    if base == BaseType::Int {
        // #1657: an `Int` is a JS safe integer.
        out.push_str(&print_numeric_guard_stmt(
            name,
            "isSafeInteger",
            "must be a safe integer",
        ));
    }
    // v0.21: validated `Float` values are finite — `.of` and the boundary
    // codec agree (ADR 0040); only in-language arithmetic is host-defined.
    if base == BaseType::Float {
        out.push_str(&print_numeric_guard_stmt(
            name,
            "isFinite",
            "must be a finite number",
        ));
    }
    if let Some(r) = refinement {
        for pred in bynk_syntax::ast::in_check_order(&r.predicates, |p| &p.kind) {
            emit_pred_check(out, name, &pred.kind);
        }
    }
}

/// `if (!Number.{method}(value)) { return Err({ field: "{name}", message:
/// "{message}", value }); }` — the `Int`/`Float` base-type guard shared by
/// both `emit_refined_checks` call sites, `field`/`message` both real
/// string literals with nothing to escape (a Bynk type name; a fixed,
/// hand-written English message).
fn print_numeric_guard_stmt(name: &str, method: &str, message: &str) -> String {
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
    print_guard_if_stmt(
        cond,
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(name.to_string())),
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(message.to_string())),
    )
}

/// `if (<cond>) { return Err({ field: <field>, message: <message>, value
/// }); }` at depth 2 — the one real shape both `print_numeric_guard_stmt`
/// and `emit_pred_check` build, differing only in `cond`/`field`/`message`.
fn print_guard_if_stmt(
    cond: bynk_ts::TsExpr,
    field: bynk_ts::TsExpr,
    message: bynk_ts::TsExpr,
) -> String {
    let err_obj = bynk_ts::TsExpr::object_entries(vec![
        bynk_ts::TsObjectEntry::Prop("field".to_string(), field),
        bynk_ts::TsObjectEntry::Prop("message".to_string(), message),
        bynk_ts::TsObjectEntry::Shorthand("value".to_string()),
    ]);
    let return_stmt = bynk_ts::TsStmt::return_stmt(
        Some(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Ident("Err".to_string())),
            args: vec![err_obj],
        }),
        None,
    );
    let if_stmt =
        bynk_ts::TsStmt::if_stmt(cond, bynk_ts::TsStmt::block(vec![return_stmt], None), None);
    bynk_ts::print_stmt(&if_stmt, 2)
}

fn emit_pred_check(out: &mut String, type_name: &str, pred: &PredKind) {
    let (cond, msg) = crate::emitter::pred_condition_and_message(pred, "value");
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
    out.push_str(&print_guard_if_stmt(
        cond_expr,
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(type_name.to_string())),
        bynk_ts::TsExpr::VerbatimExpr(format!("\"{msg}\""), bynk_ts::VerbatimOrigin::Emit),
    ));
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
) -> Vec<bynk_ts::TsStmt> {
    let type_params: Vec<String> = t.type_params.iter().map(|p| p.name.name.clone()).collect();
    let members: Vec<bynk_ts::TsTypeMember> = fields
        .iter()
        .map(|(name, ty)| {
            bynk_ts::TsTypeMember::readonly_prop(
                name.clone(),
                ts_ty_to_ts_type(*ty, &commons.ty_intern),
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
    let entries = emit_attached_methods(&t.name.name, &t.type_params, commons, runtime_use);
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
                    ts_ty_to_ts_type(*ty, &commons.ty_intern),
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
            let tag_obj = bynk_ts::TsExpr::object_entries(vec![bynk_ts::TsObjectEntry::Prop(
                "tag".to_string(),
                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(tag.clone())),
            )]);
            entries.push(bynk_ts::TsObjectEntry::Prop(
                tag.clone(),
                bynk_ts::TsExpr::As {
                    expr: Box::new(tag_obj),
                    ty: never_ty.clone(),
                },
            ));
        } else {
            let ctor_params: Vec<bynk_ts::TsParam> = payload
                .iter()
                .map(|(field, ty)| bynk_ts::TsParam {
                    name: ts_ident(&payload_prop(field)),
                    ty: Some(ts_ty_to_ts_type(*ty, &commons.ty_intern)),
                    optional: false,
                })
                .collect();
            let mut obj_entries = vec![bynk_ts::TsObjectEntry::Prop(
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
                    bynk_ts::TsObjectEntry::Prop(prop, bynk_ts::TsExpr::Ident(param))
                }
            }));
            let ctor_body =
                bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::object_entries(obj_entries)));
            entries.push(bynk_ts::TsObjectEntry::Prop(
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
    tys: &Arc<Types>,
) -> Vec<bynk_ts::TsObjectEntry> {
    let mut entries = Vec::new();
    for f in methods {
        let mut params: Vec<bynk_ts::TsParam> = Vec::new();
        let mut args: Vec<bynk_ts::TsExpr> = Vec::new();
        if f.has_self {
            params.push(bynk_ts::TsParam {
                name: "self".to_string(),
                ty: Some(bynk_ts::TsType::named(type_name)),
                optional: false,
            });
            args.push(bynk_ts::TsExpr::Ident("self".to_string()));
        }
        for (name, ty) in &f.params {
            let ident = ts_ident(name);
            params.push(bynk_ts::TsParam {
                name: ident.clone(),
                ty: Some(bynk_ts::TsType::named(ts_ty(*ty, tys))),
                optional: false,
            });
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
        };
        entries.push(entry);
    }
    entries
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
        params.push(bynk_ts::TsParam {
            name: "self".to_string(),
            ty: Some(bynk_ts::TsType::named(format!("{type_name}{self_ty_args}"))),
            optional: false,
        });
    }
    for p in &f.params {
        params.push(bynk_ts::TsParam {
            name: ts_ident(&p.name.name),
            ty: Some(bynk_ts::TsType::named(ts_type_ref(&p.type_ref))),
            optional: false,
        });
    }
    let empty = bynk_check::resolver::CrossContextInfo::default();
    let mut cx = LowerCtx::new(
        ModuleCtx::new(commons, &empty, runtime_use),
        BodyMode::Method,
    );
    // Methods are emitted as plain (non-async) members on an object literal;
    // any `Effect.pure(...)` in tail position must still wrap as
    // `Promise.resolve(...)` because there's no surrounding `async` to absorb
    // it. (Methods aren't expected to return `Effect[T]` in v0–v0.7.1.)
    let mut body_text = String::new();
    // #1653: parameters are bindings of the body's own scope, so a `let` that
    // re-binds one gets a fresh name instead of redeclaring it.
    for p in &f.params {
        cx.declare_binder(&p.name.name);
    }
    emit_block_as_function_body_with_return(
        &mut body_text,
        &f.body,
        &mut cx,
        INDENT_STEP * 2,
        false,
        Some(&f.return_type),
    );
    bynk_ts::TsObjectEntry::Method {
        name: method_name.name.clone(),
        is_async: false,
        generics,
        params,
        return_type: Some(bynk_ts::TsType::named(ts_type_ref(&f.return_type))),
        doc,
        inline: false,
        body: vec![bynk_ts::TsStmt::raw(body_text, None)],
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
) -> Vec<bynk_ts::TsStmt> {
    let FnName::Free(name) = &f.name else {
        return Vec::new();
    };
    let mut stmts = Vec::new();
    if let Some(doc) = f.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let params: Vec<bynk_ts::TsParam> = f
        .params
        .iter()
        .map(|p| bynk_ts::TsParam {
            name: ts_ident(&p.name.name),
            ty: Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
            optional: false,
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
    let body_smb = RefCell::new(SourceMapBuilder::new());
    let mut cx = LowerCtx::new(
        ModuleCtx::new(commons, &empty, runtime_use),
        BodyMode::FreeFn,
    )
    .with_source_map(Some(&body_smb));
    let async_tail = is_effectful_return(&f.return_type);
    let guarded = contracts && (!f.requires.is_empty() || !f.ensures.is_empty());
    // #1653: parameters are bindings of the body's own scope, so a `let` that
    // re-binds one gets a fresh name instead of redeclaring it.
    for p in &f.params {
        cx.declare_binder(&p.name.name);
    }
    let mut body_text = String::new();
    if guarded {
        emit_contract_guarded_body(&mut body_text, f, &mut cx, async_tail);
    } else {
        emit_block_as_function_body_with_return(
            &mut body_text,
            &f.body,
            &mut cx,
            INDENT_STEP,
            async_tail,
            Some(&f.return_type),
        );
    }

    let mut raw_body = bynk_ts::TsStmt::raw(body_text, None);
    raw_body.nested_map = Some(body_smb.into_inner());
    // #1486: `no_blank_before` when this function's own doc comment is the
    // preceding statement — `bynk_ts::print` itself now supplies the one
    // blank line between adjacent real statements (a fresh call site's own
    // preceding item, or nothing at all for the very first top-level
    // statement), but a doc comment sits flush above the declaration it
    // documents, no blank between them.
    let mut fn_stmt = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Function {
            name: ts_ident(&name.name),
            generics,
            params,
            return_type: Some(ts_type_ref_to_ts_type(&f.return_type, None)),
            body: vec![raw_body],
            is_async,
            inline: false,
        })),
        None,
    );
    fn_stmt.no_blank_before = f.documentation.is_some();
    stmts.push(fn_stmt);
    stmts
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
/// stay opaque — carried the same way #1335's own `emit_pred_check` used to
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
fn emit_contract_guarded_body(out: &mut String, f: &FnDecl, cx: &mut LowerCtx, async_tail: bool) {
    let FnName::Free(name) = &f.name else {
        return;
    };
    let fn_name = name.name.clone();
    // A `${…}` interpolation of each in-scope value for the failure report.
    let param_dump = |extra_result: bool| -> String {
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
    };
    // Precondition guards — parameters are the TS function params, in scope.
    for c in &f.requires {
        let mut pre = Pre::new();
        let pred = pre.lower(&c.predicate, cx);
        for s in pre.stmts() {
            out.push_str(&bynk_ts::print_stmt(
                &bynk_ts::TsStmt::raw(format!("  {s}\n"), None),
                0,
            ));
        }
        let msg = format!(
            "contract violated: precondition \\`{clause}\\` of {fn_name} ({dump})",
            clause = c.name.name,
            dump = param_dump(false),
        );
        out.push_str(&bynk_ts::print_stmt(
            &contract_guard_if_stmt(&pred, &msg),
            1,
        ));
    }
    // Run the original body, capturing its value as `result` for the `ensures`.
    //
    // Written directly into `out`, NOT captured into a local buffer and
    // wrapped as one opaque `TsStmt::raw` the way #1339's own `emit_refined_
    // type` does for `emit_refined_checks`'s output — that precedent is safe
    // only because `emit_refined_checks` never touches source-map recording.
    // `emit_block_as_function_body_with_return` calls `cx.record_span(out.
    // len(), ...)` internally: a local buffer's own length starts at 0, so
    // any checkpoint recorded during its lowering would land at the WRONG
    // offset once its text is later spliced into `out` at a non-zero
    // position — the exact bug #1352 already found and fixed one level up
    // (in `emit_free_fn`, for `body_text` itself); introducing a second
    // local buffer HERE reproduces it one level deeper, silently, since
    // `out` (this function's own parameter, = `emit_free_fn`'s `body_text`)
    // is the one buffer whose offsets `emit_free_fn`'s own `body_smb`/
    // `merge` machinery is already correctly rebased against. Writing here
    // exactly as the pre-conversion code did — directly into `out`, no
    // wrapping — keeps every `record_span` call's own `out.len()` correct
    // by construction, with no merge of its own needed at this level.
    if async_tail {
        writeln!(out, "  const result = await (async () => {{").unwrap();
    } else {
        writeln!(out, "  const result = (() => {{").unwrap();
    }
    emit_block_as_function_body_with_return(
        out,
        &f.body,
        cx,
        INDENT_STEP * 2,
        async_tail,
        Some(&f.return_type),
    );
    writeln!(out, "  }})();").unwrap();
    // Postcondition guards — `result` (and the parameters) are in scope.
    for c in &f.ensures {
        let mut pre = Pre::new();
        let pred = pre.lower(&c.predicate, cx);
        for s in pre.stmts() {
            out.push_str(&bynk_ts::print_stmt(
                &bynk_ts::TsStmt::raw(format!("  {s}\n"), None),
                0,
            ));
        }
        let msg = format!(
            "contract violated: postcondition \\`{clause}\\` of {fn_name} ({dump})",
            clause = c.name.name,
            dump = param_dump(true),
        );
        out.push_str(&bynk_ts::print_stmt(
            &contract_guard_if_stmt(&pred, &msg),
            1,
        ));
    }
    out.push_str(&bynk_ts::print_stmt(
        &bynk_ts::TsStmt::return_stmt(Some(bynk_ts::TsExpr::Ident("result".to_string())), None),
        1,
    ));
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
    /// `expected.ts` (`bynkc/tests/fixtures/positive/198_generic_identity_compose`)
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
pub(crate) fn http_handler_method_name_ir(method: IrHttpMethod, path: &str) -> String {
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
pub(crate) fn collect_handler_labels(commons: &TypedCommons) -> Option<String> {
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
pub(crate) fn cron_handler_method_name(service: &str, index: usize) -> String {
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
            let given = bynk_lower::lower_provider_given_ir(p);
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
pub(crate) fn queue_handler_method_name(service: &str, index: usize) -> String {
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
        params: vec![bynk_ts::TsParam {
            name: "params".to_string(),
            ty: Some(bynk_ts::TsType::named_with_args(
                "ReadonlyMap",
                vec![
                    bynk_ts::TsType::named("string"),
                    bynk_ts::TsType::named("__bynkMessageArg"),
                ],
            )),
            optional: false,
        }],
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
            params: vec![bynk_ts::TsParam {
                name: "__arg".to_string(),
                ty: None,
                optional: false,
            }],
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
    let mut table = String::new();
    writeln!(
        table,
        "const __messagesByLocale: globalThis.Record<string, globalThis.Record<string, (params: ReadonlyMap<string, __bynkMessageArg>) => string>> = {{"
    )
    .unwrap();
    for m in blocks {
        emit_doc_block(&mut table, m.documentation.as_deref(), INDENT_STEP);
        let code_entries: Vec<bynk_ts::TsObjectEntry> = m
            .entries
            .iter()
            .map(|entry| {
                bynk_ts::TsObjectEntry::Prop(
                    format!("\"{}\"", escape_ts_string(&entry.code)),
                    emit_message_entry_renderer(entry, &m.tag, runtime_use),
                )
            })
            .collect();
        let locale_entry = bynk_ts::TsObjectEntry::Prop(
            format!("\"{}\"", escape_ts_string(&m.tag)),
            bynk_ts::TsExpr::multiline_object_entries(code_entries),
        );
        table.push_str(&bynk_ts::print_object_entry(&locale_entry, 0));
    }
    writeln!(table, "}};").unwrap();
    stmts.push(bynk_ts::TsStmt::raw(table, None));

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
                bynk_ts::TsParam {
                    name: "tag".to_string(),
                    ty: Some(bynk_ts::TsType::named("__bynkLocaleTag")),
                    optional: false,
                },
                bynk_ts::TsParam {
                    name: "msg".to_string(),
                    ty: Some(bynk_ts::TsType::named("__bynkMessage")),
                    optional: false,
                },
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
/// `params`/`return_ty` off `ops` ([`bynk_ir::OpSig`], from
/// `bynk_lower::lower_capability_ops_ir`) through `ts_ty`, instead of
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
            let params: Vec<bynk_ts::TsParam> = sig
                .params
                .iter()
                .map(|(name, ty)| bynk_ts::TsParam {
                    name: ts_ident(name),
                    ty: Some(ts_ty_to_ts_type(*ty, &commons.ty_intern)),
                    optional: false,
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
                generics: op
                    .type_params
                    .iter()
                    .map(|tp| tp.name.name.clone())
                    .collect(),
                params,
                ret: ts_ty_to_ts_type(sig.return_ty, &commons.ty_intern),
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
        let out = match crate::project::compile_in_memory(
            src,
            crate::project::BuildTarget::Bundle,
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

/// The text held by a `TsClassMethod`'s sole `Raw` body statement, and —
/// when `body_smb` was built against only a sub-slice of it — where that
/// sub-slice starts.
///
/// `blob` is the exact `Raw` text. For the single-level shape (#1359's
/// provider ops, #1380's WS DO methods, [`RawBody::flat`]) `inner` IS
/// `blob` and the offset is 0; for the two-level shape (#1361's
/// events-IIFE wrapping, reused by #1375's agent handlers,
/// [`RawBody::nested`]) `inner` is the further-nested `body_out` captured
/// inside `blob`, and `inner_offset_in_blob` is its start within `blob`,
/// tracked by construction rather than searched for.
struct RawBody<'a> {
    blob: &'a str,
    inner: &'a str,
    inner_offset_in_blob: usize,
}

impl<'a> RawBody<'a> {
    fn flat(text: &'a str) -> Self {
        RawBody {
            blob: text,
            inner: text,
            inner_offset_in_blob: 0,
        }
    }

    fn nested(blob: &'a str, inner: &'a str, inner_offset_in_blob: usize) -> Self {
        RawBody {
            blob,
            inner,
            inner_offset_in_blob,
        }
    }
}

/// Prints `method` (always at `class_depth = 0` — every call site here
/// prints a whole class body) into `out`, then — if `source_map` is
/// `Some` — merges `body_smb`'s own per-statement mappings into it. Despite
/// the name, `source_map` isn't necessarily the real module map any more:
/// `emit_agent`'s own callers (#1482) pass a class-local map that its own
/// caller merges into the module map one level up, the same two-level
/// composition `emit_provider`'s own doc comment (below) already
/// establishes for its own class wrapper.
///
/// Degrades to no merge (no mapping for this method) rather than risk a
/// confidently WRONG one if any assumption is ever violated — the same
/// "suppress rather than mis-record" discipline #1360 finding 3
/// established, now shared by every `print_class_method` merge site
/// instead of copied at each one (review of #1381, finding 2).
fn emit_class_method_and_merge_source_map(
    out: &mut String,
    method: &bynk_ts::TsClassMethod,
    class_depth: usize,
    body: RawBody<'_>,
    body_smb: &RefCell<SourceMapBuilder>,
    source_map: Option<&RefCell<SourceMapBuilder>>,
) {
    let printed = bynk_ts::print_class_method(method, class_depth);
    out.push_str(&printed);
    if let Some(module_smb) = source_map {
        let trailing = format!("{}}}\n", "  ".repeat(class_depth + 1));
        if let Some(blob_offset_in_printed) =
            printed.len().checked_sub(body.blob.len() + trailing.len())
            && printed
                .get(blob_offset_in_printed..)
                .is_some_and(|tail| tail.starts_with(body.blob))
            && printed.ends_with(&trailing)
        {
            let base =
                out.len() - printed.len() + blob_offset_in_printed + body.inner_offset_in_blob;
            module_smb
                .borrow_mut()
                .merge(&body_smb.borrow(), body.inner, out, base, 0);
        }
    }
}

/// #1480: returns one real [`bynk_ts::TsStmt`] (was `out: &mut String`) — a
/// `Raw` carrier for the whole provider declaration (class header/
/// wrapper text, Decision C: a real `TsDecl::Class` would need every
/// method's own body captured into a local buffer for `Raw`-embedding
/// anyway, and this class's own spacing convention already disagrees with
/// `TsDecl::Class`'s own — not a further decomposition this slice attempts),
/// with a class-wide `nested_map` combining every op method's own
/// `body_smb` at its own real print-time offset *within this function's own
/// local `out` buffer* — computed by `bynk_ts::print_class_method_and_merge`
/// itself (#1477) rather than the exact-arithmetic recovery
/// `emit_class_method_and_merge_source_map` (above) still performs —
/// `emit_service` no longer calls it at all (#1481); `emit_agent`'s own
/// per-handler loop and `emit_ws_do_method` are its only remaining callers
/// (#1482 converted `emit_agent`'s own top-level signature, but left this
/// hand-rolled per-method recovery itself untouched, same as #1481 did).
/// The
/// caller then merges this ONE combined map into its own real module map at
/// the class's own real splice offset, via `print_stmt_and_merge` — the same
/// two-level "local map merges into a local buffer, that buffer's own map
/// merges into the real one at its own real offset" composition
/// `print_stmt_and_merge`'s own doc already establishes for a nested `Raw`.
pub(crate) fn emit_provider(
    p: &ProviderDecl,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Option<bynk_ts::TsStmt> {
    // v0.17: an external (bodiless) provider inside an adapter is supplied by
    // the adapter's binding — the compiler emits no class for it. Its symbol is
    // imported and constructed by the consumer's compose (§6.1, Phase 2).
    if p.external {
        return None;
    }
    let mut out = String::new();
    let mut class_smb = SourceMapBuilder::new();
    emit_doc_block(&mut out, p.documentation.as_deref(), 0);
    writeln!(
        out,
        "export class {prov} implements {cap} {{",
        prov = p.provider_name.name,
        cap = p.capability.name,
    )
    .unwrap();
    // v0.12: a provider with `given` receives its dependencies through a
    // constructor; its bodies call them as `this.deps.<cap>`. The deps object
    // type lists exactly the provider's `given` capabilities.
    //
    // P7.2 (review of #1300): computed once here and reused below (the
    // capability-scope collection a few lines down, and the factory's own
    // `deps_ty` further below) — previously recomputed independently at each
    // site from the same `p.given`, three calls into the same pure function
    // for the same answer.
    let given_ir = bynk_lower::lower_provider_given_ir(p);
    // #2355: a provider whose ops construct a local agent hands `this.deps` to
    // the agent's methods, so it takes a `deps` even with no `given`; on
    // `workers` that `deps` also carries the Worker's `env`, which the agent
    // factory reads its Durable Object namespace from.
    let constructs_agent =
        bynk_check::project_model::provider_constructs_agent(p, |n| ctx.local_agents.contains(n));
    let takes_deps = !p.given.is_empty() || constructs_agent;
    // Arc F, slice 4 (#1463): `cap_ref_ty` now returns a real `TsType`, but
    // this class's own wrapper (this `deps` field/constructor signature text)
    // stays hand-written per Decision C — re-serialise via
    // `bynk_ts::print_type`, no behaviour change.
    let mut deps_fields: Vec<String> = given_ir
        .iter()
        .map(|c| {
            format!(
                "{}: {}",
                c.name,
                bynk_ts::print_type(&cap_ref_ty(c, &ctx.cross_context))
            )
        })
        .collect();
    if constructs_agent && matches!(ctx.target, BuildTarget::Workers) {
        let mut agents: Vec<String> = ctx.local_agents.iter().cloned().collect();
        agents.sort();
        deps_fields.push(format!(
            "env: {}",
            bynk_ts::print_type(&workers_env_ty(&[], &agents))
        ));
    }
    let deps_ty = deps_fields.join("; ");
    if takes_deps {
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
        writeln!(out, "  private deps: {{ {deps_ty} }};").unwrap();
        writeln!(
            out,
            "  constructor(deps: {{ {deps_ty} }}) {{ this.deps = deps; }}"
        )
        .unwrap();
    }
    // #1359 (R7.1): each op converts to a real `bynk_ts::TsClassMethod`
    // (name/`is_async`/real `TsParam`s via `ts_type_ref_to_ts_type`, P7.9
    // #1315/real return type), printed through the new fragment entry
    // point `bynk_ts::print_class_method` (mirroring `print_object_entry`'s
    // own "one fragment, not a whole document" scope, #1337) — the class's
    // own wrapper (header/`implements`/deps field+constructor/closing
    // brace) stays hand-written text, Decision C: building the WHOLE class
    // as one real `TsDecl::Class` tree would need every method's own body
    // captured into a local buffer for `Raw`-embedding, and this class's
    // own real spacing (no blank line between methods) genuinely differs
    // from `TsDecl::Class`'s own established "one blank line before each
    // method" policy (`events_fanout.rs`'s real convention, #1317) — the
    // same "two real files disagree on a formatting convention" tension
    // this track has hit before.
    //
    // Each op's own body still lowers through `emit_block_as_function_
    // body_with_return` — unconverted, out of scope — but can no longer
    // write directly into `out` the way the pre-conversion code did (the
    // fragment must be fully built before `out.len()` at the splice point
    // is known). Captured into a local `body_text` buffer instead, using
    // the same local-sub-builder-then-`merge` pattern #1352/#1353 already
    // established, applied once per method instead of once per function:
    // `body_smb` collects checkpoints relative to `body_text`, then
    // `merge`s into the real `source_map`, rebased at `body_text`'s own
    // exact splice offset within `print_class_method`'s own printed
    // fragment (computed by arithmetic against the fragment's own known
    // length, not a text search — two ops could plausibly lower to
    // identical body text, which a search could match ambiguously).
    for op in &p.ops {
        let params: Vec<bynk_ts::TsParam> = op
            .params
            .iter()
            .map(|p| bynk_ts::TsParam {
                name: ts_ident(&p.name.name),
                ty: Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
                optional: false,
            })
            .collect();
        // P6.55 (design/tracks/the-ir.md §6b): computed once, not twice —
        // `async_tail` below used to call `is_effectful_return(&op.return_type)`
        // again for the identical value (the same duplicate-computation
        // pattern P6.54 fixed in `emit_agent`).
        let effectful = is_effectful_return(&op.return_type);
        let mut module = ModuleCtx::new(commons, &ctx.cross_context, &ctx.runtime_use);
        module.agent_method_givens = ctx.agent_method_givens.clone();
        module.agent_called_contexts = ctx.agent_called_contexts.clone();
        module.event_schema_versions = ctx.event_schema_versions.clone();
        module.consumed_type_ns = consumed_type_ns(ctx);
        module.consumed_context_types = ctx.consumed_context_types();
        module.target = ctx.target;
        module.in_bynk_unit = ctx.commons_name == "bynk";
        let body_smb = RefCell::new(SourceMapBuilder::new());
        let mut cx = LowerCtx::new(
            module,
            BodyMode::ProviderOp {
                handler: HandlerShared {
                    // The provider's `given` capabilities are in scope in its
                    // bodies, and resolve against the injected `this.deps`.
                    capabilities: given_ir.iter().map(|c| c.name.clone()).collect(),
                    cap_deps_expr: if takes_deps {
                        "this.deps".to_string()
                    } else {
                        "deps".to_string()
                    },
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
        )
        .with_source_map(Some(&body_smb));
        cx.local_agents = ctx.local_agents.clone();
        // #1653: parameters are bindings of the body's own scope, so a `let` that
        // re-binds one gets a fresh name instead of redeclaring it.
        for p in &op.params {
            cx.declare_binder(&p.name.name);
        }
        let mut body_text = String::new();
        emit_block_as_function_body_with_return(
            &mut body_text,
            &op.body,
            &mut cx,
            INDENT_STEP * 2,
            effectful,
            Some(&op.return_type),
        );

        // The class is always printed at depth 0 — `class_depth` names that
        // once, so a merge site's own indent assumptions can never drift out
        // of lockstep with the depth `print_class_method_and_merge` is
        // actually called with.
        let class_depth = 0;
        let mut raw_body = bynk_ts::TsStmt::raw(body_text, None);
        raw_body.nested_map = Some(body_smb.into_inner());
        let method = bynk_ts::TsClassMethod {
            name: op.name.name.clone(),
            private: false,
            is_async: effectful,
            params,
            return_type: Some(ts_type_ref_to_ts_type(&op.return_type, None)),
            doc: None,
            body: vec![raw_body],
        };
        // #2280: anchor the op's header and prologue to its own `fn` line
        // rather than the previous op's last body line.
        class_smb.record(out.len(), op.span);
        bynk_ts::print_class_method_and_merge(&mut out, &method, class_depth, &mut class_smb, 0);
    }
    // #2280: the class's closing brace and the registration below belong to the
    // provider, not to the last op's final body line.
    class_smb.record(out.len(), p.span);
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();
    let factory = if takes_deps {
        format!(
            "(deps: {{ {deps_ty} }}) => new {}(deps)",
            p.provider_name.name
        )
    } else {
        format!("() => new {}()", p.provider_name.name)
    };
    writeln!(
        out,
        "export const __{prov}Provider = {{ token: __{cap}Token, factory: {factory} }};",
        prov = p.provider_name.name,
        cap = p.capability.name,
    )
    .unwrap();
    // #1486: no trailing blank baked into this Raw text any more —
    // `bynk_ts::print` itself now supplies the one blank line between this
    // statement and whatever follows it in `emit_project`'s own top-level
    // list (see `TsStmt::no_blank_before`'s own doc for why keeping this
    // would have doubled it).
    let mut stmt = bynk_ts::TsStmt::raw(out, None);
    stmt.nested_map = Some(class_smb);
    Some(stmt)
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

/// #1481: returns one real [`bynk_ts::TsStmt`] (was `out: &mut String`) — a
/// `Raw` carrier for the whole `export const {name} = { ... };` object
/// literal (Decision C, the same posture `emit_provider` (#1480) already
/// takes for its class: a real `TsDecl::ConstDecl`/`TsExpr::multiline_
/// object_entries` pair *could* represent this literal's own skeleton, but
/// `render_decl_body`'s `ConstDecl` arm doesn't thread a source-map merge
/// target into its own `init` — only a `TsDecl::Function`/`TsDecl::Class`
/// body does, #1477's own scope — so a top-level `ConstDecl` here would
/// silently lose every handler's own per-statement mapping), with a
/// service-wide `service_smb` combining every handler's own `body_smb` at
/// its own real print-time offset *within this function's own local `out`
/// buffer* — computed by [`bynk_ts::print_object_entry_and_merge`] itself
/// (#1477) rather than the exact-arithmetic/text-search recovery this
/// function used to perform by hand, once per handler rather than once per
/// function (the clearest, most recurring live instance of the
/// nested-source-map gap #1477 closed). The caller merges this ONE combined
/// map into its own real module map at the object's own real splice offset,
/// via `print_stmt_and_merge` — the same two-level composition
/// `emit_provider`'s own doc comment already establishes.
pub(crate) fn emit_service(
    s: &ServiceDecl,
    protocol: &ProtocolIr,
    signatures: &[HandlerSignatureIr],
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> bynk_ts::TsStmt {
    let tys = commons.tys();
    let mut out = String::new();
    let mut service_smb = SourceMapBuilder::new();
    emit_doc_block(&mut out, s.documentation.as_deref(), 0);
    writeln!(out, "export const {name} = {{", name = s.name.name).unwrap();
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
        };
        // For service handlers the operation name is the handler kind
        // (e.g. `call`). v0.5 has only one handler kind, so the service is a
        // single-operation object literal.
        let mut params: Vec<bynk_ts::TsParam> = ir_params
            .iter()
            .map(|(name, ty)| bynk_ts::TsParam {
                name: ts_ident(name),
                ty: Some(ts_ty_to_ts_type(*ty, tys)),
                optional: false,
            })
            .collect();
        // v0.103/v0.106: a `from websocket` lifecycle handler receives the
        // `connection` as its first parameter (the synthetic binding the checker
        // added — the fresh socket for `on open`, the firing socket for `on
        // message`/`on close`); emit it so the lowered body's `connection` resolves.
        if is_ws_handler && let ProtocolIr::WebSocket { out_ty, .. } = protocol {
            params.insert(
                0,
                bynk_ts::TsParam {
                    name: "connection".to_string(),
                    ty: Some(bynk_ts::TsType::named_with_args(
                        "Connection",
                        vec![ts_ty_to_ts_type(*out_ty, tys)],
                    )),
                    optional: false,
                },
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
                        bynk_ts::TsParam {
                            name: "__bynkSchemaEnv".to_string(),
                            ty: Some(bynk_ts::TsType::named("EventEnvelope")),
                            optional: false,
                        },
                    );
                    Some("__bynkSchemaEnv".to_string())
                }
            }
        } else {
            None
        };
        // Lower the body first so we can detect cross-context usage and
        // adjust the deps shape accordingly.
        let mut body_out = String::new();
        // v0.70: each handler body lowers into its own source-map sub-builder
        // (offsets relative to `body_out`), merged into the module builder at the
        // splice below so handler statements map per-statement, not to the
        // `service` declaration line.
        let body_smb = RefCell::new(SourceMapBuilder::new());
        // v0.52: a multi-actor sum handler's resolved actor is threaded through
        // `deps.who`; the binder ident lowers to it so the body can `match`. A
        // sum supersedes the single-actor Bearer identity path (the per-arm
        // identity comes from the match, not a single `deps.identity`).
        // v0.47/v0.151/v0.52/v0.54: a handler's own actor-verification seam —
        // Bearer/Oidc/Caller thread their identity through `deps.identity`,
        // a sum threads the resolved-actor tagged union through `deps.who`
        // (the body `match`es it). `lower_actor_seam_ir`'s own doc comment
        // (`bynk_ir::ActorSeamIr`) has the full grounding for why sum is
        // tried first (it can otherwise collide with Bearer) and why the
        // other three are mutually exclusive by construction.
        let seam = lower_actor_seam_ir(handler, &ctx.actors);
        let deps_identity_binder = match &seam {
            ActorSeamIr::Caller(binder) => Some(binder.clone()),
            ActorSeamIr::Bearer(s) => s.binder.clone(),
            ActorSeamIr::Oidc(s) => s.binder.clone(),
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
        let mut module = ModuleCtx::new(commons, &ctx.cross_context, &ctx.runtime_use);
        module.in_bynk_unit = ctx.commons_name == "bynk";
        module.agent_method_givens = ctx.agent_method_givens.clone();
        module.agent_called_contexts = ctx.agent_called_contexts.clone();
        module.event_schema_versions = ctx.event_schema_versions.clone();
        module.consumed_type_ns = consumed_type_ns(ctx);
        module.consumed_context_types = ctx.consumed_context_types();
        module.target = ctx.target;
        let mut cx = LowerCtx::new(
            module,
            BodyMode::ServiceHandler {
                handler: HandlerShared {
                    capabilities: bynk_lower::lower_handler_given_ir(handler)
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
        )
        .with_source_map(Some(&body_smb));
        cx.local_agents = ctx.local_agents.clone();
        // #1653: parameters are bindings of the body's own scope, so a `let` that
        // re-binds one gets a fresh name instead of redeclaring it.
        for p in &handler.params {
            cx.declare_binder(&p.name.name);
        }
        let async_tail = *ir_effectful;
        emit_block_as_function_body_with_return(
            &mut body_out,
            &handler.body,
            &mut cx,
            INDENT_STEP * 2,
            async_tail,
            Some(&handler.return_type),
        );
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
            let prologue = format!(
                "{}if (!({guard})) return undefined;\n",
                " ".repeat(INDENT_STEP * 2)
            );
            body_out.insert_str(0, &prologue);
            // #1363: `body_smb`'s checkpoints were recorded against
            // `body_out`'s pre-insert contents — rebase them by the
            // prologue's own byte length so `merge`'s line lookup (against
            // the post-insert `body_out` handed to it below) still resolves
            // each statement to its own real line rather than one line too
            // early.
            body_smb.borrow_mut().shift_checkpoints(prologue.len());
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
        {
            let prologue = format!(
                "{}if (!({env_binder}.schemaVersion === {version})) return undefined;\n",
                " ".repeat(INDENT_STEP * 2)
            );
            body_out.insert_str(0, &prologue);
            // #1363: same rebase as the subscriber-filter prologue above —
            // a handler with both prologues shifts `body_smb`'s checkpoints
            // twice, once per insert, matching the two lines actually
            // prepended to `body_out`.
            body_smb.borrow_mut().shift_checkpoints(prologue.len());
        }
        // Append the deps parameter (may include surface field if the body
        // made cross-context calls). v0.47: a Bearer handler's deps also carries
        // the seam-minted `identity` — but only when a binder captures it
        // (v0.50: a binder-less Bearer handler verifies but mints no identity).
        let mut deps_members = build_deps_object_ty_with_surface(
            &effective_given(&bynk_lower::lower_handler_given_ir(handler), &cx),
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
            ActorSeamIr::Bearer(s) if s.binder.is_some() => {
                append_deps_field(
                    &mut deps_members,
                    "identity",
                    bynk_ts::TsType::named(s.identity_type.clone()),
                );
            }
            // v0.151: an `Oidc`-binding handler threads its `sub`-minted
            // identity into deps exactly like Bearer.
            ActorSeamIr::Oidc(s) if s.binder.is_some() => {
                append_deps_field(
                    &mut deps_members,
                    "identity",
                    bynk_ts::TsType::named(s.identity_type.clone()),
                );
            }
            // v0.54: a Caller-binding call handler's deps carries the
            // caller's context name as its `CallerId` identity (a `string`).
            ActorSeamIr::Caller(_) => {
                append_deps_field(
                    &mut deps_members,
                    "identity",
                    bynk_ts::TsType::named("string"),
                );
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
                let union = members
                    .iter()
                    .map(|m| match m.identity_type() {
                        Some(id) => format!("{{ tag: \"{}\", identity: {id} }}", m.actor_name),
                        None => format!("{{ tag: \"{}\" }}", m.actor_name),
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                append_deps_field(&mut deps_members, "who", bynk_ts::TsType::named(union));
            }
            // I3 (#1971): a `call` sum. `Caller` carries the calling context's
            // name; a foreign caller is its actor alone.
            ActorSeamIr::CallSum(members) => {
                append_deps_field(&mut deps_members, "who", call_sum_ts_type(members));
            }
            // A binder-less Bearer/Oidc, or no seam at all — nothing to widen.
            ActorSeamIr::Bearer(_) | ActorSeamIr::Oidc(_) | ActorSeamIr::None => {}
        }
        // v0.79: a handler whose body uses `~>` receives the execution context
        // (`__exec`) in its deps, so the fire-and-forget send can hand its promise
        // to `waitUntil`. Gated on the body so non-sending handlers are unchanged.
        if crate::emitter::block_uses_send(&handler.body) {
            append_deps_field(
                &mut deps_members,
                "__exec",
                bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::method(
                    "waitUntil",
                    vec![bynk_ts::TsParam {
                        name: "promise".to_string(),
                        ty: Some(bynk_ts::TsType::named_with_args(
                            "globalThis.Promise",
                            vec![bynk_ts::TsType::named("unknown")],
                        )),
                        optional: false,
                    }],
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
        // for `~>` exactly. Gated on more than this handler's own body: a
        // handler with no direct `Events.emit` call but that invokes a
        // *local agent* method which itself emits (`Ledger(id).bump(...)`)
        // still needs `__eventsDispatch` threaded through, to hand onward —
        // `cx.agent_given_caps_used` is exactly the existing mechanism that
        // already propagates an invoked agent method's other `given`
        // capabilities up to the caller (`effective_given`, just below);
        // `Events` rides the same path since it's declared as an ordinary
        // `given Events` on the agent handler.
        let needs_events_dispatch = cx.is_first_party_events()
            && (bynk_ir::block_uses_emit(&handler.body, &commons.callees)
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
        params.push(bynk_ts::TsParam {
            name: "deps".to_string(),
            ty: Some(bynk_ts::TsType::Object(deps_members)),
            optional: false,
        });
        // Events track, slice 0 (spine #936): release-at-commit (events.md
        // §3.0) needs a completion boundary services never had before — the
        // body runs inside an IIFE so its own `return`s resolve `__result`
        // rather than the outer method, and only a handler that completes
        // without throwing reaches past it, then flushes `__events` to
        // whatever composed it (`deps.__eventsDispatch`, threaded above).
        // Gated on `block_uses_emit` specifically (not the broader
        // `needs_events_dispatch` above) — a handler that only *forwards*
        // `__eventsDispatch` to an agent it calls has nothing of its own to
        // buffer or flush, so it keeps byte-identical output, mirroring
        // `__exec`'s gate on `block_uses_send`.
        let body_emits_directly = bynk_ir::block_uses_emit(&handler.body, &commons.callees);
        // #1361: the whole method body — the events-IIFE wrapper (if any)
        // and `body_out` — is captured as ONE opaque `TsStmtKind::Raw` blob,
        // the `emit_provider` precedent (#1359) exactly; the events-IIFE
        // stays opaque for the same reason #1327/#1352/#1359 already
        // declined to build a block-bodied `Arrow`. `raw_body`'s own
        // internal layout is built by straight concatenation, so
        // `body_out`'s own starting offset within it is known by
        // construction, not a text search.
        let mut raw_body = String::new();
        if body_emits_directly {
            writeln!(
                raw_body,
                "    const __events: globalThis.Array<{}> = [];",
                crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE
            )
            .unwrap();
            writeln!(raw_body, "    const __result = await (async () => {{").unwrap();
        }
        let body_out_offset_in_raw = raw_body.len();
        raw_body.push_str(&body_out);
        if body_emits_directly {
            writeln!(raw_body, "    }})();").unwrap();
            writeln!(raw_body, "    if (__events.length > 0) {{").unwrap();
            writeln!(raw_body, "      await deps.__eventsDispatch(__events);").unwrap();
            writeln!(raw_body, "    }}").unwrap();
            writeln!(raw_body, "    return __result;").unwrap();
        }

        // #1481: `body_smb`'s own checkpoints were recorded relative to
        // `body_out` (offset 0 = `body_out`'s own first byte) — rebase them
        // by `body_out_offset_in_raw` so they land relative to `raw_body`'s
        // own start instead, the coordinate space `nested_map` requires (its
        // checkpoints are relative to the `Raw` node's own whole text, not a
        // sub-slice of it — `TsStmt::nested_map`'s own doc). The same rebase
        // `shift_checkpoints` already performs above for the two prologues,
        // applied once more for the events-IIFE wrapper this prepends.
        body_smb
            .borrow_mut()
            .shift_checkpoints(body_out_offset_in_raw);
        let mut raw_stmt = bynk_ts::TsStmt::raw(raw_body, None);
        raw_stmt.nested_map = Some(body_smb.into_inner());

        let method_entry = bynk_ts::TsObjectEntry::Method {
            name: kind_name,
            is_async: *ir_effectful,
            generics: Vec::new(),
            params,
            return_type: Some(ts_ty_to_ts_type(*ir_ret, tys)),
            doc: handler.documentation.as_deref().map(str::to_owned),
            inline: false,
            body: vec![raw_stmt],
        };
        // The object (`export const {name} = { ... }`) is always printed at
        // depth 0 — `object_depth` names that once so this call's own depth
        // can't drift out of lockstep with `print_object_entry_and_merge`'s
        // real depth (review of #1360, finding 3's own lesson, applied
        // proactively here).
        let object_depth = 0;
        // #2280: anchor the method header and prologue to the handler's own
        // `on call` line rather than the previous statement's.
        service_smb.record(out.len(), handler.span);
        bynk_ts::print_object_entry_and_merge(
            &mut out,
            &method_entry,
            object_depth,
            &mut service_smb,
            0,
        );
    }
    // #2280: what follows the last handler belongs to the service, not to that
    // handler's final body line.
    service_smb.record(out.len(), s.span);
    writeln!(out, "}};").unwrap();
    // #1486: no trailing blank baked into this Raw text any more — see
    // `emit_provider`'s own identical note.
    let mut stmt = bynk_ts::TsStmt::raw(out, None);
    stmt.nested_map = Some(service_smb);
    stmt
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
    c: &bynk_ir::CapRefIr,
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
            .any(|h| bynk_ir::block_uses_emit(&h.body, &commons.callees)),
        CommonsItem::Agent(a) => a
            .handlers
            .iter()
            .any(|h| bynk_ir::block_uses_emit(&h.body, &commons.callees)),
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
        .map(bynk_lower::lower_handler_given_ir)
        .chain(std::iter::once(sibling_agent_givens(commons, ctx)));
    for given in givens {
        for c in given {
            // Events track, slice 0 (spine #936): `Events.emit` is
            // intercepted entirely at the call site (release-at-commit
            // buffering) and never calls through a constructed provider
            // — unlike every other capability, there is no
            // `EventsProvider` for compose to build, so the first-party
            // `Events` must not appear in any context's deps interface.
            let is_first_party_events = c.name == "Events"
                && info.flattened_caps.get(&c.name).map(String::as_str) == Some("bynk");
            if is_first_party_events {
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
    let mut collect = |given: Vec<bynk_ir::CapRefIr>| {
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
                .for_each(|h| collect(bynk_lower::lower_handler_given_ir(h))),
            CommonsItem::Agent(a) => a
                .handlers
                .iter()
                .for_each(|h| collect(bynk_lower::lower_handler_given_ir(h))),
            CommonsItem::Provider(p) => collect(bynk_lower::lower_provider_given_ir(p)),
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
fn sibling_agent_givens(commons: &TypedCommons, ctx: &EmitProjectCtx) -> Vec<bynk_ir::CapRefIr> {
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
fn effective_given(declared: &[bynk_ir::CapRefIr], cx: &LowerCtx<'_>) -> Vec<bynk_ir::CapRefIr> {
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
    out.retain(|c| {
        c.name != "Events"
            || !(cx.in_bynk_unit()
                || cx
                    .cross_context()
                    .flattened_caps
                    .get(&c.name)
                    .map(String::as_str)
                    == Some("bynk"))
    });
    out
}

/// Review of #1469: returns the bare member list, not `TsType::Object(members)`
/// — every real caller immediately widens it further (`append_deps_field`)
/// before wrapping it as the `deps` parameter's own type, and threading
/// `Vec<TsTypeMember>` end to end lets that widening take `&mut Vec<...>`
/// directly, with no `TsType::Object` match arm (and no panic path for the
/// "found something else" case that can't actually happen) in between.
fn build_deps_object_ty_with_surface(
    given: &[bynk_ir::CapRefIr],
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
/// namespace can't take [`qualified_to_ns`]'s bare name: a context `greet`
/// declaring `fn greet` would shadow its own import and read `greet` in its
/// temporal dead zone. The reserved `__ns_` prefix can't be a user identifier.
/// Every test-module namespace reference goes through this, in `tests_emit`
/// and in lowering that runs `in_test_scaffold()`.
pub(crate) fn test_scaffold_ns(q: &str) -> String {
    format!("__ns_{}", qualified_to_ns(q))
}

/// The PascalCase name a context uses for its generated `Deps` interface:
/// `shortener.links` → `ShortenerLinks`.
pub(crate) fn context_pascal(name: &str) -> String {
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
/// `-> String` (just the interface's own name, used as a plain
/// `TsType::named(deps_name)` type reference by `emit_make_surface`, its one
/// caller), only the declaration's own construction/printing changed.
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
    out: &mut String,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> String {
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
    let is_first_party_events = ctx.commons_name == "bynk"
        || ctx
            .cross_context
            .flattened_caps
            .get("Events")
            .map(String::as_str)
            == Some("bynk");
    if is_first_party_events && commons_uses_emit(commons) {
        members.push(bynk_ts::TsTypeMember::readonly_prop(
            "__eventsDispatch",
            bynk_ts::TsType::named(format!(
                "(events: globalThis.Array<{}>) => globalThis.Promise<void>",
                crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE
            )),
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
    out.push_str(&bynk_ts::print_stmt(&interface_decl, 0));
    // #1486: no trailing blank baked in here — the printer's automatic
    // top-level spacing policy supplies the single blank before whatever
    // follows this interface once `emit_make_surface` wraps it as `Raw`.
    deps_name
}

/// v0.54 (#655): whether any of `services` declares an `on call … by c: Caller`
/// handler, whose emitted `deps` carries the calling context's qualified name as
/// its `CallerId` identity (ADR 0092). This is the *single* predicate both
/// bundle caller-identity seams consult — `emit_make_surface` (which adds the
/// `__caller` parameter) and the compose root (which passes the name) — so the
/// two can never disagree on which providers thread a caller. `caller_binder_for`
/// self-guards `HandlerKind::Call`, so no kind filter is needed or wanted here.
pub(crate) fn any_service_binds_caller<'a>(
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
/// `emit_context_deps_interface` stays `out`-writing (unconverted, out of
/// scope — see its own call site's doc above), so its output is captured
/// into a local buffer and carried as one [`bynk_ts::TsStmtKind::Raw`]
/// statement, the same carrier `emit_messages_bundle`'s own conversion
/// (#1478) just used for the same reason.
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
    let mut deps_block = String::new();
    let deps_name = emit_context_deps_interface(&mut deps_block, commons, ctx);
    let mut stmts = vec![bynk_ts::TsStmt::raw(deps_block, None)];
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
    let mut params = vec![bynk_ts::TsParam {
        name: "deps".to_string(),
        ty: Some(bynk_ts::TsType::named(deps_name)),
        optional: false,
    }];
    if any_caller {
        params.push(bynk_ts::TsParam {
            name: "__caller".to_string(),
            ty: Some(bynk_ts::TsType::named("string")),
            optional: false,
        });
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
        let method_params: Vec<bynk_ts::TsParam> = h
            .params
            .iter()
            .map(|p| bynk_ts::TsParam {
                name: ts_ident(&p.name.name),
                ty: Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
                optional: false,
            })
            .collect();
        let mut call_args: Vec<bynk_ts::TsExpr> = h
            .params
            .iter()
            .map(|p| bynk_ts::TsExpr::Ident(ts_ident(&p.name.name)))
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
            });
            continue;
        }
        let deps_arg = if admission.is_some_and(|a| a.sum.is_some()) {
            bynk_ts::TsExpr::object_entries(vec![
                bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident("deps".to_string())),
                bynk_ts::TsObjectEntry::Prop(
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
                bynk_ts::TsObjectEntry::Prop(
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

/// #2039: whether `t` names any type in `renames`, at any depth.
fn type_ref_names_any(t: &TypeRef, renames: &HashMap<String, String>) -> bool {
    match t {
        TypeRef::Named(id) => renames.contains_key(&id.name),
        TypeRef::Option(inner, _) | TypeRef::List(inner, _) | TypeRef::Effect(inner, _) => {
            type_ref_names_any(inner, renames)
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            type_ref_names_any(a, renames) || type_ref_names_any(b, renames)
        }
        // #2072: a generic record (`Page[Cents]`), its head and arguments.
        TypeRef::App { name, args, .. } => {
            renames.contains_key(&name.name) || args.iter().any(|a| type_ref_names_any(a, renames))
        }
        _ => false,
    }
}

/// #2039, #2043: whether a cross-context argument holds a part the checker
/// typed as the callee's type rather than the caller's: a literal it may
/// have admitted to the callee's refined type, or a `List.empty()` /
/// `Map.empty()` typed from the parameter. Either sits at the argument
/// itself or in a `Some`/`Ok`/`Err` payload, list element or record field.
pub(crate) fn holds_callee_typed(e: &Expr, cx: &LowerCtx) -> bool {
    // A compile-time literal, a negated number (`-3`) included, as the
    // checker's `const_literal` reads one.
    let literal = match &e.kind {
        ExprKind::IntLit { .. } | ExprKind::FloatLit { .. } | ExprKind::StrLit(_) => true,
        ExprKind::UnaryOp(bynk_syntax::ast::UnaryOp::Neg, inner) => {
            matches!(
                inner.kind,
                ExprKind::IntLit { .. } | ExprKind::FloatLit { .. }
            )
        }
        _ => false,
    };
    match &e.kind {
        _ if literal => matches!(
            cx.commons().expr_ty(e.id).as_deref(),
            Some(Ty::Named {
                kind: NamedKind::Refined(_),
                ..
            })
        ),
        ExprKind::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            method.name == "empty"
                && args.is_empty()
                && matches!(&receiver.kind, ExprKind::Ident(id) if id.name == "List" || id.name == "Map")
        }
        ExprKind::Paren(inner)
        | ExprKind::Some(inner)
        | ExprKind::Ok(inner)
        | ExprKind::Err(inner) => holds_callee_typed(inner, cx),
        ExprKind::ListLit(items) => items.iter().any(|i| holds_callee_typed(i, cx)),
        ExprKind::RecordConstruction { fields, .. } => fields
            .iter()
            .filter_map(|f| f.value.as_ref())
            .any(|v| holds_callee_typed(v, cx)),
        _ => false,
    }
}

/// Lower a cross-context call in workers mode to a `callService(...)`
/// invocation. The argument types are looked up in the consumed context's
/// service signature so we know which serialise/deserialise helpers to
/// reference.
pub(crate) fn lower_workers_cross_context_call(
    consumed: &str,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx<'_>,
) -> Lowered {
    use crate::emitter::serialisation::{deserialise_ref_via, serialise_expr_via};

    let mut pre = Pre::new();

    let info = cx.cross_context();
    let binding = crate::emitter::wrangler::service_binding(consumed).name;

    // Look up the service signature on the consumed context.
    //
    // v0.176 (#642, Decision C): the checker resolved this call before the
    // emitter ran, so an absent signature is the emitter disagreeing with the
    // checker — a compiler bug. It used to lower to `value as JsonValue` and an
    // `((j: any) => ({ tag: "Ok", value: j }))` identity deserialiser, shipping
    // an unvalidated `any` to production to paper over that bug. Fail instead.
    let svc = info
        .consumed_services
        .get(consumed)
        .and_then(|map| map.get(&method.name))
        .unwrap_or_else(|| {
            panic!(
                "bynk.emit.unresolved_cross_context_signature: no signature for \
                 `{consumed}.{}` at emit time, though the checker resolved the call",
                method.name
            )
        });

    // #661 (ADR 0199 Decision G discharged): a context's own handlers generate
    // their *own* codecs for the contracts they participate in
    // (`emit_boundary_helpers`), so the call site reaches them **locally** — no
    // namespace prefix. The consumed context's module is then imported for types
    // only (`import type * as <ns>`), which is what makes each Worker
    // self-contained: `commerce-orders` no longer bundles `commerce-payment`'s
    // provider implementation.
    //
    // The generated **test harness** (`tests/*.test.ts`) is the exception: it
    // imports every participating Worker's `handlers.js` as a real value module
    // (it wires the `env` bindings from them) and does *not* generate its own
    // codecs, so there it still reaches the callee's codecs through that value
    // namespace. `in_test_scaffold()` is exactly that discriminator.
    let ns = if cx.in_test_scaffold() {
        format!("{}.", test_scaffold_ns(consumed))
    } else {
        String::new()
    };

    // One invariant for arity, asserted once, in both directions. The checker
    // validated it (`bynk.consumes.service_arity`) before emit ran, so a mismatch
    // either way is the emitter disagreeing with the checker — the same internal
    // fault the signature lookup above asserts on, and it gets the same answer.
    // (Previously the two directions disagreed: a surplus argument panicked, while
    // a missing one silently emitted `null` into the args object — quietly sending
    // a wrong payload for exactly the fault the other direction refused to ship.)
    assert_eq!(
        args.len(),
        svc.params.len(),
        "bynk.emit.unresolved_cross_context_signature: `{consumed}.{}` takes {} argument(s) \
         but the call site has {}, though the checker accepted the call's arity",
        method.name,
        svc.params.len(),
        args.len(),
    );

    // #1932: a type of the callee's this consumer neither imports nor names
    // has its codecs generated here under a renamed name (#1846); the call
    // names them the same way. Not in a test scaffold, which reaches the
    // callee's own codecs through its namespace.
    let renames = info
        .consumed_type_renames
        .get(consumed)
        .filter(|_| ns.is_empty());
    let renamed = |t: &TypeRef| match renames {
        Some(r) => crate::emitter::serialisation::rename_type_ref(t, r),
        None => t.clone(),
    };
    let mut args_serialised: Vec<bynk_ts::TsExpr> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        let mut lowered = pre.lower(a, cx);
        let (_, param_ty) = &svc.params[i];
        let wire_ty = renamed(param_ty);
        // #2025, #2039: a literal in a cross-context argument (directly, or
        // nested in a payload, list or record field) is admitted to the
        // callee's type, but lowers with the brand of the caller's name for
        // it. Where the caller doesn't name the callee's type (it was renamed
        // above), cast the argument to what the codec takes.
        if renames.is_some_and(|r| type_ref_names_any(param_ty, r)) && holds_callee_typed(a, cx) {
            let codec = bynk_ts::print_expr(&crate::emitter::serialisation::serialise_ref_via(
                &wire_ty,
                &ns,
                cx.runtime_use(),
            ));
            let codec_param = ["globalThis.Parameters<typeof ", codec.as_str(), ">[0]"].concat();
            lowered = bynk_ts::print_expr(&bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
                expr: Box::new(bynk_ts::TsExpr::As {
                    expr: Box::new(bynk_ts::TsExpr::Ident(lowered)),
                    ty: bynk_ts::TsType::named("unknown"),
                }),
                ty: bynk_ts::TsType::named(codec_param),
            })));
        } else {
            // #2035: a value the caller builds for a named parameter type
            // carries the caller's brand on its commons parts (`Map.empty()`
            // lowers to `new globalThis.Map<string, Line>()` with this
            // context's `Line`), while the codec takes the callee's type
            // (`cs_store.Batch`, holding the callee's `Line`), so tsc rejected
            // it (TS2345). Cast through `unknown` to the callee's type, as the
            // bundle path does ([`param_cast`]); the checker has already
            // matched the shapes. An admitted literal (above) already has its
            // one cast, to what the codec takes, so it gets no second. Only a
            // named parameter type is cast here (`cast_generic: false`): a
            // generic argument goes to this context's own instantiation codec,
            // whose commons types are unbranded (#2027).
            lowered = param_cast(
                consumed,
                info,
                method,
                i,
                lowered,
                false,
                cx.in_test_scaffold(),
            );
        }
        args_serialised.push(serialise_expr_via(
            &wire_ty,
            &lowered,
            &ns,
            cx.runtime_use(),
        ));
    }
    // Arc C, slice 29 (#1390): `args_json`, `deser_ref`, and the final call
    // itself now build a real `bynk_ts::TsExpr::Call`, printed through the
    // new `bynk_ts::print_expr` (#1388) and handed to `pre.finish` exactly
    // as the old hand-built string was — the P7.9 pattern (`-> Lowered`
    // stays unchanged; `lower.rs`'s own general dispatch, `pre.absorb`'s own
    // caller, still just wants a plain string). #1435 (Arc E slice 1):
    // `args_serialised`'s own entries and `deser_ref` (below) now come from
    // `serialise_expr_via`/`deserialise_ref_via` as real `bynk_ts::TsExpr`
    // trees, not opaque `TsExpr::Ident` text — consumed directly here, no
    // print. The multi-arg object literal that wraps them is, and always
    // was, real: each param name is already a valid JS identifier (a
    // service param name), so `TsObjectEntry::Prop` applies with no
    // escaping concern. The zero-arg case is the SAME `{  }` (double-space,
    // not the tight `{}` shortcut) quirk #1321 (`workers.rs`'s own `deps`
    // object) and #1327 (`project.rs`'s own `{ns}Deps`) already found and
    // carried, a third real site now — `TsExpr::Object`'s own renderer
    // always collapses an empty object to `{}`, so this one shape stays
    // opaque text too, caught here only by the zero-diff fixture check
    // (`172_integration_with_capability`), not reasoned about in the
    // abstract. Review of #1391: kept as three independent,
    // cross-referenced sites rather than one shared constant across
    // `emit.rs`/`workers.rs`/`project.rs` — each already carries its own
    // explanatory comment naming the others, and a shared constant's only
    // real payoff (staying in sync if this quirk is ever normalised away)
    // is speculative, not a real need today.
    let args_json_expr = if args_serialised.is_empty() {
        bynk_ts::TsExpr::Ident("{  }".to_string())
    } else if args_serialised.len() == 1 {
        args_serialised.into_iter().next().unwrap()
    } else {
        let entries: Vec<bynk_ts::TsObjectEntry> = svc
            .params
            .iter()
            .zip(args_serialised)
            .map(|((name, _), serialised)| bynk_ts::TsObjectEntry::Prop(name.clone(), serialised))
            .collect();
        bynk_ts::TsExpr::object_entries(entries)
    };

    let deser_ref = deserialise_ref_via(&renamed(&svc.return_type), &ns, cx.runtime_use());

    // v0.177 (#643): stamp this context's compiled view of the callee's
    // contract, so the callee can fail closed when the deployed pair disagree.
    //
    // Canonicalised in the **callee's** namespace, from the callee's own type
    // table (`consumed_types[consumed]`) — never this context's. The callee
    // builds the same table for itself via the same `combined_types_for`, so the
    // two hashes agree by construction on a single build and differ only when
    // the deployed sources genuinely differ. Canonicalising here in the caller's
    // namespace would rebrand the same types and 409 every call.
    let contract = match info.consumed_types.get(consumed) {
        Some(types) => bynk_check::contract::service_contract_hash(svc, types),
        // Unreachable alongside the signature assertion above: the same resolver
        // pass populates both maps for a consumed context.
        None => unreachable!(
            "consumed_types missing for `{consumed}`, though its service signature resolved"
        ),
    };

    let call_expr = bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Ident("__callService".to_string())),
        args: vec![
            bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("deps".to_string())),
                    property: "env".to_string(),
                }),
                property: binding,
            },
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(method.name.clone())),
            args_json_expr,
            deser_ref,
            // v0.54: stamp the calling context's qualified name so the
            // callee's `by c: Caller` handler reads a live `CallerId` (Q7).
            // A compile-time constant; the args body is unchanged. Raw
            // text, not `escape_ts_string`'s own pre-escaped output —
            // `TsLit::Str`'s own renderer applies the identical escaping
            // itself (documented byte-for-byte match, #1388).
            //
            // I3 (#1971): a system test case that calls `by <ForeignActor>`
            // the service admits signs as that foreign caller instead, under
            // the secret the harness gives it.
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(match cx.call_site_actor.as_ref() {
                Some(actor) if svc.foreign_callers.contains(actor) => actor.clone(),
                _ => cx.commons().commons.name.joined(),
            })),
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(contract)),
            // I2 (#1971): the calling Worker's `env`, which holds the key for
            // the edge to the callee, and the callee's qualified name, which
            // the signature binds the request to; the runtime signs with them,
            // and refuses to send without the key.
            bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("deps".to_string())),
                property: "env".to_string(),
            },
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(consumed.to_string())),
        ],
    };
    pre.finish(bynk_ts::print_expr(&call_expr))
}

/// If `receiver` is a dotted chain or single ident that matches one of the
/// current context's `consumes` clauses (by alias or qualified name), return
/// the consumed context's qualified name plus the surface key used to access
/// it through `deps.surface.<key>`.
pub(crate) fn cross_context_lowering_prefix(
    receiver: &Expr,
    cx: &LowerCtx<'_>,
) -> Option<(String, String)> {
    let chain = flatten_emit_ident_chain(receiver)?;
    let info = cx.cross_context();
    if info.consumed_contexts.is_empty() && info.aliases.is_empty() {
        return None;
    }
    let consumed = info.resolve_prefix(&chain)?;
    // Surface key: prefer the alias if there is one, else the last segment.
    let mut alias_for: HashMap<String, String> = HashMap::new();
    for (alias, target) in &info.aliases {
        alias_for.insert(target.clone(), alias.clone());
    }
    let key = alias_for.get(&consumed).cloned().unwrap_or_else(|| {
        consumed
            .rsplit('.')
            .next()
            .unwrap_or(consumed.as_str())
            .to_string()
    });
    Some((consumed, key))
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

/// Cast an argument crossing a context boundary to the consumed context's
/// type. For named types we emit `arg as <ns>.<TypeName>`. For other types
/// (base, ()), no cast is needed. The structural compatibility check at the
/// bynk layer guarantees the cast is sound.
///
/// #1773 review: `in_test_scaffold` qualifies through [`test_scaffold_ns`], the
/// namespace a test module binds. No test body reaches this today (below
/// `system` a case may not call another context,
/// `bynk.tier.cross_context_needs_system`, and a `system` case lowers through
/// the Workers path), but a test module's namespace references all go through
/// that one helper.
pub(crate) fn param_cast(
    consumed: &str,
    info: &bynk_check::resolver::CrossContextInfo,
    method: &Ident,
    idx: usize,
    arg: String,
    // Whether an argument at a generic parameter that names a user type is
    // cast too (the bundle path, #2039, #2042); the workers path casts only a
    // named parameter type, since its instantiation codecs are unbranded.
    cast_generic: bool,
    in_test_scaffold: bool,
) -> String {
    let Some(svcs) = info.consumed_services.get(consumed) else {
        return arg;
    };
    let Some(service) = svcs.get(&method.name) else {
        return arg;
    };
    let Some((_, ptype_ref)) = service.params.get(idx) else {
        return arg;
    };
    if let Some(name) = type_ref_named_root(ptype_ref) {
        // #2073: a type the consumed context itself consumes is named through
        // its declaring context's namespace. This relies on the checker
        // refusing a caller that does not consume that context
        // (`bynk.consumes.signature_type_unconsumed`), so the namespace is
        // always imported here.
        let owner = info
            .consumed_type_owners
            .get(consumed)
            .and_then(|o| o.get(name))
            .map(String::as_str)
            .unwrap_or(consumed);
        let ns = if in_test_scaffold {
            test_scaffold_ns(owner)
        } else {
            qualified_to_ns(owner)
        };
        // v0.9.1: when both contexts brand the same commons type (e.g., both
        // see `Money` with their own `__ctxBrand`), a direct
        // `as <ns>.<Type>` cast is rejected by `tsc --strict` because the
        // brand discriminants are incompatible. Bynk guarantees the value's
        // base type matches at the boundary, so route through `unknown` to
        // tell TypeScript to trust the structural Bynk-side check.
        //
        // Arc C, slice 29 (#1390): `arg as unknown as {ns}.{name}` is a
        // nested `As`-under-`As` chain — the same shape #1385 (slice 27)
        // already found the printer's own operand-parenthesisation rule
        // would wrongly wrap in an extra pair (`(arg as unknown) as T`
        // instead of the real, parenless `arg as unknown as T`) — so the
        // whole double cast stays one opaque leaf, wrapped in a real
        // `TsExpr::Paren` for the real outer parens, printed through
        // `bynk_ts::print_expr` (#1388) — now tagged `VerbatimExpr`, not
        // `Ident` (#1539).
        return bynk_ts::print_expr(&bynk_ts::TsExpr::Paren(Box::new(
            bynk_ts::TsExpr::VerbatimExpr(
                format!("{arg} as unknown as {ns}.{name}"),
                bynk_ts::VerbatimOrigin::Emit,
            ),
        )));
    }
    // #2039, #2042: a composite argument whose parameter names a user type
    // carries the caller's brand on it: a literal the checker admitted to the
    // callee's refined type (`Some(250)` for `Option[Cents]`), or a value the
    // caller typed itself (`byKey: Map[String, Line]`). Cast the argument to
    // the consumed context's type, every name qualified under its namespace.
    if cast_generic {
        let mut names = HashSet::new();
        type_ref_names(ptype_ref, &mut names);
        if !names.is_empty() {
            let ns = if in_test_scaffold {
                test_scaffold_ns(consumed)
            } else {
                qualified_to_ns(consumed)
            };
            let ty = crate::emitter::ts_type_ref_qualified_ts_type(ptype_ref, &names, &ns);
            return bynk_ts::print_expr(&bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
                expr: Box::new(bynk_ts::TsExpr::As {
                    expr: Box::new(bynk_ts::TsExpr::Ident(arg)),
                    ty: bynk_ts::TsType::named("unknown"),
                }),
                ty,
            })));
        }
    }
    arg
}

/// #2039: the user type names `r` mentions, at any depth.
fn type_ref_names(r: &TypeRef, out: &mut HashSet<String>) {
    match r {
        TypeRef::Named(id) => {
            out.insert(id.name.clone());
        }
        TypeRef::Option(inner, _) | TypeRef::List(inner, _) | TypeRef::Effect(inner, _) => {
            type_ref_names(inner, out)
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            type_ref_names(a, out);
            type_ref_names(b, out);
        }
        // #2072: a generic record (`Page[Cents]`), its head and arguments.
        TypeRef::App { name, args, .. } => {
            out.insert(name.name.clone());
            for a in args {
                type_ref_names(a, out);
            }
        }
        _ => {}
    }
}

/// If the type-ref names a single user type at its root, return that name.
/// (For generics like `Result[T, E]`, we don't emit a cast at the outer
/// layer — TypeScript handles the variance through the intersection.)
pub(crate) fn type_ref_named_root(r: &TypeRef) -> Option<&str> {
    match r {
        TypeRef::Named(id) => Some(id.name.as_str()),
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

/// Whether an agent emits a rehydration gate (and so imports `rehydrationViolation`).
/// Mirrors the per-field validation the gate builds, so the header import and the
/// emitted gate agree exactly (a mismatch is an unused / undefined import).
pub(crate) fn agent_needs_rehydrate(a: &AgentDecl, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    // v0.105 (slice 3b-ii): a held `Map[K, Connection]` persists `K → connId` and
    // is rehydrated like any map — no value check (the connId is opaque) but the
    // textual `K` key is validated. The `Map` arm already counts that via the key,
    // so held maps need no special handling here.
    a.store_fields
        .iter()
        .any(|f| match f.kind.head.name.as_str() {
            "Cell" | "Log" => f.kind.args.first().is_some_and(is_codecable),
            "Map" | "Cache" => {
                f.kind.args.get(1).is_some_and(is_codecable)
                    || f.kind
                        .args
                        .first()
                        .is_some_and(|k| type_base_is_string(k, types))
            }
            "Set" => f
                .kind
                .args
                .first()
                .is_some_and(|t| type_base_is_string(t, types)),
            _ => false,
        })
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
pub(crate) fn agent_has_held_storage(a: &AgentDecl) -> bool {
    !held_map_fields(a).is_empty()
}

/// True if a `store` field is a `Map[K, V]` whose value `V` is a held
/// `Connection` — the held maps split out of persistence on Workers.
fn is_held_map_field(f: &StoreField) -> bool {
    f.kind.head.name == "Map"
        && f.kind.args.len() == 2
        && bynk_check::context_checks::type_ref_is_held(&f.kind.args[1])
}

/// The agent's `store Map[K, V]` fields whose value is a held `Connection`, as
/// `(name, value-type)`. The value type is the held `Connection[F]` itself (so
/// its TS rendering is `Connection<F>`).
pub(crate) fn held_map_fields(a: &AgentDecl) -> Vec<(&Ident, &TypeRef)> {
    a.store_fields
        .iter()
        .filter(|f| is_held_map_field(f))
        .map(|f| (&f.name, &f.kind.args[1]))
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
/// Option[Connection[F]]]` program certifies (`bynkc check` exits 0), but
/// `bynkc compile` panics before reaching this function at all —
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

/// v0.119 (ADR 0155): the TS expression for a driven handler argument. An
/// `Int`-based value (bare or refined/opaque) is coerced to `number` — the
/// property generator yields `bigint`, but handler bodies do `number` arithmetic
/// (the same rule the contract attacker follows; the refinement brand is
/// compile-time only, so `Number(...)` is a no-op at runtime). Everything else —
/// `String`/refined-`String`, `Bool` — passes through.
fn history_arg_ts(p: &Param, i: usize, types: &HashMap<String, Arc<TypeDecl>>) -> String {
    if type_base_is_int(&p.type_ref, types) {
        format!("globalThis.Number(__st.args[{i}])")
    } else {
        format!("__st.args[{i}]")
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
        bynk_ir::walk_block_exprs(&h.body, &mut |e| {
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

pub(crate) fn emit_agent(
    a: &AgentDecl,
    state: &[StoreFieldIr],
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    let tys = commons.tys();
    let mut stmts = Vec::new();
    // #1187's Agent state-field slice: the checker-resolved TyId for each
    // Cell/Map/Cache/Log field's element type, keyed by field name — read
    // once here so the state-interface block below can render through
    // `ts_ty` (TyId) instead of `ts_type_ref` (raw AST) without repeating
    // this lookup at every one of its four sites.
    let store_field_ty: HashMap<&str, &StoreKindIr> =
        state.iter().map(|f| (f.field.as_str(), &f.kind)).collect();
    if let Some(doc) = a.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let state_ty = format!("{}State", a.name.name);
    // v0.81 (storage track, ADR 0109): an agent's `Cell` fields ARE its state
    // record, so the whole state machinery (interface, zero factory, load/commit,
    // invariant gate) derives the record fields from the cells. Each `Cell[T]`
    // field becomes a `T`-typed record field carrying the cell's initialiser.
    // Handler bodies lower as bare reads / `:=` over `__state`, with an implicit
    // commit at handler end.
    let is_store_agent = true;
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
    let held_maps: Vec<(&Ident, &TypeRef)> = if is_workers {
        held_map_fields(a)
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
    let held_maps_ts: HashMap<String, String> = held_maps
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
                    StoreKindIr::Map(_, v_ty) => held_frame_ty(*v_ty, tys),
                    _ => None,
                })
                .map(|frame_ty| ts_ty(frame_ty, tys))
                .unwrap_or_else(|| ts_type_ref(held_frame_ty_ref(v)));
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
    let map_value_ts: HashMap<String, String> = store_map_fields
        .iter()
        .map(|(n, v)| {
            let ts = store_field_ty
                .get(n.name.as_str())
                .and_then(|kind| match kind {
                    StoreKindIr::Map(_, v_ty) => Some(ts_ty(*v_ty, tys)),
                    _ => None,
                })
                .unwrap_or_else(|| ts_type_ref(v));
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
            .filter(|f| f.kind.head.name == "Cache" && f.kind.args.len() == 2)
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
            .filter(|f| f.kind.head.name == "Log" && f.kind.args.len() == 1)
            .map(|f| {
                let retain = store_field_ty
                    .get(f.name.name.as_str())
                    .and_then(|kind| match kind {
                        StoreKindIr::Log(_, retain) => *retain,
                        _ => None,
                    });
                (&f.name, &f.kind.args[0], retain)
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
    let log_value_ts: HashMap<String, String> = store_log_fields
        .iter()
        .map(|(n, v, _)| {
            let ts = store_field_ty
                .get(n.name.as_str())
                .and_then(|kind| match kind {
                    StoreKindIr::Log(elem_ty, _) => Some(ts_ty(*elem_ty, tys)),
                    _ => None,
                })
                .unwrap_or_else(|| ts_type_ref(v));
            (n.name.clone(), ts)
        })
        .collect();
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
    for f in &effective_fields {
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
    for (name, _) in &store_map_fields {
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
                "globalThis.Record",
                vec![
                    bynk_ts::TsType::named("string"),
                    ts_ty_to_ts_type(value_ty, tys),
                ],
            ),
            optional: false,
            readonly: true,
        });
    }
    // v0.105 (slice 3b-ii): a held `Map[K, Connection]` persists `K → connId`.
    for (name, _) in &held_maps {
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "globalThis.Record",
                vec![
                    bynk_ts::TsType::named("string"),
                    bynk_ts::TsType::named("string"),
                ],
            ),
            optional: false,
            readonly: true,
        });
    }
    for name in &set_field_names {
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.to_string(),
            ty: bynk_ts::TsType::named_with_args(
                "globalThis.Record",
                vec![
                    bynk_ts::TsType::named("string"),
                    bynk_ts::TsType::named("boolean"),
                ],
            ),
            optional: false,
            readonly: true,
        });
    }
    // v0.93 (ADR 0118): a sibling posting-list per `@indexed(by: f)` — field
    // value (stringified) → the primary keys whose value has it. Persisted and
    // committed wholesale with the map it indexes.
    for (map, fields) in sorted_index_fields(&store_map_indexes) {
        for f in fields {
            members.push(bynk_ts::TsTypeMember::Prop {
                name: format!("{map}__idx_{f}"),
                ty: bynk_ts::TsType::named_with_args(
                    "globalThis.Record",
                    vec![
                        bynk_ts::TsType::named("string"),
                        // Postfix `string[]`, not `named_with_args("Array",
                        // ..)` — the original text here is `string[]`, not
                        // `Array<string>`. Review of #1368: the two are
                        // byte-different even though `Cache`/`Log` below use
                        // the generic `Array<T>` form for their own arrays —
                        // don't "unify" this with those without re-checking
                        // the fixture text first.
                        bynk_ts::TsType::Array {
                            element: Box::new(bynk_ts::TsType::named("string")),
                            readonly: false,
                        },
                    ],
                ),
                optional: false,
                readonly: true,
            });
        }
    }
    for (name, _, _) in &store_cache_fields {
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
                "globalThis.Record",
                vec![
                    bynk_ts::TsType::named("string"),
                    bynk_ts::TsType::Object(vec![
                        bynk_ts::TsTypeMember::Prop {
                            name: "v".to_string(),
                            ty: ts_ty_to_ts_type(value_ty, tys),
                            optional: false,
                            readonly: false,
                        },
                        bynk_ts::TsTypeMember::Prop {
                            name: "exp".to_string(),
                            ty: bynk_ts::TsType::named("number"),
                            optional: false,
                            readonly: false,
                        },
                    ]),
                ],
            ),
            optional: false,
            readonly: true,
        });
    }
    for (name, _, _) in &store_log_fields {
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
                "globalThis.Array",
                vec![bynk_ts::TsType::Object(vec![
                    bynk_ts::TsTypeMember::Prop {
                        name: "t".to_string(),
                        ty: bynk_ts::TsType::named("number"),
                        optional: false,
                        readonly: false,
                    },
                    bynk_ts::TsTypeMember::Prop {
                        name: "v".to_string(),
                        ty: ts_ty_to_ts_type(elem_ty, tys),
                        optional: false,
                        readonly: false,
                    },
                ])],
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
    // v0.9.2: per-agent state registry (bundle mode + `bynkc test`) and the
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
    for f in &effective_fields {
        let val = if let Some(init) = &f.init {
            let mut pre = Pre::new();
            let mut module = ModuleCtx::new(commons, &ctx.cross_context, &ctx.runtime_use);
            module.target = ctx.target;
            module.agent_method_givens = ctx.agent_method_givens.clone();
            module.agent_called_contexts = ctx.agent_called_contexts.clone();
            module.event_schema_versions = ctx.event_schema_versions.clone();
            module.consumed_type_ns = consumed_type_ns(ctx);
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
            bynk_check::checker::zero_value_ts(&f.type_ref, f.refinement.as_ref(), &commons.types)
                .unwrap_or_else(|| "undefined as never".to_string())
        };
        if val.contains('\n') {
            zero_record_has_multiline_value = true;
        }
        zero_entries.push(bynk_ts::TsObjectEntry::Prop(
            f.name.name.clone(),
            bynk_ts::TsExpr::Ident(val),
        ));
    }
    // A fresh `store Map`/`store Set`/`store Cache` is the empty record.
    for (name, _) in &store_map_fields {
        zero_entries.push(bynk_ts::TsObjectEntry::Prop(
            name.name.clone(),
            bynk_ts::TsExpr::object(vec![]),
        ));
    }
    // A fresh held `Map[K, Connection]` (a `K → connId` record) is empty too.
    for (name, _) in &held_maps {
        zero_entries.push(bynk_ts::TsObjectEntry::Prop(
            name.name.clone(),
            bynk_ts::TsExpr::object(vec![]),
        ));
    }
    // A fresh `@indexed` posting-list is empty too (v0.93, ADR 0118).
    for (map, fields) in sorted_index_fields(&store_map_indexes) {
        for f in fields {
            zero_entries.push(bynk_ts::TsObjectEntry::Prop(
                format!("{map}__idx_{f}"),
                bynk_ts::TsExpr::object(vec![]),
            ));
        }
    }
    for name in &set_field_names {
        zero_entries.push(bynk_ts::TsObjectEntry::Prop(
            name.to_string(),
            bynk_ts::TsExpr::object(vec![]),
        ));
    }
    for (name, _, _) in &store_cache_fields {
        zero_entries.push(bynk_ts::TsObjectEntry::Prop(
            name.name.clone(),
            bynk_ts::TsExpr::object(vec![]),
        ));
    }
    // A fresh `store Log` is the empty array.
    for (name, _, _) in &store_log_fields {
        zero_entries.push(bynk_ts::TsObjectEntry::Prop(
            name.name.clone(),
            bynk_ts::TsExpr::array(vec![]),
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
    let mut encode_entries: Vec<String> = Vec::new();
    let agent_name = &a.name.name;
    let mut rehydrate_checks: Vec<String> = Vec::new();
    // The loaded record is statically typed (its fields are the agent's types),
    // but at runtime its bytes are untrusted, so each value is laundered to
    // `JsonValue` before the boundary deserialiser decodes it.
    let present =
        |name: &str| format!("globalThis.Object.prototype.hasOwnProperty.call(stored, \"{name}\")");
    let ser = |ty: &TypeRef, value: &str| {
        bynk_ts::print_expr(&serialisation::serialise_expr(ty, value, &ctx.runtime_use))
    };
    let push_value_check = |checks: &mut Vec<String>,
                            ty: &TypeRef,
                            value_expr: &str,
                            path: &str| {
        // Only codec-able types have a deserialiser; a non-storable type never
        // reaches a `store` position (the checker rejects it), but guard anyway.
        if !is_codecable(ty) {
            return;
        }
        let json = format!("({value_expr} as unknown as __JsonValue)");
        // #1435 (Arc E slice 1): `deserialise_expr` now returns a real
        // `bynk_ts::TsExpr` — this closure stays `format!`-based (splicing
        // into one `TsStmt::Raw` check line below), so it prints at the
        // boundary, the same "opaque consumer needs its own print"
        // treatment `lower.rs`/`tests_emit.rs`'s own call sites use.
        let d = bynk_ts::print_expr(&serialisation::deserialise_expr(
            ty,
            &json,
            path,
            &ctx.runtime_use,
        ));
        checks.push(format!(
            // The state interface's fields are `readonly`; the decode writes
            // through a mutable view of the freshly merged record. The cast to
            // the field's own type matters on the workers target, where the
            // state interface re-declares a commons type locally and
            // TypeScript's brands make the two nominally distinct, though
            // they share one runtime shape.
            "  if ({p}) {{ const __r = {d}; if (__r.tag === \"Err\") throw __rehydrationViolation(\"{agent_name}\", __r.error); (s as {{ -readonly [K in keyof typeof s]: (typeof s)[K] }}).{path} = __r.value as unknown as (typeof s)[\"{path}\"]; }}",
            p = present(path),
        ));
    };
    // `Cell[T]` — decode the field value as `T`.
    for f in &effective_fields {
        push_value_check(
            &mut rehydrate_checks,
            &f.type_ref,
            &format!("s.{}", f.name.name),
            &f.name.name,
        );
        if is_codecable(&f.type_ref) {
            encode_entries.push(format!(
                "{n}: {e}",
                n = f.name.name,
                e = ser(&f.type_ref, &format!("s.{}", f.name.name)),
            ));
        }
    }
    // `Map[K, V]` — validate each entry value against `V`, and each key against
    // `K` when `K` is textual (the key persists as a `String(k)` Record key).
    for (name, v) in &store_map_fields {
        if is_codecable(v) {
            rehydrate_checks.push(format!(
                "  if ({p}) for (const __k of globalThis.Object.keys(s.{n})) {{ const __r = {d}; if (__r.tag === \"Err\") throw __rehydrationViolation(\"{agent_name}\", __r.error); s.{n}[__k] = __r.value as unknown as (typeof s.{n})[string]; }}",
                p = present(&name.name),
                n = name.name,
                d = bynk_ts::print_expr(&serialisation::deserialise_expr(
                    v,
                    &format!("(s.{}[__k] as unknown as __JsonValue)", name.name),
                    &name.name,
                    &ctx.runtime_use,
                )),
            ));
            encode_entries.push(format!(
                "{n}: globalThis.Object.fromEntries(globalThis.Object.entries(s.{n}).map(([__k, __v]) => [__k, {e}]))",
                n = name.name,
                e = ser(v, "__v"),
            ));
        }
        if let Some(k) = store_field_key_type(a, &name.name)
            && type_base_is_string(k, &commons.types)
        {
            rehydrate_checks.push(format!(
                "  for (const __k of globalThis.Object.keys(s.{n})) {{ const __r = {d}; if (__r.tag === \"Err\") throw __rehydrationViolation(\"{agent_name}\", __r.error); }}",
                n = name.name,
                d = bynk_ts::print_expr(&serialisation::deserialise_expr(k, "(__k as unknown as __JsonValue)", &name.name, &ctx.runtime_use)),
            ));
        }
    }
    // v0.105 (slice 3b-ii): a held `Map[K, Connection]` persists `K → connId`. The
    // connId is an opaque platform string (no value check); validate each textual
    // `K` key, as for a plain map.
    for (name, _) in &held_maps {
        if let Some(k) = store_field_key_type(a, &name.name)
            && type_base_is_string(k, &commons.types)
        {
            rehydrate_checks.push(format!(
                "  for (const __k of globalThis.Object.keys(s.{n})) {{ const __r = {d}; if (__r.tag === \"Err\") throw __rehydrationViolation(\"{agent_name}\", __r.error); }}",
                n = name.name,
                d = bynk_ts::print_expr(&serialisation::deserialise_expr(k, "(__k as unknown as __JsonValue)", &name.name, &ctx.runtime_use)),
            ));
        }
    }
    // `Set[T]` — the elements are the (textual) Record keys; validate when `T`
    // is textual, else defer (structural string key).
    for (name, t) in &store_set_fields {
        if type_base_is_string(t, &commons.types) {
            rehydrate_checks.push(format!(
                "  for (const __k of globalThis.Object.keys(s.{n})) {{ const __r = {d}; if (__r.tag === \"Err\") throw __rehydrationViolation(\"{agent_name}\", __r.error); }}",
                n = name.name,
                d = bynk_ts::print_expr(&serialisation::deserialise_expr(t, "(__k as unknown as __JsonValue)", &name.name, &ctx.runtime_use)),
            ));
        }
    }
    // `Cache[K, V]` — validate each entry's `.v` against `V`.
    for (name, v, _) in &store_cache_fields {
        if is_codecable(v) {
            rehydrate_checks.push(format!(
                "  if ({p}) for (const __e of globalThis.Object.values(s.{n})) {{ const __r = {d}; if (__r.tag === \"Err\") throw __rehydrationViolation(\"{agent_name}\", __r.error); __e.v = __r.value as unknown as typeof __e.v; }}",
                p = present(&name.name),
                n = name.name,
                d = bynk_ts::print_expr(&serialisation::deserialise_expr(v, "(__e.v as unknown as __JsonValue)", &name.name, &ctx.runtime_use)),
            ));
            encode_entries.push(format!(
                "{n}: globalThis.Object.fromEntries(globalThis.Object.entries(s.{n}).map(([__k, __e]) => [__k, {{ ...__e, v: {e} }}]))",
                n = name.name,
                e = ser(v, "__e.v"),
            ));
        }
    }
    // `Log[T]` — validate each entry's `.v` against `T`.
    for (name, t, _) in &store_log_fields {
        if is_codecable(t) {
            rehydrate_checks.push(format!(
                "  if ({p}) for (const __e of s.{n}) {{ const __r = {d}; if (__r.tag === \"Err\") throw __rehydrationViolation(\"{agent_name}\", __r.error); __e.v = __r.value as unknown as typeof __e.v; }}",
                p = present(&name.name),
                n = name.name,
                d = bynk_ts::print_expr(&serialisation::deserialise_expr(t, "(__e.v as unknown as __JsonValue)", &name.name, &ctx.runtime_use)),
            ));
            encode_entries.push(format!(
                "{n}: s.{n}.map((__e) => ({{ ...__e, v: {e} }}))",
                n = name.name,
                e = ser(t, "__e.v"),
            ));
        }
    }
    let has_rehydrate = agent_needs_rehydrate(a, &commons.types);
    // State-migrations S1 (#2211): this class's lock version facts, present
    // only when the build has the schema lock (Decision A).
    let agent_versions = ctx.agent_schema_versions.get(&a.name.name).cloned();
    if has_rehydrate {
        // Each check is already-formed JS text (`serialisation::
        // deserialise_expr`, a confirmed unaffected, `String`-returning
        // sibling helper, out of this slice's scope) — carried as its own
        // opaque `TsStmt::Raw`, not merged into one blob: each is already a
        // real, independent statement, and `Raw` prints verbatim with no
        // indent of its own, matching every check's own hardcoded two-space
        // prefix exactly (correct because this decl is always printed at
        // depth 0, giving the body depth 1).
        let rehydrate_fn_decl = bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Function {
                name: rehydrate_fn.clone(),
                generics: Vec::new(),
                params: vec![
                    bynk_ts::TsParam {
                        name: "s".to_string(),
                        ty: Some(bynk_ts::TsType::named(state_ty.clone())),
                        optional: false,
                    },
                    // #1649: the stored record, so a field absent from it keeps
                    // its in-memory zero instead of being decoded (ADR 0124 D4).
                    bynk_ts::TsParam {
                        name: "stored".to_string(),
                        ty: Some(bynk_ts::TsType::named(state_ty.clone())),
                        optional: false,
                    },
                ],
                return_type: Some(bynk_ts::TsType::named("void")),
                body: rehydrate_checks
                    .iter()
                    .map(|c| bynk_ts::TsStmt::raw(format!("{c}\n"), None))
                    .collect(),
                is_async: false,
                inline: false,
            },
            None,
        );
        // #1486: no more explicit trailing `TsStmt::blank(None)` — see
        // `emit_refined_type`'s own identical note.
        stmts.push(rehydrate_fn_decl);
        // #1649: the wire-shape encoder `commitState` writes through. Every
        // codec-able value position is serialised; the rest of the record (held
        // maps, `Set`s, `@indexed` posting lists, unknown stored keys) is
        // already wire-shaped and spreads through unchanged.
        let mut encode_body = String::from("  return { ...s");
        for e in &encode_entries {
            encode_body.push_str(", ");
            encode_body.push_str(e);
        }
        encode_body.push_str(" };\n");
        stmts.push(bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Function {
                name: encode_fn.clone(),
                generics: Vec::new(),
                params: vec![bynk_ts::TsParam {
                    name: "s".to_string(),
                    ty: Some(bynk_ts::TsType::named(state_ty.clone())),
                    optional: false,
                }],
                return_type: Some(bynk_ts::TsType::named("globalThis.Record<string, unknown>")),
                body: vec![bynk_ts::TsStmt::raw(encode_body, None)],
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
            bynk_ts::print_expr(&serialisation::serialise_expr(ty, value, &ctx.runtime_use))
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
        let mut dispatch = String::new();
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
                ModuleCtx::new(commons, &ctx.cross_context, &ctx.runtime_use),
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
                        match bynk_check::checker::zero_value_ts(
                            &f.type_ref,
                            f.refinement.as_ref(),
                            &commons.types,
                        ) {
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
                        for s in pre.stmts() {
                            writeln!(body, "    {s}").unwrap();
                        }
                        let js = cx.bind_local_name(&l.name.name);
                        writeln!(body, "    const {js} = {v};").unwrap();
                    }
                    bynk_syntax::ast::MigrateItem::Assign(asg) => {
                        let mut pre = Pre::new();
                        let v = pre.lower(&asg.value, &mut cx);
                        for s in pre.stmts() {
                            writeln!(body, "    {s}").unwrap();
                        }
                        let n = &asg.target.name;
                        let v = if map_names.contains(n) {
                            // A `Map` value is a `ReadonlyMap`; a store `Map`
                            // is a `Record` keyed by `String(k)`.
                            format!(
                                "globalThis.Object.fromEntries(globalThis.Array.from({v}, ([__k, __v]) => [globalThis.String(__k), __v]))"
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
            let enc = if has_rehydrate {
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
            writeln!(
                dispatch,
                "  if (__v >= {lo} && __v <= {from}) return {step_fn}(stored);",
                lo = plan.lo,
                from = plan.from
            )
            .unwrap();
        }
        // The nearest step covering the stored version runs; a version no step
        // covers reads by zero-merge (it passed `__checkStoredSchemaVersion`).
        stmts.push(bynk_ts::TsStmt::raw(
            format!(
                "function {migrate_fn}(stored: globalThis.Record<string, unknown>): globalThis.Record<string, unknown> {{\n  const __v = stored[\"__v\"] === undefined ? 1 : stored[\"__v\"] as number;\n{dispatch}  return stored;\n}}\n"
            ),
            None,
        ));
    }
    // 2) Durable Object class.
    //
    // #1482: local buffer/map pair, replacing this function's own former
    // `out`/`source_map` parameters for the rest of the class's own
    // construction below (through its closing brace) — every internal
    // print-and-splice call site inside the class (`loadState`/
    // `commitState`/the per-handler loop/the WS-hosted DO methods/`fetch`)
    // is unchanged, since each already just referenced these two names.
    let mut out = String::new();
    let class_smb = RefCell::new(SourceMapBuilder::new());
    let source_map = Some(&class_smb);
    writeln!(out, "export class {name} {{", name = a.name.name).unwrap();
    writeln!(out, "  state: __DurableObjectState;").unwrap();
    let reads_key = agent_reads_key(a);
    if reads_key {
        // #1818: set before any handler runs, by the bundle factory or the
        // workers `fetch` dispatch.
        writeln!(
            out,
            "  __key!: {};",
            crate::emitter::ts_type_ref(&a.key_type)
        )
        .unwrap();
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
        .any(|h| bynk_ir::block_uses_emit(&h.body, &commons.callees));
    // #1960: likewise `deps.env` itself — its Service Bindings and DO
    // namespaces are dead plain objects once the caller's deps cross the
    // wire, so an agent whose handlers call a consumed context's service or
    // construct an agent reads them from its own `env` instead.
    let agent_reaches_env = ctx.target == BuildTarget::Workers
        && a.handlers
            .iter()
            .any(|h| bynk_ir::block_reaches_worker_env(&h.body, &commons.callees));
    let needs_env_ctor = given_deps_expr.is_some() || agent_uses_emit || agent_reaches_env;
    if needs_env_ctor {
        writeln!(out, "  private __env: unknown;").unwrap();
        writeln!(
            out,
            "  constructor(state: __DurableObjectState, env?: unknown) {{"
        )
        .unwrap();
        writeln!(out, "    this.state = state;").unwrap();
        writeln!(out, "    this.__env = env;").unwrap();
        writeln!(out, "  }}").unwrap();
    } else {
        writeln!(out, "  constructor(state: __DurableObjectState) {{").unwrap();
        writeln!(out, "    this.state = state;").unwrap();
        writeln!(out, "  }}").unwrap();
    }
    writeln!(out).unwrap();
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
                    .chain((!step_plans.is_empty()).then(|| {
                        bynk_ts::TsExpr::Array {
                            items: step_plans
                                .iter()
                                .map(|s| bynk_ts::TsExpr::Array {
                                    items: vec![
                                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(s.lo.to_string())),
                                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(
                                            s.from.to_string(),
                                        )),
                                    ],
                                    multiline: false,
                                })
                                .collect(),
                            multiline: false,
                        }
                    }))
                    .collect(),
                },
                None,
            ));
        }
        // State-migrations S2 (#2224 Decision D): a record a step covers runs
        // it first; everything below reads the step's output.
        let src = if step_plans.is_empty() {
            "stored"
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
                            bynk_ts::TsExpr::Ident("stored".to_string()),
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
        stmts.push(bynk_ts::TsStmt::return_stmt(
            Some(bynk_ts::TsExpr::Ident("__merged".to_string())),
            None,
        ));
        stmts
    };
    let load_state_method = bynk_ts::TsClassMethod {
        name: "loadState".to_string(),
        private: true,
        is_async: true,
        params: Vec::new(),
        return_type: Some(bynk_ts::TsType::named_with_args(
            "globalThis.Promise",
            vec![bynk_ts::TsType::named(state_ty.clone())],
        )),
        doc: None,
        body: load_state_body,
    };
    let class_depth = 0;
    out.push_str(&bynk_ts::print_class_method(
        &load_state_method,
        class_depth,
    ));
    writeln!(out).unwrap();
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
        let build_violation_check = |name: &str,
                                     pred: String,
                                     pre_stmts: &[String],
                                     hoist_depth: usize| {
            let hoist_indent = "  ".repeat(hoist_depth);
            let mut stmts: Vec<bynk_ts::TsStmt> = pre_stmts
                .iter()
                .map(|s| bynk_ts::TsStmt::raw(format!("{hoist_indent}{s}\n"), None))
                .collect();
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
                                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(a.name.name.clone())),
                                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(name.to_string())),
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
                    ModuleCtx::new(commons, &ctx.cross_context, &ctx.runtime_use),
                    BodyMode::Invariant {
                        name: "s".to_string(),
                        fields: field_names.clone(),
                    },
                );
                let mut pre = Pre::new();
                let pred = pre.lower(&inv.predicate, &mut cx);
                stmts.extend(build_violation_check(
                    &inv.name.name,
                    pred,
                    pre.stmts(),
                    class_depth + 2,
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
                    ModuleCtx::new(commons, &ctx.cross_context, &ctx.runtime_use),
                    BodyMode::Transition {
                        old: "__old".to_string(),
                        new: "__new".to_string(),
                    },
                );
                let mut pre = Pre::new();
                let pred = pre.lower(&tr.predicate, &mut cx);
                transition_body.extend(build_violation_check(
                    &tr.name.name,
                    pred,
                    pre.stmts(),
                    class_depth + 3,
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
                callee: Box::new(bynk_ts::TsExpr::Ident("this.state.storage.put".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str("state".to_string())),
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
                        let payload = if has_rehydrate {
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
                                bynk_ts::TsObjectEntry::Prop(
                                    "__v".to_string(),
                                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(
                                        v.version.to_string(),
                                    )),
                                ),
                            ]),
                            None => payload,
                        }
                    },
                ],
            })),
            None,
        ));
        stmts
    };
    let commit_state_method = bynk_ts::TsClassMethod {
        name: "commitState".to_string(),
        private: true,
        is_async: true,
        params: vec![bynk_ts::TsParam {
            name: "s".to_string(),
            ty: Some(bynk_ts::TsType::named(state_ty.clone())),
            optional: false,
        }],
        return_type: Some(bynk_ts::TsType::named_with_args(
            "globalThis.Promise",
            vec![bynk_ts::TsType::named("void")],
        )),
        doc: None,
        body: commit_state_body,
    };
    out.push_str(&bynk_ts::print_class_method(
        &commit_state_method,
        class_depth,
    ));
    writeln!(out).unwrap();
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
            .map(|p| bynk_ts::TsParam {
                name: ts_ident(&p.name.name),
                ty: Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
                optional: false,
            })
            .collect();
        // Lower body into a buffer so we can detect cross-context usage and
        // shape the deps type accordingly.
        let mut body_out = String::new();
        // v0.70: per-statement maps for the spliced handler body (see emit_service).
        let body_smb = RefCell::new(SourceMapBuilder::new());
        // v0.81: a store-agent handler reads/writes cells over a mutable working
        // record `__state`; a state-record handler uses `currentState`/`self.state`.
        // A store handler that performs any `:=` wraps its body in a closure so an
        // implicit commit runs at handler end on every (success) return path.
        // #1196/R6.5: write detection reads the checker's own resolved
        // `Callee::Store` classification (`bynk_lower::body_writes_state`)
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
        let mut module = ModuleCtx::new(commons, &ctx.cross_context, &ctx.runtime_use);
        module.in_bynk_unit = ctx.commons_name == "bynk";
        // #1960: the body lowers for the build target, as a service body
        // does, so a cross-context call reads the Service Binding its `deps`
        // type names.
        module.target = ctx.target;
        module.agent_method_givens = ctx.agent_method_givens.clone();
        module.agent_called_contexts = ctx.agent_called_contexts.clone();
        module.event_schema_versions = ctx.event_schema_versions.clone();
        module.consumed_type_ns = consumed_type_ns(ctx);
        module.consumed_context_types = ctx.consumed_context_types();
        let mut cx = LowerCtx::new(
            module,
            BodyMode::AgentHandler {
                handler: HandlerShared {
                    capabilities: bynk_lower::lower_handler_given_ir(h)
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
        )
        .with_source_map(Some(&body_smb));
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
        emit_block_as_function_body_with_return(
            &mut body_out,
            &h.body,
            &mut cx,
            body_indent,
            async_tail,
            Some(&h.return_type),
        );
        let mut deps_members = build_deps_object_ty_with_surface(
            &effective_given(&bynk_lower::lower_handler_given_ir(h), &cx),
            &cx,
            &ctx.cross_context,
            ctx.target,
        );
        // Events track, slice 0 (spine #936): see the matching field on the
        // service path (`emit_service`) — an agent handler that emits, or
        // that calls another local agent method which itself emits, gets
        // the same compose-supplied `__eventsDispatch` callback.
        let needs_events_dispatch = cx.is_first_party_events()
            && (bynk_ir::block_uses_emit(&h.body, &commons.callees)
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
        params.push(bynk_ts::TsParam {
            name: "deps".to_string(),
            ty: Some(bynk_ts::TsType::Object(deps_members)),
            optional: false,
        });
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
                | IrHandlerKind::Event => "call".to_string(),
            });
        // Load state at entry. A state-record handler binds `currentState` and
        // commits explicitly via `commit`. A store handler binds a mutable
        // working record `__state`; reads/writes go through it, and (if it writes)
        // the body is wrapped so `commitState` runs once at handler end — the
        // implicit, atomic commit (ADR 0109). A fault before that flush persists
        // nothing; the invariant gate inside `commitState` runs before the write.
        //
        // The whole prologue+body+epilogue is captured into one local
        // `raw_body` buffer instead of `out` directly — the same
        // restructuring #1361 (`emit_service`) already made for its own
        // near-identical events-IIFE wrapping, needed here because the
        // method's own params/return-type/doc now print through
        // `print_class_method` as one fragment, which can no longer be
        // interleaved with direct `writeln!`s into `out` the way the
        // pre-conversion code was.
        let body_emits_directly = bynk_ir::block_uses_emit(&h.body, &commons.callees);
        let events_decl = format!(
            "    const __events: globalThis.Array<{}> = [];",
            crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE
        );
        let flush = "    if (__events.length > 0) { await deps.__eventsDispatch(__events); }";
        let mut raw_body = String::new();
        let body_out_offset_in_raw;
        if is_store_agent {
            if writes_state {
                writeln!(
                    raw_body,
                    "    const __state = {{ ...(await this.loadState()) }};"
                )
                .unwrap();
                if body_emits_directly {
                    writeln!(raw_body, "{events_decl}").unwrap();
                }
                writeln!(raw_body, "    const __result = await (async () => {{").unwrap();
                body_out_offset_in_raw = raw_body.len();
                raw_body.push_str(&body_out);
                writeln!(raw_body, "    }})();").unwrap();
                writeln!(raw_body, "    await this.commitState(__state);").unwrap();
                if body_emits_directly {
                    writeln!(raw_body, "{flush}").unwrap();
                }
                writeln!(raw_body, "    return __result;").unwrap();
            } else if body_emits_directly {
                writeln!(raw_body, "    const __state = await this.loadState();").unwrap();
                writeln!(raw_body, "{events_decl}").unwrap();
                writeln!(raw_body, "    const __result = await (async () => {{").unwrap();
                body_out_offset_in_raw = raw_body.len();
                raw_body.push_str(&body_out);
                writeln!(raw_body, "    }})();").unwrap();
                writeln!(raw_body, "{flush}").unwrap();
                writeln!(raw_body, "    return __result;").unwrap();
            } else {
                writeln!(raw_body, "    const __state = await this.loadState();").unwrap();
                body_out_offset_in_raw = raw_body.len();
                raw_body.push_str(&body_out);
            }
        } else if body_emits_directly {
            writeln!(raw_body, "    const currentState = await this.loadState();").unwrap();
            writeln!(raw_body, "{events_decl}").unwrap();
            writeln!(raw_body, "    const __result = await (async () => {{").unwrap();
            body_out_offset_in_raw = raw_body.len();
            raw_body.push_str(&body_out);
            writeln!(raw_body, "    }})();").unwrap();
            writeln!(raw_body, "{flush}").unwrap();
            writeln!(raw_body, "    return __result;").unwrap();
        } else {
            writeln!(raw_body, "    const currentState = await this.loadState();").unwrap();
            body_out_offset_in_raw = raw_body.len();
            raw_body.push_str(&body_out);
        }
        let handler_method = bynk_ts::TsClassMethod {
            name: method,
            private: false,
            is_async: effectful,
            params,
            return_type: Some(ret),
            doc: h.documentation.as_deref().map(str::to_owned),
            body: vec![bynk_ts::TsStmt::raw(raw_body.clone(), None)],
        };
        // #2270: anchor the method header and prologue to the handler's own
        // `on call` line. Without this they inherit the previous checkpoint,
        // so an uncalled handler's header leaked onto the preceding body's
        // last line in coverage.
        class_smb.borrow_mut().record(out.len(), h.span);
        emit_class_method_and_merge_source_map(
            &mut out,
            &handler_method,
            class_depth,
            RawBody::nested(&raw_body, &body_out, body_out_offset_in_raw),
            &body_smb,
            source_map,
        );
        writeln!(out).unwrap();
    }
    // #2270: what follows the handlers (the rest of the class and the
    // agent's module-level factory code) belongs to the agent, not to the last
    // handler's final body line, which it would otherwise inherit.
    class_smb.borrow_mut().record(out.len(), a.span);
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
                crate::emitter::ts_type_ref(&a.key_type),
            ));
        }
    }
    for host in &ws_open_hosts {
        // #2280: anchor each hosted lifecycle method's header and prologue to
        // its own `on open`/`on message`/`on close` line, not to the previous
        // method's last body line.
        class_smb.borrow_mut().record(out.len(), host.handler.span);
        emit_ws_do_method(
            &mut out,
            a,
            host,
            host.handler,
            &ws_open_do_method_name(host.service),
            commons,
            ctx,
            source_map,
        );
        if let Some(m) = host.message {
            class_smb.borrow_mut().record(out.len(), m.span);
            emit_ws_do_method(
                &mut out,
                a,
                host,
                m,
                &ws_message_do_method_name(host.service),
                commons,
                ctx,
                source_map,
            );
        }
        if let Some(c) = host.close {
            class_smb.borrow_mut().record(out.len(), c.span);
            emit_ws_do_method(
                &mut out,
                a,
                host,
                c,
                &ws_close_do_method_name(host.service),
                commons,
                ctx,
                source_map,
            );
        }
    }
    // #2280: the dispatch methods below belong to the agent, not to the last
    // hosted lifecycle method's final body line.
    class_smb.borrow_mut().record(out.len(), a.span);
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
        for host in &ws_open_hosts {
            fetch_stmts.push(ws_open_fetch_branch_stmt(host, tys));
        }
        // State-migrations S4 (#2259 Decision D): `bynk migrate sweep`'s wake,
        // a branch of its own before the method dispatch. It carries no key
        // (steps are key-free), loads the state (running any step), and
        // commits only a record older than this build's version. Reachable
        // only through this Worker's signed `/_bynk/migrate/` route.
        if let Some(v) = &agent_versions {
            fetch_stmts.push(migrate_wake_branch_stmt(v.version));
        }
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
                &crate::emitter::ts_type_ref(&a.key_type),
                "key",
            ));
        }
        // #1678: the wire arguments are decoded through this agent's table
        // before the handler sees them (`decodeAgentArgs` throws on a decode
        // failure, an internal fault), and the result is encoded on the way out.
        let decoded_args = bynk_ts::TsExpr::Ident(format!(
            "...__decodeAgentArgs({}, methodName, args)",
            agent_wire_name(&a.name.name)
        ));
        let result_expr = if needs_env_ctor {
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
            agent_dispatch_stmts.push(bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("env".to_string()),
                None,
                bynk_ts::TsExpr::Ident(
                    "this.__env as unknown as globalThis.Record<string, unknown>".to_string(),
                ),
                None,
            ));
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
                agent_dispatch_stmts.push(bynk_ts::TsStmt::const_stmt(
                    bynk_ts::TsBindingName::Ident("__givenDeps".to_string()),
                    None,
                    bynk_ts::TsExpr::Ident(expr.clone()),
                    None,
                ));
                entries.push(bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(
                    "__givenDeps".to_string(),
                )));
            }
            if agent_uses_emit {
                let bind = crate::emitter::wrangler::events_fanout_binding().name;
                // P7.2: `env` is now `Record<string, unknown>` (above), so this
                // specific binding needs its own cast to what
                // `dispatchToEventsFanout` actually declares.
                agent_dispatch_stmts.push(bynk_ts::TsStmt::const_stmt(
                    bynk_ts::TsBindingName::Ident("__eventsDeps".to_string()),
                    None,
                    bynk_ts::TsExpr::object(vec![(
                        "__eventsDispatch".to_string(),
                        bynk_ts::TsExpr::Arrow {
                            params: vec![bynk_ts::TsParam {
                                name: "events".to_string(),
                                ty: Some(bynk_ts::TsType::named_with_args(
                                    "globalThis.Array",
                                    vec![bynk_ts::TsType::named(
                                        crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE,
                                    )],
                                )),
                                optional: false,
                            }],
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
                                                object: Box::new(bynk_ts::TsExpr::Ident(
                                                    "env".to_string(),
                                                )),
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
                ));
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
        let fetch_method = bynk_ts::TsClassMethod {
            name: "fetch".to_string(),
            private: false,
            is_async: true,
            params: vec![bynk_ts::TsParam {
                name: "request".to_string(),
                ty: Some(bynk_ts::TsType::named("globalThis.Request")),
                optional: false,
            }],
            return_type: Some(bynk_ts::TsType::named_with_args(
                "globalThis.Promise",
                vec![bynk_ts::TsType::named("globalThis.Response")],
            )),
            doc: None,
            body: fetch_stmts,
        };
        out.push_str(&bynk_ts::print_class_method(&fetch_method, 0));
        writeln!(out).unwrap();
        // v0.106 (slice 3b-iii): the inbound/close dispatch — Cloudflare calls
        // `webSocketMessage`/`webSocketClose` on a hibernatable socket. Decode the
        // frame against `in:` (reject-and-close on failure), recover the sender
        // identity + route args from the socket attachment, and run the body.
        for host in &ws_open_hosts {
            emit_ws_dispatch_handlers(&mut out, host, &ctx.runtime_use, &commons.types, tys);
        }
    }
    writeln!(out, "}}").unwrap();
    // #1486: no trailing blank baked in here — the printer's automatic
    // top-level spacing policy supplies the single blank before whatever
    // statement follows.
    let mut class_stmt = bynk_ts::TsStmt::raw(out, None);
    class_stmt.nested_map = Some(class_smb.into_inner());
    stmts.push(class_stmt);
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
    if workers {
        stmts.push(agent_wire_table(a, &ctx.runtime_use));
        if reads_key {
            stmts.push(agent_key_codec(a, &ctx.runtime_use));
        }
    }
    let key_ts = ts_type_ref_to_ts_type(&a.key_type, None);
    let bind = crate::emitter::wrangler::agent_binding(&a.name.name).name;
    let mut make_agent_args = vec![
        bynk_ts::TsExpr::Ident(registry.clone()),
        bynk_ts::TsExpr::OptionalMember {
            object: Box::new(bynk_ts::TsExpr::Ident("env".to_string())),
            property: bind.clone(),
        },
        bynk_ts::TsExpr::Ident("key".to_string()),
        bynk_ts::TsExpr::Arrow {
            params: vec![bynk_ts::TsParam {
                name: "state".to_string(),
                ty: None,
                optional: false,
            }],
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
    let factory_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Function {
            name: agent_factory_name(&a.name.name),
            generics: Vec::new(),
            params: vec![
                bynk_ts::TsParam {
                    name: "key".to_string(),
                    ty: Some(key_ts),
                    optional: false,
                },
                bynk_ts::TsParam {
                    name: "env".to_string(),
                    ty: Some(bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::Prop {
                        name: bind.clone(),
                        ty: bynk_ts::TsType::named("__DurableObjectNamespace"),
                        optional: true,
                        readonly: false,
                    }])),
                    optional: true,
                },
            ],
            return_type: Some(bynk_ts::TsType::named(a.name.name.clone())),
            body: vec![bynk_ts::TsStmt::return_stmt(
                Some(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Ident("__makeAgent".to_string())),
                    args: make_agent_args,
                }),
                None,
            )],
            is_async: false,
            inline: false,
        })),
        None,
    );
    stmts.push(factory_decl);
    // #1486: no explicit blank stmt here — the printer's automatic top-level
    // spacing policy supplies the single blank before whatever follows.

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
        // #1386/#1462: a second, separate permanent exclusion from this
        // function's own top-level signature — genuinely raw hand-templated
        // text throughout, zero `bynk_ts` nodes or `LowerCtx` involvement,
        // not decomposed further. Local buffer, replacing this block's own
        // former use of the function's `out` parameter.
        let mut out = String::new();
        let hs: Vec<&Handler> = a
            .handlers
            .iter()
            .filter(|h| {
                matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Call)
                    && h.method_name.is_some()
            })
            .collect();
        let driver = format!("__bynkDriveHistory_{}", a.name.name);
        let factory = agent_factory_name(&a.name.name);
        let key_val = bynk_check::checker::zero_value_ts(&a.key_type, None, &commons.types)
            .unwrap_or_else(|| "undefined as never".to_string());
        // P7.2: `call`'s real shape is a discriminated union across `hs` (one
        // variant per handler, built further down from `history_variant_tag` +
        // each handler's own fields) and `deps` varies per handler too (each
        // handler computes its own `deps_ty` from its own `given` set,
        // `build_deps_object_ty_with_surface`'s own per-handler call elsewhere
        // in this file) — a single driver-wide type for either needs the
        // intersection/union across every handler this agent's history targets,
        // not a same-line text change. `seq`'s own `args` is safe to narrow:
        // it's read back only through `history_arg_ts`-shaped call sites below,
        // never given a fixed shape of its own.
        let step_ty =
            format!("{{ call: any, accepted: boolean, old: {state_ty}, new: {state_ty} }}");
        writeln!(
            out,
            "export async function {driver}(seq: globalThis.Array<{{ h: number, args: unknown[] }}>, deps: any): globalThis.Promise<globalThis.Array<{step_ty}>> {{"
        )
        .unwrap();
        writeln!(out, "  {registry}.reset();").unwrap();
        // P7.2: deferred, not narrowed. A first attempt dropped this cast,
        // reasoning `{factory}` already declares `): {agent}` and `.state`/
        // `.{{method}}` are real declared members — true, but it broke real
        // `tsc --strict` fixtures (248/249): `history_arg_ts` below produces
        // unbranded plain values (`Number(__st.args[0])`), which don't
        // structurally match a branded handler param type (`Amount`) once
        // `__inst` is genuinely `{agent}`-typed instead of `any`. Narrowing
        // `history_arg_ts`'s own output to brand its values correctly is the
        // real fix, not a same-line change here.
        writeln!(out, "  const __inst = {factory}({key_val}) as any;").unwrap();
        // #1649: decode the stored (wire-shape) state exactly as `loadState` does.
        let decode = if has_rehydrate {
            format!(" {rehydrate_fn}(__m, __s);")
        } else {
            String::new()
        };
        // State-migrations S1 (#2211): the same `__v` check `loadState` runs.
        let version_check = agent_versions
            .as_ref()
            .map(|v| {
                let steps = if step_plans.is_empty() {
                    String::new()
                } else {
                    format!(
                        ", [{}]",
                        step_plans
                            .iter()
                            .map(|s| format!("[{}, {}]", s.lo, s.from))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                format!(
                    " __checkStoredSchemaVersion(\"{}\", __s, {}, {}{steps});",
                    a.name.name, v.version, v.oldest_readable
                )
            })
            .unwrap_or_default();
        // State-migrations S2 (#2224): and the same step dispatch.
        let migrate_call = if step_plans.is_empty() {
            String::new()
        } else {
            format!(" __s = {migrate_fn}(__s);")
        };
        let s_decl = if step_plans.is_empty() {
            "const"
        } else {
            "let"
        };
        writeln!(
            out,
            "  const __load = async (): globalThis.Promise<{state_ty}> => {{ {s_decl} __s = await __inst.state.storage.get(\"state\"); if (__s === undefined) return {zero_fn}();{version_check}{migrate_call} const __m = {{ ...{zero_fn}(), ...__s }};{decode} return __m; }};"
        )
        .unwrap();
        // P7.2: `e` is a caught throw of unknown shape by construction — a
        // marker type stating exactly the one field this checks, not a blanket
        // escape.
        writeln!(
            out,
            "  const __rej = (e: unknown) => !!e && (e as {{ invariantViolation?: unknown }}).invariantViolation !== undefined;"
        )
        .unwrap();
        writeln!(out, "  const __steps: globalThis.Array<{step_ty}> = [];").unwrap();
        // A driven run deliberately provokes rejected steps (an invariant/
        // `transition` refusal), each of which `console.error`s an
        // `InvariantViolation` line from `commitState` before throwing. Mute just
        // those lines for the duration of the drive so the run isn't drowned in
        // expected noise; every other `console.error` still passes through, and the
        // original is restored in `finally`.
        // P7.2: `console.error`'s own real type, `(...data: unknown[]) => void`
        // — `__ce` is already that type by inference, and the replacement
        // matches it exactly, so neither cast was ever load-bearing.
        writeln!(out, "  const __ce = globalThis.console.error;").unwrap();
        writeln!(
            out,
            "  globalThis.console.error = (...__a: unknown[]) => {{ if (typeof __a[0] === \"string\" && __a[0].startsWith(\"InvariantViolation\")) return; __ce(...__a); }};"
        )
        .unwrap();
        writeln!(out, "  try {{").unwrap();
        writeln!(out, "  for (const __st of seq) {{").unwrap();
        writeln!(out, "    const __old = await __load();").unwrap();
        writeln!(out, "    let __accepted = true;").unwrap();
        writeln!(out, "    try {{").unwrap();
        writeln!(out, "      switch (__st.h) {{").unwrap();
        for (i, h) in hs.iter().enumerate() {
            let m = &h.method_name.as_ref().unwrap().name;
            let mut call_args: Vec<String> = h
                .params
                .iter()
                .enumerate()
                .map(|(j, p)| history_arg_ts(p, j, &commons.types))
                .collect();
            call_args.push("deps".to_string());
            writeln!(
                out,
                "        case {i}: await __inst.{m}({}); break;",
                call_args.join(", ")
            )
            .unwrap();
        }
        writeln!(out, "      }}").unwrap();
        writeln!(
            out,
            "    }} catch (__e) {{ if (__rej(__e)) {{ __accepted = false; }} else {{ throw __e; }} }}"
        )
        .unwrap();
        writeln!(
            out,
            "    const __new = __accepted ? await __load() : __old;"
        )
        .unwrap();
        // P7.2: deferred, same reason as `step_ty`'s own `call` field above —
        // a real type here is the same per-handler discriminated union.
        writeln!(out, "    let __call: any;").unwrap();
        writeln!(out, "    switch (__st.h) {{").unwrap();
        for (i, h) in hs.iter().enumerate() {
            let tag = history_variant_tag(&h.method_name.as_ref().unwrap().name);
            let fields: Vec<String> = h
                .params
                .iter()
                .enumerate()
                .map(|(j, p)| {
                    format!(
                        "{}: {}",
                        ts_ident(&p.name.name),
                        history_arg_ts(p, j, &commons.types)
                    )
                })
                .collect();
            let obj = if fields.is_empty() {
                format!("{{ tag: \"{tag}\" }}")
            } else {
                format!("{{ tag: \"{tag}\", {} }}", fields.join(", "))
            };
            writeln!(out, "      case {i}: __call = {obj}; break;").unwrap();
        }
        writeln!(out, "    }}").unwrap();
        writeln!(
            out,
            "    __steps.push({{ call: __call, accepted: __accepted, old: __old, new: __new }});"
        )
        .unwrap();
        writeln!(out, "  }}").unwrap();
        writeln!(out, "  return __steps;").unwrap();
        writeln!(out, "  }} finally {{ globalThis.console.error = __ce; }}").unwrap();
        writeln!(out, "}}").unwrap();
        // #1486: no trailing blank baked in here — the printer's automatic
        // top-level spacing policy supplies the single blank before whatever
        // statement follows.
        stmts.push(bynk_ts::TsStmt::raw(out, None));
    }
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
    // #1818: for a hosting agent that reads its own key, the key codec's name
    // and the key's TS type. A WebSocket body runs through these entry points,
    // not the `/_bynk/agent/` dispatch, so each sets `this.__key` itself.
    agent_key: Option<(String, String)>,
}

impl WsOpenHost<'_> {
    /// True if the service has an inbound/close handler — then the `on open` accept
    /// writes the identity + route args into the socket attachment so a waking
    /// `webSocketMessage`/`webSocketClose` can recover them.
    fn has_inbound(&self) -> bool {
        self.message.is_some() || self.close.is_some()
    }
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
                    seam: bynk_check::actors::bearer_seam_for(h, actors),
                    agent_key: None,
                });
            }
        }
    }
    hosts
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
    out: &mut String,
    agent: &AgentDecl,
    host: &WsOpenHost<'_>,
    h: &Handler,
    method: &str,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    source_map: Option<&RefCell<SourceMapBuilder>>,
) {
    let mut params = vec![bynk_ts::TsParam {
        name: "connection".to_string(),
        ty: Some(bynk_ts::TsType::named(format!(
            "Connection<{}>",
            ts_ty(host.out_ty, commons.tys())
        ))),
        optional: false,
    }];
    for p in &h.params {
        params.push(bynk_ts::TsParam {
            name: ts_ident(&p.name.name),
            ty: Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
            optional: false,
        });
    }
    let body_smb = RefCell::new(SourceMapBuilder::new());
    let mut module = ModuleCtx::new(commons, &ctx.cross_context, &ctx.runtime_use);
    module.in_bynk_unit = ctx.commons_name == "bynk";
    module.agent_method_givens = ctx.agent_method_givens.clone();
    module.agent_called_contexts = ctx.agent_called_contexts.clone();
    module.event_schema_versions = ctx.event_schema_versions.clone();
    module.consumed_type_ns = consumed_type_ns(ctx);
    module.consumed_context_types = ctx.consumed_context_types();
    module.target = ctx.target;
    let mut cx = LowerCtx::new(
        module,
        BodyMode::WsDoMethod {
            handler: HandlerShared {
                capabilities: bynk_lower::lower_handler_given_ir(h)
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
            deps_identity_binder: host.seam.as_ref().and_then(|s| s.binder.clone()),
            ws_self_agent: Some(agent.name.name.clone()),
        },
    )
    .with_source_map(Some(&body_smb));
    cx.local_agents = ctx.local_agents.clone();
    // #1653: parameters are bindings of the body's own scope, so a `let` that
    // re-binds one gets a fresh name instead of redeclaring it.
    for p in &h.params {
        cx.declare_binder(&p.name.name);
    }
    let async_tail = is_effectful_return(&h.return_type);
    let mut body_out = String::new();
    emit_block_as_function_body_with_return(
        &mut body_out,
        &h.body,
        &mut cx,
        INDENT_STEP * 2,
        async_tail,
        Some(&h.return_type),
    );
    let mut deps_members = build_deps_object_ty_with_surface(
        &effective_given(&bynk_lower::lower_handler_given_ir(h), &cx),
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
    params.push(bynk_ts::TsParam {
        name: "deps".to_string(),
        ty: Some(bynk_ts::TsType::Object(deps_members)),
        optional: false,
    });
    let method_entry = bynk_ts::TsClassMethod {
        name: method.to_string(),
        private: false,
        is_async: async_tail,
        params,
        return_type: Some(ts_type_ref_to_ts_type(&h.return_type, None)),
        doc: None,
        body: vec![bynk_ts::TsStmt::raw(body_out.clone(), None)],
    };
    let class_depth = 0;
    emit_class_method_and_merge_source_map(
        out,
        &method_entry,
        class_depth,
        RawBody::flat(&body_out),
        &body_smb,
        source_map,
    );
    writeln!(out).unwrap();
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
                vec![bynk_ts::TsParam {
                    name: "s".to_string(),
                    ty: None,
                    optional: false,
                }],
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

fn ws_open_fetch_branch_stmt(host: &WsOpenHost<'_>, tys: &Arc<Types>) -> bynk_ts::TsStmt {
    let h = host.handler;
    let path = format!("/_bynk/ws/open/{}", host.service);
    let method = ws_open_do_method_name(host.service);
    // The trusted internal header carries the route args, and the verified
    // identity only when the actor binds one (a binder-less `by` forwards none).
    let binds_identity = host.seam.as_ref().is_some_and(|s| s.binder.is_some());
    let payload_ty = match (binds_identity, host.agent_key.is_some()) {
        (true, false) => "{ args: unknown[]; identity: string }",
        (false, false) => "{ args: unknown[] }",
        (true, true) => "{ args: unknown[]; identity: string; key: unknown }",
        (false, true) => "{ args: unknown[]; key: unknown }",
    };
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
    let deps_arg = match host.seam.as_ref().filter(|s| s.binder.is_some()) {
        Some(seam) => format!(
            "{{ identity: __payload.identity as {} }}",
            seam.identity_type
        ),
        None => "{}".to_string(),
    };
    // An object-literal expression — tagged `VerbatimExpr`, not `Ident`
    // (#1539).
    call_args.push(bynk_ts::TsExpr::VerbatimExpr(
        deps_arg,
        bynk_ts::VerbatimOrigin::Emit,
    ));
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
fn ws_attachment_deps_arg(seam: &Option<bynk_check::actors::BearerSeam>) -> String {
    match seam.as_ref().filter(|s| s.binder.is_some()) {
        Some(seam) => format!("{{ identity: __att.identity as {} }}", seam.identity_type),
        None => "{}".to_string(),
    }
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
    out: &mut String,
    host: &WsOpenHost<'_>,
    runtime_use: &RuntimeUse,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
) {
    if !host.has_inbound() {
        return;
    }
    let out_ts = ts_ty(host.out_ty, tys);
    let att_ty = if host.agent_key.is_some() {
        "{ connId: string; identity: string; args: unknown[]; key: unknown }"
    } else {
        "{ connId: string; identity: string; args: unknown[] }"
    };
    // The firing socket's minimal structural surface (attachment + send/close), so
    // emitted code stays free of `@cloudflare/workers-types`.
    let ws_ty = "{ deserializeAttachment(): unknown; send(data: string): void; close(code?: number, reason?: string): void }";
    let deps_arg = ws_attachment_deps_arg(&host.seam);
    let att_or_null = bynk_ts::TsType::union(vec![
        bynk_ts::TsType::named(att_ty),
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
        let in_type = ty_to_type_ref(host.in_ty, tys).unwrap_or_else(|| {
            panic!(
                "bynk internal error: a `from websocket` service's own `in:` frame type did \
                 not round-trip to a TypeRef, but check_service_protocols already certified it \
                 codec-eligible"
            )
        });
        let decode = serialisation::deserialise_expr(&in_type, "__json", "frame", runtime_use);
        let mut stmts = Vec::new();
        att_guard(&mut stmts);
        stmts.push(bynk_ts::TsStmt::let_stmt(
            bynk_ts::TsBindingName::Ident("__raw".to_string()),
            Some(bynk_ts::TsType::named("string")),
            None,
            None,
        ));
        stmts.push(bynk_ts::TsStmt::raw(
            "    try { __raw = typeof message === \"string\" ? message : new globalThis.TextDecoder().decode(message); } catch { ws.close(1003, \"unreadable frame\"); return; }\n",
            None,
        ));
        stmts.push(bynk_ts::TsStmt::let_stmt(
            bynk_ts::TsBindingName::Ident("__json".to_string()),
            Some(bynk_ts::TsType::named("__JsonValue")),
            None,
            None,
        ));
        stmts.push(bynk_ts::TsStmt::raw(
            "    try { __json = globalThis.JSON.parse(__raw) as __JsonValue; } catch { ws.close(1003, \"malformed frame\"); return; }\n",
            None,
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
            bynk_check::checker::resolve_type_ref_in(t, types, &no_vars, tys)
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
        call_args.push(deps_arg.clone());
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
                    property: method,
                }),
                args: call_args.into_iter().map(bynk_ts::TsExpr::Ident).collect(),
            })),
            None,
        ));
        let message_method = bynk_ts::TsClassMethod {
            name: "webSocketMessage".to_string(),
            private: false,
            is_async: true,
            params: vec![
                bynk_ts::TsParam {
                    name: "ws".to_string(),
                    ty: Some(bynk_ts::TsType::named(ws_ty)),
                    optional: false,
                },
                bynk_ts::TsParam {
                    name: "message".to_string(),
                    ty: Some(bynk_ts::TsType::union(vec![
                        bynk_ts::TsType::named("string"),
                        bynk_ts::TsType::named("globalThis.ArrayBuffer"),
                    ])),
                    optional: false,
                },
            ],
            return_type: Some(bynk_ts::TsType::named_with_args(
                "globalThis.Promise",
                vec![bynk_ts::TsType::named("void")],
            )),
            doc: None,
            body: stmts,
        };
        out.push_str(&bynk_ts::print_class_method(&message_method, 0));
        writeln!(out).unwrap();
    }

    if let Some(c) = host.close {
        let method = ws_close_do_method_name(host.service);
        let mut stmts = vec![bynk_ts::TsStmt::raw(
            "    void code; void reason; void wasClean;\n",
            None,
        )];
        att_guard(&mut stmts);
        let mut call_args = vec!["connection".to_string()];
        for (i, p) in c.params.iter().enumerate() {
            call_args.push(format!("__att.args[{i}] as {}", ts_type_ref(&p.type_ref)));
        }
        call_args.push(deps_arg);
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
                    property: method,
                }),
                args: call_args.into_iter().map(bynk_ts::TsExpr::Ident).collect(),
            })),
            None,
        ));
        let close_method = bynk_ts::TsClassMethod {
            name: "webSocketClose".to_string(),
            private: false,
            is_async: true,
            params: vec![
                bynk_ts::TsParam {
                    name: "ws".to_string(),
                    ty: Some(bynk_ts::TsType::named(ws_ty)),
                    optional: false,
                },
                bynk_ts::TsParam {
                    name: "code".to_string(),
                    ty: Some(bynk_ts::TsType::named("number")),
                    optional: false,
                },
                bynk_ts::TsParam {
                    name: "reason".to_string(),
                    ty: Some(bynk_ts::TsType::named("string")),
                    optional: false,
                },
                bynk_ts::TsParam {
                    name: "wasClean".to_string(),
                    ty: Some(bynk_ts::TsType::named("boolean")),
                    optional: false,
                },
            ],
            return_type: Some(bynk_ts::TsType::named_with_args(
                "globalThis.Promise",
                vec![bynk_ts::TsType::named("void")],
            )),
            doc: None,
            body: stmts,
        };
        out.push_str(&bynk_ts::print_class_method(&close_method, 0));
        writeln!(out).unwrap();
    }
}

#[cfg(test)]
mod doc_block_tests {
    use super::emit_doc_block;

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
    use super::{emit_pred_check, print_numeric_guard_stmt};
    use bynk_syntax::ast::PredKind;

    /// #1335: the real, converted `print_numeric_guard_stmt` matches
    /// `254_multi_file_commons_workers_codec`'s own real `Cents.of`
    /// `Int`-base guard byte-for-byte.
    #[test]
    fn numeric_guard_matches_the_real_fixtures_own_int_guard_byte_for_byte() {
        assert_eq!(
            print_numeric_guard_stmt("Cents", "isInteger", "must be an integer"),
            "    if (!globalThis.Number.isInteger(value)) {\n      \
             return Err({ field: \"Cents\", message: \"must be an integer\", value });\n    \
             }\n"
        );
    }

    /// #1335: the real, converted `emit_pred_check` matches
    /// `254_multi_file_commons_workers_codec`'s own real `Cents.of`
    /// `NonNegative` predicate check byte-for-byte.
    #[test]
    fn pred_check_matches_the_real_fixtures_own_non_negative_check_byte_for_byte() {
        let mut out = String::new();
        emit_pred_check(&mut out, "Cents", &PredKind::NonNegative);
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
        let mut out = String::new();
        emit_pred_check(&mut out, "Code", &PredKind::Matches(r"\d+".to_string()));
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
    /// (`bynkc/tests/fixtures/positive/406_generic_sum_envelope`), confirmed
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
        let out = crate::project::compile_in_memory(
            "context demo {\n  event Notified = { id: String }\n}\n",
            crate::project::BuildTarget::Bundle,
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
        let out = crate::project::compile_in_memory(
            src,
            crate::project::BuildTarget::Bundle,
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
