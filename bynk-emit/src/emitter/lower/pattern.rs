//! Patterns (back-half paydown 1A.13, 2.2a, #4087): the tests and bindings a `match` arm, an
//! `is` pattern or an event subscription's pattern binds, with or-pattern dispatch and the
//! event-pattern guard. Moved out of `lower.rs` byte for byte.

use std::sync::Arc;

use bynk_check::checker::{NamedKind, Ty, TyId, Types};
use bynk_model::{ConstVal, EventPatternIr, EventPatternValueIr};
use bynk_syntax::ast::{BaseType, LiteralValue, Pattern, PatternBindingKind, TypeBody};

use super::super::lower_ctx::LowerCtx;
use super::super::lower_stmt::{Body, Item};
use super::super::{payload_prop, ts_ident};
use super::is::refined_check_as_bool;
use crate::ts_build::{bool_lit, ident, num_lit};

/// A literal pattern's value as a node (1A.13, Decision C of #3714): the twin of
/// [`literal_case_label`], which prints byte for byte the same (`TsLit::Str`
/// shares `escape_ts_string`'s escaper, `TsLit::Num` is verbatim).
pub(in crate::emitter) fn literal_label_node(value: &LiteralValue) -> bynk_ts::TsExpr {
    use bynk_ts::{TsExpr, TsLit};
    match value {
        LiteralValue::Int(n) => num_lit(n.to_string()),
        LiteralValue::Str(s) => TsExpr::Lit(TsLit::Str(s.clone())),
        LiteralValue::Bool(b) => TsExpr::Lit(TsLit::Bool(*b)),
    }
}

/// The runtime path a pattern is tested against (`__d`, `x.f`, `x[0].g`), as the node every test
/// and binding built over it reads (1A.13, Decision C of #3714; text-free since 1A.35a part C).
#[derive(Clone)]
pub(in crate::emitter) struct PatPath {
    node: bynk_ts::TsExpr,
}

impl PatPath {
    /// A bare local.
    pub(in crate::emitter) fn ident(name: &str) -> Self {
        PatPath { node: ident(name) }
    }

    /// The path a lowered scrutinee is read at.
    pub(in crate::emitter) fn from_lowered(value: bynk_ts::TsExpr) -> Self {
        PatPath { node: value }
    }

    pub(in crate::emitter) fn node(&self) -> &bynk_ts::TsExpr {
        &self.node
    }

    /// `path.property`.
    pub(in crate::emitter) fn member(&self, property: &str) -> Self {
        PatPath {
            node: member_node_of(&self.node, property),
        }
    }

    /// `path[i]`.
    pub(in crate::emitter) fn index(&self, i: usize) -> Self {
        PatPath {
            node: bynk_ts::TsExpr::Index {
                object: Box::new(self.node.clone()),
                index: Box::new(num_lit(i.to_string())),
            },
        }
    }
}

/// `path <op> rhs`.
pub(super) fn path_test(
    path: &PatPath,
    op: bynk_ts::TsBinaryOp,
    rhs: bynk_ts::TsExpr,
) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Binary {
        op,
        left: Box::new(path.node.clone()),
        right: Box::new(rhs),
    }
}

/// `a && b && …`: a left-leaning `And` chain. `tests` is not empty.
pub(in crate::emitter) fn and_join(tests: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    tests
        .into_iter()
        .reduce(|left, right| bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::And,
            left: Box::new(left),
            right: Box::new(right),
        })
        .unwrap_or_else(|| unreachable!("and_join is given at least one test"))
}

/// #472: the `Int`/`String`/`Bool` base of a literal-kind type, or `None` for
/// anything else. Mirrors the checker's `literal_base_of` (and the
/// per-variant logic in [`is_literal_match`]) — used to feed
/// [`refined_check_as_bool`] the base a `_ where predicate` pattern's
/// scrutinee actually has.
fn literal_base_of_ty(ty: TyId, tys: &Arc<Types>) -> Option<BaseType> {
    let base = tys.get(ty).base()?;
    matches!(base, BaseType::Int | BaseType::String | BaseType::Bool).then_some(base)
}

/// #2641: the type of field `field` of the record at static type `path_ty`, at
/// that type's arguments. `None` when `path_ty` is not a known record (a loose
/// scrutinee), so a nested multi-field variant under it falls back as it
/// does for any untyped path.
fn record_field_ty(path_ty: Option<TyId>, field: &str, cx: &LowerCtx) -> Option<TyId> {
    let tys = cx.ty_render();
    let ty = path_ty?;
    let Ty::Named {
        name,
        kind: NamedKind::Record,
        args,
    } = &*tys.get(ty)
    else {
        return None;
    };
    let types = &cx.commons().types;
    let decl = types.get(name)?;
    let TypeBody::Record(r) = &decl.body else {
        return None;
    };
    let f = r.fields.iter().find(|f| f.name.name == field)?;
    bynk_check::checker::instantiate_field_ty(decl, args, &f.type_ref, types, tys.types())
}

/// #2680: the element type of the list at static type `path_ty`.
fn list_elem_ty(path_ty: Option<TyId>, cx: &LowerCtx) -> Option<TyId> {
    cx.commons().tys().get(path_ty?).as_list()
}

/// Emit the boolean tests that must hold for `pattern` to match the value at
/// runtime `path` (static type `path_ty`), each a node (1A.13, Decision C of #3714). Irrefutable
/// patterns add nothing; nested variant/literal payloads recurse into
/// `path.<field>`.
pub(in crate::emitter) fn pattern_tests(
    path: &PatPath,
    path_ty: Option<TyId>,
    pattern: &Pattern,
    cx: &LowerCtx,
    tests: &mut Vec<bynk_ts::TsExpr>,
) {
    use bynk_ts::{TsBinaryOp, TsExpr, TsLit};
    let tys = cx.ty_render();
    match pattern {
        Pattern::Wildcard(_) | Pattern::Binding(_) => {}
        Pattern::As { inner, .. } => pattern_tests(path, path_ty, inner, cx, tests),
        Pattern::Literal { value, .. } => {
            tests.push(path_test(
                path,
                TsBinaryOp::StrictEq,
                literal_label_node(value),
            ));
        }
        // #2699: `x >= lo && x <= hi`, or the one side an open range has.
        Pattern::Range(r) => {
            if let Some(lo) = r.lo {
                tests.push(path_test(
                    path,
                    TsBinaryOp::GreaterThanEq,
                    num_lit(lo.to_string()),
                ));
            }
            if let Some(hi) = r.hi {
                tests.push(path_test(
                    path,
                    TsBinaryOp::LessThanEq,
                    num_lit(hi.to_string()),
                ));
            }
        }
        // #472: `p where predicate` — the inner pattern's own tests, then the
        // predicate as a boolean guard over the runtime value at `path`,
        // reusing `refined_check_as_bool` (the same helper `value is
        // RefinedType` lowers through).
        Pattern::Refined {
            inner, predicate, ..
        } => {
            pattern_tests(path, path_ty, inner, cx, tests);
            // The checker (`check_pattern`) already rejected any scrutinee
            // that isn't literal-kind before this point, so `path_ty` is
            // always `Int` or `String` here for a well-typed program — a
            // silent fallback would mask a real bug (wrong predicate codegen
            // against the wrong base) rather than surfacing it loudly, the
            // same posture as `match_case`'s `Pattern::Refined` arm.
            let base = path_ty
                .and_then(|t| literal_base_of_ty(t, tys.types()))
                .unwrap_or_else(|| {
                    panic!(
                        "a refined pattern's scrutinee must be literal-kind (Int/String), got {:?}",
                        path_ty
                    )
                });
            tests.push(refined_check_as_bool(
                path.node().clone(),
                base,
                Some(predicate),
            ));
        }
        // #2680: a length test (none for `[..]`, which matches every list),
        // then each refutable element against `path[i]`.
        Pattern::List { elems, rest, .. } => {
            let length = path.member("length");
            let length_test =
                |op: TsBinaryOp, n: usize| path_test(&length, op, num_lit(n.to_string()));
            match (rest.is_some(), elems.len()) {
                (true, 0) => {}
                (true, n) => tests.push(length_test(TsBinaryOp::GreaterThanEq, n)),
                (false, n) => tests.push(length_test(TsBinaryOp::StrictEq, n)),
            }
            let elem_ty = list_elem_ty(path_ty, cx);
            for (i, sub) in elems.iter().enumerate() {
                if sub.is_irrefutable() {
                    continue;
                }
                pattern_tests(&path.index(i), elem_ty, sub, cx, tests);
            }
        }
        // #2641: test each refutable field sub-pattern against `path.field`;
        // there is no tag test, since the checker proved the scrutinee is this
        // record.
        Pattern::Record { fields, .. } => {
            for (name, sub) in fields {
                let Some(sub) = sub else { continue };
                if sub.is_irrefutable() {
                    continue;
                }
                let field_ty = record_field_ty(path_ty, &name.name, cx);
                pattern_tests(&path.member(&name.name), field_ty, sub, cx, tests);
            }
        }
        Pattern::Variant {
            variant, bindings, ..
        } => {
            tests.push(path_test(
                &path.member("tag"),
                TsBinaryOp::StrictEq,
                TsExpr::Lit(TsLit::Str(variant.name.clone())),
            ));
            for (i, b) in bindings.iter().enumerate() {
                let sp = b.pattern();
                if sp.is_irrefutable() {
                    continue;
                }
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(path_ty, &variant.name, i, tys.types())
                    }
                };
                let field_ty = cx.payload_field_ty(path_ty, &variant.name, i, tys.types());
                pattern_tests(&path.member(&field), field_ty, sp, cx, tests);
            }
        }
        // #474: an or-pattern matches `path` when any alternative does — AND
        // each alternative's own tests, then OR the alternatives together, and
        // push the whole disjunction as a single combined test (so a caller
        // AND-joining `tests` composes it correctly with sibling tests, e.g. a
        // nested `Some(1 | 2)`'s outer `.tag === "Some"` test).
        Pattern::Or(alts, _) => {
            let terms: Vec<TsExpr> = alts
                .iter()
                .map(|alt| {
                    let mut t = Vec::new();
                    pattern_tests(path, path_ty, alt, cx, &mut t);
                    if t.is_empty() {
                        TsExpr::Lit(TsLit::Bool(true))
                    } else {
                        and_join(t)
                    }
                })
                .collect();
            tests.push(TsExpr::Paren(Box::new(
                terms
                    .into_iter()
                    .reduce(|left, right| TsExpr::Binary {
                        op: TsBinaryOp::Or,
                        left: Box::new(left),
                        right: Box::new(right),
                    })
                    .unwrap_or_else(|| unreachable!("an or-pattern has alternatives")),
            )));
        }
    }
}

/// #1187's slice 5 (the `Service` emitter cutover): the JS boolean guard for
/// a `from Events(E { field: value, .. })` subscription filter, AND-joining
/// one test per listed field — reads [`bynk_model::EventPatternIr`] (already
/// resolved by [`bynk_model::lower_protocol_ir`]) rather than the raw
/// AST `EventPattern`; [`EventPatternValueIr`]'s own doc comment already
/// named this function as its sole intended consumer, never wired up until
/// now (the AST-driven original this replaces produced byte-identical guard
/// text: `EventPatternValueIr::Const` only ever holds the same closed
/// `Int`/`Str`/`Bool` set `literal_case_label` covers, and `Variant { tag }`
/// already carries exactly the bare, unqualified tag the AST version's
/// `variant.name` destructured down to). No `Ty`/`LowerCtx` needed:
/// `check_event_pattern` (`bynk-check/src/project_model.rs`) has already
/// proven every field exists and every value type-checks, so this is total
/// and cannot panic. Returns `None` for a pattern-less subscription (nothing
/// to guard).
pub(crate) fn event_pattern_guard_ir(
    path: &str,
    pattern: Option<&EventPatternIr>,
) -> Option<bynk_ts::TsExpr> {
    use crate::ts_build::{and_expr, bool_lit, ident, member, num_lit, str_lit, strict_eq};
    let pattern = pattern?;
    pattern
        .fields
        .iter()
        .map(|(name, value)| {
            let field = member(ident(path), name.as_str());
            match value {
                EventPatternValueIr::Const(ConstVal::Int(n)) => strict_eq(field, num_lit(n)),
                EventPatternValueIr::Const(ConstVal::Str(s)) => {
                    strict_eq(field, str_lit(s.as_str()))
                }
                EventPatternValueIr::Const(ConstVal::Bool(b)) => strict_eq(field, bool_lit(*b)),
                EventPatternValueIr::Const(
                    ConstVal::Float(_) | ConstVal::DurationMillis(_) | ConstVal::Unit,
                ) => {
                    unreachable!(
                        "EventPatternValueIr::Const only ever holds Int/Str/Bool (its own doc \
                         comment) — the checker's own check_event_pattern already restricted a \
                         from Events(...) filter to that closed set before this pattern could exist"
                    )
                }
                EventPatternValueIr::Variant { tag } => {
                    strict_eq(member(field, "tag"), str_lit(tag.as_str()))
                }
            }
        })
        .reduce(and_expr)
}

/// Emit `const` declarations binding the names in `pattern` from runtime `path`,
/// recursing through nested payloads (ADR 0169). Each is a `Const` node when
/// the path is one (1A.13, Decision C of #3714), else its text line; an
/// or-pattern's `let` and `if`/`else` dispatch stays text lines.
pub(in crate::emitter) fn emit_pattern_bindings(
    body: &mut Body,
    path: &PatPath,
    path_ty: Option<TyId>,
    pattern: &Pattern,
    cx: &mut LowerCtx,
) {
    let tys = cx.ty_render();
    match pattern {
        Pattern::Wildcard(_) | Pattern::Literal { .. } | Pattern::Range(_) => {}
        Pattern::Binding(id) => {
            let local = cx.bind_local_name(&id.name);
            body.push(const_binding(&local, path.node().clone()));
        }
        // #3971: `p as name` binds the value at `path`, then whatever `p`
        // binds inside it.
        Pattern::As { inner, name, .. } => {
            let local = cx.bind_local_name(&name.name);
            body.push(const_binding(&local, path.node().clone()));
            emit_pattern_bindings(body, path, path_ty, inner, cx);
        }
        // #472: a refined pattern binds only what its inner pattern binds
        // (`_ where P` binds nothing today).
        Pattern::Refined { inner, .. } => {
            emit_pattern_bindings(body, path, path_ty, inner, cx);
        }
        // #2680: each element binds from `path[i]`; a named rest from the
        // remaining elements.
        Pattern::List { elems, rest, .. } => {
            let elem_ty = list_elem_ty(path_ty, cx);
            for (i, sub) in elems.iter().enumerate() {
                emit_pattern_bindings(body, &path.index(i), elem_ty, sub, cx);
            }
            if let Some(binder) = rest.as_ref().and_then(|r| r.binder.as_ref()) {
                let local = cx.bind_local_name(&binder.name);
                let value = list_rest_value(path, elems.len());
                body.push(const_binding(&local, value));
            }
        }
        // #2641: a shorthand field binds a local of its own name; `f: p`
        // recurses into `path.f`.
        Pattern::Record { fields, .. } => {
            for (name, sub) in fields {
                let field_path = path.member(&name.name);
                match sub {
                    Some(sub) => {
                        let field_ty = record_field_ty(path_ty, &name.name, cx);
                        emit_pattern_bindings(body, &field_path, field_ty, sub, cx);
                    }
                    None => {
                        let local = cx.bind_local_name(&name.name);
                        body.push(const_binding(&local, field_path.node().clone()));
                    }
                }
            }
        }
        Pattern::Variant {
            variant, bindings, ..
        } => {
            for (i, b) in bindings.iter().enumerate() {
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(path_ty, &variant.name, i, tys.types())
                    }
                };
                let field_ty = cx.payload_field_ty(path_ty, &variant.name, i, tys.types());
                emit_pattern_bindings(body, &path.member(&field), field_ty, b.pattern(), cx);
            }
        }
        // #474: different alternatives can bind the same name at different
        // structural paths (`Held`'s field 2 vs `Confirmed`'s field 4), so a
        // single `const` can't express it. Declare each shared name once with
        // `let`, then dispatch per alternative — only the (small) binding
        // resolution is duplicated per alternative, not the arm body that
        // follows. Alternatives are mutually exclusive at runtime (a value has
        // exactly one tag, or a literal one value), so a plain if/else-if/else
        // chain is exhaustive over them once the caller's own combined test
        // (built by `pattern_tests`) has already passed.
        Pattern::Or(alts, _) => {
            let names = pattern.bound_names();
            if names.is_empty() {
                return;
            }
            let decl: Vec<String> = names
                .iter()
                .map(|id| cx.bind_local_name(&id.name))
                .collect();
            body.push(Item::Node(Box::new(or_pattern_decl(decl))));
            let last = alts.len() - 1;
            let mut arms = Vec::with_capacity(alts.len());
            for (i, alt) in alts.iter().enumerate() {
                let cond = if i == last {
                    None
                } else {
                    let mut t = Vec::new();
                    pattern_tests(path, path_ty, alt, cx, &mut t);
                    Some(if t.is_empty() {
                        bool_lit(true)
                    } else {
                        and_join(t)
                    })
                };
                let mut pairs = Vec::new();
                pattern_binding_paths(path, path_ty, alt, cx, &mut pairs);
                let assigns = pairs
                    .into_iter()
                    .map(|(name, value)| {
                        let local = cx
                            .resolved_local_name(&name)
                            .unwrap_or_else(|| ts_ident(&name));
                        (local, value)
                    })
                    .collect();
                arms.push(OrArm { cond, assigns });
            }
            body.push(Item::Node(Box::new(or_dispatch_stmt(arms))));
        }
    }
}

/// `let a, b;`: an or-pattern's shared names, declared once before its dispatch assigns them.
pub(super) fn or_pattern_decl(names: Vec<String>) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::let_list(
        names.into_iter().map(bynk_ts::TsDeclarator::new).collect(),
        None,
    )
}

/// One alternative of an or-pattern's dispatch: its test (none for the last, the bare
/// `else`) and the shared names it assigns, each from the path the alternative binds it at.
pub(super) struct OrArm {
    pub(super) cond: Option<bynk_ts::TsExpr>,
    pub(super) assigns: Vec<(String, bynk_ts::TsExpr)>,
}

/// `recv.property` over a node receiver.
pub(super) fn member_node_of(recv: &bynk_ts::TsExpr, property: &str) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Member {
        object: Box::new(recv.clone()),
        property: property.to_string(),
    }
}

/// `if (c0) { a = p; } else if (c1) { … } else { … }`.
pub(super) fn or_dispatch_stmt(arms: Vec<OrArm>) -> bynk_ts::TsStmt {
    use bynk_ts::{TsExpr, TsStmt};
    let block = |assigns: Vec<(String, TsExpr)>| -> TsStmt {
        TsStmt::block(
            assigns
                .into_iter()
                .map(|(local, value)| TsStmt::assign(TsExpr::Ident(local), value, None))
                .collect(),
            None,
        )
    };
    let mut arms = arms.into_iter().rev();
    let last = arms.next().expect("an or-pattern has alternatives");
    let mut chain = block(last.assigns);
    for arm in arms {
        let cond = arm.cond.expect("every arm but the last has a test");
        chain = TsStmt::if_else_same_line_stmt(cond, block(arm.assigns), chain, None);
    }
    chain
}

/// `const <local> = <value>;` as an item (see `lower_stmt::const_item`).
fn const_binding(local: &str, value: bynk_ts::TsExpr) -> Item {
    super::lower_stmt::const_node_item(local, None, value, false)
}

/// The value a list pattern's `..rest` binds: the list itself, or `path.slice(n)`. `[..rest]`
/// binds the list itself: lists are never mutated in place, so sharing it is unobservable. Any
/// other rest copies the remaining elements, O(length - n) (#2680).
pub(super) fn list_rest_value(path: &PatPath, n: usize) -> bynk_ts::TsExpr {
    if n == 0 {
        return path.node().clone();
    }
    bynk_ts::TsExpr::Call {
        callee: Box::new(member_node_of(path.node(), "slice")),
        args: vec![num_lit(n.to_string())],
    }
}

/// Collect `(name, runtime-path)` pairs for the names `pattern` binds from
/// `path`, recursing through nested payloads — the same traversal as
/// `emit_pattern_bindings`, but returning the resolved paths instead of
/// writing `const` declarations, so an or-pattern's per-alternative dispatch
/// (above) can assign a shared `let` from whichever alternative matched.
fn pattern_binding_paths(
    path: &PatPath,
    path_ty: Option<TyId>,
    pattern: &Pattern,
    cx: &LowerCtx,
    out: &mut Vec<(String, bynk_ts::TsExpr)>,
) {
    let tys = cx.ty_render();
    match pattern {
        Pattern::Wildcard(_) | Pattern::Literal { .. } | Pattern::Range(_) => {}
        Pattern::Binding(id) => out.push((id.name.clone(), path.node().clone())),
        Pattern::As { inner, name, .. } => {
            out.push((name.name.clone(), path.node().clone()));
            pattern_binding_paths(path, path_ty, inner, cx, out);
        }
        Pattern::Variant {
            variant, bindings, ..
        } => {
            for (i, b) in bindings.iter().enumerate() {
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(path_ty, &variant.name, i, tys.types())
                    }
                };
                let field_ty = cx.payload_field_ty(path_ty, &variant.name, i, tys.types());
                pattern_binding_paths(&path.member(&field), field_ty, b.pattern(), cx, out);
            }
        }
        // Never reached from the parser (an `Or`'s alternatives are always
        // leaves), but recurse into the first alternative defensively rather
        // than panic, mirroring `gather_pattern_bindings`'s checker-side choice.
        Pattern::Or(alts, _) => {
            if let Some(first) = alts.first() {
                pattern_binding_paths(path, path_ty, first, cx, out);
            }
        }
        // #472: binds only what its inner pattern binds (`_ where P` binds
        // nothing today) — same recursion as `emit_pattern_bindings`'s
        // `Refined` arm.
        Pattern::Refined { inner, .. } => pattern_binding_paths(path, path_ty, inner, cx, out),
        Pattern::List { elems, rest, .. } => {
            let elem_ty = list_elem_ty(path_ty, cx);
            for (i, sub) in elems.iter().enumerate() {
                pattern_binding_paths(&path.index(i), elem_ty, sub, cx, out);
            }
            if let Some(binder) = rest.as_ref().and_then(|r| r.binder.as_ref()) {
                out.push((binder.name.clone(), list_rest_value(path, elems.len())));
            }
        }
        Pattern::Record { fields, .. } => {
            for (name, sub) in fields {
                let field_path = path.member(&name.name);
                match sub {
                    Some(sub) => {
                        let field_ty = record_field_ty(path_ty, &name.name, cx);
                        pattern_binding_paths(&field_path, field_ty, sub, cx, out);
                    }
                    None => out.push((name.name.clone(), field_path.node().clone())),
                }
            }
        }
    }
}

#[cfg(test)]
mod event_pattern_guard_ir_tests {
    use super::*;

    /// Review of #1198: the byte-identity evidence against the deleted
    /// AST-driven `event_pattern_guard` was entirely fixture-level. This
    /// pins the guard text format directly, over a hand-built
    /// `EventPatternIr` covering all four cases `EventPatternValueIr` can
    /// hold (`Int`/`Str`/`Bool`/`Variant`).
    #[test]
    fn covers_every_value_shape() {
        let pattern = EventPatternIr {
            fields: vec![
                (
                    "n".to_string(),
                    EventPatternValueIr::Const(ConstVal::Int(3)),
                ),
                (
                    "s".to_string(),
                    EventPatternValueIr::Const(ConstVal::Str("a\"b".to_string())),
                ),
                (
                    "b".to_string(),
                    EventPatternValueIr::Const(ConstVal::Bool(true)),
                ),
                (
                    "status".to_string(),
                    EventPatternValueIr::Variant {
                        tag: "Active".to_string(),
                    },
                ),
            ],
        };
        assert_eq!(
            event_pattern_guard_ir("e", Some(&pattern)).map(|g| bynk_ts::print_expr(&g)),
            Some(
                "e.n === 3 && e.s === \"a\\\"b\" && e.b === true && e.status.tag === \"Active\""
                    .to_string()
            )
        );
        assert_eq!(
            event_pattern_guard_ir("e", None),
            None,
            "a pattern-less subscription has nothing to guard"
        );
    }
}
