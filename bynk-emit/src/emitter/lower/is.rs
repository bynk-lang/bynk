//! `is` and narrowing (back-half paydown 1A.14, 2.2a, #4087): `lhs is Pattern`, the `&&`/`implies`
//! forms that bind its names for the right-hand side, the narrowing temporaries and the
//! refined `is` check. Moved out of `lower.rs` byte for byte.

use std::collections::HashMap;

use bynk_syntax::ast::{
    BaseType, ChainLink, Expr, ExprKind, Pattern, PatternBindingKind, PredKind, Refinement,
    TypeBody,
};

use super::super::lower_ctx::LowerCtx;
use super::super::{payload_prop, ts_ident};
use super::pattern::{
    OrArm, PatPath, and_join, list_rest_value, literal_label_node, member_node_of,
    or_dispatch_stmt, or_pattern_decl, path_test, pattern_tests,
};
use super::{Lowered, Pre, cond_contains_is, lower_expr, member_node};
use crate::ts_build::{bool_lit, const_, ident, str_lit};

/// When we encounter `lhs && rhs`, see if lhs is an `is` (possibly wrapped
/// in parens or nested `&&`) and if so collect the bindings to inject into
/// rhs. Returns `(binding_const_decls, lowered_lhs, lowered_rhs)` if
/// special handling is appropriate; otherwise returns None.
/// What [`lower_and_with_is`] produces for its caller to assemble.
pub(super) struct AndWithIs {
    /// The left operand's `is` bindings, then the right operand's own hoisted
    /// statements: both run, in order, inside the right operand's IIFE.
    pub(super) bindings: Vec<bynk_ts::TsStmt>,
    pub(super) lhs: Lowered,
    pub(super) rhs: bynk_ts::TsExpr,
    /// The right operand hoisted a `?`'s propagating early return.
    pub(super) rhs_returns: bool,
    /// #1654: declarations of the `is` receiver temps the right operand
    /// introduced, for the caller to emit before the whole condition.
    pub(super) hoisted: Vec<bynk_ts::TsStmt>,
}

pub(super) fn lower_and_with_is(lhs: &Expr, rhs: &Expr, cx: &mut LowerCtx) -> Option<AndWithIs> {
    // Probe structurally (no lowering) so a `&&` without an `is` falls through
    // to the caller's ordinary lowering untouched. This mirrors exactly the
    // shapes `gather_is_bindings_for_emit` walks (`&&` and parens), preserving
    // the original "Some iff lhs contains an `is`" behaviour.
    if !cond_contains_is(lhs) {
        return None;
    }
    // Lower lhs *before* gathering its bindings. Lowering lifts any complex
    // `is` receiver (e.g. a call) into a shared temp recorded on `cx`; the
    // binding gatherer below then references that temp via `is_receiver_text`
    // instead of re-emitting the receiver. For simple receivers nothing is
    // cached and the output is byte-identical to before.
    let lhs_lowered = lower_expr(lhs, cx);
    // #908: the gathered bindings are only in scope for `rhs` (the caller
    // wraps them together in one IIFE) — push a frame so a same-named `rhs`
    // read resolves to the binding rather than an outer `let` rename, and pop
    // once `rhs` is lowered so the registration doesn't leak past this `&&`.
    cx.shadow_scopes.push(HashMap::new());
    let mut bindings = Vec::new();
    let mut found = false;
    gather_is_bindings_for_emit(lhs, true, cx, &mut bindings, &mut found);
    let _ = found; // guaranteed true by the `cond_contains_is` guard above
    // #1/#3 review: `rhs`'s own hoisted statements used to be joined into one
    // string and spliced back in as if they were part of the expression text
    // (`bindings.is_empty()` — the common case, not an edge one — took the
    // caller's `format!("{lhs_expr} && {rhs_expr}")` path unedited, which is a
    // syntax error the moment `rhs` hoists anything: a `const` declaration
    // where JS expects an expression). `lower_expr` keeps them as real
    // statements instead, appended after the is-bindings — both must run, in
    // order, before the final `return rhs_expr` the caller wraps them in, so
    // one combined list is exactly what the caller's existing wrap already
    // expects.
    // T2.3 (R6.3): isolate the flag to just `rhs`'s own lowering, so a `?`
    // inside `lhs`'s `is`-receiver (already hoisted unconditionally into the
    // caller's own statement position, not this arrow — see `lower_bin_op`)
    // doesn't falsely report a propagating return coming from `bindings`.
    let saved_early_return = std::mem::take(&mut cx.emitted_early_return);
    // #1654: `rhs` is lowered inside an IIFE (below, in the caller), so an `is`
    // receiver temp it introduces would be scoped away from a then-branch that
    // reads it. Hoist the temps' declarations to the outermost such `&&`; a
    // nested one adds to the same list.
    let outermost = cx.is_temp_hoist.is_none();
    if outermost {
        cx.is_temp_hoist = Some(Vec::new());
    }
    let rhs_lowered = lower_expr(rhs, cx);
    let hoisted = if outermost {
        cx.is_temp_hoist.take().unwrap_or_default()
    } else {
        Vec::new()
    };
    let rhs_returns = cx.emitted_early_return;
    cx.emitted_early_return = saved_early_return || rhs_returns;
    let Lowered {
        pre: rhs_pre,
        expr: rhs_expr,
    } = rhs_lowered;
    bindings.extend(rhs_pre);
    cx.shadow_scopes.pop();
    Some(AndWithIs {
        bindings,
        lhs: lhs_lowered,
        rhs: rhs_expr,
        rhs_returns,
        hoisted,
    })
}

/// #1751/#1752: the receiver to read variant `tag`'s payload fields from: `(recv as Extract<typeof
/// recv, { tag: "V" }>)`. A test is emitted out of line (#2836: every tag-based `is` test is), so
/// TypeScript has not narrowed `recv` there, and the read goes through the variant the checker
/// proved.
fn variant_payload_node(recv: &bynk_ts::TsExpr, tag: &str) -> bynk_ts::TsExpr {
    use bynk_ts::{TsExpr, TsType, TsTypeMember};
    let proven = TsType::Object(vec![TsTypeMember::Prop {
        name: "tag".to_string(),
        ty: TsType::string_literal(tag),
        optional: false,
        readonly: false,
    }]);
    TsExpr::Paren(Box::new(TsExpr::As {
        expr: Box::new(recv.clone()),
        ty: TsType::named_with_args("Extract", vec![TsType::type_of(recv.clone()), proven]),
    }))
}

/// #1752: record the `is` tests in `rhs`, a short-circuit right operand
/// about to be emitted out of line (inside an arrow, or a hoisted `if`), as
/// unable to narrow their receivers anywhere else. A test is recorded under
/// either polarity, since an enclosing condition may gather its bindings
/// through `!`. See [`LowerCtx::unnarrowed_is_tests`].
pub(super) fn mark_unnarrowed(rhs: &Expr, cx: &mut LowerCtx) {
    for when_true in [true, false] {
        for test in bynk_check::narrowing::matched_is_tests(rhs, when_true) {
            cx.unnarrowed_is_tests.insert(test.span);
        }
    }
}

/// Walk an expression collecting `const name = expr.field;` strings for
/// any `is`-pattern bindings on the truthy path. `found` indicates whether
/// at least one `is` was seen.
pub(in crate::emitter) fn gather_is_bindings_for_emit(
    e: &Expr,
    when_true: bool,
    cx: &mut LowerCtx,
    out: &mut Vec<bynk_ts::TsStmt>,
    found: &mut bool,
) {
    // #1654: which `is` tests `e` proves is the shared rule the resolver and
    // the checker scope bindings by (`bynk_check::narrowing`).
    for test in bynk_check::narrowing::matched_is_tests(e, when_true) {
        emit_is_test_bindings(test, cx, out, found);
    }
}

/// The `const name = …;` declarations for one matched `is` test, as statement nodes (a record
/// field, a list element or rest, the refined narrowing cast, a variant payload read through its
/// `Extract` cast, an or-pattern's `let a, b;` and `if`/`else` dispatch).
fn emit_is_test_bindings(
    e: &Expr,
    cx: &mut LowerCtx,
    out: &mut Vec<bynk_ts::TsStmt>,
    found: &mut bool,
) {
    let tys = cx.ty_render();
    let ExprKind::Is { value, pattern } = &e.kind else {
        return;
    };
    {
        *found = true;
        let recv = cx.is_receiver_path(value);
        let disc_ty = cx.commons().expr_types.get(&value.id).map(|te| te.ty);
        if let Pattern::Variant {
            variant, bindings, ..
        } = pattern.as_ref()
        {
            // #1751/#1752: a test emitted out of line (inside a short-circuit
            // arrow) doesn't narrow its receiver here; read through the
            // variant the checker proved instead.
            // #2836: every tag-based `is` test is emitted un-narrowing (see
            // `lower_is`), so its payload is always read through the proven variant.
            // v0.13: refinement narrowing re-binds the value's name to the
            // branded refined type, read from the forced receiver temp.
            if bindings.is_empty()
                && cx.is_refined_is_check(value, &variant.name)
                && let ExprKind::Ident(id) = &value.kind
            {
                let cast = bynk_ts::TsExpr::As {
                    expr: Box::new(recv.node().clone()),
                    ty: bynk_ts::TsType::named(variant.name.clone()),
                };
                out.push(const_(ts_ident(&id.name), cast));
                cx.declare_binder(&id.name);
                return;
            }
            for (i, b) in bindings.iter().enumerate() {
                // `is` emits only flat, depth-1 name bindings (ADR 0169 keeps
                // nesting/guards match-only); `_` and nested patterns bind nothing.
                let Pattern::Binding(name) = b.pattern() else {
                    continue;
                };
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(disc_ty, &variant.name, i, tys.types())
                    }
                };
                // #1653: renamed when it shadows (`o is Some(o)`), so the
                // declaration can't read itself in its temporal dead zone.
                let local = cx.bind_local_name(&name.name);
                let read = bynk_ts::TsExpr::Member {
                    object: Box::new(variant_payload_node(recv.node(), &variant.name)),
                    property: field.clone(),
                };
                out.push(const_(&local, read));
            }
        }
        // #2641: a record pattern's depth-1 names — shorthand fields and
        // `f: name` — read straight off the receiver.
        if let Pattern::Record { fields, .. } = pattern.as_ref() {
            for (fname, sub) in fields {
                let bound = match sub {
                    None => fname,
                    Some(Pattern::Binding(n)) => n,
                    Some(_) => continue,
                };
                let local = cx.bind_local_name(&bound.name);
                out.push(const_(&local, recv.member(&fname.name).node().clone()));
            }
        }
        // #2680: a list pattern's depth-1 names — bare-name elements and a
        // named rest.
        if let Pattern::List { elems, rest, .. } = pattern.as_ref() {
            for (i, sub) in elems.iter().enumerate() {
                if let Pattern::Binding(n) = sub {
                    let local = cx.bind_local_name(&n.name);
                    out.push(const_(&local, recv.index(i).node().clone()));
                }
            }
            if let Some(binder) = rest.as_ref().and_then(|r| r.binder.as_ref()) {
                let local = cx.bind_local_name(&binder.name);
                out.push(const_(&local, list_rest_value(&recv, elems.len())));
            }
        }
        // #474 §2.3.6: an or-pattern's shared names can live at different
        // structural paths per alternative (`Held`'s field 2 vs
        // `Confirmed`'s field 4), so — like the `match` if-chain's
        // `emit_pattern_bindings` — declare each name once with `let`,
        // then dispatch per alternative. Still depth-1 only (the same
        // existing `is` limitation as the `Variant` case above): an
        // alternative that isn't itself a flat `Variant` contributes no
        // bindings, only its tag test.
        if let Pattern::Or(alts, _) = pattern.as_ref() {
            let names = pattern.bound_names();
            if names.is_empty() {
                return;
            }
            let decl: Vec<String> = names
                .iter()
                .map(|id| cx.bind_local_name(&id.name))
                .collect();
            out.push(or_pattern_decl(decl));
            let last = alts.len() - 1;
            let mut arms = Vec::with_capacity(alts.len());
            for (i, alt) in alts.iter().enumerate() {
                let (tag, pairs) = match alt {
                    // #2680: a list alternative is told apart by its length,
                    // so its dispatch condition is its whole structural test.
                    Pattern::List { elems, rest, .. } => {
                        let mut pairs = Vec::new();
                        for (j, sub) in elems.iter().enumerate() {
                            if let Pattern::Binding(n) = sub {
                                let local = cx
                                    .resolved_local_name(&n.name)
                                    .unwrap_or_else(|| ts_ident(&n.name));
                                pairs.push((local, recv.index(j).node().clone()));
                            }
                        }
                        if let Some(binder) = rest.as_ref().and_then(|r| r.binder.as_ref()) {
                            let local = cx
                                .resolved_local_name(&binder.name)
                                .unwrap_or_else(|| ts_ident(&binder.name));
                            pairs.push((local, list_rest_value(&recv, elems.len())));
                        }
                        let mut tests = Vec::new();
                        pattern_tests(&recv, disc_ty, alt, cx, &mut tests);
                        let cond = if tests.is_empty() {
                            bool_lit(true)
                        } else {
                            and_join(tests)
                        };
                        (Some(cond), pairs)
                    }
                    Pattern::Variant {
                        variant, bindings, ..
                    } => {
                        let mut pairs = Vec::new();
                        for (j, b) in bindings.iter().enumerate() {
                            let Pattern::Binding(name) = b.pattern() else {
                                continue;
                            };
                            let field = match &b.kind {
                                PatternBindingKind::Named { field, .. } => {
                                    payload_prop(&field.name)
                                }
                                PatternBindingKind::Positional { .. } => {
                                    cx.positional_field_name(disc_ty, &variant.name, j, tys.types())
                                }
                            };
                            let local = cx
                                .resolved_local_name(&name.name)
                                .unwrap_or_else(|| ts_ident(&name.name));
                            // #1765 review: out of line, the dispatch below
                            // starts from the whole union, so its last arm (a
                            // bare `else`) isn't narrowed to this variant.
                            let read = bynk_ts::TsExpr::Member {
                                object: Box::new(variant_payload_node(recv.node(), &variant.name)),
                                property: field.clone(),
                            };
                            pairs.push((local, read));
                        }
                        let tag_test = bynk_ts::TsExpr::Binary {
                            op: bynk_ts::TsBinaryOp::StrictEq,
                            left: Box::new(member_node_of(recv.node(), "tag")),
                            right: Box::new(str_lit(variant.name.clone())),
                        };
                        (Some(tag_test), pairs)
                    }
                    // #2641: a record alternative has no tag to test; its
                    // depth-1 names read straight off the receiver.
                    Pattern::Record { fields, .. } => {
                        let mut pairs = Vec::new();
                        for (fname, sub) in fields {
                            let bound = match sub {
                                None => fname,
                                Some(Pattern::Binding(n)) => n,
                                Some(_) => continue,
                            };
                            let local = cx
                                .resolved_local_name(&bound.name)
                                .unwrap_or_else(|| ts_ident(&bound.name));
                            pairs.push((local, recv.member(&fname.name).node().clone()));
                        }
                        (None, pairs)
                    }
                    Pattern::Wildcard(..)
                    | Pattern::Binding(..)
                    | Pattern::Literal { .. }
                    | Pattern::Range(..)
                    | Pattern::Refined { .. }
                    | Pattern::Or(..) => (None, Vec::new()),
                };
                let cond = if i == last {
                    None
                } else {
                    Some(tag.unwrap_or_else(|| bool_lit(true)))
                };
                arms.push(OrArm {
                    cond,
                    assigns: pairs,
                });
            }
            out.push(or_dispatch_stmt(arms));
        }
    }
}

/// True when an `is` receiver is a simple, side-effect-free, repeatable
/// lvalue — an identifier or a field-access chain ending at one (optionally
/// parenthesised). Such receivers can be referenced textually as many times
/// as there are pattern bindings without re-evaluation. Anything else (calls,
/// matches, arithmetic, …) must be lifted to a temp before use; see
/// `LowerCtx::is_receiver_ref`.
pub(crate) fn is_simple_is_receiver(value: &Expr) -> bool {
    match value.kind.chain_link() {
        Some(ChainLink::Root(_)) => true,
        Some(ChainLink::Field { receiver, .. } | ChainLink::Paren(receiver)) => {
            is_simple_is_receiver(receiver)
        }
        None => false,
    }
}

/// The path of a simple `is` receiver: a local, or a field chain over one. A complex receiver
/// never reaches here: it was lifted to a temp when its condition was lowered, which every
/// gatherer of `is` bindings does first (`lower_and_with_is`, the value-position `if`,
/// `emit_if_tail`), so `LowerCtx::is_receiver_path` finds the temp (#1668). Reaching the last
/// arm means that ordering broke; it used to emit a placeholder into the output, invalid
/// TypeScript that surfaced as a distant `tsc` error, and now fails here, like the emitter's other
/// broken invariants.
pub(in crate::emitter) fn path_for_is(value: &Expr) -> PatPath {
    match value.kind.chain_link() {
        Some(ChainLink::Root(id)) => PatPath::ident(&ts_ident(&id.name)),
        Some(ChainLink::Field { receiver, field }) => path_for_is(receiver).member(&field.name),
        Some(ChainLink::Paren(inner)) => path_for_is(inner),
        None => panic!(
            "bynk internal error (#1668): the `is` receiver at {:?} is not a simple \
             lvalue and was not lifted to a temp before its bindings were gathered \
             — the condition must be lowered first",
            value.span
        ),
    }
}

pub(super) fn lower_is(value: &Expr, pattern: &Pattern, cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    // v0.13: refinement check — `value is RefinedType` lowers to the refined
    // type's predicates as a boolean expression. The receiver is forced to a
    // temp so the narrowing binding (`const n = <temp> as Quantity`) can shadow
    // the value's name without a TDZ.
    if let Pattern::Variant {
        variant, bindings, ..
    } = pattern
        && bindings.is_empty()
        && cx.is_refined_is_check(value, &variant.name)
        && let Some(TypeBody::Refined {
            base, refinement, ..
        }) = cx
            .commons()
            .types
            .get(&variant.name)
            .map(|d| d.body.clone())
    {
        let recv = pre.absorb_node(cx.is_receiver_ref_forced(value));
        return pre.finish(refined_check_as_bool(recv, base, refinement.as_ref()));
    }
    let path = PatPath::from_lowered(pre.absorb_node(cx.is_receiver_ref(value)));
    let scrut_ty = cx.commons().expr_types.get(&value.id).map(|te| te.ty);
    let tested: bynk_ts::TsExpr = match pattern {
        // A bare name binding after `is` matches anything over a sum (ADR 0169).
        Pattern::Wildcard(_) | Pattern::Binding(_) => bool_lit(true),
        // v0.130 (DECISION F): the checker rejects a literal on the RHS of `is`,
        // so this is unreachable for a well-typed program; lower it to the
        // value-equality it would mean, defensively.
        Pattern::Literal { value, .. } => path_test(
            &path,
            bynk_ts::TsBinaryOp::StrictEq,
            literal_label_node(value),
        ),
        // #472: the checker rejects a refined pattern on the RHS of `is`
        // (D5), so this is unreachable for a well-typed program; lower it
        // defensively via the same `pattern_tests` a `match` arm uses (which
        // already handles `Pattern::Refined` — inner tests plus the predicate
        // via `refined_check_as_bool`).
        //
        // #705: a variant pattern lowers to its full structural test — the outer
        // tag *and* any nested refutable payload patterns, so
        // `r is Rejected(RefinementViolation(_))` checks both levels (the same
        // tests a `match` arm emits). A plain `is Ok(_)` / `is Rejected(_)` still
        // yields a single `.tag` test (the `_` payload is irrefutable). For a
        // loose scrutinee — the `Wire`-call outcome — the single-field `"value"`
        // fallback matches the runtime `{ tag, value }` shape, so the nested test
        // lands without a static type. A multi-payload nested pattern on a loose
        // scrutinee is unreachable today (the only loose variant sums are
        // single-field) and shared with `match`'s lowering; precise for any
        // typed scrutinee.
        //
        // #2641: a record pattern is its fields' tests, `true` when every field
        // is irrefutable. #474 §2.3.6: an or-pattern is legal after `is`;
        // `pattern_tests` ORs its alternatives into one combined term.
        Pattern::Refined { .. }
        | Pattern::Record { .. }
        | Pattern::List { .. }
        | Pattern::Range(_)
        | Pattern::Variant { .. }
        | Pattern::Or(..) => {
            let mut tests = Vec::new();
            pattern_tests(&path, scrut_ty, pattern, cx, &mut tests);
            // `pattern_tests` always pushes the outer `.tag` test for a variant
            // pattern, so a variant `is` is never vacuously `true` — a
            // silently-passing `expect` is exactly the failure this reuse removes.
            debug_assert!(
                !matches!(pattern, Pattern::Variant { .. }) || !tests.is_empty(),
                "a variant `is` pattern must emit at least its outer tag test"
            );
            if tests.is_empty() {
                bool_lit(true)
            } else {
                and_join(tests)
            }
        }
    };
    // #2836: a bare `r.tag === "Ok"` condition makes tsc narrow `r` for the code
    // it guards, so a later exhaustive `match r` (after `expect r is Ok(_)`, or in
    // the `else` of `if r is Err(_)`) has an impossible `case "Err":` (TS2678).
    // Passing the boolean through an identity arrow keeps the test and hands tsc
    // nothing to narrow by; the bindings read the payload through the proven
    // variant instead (`variant_payload`).
    let is_true = matches!(&tested, bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true)));
    if matches!(pattern, Pattern::Variant { .. } | Pattern::Or(..)) && !is_true {
        return pre.finish(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Arrow {
                params: vec![bynk_ts::TsParam::new(
                    "__c".to_string(),
                    Some(bynk_ts::TsType::named("boolean")),
                )],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(ident("__c")))),
            }))),
            args: vec![tested],
        });
    }
    pre.finish(tested)
}

/// v0.13: render a refined type's predicates as a single boolean expression over
/// `recv`, for `value is RefinedType`. Mirrors `emit_pred_check`'s per-predicate
/// logic but as `&&`-joined terms instead of `Result`-returning statements.
pub(super) fn refined_check_as_bool(
    recv: bynk_ts::TsExpr,
    base: BaseType,
    refinement: Option<&Refinement>,
) -> bynk_ts::TsExpr {
    use bynk_ts::{TsBinaryOp, TsExpr};
    let number_check = |name: &str| TsExpr::Call {
        callee: Box::new(TsExpr::Member {
            object: Box::new(member_node("globalThis", "Number")),
            property: name.to_string(),
        }),
        args: vec![recv.clone()],
    };
    let mut terms: Vec<TsExpr> = Vec::new();
    if base == BaseType::Int {
        // #1657: an `Int` is a JS safe integer.
        terms.push(number_check("isSafeInteger"));
    }
    // v0.21: validated `Float` values are finite (ADR 0040).
    if base == BaseType::Float {
        terms.push(number_check("isFinite"));
    }
    if let Some(r) = refinement {
        for p in bynk_syntax::ast::in_check_order(&r.predicates, |p| &p.kind) {
            let (cond, _) = bynk_ts::pred_condition_and_message_over(&p.kind, &recv, Some(base));
            // A range is one bracketed term; every other predicate stands bare.
            terms.push(match &p.kind {
                PredKind::InRange(..) | PredKind::InRangeF(..) | PredKind::InRangeDur(..)
                    if base != BaseType::Decimal =>
                {
                    TsExpr::Paren(Box::new(cond))
                }
                PredKind::Matches(..)
                | PredKind::InRange(..)
                | PredKind::InRangeF(..)
                | PredKind::InRangeD(..)
                | PredKind::InRangeDur(..)
                | PredKind::AtLeast(_)
                | PredKind::AtMost(_)
                | PredKind::MinLength(..)
                | PredKind::MaxLength(..)
                | PredKind::Length(..)
                | PredKind::NonNegative
                | PredKind::Positive
                | PredKind::NonEmpty
                | PredKind::IsoDate
                | PredKind::Cron => cond,
            });
        }
    }
    let mut terms = terms.into_iter();
    let Some(first) = terms.next() else {
        return bool_lit(true);
    };
    let joined = terms.fold(first, |left, right| TsExpr::Binary {
        op: TsBinaryOp::And,
        left: Box::new(left),
        right: Box::new(right),
    });
    TsExpr::Paren(Box::new(joined))
}

#[cfg(test)]
mod path_for_is_tests {
    use super::*;
    use bynk_syntax::ast::ExprId;
    use bynk_syntax::ast::Ident;
    use bynk_syntax::span::Span;

    fn expr(kind: ExprKind) -> Expr {
        Expr {
            id: ExprId::SYNTHETIC,
            kind,
            span: Span::new(0, 0),
        }
    }

    fn ident(name: &str) -> Ident {
        Ident {
            name: name.to_string(),
            span: Span::new(0, 0),
        }
    }

    #[test]
    fn a_simple_receiver_is_its_lvalue_path() {
        let field = expr(ExprKind::FieldAccess {
            receiver: Box::new(expr(ExprKind::Paren(Box::new(expr(ExprKind::Ident(
                ident("b"),
            )))))),
            field: ident("room"),
        });
        assert_eq!(bynk_ts::print_expr(path_for_is(&field).node()), "b.room");
    }

    // #1668 review: a complex receiver reaching here means it was not lifted
    // to a temp before its bindings were gathered. It must fail loudly, not
    // fall back to emitted text, which is what the arm used to do.
    #[test]
    #[should_panic(expected = "bynk internal error (#1668)")]
    fn a_complex_receiver_panics_instead_of_emitting_a_placeholder() {
        path_for_is(&expr(ExprKind::IntLit {
            value: 1,
            lexeme: "1".to_string(),
        }));
    }
}

#[cfg(test)]
mod leaf_node_tests {
    use super::*;
    use bynk_ts::{print_expr, print_stmt};

    /// `(recv as Extract<typeof recv, { tag: "V" }>)`: the variant a test already proved.
    #[test]
    fn a_variant_payload_cast_is_the_extract_of_the_receiver() {
        assert_eq!(
            print_expr(&variant_payload_node(&ident("s"), "Circle")),
            "(s as Extract<typeof s, { tag: \"Circle\" }>)"
        );
        assert_eq!(
            print_expr(&variant_payload_node(&member_node("a", "b"), "V")),
            "(a.b as Extract<typeof a.b, { tag: \"V\" }>)"
        );
    }

    /// The base-type terms of a refined check, over a node receiver.
    #[test]
    fn a_refined_check_is_the_base_terms_over_the_receiver() {
        let check = |base| print_expr(&refined_check_as_bool(ident("n"), base, None));
        assert_eq!(check(BaseType::Int), "(globalThis.Number.isSafeInteger(n))");
        assert_eq!(check(BaseType::Float), "(globalThis.Number.isFinite(n))");
        assert_eq!(check(BaseType::String), "true");
    }

    /// An or-pattern's dispatch is an `if`/`else if`/`else` chain assigning each shared name.
    #[test]
    fn an_or_dispatch_is_an_if_chain_over_nodes() {
        let arm = |cond: Option<bynk_ts::TsExpr>, to: &str| OrArm {
            cond,
            assigns: vec![("r".to_string(), member_node("s", to))],
        };
        let chain = or_dispatch_stmt(vec![
            arm(Some(ident("a")), "x"),
            arm(Some(ident("b")), "y"),
            arm(None, "z"),
        ]);
        assert_eq!(
            print_stmt(&chain, 0),
            "if (a) {\n  r = s.x;\n} else if (b) {\n  r = s.y;\n} else {\n  r = s.z;\n}\n"
        );
    }
}
