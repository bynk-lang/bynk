//! Imports, re-exports and rebrands of a project-mode module (2.5a, #4090).
//!
//! The import statements `emit_project` writes for a unit (`emit_project_imports`
//! and `emit_cross_context_namespace_imports`), the rebrands and re-exports a
//! context emits for the types it consumes from another (`emit_context_rebrands`,
//! `emit_consumed_type_reexports`), the `ExternalReferences` resolver that finds
//! which names a body takes from outside its own file, and the specifier helpers
//! (`ts_specifier`, `cross_commons_import_specifier_for_path`) the driver and
//! `compose/` also read. `emitter` re-exports the ones that are public.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::ctx::UnitFacts;
use super::ts_types::ts_base;
use super::{cross_context_cap_namespaces, emit_forwarded_methods, qualified_to_ns, ts_ident};
use crate::emit_model::{EmitProjectCtx, ExtraImport, ImportExt};
use bynk_check::checker::{Ty, TypedCommons};
use bynk_check::project_model::BuildTarget;
use bynk_project::UnitKind;
use bynk_syntax::ast::{BaseType, CommonsItem, TypeBody, TypeDecl};

/// For each type imported via `uses` that's referenced in this file, emit:
/// 1. (Done in imports) an aliased import: `import { Money as __CommonsMoney } from ...`
/// 2. A rebranded type alias: `export type Money = __CommonsMoney & { readonly __ctxBrand?: "..." }`
///
/// The brand makes two contexts that both `uses` the same commons see distinct
/// nominal `Money` types in their TypeScript output (v0.4 §3.4 / §6.2). It is
/// optional (#1704), so an unbranded commons value (a record literal, a nested
/// field of another rebranded record, a commons fn's result) is accepted where
/// the context's type is expected, while a value carrying another context's
/// brand is still rejected.
/// #1478: real-node-internally already — returns its real declarations
/// (a rebrand type alias, and — for a refined/opaque base — a forwarding
/// const, per name) instead of writing into `out: &mut String`.
/// #2036: on `workers`, re-export as types the names this `handlers.ts`
/// imports from a consumed context. `compose.ts` names every type in a
/// wrapper's signature as `handlers.<Name>` (it imports no type names of its
/// own), so a consumed `Batch` in a handler's parameters must be a member of
/// `handlers` too (a consumed *adapter's* type — `R2Upload` — likewise, #2647:
/// it is imported from the adapter's module by the same statement), the way a
/// `uses`d commons type is through its rebrand alias. Type-only
/// (`export { type Batch }`): `compose.ts` uses the name in type position
/// only, a consumed refined alias has no value to re-export (TS1205 under
/// `isolatedModules`), and a value re-export would deepen the caller's
/// dependency on the callee's module. Only names the module imports
/// are re-exported, so each is bound here.
pub(super) fn emit_consumed_type_reexports(
    refs: &ExternalReferences,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    if ctx.target != BuildTarget::Workers {
        return Vec::new();
    }
    // #2647: a consumed *adapter's* types are re-exported only when some
    // service handler's parameter names them, in the handler kinds below. A consumed
    // context's are re-exported whenever imported, as before.
    let handler_param_types: HashSet<String> = commons
        .commons
        .items
        .iter()
        .filter_map(CommonsItem::as_service)
        .flat_map(|s| s.handlers.iter())
        // The kinds whose `compose.ts` wrapper types its parameters with
        // `qualified_ts_type_ref` (call, cron, queue `message`, http); an
        // `event` handler's other parameters never reach `compose.ts`.
        .filter(|h| {
            matches!(
                h.kind,
                bynk_syntax::ast::HandlerKind::Call
                    | bynk_syntax::ast::HandlerKind::Http { .. }
                    | bynk_syntax::ast::HandlerKind::Cron { .. }
                    | bynk_syntax::ast::HandlerKind::Message
            )
        })
        .flat_map(|h| h.params.iter())
        .flat_map(|p| super::workers::named_types_in(&p.type_ref))
        .collect();
    let mut names: Vec<String> = refs
        .by_commons
        .iter()
        .flat_map(|(unit, set)| {
            let adapter = ctx.consumed_adapters.contains(unit.as_str());
            let handler_param_types = &handler_param_types;
            set.iter()
                .filter(move |n| !adapter || handler_param_types.contains(n.as_str()))
        })
        .filter(|n| ctx.imported_from_kind.get(n.as_str()) == Some(&UnitKind::Context))
        .map(|n| format!("type {}", ts_ident(n)))
        .collect();
    if names.is_empty() {
        return Vec::new();
    }
    names.sort();
    names.dedup();
    vec![bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::ExportNames { names },
        None,
    )]
}

pub(super) fn emit_context_rebrands(
    refs: &ExternalReferences,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    let Some(owning) = &ctx.owning_context else {
        return Vec::new();
    };
    // Collect names imported via `uses` (kind == Commons in imported_from_kind).
    // R4.10/R8.2: reads `bynk_check::resolver::compute_is_uses_commons_type`, the same
    // predicate `prepare_unit_check_ctx` (`check_pipeline.rs`) computes its own
    // `uses_commons_type_names` from — one definition, not two independently
    // maintained copies linked only by a doc-comment promise (see that
    // function's own doc comment for the real defect this closes, ADR 0226).
    let mut names: Vec<String> = Vec::new();
    for set in refs.by_commons.values() {
        for n in set {
            // v0.20b: only *types* get the context rebrand — a
            // `uses`-imported function is a value and imports plainly.
            if bynk_check::resolver::compute_is_uses_commons_type(
                &ctx.imported_from_kind,
                &commons.types,
                n,
            ) {
                names.push(n.clone());
            }
        }
    }
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Vec::new();
    }
    let mut stmts = Vec::new();
    for name in &names {
        // v0.174 (#592): a generic commons type keeps its parameters across the
        // rebrand — `Paginated[T]` aliases as `Paginated<T> =
        // __CommonsPaginated<T> & { … }`, not a bare `Paginated`, which would
        // both drop the parameter and make every `Paginated<User>` reference in
        // the context a "type is not generic" error.
        let params: Vec<&str> = commons
            .types
            .get(name)
            .map(|d| d.type_params.iter().map(|p| p.name.name.as_str()).collect())
            .unwrap_or_default();
        let generic_args: Vec<bynk_ts::TsType> =
            params.iter().map(|p| bynk_ts::TsType::named(*p)).collect();
        // Arc C, slice 30 (#1392): the SAME `TsDecl::TypeAlias` over
        // `TsType::Intersection` shape #1339's own `emit_refined_type`
        // already established for its sibling `__brand` alias (a real,
        // proven precedent, not this slice's own new gap).
        let type_alias = bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::TypeAlias {
                name: name.clone(),
                type_params: params.iter().map(|p| p.to_string()).collect(),
                ty: bynk_ts::TsType::intersection(vec![
                    bynk_ts::TsType::named_with_args(commons_alias(name), generic_args),
                    // #1704: the brand is *optional*. A plain commons value (a
                    // record literal, a nested field, a commons fn's result)
                    // has no brand, and it is assignable here; another
                    // context's brand (`"a"` against `"b"`) still is not, so
                    // the two contexts' types stay nominally distinct.
                    bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::Prop {
                        name: "__ctxBrand".to_string(),
                        ty: bynk_ts::TsType::named(format!("\"{owning}\"")),
                        optional: true,
                        readonly: true,
                    }]),
                ]),
            })),
            None,
        );
        stmts.push(type_alias);
        // v0.9.2: a commons refined/opaque type carries a value-side
        // constructor (`.of`, and `.unsafe` for opaque). Re-export it under the
        // rebranded name so a context calling `ShortCode.of(...)` resolves to a
        // value — delegating to the imported commons constructor but reporting
        // the context-branded type. (Without this, `ShortCode` is type-only in
        // the context and `.of` fails to resolve.)
        if let Some(base) = commons
            .types
            .get(name)
            .and_then(|d| refined_or_opaque_base(d))
        {
            let ts_base_name = ts_base(base);
            let is_opaque = matches!(
                commons.types.get(name).map(|d| &d.body),
                Some(TypeBody::Opaque { .. })
            );
            // #1704: `return __CommonsX.of(value);` with no cast — the
            // commons result is unbranded, which fits the context's
            // optionally-branded `Result<X, ValidationError>` directly.
            let of_entry = bynk_ts::TsObjectEntry::Method {
                name: "of".to_string(),
                is_async: false,
                generics: Vec::new(),
                params: vec![bynk_ts::TsParam::new(
                    "value".to_string(),
                    Some(bynk_ts::TsType::named(ts_base_name)),
                )],
                return_type: Some(bynk_ts::TsType::named_with_args(
                    "Result",
                    vec![
                        bynk_ts::TsType::named(name.clone()),
                        bynk_ts::TsType::named("ValidationError"),
                    ],
                )),
                doc: None,
                inline: true,
                body: vec![bynk_ts::TsStmt::return_stmt(
                    Some(bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Member {
                            object: Box::new(bynk_ts::TsExpr::Ident(commons_alias(name))),
                            property: "of".to_string(),
                        }),
                        args: vec![bynk_ts::TsExpr::Ident("value".to_string())],
                    }),
                    None,
                )],
                span: None,
            };
            let mut entries = vec![of_entry];
            // ADR 0182: only opaque types have a public `.unsafe` to forward.
            // A refined/alias type has none — a consuming context brands an
            // admitted literal with an inline `as` cast, not a forwarder call.
            if is_opaque {
                entries.push(bynk_ts::TsObjectEntry::Method {
                    name: "unsafe".to_string(),
                    is_async: false,
                    generics: Vec::new(),
                    params: vec![bynk_ts::TsParam::new(
                        "value".to_string(),
                        Some(bynk_ts::TsType::named(ts_base_name)),
                    )],
                    return_type: Some(bynk_ts::TsType::named(name.clone())),
                    doc: None,
                    inline: true,
                    body: vec![bynk_ts::TsStmt::return_stmt(
                        Some(bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Member {
                                object: Box::new(bynk_ts::TsExpr::Ident(commons_alias(name))),
                                property: "unsafe".to_string(),
                            }),
                            args: vec![bynk_ts::TsExpr::Ident("value".to_string())],
                        }),
                        None,
                    )],
                    span: None,
                });
            }
            // v0.132.1 (#481): forward the commons' user-defined attached methods
            // (`Cents.fromInt`, …) so the rebranded const carries more than the
            // built-in `of`/`unsafe`. Without this a consumer's `Cents.fromInt(n)`
            // — which `bynk check` accepts — fails `tsc`. The methods aren't in
            // this context's own `commons` (only imported *types* are merged);
            // they arrive via `ctx.imported_methods`, keyed by type name.
            if let Some(methods) = ctx.imported_methods.get(name) {
                entries.extend(emit_forwarded_methods(
                    name,
                    methods,
                    UnitFacts::of(ctx).render(commons),
                ));
            }
            let const_decl = bynk_ts::TsStmt::decl(
                bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
                    name: name.clone(),
                    ty: None,
                    init: bynk_ts::TsExpr::multiline_object_entries(entries),
                })),
                None,
            );
            stmts.push(const_decl);
        } else if ctx
            .imported_methods
            .get(name)
            .is_some_and(|m| !m.is_empty())
        {
            // #2751: a record/sum commons type has no `.of`, but its
            // attached methods are still called as `Type.method(recv)`, which
            // needs a value `Type` in the context (the rebrand above is only
            // a type). The value is the commons' own, re-exported unbranded:
            // its methods already speak types the context's optionally-branded
            // ones fit, and — unlike a re-declared forwarder — it carries a
            // generic type's parameters, a bounded method's hidden comparator
            // and codec witnesses, and a sum's variants unchanged.
            stmts.push(bynk_ts::TsStmt::decl(
                bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
                    name: name.clone(),
                    ty: None,
                    init: bynk_ts::TsExpr::Ident(commons_alias(name)),
                })),
                None,
            ));
        }
    }
    // #1486: no explicit trailing blank stmt here — the printer's automatic
    // top-level spacing policy supplies the single blank before whatever
    // follows.
    //
    // Every rebrand entry (a lone `type_alias`, or a `type_alias`+`const_decl`
    // pair) sits directly under its predecessor with no blank between them —
    // confirmed against `129_full_orders_http_api`'s own real fixture output,
    // several rebrands stacked with zero blank lines anywhere among them.
    // Only the very first statement in this whole function's own returned
    // `Vec` keeps the printer's ordinary automatic blank relative to
    // whatever precedes `emit_context_rebrands`'s own call site.
    for stmt in stmts.iter_mut().skip(1) {
        stmt.no_blank_before = true;
    }
    stmts
}

/// If a type declaration is a refined or opaque base type, return its base
/// (both lower to a branded base with a `.of` / `.unsafe` constructor object).
///
/// P6.56 (design/tracks/the-ir.md §6b): investigated routing this through
/// `TypeShape::Refined` and declined — that variant's own `base` field is
/// `bynk_syntax::ast::BaseType` (P6.41 ruled it stays, phase 7 — the bounds
/// keep source lexemes for byte-stable emission), so this function's own
/// `Option<BaseType>` return type is identical either way. Converting would
/// add a `CheckedProgram`/`TypedCommons` dependency this function doesn't
/// have today for zero reduction in AST-type surface.
fn refined_or_opaque_base(decl: &TypeDecl) -> Option<BaseType> {
    match &decl.body {
        TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. } => Some(*base),
        TypeBody::Record(_) | TypeBody::Sum(_) => None,
    }
}

/// Names that this file needs to import from elsewhere (sibling files of
/// the same commons, or other commons via `uses`).
#[derive(Default)]
pub(super) struct ExternalReferences {
    /// `commons name` → set of names to import.
    by_commons: HashMap<String, HashSet<String>>,
    /// `sibling source path` → set of names to import (same-commons).
    by_sibling: HashMap<PathBuf, HashSet<String>>,
    /// #1820: names imported from a sibling file as types only: a capability
    /// is an `interface`, which has no value to import once the module is
    /// stripped to JavaScript.
    types_by_sibling: HashMap<PathBuf, HashSet<String>>,
}

impl ExternalReferences {
    pub(super) fn is_empty(&self) -> bool {
        self.by_commons.is_empty() && self.by_sibling.is_empty() && self.types_by_sibling.is_empty()
    }
}

/// The names this file declares, so a reference to one needs no import. A
/// `messages` block declares no importable identifier of its own (its
/// `render` is synthesised separately), so `name()` is `None` there and it
/// contributes nothing.
fn local_names(commons: &TypedCommons) -> HashSet<String> {
    commons
        .commons
        .items
        .iter()
        .filter_map(|i| i.name().map(|n| n.name.clone()))
        .collect()
}

/// Whether `name` is declared elsewhere than this file and so imported: a name of a used
/// commons, of a sibling file (a type, a function or a capability) or of a consumed context.
fn resolves_externally(name: &str, local_to_file: &HashSet<String>, ctx: &EmitProjectCtx) -> bool {
    let mut found = ExternalReferences::default();
    record_external_name(name, local_to_file, ctx, &mut found);
    !found.is_empty()
}

/// [`record_name_ref`], a sibling file's capability (an `interface`, imported as a type only,
/// #1820) and a sibling file's agent factory (`__make<Agent>`, which the file exports, #1820).
fn record_external_name(
    name: &str,
    local_to_file: &HashSet<String>,
    ctx: &EmitProjectCtx,
    out: &mut ExternalReferences,
) {
    record_name_ref(name, local_to_file, ctx, out);
    if let Some(agent) = name.strip_prefix("__make")
        && !local_to_file.contains(agent)
        && let Some(path) = ctx.file_decl_index.agents.get(agent)
        && path != &ctx.source_path
    {
        out.by_sibling
            .entry(path.clone())
            .or_default()
            .insert(name.to_string());
    }
    if !local_to_file.contains(name)
        && let Some(path) = ctx.file_decl_index.capabilities.get(name)
        && path != &ctx.source_path
    {
        out.types_by_sibling
            .entry(path.clone())
            .or_default()
            .insert(name.to_string());
    }
}

/// The names `body` uses and does not declare, that are not bound by `bound` (the module's other
/// imports) and are not the runtime's: the candidates for the type imports and the namespace
/// imports (1A.36b-2). A name a used commons' type is imported under (`__CommonsLine`,
/// `unbranded_commons_qual`) is read as the name it aliases.
///
/// A name that resolves to an import is never hidden by the global tables (`Request`/`Response`
/// of `bynk`).
pub(super) fn used_external_names(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    body: &[bynk_ts::TsStmt],
    bound: &std::collections::BTreeSet<String>,
) -> std::collections::BTreeSet<String> {
    let local_to_file = local_names(commons);
    // A TypeScript type keyword (`any`) is never an import, even where a used commons has a
    // function of that name.
    let resolves = |n: &str| {
        !TS_TYPE_KEYWORDS.contains(&n)
            && resolves_externally(external_alias_target(n), &local_to_file, ctx)
    };
    bynk_ts::unbound_names_in(body, resolves)
        .into_iter()
        .filter(|n| !bound.contains(n))
        .collect()
}

/// The lower-case names TypeScript reads as types, which no import brings in.
const TS_TYPE_KEYWORDS: &[&str] = &[
    "any",
    "unknown",
    "never",
    "void",
    "string",
    "number",
    "boolean",
    "object",
    "symbol",
    "bigint",
    "null",
    "undefined",
];

/// The prefix of a used commons type's import alias.
const COMMONS_ALIAS_PREFIX: &str = "__Commons";

/// The import alias of a used commons' `X` (`__CommonsX`). The one spelling: the import, the
/// type alias and every value reference name it through here, so they cannot drift apart
/// (1A.39b, Decision C).
pub(crate) fn commons_alias(name: &str) -> String {
    format!("{COMMONS_ALIAS_PREFIX}{name}")
}

/// `__CommonsX` is the import alias of a used commons' `X`: the inverse of [`commons_alias`].
fn external_alias_target(name: &str) -> &str {
    name.strip_prefix(COMMONS_ALIAS_PREFIX).unwrap_or(name)
}

/// The imports the names `used` ask for, each resolved to its home by [`record_name_ref`].
pub(super) fn external_references_of(
    used: &std::collections::BTreeSet<String>,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> ExternalReferences {
    let local_to_file = local_names(commons);
    let mut refs = ExternalReferences::default();
    for name in used {
        record_external_name(external_alias_target(name), &local_to_file, ctx, &mut refs);
    }
    refs
}

fn record_name_ref(
    name: &str,
    local_to_file: &HashSet<String>,
    ctx: &EmitProjectCtx,
    out: &mut ExternalReferences,
) {
    if local_to_file.contains(name) {
        return;
    }
    // Imported from another commons?
    if let Some(commons_name) = ctx.imported_from.get(name) {
        out.by_commons
            .entry(commons_name.clone())
            .or_default()
            .insert(name.to_string());
        return;
    }
    // Sibling file in the same commons?
    if let Some(path) = ctx.file_decl_index.types.get(name)
        && path != &ctx.source_path
    {
        out.by_sibling
            .entry(path.clone())
            .or_default()
            .insert(name.to_string());
        return;
    }
    if let Some(path) = ctx.file_decl_index.fns.get(name)
        && path != &ctx.source_path
    {
        out.by_sibling
            .entry(path.clone())
            .or_default()
            .insert(name.to_string());
    }
}

/// Emit `import * as <ns> from "..."` for each consumed context that
/// exposes services (so the consuming file can reference its `__makeSurface`
/// return type and brand the cross-context call arguments).
/// #1478: real-node-internally already — returns one real `TsDecl::
/// ImportNamespace` per consumed context instead of writing into
/// `out: &mut String`.
pub(super) fn emit_cross_context_namespace_imports(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    used: &std::collections::BTreeSet<String>,
) -> Vec<bynk_ts::TsStmt> {
    let info = &ctx.cross_context;
    // 1A.36b-2: a consumed context's namespace is imported iff the module's tree uses it
    // (`ns.__makeSurface`, `ns.Code`, `ns.Cap`).
    let mut candidates: std::collections::BTreeSet<String> =
        info.consumed_contexts.iter().cloned().collect();
    candidates.extend(info.consumed_services.keys().cloned());
    candidates.extend(cross_context_cap_namespaces(commons, ctx));
    let needed: std::collections::BTreeSet<String> = candidates
        .into_iter()
        .filter(|q| used.contains(&qualified_to_ns(q)))
        .collect();
    if needed.is_empty() {
        return Vec::new();
    }
    let mut stmts = Vec::new();
    let consumed_with_services: Vec<&String> = needed.iter().collect();
    for q in &consumed_with_services {
        // Pick the first known file path for the consumed context as the
        // import target. (The composition root lives in the consumed
        // context's directory; any of its files would work as an import
        // target since they're all in the same module namespace, but we
        // currently emit one file per .bynk source so a single import per
        // consumed name suffices for the surface contract.)
        let target_paths = ctx.imported_decl_paths.get(q.as_str());
        let target = target_paths
            // #3807: the smallest path, so the output does not depend on hash order. Any
            // single file is still wrong for a context whose declarations span several.
            .and_then(|m| m.values().min().cloned())
            .unwrap_or_else(|| {
                // No imported declaration pins the path (e.g. a capability-only
                // consumed context, v0.15). Fall back to the unit's own module:
                // its per-Worker handlers in workers mode, or its <segment>.bynk
                // source in bundle mode. v0.17: a consumed *adapter* is not a
                // Worker — its capability types live in its root module
                // (`<adapter>.ts`) in both targets.
                if ctx.consumed_adapters.contains(q.as_str()) {
                    let mut p = EmitProjectCtx::commons_path(q);
                    p.set_extension("bynk");
                    p
                } else {
                    match ctx.target {
                        BuildTarget::Workers => ctx.worker_layout.handlers_source_path(q),
                        BuildTarget::Bundle => {
                            let mut p = EmitProjectCtx::commons_path(q);
                            p.set_extension("bynk");
                            p
                        }
                    }
                }
            });
        let import =
            cross_commons_import_specifier_for_path(&ctx.source_path, &target, ctx.import_ext);
        let ns = qualified_to_ns(q);
        // #661: under `workers`, a consumed *context*'s module is imported for
        // its **types only** — the caller now generates its own codecs
        // (`emit_boundary_helpers`) and reaches the callee's types through this
        // alias in type position (`deps: { Clock: platform_time.Clock }`,
        // `Result<commerce_payment.AuthId, …>`). An `import type` is erased
        // outright, so the callee's *module* — and its provider implementation —
        // never enters the caller's Worker bundle. This does **not** apply to a
        // consumed *adapter* (its binding namespace, e.g. `tokens`, is a real
        // value import used by `compose.ts`) nor on `bundle` (contexts compile
        // together, and the value uses in `compose.ts` are legitimate).
        let type_only = matches!(ctx.target, BuildTarget::Workers)
            && !ctx.consumed_adapters.contains(q.as_str());
        // Arc C, slice 30 (#1392): a real `TsDecl::ImportNamespace` —
        // `type_only` (#1392's own new field) covers the `import type * as`
        // form.
        stmts.push(bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::ImportNamespace {
                type_only,
                alias: ns,
                from: import,
            },
            None,
        ));
    }
    // #1486: no explicit trailing blank stmt here — the printer's automatic
    // top-level spacing policy supplies the single blank before whatever
    // follows (never import-kind at this function's own call sites, so
    // never exempted from it). The boundary this function's own *first*
    // statement forms with `emit_project_imports`'s own last statement
    // (both import-kind, genuinely wanting a blank despite the "no blank
    // between adjacent imports" exemption) is handled at `emit_project`'s
    // own call site instead — the only place that knows both neighbours.
    stmts
}

/// #1478: real-node-internally already — returns one real `TsDecl::Import`
/// per sibling/cross-unit import group instead of writing into
/// `out: &mut String`.
pub(super) fn emit_project_imports(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    refs: &ExternalReferences,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    // Events track, slice 0 (spine #936): the bare event-type names this
    // context's own `from Events(E)` service headers name — see the
    // Workers type-only-import narrowing below.
    // P6.24a/P6.19: reads the protocol's own resolved `ProtocolIr::Events`
    // instead of matching `ServiceProtocol::Events { event_type:
    // TypeRef::Named(id), .. }` directly — a resolve miss (`lower_protocol_
    // ir_from_commons` degrades to `Ty::Unit` on one, never panics) is
    // indistinguishable from a legitimately non-`Named` header, so falls
    // through to `None` exactly like the raw match's own `_ => None` arm
    // did, not a new failure mode.
    let subscribed_event_type_names: HashSet<String> = commons
        .commons
        .items
        .iter()
        .filter_map(CommonsItem::as_service)
        .filter_map(|s| {
            let bynk_model::ProtocolIr::Events { event, .. } =
                bynk_model::lower_protocol_ir_from_commons(&s.protocol, commons)
            else {
                return None;
            };
            let Ty::Named { name, .. } = &*commons.tys().get(event) else {
                return None;
            };
            Some(name.clone())
        })
        .collect();
    // Sibling imports: relative path within the same commons/context directory.
    let mut sibling_paths: Vec<(&PathBuf, &HashSet<String>)> = refs.by_sibling.iter().collect();
    sibling_paths.sort_by(|a, b| a.0.cmp(b.0));
    for (path, names) in sibling_paths {
        let import = sibling_import_specifier(&ctx.source_path, path, ctx.import_ext);
        let mut sorted: Vec<&String> = names.iter().collect();
        sorted.sort();
        // Arc C, slice 30 (#1392): a real `TsDecl::Import`.
        stmts.push(bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Import {
                type_only: false,
                names: sorted.iter().map(|s| ts_ident(s)).collect(),
                from: import,
            },
            None,
        ));
    }
    let mut type_paths: Vec<(&PathBuf, &HashSet<String>)> = refs.types_by_sibling.iter().collect();
    type_paths.sort_by(|a, b| a.0.cmp(b.0));
    for (path, names) in type_paths {
        let mut sorted: Vec<&String> = names.iter().collect();
        sorted.sort();
        stmts.push(bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Import {
                type_only: true,
                names: sorted.iter().map(|s| ts_ident(s)).collect(),
                from: sibling_import_specifier(&ctx.source_path, path, ctx.import_ext),
            },
            None,
        ));
    }
    // Cross-unit imports: group by *target file path*.
    let mut unit_names: Vec<(&String, &HashSet<String>)> = refs.by_commons.iter().collect();
    unit_names.sort_by(|a, b| a.0.cmp(b.0));
    for (unit_name, names) in unit_names {
        let target_paths = ctx.imported_decl_paths.get(unit_name.as_str());
        let mut by_target: std::collections::BTreeMap<PathBuf, Vec<&String>> =
            std::collections::BTreeMap::new();
        for n in names {
            let path = target_paths
                .and_then(|p| p.get(n))
                .cloned()
                .unwrap_or_else(|| EmitProjectCtx::commons_path(unit_name));
            by_target.entry(path).or_default().push(n);
        }
        for (target, mut name_list) in by_target {
            name_list.sort();
            let import =
                cross_commons_import_specifier_for_path(&ctx.source_path, &target, ctx.import_ext);
            // For context units, aliase commons-source imports so we can emit
            // rebrand aliases of the same short name. Imports from consumed
            // contexts keep their original name. v0.20b: the rebrand applies
            // to *types* only — a `uses`-imported function (bynk.list's
            // `traverse`) is a value, imports plainly, and is never branded.
            let mut parts: Vec<String> = Vec::new();
            for n in &name_list {
                let is_subscribed_event_type = ctx.target == BuildTarget::Workers
                    && subscribed_event_type_names.contains(n.as_str());
                // #2045: under Workers, a consumed *context*'s type is named in
                // type position only — its codecs are the caller's own
                // (`emit_consumed_context_helpers`) — so it imports type-only and
                // the callee's module never enters the caller's Worker bundle
                // (#661). A consumed *adapter* is linked into each Worker, and
                // its values (`Method.Get`) are real imports.
                let is_consumed_context_type = ctx.target == BuildTarget::Workers
                    && ctx.unit_kind == UnitKind::Context
                    && !ctx.consumed_adapters.contains(unit_name.as_str())
                    && ctx.imported_from_kind.get(n.as_str()) == Some(&UnitKind::Context)
                    && !ctx.consumed_types_with_methods.contains(n.as_str());
                // R4.10/R8.2: the same shared `is_uses_commons_type` predicate
                // `emit_context_rebrands` below reads — this import-aliasing
                // site is that function's own step 1 ("Done in imports", its
                // own doc comment), and the two must agree exactly: an alias
                // narrower than the rebrand leaves an undefined name in the
                // generated import; a rebrand narrower than the alias leaves
                // an alias imported and never used. `ctx.unit_kind ==
                // UnitKind::Context` is the same guard as
                // `emit_context_rebrands`'s own `ctx.owning_context.is_some()`
                // early return (`owning_context` is `Some` exactly when
                // `unit_kind == Context`, `project.rs`'s own construction) —
                // kept explicit here since this loop runs for every unit
                // kind, not just contexts.
                if ctx.unit_kind == UnitKind::Context
                    && bynk_check::resolver::compute_is_uses_commons_type(
                        &ctx.imported_from_kind,
                        &commons.types,
                        n,
                    )
                {
                    parts.push(format!("{n} as {}", commons_alias(n)));
                } else if is_subscribed_event_type || is_consumed_context_type {
                    // Events track, slice 0 (spine #936): under Workers, a
                    // context deploys as its own separate Worker script —
                    // there is no shared module graph to import a peer
                    // context's *value* across (the #661 hazard this
                    // mirrors: a caller generates its own codec rather than
                    // importing the callee's runtime code). `from
                    // Events(E)`'s `E` is the one plain named type crossing
                    // a context boundary directly by name (every other
                    // cross-context reference goes through a generated
                    // Service-Binding codec instead) — used only in type
                    // position (`e: E`), so this specific name is type-only.
                    // Narrowly scoped to event types specifically, not every
                    // cross-context import: a `type`/`enum` crossing via
                    // `uses`/`consumes` (e.g. `bynk`'s `Method`) is often
                    // used as a *value* too (`Method.Get`), which a blanket
                    // `import type` would wrongly break.
                    parts.push(format!("type {}", import_spec_of(ctx, n)));
                } else {
                    parts.push(import_spec_of(ctx, n));
                }
            }
            // Arc C, slice 30 (#1392): a real `TsDecl::Import` — `parts`'
            // own per-name `as __CommonsX`/`type X` prefixes already match
            // `names`'s own documented "raw text slot" convention, no new
            // gap.
            stmts.push(bynk_ts::TsStmt::decl(
                bynk_ts::TsDecl::Import {
                    type_only: false,
                    names: parts,
                    from: import,
                },
                None,
            ));
        }
    }
    stmts.extend(extra_import_stmts(ctx));
    // #1486: every statement in this function's own return, whatever
    // section it came from (sibling imports, cross-unit imports, the
    // rebrand-aliasing pair, `extra_imports`), sits directly under its
    // predecessor with no blank between them — this function never inserted
    // one under the pre-#1486 flat-print regime (confirmed: no explicit blank
    // `TsStmt` anywhere in its own body), so every internal adjacency is forced
    // regardless of `TsStmtKind`. The very first statement is left alone — its
    // own blank-or-not relative to whatever precedes this function's call
    // site is `emit_project`'s own call-site decision (the header/`emit_
    // project_imports` boundary, and the project/cross-context import
    // boundary, both forced explicitly there instead).
    for stmt in stmts.iter_mut().skip(1) {
        stmt.no_blank_before = true;
    }
    stmts
}

/// #527: imports the DO-side agent-deps expressions need (binding modules,
/// other Workers' handlers, the locale `render`). The project driver decides
/// which; each is an import node here.
pub(super) fn extra_import_stmts(ctx: &EmitProjectCtx) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    for import in &ctx.extra_imports {
        if let ExtraImport::Qualifier { qualifier, from } = import {
            stmts.extend(qualifier_stmts(qualifier, from));
            continue;
        }
        let decl = match import {
            ExtraImport::Namespace { alias, from } => bynk_ts::TsDecl::ImportNamespace {
                type_only: false,
                alias: alias.clone(),
                from: from.clone(),
            },
            ExtraImport::Named {
                names,
                from,
                type_only,
            } => bynk_ts::TsDecl::Import {
                type_only: *type_only,
                names: names.clone(),
                from: from.clone(),
            },
            ExtraImport::Qualifier { .. } => unreachable!("handled above"),
        };
        stmts.push(bynk_ts::TsStmt::decl(decl, None));
    }
    stmts
}

/// #4045: the statements that make a `uses` qualifier readable in TypeScript. An
/// alias (`Y`) is the module's namespace import. A commons' full name (`lib.y`) is
/// imported under a private name and hung on a `namespace`, so `lib.y.describe(x)`
/// and the type `lib.y.Status` read as written.
fn qualifier_stmts(qualifier: &str, from: &str) -> Vec<bynk_ts::TsStmt> {
    let Some((head, rest)) = qualifier.split_once('.') else {
        return vec![bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::ImportNamespace {
                type_only: false,
                alias: qualifier.to_string(),
                from: from.to_string(),
            },
            None,
        )];
    };
    let private = ["__use_", &qualifier.replace('.', "_")].concat();
    let segments: Vec<&str> = rest.split('.').collect();
    // `namespace a { export namespace b { export import c = __use_a_b_c; } }`
    let mut text = ["namespace ", head, " { "].concat();
    let mut close = String::from("}");
    for seg in &segments[..segments.len() - 1] {
        text.push_str("export namespace ");
        text.push_str(seg);
        text.push_str(" { ");
        close.push_str(" }");
    }
    for part in [
        "export import ",
        segments[segments.len() - 1],
        " = ",
        &private,
        "; ",
        &close,
    ] {
        text.push_str(part);
    }
    vec![
        bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::ImportNamespace {
                type_only: false,
                alias: private.clone(),
                from: from.to_string(),
            },
            None,
        ),
        bynk_ts::TsStmt::raw_using(text, &[private.as_str()]),
    ]
}

/// #2887: `n` as an import specifier. A type this unit renames with `consumes
/// U { T as Alias }` is imported under the name `U` declares and bound to the
/// alias (`Digest as CryptoDigest`), so every use in the module reaches the
/// module's real type, not a new one.
fn import_spec_of(ctx: &EmitProjectCtx, n: &str) -> String {
    match ctx.consumed_type_aliases.get(n) {
        Some(original) => format!("{} as {}", ts_ident(original), ts_ident(n)),
        None => ts_ident(n),
    }
}

/// Compute a relative import specifier from `from_source` (a `.bynk` path)
/// to `to_source` (another `.bynk` path), with `.bynk` rewritten to `.js`
/// for compatibility with NodeNext/strict TS resolution.
fn sibling_import_specifier(from_source: &Path, to_source: &Path, ext: ImportExt) -> String {
    let from_dir = from_source.parent().unwrap_or(Path::new(""));
    let target = to_source.with_extension(ext.as_str());
    let rel = relative_to(from_dir, &target);
    format!("./{}", ts_specifier(&rel))
}

/// Render a path as a TypeScript module specifier: **always forward
/// slashes**. `Path::display()` uses the platform separator, and on Windows
/// that emitted `import ... from "./commerce\orders.js"` — broken ESM
/// output, caught by the first CI matrix run on windows-latest.
pub fn ts_specifier(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Compute a relative import specifier from this file's location to a
/// specific source file in another commons. `target_source` is the project-
/// relative path of the target `.bynk` file. The result is suitable for
/// `import { ... } from "..."` in NodeNext/strict TypeScript.
pub fn cross_commons_import_specifier_for_path(
    from_source: &Path,
    target_source: &Path,
    ext: ImportExt,
) -> String {
    let from_dir = from_source.parent().unwrap_or(Path::new(""));
    let target = target_source.with_extension(ext.as_str());
    let rel = relative_to(from_dir, &target);
    let display = ts_specifier(&rel);
    if display.starts_with("../") || display.starts_with("./") {
        display
    } else {
        format!("./{display}")
    }
}

/// Compute `target` as a path relative to `from`. Handles parent traversal
/// (`..`) for cases where `target` lives in a sibling directory.
fn relative_to(from: &Path, target: &Path) -> PathBuf {
    use std::path::Component as C;
    let f_comps: Vec<C> = from.components().collect();
    let t_comps: Vec<C> = target.components().collect();
    let mut shared = 0;
    while shared < f_comps.len() && shared < t_comps.len() && f_comps[shared] == t_comps[shared] {
        shared += 1;
    }
    let mut out = PathBuf::new();
    for _ in shared..f_comps.len() {
        out.push("..");
    }
    for c in &t_comps[shared..] {
        out.push(c.as_os_str());
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}
