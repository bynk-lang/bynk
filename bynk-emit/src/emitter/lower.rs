//! Expression lowering: the core of the `LowerCtx`-driven engine that turns Bynk expressions into
//! TypeScript nodes. Split out of `emitter.rs` (ADR 0060) and, in the back-half paydown's Phase 2
//! (#4087), into one module per family under `lower/`. This file keeps what every family calls
//! back into: [`Lowered`], [`Pre`], the leaf builders and [`lower_expr`], whose `match` reads as the
//! table of contents. Each family module is re-exported below, so `lower::X` still names its items.
//!
//! - [`calls`]: the method-call dispatcher, UFCS and cross-context calls, function calls, `settle`.
//! - [`claims`]: the Idempotency claim scope a service handler opens.
//! - [`control`]: `?`, the value-position `if`, lambdas, block expressions, the `match` IIFE.
//! - [`expect`]: `expect`, fault claims, recorded calls, `trace` and observations.
//! - [`ident`]: identifiers, field access and map-key decoding.
//! - [`is`]: `is` and narrowing, and the `&&`/`implies` forms that bind its names.
//! - [`literal`]: scalar and interpolated-string literals, record construction and spread.
//! - [`mock`]: `Val[T]` mocks and refined defaults.
//! - [`operator`]: binary, short-circuit and equality operators.
//! - [`pattern`]: pattern tests and bindings, or-patterns and the event-pattern guard.
//!
//! The statement emitter is [`lower_stmt`](super::lower_stmt), the `List` kernel is
//! [`lower_list`](super::lower_list), and the other method-call families are under
//! [`lower_call`](super::lower_call).

use bynk_check::checker::{NamedKind, Ty};
use bynk_check::kernel_op::IntrinsicNs;
use bynk_syntax::ast::{BaseType, Expr, ExprKind, UnaryOp};

use super::lower_ctx::LowerCtx;
use super::lower_stmt::{
    Item, arms_contain_question, block_body_into, block_contains_question,
    hoist_value_as_statement, if_tail, match_tail, ternary_shaped,
};
use super::ts_types::{unchecked_construct, unchecked_construct_test_node};
use super::*;
use crate::ts_build::{const_, ident};
#[cfg(test)]
use crate::ts_build::{num_lit, str_lit};

mod calls;
mod claims;
mod control;
mod expect;
pub(super) mod expect_bindings;
pub(super) mod expect_narrowing;
pub(super) mod expect_was;
mod ident;
mod is;
mod literal;
mod mock;
mod operator;
mod pattern;

pub(super) use calls::*;
pub(crate) use claims::*;
pub(super) use control::*;
pub(super) use expect::*;
use ident::*;
pub(crate) use is::*;
pub(super) use literal::*;
use mock::*;
pub(crate) use operator::*;
pub(crate) use pattern::*;

/// #1 review: an expression's lowered text, paired with the statements that
/// must run before it — the typed replacement for the `stmts: &mut
/// Vec<String>` sink `lower_expr_into` used to take. A caller that receives a
/// `Lowered` cannot accidentally drop `pre` (unlike the old sink, dropping it
/// took no code at all) or splice it into a string where a statement is
/// required (`Lowered` is not a `String`) — the two failure modes the sink
/// pattern actually produced (`lower_if`'s/`lower_tail_expr`'s ternary paths,
/// `lower_and_with_is`'s splice).
///
/// T2.1 (R6.2): this is now the *only* way a lowering function hands
/// statements to its caller. There is no sink parameter left in `bynk-emit`
/// to hand them to instead.
pub(crate) struct Lowered {
    pub pre: Vec<bynk_ts::TsStmt>,
    pub expr: bynk_ts::TsExpr,
}

/// `-x` / `!x`: a `Unary` node (the printer brackets the operand), or today's
/// `{sym}{operand}` text.
fn unary_op(op: UnaryOp, child: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    let ts_op = match op {
        UnaryOp::Neg => bynk_ts::TsUnaryOp::Neg,
        UnaryOp::Not => bynk_ts::TsUnaryOp::Not,
    };
    {
        let c = vec![child];
        bynk_ts::TsExpr::Unary {
            op: ts_op,
            expr: Box::new(one(c)),
        }
    }
}

/// `[a, b, …]`: an `Array` node, or today's `[a, b]` text.
fn array(children: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    let items = children;
    bynk_ts::TsExpr::Array {
        items,
        multiline: false,
    }
}

/// An explicit source `( … )`: `bynk_ts` has `TsExpr::Paren`, so the source's
/// own parentheses are kept as a node.
pub(super) fn paren(child: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    let c = vec![child];
    bynk_ts::TsExpr::Paren(Box::new(one(c)))
}

/// `(test ? consequent : alternate)`: a `Paren(Conditional)` node, or today's
/// text. Shared by the value-position ternary `if` and the statement emitter's
/// ternary tail (1A.11), so both keep today's outer brackets.
pub(super) fn ternary_built(
    test: bynk_ts::TsExpr,
    consequent: bynk_ts::TsExpr,
    alternate: bynk_ts::TsExpr,
) -> bynk_ts::TsExpr {
    let c = vec![test, consequent, alternate];
    let [test, consequent, alternate]: [bynk_ts::TsExpr; 3] = c
        .try_into()
        .unwrap_or_else(|_| unreachable!("a ternary has three children"));
    bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Conditional {
        test: Box::new(test),
        consequent: Box::new(consequent),
        alternate: Box::new(alternate),
    }))
}

/// `object.property` over a genuine identifier `object`.
pub(super) fn member_node(object: &str, property: &str) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Member {
        object: Box::new(ident(object)),
        property: property.to_string(),
    }
}

/// `Type.property` for a Bynk type name, which a `uses` full-name qualifier may
/// spell `lib.y.Status`: that is read through the unit's namespace import of the
/// commons (#4397), as every other name written that way is.
pub(super) fn type_member_node(type_name: &str, property: &str) -> bynk_ts::TsExpr {
    let object = bynk_check::emitted_names::use_qualified_name(type_name)
        .unwrap_or_else(|| type_name.to_string());
    member_node(&object, property)
}

/// A runtime-constructor call with one argument — `Ok(x)`, `Some(x)`,
/// `__bynkDecimalNeg(x)`, or `HttpResult.Ok(x)` when `object` is given — as a
/// call node over genuine identifiers.
pub(super) fn runtime_ctor_call(
    object: Option<&'static str>,
    name: &'static str,
    arg: bynk_ts::TsExpr,
) -> bynk_ts::TsExpr {
    let c = vec![arg];
    let callee = match object {
        Some(o) => bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident(o.to_string())),
            property: name.to_string(),
        },
        None => bynk_ts::TsExpr::Ident(name.to_string()),
    };
    bynk_ts::TsExpr::Call {
        callee: Box::new(callee),
        args: vec![one(c)],
    }
}

/// The single element of a one-child builder's argument.
fn one<T>(children: Vec<T>) -> T {
    let mut it = children.into_iter();
    match (it.next(), it.next()) {
        (Some(only), None) => only,
        _ => unreachable!("a one-child builder is given exactly one child"),
    }
}

impl Lowered {
    /// An expression that hoists nothing: the common leaf case.
    pub(crate) fn node(expr: bynk_ts::TsExpr) -> Self {
        Lowered {
            pre: Vec::new(),
            expr,
        }
    }

    /// The hoisted statements as statement-body items, for the statement emitter.
    pub(super) fn pre_items(&self) -> Vec<Item> {
        self.pre.iter().map(stmt_item).collect()
    }
}

/// A statement as a statement-body item.
pub(super) fn stmt_item(stmt: &bynk_ts::TsStmt) -> Item {
    Item::Node(Box::new(stmt.clone()))
}

/// T2.1 (R6.2): the statements an expression under construction has hoisted so
/// far. This is the *owned* counterpart to the deleted sink: a `Pre` belongs to
/// the one function assembling an expression, and leaves it only by being
/// [finished](Pre::finish) into that expression's [`Lowered`]. It is
/// deliberately never a parameter — that is the whole point. A lowering
/// function cannot be handed somewhere to write statements, so it cannot write
/// them anywhere but its own return value, and the caller decides where they
/// land (a real statement position, or a hoist-safe wrapper).
pub(crate) struct Pre(Vec<bynk_ts::TsStmt>);

impl Pre {
    pub(crate) fn new() -> Self {
        Pre(Vec::new())
    }

    /// Lower `e`, absorbing whatever it hoists, and hand back its node.
    pub(crate) fn lower_node(&mut self, e: &Expr, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
        let lowered = lower_expr(e, cx);
        self.absorb_node(lowered)
    }

    /// Absorb an already-built [`Lowered`] and hand back its expression.
    pub(crate) fn absorb_node(&mut self, lowered: Lowered) -> bynk_ts::TsExpr {
        self.0.extend(lowered.pre);
        lowered.expr
    }

    /// Absorb an optional [`Lowered`]: a delegated kernel's node passes through to the
    /// dispatcher's caller. Declining hoists nothing.
    pub(crate) fn absorb_built(&mut self, lowered: Option<Lowered>) -> Option<bynk_ts::TsExpr> {
        lowered.map(|l| self.absorb_node(l))
    }

    /// Hoist a statement this function generated itself, as a typed node.
    pub(crate) fn push_node(&mut self, stmt: bynk_ts::TsStmt) {
        self.0.push(stmt);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many statements have been hoisted so far.
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// Hoist `const __rN = <value>;` and hand back a read of `__rN`, so a
    /// value used twice is evaluated once.
    pub(super) fn bind_temp(
        &mut self,
        value: bynk_ts::TsExpr,
        cx: &mut LowerCtx,
    ) -> bynk_ts::TsExpr {
        let tmp = cx.fresh();
        self.0.push(const_(&tmp, value));
        ident(&tmp)
    }

    /// The accumulated statements as statement-body items (see
    /// [`Lowered::pre_items`]).
    pub(super) fn items(&self) -> Vec<Item> {
        self.0.iter().map(stmt_item).collect()
    }

    /// The accumulated statements as `TsStmt`s for a typed body.
    pub(crate) fn ts_stmts(&self) -> Vec<bynk_ts::TsStmt> {
        self.0.clone()
    }

    /// Pair the accumulated statements with the expression they precede.
    pub(crate) fn finish(self, expr: bynk_ts::TsExpr) -> Lowered {
        Lowered { pre: self.0, expr }
    }

    /// Finish with an already-built [`Lowered`]: its hoisted statements join this one's, and its
    /// expression is the result.
    pub(crate) fn finish_with(mut self, lowered: Lowered) -> Lowered {
        let expr = self.absorb_node(lowered);
        self.finish(expr)
    }
}

/// The canonical entry point: lower `e`, returning its text and whatever
/// statements must run first.
pub(crate) fn lower_expr(e: &Expr, cx: &mut LowerCtx) -> Lowered {
    // v0.9.4: a literal the checker admitted as a refined type (expected-type-
    // directed construction) is branded directly — the refinement was already
    // verified at compile time, so there is no runtime check and no `Result`.
    // ADR 0182: a refined/alias type has no public `.unsafe`, so this brands with
    // an inline `as` cast (`unchecked_construct` with `is_opaque = false`) rather
    // than calling one; opaque is never admitted here (`NamedKind::Refined` only).
    if let Some(Ty::Named {
        name,
        kind: NamedKind::Refined(_),
        ..
    }) = cx.commons().expr_ty(e.id).as_deref()
        && let Some(raw) = lower_const_literal_node(e)
    {
        // ADR 0182: in GENERATED TEST scaffolding a branded type is in scope only
        // as an `any`-typed value binding (`const { T } = ns as any`), never as a
        // type, so a `(raw as T)` cast fails to resolve `T` ("refers to a value,
        // but is being used as a type"). Brand via `unchecked_construct_test_node`
        // (refined → `(raw as any)`) there, exactly as the `Val[T](lit)` mock path
        // does. Production emission keeps the real `(raw as T)` brand.
        if cx.in_test_scaffold() {
            return Lowered::node(unchecked_construct_test_node(name, raw, false));
        }
        // #2058: a consumed context's refined type is named under its
        // namespace; the module imports no bare name for it.
        return Lowered::node(unchecked_construct(
            &cx.consumed_type_name(name),
            raw,
            false,
        ));
    }
    let mut pre = Pre::new();
    match &e.kind {
        // Literals are typed nodes (slice 1A.2): see `Lit::lower`.
        ExprKind::IntLit { value, .. } => Lit::Int(value.to_string()).lower(),
        ExprKind::FloatLit { lexeme, .. } => Lit::Float(lexeme).lower(),
        ExprKind::DecimalLit { coef, scale, .. } => Lit::Decimal(coef, *scale).lower(),
        ExprKind::DurationLit { millis, .. } => Lit::Duration(millis.to_string()).lower(),
        ExprKind::StrLit(s) => Lit::Str(s).lower(),
        ExprKind::BoolLit(b) => Lit::Bool(*b).lower(),
        ExprKind::None => Lit::None.lower(),
        ExprKind::UnitLit => Lit::Unit.lower(),
        // Slice C: `Wire(<String>)` in a generic position lowers to its raw inner
        // string. The system-http driver site intercepts `Wire` args before this
        // to route them raw (no serialisation) and switch to the outcome decoder.
        ExprKind::Wire(inner) => {
            let child = pre.lower_node(inner, cx);
            pre.finish(child)
        }
        // v0.43 (ADR 0075): an interpolated string lowers to a TS template
        // literal — chunks as escaped literal text, holes as `${String(…)}`.
        ExprKind::InterpStr(parts) => pre.finish_with(lower_interp_str(parts, cx)),
        // v0.20b: a list literal lowers to a TS array literal; `readonly` is
        // a type-level property and the checker owns the element typing.
        ExprKind::ListLit(elems, ..) => {
            let children: Vec<bynk_ts::TsExpr> =
                elems.iter().map(|el| pre.lower_node(el, cx)).collect();
            pre.finish(array(children))
        }
        ExprKind::Ident(id) => pre.finish(lower_ident(e, id, cx)),
        ExprKind::Call { name, args, .. } => pre.finish_with(lower_call(e, name, args, cx)),
        ExprKind::UnaryOp(UnaryOp::Neg, inner)
            if cx
                .commons()
                .expr_types
                .get(&inner.id)
                .and_then(|te| te.ty.base(cx.commons().tys()))
                == Some(BaseType::Decimal) =>
        {
            // #2622: negating a `Decimal` is exact, not a JS unary minus.
            let child = pre.lower_node(inner, cx);
            cx.note_decimal("__bynkDecimalNeg");
            pre.finish(runtime_ctor_call(None, "__bynkDecimalNeg", child))
        }
        ExprKind::UnaryOp(op, inner) => {
            let child = pre.lower_node(inner, cx);
            pre.finish(unary_op(*op, child))
        }
        ExprKind::BinOp(op, lhs, rhs) => pre.finish_with(lower_bin_op(*op, lhs, rhs, cx)),
        ExprKind::Paren(inner) => {
            let child = pre.lower_node(inner, cx);
            pre.finish(paren(child))
        }
        ExprKind::Ok(inner) => {
            let child = pre.lower_node(inner, cx);
            // v0.9: `Ok` is overloaded — use the checker's recorded type to
            // decide between `Result.Ok` and `HttpResult.Ok`.
            let http = matches!(
                cx.commons().expr_ty(e.id).as_deref(),
                Some(Ty::HttpResult(_, _))
            );
            pre.finish(runtime_ctor_call(http.then_some("HttpResult"), "Ok", child))
        }
        ExprKind::Err(inner) => {
            let arg = pre.lower_node(inner, cx);
            pre.finish(runtime_ctor_call(None, "Err", arg))
        }
        ExprKind::Some(inner) => {
            let child = pre.lower_node(inner, cx);
            pre.finish(runtime_ctor_call(None, "Some", child))
        }
        ExprKind::Question(inner) => lower_question(e, inner, cx),
        ExprKind::RecordConstruction {
            type_name, fields, ..
        } => pre.finish_with(lower_record_construction(e, type_name, fields, cx)),
        ExprKind::FieldAccess { .. }
            if matches!(
                cx.commons().callees.get(&e.id),
                Some(bynk_check::checker::Callee::Intrinsic {
                    ns: IntrinsicNs::TestRun,
                    ..
                })
            ) =>
        {
            // #2138: `run.id` — the module's `__bynkRunId`.
            pre.finish(ident("__bynkRunId"))
        }
        ExprKind::FieldAccess { receiver, field } => {
            pre.finish_with(lower_field_access(e, receiver, field, cx))
        }
        ExprKind::MethodCall { .. } => pre.finish_with(lower_method_call(e, cx)),
        // #1750: a value-position `if`/`match`/block that holds a `?` becomes a
        // real statement, so the `?`'s `return` exits the function rather than
        // an arrow. A ternary-shaped `if` is left to `lower_if`, whose own
        // hoist already handles a branch that hoists.
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } if !ternary_shaped(cond, then_block, else_block, cx)
            && (block_contains_question(then_block) || block_contains_question(else_block)) =>
        {
            pre.finish_with(hoist_value_as_statement(e, cx, |body, cx| {
                if_tail(body, cond, then_block, else_block, cx, false)
            }))
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => pre.finish_with(lower_if(e, cond, then_block, else_block, cx)),
        // v0.20a: a lambda lowers to a TS arrow; `async` iff its checked type
        // is an effectful function. A lambda is deliberately its own return
        // scope (ADR 0178) — its body is lowered with no access to the outer
        // `pre`, so anything it hoists (match-as-IIFE etc.) stays local to
        // the arrow, correctly, because a `?`'s `return` there is supposed to
        // exit the lambda.
        ExprKind::Lambda(lambda) => pre.finish(lower_lambda(e, lambda, cx)),
        // Defensive: the parser builds `ExprKind::Block` only for a lambda body,
        // which `lower_lambda` emits itself, so neither `Block` arm is reached
        // today. Routed like `if`/`match` so a future value-position block
        // with a `?` would not reintroduce the arrow miscompile.
        ExprKind::Block(b) if block_contains_question(b) => {
            pre.finish_with(hoist_value_as_statement(e, cx, |body, cx| {
                block_body_into(body, b, cx, false)
            }))
        }
        ExprKind::Block(b) => pre.finish(lower_block_as_expr(b, cx)),
        ExprKind::Match { discriminant, arms } if arms_contain_question(arms) => {
            pre.finish_with(hoist_value_as_statement(e, cx, |body, cx| {
                match_tail(body, discriminant, arms, cx, false)
            }))
        }
        ExprKind::Match { discriminant, arms } => {
            pre.finish_with(lower_match_as_iife(discriminant, arms, cx))
        }
        ExprKind::Is { value, pattern } => pre.finish_with(lower_is(value, pattern, cx)),
        ExprKind::EffectPure(inner) => {
            let child = pre.lower_node(inner, cx);
            pre.finish({
                let c = vec![child];
                bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(member_node("globalThis", "Promise")),
                        property: "resolve".to_string(),
                    }),
                    args: vec![one(c)],
                }
            })
        }
        ExprKind::RecordSpread {
            type_name: _,
            base,
            overrides,
            ..
        } => pre.finish_with(lower_record_spread(base, overrides, cx)),
        ExprKind::Expect(inner) => {
            let built = expect_built(inner, &mut pre, cx);
            pre.finish(built)
        }
        // 1A.17: the test-domain constructs build nodes.
        ExprKind::Val { type_ref, args, .. } => pre.finish_with(lower_val(type_ref, args, cx)),
        ExprKind::Observation(o) => {
            let built = lower_observation(o, cx);
            pre.finish(built)
        }
        ExprKind::Faults(f) => {
            let built = lower_faults(f, e.span, cx);
            pre.finish(built)
        }
        // `trace(Cap.op)` → the recorded calls mapped to per-call records
        // whose fields are the operation's parameters (positionally).
        ExprKind::Trace { cap, op } => pre.finish(trace_node(cx, &cap.name, &op.name)),
    }
}

/// Test-only: a node as the text the printer gives it.
#[cfg(test)]
mod test_support {
    pub(super) trait Printed {
        fn printed(&self) -> String;
    }

    impl Printed for bynk_ts::TsExpr {
        fn printed(&self) -> String {
            bynk_ts::print_expr(self)
        }
    }
}
#[cfg(test)]
use test_support::Printed;

#[cfg(test)]
mod bridge_tests {
    use super::*;
    use crate::emitter::lower_stmt::write_line;

    /// The hoisted statements of `pre`, printed at depth 0, one entry per line.
    fn stmts_text(pre: &Pre) -> Vec<String> {
        pre.ts_stmts()
            .iter()
            .flat_map(|s| {
                bynk_ts::print_stmt(s, 0)
                    .lines()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// A plain `Set.remove`: its body is a comma expression node (1A.35a part B).
    fn removed(set: &str, key: &str) -> String {
        format!(
            "((__s: globalThis.Set<string>) => (__s.delete({key}), __s as ReadonlySet<string>))(new globalThis.Set<string>({set}))"
        )
    }

    /// A `contains` over a plain `Set` `remove` is a node (a comma expression, 1A.35a part B): a
    /// call, a ternary, the short-circuit forms and a hoisted `if` over it print the call as a
    /// member's object with no brackets of its own.
    #[test]
    fn a_set_remove_is_a_node_in_every_parent() {
        let src = "commons rmnode\n\
            fn risky(n: Int) -> Result[Int, String] { Ok(n) }\n\
            fn g(b: Bool) -> Bool { b }\n\
            fn call(a: Set[String], c: String) -> Bool { g(a.remove(c).contains(c)) }\n\
            fn tern(c: Bool, x: Set[String], y: String) -> Bool { if c { x.remove(y).contains(y) } else { false } }\n\
            fn and(s: Set[String], u: String, k: Bool) -> Bool { k && s.remove(u).contains(u) }\n\
            fn hoist(c: Bool, x: Set[String], y: String, r: Result[Bool, String]) -> Result[Bool, String] {\n\
              let v = if c { r? } else { x.remove(y).contains(y) }\n\
              Ok(v)\n\
            }\n";
        let ts = crate::testkit::emit_source(src);
        let has = |set: &str, key: &str| format!("{}.has({key})", removed(set, key));
        assert_eq!(returned(&ts, "call"), format!("g({})", has("a", "c")));
        assert_eq!(
            returned(&ts, "tern"),
            format!("(c ? {} : false)", has("x", "y"))
        );
        assert_eq!(returned(&ts, "and"), format!("k && {}", has("s", "u")));
        assert!(
            ts.contains(&format!("    __r1 = {};\n", has("x", "y"))),
            "{ts}"
        );
    }

    fn lit_text(l: Lowered) -> String {
        assert!(l.pre.is_empty(), "a literal hoists nothing");
        l.expr.printed()
    }

    /// 1A.20, Decision G: a delegated kernel's `Lowered` passes through `Pre::absorb_built` as a
    /// node, a declined kernel as `None`, and the hoisted statements land in the `Pre`.
    #[test]
    fn absorb_built_passes_a_kernel_node_through() {
        let mut pre = Pre::new();
        let stub = Lowered {
            pre: vec![bynk_ts::TsStmt::expr_stmt(ident("t"), None)],
            expr: ident("x"),
        };
        match pre.absorb_built(Some(stub)) {
            Some(bynk_ts::TsExpr::Ident(name)) => assert_eq!(name, "x"),
            other => panic!("expected a node, got {other:?}"),
        }
        assert_eq!(stmts_text(&pre), ["t;"]);
        assert!(pre.absorb_built(None).is_none());
        assert_eq!(stmts_text(&pre).len(), 1, "declining hoists nothing");
    }

    #[test]
    fn numeric_literal_nodes_print_their_digits_verbatim() {
        for digits in [
            "0",
            "42",
            "170141183460469231731687303715884105727",
            "1e10",
            "1.50",
        ] {
            assert_eq!(lit_text(Lowered::node(num_lit(digits.to_string()))), digits);
        }
    }

    #[test]
    fn decimal_literal_node_matches_the_text_form() {
        for (coef, scale) in [
            ("5", 2),
            ("-5", 2),
            ("007", 0),
            ("-0", 3),
            ("12345678901234567890", 18),
        ] {
            assert_eq!(
                lit_text(Lowered::node(decimal_lit(coef, scale))),
                format!(
                    "{{ coefficient: {}n, scale: {scale} }}",
                    decimal_signed_coef(coef)
                ),
                "{coef}/{scale}"
            );
        }
    }

    #[test]
    fn string_literal_node_matches_the_hand_escaped_text() {
        for s in [
            "",
            "plain",
            "say \"hi\"",
            "a\\b",
            "line\nbreak\ttab\r",
            "nul\0",
            "ls\u{2028}ps\u{2029}",
            "`tick` ${x}",
            "unicode ·",
        ] {
            let node = bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(s.to_string()));
            assert_eq!(
                lit_text(Lowered::node(node)),
                format!("\"{}\"", escape_ts_string(s)),
                "{s:?}"
            );
        }
    }

    #[test]
    fn keyword_literal_nodes_print_as_before() {
        let t = bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true));
        let f = bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(false));
        assert_eq!(lit_text(Lowered::node(t)), "true");
        assert_eq!(lit_text(Lowered::node(f)), "false");
    }

    const LITERALS: &str = r#"
commons lits

fn int() -> Int { 42 }
fn big() -> Int { 9007199254740991 }
fn float() -> Float { 1.5e3 }
fn dec() -> Decimal { 1.20d }
fn dur() -> Duration { 5.minutes }
fn text() -> String { "a\"b\nc" }
fn yes() -> Bool { true }
fn no() -> Bool { false }
fn unit() -> () { () }
fn none() -> Option[Int] { None }
fn neg() -> Int { -5 }
fn negd() -> Decimal { -1.20d }
fn minus() -> Float { 2.5 - 1.5e3 }
fn minus_neg() -> Int { 5 - -3 }
"#;

    /// The body of the emitted function `name`: the text of its `return`.
    fn returned(ts: &str, name: &str) -> String {
        let head = format!("export function {name}(");
        let from = ts
            .find(&head)
            .unwrap_or_else(|| panic!("no {name} in:\n{ts}"));
        let rest = &ts[from..];
        let ret = rest.find("return ").expect("a return") + "return ".len();
        let end = rest[ret..].find(";\n").expect("a terminator");
        rest[ret..ret + end].to_string()
    }

    #[test]
    fn each_literal_kind_lowers_to_its_emitted_text() {
        let ts = crate::testkit::emit_source(LITERALS);
        for (name, want) in [
            ("int", "42"),
            ("big", "9007199254740991"),
            ("float", "1.5e3"),
            ("dec", "{ coefficient: 120n, scale: 2 }"),
            ("dur", "300000"),
            ("text", "\"a\\\"b\\nc\""),
            ("yes", "true"),
            ("no", "false"),
            ("unit", "undefined"),
            ("none", "None"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
    }

    #[test]
    fn a_negative_literal_in_operand_position_lowers_as_before() {
        // `UnaryOp::Neg` splices its operand's printed text after `-`, and a
        // binary operator splices a node child between its own spaces.
        let ts = crate::testkit::emit_source(LITERALS);
        for (name, want) in [
            ("neg", "-5"),
            ("negd", "__bynkDecimalNeg({ coefficient: 120n, scale: 2 })"),
            ("minus", "2.5 - 1.5e3"),
            ("minus_neg", "5 - -3"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
    }

    fn bin(op: bynk_ts::TsBinaryOp, l: &str, r: &str) -> bynk_ts::TsExpr {
        bynk_ts::TsExpr::Binary {
            op,
            left: Box::new(ident(l)),
            right: Box::new(ident(r)),
        }
    }

    fn node(e: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
        e
    }

    fn t_unary_op(op: UnaryOp, c: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
        super::unary_op(op, c)
    }

    fn t_paren(c: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
        super::paren(c)
    }

    fn t_runtime_ctor_call(
        object: Option<&'static str>,
        name: &'static str,
        arg: bynk_ts::TsExpr,
    ) -> bynk_ts::TsExpr {
        super::runtime_ctor_call(object, name, arg)
    }

    #[test]
    fn unary_operators_take_their_brackets_from_the_printer() {
        use bynk_ts::TsBinaryOp::{Add, And};
        // `-(a - b)`: the source's explicit `Paren` is kept as a node.
        let sub = t_paren(node(bin(Add, "a", "b")));
        let neg = t_unary_op(UnaryOp::Neg, sub);
        assert_eq!(neg.printed(), "-(a + b)");
        // `!(x && y)`.
        let not = t_unary_op(UnaryOp::Not, t_paren(node(bin(And, "x", "y"))));
        assert_eq!(not.printed(), "!(x && y)");
        // A bare binary operand (no source `Paren`) is bracketed by the printer.
        let bare = t_unary_op(UnaryOp::Not, node(bin(And, "x", "y")));
        assert_eq!(bare.printed(), "!(x && y)");
        let bare = t_unary_op(UnaryOp::Neg, node(bin(Add, "a", "b")));
        assert_eq!(bare.printed(), "-(a + b)");
        // Nested unaries: `!!x`, `-!x`, and `-(-x)`, never the decrement `--x`.
        let not_not = t_unary_op(UnaryOp::Not, t_unary_op(UnaryOp::Not, node(ident("x"))));
        assert_eq!(not_not.printed(), "!!x");
        let neg_not = t_unary_op(UnaryOp::Neg, t_unary_op(UnaryOp::Not, node(ident("x"))));
        assert_eq!(neg_not.printed(), "-!x");
        let neg_neg = t_unary_op(UnaryOp::Neg, t_unary_op(UnaryOp::Neg, node(ident("x"))));
        assert_eq!(neg_neg.printed(), "-(-x)");
    }

    #[test]
    fn a_negated_decimal_is_a_call_over_a_genuine_identifier() {
        let arg = node(decimal_lit("120", 2));
        let neg = t_runtime_ctor_call(None, "__bynkDecimalNeg", arg);
        let bynk_ts::TsExpr::Call { callee, .. } = &neg else {
            panic!("a node child gives a call node");
        };
        assert!(matches!(&**callee, bynk_ts::TsExpr::Ident(n) if n == "__bynkDecimalNeg"));
        assert_eq!(
            neg.printed(),
            "__bynkDecimalNeg({ coefficient: 120n, scale: 2 })"
        );
    }

    #[test]
    fn constructor_calls_and_lists_are_nodes_over_node_children() {
        let ok = t_runtime_ctor_call(None, "Ok", node(num_lit("1")));
        assert_eq!(ok.printed(), "Ok(1)");
        let http = t_runtime_ctor_call(Some("HttpResult"), "Ok", node(ident("v")));
        let bynk_ts::TsExpr::Call { callee, .. } = &http else {
            panic!("expected a call node");
        };
        assert!(matches!(**callee, bynk_ts::TsExpr::Member { .. }));
        assert_eq!(http.printed(), "HttpResult.Ok(v)");
        assert_eq!(
            t_runtime_ctor_call(None, "Err", node(ident("e"))).printed(),
            "Err(e)"
        );
        assert_eq!(
            t_runtime_ctor_call(None, "Some", node(ident("s"))).printed(),
            "Some(s)"
        );
        let list = array(vec![node(ident("a")), node(num_lit("2"))]);
        assert_eq!(list.printed(), "[a, 2]");
    }

    #[test]
    fn wrappers_over_source_lower_as_before() {
        let src = "commons wraps\n\
                   fn f(a: Int, b: Int) -> Int { -(a - b) }\n\
                   fn g(ok: Bool) -> Bool { !(ok && ok) }\n\
                   fn h() -> Result[Int, String] { Ok(1) }\n\
                   fn i() -> List[Int] { [1, 2, 3] }\n\
                   fn j() -> Option[Int] { Some(2) }\n\
                   fn k() -> Decimal { -1.5d }\n\
                   fn m() -> Int { - -1 }\n\
                   fn n(x: Int) -> Int { - -x }\n\
                   fn o(x: Int) -> Int { - - -x }\n";
        let ts = crate::testkit::emit_source(src);
        for (name, want) in [
            ("f", "-(a - b)"),
            ("g", "!(ok && ok)"),
            ("h", "Ok(1)"),
            ("i", "[1, 2, 3]"),
            ("j", "Some(2)"),
            ("k", "__bynkDecimalNeg({ coefficient: 15n, scale: 1 })"),
            // A negated negation is `-(-x)`, never the decrement `--x`.
            ("m", "-(-1)"),
            ("n", "-(-x)"),
            ("o", "-(-(-x))"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
    }

    #[test]
    fn a_negated_negation_never_lowers_to_a_decrement() {
        // `- -x` must not splice to `--x` (a TS pre-decrement) nor `- -5` to
        // `--5` (a TS syntax error): the operand is bracketed, `-(-x)`.
        let ts = crate::testkit::emit_source(
            r#"
commons negneg

fn twice(x: Int) -> Int { - -x }
fn lit() -> Int { - -5 }
fn triple(x: Int) -> Int { - - -x }
fn paren(x: Int) -> Int { -(-x) }
fn minus(x: Int) -> Int { 5 - - -x }
"#,
        );
        for (name, want) in [
            ("twice", "-(-x)"),
            ("lit", "-(-5)"),
            ("triple", "-(-(-x))"),
            ("paren", "-(-x)"),
            ("minus", "5 - -(-x)"),
        ] {
            let got = returned(&ts, name);
            assert_eq!(got, want, "{name}");
            assert!(!got.contains("--"), "{name}: `--` in {got}");
        }
    }

    #[test]
    fn identifiers_and_wrappers_over_them_lower_to_nodes() {
        // 1A.4 (#3701): an identifier is a node, so `Some(x)`, `-x` and
        // `[x, y]` over identifiers are nodes too (before, each was text
        // because its child was). No expression below leaves as text.
        let src = "commons idnodes\n\
                   fn a(x: Int) -> Option[Int] { Some(x) }\n\
                   fn b(x: Int) -> Int { -x }\n\
                   fn c(x: Int, y: Int) -> List[Int] { [x, y] }\n\
                   fn d(x: Int) -> Int { - -x }\n\
                   fn e(ok: Bool) -> Bool { !ok }\n";
        let ts = crate::testkit::emit_source(src);
        for (name, want) in [
            ("a", "Some(x)"),
            ("b", "-x"),
            ("c", "[x, y]"),
            // Now on the node path, a negated negation still brackets.
            ("d", "-(-x)"),
            ("e", "!ok"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
    }

    #[test]
    fn field_access_and_constant_reads_over_identifiers_lower_to_nodes() {
        let head = "commons fieldnodes\n\
                    type P = { name: String, age: Int }\n\
                    const K: Int = 4\n";
        let fields = "fn a(p: P) -> String { p.name }\n\
                      fn c(p: P) -> Int { p.age }\n";
        let src = format!("{head}{fields}fn b() -> Int {{ K }}\n");
        let ts = crate::testkit::emit_source(&src);
        for (name, want) in [("a", "p.name"), ("b", "K()"), ("c", "p.age")] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
    }

    #[test]
    fn a_nullary_variant_is_a_node_over_genuine_names() {
        let src = "commons variants\n\
                   type Colour = | Red | Green\n\
                   fn a() -> Colour { Red }\n";
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "a"), "Colour.Red");
    }

    #[test]
    fn an_http_result_variant_read_is_a_node() {
        // 1A.4's `HttpResult.X` read.
        let src = "commons hr\n\
                   fn f() -> HttpResult[Int] { HttpResult.NotFound }\n";
        assert_eq!(
            returned(&crate::testkit::emit_source(src), "f"),
            "HttpResult.NotFound"
        );
    }

    // ---- 1A.5 (#3703): value binary operators -------------------------------

    fn t_plain_binary(op: BinOp, l: bynk_ts::TsExpr, r: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
        super::plain_binary(op, vec![l, r])
    }

    fn t_helper_call(
        helper: &'static str,
        l: bynk_ts::TsExpr,
        r: bynk_ts::TsExpr,
    ) -> bynk_ts::TsExpr {
        super::helper_call(helper, vec![l, r])
    }

    #[test]
    fn each_value_operator_shape_is_a_node_over_node_children() {
        let n = |s: &str| node(ident(s));
        // Plain operator and `Float /`: `Binary`.
        for (op, want) in [
            (BinOp::Add, "a + b"),
            (BinOp::Sub, "a - b"),
            (BinOp::Mul, "a * b"),
            (BinOp::Div, "a / b"),
            (BinOp::Eq, "a === b"),
            (BinOp::NotEq, "a !== b"),
            (BinOp::Lt, "a < b"),
            (BinOp::LtEq, "a <= b"),
            (BinOp::Gt, "a > b"),
            (BinOp::GtEq, "a >= b"),
        ] {
            let got = t_plain_binary(op, n("a"), n("b"));
            assert!(matches!(&got, bynk_ts::TsExpr::Binary { .. }), "{want}");
            assert_eq!(got.printed(), want);
        }
        // Decimal, `Int /` and structural `==`: a call over a genuine identifier.
        let call = t_helper_call("__bynkDecimalAdd", n("a"), n("b"));
        let bynk_ts::TsExpr::Call { callee, args } = &call else {
            panic!("a node pair gives a call node");
        };
        assert!(matches!(&**callee, bynk_ts::TsExpr::Ident(h) if h == "__bynkDecimalAdd"));
        assert_eq!(args.len(), 2);
        assert_eq!(call.printed(), "__bynkDecimalAdd(a, b)");
        // Structural `!=`: `Unary(Not, Call)`.
        let ne = crate::ts_build::not_expr(super::helper_call("__bynkEq", vec![n("a"), n("b")]));
        assert!(matches!(
            &ne,
            bynk_ts::TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Not,
                ..
            }
        ));
        assert_eq!(ne.printed(), "!__bynkEq(a, b)");
        // `Ord`: `cmp(l, r) < 0`.
        let ord = super::ordered_comparison(BinOp::Lt, ident("__cmp_P"), vec![n("a"), n("b")]);
        assert!(matches!(&ord, bynk_ts::TsExpr::Binary { .. }));
        assert_eq!(ord.printed(), "__cmp_P(a, b) < 0");
    }

    #[test]
    fn a_composite_comparator_is_a_node_over_nodes() {
        // 1A.16: the comparator is itself a node, so a composite one is an
        // ordinary callee (the 1A.5 text path is closed).
        let cmp = bynk_ts::TsExpr::Call {
            callee: Box::new(ident("__bynkCmpList")),
            args: vec![ident("__bynkCmpInt")],
        };
        let got =
            super::ordered_comparison(BinOp::Lt, cmp, vec![node(ident("a")), node(ident("b"))]);
        assert_eq!(got.printed(), "__bynkCmpList(__bynkCmpInt)(a, b) < 0");
    }

    #[test]
    fn comparison_over_a_derived_ord_record_lowers_to_nodes() {
        // 1A.5's composite-comparator fallback, closed by 1A.16.
        let src = "commons ordrec\n\
            type P = { a: Int, b: String } derives Ord\n\
            fn lt(x: P, y: P) -> Bool { x < y }\n\
            fn lts(x: List[Int], y: List[Int]) -> Bool { x < y }\n";
        let ts = crate::testkit::emit_source(src);
        assert!(
            returned(&ts, "lt").contains("(x, y) < 0"),
            "{}",
            returned(&ts, "lt")
        );
        assert_eq!(
            returned(&ts, "lts"),
            "__bynkCmpList(__bynkCmpInt)(x, y) < 0"
        );
    }

    #[test]
    fn binary_nodes_keep_their_brackets_inside_text_parents() {
        // Source brackets are `Paren` nodes, so a printed operator node spliced
        // into an unconverted text parent (a method-call receiver, a unary
        // operand) keeps them end to end.
        let ts = crate::testkit::emit_source(
            "commons parents\n\
             fn f(a: Int, b: Int) -> String { (a + b).toString() }\n\
             fn g(a: Int, b: Int, c: Int) -> Int { (a - b) * c }\n\
             fn h(a: Int, b: Int) -> Int { -(a + b) }\n",
        );
        assert_eq!(returned(&ts, "g"), "(a - b) * c");
        assert_eq!(returned(&ts, "h"), "-(a + b)");
        assert!(
            returned(&ts, "f").contains("(a + b)"),
            "{}",
            returned(&ts, "f")
        );
    }

    #[test]
    fn value_operators_over_identifiers_print_by_the_precedence_table() {
        let src = "commons prec\n\
                   fn a(a: Int, b: Int, c: Int) -> Int { a - b - c }\n\
                   fn b(a: Int, b: Int, c: Int) -> Int { a - (b - c) }\n\
                   fn c(a: Float, b: Float, c: Float) -> Float { a / b * c }\n\
                   fn d(a: Int, b: Int, c: Int) -> Int { a * (b + c) }\n\
                   fn e(a: Int, b: Int, c: Int, d: Int) -> Bool { a < b == c < d }\n\
                   fn f(a: Int, b: Int) -> Bool { !(a == b) }\n\
                   fn g(a: Int, b: Int) -> Int { -(a * b) }\n\
                   fn h(a: Int, b: Int, c: Int) -> Int { a + b * c }\n\
                   fn i(a: Int, b: Int, c: Int) -> Int { a - b + c }\n";
        let ts = crate::testkit::emit_source(src);
        for (name, want) in [
            ("a", "a - b - c"),
            ("b", "a - (b - c)"),
            ("c", "a / b * c"),
            ("d", "a * (b + c)"),
            ("e", "a < b === c < d"),
            ("f", "!(a === b)"),
            ("g", "-(a * b)"),
            ("h", "a + b * c"),
            ("i", "a - b + c"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
    }

    #[test]
    fn helper_branches_lower_to_nodes_and_still_note_their_imports() {
        let src = "commons helpers\n\
                   type P = { a: Int }\n\
                   fn dec(x: Decimal, y: Decimal) -> Decimal { x + y }\n\
                   fn decs(x: Decimal, y: Decimal) -> Decimal { x - y }\n\
                   fn decm(x: Decimal, y: Decimal) -> Decimal { x * y }\n\
                   fn idiv(x: Int, y: Int) -> Int { x / y }\n\
                   fn fdiv(x: Float, y: Float) -> Float { x / y }\n\
                   fn eq(x: P, y: P) -> Bool { x == y }\n\
                   fn ne(x: P, y: P) -> Bool { x != y }\n\
                   fn ord[T: Ord](x: T, y: T) -> Bool { x < y }\n";
        let ts = crate::testkit::emit_source(src);
        for (name, want) in [
            ("dec", "__bynkDecimalAdd(x, y)"),
            ("decs", "__bynkDecimalSub(x, y)"),
            ("decm", "__bynkDecimalMul(x, y)"),
            ("idiv", "__bynkIntDiv(x, y)"),
            ("fdiv", "x / y"),
            ("eq", "__bynkEq(x, y)"),
            ("ne", "!__bynkEq(x, y)"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
        assert!(
            ts.contains("export function ord<T>(x: T, y: T, __cmp_T: (a: T, b: T) => number): boolean {\n  return __cmp_T(x, y) < 0;\n}"),
            "{ts}"
        );
        for helper in [
            "__bynkDecimalAdd",
            "__bynkDecimalSub",
            "__bynkDecimalMul",
            "__bynkIntDiv",
            "__bynkEq",
        ] {
            assert!(
                ts.contains(&format!("{helper},")) || ts.contains(&format!("{helper} }}")),
                "{helper} is imported:\n{ts}"
            );
        }
        // The text path (a call operand) imports the same helpers.
        let text_src = "commons helpers2\n\
                        fn d() -> Decimal { 1.5d }\n\
                        fn dec(x: Decimal) -> Decimal { x + d() }\n\
                        fn idiv(x: Int) -> Int { x / 2 }\n";
        let ts = crate::testkit::emit_source(text_src);
        assert!(ts.contains("__bynkDecimalAdd"), "{ts}");
        assert!(ts.contains("__bynkIntDiv"), "{ts}");
    }

    #[test]
    fn short_circuit_operators_over_nodes_lower_to_nodes() {
        // 1A.14: `&&`, `||` and `implies` over node operands are nodes; `implies`
        // keeps its explicit brackets.
        let src = "commons shortc\n\
                   fn a(x: Bool, y: Bool) -> Bool { x && y }\n\
                   fn b(x: Bool, y: Bool) -> Bool { x || y }\n\
                   fn c(x: Bool, y: Bool) -> Bool { x implies y }\n\
                   fn d(x: Bool, y: Bool, z: Bool) -> Bool { (x || y) && z }\n";
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "a"), "x && y");
        assert_eq!(returned(&ts, "b"), "x || y");
        assert_eq!(returned(&ts, "c"), "(!(x) || y)");
        assert_eq!(returned(&ts, "d"), "(x || y) && z");
    }

    #[test]
    fn identifier_node_helpers_build_genuine_names() {
        let m = member_node("deps", "who");
        assert!(matches!(&m, bynk_ts::TsExpr::Member { object, property }
            if property == "who" && matches!(&**object, bynk_ts::TsExpr::Ident(n) if n == "deps")));
        assert_eq!(bynk_ts::print_expr(&m), "deps.who");
        assert_eq!(bynk_ts::print_expr(&ident("__bynkRunId")), "__bynkRunId");
    }

    /// An `if (flag) { done(); }` statement node: multi-line when printed.
    fn if_node() -> bynk_ts::TsStmt {
        bynk_ts::TsStmt::if_stmt(
            bynk_ts::TsExpr::Ident("flag".to_string()),
            bynk_ts::TsStmt::block(
                vec![bynk_ts::TsStmt::expr_stmt(
                    bynk_ts::TsExpr::Ident("done".to_string()),
                    None,
                )],
                None,
            ),
            None,
        )
    }

    /// A node statement indents every line of its print, not only the first.
    #[test]
    fn a_node_statement_indents_every_line() {
        let mut pre = Pre::new();
        pre.push_node(if_node());
        assert_eq!(stmts_text(&pre), ["if (flag) {", "  done;", "}"]);
        for indent in [0, 2, 6] {
            let mut n = String::new();
            for s in &stmts_text(&pre) {
                write_line(&mut n, indent, s);
            }
            assert!(
                n.lines().all(|l| l.starts_with(&" ".repeat(indent))),
                "every line carries the caller's indentation:\n{n}"
            );
            assert_eq!(n.lines().count(), 3, "indent {indent}");
        }
    }

    // -- 1A.6 (#3705): calls --

    const CALLS: &str = "commons calls\n\
        type S = | A(n: Int) | B\n\
        type Box2 = { v: Int }\n\
        fn Box2.size(self) -> Int { self.v }\n\
        fn g(x: Int, y: Int) -> Int { x }\n\
        fn plain(x: Int, y: Int) -> Int { g(x, y) }\n\
        fn ctor(x: Int) -> S { A(x) }\n\
        fn ufcs(b: Box2) -> Int { b.size() }\n\
        fn fault() -> Int { unreachable(\"no\\n\\\"way\\\"\") }\n";

    #[test]
    fn calls_over_identifiers_lower_to_nodes_and_still_note_their_imports() {
        let ts = crate::testkit::emit_source(CALLS);
        for (name, want) in [
            ("plain", "g(x, y)"),
            ("ctor", "S.A(x)"),
            ("ufcs", "Box2.size(b)"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
        // The fault call is a node (kind and location are string nodes) and
        // still imports `__fault`.
        let fault = returned(&ts, "fault");
        assert!(
            fault.starts_with("__fault(\"Unreachable\", \"no\\n\\\"way\\\"\", \""),
            "{fault}"
        );
        assert!(ts.contains("__fault"), "{ts}");
    }

    #[test]
    fn call_arguments_keep_their_precedence() {
        let src = "commons callprec\n\
            fn g(x: Int, y: Int) -> Int { x }\n\
            fn sum(a: Int, b: Int) -> Int { g(a + b, a * b) }\n\
            fn neg(a: Int, b: Int) -> Int { g(-a, b - a) }\n\
            fn tern(a: Int, b: Int, c: Bool) -> Int { g(if c { a } else { b }, a) }\n";
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "sum"), "g(a + b, a * b)");
        assert_eq!(returned(&ts, "neg"), "g(-a, b - a)");
        let tern = returned(&ts, "tern");
        assert!(tern.starts_with("g("), "{tern}");
        assert!(tern.ends_with(", a)"), "{tern}");
    }

    #[test]
    fn witness_calls_are_nodes_and_explicit_type_argument_calls_stay_text() {
        // An `Ord`-bounded callee takes comparator witnesses, which are nodes
        // (1A.16 closes 1A.6's witness fallback).
        let witness = "commons witnesses\n\
            fn lt[T: Ord](a: T, b: T) -> Bool { a < b }\n\
            fn use(a: Int, b: Int) -> Bool { lt(a, b) }\n";
        let ts = crate::testkit::emit_source(witness);
        assert_eq!(returned(&ts, "use"), "lt(a, b, __bynkCmpInt)");
        // Explicit type arguments on a generic instance method (#2795): `Call`
        // has no `type_args`, so the call stays text.
        let explicit = "commons explicit\n\
            type Box2[A] = { v: A }\n\
            fn Box2.cast[U](self) -> Option[U] { None }\n\
            fn use(b: Box2[Int]) -> Option[Int] { b.cast[Int]() }\n";
        let ts = crate::testkit::emit_source(explicit);
        assert_eq!(returned(&ts, "use"), "Box2.cast<number, number>(b)");
    }

    // -- 1A.7 (#3706): data literals --

    const DATA: &str = "commons data\n\
        type P = { name: String, age: Int }\n\
        fn mk(n: String, a: Int) -> P { P { name: n, age: a } }\n\
        fn short(name: String, age: Int) -> P { P { name, age } }\n\
        fn spread(p: P, a: Int) -> P { P { ...p, age: a } }\n\
        fn greet(n: String) -> String { \"hi \\(n)!\" }\n\
        fn adjacent(a: Int, b: Int) -> String { \"\\(a)\\(b)\" }\n\
        fn money(d: Decimal) -> String { \"cost \\(d)\" }\n";

    #[test]
    fn data_literals_over_nodes_lower_to_nodes() {
        let ts = crate::testkit::emit_source(DATA);
        for (name, want) in [
            ("mk", "{ name: n, age: a }"),
            ("short", "{ name, age }"),
            ("spread", "{ ...p, age: a }"),
            ("greet", "`hi ${String(n)}!`"),
            ("adjacent", "`${String(a)}${String(b)}`"),
            ("money", "`cost ${__bynkDecimalToString(d)}`"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
        // `note_decimal` fires on the node path.
        assert!(ts.contains("__bynkDecimalToString"), "{ts}");
    }

    #[test]
    fn an_interpolated_string_escapes_its_chunks_exactly() {
        let src = "commons esc\n\
            fn f(n: String) -> String { \"a`b\\(n)c\\\\d $ ${n}\" }\n\
            fn g(n: String) -> String { \"\\(n)\" }\n\
            fn h(n: String) -> String { \"x\\(n)\\n\" }\n";
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "f"), "`a\\`b${String(n)}c\\\\d \\$ \\${n}`");
        assert_eq!(returned(&ts, "g"), "`${String(n)}`");
        assert_eq!(returned(&ts, "h"), "`x${String(n)}\\n`");
    }

    #[test]
    fn a_renamed_shorthand_field_keeps_its_wire_key() {
        // `default` and `class` are TS reserved words: the binding is renamed,
        // the key is not, and it prints unquoted (legal in an object literal).
        let src = "commons renamed\n\
            type R = { default: Int, class: Int }\n\
            fn mk(default: Int, class: Int) -> R { R { default, class } }\n\
            fn sp(r: R, default: Int) -> R { R { ...r, default } }\n";
        let ts = crate::testkit::emit_source(src);
        assert_eq!(
            returned(&ts, "mk"),
            "{ default: __id_default, class: __id_class }"
        );
        assert_eq!(returned(&ts, "sp"), "{ ...r, default: __id_default }");
    }

    #[test]
    fn a_record_that_takes_a_declared_default_lowers_to_a_node() {
        // 1A.31 (#3732): a default is a node, so the whole literal is one (before, it was text).
        let src = "commons defaults\n\
            type C = { n: Int, label: String = \"none\" }\n\
            fn mk(n: Int) -> C { C { n } }\n\
            fn full(n: Int) -> C { C { n, label: \"x\" } }\n";
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "mk"), "{ n, label: \"none\" }");
        assert_eq!(returned(&ts, "full"), "{ n, label: \"x\" }");
    }

    #[test]
    fn defaults_of_every_shape_and_their_neighbours_lower_to_nodes() {
        let src = "commons defaults2\n\
            type Plan = enum { Free, Pro }\n\
            type In = { k: Int = -3 }\n\
            type C = { a: Int, plan: Plan = Free, o: Option[Int] = None, l: List[Int] = [1, 2], \
                m: Map[String, Int] = Map.empty(), i: In = In { }, p: Float = 1e3 }\n\
            fn short(a: Int) -> C { C { a } }\n\
            fn some(a: Int) -> C { C { a, o: Some(a) } }\n\
            fn lam(xs: List[Int]) -> List[C] { xs.map((x) => C { a: x }) }\n";
        let ts = crate::testkit::emit_source(src);
        let tail = "plan: { tag: \"Free\" as const }, o: { tag: \"None\" as const }, l: [1, 2], \
            m: new globalThis.Map(), i: { k: -3 }, p: 1e3 }";
        assert_eq!(returned(&ts, "short"), format!("{{ a, {tail}"));
        assert_eq!(
            returned(&ts, "some"),
            "{ a, o: Some(a), plan: { tag: \"Free\" as const }, \
                l: [1, 2], m: new globalThis.Map(), i: { k: -3 }, p: 1e3 }"
        );
        assert!(ts.contains(&format!("=> ({{ a: x, {tail})")), "{ts}");
    }

    #[test]
    fn a_zero_field_record_prints_as_empty_braces() {
        // Layout-only: the old text was `{  }`, the printer's form is `{}`.
        // No fixture reaches a zero-field record.
        let src = "commons empty\n\
            type E = {}\n\
            fn mk() -> E { E {} }\n";
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "mk"), "{}");
    }

    #[test]
    fn a_record_literal_as_a_lambda_body_keeps_its_brackets() {
        let src = "commons lam\n\
            type P = { v: Int }\n\
            fn f(xs: List[Int]) -> List[P] { xs.map((x) => P { v: x }) }\n";
        let ts = crate::testkit::emit_source(src);
        assert!(ts.contains("=> ({ v: x })"), "{ts}");
    }

    #[test]
    fn expect_location_and_source_print_as_their_escaped_text() {
        // The `Str` nodes carry the raw strings; the printer's escape is the
        // one `escape_ts_string` applies to the text form.
        for raw in [
            "tests/a.test.bynk:3:7",
            "expect a == \"\u{e9}\\n\" `x` ${y}",
        ] {
            let node = str_lit(raw);
            assert_eq!(
                bynk_ts::print_expr(&node),
                format!("\"{}\"", crate::emitter::escape_ts_string(raw))
            );
        }
    }

    #[test]
    fn http_and_queue_result_variant_calls_are_nodes() {
        // 1A.6 review nit: direct node-path assertions for the `HttpResult.V(…)`
        // and `QueueResult.V(…)` call forms.
        let src = "commons results\n\
            fn created(x: Int) -> HttpResult[Int] { Created(x) }\n\
            fn raw(b: Bytes) -> HttpResult[Int] { RawError(Forbidden, b, \"text/plain\") }\n";
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "created"), "HttpResult.Created(x)");
        assert_eq!(
            returned(&ts, "raw"),
            "HttpResult.RawError(403, b, \"text/plain\")"
        );
    }

    #[test]
    fn a_question_over_a_node_operand_hoists_statement_nodes() {
        let src = "commons qnodes\n\
            fn f(x: Result[Int, String]) -> Result[Int, String] { Ok(x? + 1) }\n\
            fn g(x: Result[Int, String]) -> Result[Int, String] { Ok(x?) }\n";
        let ts = crate::testkit::emit_source(src);
        assert!(
            ts.contains(
                "  const __r0 = x;\n  if (__r0.tag === \"Err\") return __r0;\n  return Ok(__r0.value + 1);\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_question_over_an_option_in_an_http_result_function_exits_with_not_found() {
        // 1A.8 review nit: the `NotFound` exit, driven from source. Both
        // statements are nodes, and the guard prints as today's text does.
        let src = "commons nf\n\
            fn lookup(i: Int) -> Option[Int] { if i > 0 { Some(i) } else { None } }\n\
            fn f(i: Int) -> HttpResult[Int] { Ok(lookup(i)?) }\n";
        let ts = crate::testkit::emit_source(src);
        assert!(
            ts.contains(
                "  const __r0 = lookup(i);\n  if (__r0.tag === \"None\") return HttpResult.NotFound;\n  return HttpResult.Ok(__r0.value);\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_question_that_embeds_its_error_wraps_it_in_the_declared_variant() {
        // 1A.8 review nit: the `Embed` exit (`embeds E as V`), driven from source.
        let src = "commons emb\n\
            type IoError = enum { Disk }\n\
            type AppError =\n  | Io(reason: IoError)\n  | Other\n  embeds IoError as Io\n\
            fn g(r: Result[Int, IoError]) -> Result[Int, AppError] { Ok(r? + 1) }\n";
        let ts = crate::testkit::emit_source(src);
        assert!(
            ts.contains(
                "  const __r0 = r;\n  if (__r0.tag === \"Err\") return Err(AppError.Io(__r0.error));\n  return Ok(__r0.value + 1);\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_question_over_a_value_position_match_is_a_node() {
        // 1A.15: a `match` in value position is a node (its IIFE) when its arm
        // tails are, so a `?` over one is a node as well: no statement stays a
        // text line, and the guard is the same bytes.
        let src = "commons qtext\n\
            fn f(x: Result[Int, String], n: Int) -> Result[Int, String] {\n\
              Ok((match n { 0 => x, _ => x })?)\n\
            }\n";
        let ts = crate::testkit::emit_source(src);
        assert!(
            ts.contains("  const __r0 = (((__d) => {")
                && ts.contains(
                    "\n  if (__r0.tag === \"Err\") return __r0;\n  return Ok(__r0.value);\n"
                ),
            "{ts}"
        );
    }

    #[test]
    fn statement_nodes_re_indent_at_depth_through_both_consumers() {
        // 1A.8, Decision C: a multi-line node (the hoisted `if`) printed at
        // depth 0 by `lines()` and re-indented by the caller must equal the
        // flat text lines it replaces; `ts_stmts` keeps it as a node.
        let node = bynk_ts::TsStmt::if_else_same_line_stmt(
            ident("c"),
            bynk_ts::TsStmt::block(
                vec![bynk_ts::TsStmt::assign(ident("__r1"), ident("a"), None)],
                None,
            ),
            bynk_ts::TsStmt::block(
                vec![bynk_ts::TsStmt::assign(ident("__r1"), ident("b"), None)],
                None,
            ),
            None,
        );
        let mut pre = Pre::new();
        pre.push_node(node);
        let text_lines = [
            "if (c) {".to_string(),
            "  __r1 = a;".to_string(),
            "} else {".to_string(),
            "  __r1 = b;".to_string(),
            "}".to_string(),
        ];
        assert_eq!(stmts_text(&pre), text_lines);
        // Written two levels (four columns) deep, as `write_line` indents each entry.
        let mut out = String::new();
        for l in stmts_text(&pre) {
            write_line(&mut out, 4, &l);
        }
        assert_eq!(
            out,
            "    if (c) {\n      __r1 = a;\n    } else {\n      __r1 = b;\n    }\n"
        );
        // `ts_stmts` keeps the node; the printer at depth 2 prints the same bytes.
        let stmts = pre.ts_stmts();
        assert_eq!(stmts.len(), 1);
        assert_eq!(bynk_ts::print_stmt(&stmts[0], 2), out);
    }

    #[test]
    fn a_hoisted_if_over_nodes_is_built_from_nodes() {
        let src = "commons ifnodes\n\
            fn f(c: Bool, x: Result[Int, String]) -> Result[Int, String] {\n\
              let v = if c { x? } else { 0 }\n\
              Ok(v)\n\
            }\n";
        let ts = crate::testkit::emit_source(src);
        assert!(
            ts.contains(
                "  let __r1: number;\n  if (c) {\n    const __r0 = x;\n    if (__r0.tag === \"Err\") return __r0;\n    __r1 = __r0.value;\n  } else {\n    __r1 = 0;\n  }\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_question_nested_two_deep_in_hoisted_ifs_indents_per_level() {
        // #1750 / T2.1: the multi-line node is re-indented by `lines()` into a
        // handler body at depth, one printer level per nested `if`.
        let src = "commons ifnest\n\
            fn f(a: Bool, b: Bool, x: Result[Int, String]) -> Result[Int, String] {\n\
              let v = if a { if b { x? } else { 1 } } else { 2 }\n\
              Ok(v)\n\
            }\n";
        let ts = crate::testkit::emit_source(src);
        assert!(
            ts.contains("  let __r2: number;\n  if (a) {\n    let __r1: number;\n    if (b) {\n"),
            "{ts}"
        );
        assert!(
            ts.contains(
                "    if (b) {\n      const __r0 = x;\n      if (__r0.tag === \"Err\") return __r0;\n      __r1 = __r0.value;\n    } else {\n      __r1 = 1;\n    }\n    __r2 = __r1;\n  } else {\n    __r2 = 2;\n  }\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_ternary_over_nodes_is_a_paren_conditional() {
        let src = "commons tern\n\
            fn f(c: Bool, x: Int, y: Int) -> Int { if c { x } else { y } }\n";
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "f"), "(c ? x : y)");
    }

    /// The function `name` as emitted, from its header to its closing brace.
    fn func(ts: &str, name: &str) -> String {
        let head = format!("export function {name}(");
        let at = ts.find(&head).unwrap_or_else(|| panic!("no {name}:\n{ts}"));
        let rest = &ts[at..];
        rest[..rest.find("\n}\n").expect("a closing brace") + 3].to_string()
    }

    const IS_HDR: &str = "commons is1\n\ntype Shape = | Circle(r: Int) | Square(w: Int) | Dot\nfn mk(s: Shape) -> Shape { s }\nfn bump(n: Int) -> Int { n }\nfn risky(n: Int) -> Result[Int, String] { Ok(n) }\n";

    /// 1A.14: `is` over every pattern kind is a node, with the identity-arrow
    /// wrapper (#2836) on a tag test and none on a wildcard.
    #[test]
    fn is_over_every_pattern_kind_lowers_to_nodes() {
        let src = format!(
            "{IS_HDR}fn v(s: Shape) -> Bool {{ s is Circle(_) }}\n\
             fn n(s: Option[Shape]) -> Bool {{ s is Some(Circle(_)) }}\n\
             fn o(s: Shape) -> Bool {{ s is Circle(_) | Square(_) }}\n\
             fn w(s: Shape) -> Bool {{ s is _ }}\n\
             fn l(xs: List[Int]) -> Bool {{ xs is [1, ..] }}\n"
        );
        let ts = crate::testkit::emit_source(&src);
        assert_eq!(
            returned(&ts, "v"),
            "((__c: boolean) => __c)(s.tag === \"Circle\")"
        );
        assert_eq!(
            returned(&ts, "n"),
            "((__c: boolean) => __c)(s.tag === \"Some\" && s.value.tag === \"Circle\")"
        );
        assert_eq!(
            returned(&ts, "o"),
            "((__c: boolean) => __c)((s.tag === \"Circle\" || s.tag === \"Square\"))"
        );
        assert_eq!(returned(&ts, "w"), "true");
        assert_eq!(returned(&ts, "l"), "xs.length >= 1 && xs[0] === 1");
    }

    /// A complex receiver is evaluated once into a temp, a `const` statement
    /// node; inside a hoisting right operand it is a hoisted declaration and an
    /// in-place assignment (#1654), the assignment a node.
    #[test]
    fn an_is_receiver_temp_is_a_const_node_or_an_in_place_assignment() {
        let src = format!(
            "{IS_HDR}fn a(s: Shape, k: Int) -> Bool {{ mk(s) is Dot && k > 0 }}\n\
             fn b(s: Shape, k: Int) -> Bool {{ k > 0 && mk(s) is Dot }}\n"
        );
        let ts = crate::testkit::emit_source(&src);
        // The hoisted `let __r0!: Shape;` declaration is a definite-assignment `let` node
        // (1A.35a part B), so no statement is text.
        assert_eq!(
            func(&ts, "a"),
            "export function a(s: Shape, k: number): boolean {\n  const __r0 = mk(s);\n  return ((__c: boolean) => __c)(__r0.tag === \"Dot\") && k > 0;\n}\n"
        );
        assert_eq!(
            func(&ts, "b"),
            "export function b(s: Shape, k: number): boolean {\n  let __r0!: Shape;\n  return k > 0 && (() => { __r0 = mk(s); return ((__c: boolean) => __c)(__r0.tag === \"Dot\"); })();\n}\n"
        );
    }

    /// `&&` and `implies` with an `is` antecedent, the three cases: no
    /// bindings, the binding IIFE, and a right operand holding a `?` (hoisted
    /// as an `if`, the antecedent kept in the returned expression for `tsc`'s
    /// narrowing, #1044).
    #[test]
    fn and_and_implies_with_is_bindings_cover_their_three_cases() {
        let src = format!(
            "{IS_HDR}fn none(s: Shape, k: Bool) -> Bool {{ s is Dot && k }}\n\
             fn iife(xs: List[Int], k: Int) -> Bool {{ xs is [x] && x > k }}\n\
             fn imp(xs: List[Int], k: Int) -> Bool {{ xs is [x, ..rest] implies x > k }}\n\
             fn ret(xs: List[Int], k: Int) -> Result[Bool, String] {{ Ok(xs is [x, ..] && risky(x)? > k) }}\n\
             fn retimp(xs: List[Int], k: Int) -> Result[Bool, String] {{ Ok(xs is [x, ..] implies risky(x)? > k) }}\n"
        );
        let ts = crate::testkit::emit_source(&src);
        assert_eq!(
            returned(&ts, "none"),
            "((__c: boolean) => __c)(s.tag === \"Dot\") && k"
        );
        assert_eq!(
            returned(&ts, "iife"),
            "xs.length === 1 && ((() => { const x = xs[0]; return x > k; })())"
        );
        assert_eq!(
            returned(&ts, "imp"),
            "(!(xs.length >= 1) || ((() => { const x = xs[0]; const rest = xs.slice(1); return x > k; })()))"
        );
        assert_eq!(
            func(&ts, "ret"),
            "export function ret(xs: readonly number[], k: number): Result<boolean, string> {\n  let __r1: boolean;\n  if (xs.length >= 1) {\n    const x = xs[0];\n    const __r0 = risky(x);\n    if (__r0.tag === \"Err\") return __r0;\n    __r1 = __r0.value > k;\n  } else {\n    __r1 = false;\n  }\n  return Ok(xs.length >= 1 && __r1);\n}\n"
        );
        assert_eq!(
            func(&ts, "retimp"),
            "export function retimp(xs: readonly number[], k: number): Result<boolean, string> {\n  let __r1: boolean;\n  if (xs.length >= 1) {\n    const x = xs[0];\n    const __r0 = risky(x);\n    if (__r0.tag === \"Err\") return __r0;\n    __r1 = __r0.value > k;\n  } else {\n    __r1 = true;\n  }\n  return Ok((!(xs.length >= 1) || __r1));\n}\n"
        );
    }

    /// A variant binding reads its payload through the `Extract` cast, now a node
    /// (`typeof` under a generic type), so the IIFE is a node and prints as it did; the antecedent stays in the hoisted form for narrowing
    /// (#1044).
    #[test]
    fn a_variant_payload_binding_is_a_node_through_the_extract_cast() {
        let src = format!(
            "{IS_HDR}fn g(s: Shape, k: Int) -> Bool {{ s is Circle(r) && r > k }}\n\
             fn h(s: Shape, k: Int) -> Result[Bool, String] {{ Ok(s is Circle(r) && risky(r)? > k) }}\n"
        );
        let ts = crate::testkit::emit_source(&src);
        assert_eq!(
            returned(&ts, "g"),
            "((__c: boolean) => __c)(s.tag === \"Circle\") && ((() => { const r = (s as Extract<typeof s, { tag: \"Circle\" }>).r; return r > k; })())"
        );
        assert_eq!(
            func(&ts, "h"),
            "export function h(s: Shape, k: number): Result<boolean, string> {\n  let __r1: boolean;\n  if (((__c: boolean) => __c)(s.tag === \"Circle\")) {\n    const r = (s as Extract<typeof s, { tag: \"Circle\" }>).r;\n    const __r0 = risky(r);\n    if (__r0.tag === \"Err\") return __r0;\n    __r1 = __r0.value > k;\n  } else {\n    __r1 = false;\n  }\n  return Ok(((__c: boolean) => __c)(s.tag === \"Circle\") && __r1);\n}\n"
        );
    }

    /// Plain `&&`, `||` and `implies` with a hoisting right operand: without an
    /// early return the right operand sits in a compact arrow so its hoist runs
    /// only when reached; with one it is a hoisted `if`, `false` / `true` in the
    /// other arm.
    #[test]
    fn plain_short_circuit_with_a_hoisting_rhs_is_an_arrow_or_a_hoisted_if() {
        let src = format!(
            "{IS_HDR}fn a(x: Int, s: Shape) -> Bool {{ x > 0 && mk(s) is Dot }}\n\
             fn b(x: Int, s: Shape) -> Bool {{ x > 0 || mk(s) is Dot }}\n\
             fn c(x: Int, s: Shape) -> Bool {{ x > 0 implies mk(s) is Dot }}\n\
             fn d(x: Int) -> Result[Bool, String] {{ Ok(x > 0 && risky(x)? > 0) }}\n\
             fn e(x: Int) -> Result[Bool, String] {{ Ok(x > 0 || risky(x)? > 0) }}\n\
             fn f(x: Int) -> Result[Bool, String] {{ Ok(x > 0 implies risky(x)? > 0) }}\n"
        );
        let ts = crate::testkit::emit_source(&src);
        let arrow =
            "(() => { __r0 = mk(s); return ((__c: boolean) => __c)(__r0.tag === \"Dot\"); })()";
        assert_eq!(returned(&ts, "a"), format!("x > 0 && {arrow}"));
        assert_eq!(returned(&ts, "b"), format!("x > 0 || {arrow}"));
        assert_eq!(returned(&ts, "c"), format!("(!(x > 0) || {arrow})"));
        let hoisted = |cond: &str, then: &str, other: &str| {
            format!(
                "  let __r1: boolean;\n  if ({cond}) {{\n{then}  }} else {{\n{other}  }}\n  return Ok(__r1);\n}}\n"
            )
        };
        let reached = "    const __r0 = risky(x);\n    if (__r0.tag === \"Err\") return __r0;\n    __r1 = __r0.value > 0;\n";
        let skipped_true = "    __r1 = true;\n";
        let skipped_false = "    __r1 = false;\n";
        assert!(
            func(&ts, "d").ends_with(&hoisted("x > 0", reached, skipped_false)),
            "{ts}"
        );
        assert!(
            func(&ts, "e").ends_with(&hoisted("x > 0", skipped_true, reached)),
            "{ts}"
        );
        assert!(
            func(&ts, "f").ends_with(&hoisted("!(x > 0)", skipped_true, reached)),
            "{ts}"
        );
    }
}
