//! The cross-context service call (back-half paydown 1A.25, #3726): the bundle call
//! `deps.surface.<key>.<method>(args)`, the Workers `__callService(…)`, and the boundary casts
//! (`param_cast`, `return_cast`) they share. Moved out of `emit.rs` and `lower.rs` byte for byte
//! (Decision E) before they convert to nodes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bynk_check::checker::Callee;
use bynk_check::kernel_op::{ListStatic, MapStatic, StaticOp};
use bynk_ts::{TsExpr, TsType};

use super::super::emit::{flatten_emit_ident_chain, test_scaffold_ns};
use super::super::lower::Pre;
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{call, id, member, paren};
use super::super::{TypeBody, TypeDecl, UnaryOp, *};

/// v0.6 cross-context service call: receiver is an alias or the dotted name
/// of a consumed context. In bundle mode, lower to
/// `deps.surface.<key>.<method>(args as <consumed_ns>.<T>)`; in workers mode
/// (v0.8), lower to
/// `callService(deps.env.<BINDING>, "<method>", serialise...(args), deserialise_<R>)`.
/// Returns `None` when the receiver is not a consumed-context prefix.
///
/// 1A.25 (#3726): a node.
pub(in crate::emitter) fn lower_cross_context_service_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    let (consumed, key) = cross_context_lowering_prefix(receiver, cx)?;
    cx.note_cross_context_used(&consumed);
    Some(match cx.target() {
        BuildTarget::Bundle => {
            let args_lowered: Vec<TsExpr> = args
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let lowered = pre.lower_node(a, cx);
                    // #2042: every argument at a generic parameter
                    // that names a user type is cast, not only one
                    // holding a callee-typed part (#2039): a value the
                    // caller typed itself (`byKey: Map[String, Line]`)
                    // carries the caller's brand against the callee's.
                    param_cast(
                        &consumed,
                        cx.cross_context(),
                        method,
                        i,
                        lowered,
                        true,
                        cx.in_test_scaffold(),
                    )
                })
                .collect();
            let surface = member(member(id("deps"), "surface"), &key);
            let service = call(member(surface, &method.name), args_lowered);
            // #2825: the result takes the caller's declaration of a
            // same-named refined or opaque type, whose brand differs.
            return_cast(
                &consumed,
                cx.cross_context(),
                &cx.commons().types,
                cx.consumed_type_ns(),
                method,
                service,
                cx.in_test_scaffold(),
            )
        }
        BuildTarget::Workers => {
            lower_workers_cross_context_call(&consumed, method, args, receiver.span.start, cx, pre)
        }
    })
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
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::DecimalLit { .. }
        | ExprKind::StrLit(_) => true,
        ExprKind::UnaryOp(UnaryOp::Neg, inner) => {
            matches!(
                inner.kind,
                ExprKind::IntLit { .. } | ExprKind::FloatLit { .. } | ExprKind::DecimalLit { .. }
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
        // 1A.33: the recorded intrinsic, not the receiver's spelling.
        ExprKind::MethodCall { args, .. } => {
            args.is_empty()
                && matches!(
                    cx.commons().callee(e.id),
                    Some(Callee::Intrinsic {
                        op: StaticOp::List(ListStatic::Empty) | StaticOp::Map(MapStatic::Empty),
                        ..
                    })
                )
        }
        ExprKind::Paren(inner)
        | ExprKind::Some(inner)
        | ExprKind::Ok(inner)
        | ExprKind::Err(inner) => holds_callee_typed(inner, cx),
        ExprKind::ListLit(items, ..) => items.iter().any(|i| holds_callee_typed(i, cx)),
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
    // #3580: where the call starts in the source, for a traced build's span.
    call_start: usize,
    cx: &mut LowerCtx<'_>,
    pre: &mut Pre,
) -> bynk_ts::TsExpr {
    use crate::emitter::serialisation::{deserialise_ref_via, serialise_expr_via};

    let info = cx.cross_context();
    let binding = crate::emitter::wrangler::service_binding(consumed).name;

    // Look up the service signature on the consumed context.
    //
    // v0.176 (#642, Decision C): the checker resolved this call before the
    // emitter ran, so an absent signature is the emitter disagreeing with the
    // checker — a compiler bug. It used to lower to `value as JsonValue` and an
    // `((j: any) => ({ tag: "Ok", value: j }))` identity deserialiser, shipping
    // an unvalidated `any` to production to paper over that bug. Fail instead:
    // 1A.34 reports it as a `bynk.internal.missing_checker_fact` fault at the call.
    let Some(svc) = info
        .consumed_services
        .get(consumed)
        .and_then(|map| map.get(&method.name))
    else {
        return cx.missing_fact(
            method.span,
            format_args!("service signature for `{consumed}.{}`", method.name),
        );
    };

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

    // One invariant for arity, checked once, in both directions. The checker
    // validated it (`bynk.consumes.service_arity`) before emit ran, so a mismatch
    // either way is the emitter disagreeing with the checker — the same internal
    // fault the signature lookup above reports, and it gets the same answer.
    // (Previously the two directions disagreed: a surplus argument panicked, while
    // a missing one silently emitted `null` into the args object — quietly sending
    // a wrong payload for exactly the fault the other direction refused to ship.)
    if args.len() != svc.params.len() {
        return cx.missing_fact(
            method.span,
            format_args!(
                "argument count for `{consumed}.{}`: its signature takes {} but the call has {}",
                method.name,
                svc.params.len(),
                args.len()
            ),
        );
    }

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
    let mut args_serialised: Vec<TsExpr> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        let mut lowered = pre.lower_node(a, cx);
        let (_, param_ty) = &svc.params[i];
        let wire_ty = renamed(param_ty);
        // #2025, #2039: a literal in a cross-context argument (directly, or
        // nested in a payload, list or record field) is admitted to the
        // callee's type, but lowers with the brand of the caller's name for
        // it. Where the caller doesn't name the callee's type (it was renamed
        // above), cast the argument to what the codec takes.
        if renames.is_some_and(|r| type_ref_names_any(param_ty, r)) && holds_callee_typed(a, cx) {
            // `globalThis.Parameters<typeof codec>[0]`: the codec's parameter type.
            let codec =
                crate::emitter::serialisation::serialise_ref_via(&wire_ty, &ns, cx.runtime_use());
            let codec_param = TsType::indexed(
                TsType::named_with_args("globalThis.Parameters", vec![TsType::type_of(codec)]),
                TsType::number_literal(0),
            );
            lowered = cast_through_unknown(lowered, codec_param);
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
    // escaping concern. The zero-arg case is an empty `Object`, printed `{}`: it
    // was the opaque `Ident("{  }")` (two spaces) while three sites carried
    // that quirk (#1321, #1327); 1A.25 (#3726, Decision D) takes the layout
    // change here, and `workers.rs` and `project.rs` keep their own text.
    let args_json_expr = if args_serialised.is_empty() {
        TsExpr::object_entries(Vec::new())
    } else if args_serialised.len() == 1 {
        args_serialised.into_iter().next().unwrap()
    } else {
        let entries: Vec<bynk_ts::TsObjectEntry> = svc
            .params
            .iter()
            .zip(args_serialised)
            .map(|((name, _), serialised)| bynk_ts::TsObjectEntry::prop(name.clone(), serialised))
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
    // #2901: the signature names a type the callee renamed by the key this
    // context holds it under; the callee hashes its own alias.
    let realiased;
    let svc = match info.callee_alias_keys.get(consumed) {
        Some(keys) => {
            let mut s = svc.clone();
            for (_, tr) in s.params.iter_mut() {
                bynk_check::symbols::rename_type_ref(tr, keys);
            }
            bynk_check::symbols::rename_type_ref(&mut s.return_type, keys);
            realiased = s;
            &realiased
        }
        None => svc,
    };
    let contract = match info.consumed_types.get(consumed) {
        Some(types) => match info.consumed_type_aliases.get(consumed) {
            // #2887: the callee canonicalises its own names; undo this
            // context's aliases so both hashes read the same declarations.
            Some(aliases) => {
                let mut svc = svc.clone();
                for (_, tr) in svc.params.iter_mut() {
                    bynk_check::symbols::rename_type_ref(tr, aliases);
                }
                bynk_check::symbols::rename_type_ref(&mut svc.return_type, aliases);
                let types: HashMap<String, Arc<TypeDecl>> = types
                    .iter()
                    .map(|(k, d)| {
                        let key = aliases.get(k).unwrap_or(k);
                        (
                            key.clone(),
                            bynk_check::symbols::rename_consumed_decl(d, key, aliases),
                        )
                    })
                    .collect();
                bynk_check::contract::service_contract_hash(&svc, &types)
            }
            None => bynk_check::contract::service_contract_hash(svc, types),
        },
        // Unreachable alongside the signature assertion above: the same resolver
        // pass populates both maps for a consumed context.
        None => unreachable!(
            "consumed_types missing for `{consumed}`, though its service signature resolved"
        ),
    };

    let call_expr = bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Ident("__callService".to_string())),
        args: vec![
            cx.trace_binding(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("deps".to_string())),
                    property: "env".to_string(),
                }),
                property: binding,
            }),
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
    let target = format!("{consumed}.{}", method.name);
    cx.trace_step_node("call", &target, call_start, call_expr)
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
    arg: TsExpr,
    // Whether an argument at a generic parameter that names a user type is
    // cast too (the bundle path, #2039, #2042); the workers path casts only a
    // named parameter type, since its instantiation codecs are unbranded.
    cast_generic: bool,
    in_test_scaffold: bool,
) -> TsExpr {
    match param_cast_type(consumed, info, method, idx, cast_generic, in_test_scaffold) {
        Some(ty) => cast_through_unknown(arg, ty),
        None => arg,
    }
}

/// `(value as unknown as ty)`: the cast every boundary crossing takes, built as
/// `Paren(As(As(value, unknown), ty))` (the printer leaves the left-nested `As` unbracketed, so
/// this prints `(value as unknown as ty)`).
fn cast_through_unknown(value: TsExpr, ty: TsType) -> TsExpr {
    paren(TsExpr::As {
        expr: Box::new(TsExpr::As {
            expr: Box::new(value),
            ty: TsType::named("unknown"),
        }),
        ty,
    })
}

/// The type [`param_cast`] casts argument `idx` to, or `None` when it takes no cast.
fn param_cast_type(
    consumed: &str,
    info: &bynk_check::resolver::CrossContextInfo,
    method: &Ident,
    idx: usize,
    cast_generic: bool,
    in_test_scaffold: bool,
) -> Option<TsType> {
    let svcs = info.consumed_services.get(consumed)?;
    let service = svcs.get(&method.name)?;
    let (_, ptype_ref) = service.params.get(idx)?;
    // #2890: the owner table is keyed by the names as `consumed` spells them.
    let spelled = type_ref_named_root(ptype_ref).map(str::to_string);
    let ptype_ref = &unalias_type_ref(info, consumed, ptype_ref);
    if let Some(name) = type_ref_named_root(ptype_ref) {
        // #2073: a type the consumed context itself consumes is named through
        // its declaring context's namespace. This relies on the checker
        // refusing a caller that does not consume that context
        // (`bynk.consumes.signature_type_unconsumed`), so the namespace is
        // always imported here.
        let owner = info
            .consumed_type_owners
            .get(consumed)
            .and_then(|o| {
                o.get(name)
                    .or_else(|| spelled.as_ref().and_then(|s| o.get(s)))
            })
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
        // tell TypeScript to trust the structural Bynk-side check
        // ([`cast_through_unknown`]; a nested `As` chain since 1A.25, #3726).
        return Some(TsType::named(format!("{ns}.{name}")));
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
            return Some(crate::emitter::ts_types::ts_type_ref_qualified_ts_type(
                ptype_ref, &names, &ns,
            ));
        }
    }
    None
}

/// #2887: `tr` with the types this context renames from `consumed` spelled as
/// `consumed` declares them: its module exports those names, not the aliases.
fn unalias_type_ref(
    info: &bynk_check::resolver::CrossContextInfo,
    consumed: &str,
    tr: &TypeRef,
) -> TypeRef {
    let mut tr = tr.clone();
    if let Some(aliases) = info.consumed_type_aliases.get(consumed) {
        bynk_check::symbols::rename_type_ref(&mut tr, aliases);
    }
    // #2890: and the names `consumed` renamed itself. #2901: its signature
    // names them by the key this context holds them under, which is the alias
    // again first.
    if let Some(keys) = info.callee_alias_keys.get(consumed) {
        bynk_check::symbols::rename_type_ref(&mut tr, keys);
    }
    if let Some(aliases) = info.callee_type_aliases.get(consumed) {
        bynk_check::symbols::rename_type_ref(&mut tr, aliases);
    }
    tr
}

/// #2887: `consumed_type_ns` with each type this context renames from
/// `consumed` also keyed by its declared name.
fn consumed_type_ns_unaliased(
    info: &bynk_check::resolver::CrossContextInfo,
    consumed: &str,
    consumed_type_ns: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut out = consumed_type_ns.clone();
    for (alias, original) in info
        .consumed_type_aliases
        .get(consumed)
        .into_iter()
        .chain(info.callee_type_aliases.get(consumed))
        .flatten()
    {
        if let Some(ns) = consumed_type_ns.get(alias) {
            out.insert(original.clone(), ns.clone());
        }
    }
    out
}

/// #2825: cast the result of a bundle-mode cross-context call to the *caller's*
/// view of the service's return type.
///
/// A returned value is projected by name (spec §6.5): where the caller declares
/// a type of the same name, the value takes it. A refined or opaque type is
/// branded per context (`"a.Reply"` against `"b.Reply"`), and a `uses`d commons
/// type is rebranded per context (`__ctxBrand`), so the callee's value is not
/// assignable to the caller's declaration of the same name under
/// `tsc --strict`, although the checker proved the two compatible. Route the
/// call through `unknown` to the type as the caller names it, exactly as
/// [`param_cast`] does for an argument.
///
/// Each name in the cast type is spelled as the caller's module spells it: a
/// type of a consumed context under its namespace (`consumed_type_ns`, the
/// provider's own types such as `Token`, which the caller never redeclares), a
/// name the caller does not bind under the owning context's namespace, and
/// every other name bare. A bare commons name is defined by the module's
/// rebrand, which the emitter keeps because the cast spells it.
///
/// A call is cast when the return type reaches a refined or opaque type the
/// caller binds, directly or through the fields of a record or sum it binds
/// (#2828). Every other call lowers as before.
///
/// `in_test_scaffold` is carried for symmetry with [`param_cast`]: no test body
/// reaches this lowering today. Below `system` a case that crosses a context is
/// `bynk.tier.cross_context_needs_system`, and a `system` case lowers through
/// the Workers codecs.
pub(crate) fn return_cast(
    consumed: &str,
    info: &bynk_check::resolver::CrossContextInfo,
    caller_types: &HashMap<String, Arc<TypeDecl>>,
    consumed_type_ns: &HashMap<String, String>,
    method: &Ident,
    call: TsExpr,
    in_test_scaffold: bool,
) -> bynk_ts::TsExpr {
    let Some(service) = info
        .consumed_services
        .get(consumed)
        .and_then(|svcs| svcs.get(&method.name))
    else {
        return call;
    };
    let return_type = unalias_type_ref(info, consumed, &service.return_type);
    let consumed_type_ns = &consumed_type_ns_unaliased(info, consumed, consumed_type_ns);
    let mut names = HashSet::new();
    type_ref_names(&return_type, &mut names);
    if !reaches_brand(&names, caller_types, consumed_type_ns) {
        return call;
    }
    let ns_of = |owner: &str| {
        if in_test_scaffold {
            test_scaffold_ns(owner)
        } else {
            qualified_to_ns(owner)
        }
    };
    let type_ns: HashMap<String, String> = names
        .iter()
        .filter(|n| consumed_type_ns.contains_key(*n) || !caller_types.contains_key(*n))
        .map(|n| {
            if let Some(ns) = consumed_type_ns.get(n) {
                return (n.clone(), ns.clone());
            }
            let owner = info
                .consumed_type_owners
                .get(consumed)
                .and_then(|o| o.get(n))
                .map(String::as_str)
                .unwrap_or(consumed);
            (n.clone(), ns_of(owner))
        })
        .collect();
    let ty = crate::emitter::ts_types::ts_type_ref_qualified_multi_ts_type(&return_type, &type_ns);
    cast_through_unknown(call, ty)
}

/// #2828: whether any of `names`, or a type their caller-bound declarations
/// name in a field or payload, is a refined or opaque type the caller binds.
/// A consumed context's own type (`consumed_type_ns`) is never the caller's
/// declaration, so it neither counts nor is searched.
fn reaches_brand(
    names: &HashSet<String>,
    caller_types: &HashMap<String, Arc<TypeDecl>>,
    consumed_type_ns: &HashMap<String, String>,
) -> bool {
    let mut seen: HashSet<String> = HashSet::new();
    let mut todo: Vec<String> = names.iter().cloned().collect();
    while let Some(n) = todo.pop() {
        if consumed_type_ns.contains_key(&n) || !seen.insert(n.clone()) {
            continue;
        }
        let Some(decl) = caller_types.get(&n) else {
            continue;
        };
        let mut inner = HashSet::new();
        match &decl.body {
            TypeBody::Refined { .. } | TypeBody::Opaque { .. } => return true,
            TypeBody::Record(r) => {
                for f in &r.fields {
                    type_ref_names(&f.type_ref, &mut inner);
                }
            }
            TypeBody::Sum(sum) => {
                for v in &sum.variants {
                    for f in &v.payload {
                        type_ref_names(&f.type_ref, &mut inner);
                    }
                }
            }
        }
        todo.extend(inner);
    }
    false
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emitter::ctx::{BodyMode, ModuleCtx, UnitFacts};
    use crate::emitter::runtime_use::RuntimeUse;
    use bynk_check::checker::TypedCommons;
    use bynk_check::resolver::{CrossContextInfo, CrossContextService};
    use bynk_syntax::span::Span;

    fn ident(name: &str) -> Ident {
        Ident {
            name: name.to_string(),
            span: Span::default(),
        }
    }

    fn named(name: &str) -> TypeRef {
        TypeRef::Named(ident(name))
    }

    /// A consumer that consumes `shop.pay`, whose `charge` service takes `params` (none, when
    /// `signed` is false: the consumed context then has no signature for it at all).
    fn info_with(signed: bool, params: Vec<(String, TypeRef)>) -> CrossContextInfo {
        let mut info = CrossContextInfo::default();
        info.consumed_contexts.push("shop.pay".to_string());
        if signed {
            let service = CrossContextService {
                name: "charge".to_string(),
                params,
                return_type: named("Receipt"),
                span: Span::default(),
                foreign_callers: Vec::new(),
            };
            info.consumed_services.insert(
                "shop.pay".to_string(),
                std::iter::once(("charge".to_string(), service)).collect(),
            );
        }
        info
    }

    fn lower_workers(
        info: &CrossContextInfo,
        args: &[Expr],
    ) -> (bynk_ts::TsExpr, Vec<crate::emitter::InternalFault>) {
        let commons = TypedCommons::empty();
        let ru = RuntimeUse::default();
        let module = ModuleCtx::new(&commons, info, &ru, UnitFacts::detached());
        let mut cx = LowerCtx::new(module, BodyMode::Method);
        let mut pre = Pre::new();
        let built = lower_workers_cross_context_call(
            "shop.pay",
            &ident("charge"),
            args,
            0,
            &mut cx,
            &mut pre,
        );
        (built, ru.take_faults())
    }

    /// The checker resolved the call before the emitter ran, so an absent signature is a compiler
    /// bug and fails (#642), as it did before the call was a node.
    #[test]
    fn a_call_with_no_signature_is_an_internal_fault() {
        let (built, faults) = lower_workers(&info_with(false, Vec::new()), &[]);
        assert_eq!(bynk_ts::print_expr(&built), "undefined");
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert_eq!(faults[0].span, ident("charge").span);
        assert!(
            faults[0]
                .what
                .contains("service signature for `shop.pay.charge`"),
            "{faults:?}"
        );
    }

    /// An arity mismatch is the same fault, in either direction, and lowering continues.
    #[test]
    fn a_call_whose_arity_disagrees_with_the_signature_is_an_internal_fault() {
        let info = info_with(true, vec![("total".to_string(), named("Int"))]);
        let (built, faults) = lower_workers(&info, &[]);
        assert_eq!(bynk_ts::print_expr(&built), "undefined");
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert!(
            faults[0].what.contains("takes 1 but the call has 0"),
            "{faults:?}"
        );
    }

    /// A zero-argument call is a node whose payload is the empty object.
    #[test]
    fn a_zero_argument_workers_call_is_a_node_with_an_empty_payload() {
        let mut info = info_with(true, Vec::new());
        info.consumed_types
            .insert("shop.pay".to_string(), Default::default());
        let (call, faults) = lower_workers(&info, &[]);
        assert!(faults.is_empty(), "{faults:?}");
        let printed = bynk_ts::print_expr(&call);
        assert!(
            printed.starts_with("__callService(deps.env.SHOP_PAY, \"charge\", {}, "),
            "{printed}"
        );
    }

    /// The cast builds `Paren(As(As(..)))`, which prints as one bracketed double cast.
    #[test]
    fn the_cast_is_one_bracketed_double_cast() {
        let cast = cast_through_unknown(id("total"), TsType::named("shop_pay.Money"));
        assert_eq!(
            bynk_ts::print_expr(&cast),
            "(total as unknown as shop_pay.Money)"
        );
    }
}
