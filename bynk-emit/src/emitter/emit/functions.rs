//! Methods, free functions, constants and contract guards: attached and forwarded methods,
//! `fn` declarations (`emit_free_fn`), `const` declarations and the `requires`/`ensures` guard
//! statements around a body.

use super::{cap_ref_ty, consumed_type_ns, iife_const_item, ts_type_params};
use crate::emit_model::EmitProjectCtx;
use crate::emitter::ctx::{BodyMode, HandlerShared, ModuleCtx};
use crate::emitter::lower_ctx::LowerCtx;
use crate::emitter::lower_stmt::{Body, Item, block_body_with_return, function_body_stmts};
use crate::emitter::ts_types::{ts_ty_to_ts_type, ts_type_ref_to_ts_type};
use crate::emitter::{Pre, RuntimeUse, TyRender, UnitFacts, ts_ident};
use bynk_check::checker::TypedCommons;
use bynk_model::{FnSig, is_effectful_return};
use bynk_syntax::ast::{CommonsItem, ConstDecl, FnDecl, FnName, Ident, TypeParam};

/// #1337: returns real `bynk_ts::TsObjectEntry`s (was `out: &mut String`) —
/// #1478's own `emit_refined_type`/`emit_record_type`/`emit_sum_type`
/// conversion appends these directly into a real `TsExpr::Object`'s own
/// entries, with no opaque-entries carrier needed at all (the question this
/// slice's own accepted proposal was asked to resolve).
pub(super) fn emit_attached_methods(
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
                Some(ts_ty_to_ts_type(*ty, tys)),
            ));
            args.push(bynk_ts::TsExpr::Ident(ident));
        }
        let ret = ts_ty_to_ts_type(f.return_ty, tys);
        let call = bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident(super::commons_alias(type_name))),
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
            return_type: Some(ret),
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
                    array_pattern: None,
                    default: None,
                }],
                bynk_ts::TsType::named("void"),
            ),
        ])),
        optional: false,
        rest: false,
        array_pattern: None,
        default: None,
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
            Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
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
        super::tco::loop_body_stmts(f, &plan, &mut cx, is_async, |_| Body::new())
    } else {
        function_body_stmts(&f.body, &mut cx, is_async, Some(&f.return_type))
    };
    bynk_ts::TsObjectEntry::Method {
        name: method_name.name.clone(),
        is_async,
        generics,
        params,
        return_type: Some(ts_type_ref_to_ts_type(&f.return_type, None)),
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

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_free_fn_in(
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
                    .zip(&f.given)
                    .map(|(c, g)| {
                        bynk_ts::TsTypeMember::prop(
                            c.name.clone(),
                            given_entry_ty(c, g, cross_context),
                        )
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
        super::tco::loop_body_stmts(f, &plan, &mut cx, async_tail, guards)
    } else if guarded {
        contract_guarded_body_stmts(f, &mut cx, async_tail)
    } else {
        function_body_stmts(&f.body, &mut cx, async_tail, Some(&f.return_type))
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
pub(super) fn emit_const_in(c: &ConstDecl, module: ModuleCtx<'_>) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    {
        let mut pre = Pre::new();
        let mut icx = LowerCtx::new(module, BodyMode::StaticInit);
        let init = pre.lower_node(&c.init, &mut icx);
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
                    Some(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Paren(Box::new(
                            bynk_ts::TsExpr::NullishAssign {
                                target: Box::new(bynk_ts::TsExpr::Ident(cache)),
                                value: Box::new(bynk_ts::TsExpr::object(vec![(
                                    "v".to_string(),
                                    init,
                                )])),
                            },
                        ))),
                        property: "v".to_string(),
                    }),
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
        let pred = pre.lower_node(&c.predicate, cx);
        body.extend(pre.items());
        let msg = format!(
            "contract violated: precondition \\`{clause}\\` of {fn_name} ({dump})",
            clause = c.name.name,
            dump = contract_param_dump(f, false),
        );
        body.push(Item::Node(Box::new(contract_guard_if_stmt(pred, &msg))));
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
/// 1A.15: the body is the statements of the function body, not text. 1A.ws: the closure the
/// original body runs in is a node too ([`iife_const_item`]), the body its arrow's statements.
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
    let original = block_body_with_return(&f.body, cx, async_tail, Some(&f.return_type));
    body.push(iife_const_item("result", async_tail, original));
    // Postcondition guards — `result` (and the parameters) are in scope.
    for c in &f.ensures {
        let mut pre = Pre::new();
        let pred = pre.lower_node(&c.predicate, cx);
        body.extend(pre.items());
        let msg = format!(
            "contract violated: postcondition \\`{clause}\\` of {fn_name} ({dump})",
            clause = c.name.name,
            dump = contract_param_dump(f, true),
        );
        body.push(Item::Node(Box::new(contract_guard_if_stmt(pred, &msg))));
    }
    body.push(Item::Node(Box::new(bynk_ts::TsStmt::return_stmt(
        Some(bynk_ts::TsExpr::Ident("result".to_string())),
        None,
    ))));
    body.into_stmts()
}

/// Shared builder for the contract call-site guard's own real `if (!(pred))
/// { const __e = new Error(`msg`); __e.name = "BynkContractError"; throw
/// __e; }` shape — identical for a `requires`/`ensures` clause, differing
/// only in `pred`/`msg`, both still-opaque text (see `emit_contract_guarded_
/// body`'s own doc for why).
fn contract_guard_if_stmt(pred: bynk_ts::TsExpr, msg: &str) -> bynk_ts::TsStmt {
    let cond = bynk_ts::TsExpr::Unary {
        op: bynk_ts::TsUnaryOp::Not,
        expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(pred))),
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

/// The type of one entry of a `given` function's hidden `deps`: the
/// capability's interface, or (#3959) for a capability parameter
/// (`Store: Kv`) its bound kind's, so any instance of the kind satisfies it.
fn given_entry_ty(
    c: &bynk_model::CapRefIr,
    g: &bynk_syntax::ast::CapRef,
    cross_context: &bynk_check::resolver::CrossContextInfo,
) -> bynk_ts::TsType {
    match &g.bound {
        Some(bound) => cap_ref_ty(
            &bynk_model::CapRefIr {
                context: None,
                name: bound.name.clone(),
            },
            cross_context,
        ),
        None => cap_ref_ty(c, cross_context),
    }
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
