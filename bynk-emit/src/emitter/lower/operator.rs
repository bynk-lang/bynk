//! Operators (back-half paydown 1A.5, 1A.14, 2.2c, #4087): binary, short-circuit and equality
//! operators. Moved out of `lower.rs` byte for byte.

use bynk_check::checker::{NamedKind, Ty, TyId, Types};
use bynk_syntax::ast::{BaseType, BinOp, Expr};

use super::super::lower_ctx::LowerCtx;
use super::{
    AndWithIs, Lowered, Pre, hoist_if_slot, lower_and_with_is, lower_expr, mark_unnarrowed, one,
    paren,
};
use crate::ts_build::{bool_lit, ident, not_expr, num_lit};

/// `!(l)`: a `Unary(Not)` over an explicit `Paren` (Decision C of #3715: the
/// printer's own brackets would give `!x`), or today's `!({l})` text.
fn negated_paren(l: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    let c = vec![l];
    bynk_ts::TsExpr::Unary {
        op: bynk_ts::TsUnaryOp::Not,
        expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(one(c)))),
    }
}

/// The short-circuit operators over two operands: `l && r`, `l || r`, and
/// `P implies Q` as `(!(P) || Q)` with its explicit brackets kept. The printer
/// brackets `&&`/`||` by precedence. The left operand is
/// one value used once, so it stays textually present for `tsc`'s narrowing
/// (#1044).
fn short_circuit(op: BinOp, l: bynk_ts::TsExpr, r: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    let (ts_op, implies) = match op {
        BinOp::And => (bynk_ts::TsBinaryOp::And, false),
        BinOp::Or => (bynk_ts::TsBinaryOp::Or, false),
        BinOp::Implies => (bynk_ts::TsBinaryOp::Or, true),
        BinOp::Eq
        | BinOp::NotEq
        | BinOp::Lt
        | BinOp::LtEq
        | BinOp::Gt
        | BinOp::GtEq
        | BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div => unreachable!("only the short-circuit operators reach `short_circuit`"),
    };
    {
        let c = vec![l, r];
        let (l, r) = two(c);
        if implies {
            bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Binary {
                op: ts_op,
                left: Box::new(bynk_ts::TsExpr::Unary {
                    op: bynk_ts::TsUnaryOp::Not,
                    expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(l))),
                }),
                right: Box::new(r),
            }))
        } else {
            bynk_ts::TsExpr::Binary {
                op: ts_op,
                left: Box::new(l),
                right: Box::new(r),
            }
        }
    }
}

/// `(() => { s1; s2; return r; })()`: a `Call` over a bracketed `Arrow`. The statements are an
/// expression's own hoists (and an `is` test's bindings), which must run only when the operator
/// reaches its right operand. The body is the compact `Block`; the printer falls back to the
/// multi-line form when a statement or the value spans lines, because the compact form cannot hold
/// one (1A.39a, Decision E).
pub(crate) fn compact_iife(
    mut body: Vec<bynk_ts::TsStmt>,
    ret: bynk_ts::TsExpr,
) -> bynk_ts::TsExpr {
    body.push(bynk_ts::TsStmt::return_stmt(Some(ret), None));
    let body = bynk_ts::TsArrowBody::Block(body);
    bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Arrow {
            params: Vec::new(),
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(body),
        }))),
        args: Vec::new(),
    }
}

/// A short-circuit operator's `?`-hoisting form (T2.1/T2.3): `let slot: boolean; if (cond) { …;
/// slot = then; } else { …; slot = else; }` ([`hoist_if_slot`]). Returns the slot.
fn hoist_bool_if(
    pre: &mut Pre,
    cond: bynk_ts::TsExpr,
    then_tail: Lowered,
    else_tail: Lowered,
    cx: &mut LowerCtx,
) -> String {
    let ty = Some(bynk_ts::TsType::named("boolean"));
    hoist_if_slot(pre, ty, cond, then_tail, else_tail, cx)
}

pub(super) fn lower_bin_op(op: BinOp, lhs: &Expr, rhs: &Expr, cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    // For `&&` we need to lower `is` bindings into the rhs scope.
    // We handle that here by collecting bindings from lhs, emitting
    // them as `const` declarations before evaluating rhs — but
    // `&&` short-circuits, so simply emitting them inline is wrong.
    // We compile `lhs && (...is binding flow...) rhs` to a function
    // expression: `(lhs && ((bindings) => rhs)())`. Simpler: rely
    // on TypeScript's narrowing for the value-from-is part of
    // `is` patterns. For now, for the special pattern `x is Ok(n)`
    // we lower the rhs assuming the binding `n = x.value` was
    // captured. We use a parenthesised IIFE to scope the binding.
    // `&&` and `implies` with an `is` in the antecedent: its bindings go in the
    // consequent's scope. v0.80: `P implies Q` lowers to `(!(P) || Q)`, and as
    // with `&&` an `is` test in the antecedent binds into the consequent (only
    // reached when the antecedent holds), so both reuse the same is-binding IIFE
    // flow.
    if matches!(op, BinOp::And | BinOp::Implies)
        && let Some(AndWithIs {
            bindings,
            lhs: lhs_lowered,
            rhs: rhs_expr,
            rhs_returns,
            hoisted,
        }) = lower_and_with_is(lhs, rhs, cx)
    {
        for decl in hoisted {
            pre.push_node(decl);
        }
        let lhs_expr = pre.absorb_node(lhs_lowered);
        if bindings.is_empty() {
            return pre.finish(short_circuit(op, lhs_expr, rhs_expr));
        }
        // Every path below emits `rhs` out of line.
        mark_unnarrowed(rhs, cx);
        // T2.3 (R6.3): `bindings` includes rhs's own hoisted statements
        // (`lower_and_with_is`'s doc comment above), so if one of those is a
        // `?`'s propagating early return, the arrow-IIFE below would capture
        // it instead of letting it exit the enclosing function — reuse
        // `hoist_bool_if` (built on T2.1's `if`-hoisting) to hoist this
        // operator as a real `if` statement instead.
        //
        // #1044 review: `hoist_if_slot`'s bare `slot` return is right
        // for the general path (no narrowing at stake there — see below), but
        // wrong here. `lhs_expr` is the very `is`-check `lower_if`'s own
        // `gather_is_bindings_for_emit` relies on being *textually present* in
        // the condition it hands to `tsc` — that's what lets `o` narrow to
        // `Hit` for a hoisted `const score = o.score;` in the caller's
        // then-block. Dropping `lhs_expr` for a disconnected `let __rN:
        // boolean` (assigned across an `if`/`else`, not a `tsc`-4.4+ aliased
        // `const`) severs that link and turns a silent `?`-escape miscompile
        // into a hard `tsc --strict` failure on the same input. Re-including
        // `lhs_expr` in the returned text — safe to re-evaluate, since it is
        // always a pure tag/field read on an already-hoisted receiver, never
        // the original (possibly effectful) `is` subject — keeps the
        // conjunction's left side exactly where a caller's narrowing expects
        // to find it; `slot` only stands in for rhs's value, gated by the
        // same `if` that already guards evaluating it. (For `implies` the guard
        // is `lhs_expr` itself, not its negation, for the same reason.)
        if rhs_returns {
            let otherwise = if op == BinOp::And { "false" } else { "true" };
            let slot = hoist_bool_if(
                &mut pre,
                lhs_expr.clone(),
                Lowered {
                    pre: bindings,
                    expr: rhs_expr,
                },
                Lowered::node(bool_lit(otherwise == "true")),
                cx,
            );
            return pre.finish(short_circuit(op, lhs_expr, ident(&slot)));
        }
        // Emit:  lhs && ((() => { const n = ...; return rhs; })())
        let iife: bynk_ts::TsExpr = compact_iife(bindings, rhs_expr);
        return pre.finish(short_circuit(op, lhs_expr, paren(iife)));
    }
    let l_lowered = pre.lower_node(lhs, cx);
    // #1/#3 review: `&&`/`||`/`implies` short-circuit — `design/bynk-type-
    // system.md:1671-1678` calls this "both a performance property and a
    // safety property … developers can rely on." Lowering rhs into the same
    // `pre` as lhs — the previous behaviour for every case that didn't go
    // through `lower_and_with_is` above (i.e. `||` always, and `&&`/`implies`
    // whenever the antecedent contains no `is`) — hoisted rhs's
    // pre-statements unconditionally, running them even when lhs's value
    // means rhs is never evaluated. Only inline `r.expr` bare when rhs
    // hoists nothing; otherwise wrap it in an IIFE so the hoist runs only
    // when the operator actually reaches rhs.
    //
    // T2.3 (R6.3): if rhs's hoist itself contains a `?`'s propagating early
    // return (`if (__rN.tag === "Err") return __rN;`), a plain
    // `(() => { ...; return expr; })()` wrap would capture that `return`
    // instead of letting it exit the enclosing function — the same class of
    // miscompile `lower_match_as_iife` had. `LowerCtx::emitted_early_return`
    // says whether rhs's own lowering hit that case; when it did, hoist this
    // operator as a real `if` statement via `hoist_bool_if` instead of an
    // expression-position IIFE — the `return` then keeps exiting the
    // enclosing function, and still only runs when the operator actually
    // reaches rhs. An ordinary hoist (no `?` involved) keeps the cheaper arrow
    // form; nothing inside it needs to escape past the arrow, and this is the
    // shape the `947_short_circuit_rhs_hoist` fixture pins.
    if matches!(op, BinOp::And | BinOp::Or | BinOp::Implies) {
        let l = l_lowered;
        let saved_early_return = std::mem::take(&mut cx.emitted_early_return);
        // #1751: `rhs` may be lowered inside an arrow (below), which would scope
        // an `is` receiver temp it introduces away from a branch that reads it
        // (`if !(f(a) is Some(x)) || !(f(b) is Some(y)) { … } else { x + y }`).
        // Hoist the temps' declarations out of the arrow, as `lower_and_with_is`
        // does for `&&` (#1654); a nested operator adds to the outermost list.
        let outermost = cx.is_temp_hoist.is_none();
        if outermost {
            cx.is_temp_hoist = Some(Vec::new());
        }
        let r = lower_expr(rhs, cx);
        if outermost {
            for decl in cx.is_temp_hoist.take().unwrap_or_default() {
                pre.push_node(decl);
            }
        }
        let rhs_returns = cx.emitted_early_return;
        cx.emitted_early_return = saved_early_return || rhs_returns;
        if !r.pre.is_empty() {
            // Both paths below emit `rhs` out of line.
            mark_unnarrowed(rhs, cx);
        }
        if !r.pre.is_empty() && rhs_returns {
            let value = match op {
                BinOp::And => hoist_bool_if(&mut pre, l, r, Lowered::node(bool_lit(false)), cx),
                BinOp::Or => hoist_bool_if(&mut pre, l, Lowered::node(bool_lit(true)), r, cx),
                BinOp::Implies => hoist_bool_if(
                    &mut pre,
                    negated_paren(l),
                    Lowered::node(bool_lit(true)),
                    r,
                    cx,
                ),
                BinOp::Eq
                | BinOp::NotEq
                | BinOp::Lt
                | BinOp::LtEq
                | BinOp::Gt
                | BinOp::GtEq
                | BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div => unreachable!("guarded by the outer `matches!`"),
            };
            return pre.finish(ident(&value));
        }
        let Lowered {
            pre: r_pre,
            expr: r_expr,
        } = r;
        let rhs_value: bynk_ts::TsExpr = if r_pre.is_empty() {
            r_expr
        } else {
            compact_iife(r_pre, r_expr)
        };
        // `P implies Q` ≡ `!P || Q` (no `is` bindings in the antecedent — that
        // case returned above via `lower_and_with_is`).
        return pre.finish(short_circuit(op, l, rhs_value));
    }
    let r_lowered = pre.lower_node(rhs, cx);
    let built = build_value_bin_op(op, lhs, vec![l_lowered, r_lowered], cx);
    pre.finish(built)
}

/// A value operator (`==`, `<`, `+`, …; never `&&`/`||`/`implies`) over
/// operands that are already lowered, `children` = `[lhs, rhs]`. `lhs` is the
/// source operand, read only for its type. Split out of [`lower_bin_op`] so
/// that `expect` can build its condition over operands it lowered once and
/// also shows in the failure detail.
pub(in crate::emitter) fn build_value_bin_op(
    op: BinOp,
    lhs: &Expr,
    children: Vec<bynk_ts::TsExpr>,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    debug_assert!(!matches!(op, BinOp::And | BinOp::Or | BinOp::Implies));
    let tys = cx.ty_render();
    // #2622: `Decimal` arithmetic is exact and goes through the runtime; the
    // checker rejects `/` and any mixed operand, so the left operand decides.
    let decimal_helper = matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul)
        .then(|| {
            cx.commons()
                .expr_types
                .get(&lhs.id)
                .and_then(|te| te.ty.base(tys.types()))
        })
        .flatten()
        .filter(|b| *b == BaseType::Decimal)
        .map(|_| match op {
            BinOp::Add => "__bynkDecimalAdd",
            BinOp::Sub => "__bynkDecimalSub",
            BinOp::Implies
            | BinOp::Or
            | BinOp::And
            | BinOp::Eq
            | BinOp::NotEq
            | BinOp::Lt
            | BinOp::LtEq
            | BinOp::Gt
            | BinOp::GtEq
            | BinOp::Mul
            | BinOp::Div => "__bynkDecimalMul",
        });
    // Slice 1A.5 (#3703): each value-operator branch builds a node. The
    // `cx.note_*` side effects fire here, once.
    if let Some(helper) = decimal_helper {
        cx.note_decimal(helper);
        helper_call(helper, children)
    } else if op == BinOp::Div {
        // v0.21: division is operand-typed (ADR 0042) — `Float`
        // true-divides; `Int` keeps truncating. The checker rejects
        // mixed operands, so the left operand decides; a missing
        // type entry falls back to the `Int` (truncating) lowering.
        let lhs_is_float = cx
            .commons()
            .expr_types
            .get(&lhs.id)
            .and_then(|te| te.ty.base(tys.types()))
            == Some(BaseType::Float);
        if lhs_is_float {
            plain_binary(op, children)
        } else {
            // #1657: `Int` division by zero traps, a runtime fault, rather than
            // producing `Infinity`/`NaN` as an `Int`. A runtime helper rather than
            // an inline guard, so coverage never reports its fault branch.
            helper_call("__bynkIntDiv", children)
        }
    } else if let Some(helper) = matches!(op, BinOp::Eq | BinOp::NotEq)
        .then(|| cx.commons().expr_types.get(&lhs.id).map(|te| te.ty))
        .and_then(|operand| equality_helper(operand, tys.types(), cx))
    {
        // #1652 (runtime-semantics track §3.1): `==` is structural, dispatched by
        // the operand's type, the same operand-typed shape as `Div` above (and
        // `Bytes`, ADR 0142 D4). The checker rejects mixed operands, so the left
        // operand decides. A primitive operand returns `None` here and falls
        // through to host `===`/`!==` below.
        let call = helper_call(helper, children);
        if op == BinOp::Eq {
            call
        } else {
            not_expr(call)
        }
    } else if let Some(operand) = matches!(op, BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq)
        .then(|| cx.commons().expr_types.get(&lhs.id).map(|te| te.ty))
        .flatten()
        .filter(|t| !is_bare_ordered(*t, tys.types()))
    {
        // Constrained generics slice 1 (DECISION F): an `Ord` type that is not
        // a plain base compares through its comparator. A base type keeps the
        // bare JS operator (and so `Float` keeps its IEEE meaning, `NaN < x` is
        // `false`).
        let cmp = cx.comparator(operand);
        ordered_comparison(op, cmp, children)
    } else {
        plain_binary(op, children)
    }
}

/// The `TsBinaryOp` for a value operator (the short-circuit operators are
/// lowered by their own arms and never reach this function).
fn value_binary_op(op: BinOp) -> bynk_ts::TsBinaryOp {
    use bynk_ts::TsBinaryOp as T;
    match op {
        BinOp::Eq => T::StrictEq,
        BinOp::NotEq => T::StrictNotEq,
        BinOp::Lt => T::LessThan,
        BinOp::LtEq => T::LessThanEq,
        BinOp::Gt => T::GreaterThan,
        BinOp::GtEq => T::GreaterThanEq,
        BinOp::Add => T::Add,
        BinOp::Sub => T::Sub,
        BinOp::Mul => T::Mul,
        BinOp::Div => T::Div,
        BinOp::And | BinOp::Or | BinOp::Implies => {
            unreachable!("short-circuit operators are lowered as text before `value_binary_op`")
        }
    }
}

/// `l <op> r`: a `Binary` node (the printer decides the brackets), or today's
/// `{l} {op} {r}` text.
pub(super) fn plain_binary(op: BinOp, children: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    let c = children;
    let (l, r) = two(c);
    bynk_ts::TsExpr::Binary {
        op: value_binary_op(op),
        left: Box::new(l),
        right: Box::new(r),
    }
}

/// `helper(l, r)` over a runtime helper's genuine identifier.
pub(super) fn helper_call(helper: &'static str, children: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    let c = children;
    let (l, r) = two(c);
    bynk_ts::TsExpr::Call {
        callee: Box::new(ident(helper)),
        args: vec![l, r],
    }
}

/// `cmp(l, r) <op> 0` for a non-base `Ord` type. The comparator is a node, so a
/// composite comparator (`__bynkCmpList(__bynkCmpInt)`) is a callee like any
/// other (1A.16 closes the text path 1A.5 left for it).
pub(super) fn ordered_comparison(
    op: BinOp,
    cmp: bynk_ts::TsExpr,
    children: Vec<bynk_ts::TsExpr>,
) -> bynk_ts::TsExpr {
    let c = children;
    let (l, r) = two(c);
    bynk_ts::TsExpr::Binary {
        op: value_binary_op(op),
        left: Box::new(bynk_ts::TsExpr::Call {
            callee: Box::new(cmp),
            args: vec![l, r],
        }),
        right: Box::new(num_lit("0".to_string())),
    }
}

/// The two elements of a two-child builder's argument.
fn two<T>(children: Vec<T>) -> (T, T) {
    let mut it = children.into_iter();
    match (it.next(), it.next(), it.next()) {
        (Some(l), Some(r), None) => (l, r),
        _ => unreachable!("a two-child builder is given exactly two children"),
    }
}

/// Whether a key or operand of this type orders with the bare JS `<`/`>`: a base
/// type other than `Bytes` (a `Uint8Array`) and `String` (code-point order, #3954),
/// or a refined type over one. Every
/// other `Ord` type goes through a comparator.
fn is_bare_ordered(ty: TyId, tys: &Types) -> bool {
    matches!(
        ty.base(tys),
        Some(BaseType::Int | BaseType::Float | BaseType::Duration | BaseType::Instant)
    )
}

/// How `==`/`!=` lowers for an operand of a given type. #1652 (runtime-semantics
/// track §3.1).
#[derive(Debug, PartialEq)]
enum EqualityLowering {
    /// Host `===`: every base type except `Bytes` erases to a JS primitive
    /// (`number`/`string`/`boolean`), as does a refined or opaque type over one,
    /// and `()` erases to `undefined`.
    Host,
    /// `__bynkBytesEqual`: `Bytes`, including an opaque type over it, erases to
    /// a `Uint8Array`, which `===` compares by reference (ADR 0142 D4).
    Bytes,
    /// `__bynkDecimalEqual` (#2622): a `Decimal` is a `{ coefficient, scale }`
    /// object that compares by numeric value (`1.20 == 1.2`).
    Decimal,
    /// `__bynkEq`, the runtime's structural walker: records, sums, `Option`,
    /// `Result`, `List`, value-level `Map`, the built-in error records, and type
    /// variables (a generic `T` is not monomorphised, so its runtime shape is
    /// only known to the walker).
    Structural,
}

/// The runtime helper `==`/`!=` calls for an operand type, noting its import, or
/// `None` for a host-`===` operand. A missing type entry takes the walker, which
/// is also correct for primitives.
fn equality_helper(operand: Option<TyId>, tys: &Types, cx: &LowerCtx) -> Option<&'static str> {
    match operand.map(|t| equality_lowering(t, tys)) {
        Some(EqualityLowering::Host) => None,
        Some(EqualityLowering::Bytes) => {
            cx.note_bytes();
            Some("__bynkBytesEqual")
        }
        Some(EqualityLowering::Decimal) => {
            cx.note_decimal("__bynkDecimalEqual");
            Some("__bynkDecimalEqual")
        }
        Some(EqualityLowering::Structural) | None => Some("__bynkEq"),
    }
}

fn equality_lowering(ty: TyId, tys: &Types) -> EqualityLowering {
    match &*tys.get(ty) {
        Ty::Base(BaseType::Bytes)
        | Ty::Named {
            kind: NamedKind::Refined(BaseType::Bytes) | NamedKind::Opaque(BaseType::Bytes),
            ..
        } => EqualityLowering::Bytes,
        Ty::Base(BaseType::Decimal)
        | Ty::Named {
            kind: NamedKind::Refined(BaseType::Decimal) | NamedKind::Opaque(BaseType::Decimal),
            ..
        } => EqualityLowering::Decimal,
        Ty::Base(_)
        | Ty::Unit
        | Ty::Named {
            kind: NamedKind::Refined(_) | NamedKind::Opaque(_),
            ..
        } => EqualityLowering::Host,
        Ty::Named {
            kind: NamedKind::Record | NamedKind::Sum,
            ..
        }
        | Ty::Option(_)
        | Ty::Result(..)
        | Ty::List(_)
        | Ty::Set(_)
        | Ty::Map(..)
        | Ty::HttpResult(_)
        | Ty::HttpOutcome(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::RequestQuery
        | Ty::RequestCookies
        | Ty::TokenClaims
        | Ty::ValidationError
        | Ty::DecodeError
        | Ty::Actor(_)
        | Ty::ActorSum(_)
        | Ty::Var(_) => EqualityLowering::Structural,
        // Not equality-supporting: the checker rejects `==` on these
        // (`checker/equality.rs`), so no certified program reaches here. The
        // walker is the safe answer if one ever did.
        Ty::Error
        | Ty::Never
        | Ty::Fn { .. }
        | Ty::Effect(_)
        | Ty::Query(_)
        | Ty::Stream(_)
        | Ty::TestConnection(_)
        | Ty::Connection(_) => EqualityLowering::Structural,
    }
}

#[cfg(test)]
mod compact_iife_tests {
    use super::*;

    fn multi_line_stmt() -> bynk_ts::TsStmt {
        bynk_ts::TsStmt::if_stmt(
            ident("flag"),
            bynk_ts::TsStmt::block(vec![bynk_ts::TsStmt::expr_stmt(ident("done"), None)], None),
            None,
        )
    }

    /// A hoisted statement that prints over several lines cannot sit in the compact `Block`
    /// body (the printer refuses it); the arrow takes the multi-line body instead.
    #[test]
    fn a_multi_line_hoist_takes_the_multi_line_body() {
        let iife = compact_iife(vec![multi_line_stmt()], ident("r"));
        let printed = bynk_ts::print_expr(&iife);
        assert!(printed.starts_with("(() => {\n"), "{printed}");
        assert!(printed.contains("if (flag) {"), "{printed}");
        assert!(printed.contains("return r;"), "{printed}");
    }

    /// One-line statements keep the compact one-line form the corpus has always had.
    #[test]
    fn one_line_hoists_stay_compact() {
        let stmt = bynk_ts::TsStmt::expr_stmt(ident("a"), None);
        let iife = compact_iife(vec![stmt], ident("r"));
        assert_eq!(bynk_ts::print_expr(&iife), "(() => { a; return r; })()");
    }
}
