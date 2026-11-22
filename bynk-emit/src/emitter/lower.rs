//! Expression and statement lowering — the `LowerCtx`-driven engine that
//! turns Bynk expressions, statements, matches, and mocks into TypeScript
//! source. Split out of `emitter.rs` (ADR 0060); `LowerCtx` and the `ts_*`
//! type renderers stay in the parent and are reached via `use super::*`.

use std::sync::Arc;

use bynk_check::actors::CallSitePrincipal;
use bynk_check::checker::{Callee, NamedKind, Ty, TyId};
use bynk_check::kernel_op::{IntrinsicNs, StaticOp};
use bynk_syntax::ast::{
    BaseType, BinOp, Block, ChainLink, Expr, ExprKind, FaultsExpr, FieldInit, Ident, InterpPart,
    LambdaExpr, LiteralValue, MatchArm, ObservationExpr, ObservationMatcher, Pattern,
    PatternBindingKind, PredKind, RefinableLiteral, Refinement, TypeBody, TypeDecl, TypeRef,
    UnaryOp,
};

use bynk_model::{ConstVal, EventPatternIr, EventPatternValueIr};

use super::default_value::{Slot, default_value_node};
use super::internal_fault::{FaultSite, undefined_node, unnameable_receiver_type_arg};
use super::lower_call::agents::{
    lower_agent_init, lower_inline_agent_call, lower_let_bound_agent_call,
};
use super::lower_call::cross_context::lower_cross_context_service_call;
use super::lower_call::kernel::lower_kernel_method_call;
use super::lower_call::platform::{
    lower_capability_call, lower_cookie_field_call, lower_cross_capability_call,
    lower_d1_peek_call, lower_d1_select_call, lower_events_emit_call, lower_signed_cookie_call,
};
use super::lower_call::query::{held_values_source, path_values_source};
use super::lower_call::statics::{
    lower_runtime_value_static_call, lower_scalar_static_call, lower_type_static_call,
};
use super::lower_call::store::lower_store_method_call;
use super::lower_call::test_calls::{
    lower_system_http_call, lower_system_schedule_call, lower_test_asset_fetch_call,
    lower_test_service_call,
};
use super::lower_ctx::LowerCtx;
use super::lower_stmt::{
    Body, Item, arms_contain_question, block_body, block_body_into, block_contains_question,
    hoist_value_as_statement, if_tail, match_dispatch, match_tail, ternary_shaped,
};
use super::ts_types::{
    ts_base, ts_ty_to_ts_type, ts_type_ref_to_ts_type, unchecked_construct,
    unchecked_construct_test_node,
};
use super::*;

/// v0.154 (ADR 0178): the `Err`-wrap a `?` on a `Result` operand needs, if the
/// operand's error type differs from the enclosing return's and a declared
/// embedding converts it. Returns `(sum_type, variant)` to emit
/// `Err(sum_type.variant(err))`, or `None` to propagate the `Err` unchanged.
/// Uses the checker's `embedding_for`, so it can never diverge from what the
/// checker accepted.
fn embed_conversion(
    operand_ty: Option<bynk_check::checker::TyId>,
    cx: &LowerCtx,
) -> Option<(String, String)> {
    let tys = cx.ty_render();
    use bynk_check::checker::Ty;
    let operand_node = operand_ty.map(|t| tys.get(t));
    let Some(Ty::Result(_, e)) = operand_node.as_deref() else {
        return None;
    };
    let f_err = peel_result_err(cx.return_ty?, tys.types())?;
    // An exact/compatible match propagates as-is; only a genuine mismatch that a
    // declared embedding resolves gets wrapped.
    if bynk_check::checker::compatible(*e, f_err, tys.types()) {
        return None;
    }
    bynk_check::checker::embedding_for(f_err, *e, &cx.commons().types, tys.types())
}

/// Peel `Result[_, E]` / `Effect[Result[_, E]]` to the error type `E`.
fn peel_result_err(
    ty: bynk_check::checker::TyId,
    tys: &Arc<Types>,
) -> Option<bynk_check::checker::TyId> {
    let ty = tys.get(ty);
    if let Some((_, e)) = ty.as_result() {
        Some(e)
    } else {
        peel_result_err(ty.as_effect()?, tys)
    }
}

/// Slice 1A.8: the value-position `if`'s hoist: `let slot: T; if (cond) { …; slot = then; } else
/// { …; slot = else; }` as statement nodes, the slot typed as the checker typed `e`. Also the
/// short-circuit operators' (`hoist_bool_if`) and the async tail's (`lower_tail_expr`, whose slot
/// annotation admits a promise). Returns the slot.
pub(super) fn hoist_if_slot(
    pre: &mut Pre,
    slot_ty: Option<bynk_ts::TsType>,
    cond: bynk_ts::TsExpr,
    then_tail: Lowered,
    else_tail: Lowered,
    cx: &mut LowerCtx,
) -> String {
    let slot = cx.fresh();
    // The slot needs its type spelled out. TypeScript's "evolving `let`" inference gives up
    // once a branch also `return`s (TS7034/TS7005 under `--strict`), which is exactly the shape
    // a hoisted `?` produces, so the type is the one the checker recorded for the `if`.
    pre.push_node(bynk_ts::TsStmt::let_stmt(
        bynk_ts::TsBindingName::Ident(slot.clone()),
        slot_ty,
        None,
        None,
    ));
    let branch = |tail: Lowered| {
        let mut stmts: Vec<bynk_ts::TsStmt> = tail.pre.into_iter().collect();
        stmts.push(bynk_ts::TsStmt::assign(ident_node(&slot), tail.expr, None));
        bynk_ts::TsStmt::block(stmts, None)
    };
    let then_branch = branch(then_tail);
    let else_branch = branch(else_tail);
    pre.push_node(bynk_ts::TsStmt::if_else_same_line_stmt(
        cond,
        then_branch,
        else_branch,
        None,
    ));
    slot
}

/// The type the checker recorded for `e` as a type node, if any: the annotation of the slot a
/// hoisted `if` declares.
pub(super) fn checked_ty_node(e: &Expr, cx: &LowerCtx) -> Option<bynk_ts::TsType> {
    cx.commons()
        .expr_ty(e.id)
        .map(|_| ts_ty_to_ts_type(cx.commons().expr_types[&e.id].ty, cx.ty_render()))
}

/// How a `?` leaves the function when its operand is the failing side. One
/// value drives both the node guard and today's text guard, so they cannot
/// disagree about which case applies.
enum QuestionExit {
    /// #2857: a test case's `?` fails the case, reporting the `Err` at the
    /// `?`'s location. Strings are raw (the printer, or `escape_ts_string` on
    /// the text side, escapes them).
    Case {
        location: String,
        start: usize,
        end: usize,
        src: String,
    },
    /// v0.153 (ADR 0177): `None` early-returns `HttpResult.NotFound`.
    NotFound,
    /// v0.154 (ADR 0178): `Err(T.V(__rN.error))`.
    Embed { ty_name: String, variant: String },
    /// `Err` propagates unchanged.
    Plain,
}

/// The `?` guard as a statement node: `if (__rN.tag === "Err") return …;`. The
/// return is not a `Block`, so the printer keeps it inline and the output is
/// byte-identical to the text form. The case form is an `InlineBlock`, which
/// prints `{ throw …; }` on the one line today's text has.
fn question_guard_node(tmp: &str, exit: &QuestionExit) -> bynk_ts::TsStmt {
    let tag = if matches!(exit, QuestionExit::NotFound) {
        "None"
    } else {
        "Err"
    };
    let cond = bynk_ts::TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::StrictEq,
        left: Box::new(member_node(tmp, "tag")),
        right: Box::new(str_node(tag)),
    };
    let call = |callee: bynk_ts::TsExpr, args: Vec<bynk_ts::TsExpr>| bynk_ts::TsExpr::Call {
        callee: Box::new(callee),
        args,
    };
    let then = match exit {
        QuestionExit::Case {
            location,
            start,
            end,
            src,
        } => {
            let detail = bynk_ts::TsExpr::Binary {
                op: bynk_ts::TsBinaryOp::Add,
                left: Box::new(str_node(format!("{src}\n  the operand was Err: "))),
                right: Box::new(call(
                    ident_node("__bynkShow"),
                    vec![member_node(tmp, "error")],
                )),
            };
            let failure = call(
                ident_node("__bynkExpectFailure"),
                vec![
                    str_node(location.clone()),
                    num_lit(start.to_string()),
                    num_lit(end.to_string()),
                    detail,
                ],
            );
            bynk_ts::TsStmt::inline_block(vec![bynk_ts::TsStmt::throw_stmt(failure, None)], None)
        }
        QuestionExit::NotFound => {
            bynk_ts::TsStmt::return_stmt(Some(member_node("HttpResult", "NotFound")), None)
        }
        QuestionExit::Embed { ty_name, variant } => bynk_ts::TsStmt::return_stmt(
            Some(call(
                ident_node("Err"),
                vec![call(
                    member_node(ty_name, variant),
                    vec![member_node(tmp, "error")],
                )],
            )),
            None,
        ),
        QuestionExit::Plain => bynk_ts::TsStmt::return_stmt(Some(ident_node(tmp)), None),
    };
    bynk_ts::TsStmt::if_stmt(cond, then, None)
}

/// `inner?` (slice 1A.8): hoists `const __rN = <inner>;` and the early-return
/// guard, and yields `__rN.value`. The two statements and the yielded member are nodes.
fn lower_question(e: &Expr, inner: &Expr, cx: &mut LowerCtx) -> Lowered {
    let tys = cx.ty_render();
    // v0.153 (ADR 0177): an `Option[T]?` operand lifts into an
    // HttpResult handler — `None` early-returns `NotFound` (404),
    // `Some(v)` yields `v`. Any other operand is a `Result`: `Err`
    // propagates unchanged. Branch on the operand's checked type.
    // The checker rejects any other `?` operand, so a typed operand is
    // always present — assert it, so a future gap surfaces loudly in
    // tests rather than silently emitting the `Result` branch (which on
    // an untyped `Option` would leak `None` → `undefined`).
    let operand_ty = cx.commons().expr_types.get(&inner.id).map(|te| te.ty);
    let operand_node = operand_ty.map(|t| tys.get(t));
    debug_assert!(
        matches!(
            operand_node.as_deref(),
            Some(Ty::Option(_) | Ty::Result(_, _))
        ),
        "`?` operand has no `Option`/`Result` checked type at {:?}: {operand_node:?}",
        inner.span,
    );
    let is_option = matches!(operand_node.as_deref(), Some(Ty::Option(_)));
    // v0.154 (ADR 0178): a `Result` operand whose error type differs from
    // the enclosing return's is converted by a declared embedding
    // (`embeds E as V`) — the same rule the checker accepted it under.
    let embed = if is_option || cx.question_fails_case() {
        None
    } else {
        embed_conversion(operand_ty, cx)
    };
    let mut pre = Pre::new();
    let operand = pre.lower_node(inner, cx);
    let tmp = cx.fresh();
    let exit = if cx.question_fails_case() {
        // #2857: a case returns a `CaseResult`, not the operand's
        // `Result` — so the `?` fails the case, reporting the `Err`
        // value at the `?`'s location. (The checker rejects an `Option`
        // operand outside an `HttpResult` handler.)
        QuestionExit::Case {
            location: expect_location_raw(cx, e.span.start),
            start: e.span.start,
            end: e.span.end,
            src: expect_source_text_raw(cx, e.span),
        }
    } else if is_option {
        QuestionExit::NotFound
    } else if let Some((ty_name, variant)) = embed {
        QuestionExit::Embed { ty_name, variant }
    } else {
        QuestionExit::Plain
    };
    pre.push_node(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(tmp.clone()),
        None,
        operand,
        None,
    ));
    pre.push_node(question_guard_node(&tmp, &exit));
    // T2.3 (R6.3): every branch above just pushed a `return` that must
    // exit the enclosing function, not whatever scope this `Lowered`
    // eventually lands in — see `LowerCtx::emitted_early_return`.
    cx.emitted_early_return = true;
    pre.finish(member_node(&tmp, "value"))
}

/// v0.59: the `location` string an `expect` failure carries. With a test-body
/// [`AssertLoc`](crate::emitter::ctx::AssertLoc) in scope it is a real, escaped
/// `path:line:col` (so `--format json` consumers can link to the source);
/// otherwise it falls back to the bare byte offset (`expect`s only appear in test
/// bodies, so the fallback is defensive).
pub(super) fn expect_location(cx: &LowerCtx, offset: usize) -> String {
    crate::emitter::escape_ts_string(&expect_location_raw(cx, offset))
}

/// [`expect_location`] before escaping, for a `Str` node (the printer escapes
/// it with the same one function, `escape_ts_literal`).
pub(super) fn expect_location_raw(cx: &LowerCtx, offset: usize) -> String {
    match cx.assert_loc() {
        Some(loc) => {
            let (line, col) = bynk_syntax::span::line_col(&loc.source, offset);
            // Normalise to forward slashes so the location is identical on
            // Windows (where `PathBuf` joins with `\`) — matching the
            // diagnostic path rendering and the committed goldens.
            let path = loc.rel_path.replace('\\', "/");
            format!("{path}:{line}:{col}")
        }
        None => format!("offset {offset}"),
    }
}

/// v0.112: the trimmed source text of an `expect` predicate, embedded in the
/// structural failure report (`expect <src>`), unescaped: it goes into a `Str`
/// node (the printer escapes it).
/// Empty when no test-body source is in scope (defensive — `expect`s only appear
/// in test bodies).
pub(super) fn expect_source_text_raw(cx: &LowerCtx, span: bynk_syntax::span::Span) -> String {
    match cx.assert_loc() {
        Some(loc) => loc
            .source
            .get(span.start..span.end)
            .unwrap_or("")
            .trim()
            .to_string(),
        None => String::new(),
    }
}

/// A compile-time literal that v0.9.4 may admit as a refined type (int, float,
/// decimal or string), as a node (1A.17, Decision D). `None` for anything else.
///
/// A negated numeric literal is admissible at compile time (the checker folds the
/// sign in `const_literal`), so the lowering must route it through `unsafe` like
/// any other admitted literal; [`ExprKind::refinable_literal`] reads it as one.
fn lower_const_literal_node(e: &Expr) -> Option<bynk_ts::TsExpr> {
    let sign = |negated: bool| if negated { "-" } else { "" };
    Some(match e.kind.refinable_literal()? {
        RefinableLiteral::Int { value, negated } => num_lit(format!("{}{value}", sign(negated))),
        // v0.21: the stored lexeme verbatim.
        RefinableLiteral::Float { lexeme, negated } => {
            num_lit(format!("{}{lexeme}", sign(negated)))
        }
        RefinableLiteral::Decimal {
            coef,
            scale,
            negated,
        } => decimal_lit(&format!("{}{coef}", sign(negated)), scale),
        RefinableLiteral::Str(s) => str_node(s),
    })
}

/// A literal's data, destructured from its `ExprKind` by `lower_expr`'s own
/// arms, so [`Lit::lower`] matches an exhaustive type with no catch-all.
enum Lit<'a> {
    /// The integer's digits.
    Int(String),
    /// The stored lexeme, verbatim.
    Float(&'a str),
    Decimal(&'a str, u32),
    /// The duration's resolved milliseconds (v0.86, ADR 0112): the value `Duration` erases to.
    Duration(String),
    Str(&'a str),
    Bool(bool),
    Unit,
    None,
}

/// Slice 1A.2 (#3694): the literal family lowers to typed nodes ([`Lit::lower`]). The string's
/// escaping is `TsLit::Str`'s (the one shared escaper); numbers print their
/// digits verbatim; `None` and the unit value are the runtime's `None` /
/// `undefined`.
impl Lit<'_> {
    fn lower(self) -> Lowered {
        use bynk_ts::{TsExpr, TsLit};
        Lowered::node(match self {
            Lit::Int(digits) | Lit::Duration(digits) => num_lit(digits),
            // v0.21: the stored lexeme verbatim — `1e10` must not normalise.
            Lit::Float(lexeme) => num_lit(lexeme.to_string()),
            // #2622: a `Decimal` literal is an object literal (no runtime import).
            Lit::Decimal(coef, scale) => decimal_lit(coef, scale),
            Lit::Str(s) => TsExpr::Lit(TsLit::Str(s.to_string())),
            Lit::Bool(b) => TsExpr::Lit(TsLit::Bool(b)),
            Lit::None => TsExpr::Ident("None".to_string()),
            Lit::Unit => TsExpr::Ident("undefined".to_string()),
        })
    }
}

/// A numeric literal node: the digits are printed verbatim (`TsLit::Num`), so a
/// big integer, a float lexeme (`1e10`) or a duration's milliseconds render
/// exactly as the old hand-built text did.
pub(super) fn num_lit(digits: String) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(digits))
}

/// #2622: a `Decimal` literal as an object-literal node — the node twin of
/// [`decimal_ts_literal`](crate::emitter::ts_types::decimal_ts_literal)
/// (`{ coefficient: <signed>n, scale: <scale> }`, no runtime import).
pub(super) fn decimal_lit(coef: &str, scale: u32) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::object(vec![
        (
            "coefficient".to_string(),
            bynk_ts::TsExpr::bigint(decimal_signed_coef(coef)),
        ),
        ("scale".to_string(), num_lit(scale.to_string())),
    ])
}

/// Lower an interpolated string (v0.43, ADR 0075) to a TS template literal.
/// Chunks become escaped literal text; each hole becomes `${String(<expr>)}`.
/// `String(…)` is identity for a `String` hole and the display form for
/// `Int`/`Float`/`Bool` — and the checker guarantees only base scalars reach
/// here, so no `[object Object]` can be emitted.
fn lower_interp_str(parts: &[InterpPart], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    // `chunks` are the already-escaped literal parts (`escape_ts_template`, which
    // `TemplateLit` prints verbatim): always one more than there are holes, with
    // an empty chunk between adjacent holes.
    let mut chunks: Vec<String> = vec![String::new()];
    let mut holes: Vec<bynk_ts::TsExpr> = Vec::new();
    let mut decimal_holes: Vec<bool> = Vec::new();
    for part in parts {
        match part {
            InterpPart::Chunk(text) => {
                if let Some(last) = chunks.last_mut() {
                    last.push_str(&escape_ts_template(text));
                }
            }
            InterpPart::Hole(hole) => {
                holes.push(pre.lower_node(hole, cx));
                // #2622: a `Decimal` prints as its canonical text, not as the
                // host's `[object Object]`.
                let decimal = cx
                    .commons()
                    .expr_types
                    .get(&hole.id)
                    .and_then(|te| te.ty.base(cx.commons().tys()))
                    == Some(BaseType::Decimal);
                if decimal {
                    cx.note_decimal("__bynkDecimalToString");
                }
                decimal_holes.push(decimal);
                chunks.push(String::new());
            }
        }
    }
    let wrapper = |decimal: bool| {
        if decimal {
            "__bynkDecimalToString"
        } else {
            "String"
        }
    };
    let built = {
        let nodes = holes;
        let exprs = nodes
            .into_iter()
            .zip(&decimal_holes)
            .map(|(hole, d)| bynk_ts::TsExpr::Call {
                callee: Box::new(ident_node(wrapper(*d))),
                args: vec![hole],
            })
            .collect();
        bynk_ts::TsExpr::template_lit(chunks.clone(), exprs)
    };
    pre.finish(built)
}

/// Escape a literal chunk for a TS template-literal context: backslash,
/// backtick, and `$` (to neutralise `${`), plus the control-char escapes
/// [`escape_ts_string`] applies. (v0.43.)
fn escape_ts_template(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '`' => out.push_str("\\`"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            // #2619: `\0`, other control characters and the JS line terminators
            // U+2028/U+2029 print as `\u{…}` escapes (valid in strings and templates).
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') => {
                out.push_str(&format!("\\u{{{:X}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

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

/// A genuine identifier node. Never built from expression text.
pub(super) fn ident_node(name: &str) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Ident(name.to_string())
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
        object: Box::new(ident_node(object)),
        property: property.to_string(),
    }
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

/// `callee(a, b, …)` over a genuine callee node.
pub(super) fn call_built(callee: bynk_ts::TsExpr, args: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Call {
        callee: Box::new(callee),
        args,
    }
}

/// A string-literal node: the printer escapes it exactly as
/// `escape_ts_string` does (they are one function, `escape_ts_literal`).
pub(super) fn str_node(s: impl Into<String>) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(s.into()))
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
        self.0.push(binding_stmt(&tmp, value));
        ident_node(&tmp)
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
        // but is being used as a type"). Brand via `unchecked_construct_test`
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
                Some(Ty::HttpResult(_))
            );
            if http {
                cx.note_http_result();
            }
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
            pre.finish(ident_node("__bynkRunId"))
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
            let built = lower_faults(f, cx);
            pre.finish(built)
        }
        // `trace(Cap.op)` → the recorded calls mapped to per-call records
        // whose fields are the operation's parameters (positionally).
        ExprKind::Trace { cap, op } => pre.finish(trace_node(cx, &cap.name, &op.name)),
    }
}

/// v0.9.1: expect as an expression (v0.112, renamed from `assert`).
/// Emit a runtime helper call that returns void (i.e., evaluates to
/// `undefined` at runtime and is treated as the unit value `()` in Bynk
/// terms). The expression form reports the predicate source only — the
/// statement form carries the structural expected-vs-actual report.
fn expect_built(inner: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let value = pre.lower_node(inner, cx);
    let span_start = inner.span.start;
    let span_end = inner.span.end;
    let location = expect_location_raw(cx, span_start);
    let src = format!("expect {}", expect_source_text_raw(cx, inner.span));
    // The `Paren` keeps today's `((value))` bytes; the `Str` nodes
    // escape exactly as `escape_ts_string` does.
    {
        let c = vec![value];
        bynk_ts::TsExpr::Call {
            callee: Box::new(ident_node("__bynkExpect")),
            args: vec![
                bynk_ts::TsExpr::Paren(Box::new(one(c))),
                str_node(location.clone()),
                num_lit(span_start.to_string()),
                num_lit(span_end.to_string()),
                str_node(src.clone()),
            ],
        }
    }
}

/// The parameter names of a capability operation, looked up from the capability
/// declarations in scope (v0.117). Used to destructure a recorded call's
/// arguments for a `with` predicate and to build `trace(Cap.op)` records.
/// P6.29 (design/tracks/the-ir.md §6a): the declaration lookup itself now reads
/// `bynk_model::capability_op_sig_from_commons` — still a by-name walk
/// under the hood (nothing IR-native replaces "find the op named `op` on the
/// capability named `cap`"), but relocated to `ir/lower.rs` (excluded from
/// `ast_importers` — the `Ast → Ir` lowering pass's own job), so this file no
/// longer spells `bynk_syntax::ast::CommonsItem` itself.
fn cap_op_param_names(cx: &LowerCtx, cap: &str, op: &str) -> Vec<String> {
    bynk_model::capability_op_sig_from_commons(cx.commons(), cap, op)
        .map(|sig| sig.params.into_iter().map(|(name, _)| name).collect())
        .unwrap_or_default()
}

/// 1A.33: what the checker recorded for a call-site principal clause (`by Nobody`, a reserved
/// bad credential driven by `BadCredential::driver`, or an actor).
///
/// Every checker path that accepts a `by` clause records it. If one ever does
/// not, fall back to the same single classification the checker uses
/// (`CallSitePrincipal::of`) rather than treating `Nobody` or `Forged` as an
/// ordinary actor of that name, and flag the gap in debug builds.
pub(super) fn call_site_principal(
    cx: &LowerCtx,
    principal: &bynk_syntax::ast::CallSiteActor,
) -> CallSitePrincipal {
    match cx.commons().callee(principal.id) {
        Some(Callee::Principal(p)) => p.clone(),
        _ => {
            debug_assert!(
                false,
                "unrecorded call-site principal `{}`",
                principal.actor.name
            );
            CallSitePrincipal::of(&principal.actor.name)
        }
    }
}

/// Lower a fault claim, `<call> faults` (#1706), to a `Bool` JavaScript
/// expression: an async IIFE that awaits the call and yields `true` when it
/// threw. An `ExpectationError` is re-thrown, not counted — a failed `expect`
/// inside the call's own evaluation is the case failing, not the call
/// faulting. Every other throw counts: an injected `stub … fails`, a real
/// provider's failure, an invariant violation. A fault is untyped, so the
/// claim does not discriminate between them.
///
/// The call is lowered as an `<-` subject is, under the claim's own call-site
/// principal (#1812) set exactly as the effect-let arm sets it — the
/// identity the addressed handler reads, or `by Nobody`'s no-credential
/// driver — and never an enclosing one. Whatever the call (or the identity)
/// hoists is placed inside the `try`, not in the enclosing statement's
/// prelude: a fault raised while evaluating the call's arguments is the call
/// faulting too, and must not escape the claim.
fn lower_faults(faults: &FaultsExpr, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let FaultsExpr { call, principal } = faults;
    let saved_identity = cx.call_site_identity.take();
    let saved_actor = cx.call_site_actor.take();
    let saved_no_credential = std::mem::replace(&mut cx.call_site_no_credential, false);
    let saved_bad_signature = cx.call_site_bad_signature.take();
    let mut inner = Pre::new();
    if let Some(principal) = principal {
        // `by Nobody … faults` (and #2135 `by Forged`/`by Stale`) is
        // rejected before emit (`bynk.test.faults_needs_in_process`);
        // mirrored here so the claim lowers as the effect-let arm does
        // regardless.
        let kind = call_site_principal(cx, principal);
        if kind == CallSitePrincipal::Nobody {
            cx.call_site_no_credential = true;
        } else if let CallSitePrincipal::BadCredential(bad) = kind {
            cx.call_site_bad_signature = Some(bad.driver());
        } else {
            cx.call_site_identity = principal
                .identity
                .as_ref()
                .map(|id| inner.lower_node(id, cx));
            cx.call_site_actor = Some(principal.actor.name.clone());
        }
    }
    let value = inner.lower_node(call, cx);
    cx.call_site_identity = saved_identity;
    cx.call_site_actor = saved_actor;
    cx.call_site_no_credential = saved_no_credential;
    cx.call_site_bad_signature = saved_bad_signature;
    // The literal `await` below is a real source of effectfulness for any
    // synchronous-looking IIFE further out — see `emitted_await`.
    cx.emitted_await = true;
    // 1A.17, Decision F: an async arrow IIFE over a `try`/`catch`.
    {
        let c = vec![value];
        use bynk_ts::{TsBinaryOp, TsExpr, TsLit, TsStmt};
        let mut try_body = inner.ts_stmts();
        try_body.push(TsStmt::expr_stmt(
            TsExpr::Await(Box::new(TsExpr::Paren(Box::new(one(c))))),
            None,
        ));
        try_body.push(TsStmt::return_stmt(
            Some(TsExpr::Lit(TsLit::Bool(false))),
            None,
        ));
        let rethrow = TsStmt::if_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::InstanceOf,
                left: Box::new(ident_node("__e")),
                right: Box::new(ident_node("ExpectationError")),
            },
            TsStmt::block(vec![TsStmt::throw_stmt(ident_node("__e"), None)], None),
            None,
        );
        let catch_body = vec![
            rethrow,
            TsStmt::return_stmt(Some(TsExpr::Lit(TsLit::Bool(true))), None),
        ];
        let try_catch = TsStmt::try_catch(
            TsStmt::block(try_body, None),
            Some("__e"),
            TsStmt::block(catch_body, None),
            None,
        );
        TsExpr::Paren(Box::new(arrow_call(
            Vec::new(),
            Vec::new(),
            true,
            vec![try_catch],
        )))
    }
}

/// `(<obs>.log["Cap.op"] ?? [])`: the calls recorded for one capability
/// operation (v0.117). `obs` is the per-case trace object, always `__obs`.
fn recorded_calls(cx: &LowerCtx, cap: &str, op: &str) -> bynk_ts::TsExpr {
    use bynk_ts::{TsBinaryOp, TsExpr};
    let obs = cx.observation_trace().unwrap_or("__obs");
    TsExpr::Paren(Box::new(TsExpr::Binary {
        op: TsBinaryOp::NullishCoalescing,
        left: Box::new(TsExpr::Index {
            object: Box::new(member_node(obs, "log")),
            index: Box::new(str_node(format!("{cap}.{op}"))),
        }),
        right: Box::new(TsExpr::array(Vec::new())),
    }))
}

/// `{ args: unknown[] }`, the recorded call's shape as far as `.args` is read
/// positionally (P7.2).
fn call_args_type() -> bynk_ts::TsType {
    use bynk_ts::{TsType, TsTypeMember};
    TsType::Object(vec![TsTypeMember::prop(
        "args",
        TsType::array(TsType::named("unknown")),
    )])
}

/// `{ n: __c.args[0], … }`: the operation's parameters read positionally off a
/// recorded call.
fn call_fields(names: &[String]) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::object(
        names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                (
                    n.clone(),
                    bynk_ts::TsExpr::Index {
                        object: Box::new(member_node("__c", "args")),
                        index: Box::new(num_lit(i.to_string())),
                    },
                )
            })
            .collect(),
    )
}

/// `trace(Cap.op)` (v0.117): the recorded calls mapped to per-call records.
fn trace_node(cx: &LowerCtx, cap: &str, op: &str) -> bynk_ts::TsExpr {
    use bynk_ts::{TsArrowBody, TsExpr, TsParam, TsType};
    let names = cap_op_param_names(cx, cap, op);
    // P7.2: only `.args` is read below (positionally), so `unknown[]` is
    // what's actually known about a recorded call's arguments here — but
    // the mapped-out `{ msg: unknown, … }` shape then flows into whatever
    // this `trace()` result feeds (a test's own `.filter`/projection,
    // typed against the checker's own per-op record type), which needs
    // the real field types, not `unknown`. Asserted into
    // `observation_call_record_types`' own generated alias (same name
    // `checker::call_record_type_name` derives, same test module, so no
    // qualification needed) — legal under `tsc --strict` because every
    // field of that alias is structurally assignable to `unknown`.
    let record_ty = bynk_check::checker::call_record_type_name(cap, op);
    let map = TsExpr::Call {
        callee: Box::new(TsExpr::Member {
            object: Box::new(recorded_calls(cx, cap, op)),
            property: "map".to_string(),
        }),
        args: vec![TsExpr::Arrow {
            params: vec![TsParam::new("__c".to_string(), Some(call_args_type()))],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(call_fields(&names)))),
        }],
    };
    TsExpr::Paren(Box::new(TsExpr::As {
        expr: Box::new(TsExpr::Paren(Box::new(map))),
        ty: TsType::array(TsType::named(record_ty)),
    }))
}

/// Lower an observation (v0.117) to a `Bool` JavaScript expression over the
/// recorded trace object. `total`/`matching` counts drive the sugar; `before`
/// compares call order.
fn lower_observation(o: &ObservationExpr, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    use bynk_ts::{TsBinaryOp, TsExpr};
    let calls = recorded_calls(cx, &o.cap.name, &o.op.name);
    let length = |calls: &TsExpr| TsExpr::Member {
        object: Box::new(calls.clone()),
        property: "length".to_string(),
    };
    let binary = |op: TsBinaryOp, left: TsExpr, right: TsExpr| TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    };
    let paren = |e: TsExpr| TsExpr::Paren(Box::new(e));
    // The text twin of `recorded_calls`, for the fallbacks below.
    match &o.matcher {
        ObservationMatcher::NeverCalled => paren(binary(
            TsBinaryOp::StrictEq,
            length(&calls),
            num_lit("0".to_string()),
        )),
        ObservationMatcher::Before { cap, op } => {
            let calls2 = recorded_calls(cx, &cap.name, &op.name);
            let order = |calls: &TsExpr| TsExpr::Member {
                object: Box::new(TsExpr::Index {
                    object: Box::new(calls.clone()),
                    index: Box::new(num_lit("0".to_string())),
                }),
                property: "order".to_string(),
            };
            let zero = || num_lit("0".to_string());
            let both = binary(
                TsBinaryOp::And,
                binary(TsBinaryOp::GreaterThan, length(&calls), zero()),
                binary(TsBinaryOp::GreaterThan, length(&calls2), zero()),
            );
            paren(binary(
                TsBinaryOp::And,
                both,
                binary(TsBinaryOp::LessThan, order(&calls), order(&calls2)),
            ))
        }
        ObservationMatcher::Called { count, with_pred } => match with_pred {
            None => match count {
                None => paren(binary(
                    TsBinaryOp::GreaterThanEq,
                    length(&calls),
                    num_lit("1".to_string()),
                )),
                Some(c) => {
                    let n = lower_expr(c, cx).expr;
                    {
                        let c = vec![n];
                        paren(binary(TsBinaryOp::StrictEq, length(&calls), paren(one(c))))
                    }
                }
            },
            Some(p) => {
                let names = cap_op_param_names(cx, &o.cap.name, &o.op.name);
                let record_ty = bynk_check::checker::call_record_type_name(&o.cap.name, &o.op.name);
                let pred_lowered = lower_expr(p, cx);
                // The predicate's hoisted statements go in the arrow body.
                let pre_stmts: Vec<bynk_ts::TsStmt> = pred_lowered.pre.into_iter().collect();
                let mut children = vec![pred_lowered.expr];
                if let Some(c) = count {
                    children.push(lower_expr(c, cx).expr);
                }
                let has_count = count.is_some();
                {
                    let mut c = children;
                    let n = has_count.then(|| c.pop()).flatten();
                    let pred = one(c);
                    let mut stmts = Vec::new();
                    // #291: typed through the operation's call-record alias (as
                    // `trace` is), so a predicate may read a record parameter's
                    // fields (`with req.url == …`) — `__c.args` is `unknown[]`.
                    if !names.is_empty() {
                        stmts.push(bynk_ts::TsStmt::const_stmt(
                            bynk_ts::TsBindingName::ObjectPattern(names.clone()),
                            None,
                            TsExpr::As {
                                expr: Box::new(call_fields(&names)),
                                ty: bynk_ts::TsType::named(record_ty.clone()),
                            },
                            None,
                        ));
                    }
                    stmts.extend(pre_stmts);
                    stmts.push(bynk_ts::TsStmt::return_stmt(Some(paren(pred)), None));
                    // P7.2: same shape as `trace` — only `.args` is destructured,
                    // so `unknown[]` is what's actually known.
                    let matching = TsExpr::Member {
                        object: Box::new(TsExpr::Call {
                            callee: Box::new(TsExpr::Member {
                                object: Box::new(calls.clone()),
                                property: "filter".to_string(),
                            }),
                            args: vec![TsExpr::Arrow {
                                params: vec![bynk_ts::TsParam::new(
                                    "__c".to_string(),
                                    Some(call_args_type()),
                                )],
                                is_async: false,
                                generics: Vec::new(),
                                return_type: None,
                                body: Box::new(bynk_ts::TsArrowBody::Multiline(stmts)),
                            }],
                        }),
                        property: "length".to_string(),
                    };
                    match n {
                        None => paren(binary(
                            TsBinaryOp::GreaterThanEq,
                            paren(matching),
                            num_lit("1".to_string()),
                        )),
                        Some(n) => paren(binary(TsBinaryOp::StrictEq, paren(matching), paren(n))),
                    }
                }
            }
        },
    }
}

/// A default base-type literal (as TypeScript source) that satisfies a refined
/// type's predicates, for bare `Val[T]`. `None` when no default can be derived
/// (a `Matches` refinement — the checker rejects bare `Val` for those).
fn refined_default(decl: &TypeDecl) -> Option<bynk_ts::TsExpr> {
    match &decl.body {
        TypeBody::Refined {
            base, refinement, ..
        } => refinement_default(*base, refinement.as_ref()),
        TypeBody::Record(..) | TypeBody::Sum(..) | TypeBody::Opaque { .. } => None,
    }
}

/// [`refined_default`] over a base type and its refinement, wherever it is
/// written: a refined type's declaration, or a record field's inline `where`
/// (#2658: `qty: Int where Positive` must fabricate `1`, not `0`).
fn refinement_default(base: BaseType, refinement: Option<&Refinement>) -> Option<bynk_ts::TsExpr> {
    // #2686: an `@example` is the value for a refinement `Val` cannot generate
    // (`Matches`, `IsoDate`); the checker proved it satisfies the refinement.
    if let Some(lit) = refinement
        .and_then(bynk_check::checker::refinement_example)
        .and_then(lower_const_literal_node)
    {
        return Some(lit);
    }
    match base {
        BaseType::Int => {
            let mut lo: i64 = 0;
            let mut hi = i64::MAX;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match &p.kind {
                        PredKind::Positive => lo = lo.max(1),
                        PredKind::NonNegative => lo = lo.max(0),
                        PredKind::InRange(a, b) => {
                            lo = lo.max(a.value);
                            hi = hi.min(b.value);
                        }
                        PredKind::Matches(..)
                        | PredKind::InRangeF(..)
                        | PredKind::InRangeD(..)
                        | PredKind::MinLength(..)
                        | PredKind::MaxLength(..)
                        | PredKind::Length(..)
                        | PredKind::NonEmpty
                        | PredKind::IsoDate => {}
                    }
                }
            }
            // The floor of 0 can overshoot a wholly negative range
            // (`InRange(-5, -1)`); fall back to the upper bound. An empty
            // range never reaches here: the checker rejects it.
            if lo > hi {
                lo = hi;
            }
            Some(num_lit(lo.to_string()))
        }
        BaseType::String => {
            let mut len: i64 = 0;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match p.kind {
                        PredKind::NonEmpty => len = len.max(1),
                        PredKind::MinLength(k) | PredKind::Length(k) => len = len.max(k),
                        PredKind::Matches(_) | PredKind::IsoDate => return None,
                        PredKind::InRange(..)
                        | PredKind::InRangeF(..)
                        | PredKind::InRangeD(..)
                        | PredKind::MaxLength(..)
                        | PredKind::NonNegative
                        | PredKind::Positive => {}
                    }
                }
            }
            if len < 1 {
                len = 1;
            }
            Some(str_node("x".repeat(len as usize)))
        }
        BaseType::Bool => Some(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true))),
        BaseType::Float => {
            let mut lo: f64 = 0.0;
            let mut hi = f64::INFINITY;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match &p.kind {
                        PredKind::Positive => lo = lo.max(1.0),
                        PredKind::NonNegative => lo = lo.max(0.0),
                        PredKind::InRangeF(a, b) => {
                            lo = lo.max(a.value);
                            hi = hi.min(b.value);
                        }
                        PredKind::Matches(..)
                        | PredKind::InRange(..)
                        | PredKind::InRangeD(..)
                        | PredKind::MinLength(..)
                        | PredKind::MaxLength(..)
                        | PredKind::Length(..)
                        | PredKind::NonEmpty
                        | PredKind::IsoDate => {}
                    }
                }
            }
            // The `Positive` floor of 1.0 can overshoot a tight fractional
            // range (`InRange(0.0, 0.5)`); fall back to the upper bound.
            if lo > hi {
                lo = hi;
            }
            Some(num_lit(lo.to_string()))
        }
        // v0.86: `Duration` carries no refinement; `0` millis is its default.
        BaseType::Duration | BaseType::Instant => Some(num_lit("0".to_string())),
        // v0.110: `Bytes` carries no refinement; the empty octet sequence is
        // its default.
        BaseType::Bytes => Some(empty_bytes_node()),
        // #2622: the smallest value the refinement admits, at scale 2.
        BaseType::Decimal => {
            let mut lo: i128 = 0;
            let mut hi: Option<i128> = None;
            let mut scale = 2u32;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    if let PredKind::InRangeD(a, b) = &p.kind {
                        scale = scale.max(a.scale).max(b.scale);
                    }
                }
                for p in &r.predicates {
                    match &p.kind {
                        PredKind::Positive => lo = lo.max(1),
                        PredKind::InRangeD(a, b) => {
                            match scaled_coef(&a.coef, a.scale, scale) {
                                Some(v) => lo = lo.max(v),
                                // The bound does not fit once lifted to the
                                // common scale: the bound itself, at its own
                                // scale, is the smallest value in range.
                                None => {
                                    return Some(decimal_lit(&a.coef, a.scale));
                                }
                            }
                            if let Some(v) = scaled_coef(&b.coef, b.scale, scale) {
                                hi = Some(hi.map_or(v, |h| h.min(v)));
                            }
                        }
                        PredKind::Matches(..)
                        | PredKind::InRange(..)
                        | PredKind::InRangeF(..)
                        | PredKind::MinLength(..)
                        | PredKind::MaxLength(..)
                        | PredKind::Length(..)
                        | PredKind::NonNegative
                        | PredKind::NonEmpty
                        | PredKind::IsoDate => {}
                    }
                }
            }
            // As for `Float`: a floor above the ceiling falls back to the ceiling.
            if let Some(h) = hi
                && lo > h
            {
                lo = h;
            }
            Some(decimal_lit(&lo.to_string(), scale))
        }
    }
}

/// `coef` (a signed digit string at `from` scale) raised to scale `to`, as an
/// `i128`, or `None` when it does not fit.
fn scaled_coef(coef: &str, from: u32, to: u32) -> Option<i128> {
    let v: i128 = coef.parse().ok()?;
    v.checked_mul(10i128.checked_pow(to.checked_sub(from)?)?)
}

/// v0.9.4 Part B (slice 1): lower a refined-type `Val[T]` / `Val[T](lit)` to
/// the branded `unsafe` constructor. The checker has already validated this is a
/// refined type in a test body, and recorded the refined type at `span`.
/// v0.9.4 slice 2 recursion cap for bare `Val` generation (mirrors the
/// checker's `MOCK_DEPTH`).
const MOCK_DEPTH: u32 = 12;

/// `new globalThis.Uint8Array()`: the empty octet sequence, `Bytes`'s default.
fn empty_bytes_node() -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::New {
        callee: Box::new(member_node("globalThis", "Uint8Array")),
        args: Vec::new(),
    }
}

/// A base-literal default for an opaque type's underlying base, as a node. Not
/// distinct per call in this increment — per-call distinctness via a runtime
/// counter is a follow-up.
fn base_default_ts(base: BaseType) -> bynk_ts::TsExpr {
    match base {
        BaseType::Int => num_lit("0".to_string()),
        BaseType::String => str_node("mock"),
        BaseType::Bool => bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true)),
        BaseType::Float => num_lit("0".to_string()),
        BaseType::Duration | BaseType::Instant => num_lit("0".to_string()),
        BaseType::Bytes => empty_bytes_node(),
        BaseType::Decimal => decimal_lit("0", 0),
    }
}

/// Generate a TypeScript expression for a bare `Val` of `ty` (v0.9.4 Part B,
/// slice 2), as a node (1A.17, Decision D). Recurses through sum payloads and
/// record fields; refined types use `refined_default`, opaque types wrap a base
/// default, bare bases use 0/""/true.
///
/// A shape with no mock value renders as the `undefined` identifier here and
/// nowhere else (1A.34): [`mock_value_of`] says `None` for it, and this is the
/// one place that renders it. `at` is the `Val[T]` call, where a checker fact
/// the recursion needs and does not find is reported.
fn mock_value(ty: TyId, cx: &LowerCtx, depth: u32, at: bynk_syntax::span::Span) -> bynk_ts::TsExpr {
    mock_value_of(ty, cx, depth, at).unwrap_or_else(undefined_node)
}

/// [`mock_value`], with the legitimate absence of a value as `None`: the
/// recursion bound, a type no mock builds (`Option`, `List`, `Unit`, ...), a
/// compiler-known type with no declaration, and a sum with no variant.
fn mock_value_of(
    ty: TyId,
    cx: &LowerCtx,
    depth: u32,
    at: bynk_syntax::span::Span,
) -> Option<bynk_ts::TsExpr> {
    let tys = cx.ty_render();
    if depth == 0 {
        return None;
    }
    // A declared field or payload type the checker resolved when it checked the
    // declaration: it not resolving here is a missing fact.
    let field_value = |type_ref: &TypeRef| match bynk_check::checker::resolve_type_ref(
        type_ref,
        &cx.commons().types,
        tys.types(),
    ) {
        Some(t) => mock_value(t, cx, depth - 1, at),
        None => cx.missing_fact(at, "resolved type for a mocked field"),
    };
    Some(match &*tys.get(ty) {
        Ty::Base(BaseType::Int) => num_lit("0".to_string()),
        Ty::Base(BaseType::String) => str_node(""),
        Ty::Base(BaseType::Bool) => bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true)),
        Ty::Base(BaseType::Float) => num_lit("0".to_string()),
        Ty::Base(BaseType::Decimal) => decimal_lit("0", 0),
        Ty::Named { name, .. } => {
            let decl = cx.commons().types.get(name.as_str())?;
            match &decl.body {
                TypeBody::Refined { .. } => {
                    let d = refined_default(decl).unwrap_or_else(|| num_lit("0".to_string()));
                    unchecked_construct_test_node(name, d, false)
                }
                TypeBody::Opaque { base, .. } => {
                    unchecked_construct_test_node(name, base_default_ts(*base), true)
                }
                TypeBody::Sum(s) => match s.variants.first()? {
                    v if v.payload.is_empty() => member_node(name, &v.name.name),
                    v => {
                        let args = v.payload.iter().map(|f| field_value(&f.type_ref)).collect();
                        bynk_ts::TsExpr::Call {
                            callee: Box::new(member_node(name, &v.name.name)),
                            args,
                        }
                    }
                },
                TypeBody::Record(r) => bynk_ts::TsExpr::object(
                    r.fields
                        .iter()
                        .map(|f| {
                            // An inline `where` on a base-typed field narrows
                            // the base's default, as a declared refined type's
                            // does; the field stays a plain base value.
                            let inline = match (&f.type_ref, &f.refinement) {
                                (TypeRef::Base(base, _), Some(r)) => {
                                    refinement_default(*base, Some(r))
                                }
                                _ => None,
                            };
                            let fv = inline.unwrap_or_else(|| field_value(&f.type_ref));
                            (f.name.name.clone(), fv)
                        })
                        .collect(),
                ),
            }
        }
        Ty::Base(BaseType::Bytes | BaseType::Duration | BaseType::Instant)
        | Ty::Error
        | Ty::Never
        | Ty::Result(..)
        | Ty::Option(..)
        | Ty::Effect(..)
        | Ty::HttpResult(..)
        | Ty::QueueResult
        | Ty::HttpOutcome(..)
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::RequestQuery
        | Ty::RequestCookies
        | Ty::TokenClaims
        | Ty::List(..)
        | Ty::Map(..)
        | Ty::Set(..)
        | Ty::Query(..)
        | Ty::Stream(..)
        | Ty::Connection(..)
        | Ty::TestConnection(..)
        | Ty::ValidationError
        | Ty::DecodeError
        | Ty::Unit
        | Ty::Actor(..)
        | Ty::ActorSum(..)
        | Ty::Fn { .. }
        | Ty::Var(..) => return None,
    })
}

/// W30 (#2840): the statement that opens a service handler's claim scope, which
/// wraps the already-lowered function body (`lower_stmt::claim_scoped`) so the
/// claims it made are settled when it exits. `__claims` is the provider's own
/// scope (`Idempotency.__track()`, hidden: not in the capability), or a no-op
/// scope for a provider without one, which takes the claim for its full
/// window and never settles it. A thrown fault releases the claims; a
/// returned `Err` or a 5xx `HttpResult` does too; any other outcome confirms
/// them.
/// The text the scope declaration was written as, kept to check [`claim_scope_declaration`]
/// prints exactly it.
#[cfg(test)]
pub(crate) const CLAIM_SCOPE_DECLARATION: &str = "const __claims = (deps.Idempotency as unknown as { __track?: () => { note<C>(k: string, c: C, t: number): C; lease(t: number): number; settle(r: unknown, threw: boolean): globalThis.Promise<void> } }).__track?.() ?? { note: <C>(_k: string, c: C, _t: number) => c, lease: (t: number) => t, settle: async (_r: unknown, _threw: boolean) => {} };";

/// [`CLAIM_SCOPE_DECLARATION`] as a node (1A.35a part B): `const __claims = (deps.Idempotency
/// as unknown as { __track?: () => { note<C>(…): C; lease(…): number; settle(…): … } })
/// .__track?.() ?? { note: …, lease: …, settle: … };`.
pub(crate) fn claim_scope_declaration() -> bynk_ts::TsStmt {
    use bynk_ts::{
        TsArrowBody, TsBinaryOp, TsBindingName, TsExpr, TsObjectEntry, TsParam, TsStmt, TsType,
        TsTypeMember,
    };
    let named = |name: &str| TsType::named(name);
    let param = |name: &str, ty: &str| TsParam::new(name, Some(named(ty)));
    // The provider's scope: `{ note<C>(k, c, t): C; lease(t): number; settle(r, threw): Promise<void> }`.
    let scope_ty = TsType::Object(vec![
        TsTypeMember::Method {
            name: "note".to_string(),
            generics: vec!["C".to_string()],
            params: vec![param("k", "string"), param("c", "C"), param("t", "number")],
            ret: named("C"),
            doc: None,
        },
        TsTypeMember::method("lease", vec![param("t", "number")], named("number")),
        TsTypeMember::method(
            "settle",
            vec![param("r", "unknown"), param("threw", "boolean")],
            TsType::named_with_args("globalThis.Promise", vec![named("void")]),
        ),
    ]);
    let track_ty = TsType::Object(vec![TsTypeMember::optional_prop(
        "__track",
        TsType::Fn {
            params: Vec::new(),
            names: Vec::new(),
            ret: Box::new(scope_ty),
        },
    )]);
    let provider = TsExpr::Paren(Box::new(TsExpr::As {
        expr: Box::new(TsExpr::As {
            expr: Box::new(member_node("deps", "Idempotency")),
            ty: named("unknown"),
        }),
        ty: track_ty,
    }));
    let tracked = TsExpr::OptionalCall {
        callee: Box::new(TsExpr::Member {
            object: Box::new(provider),
            property: "__track".to_string(),
        }),
        args: Vec::new(),
    };
    // The no-op scope: takes every claim for its full window and never settles.
    let arrow = |generics: &[&str], params: Vec<TsParam>, is_async: bool, body: TsArrowBody| {
        TsExpr::Arrow {
            params,
            is_async,
            generics: generics.iter().map(ToString::to_string).collect(),
            return_type: None,
            body: Box::new(body),
        }
    };
    let ident = |n: &str| TsExpr::Ident(n.to_string());
    let noop = TsExpr::object_entries(vec![
        TsObjectEntry::prop(
            "note",
            arrow(
                &["C"],
                vec![
                    param("_k", "string"),
                    param("c", "C"),
                    param("_t", "number"),
                ],
                false,
                TsArrowBody::Expr(Box::new(ident("c"))),
            ),
        ),
        TsObjectEntry::prop(
            "lease",
            arrow(
                &[],
                vec![param("t", "number")],
                false,
                TsArrowBody::Expr(Box::new(ident("t"))),
            ),
        ),
        TsObjectEntry::prop(
            "settle",
            arrow(
                &[],
                vec![param("_r", "unknown"), param("_threw", "boolean")],
                true,
                TsArrowBody::Block(Vec::new()),
            ),
        ),
    ]);
    TsStmt::const_stmt(
        TsBindingName::Ident("__claims".to_string()),
        None,
        TsExpr::Binary {
            op: TsBinaryOp::NullishCoalescing,
            left: Box::new(tracked),
            right: Box::new(noop),
        },
        None,
    )
}

/// Lower an `ExprKind::MethodCall`. This is a dispatcher: a sequence of
/// independent guard-and-`return` branches, tried in order (the order is
/// load-bearing — earlier guards take precedence), falling through to the
/// UFCS instance-call tail. The collection/numeric/string/option/result
/// kernels and the typed JSON codec delegate to dedicated helpers that
/// return `Option<String>`.
fn lower_method_call(e: &Expr, cx: &mut LowerCtx) -> Lowered {
    let ExprKind::MethodCall {
        receiver,
        method,
        type_args,
        args,
        ..
    } = &e.kind
    else {
        unreachable!("lower_method_call is called on a method call")
    };
    let mut pre = Pre::new();
    // Each dispatcher below returns a node that ends in
    // `pre.finish` (1A.20, #3721, Decision G): every family builds nodes (the statics in
    // `lower_call/statics.rs`, the store, D1, cross-context, events, cookie, capability,
    // test-service and agent families each under `lower_call/`), every delegated kernel's node
    // passes through unprinted, and the plain instance call that ends this function is a node.
    // Persisted-store receivers: held Map, Map, Set, Cache, Log, Cell (#3167).
    if let Some(built) = lower_store_method_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish(built);
    }
    // Intrinsic statics: scalar and collection constructors, then runtime values (#3167).
    if let Some(built) = lower_scalar_static_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_runtime_value_static_call(e, receiver, args, cx, &mut pre) {
        return pre.finish(built);
    }
    // Capability, cross-context, events and static-type calls (#3167).
    if let Some(built) = lower_d1_peek_call(e, cx) {
        return pre.finish(built);
    }
    if let Some(built) = lower_d1_select_call(e, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) =
        lower_cross_context_call(e, receiver, method, type_args, args, cx, &mut pre)
    {
        return pre.finish(built);
    }
    if let Some(built) = lower_events_emit_call(e, receiver, type_args, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_signed_cookie_call(e, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_cookie_field_call(e, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_capability_call(e, receiver, method, type_args, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_type_static_call(e, receiver, method, type_args, args, cx, &mut pre)
    {
        return pre.finish(built);
    }
    // Test-case and system-tier service addresses (1A.26, #3727), then agent invocations (#3167).
    if let Some(built) = lower_test_asset_fetch_call(e, receiver, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_system_schedule_call(e, receiver, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_system_http_call(e, receiver, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_test_service_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_inline_agent_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_let_bound_agent_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish(built);
    }
    // v0.20b: built-in kernel methods, resolved by the checker (#3164).
    if let Some(built) = lower_kernel_method_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish(built);
    }
    // Instance call: UFCS lowering with the receiver as first arg.
    let built = lower_ufcs_method_call(e, receiver, method, type_args, args, cx, &mut pre);
    pre.finish(built)
}

/// Lower a cross-context capability call (`B.Cap.op(…)`) or service call, or `None` when the call is not one. The
/// capability half is `lower_call/platform.rs`'s (1A.24), the service-call half `lower_call/cross_context.rs`'s (1A.25).
fn lower_cross_context_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    if let Some(built) = lower_cross_capability_call(e, receiver, method, type_args, args, cx, pre)
    {
        return Some(built);
    }
    // v0.6 cross-context service call: receiver is an alias or the
    // dotted name of a consumed context. #1961: unless the checker resolved
    // this call to something else — a local shadowing the alias
    // (`1203_cross_context_call_shadowed_by_local`) is a UFCS call, and
    // lowering it over the wire named a Service Binding the Worker, which
    // binds only what it calls, does not declare.
    if !matches!(cx.commons().callee(e.id), Some(c) if !matches!(c, Callee::Cross { .. })) {
        return lower_cross_context_service_call(receiver, method, args, cx, pre);
    }
    None
}

/// The instance-call fallback (UFCS lowering, #3167): the receiver becomes the
/// first argument of the namespace function for the receiver's checked type. The
/// result is the call expression; hoisted statements go to `pre`.
fn lower_ufcs_method_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> bynk_ts::TsExpr {
    let tys = cx.ty_render();
    // Instance call: UFCS lowering with the receiver as first arg. The
    // namespace comes from the receiver's checked type. Every method call the
    // checker accepts has one: a test-service call's outcome is typed by its
    // handler (#1934), where it once reached a placeholder and failed only at
    // `tsc` (#1931). A receiver with no namespace is a missing checker fact
    // (1A.34), reported at the receiver.
    let Some(ns) = cx.receiver_namespace(receiver) else {
        return cx.missing_fact(receiver.span, "receiver namespace for an instance call");
    };
    let recv = pre.lower_node(receiver, cx);
    let mut all = vec![recv];
    for a in args {
        all.push(pre.lower_node(a, cx));
    }
    // Constrained generics slice 1: the call's comparator witnesses follow the
    // ordinary arguments.
    let mut needs_exec = false;
    if let Some(Callee::Method(decl, witnesses)) = cx.commons().callees.get(&e.id) {
        all.extend(cx.witness_args(decl, witnesses));
        // #2719: a method in the needs-context set takes the context last.
        if cx.commons().exec_plan.fn_needs(decl) {
            all.push(cx.exec_node());
            needs_exec = true;
        }
    }
    // #2795: explicit type arguments on a generic instance method are kept, the
    // receiver's own parameters first (the emitted `<A, U>` order), so a call
    // with no argument of type `U` still types as written. The method's own
    // arguments are what the author wrote; a receiver argument is spelled only
    // when it is a base or built-in type, which is in scope everywhere, and is
    // the named widening `unnameable_receiver_type_arg` otherwise (a type the
    // calling module may never import, or a type variable that may not be this
    // function's).
    let explicit: Vec<bynk_ts::TsType> = if type_args.is_empty() {
        Vec::new()
    } else {
        let recv_args: Option<Vec<TyId>> =
            cx.commons().expr_types.get(&receiver.id).and_then(|te| {
                if let Ty::Named { args, .. } = &*tys.get(te.ty) {
                    Some(args.clone())
                } else {
                    None
                }
            });
        match recv_args {
            Some(recv_args) => recv_args
                .iter()
                .map(|a| {
                    if ts_spelling_in_scope_everywhere(*a, tys.types()) {
                        ty_to_type_ref(*a, tys.types())
                            .map(|r| ts_type_ref_to_ts_type(&r, None))
                            .unwrap_or_else(|| {
                                cx.missing_fact_ty(
                                    receiver.span,
                                    "codec shape for a receiver type argument",
                                )
                            })
                    } else {
                        unnameable_receiver_type_arg()
                    }
                })
                .chain(type_args.iter().map(|t| ts_type_ref_to_ts_type(t, None)))
                .collect(),
            None => Vec::new(),
        }
    };
    // Every call is a node, with a `TypeApp` callee when type arguments are explicit (1A.20,
    // #3721, Decision A).
    let mut callee = bynk_ts::TsExpr::Member {
        object: Box::new(ident_node(&ns)),
        property: method.name.clone(),
    };
    if !explicit.is_empty() {
        callee = bynk_ts::TsExpr::TypeApp {
            callee: Box::new(callee),
            type_args: explicit,
        };
    }
    let built = call_built(callee, all);
    if needs_exec {
        settle_built(built, cx)
    } else {
        built
    }
}

/// #2795: whether a type's TypeScript spelling is in scope in every emitted
/// module: base types, `Unit` and the built-in containers over them.
fn ts_spelling_in_scope_everywhere(t: TyId, tys: &Arc<Types>) -> bool {
    match &*tys.get(t) {
        Ty::Base(_) | Ty::Unit => true,
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => ts_spelling_in_scope_everywhere(*a, tys),
        Ty::Result(a, b) | Ty::Map(a, b) => {
            ts_spelling_in_scope_everywhere(*a, tys) && ts_spelling_in_scope_everywhere(*b, tys)
        }
        Ty::Error
        | Ty::Never
        | Ty::Named { .. }
        | Ty::Effect(..)
        | Ty::HttpResult(..)
        | Ty::QueueResult
        | Ty::HttpOutcome(..)
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::RequestQuery
        | Ty::RequestCookies
        | Ty::TokenClaims
        | Ty::Query(..)
        | Ty::Stream(..)
        | Ty::Connection(..)
        | Ty::TestConnection(..)
        | Ty::ValidationError
        | Ty::DecodeError
        | Ty::Actor(..)
        | Ty::ActorSum(..)
        | Ty::Fn { .. }
        | Ty::Var(..) => false,
    }
}

fn lower_val(type_ref: &TypeRef, args: &[Expr], cx: &mut LowerCtx) -> Lowered {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    // Resolve the mocked type straight from the AST node rather than the
    // checker's `expr_types` side-table — the static type table is always
    // populated, whereas a test body's per-expression types may not be visible
    // to the emitter.
    let ty = match bynk_check::checker::resolve_type_ref(type_ref, &cx.commons().types, tys.types())
    {
        Some(t) => t,
        // A `Val[T]` whose `T` the checker resolved when it checked the call:
        // it not resolving here is a missing fact (1A.34).
        None => {
            return pre.finish(cx.missing_fact(type_ref.span(), "resolved type for a `Val` call"));
        }
    };
    // Refined literal pin. This `Val[T](lit)` path is test-only scaffolding, where
    // the branded type is an `any` value binding, not a type (ADR 0182) — so brand
    // via `unchecked_construct_test` (refined → `(lit as any)`), not a `(lit as T)`
    // that would fail to resolve `T`. Opaque never reaches here (`NamedKind::Refined`).
    let ty_node = tys.get(ty);
    if let (
        Some(arg),
        Ty::Named {
            name,
            kind: NamedKind::Refined(_),
            ..
        },
    ) = (args.first(), &*ty_node)
    {
        let value = match lower_const_literal_node(arg) {
            Some(n) => n,
            None => pre.lower_node(arg, cx),
        };
        let built = {
            let c = vec![value];
            unchecked_construct_test_node(name, one(c), false)
        };
        return pre.finish(built);
    }
    // Bare mock (refined / opaque / sum / record).
    pre.finish(mock_value(ty, cx, MOCK_DEPTH, type_ref.span()))
}

/// When we encounter `lhs && rhs`, see if lhs is an `is` (possibly wrapped
/// in parens or nested `&&`) and if so collect the bindings to inject into
/// rhs. Returns `(binding_const_decls, lowered_lhs, lowered_rhs)` if
/// special handling is appropriate; otherwise returns None.
/// What [`lower_and_with_is`] produces for its caller to assemble.
struct AndWithIs {
    /// The left operand's `is` bindings, then the right operand's own hoisted
    /// statements: both run, in order, inside the right operand's IIFE.
    bindings: Vec<bynk_ts::TsStmt>,
    lhs: Lowered,
    rhs: bynk_ts::TsExpr,
    /// The right operand hoisted a `?`'s propagating early return.
    rhs_returns: bool,
    /// #1654: declarations of the `is` receiver temps the right operand
    /// introduced, for the caller to emit before the whole condition.
    hoisted: Vec<bynk_ts::TsStmt>,
}

fn lower_and_with_is(lhs: &Expr, rhs: &Expr, cx: &mut LowerCtx) -> Option<AndWithIs> {
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
fn mark_unnarrowed(rhs: &Expr, cx: &mut LowerCtx) {
    for when_true in [true, false] {
        for test in bynk_check::narrowing::matched_is_tests(rhs, when_true) {
            cx.unnarrowed_is_tests.insert(test.span);
        }
    }
}

/// Walk an expression collecting `const name = expr.field;` strings for
/// any `is`-pattern bindings on the truthy path. `found` indicates whether
/// at least one `is` was seen.
pub(super) fn gather_is_bindings_for_emit(
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

/// `const <local> = <init>;` as a statement node.
fn binding_stmt(local: &str, init: bynk_ts::TsExpr) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(local.to_string()),
        None,
        init,
        None,
    )
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
                out.push(binding_stmt(&ts_ident(&id.name), cast));
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
                out.push(binding_stmt(&local, read));
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
                out.push(binding_stmt(
                    &local,
                    recv.member(&fname.name).node().clone(),
                ));
            }
        }
        // #2680: a list pattern's depth-1 names — bare-name elements and a
        // named rest.
        if let Pattern::List { elems, rest, .. } = pattern.as_ref() {
            for (i, sub) in elems.iter().enumerate() {
                if let Pattern::Binding(n) = sub {
                    let local = cx.bind_local_name(&n.name);
                    out.push(binding_stmt(&local, recv.index(i).node().clone()));
                }
            }
            if let Some(binder) = rest.as_ref().and_then(|r| r.binder.as_ref()) {
                let local = cx.bind_local_name(&binder.name);
                out.push(binding_stmt(&local, list_rest_value(&recv, elems.len())));
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
                            bool_node(true)
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
                            right: Box::new(str_node(variant.name.clone())),
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
                    Some(tag.unwrap_or_else(|| bool_node(true)))
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
pub(super) fn path_for_is(value: &Expr) -> PatPath {
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

fn lower_if(
    e: &Expr,
    cond: &Expr,
    then_block: &Block,
    else_block: &Block,
    cx: &mut LowerCtx,
) -> Lowered {
    let mut pre = Pre::new();
    let cond_lowered = pre.lower_node(cond, cx);
    // If the cond contains `is`-bindings, the then-branch needs a place
    // for the `const name = receiver.field;` declarations — a ternary
    // has no such place. Force the IIFE form. (`ternary_shaped`.)
    if ternary_shaped(cond, then_block, else_block, cx) {
        // T2.1 (R6.2): lower each branch's tail once, then ask the branches
        // themselves whether they hoisted, rather than asking `simple_expr` to
        // predict it.
        //
        // #1029 review: `return_ty` is deliberately **not** cleared here, unlike
        // the IIFE path below. That path clears it because a `return` in its
        // arms exits the arrow rather than the enclosing function, so an
        // embedding `?` must behave like a plain one (ADR 0178). This path
        // inverts that premise: when a branch hoists, `hoist_if_slot`
        // puts the statements in the *caller's* statement position, so the `?`
        // expansion's `return` does exit the enclosing function and the declared
        // embedding must still apply. Clearing it emitted a bare `return __rN;`
        // — an `Err` of the operand's error type returned from a function
        // declared with the embedding sum's.
        //
        // Preserving it cannot move the no-hoist branch: `return_ty` is read
        // only by `embed_conversion`, only from the `?` expansion, and a `?`
        // expansion always pushes to `pre` — so where `pre` is empty, nothing
        // read it.
        let then_tail = lower_expr(&then_block.tail, cx);
        let else_tail = lower_expr(&else_block.tail, cx);
        if then_tail.pre.is_empty() && else_tail.pre.is_empty() {
            // The `Paren` keeps today's outer brackets.
            return pre.finish(ternary_built(cond_lowered, then_tail.expr, else_tail.expr));
        }
        // A branch hoisted. Before T2.1 there was nowhere to put its statements
        // from here — a value position had no statement slot — and the only
        // shape available was the arrow below, which swallows a hoisted `?`'s
        // early return. `lower_if` now returns its own `pre`, so the `if` hoists
        // as a real statement in the caller's statement position and the
        // `return` keeps exiting the enclosing function. See
        // `hoist_if_slot`.
        let slot_ty = checked_ty_node(e, cx);
        let slot = hoist_if_slot(&mut pre, slot_ty, cond_lowered, then_tail, else_tail, cx);
        pre.finish(ident_node(&slot))
    } else {
        // T2.2 (R6.4): isolate the effectfulness flag to just this IIFE's own
        // body. Unlike `lower_match_as_iife`'s `disc_expr` (spliced *outside*
        // the returned arrow, at the call site), `cond`'s *text* is spliced
        // inside this arrow's own `if (...)` below — only its hoisted
        // statements landed in `pre` above. A bare `await` in `cond`'s text
        // would need to feed `needs_async`, not `saved_await`; the
        // debug_assert after the arrow closes documents that this can't
        // happen today (a literal `await` comes only from `EffectLet`/`Do`,
        // both hoisted to `pre`, or from a nested `finish_async_iife` wrap,
        // which would require a value-position `match`/`if` needing the async
        // wrap to itself type as `Bool` — ruled out by the checker's
        // Effect-typing) rather than silently trusting it.
        let saved_await = std::mem::take(&mut cx.emitted_await);
        debug_assert!(
            !bynk_ts::expr_contains_await(&cond_lowered),
            "cond lands inside this arrow; its awaits must feed `needs_async`, not `saved_await`"
        );
        // Inject is-binding declarations on the truthy side. #908: scoped to
        // the then-branch only — pushed here, popped once it's emitted —
        // so a read inside resolves to the binding rather than an outer
        // `let` rename, and the registration doesn't leak into the else
        // branch or beyond.
        cx.shadow_scopes.push(HashMap::new());
        let mut is_bindings = Vec::new();
        let mut found = false;
        gather_is_bindings_for_emit(cond, true, cx, &mut is_bindings, &mut found);
        // 1A.12: each branch is built as a `Body` and rendered, or turned into
        // nodes, once both are in hand (its `is` bindings, nodes or text lines,
        // open it); the context saves and restores below
        // sit around the same lowering calls, in the same order, as when each
        // branch was rendered straight into the text.
        let mut then_body = Body::new();
        // The `is` bindings open their branch, before its statements.
        then_body.extend(is_bindings.iter().map(stmt_item));
        // v0.154 (ADR 0178): a value-position `if` lowers to an IIFE, whose
        // `return`s exit the arrow, so clear the enclosing `return_ty`. #1750:
        // an `if` whose branches hold a `?` no longer reaches here (it becomes a
        // statement, `hoist_value_as_statement`, so the `?` exits the function
        // and keeps its embedding), so nothing inside this arrow reads it.
        let saved = cx.return_ty.take();
        // #1750: the arrow's own tails `return` from the arrow.
        let saved_slot = cx.tail_slot.take();
        // The branches keep their marks (1A.15, Decision D): as nodes the printer
        // records them, and the text fallback below drops them (`render_text`).
        block_body_into(&mut then_body, then_block, cx, false);
        cx.shadow_scopes.pop();
        // #1654: the bindings the condition's falsity proves, scoped to the
        // else-branch the same way.
        cx.shadow_scopes.push(HashMap::new());
        let mut else_bindings = Vec::new();
        gather_is_bindings_for_emit(cond, false, cx, &mut else_bindings, &mut found);
        let mut else_body = Body::new();
        else_body.extend(else_bindings.iter().map(stmt_item));
        block_body_into(&mut else_body, else_block, cx, false);
        cx.shadow_scopes.pop();
        cx.return_ty = saved;
        cx.tail_slot = saved_slot;
        // #2 review: unlike `lower_match_as_iife`'s IIFE, this one used to never
        // consult the async-wrap decision at all — confirmed live: `let r = if
        // c { let y <- fetch(); y + 1 } else { 0 }` emitted a bare `await`
        // inside this arrow with no `async` keyword, a hard JS syntax error
        // ("Unexpected reserved word"), not merely a missed optimisation.
        let needs_async = cx.emitted_await;
        cx.emitted_await = saved_await || needs_async;
        let branch = bynk_ts::TsStmt::if_else_same_line_stmt(
            cond_lowered,
            bynk_ts::TsStmt::block(then_body.into_stmts(), None),
            bynk_ts::TsStmt::block(else_body.into_stmts(), None),
            None,
        );
        pre.finish(iife_call(needs_async, vec![branch]))
    }
}

/// `(() => { <body> })()`, an immediately invoked arrow over statement nodes,
/// `async` and awaited when `is_async` (1A.12).
fn iife_call(is_async: bool, body: Vec<bynk_ts::TsStmt>) -> bynk_ts::TsExpr {
    arrow_call(Vec::new(), Vec::new(), is_async, body)
}

/// `((params) => { <body> })(args)`, `async` and awaited when `is_async`: the
/// `match` IIFE's `(__d) => { … }` over its discriminant, and [`iife_call`]'s
/// parameterless one (1A.13).
fn arrow_call(
    params: Vec<bynk_ts::TsParam>,
    args: Vec<bynk_ts::TsExpr>,
    is_async: bool,
    body: Vec<bynk_ts::TsStmt>,
) -> bynk_ts::TsExpr {
    let call = bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Arrow {
            params,
            is_async,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Multiline(body)),
        }))),
        args,
    };
    if is_async {
        bynk_ts::TsExpr::Await(Box::new(call))
    } else {
        call
    }
}

/// True if `e` proves at least one `is` test when true — the tests
/// `gather_is_bindings_for_emit` declares, by the shared rule
/// [`bynk_check::narrowing::matched_is_tests`] (#1654), which descends `!`,
/// `&&`, `||` and `implies` with polarity. Used by `lower_and_with_is` to decide
/// whether the `is`-binding flow applies before doing any lowering.
fn cond_contains_is(e: &Expr) -> bool {
    !bynk_check::narrowing::matched_is_tests(e, true).is_empty()
}

/// Slice 1A.4 (#3701): the identifier lowering, as a [`bynk_ts::TsExpr`] (a node, or the
/// text of a branch that still builds a closure or thunk). The branch order is
/// deliberate and unchanged: constants, `Ord`/needs closures, transition
/// records, invariant fields, store cells, the held/store-`Map` and `Log`
/// thunks, `HttpResult`/`QueueResult` constructors, sum variants, the actor-sum
/// binder, then a plain local.
/// `(() => values)`: a store read used as a value, the deferred `Query` thunk (1A.23, #3724).
fn store_thunk(values: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Arrow {
        params: vec![],
        is_async: false,
        generics: vec![],
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(values))),
    }))
}

fn lower_ident(e: &Expr, id: &Ident, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    // #2617: a constant is a hoisted, cached function (`emit_consts`), so a
    // name the checker resolved to one reads it with a call. First, since the
    // checker has already ruled out a local or any other meaning of the name.
    if let Some(name) = cx.const_ref(e) {
        return bynk_ts::TsExpr::Call {
            callee: Box::new(ident_node(&ts_ident(name))),
            args: Vec::new(),
        };
    }
    // Constrained generics slice 1 (DECISION E): a generic function with `Ord`
    // bounds used as a value is instantiated at the reference, as a closure that
    // captures the comparator witnesses.
    if let Some(Callee::FnValue(decl, witnesses)) = cx.commons().callee(e.id)
        && (!witnesses.is_empty()
            || !decl.given.is_empty()
            || cx.commons().exec_plan.fn_needs(decl))
    {
        let tys = cx.ty_render();
        let params: Vec<bynk_ts::TsParam> = match cx
            .commons()
            .expr_types
            .get(&e.id)
            .map(|te| (*tys.get(te.ty)).clone())
        {
            Some(Ty::Fn { params, .. }) => params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    bynk_ts::TsParam::new(format!("__a{i}"), Some(ts_ty_to_ts_type(*p, tys)))
                })
                .collect(),
            _ => (0..decl.params.len())
                .map(|i| {
                    bynk_ts::TsParam::new(format!("__a{i}"), Some(super::lower_list::any_ty()))
                })
                .collect(),
        };
        let mut args: Vec<bynk_ts::TsExpr> = params.iter().map(|p| ident_node(&p.name)).collect();
        args.extend(cx.witness_args(decl, witnesses));
        // #2868: a `given` function closes over the `deps` in scope where it is
        // named.
        if !decl.given.is_empty() {
            args.push(cx.cap_deps_node());
        }
        // #2719: a function in the needs-context set closes over the context in
        // scope where it is named.
        if cx.commons().exec_plan.fn_needs(decl) {
            args.push(cx.exec_node());
        }
        return bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Arrow {
            params,
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(
                bynk_ts::TsExpr::Call {
                    callee: Box::new(ident_node(&ts_ident(&id.name))),
                    args,
                },
            ))),
        }));
    }
    // v0.116: inside a `transition` predicate, the contextual `old`/`new` idents
    // are the pre-/post-commit state records, lowered to their JS names (`new` is
    // reserved, so both are renamed). Field access `old.<f>` reads off the record.
    // Checked first so `old`/`new` never collide with the heuristics below.
    if let Some((old_var, new_var)) = cx.transition_states() {
        if id.name == "old" {
            return ident_node(old_var);
        }
        if id.name == "new" {
            return ident_node(new_var);
        }
    }
    // v0.80: inside an invariant predicate, a bare ident naming a state field
    // reads it off the proposed-state value (`s.<field>`). Checked first so a
    // field never collides with the variant-constructor heuristics below.
    if let Some((var, fields)) = cx.invariant_state()
        && fields.contains(&id.name)
    {
        return member_node(var, &id.name);
    }
    // v0.81: inside a `store`-agent handler, a bare ident naming a `Cell` field
    // reads it off the mutable working state (`__state.<cell>`), so a read after
    // a `:=` write in the same handler sees the written value (read-your-writes).
    if let Some((var, cells)) = cx.agent_store_cells()
        && cells.contains(&id.name)
    {
        return member_node(var, &id.name);
    }
    // v0.104/v0.105 (real-time track slice 3b): a bare held-`Map` ident used as a
    // value is a lazy `Query` over its **resolved** connections — the persisted
    // `Record<K, connId>` mapped through `resolveConnection`, keeping the present
    // ones. Checked before the persisted-`Map` branch (held maps are excluded from
    // `agent_store_maps`).
    if !cx.is_local(&id.name)
        && let Some(frame) = cx.agent_held_map_frame(&id.name).cloned()
    {
        let recv = member_node(cx.agent_store_var(), &id.name);
        return store_thunk(held_values_source(recv, &frame));
    }
    // v0.94 (ADR 0120): a bare `store Map` ident used as a **value** — not a
    // method receiver (those are handled in the method dispatch) — is a lazy
    // `Query` over the whole map, e.g. the `other` side of a join. It lowers to
    // the same deferred thunk a query builder yields: `() => Object.values(map)`.
    if !cx.is_local(&id.name) && cx.is_agent_store_map(&id.name) {
        let recv = member_node(cx.agent_store_var(), &id.name);
        return store_thunk(path_values_source(recv));
    }
    // v0.95 (ADR 0121): a bare `store Log` ident used as a value is a lazy
    // `Query` over its entry values — `() => log.map((__e) => __e.v)`.
    if !cx.is_local(&id.name) && cx.is_agent_store_log(&id.name) {
        let recv = member_node(cx.agent_store_var(), &id.name);
        return store_thunk(path_values_source(recv));
    }
    // v0.9: a nullary HttpResult variant (whose checker type is
    // `HttpResult[_]`) constructs an HttpResult.<Variant>.
    //
    // P6.21/P6.23 (review of #1251): reads `Callee::Intrinsic` instead of
    // the `Ty::HttpResult`/`Ty::QueueResult` type check plus a bare
    // `http_variant`/`queue_variant` keyword lookup — consistency with
    // every other static-constructor branch in this module, not a new
    // defect closure: the resolved-type guard here was already immune to
    // name-matching (a shadowing local never types as `Ty::HttpResult`).
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic {
            ns: IntrinsicNs::HttpResult,
            ..
        })
    ) {
        cx.note_http_result();
        return member_node("HttpResult", &id.name);
    }
    // v0.44: a nullary QueueResult variant (`Ack`) constructs `QueueResult.Ack`.
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic {
            ns: IntrinsicNs::QueueResult,
            ..
        })
    ) {
        cx.note_queue_result();
        return member_node("QueueResult", &id.name);
    }
    // A bare ident whose name matches a declared variant of a sum
    // type (and whose checker type is that sum) is a nullary
    // variant constructor reference. Qualify it as `Type.Variant`.
    // Otherwise (locals, params, `self`) emit the identifier as-is.
    if let Some(Ty::Named {
        kind: NamedKind::Sum,
        name: type_name,
        ..
    }) = cx.commons().expr_ty(e.id).as_deref()
        && let Some(decl) = cx.commons().types.get(type_name)
        && let TypeBody::Sum(s) = &decl.body
        && s.variants.iter().any(|v| v.name.name == id.name)
    {
        if let Ok(built) = consumed_variant_literal(e, type_name, &id.name, Vec::new(), cx) {
            return built;
        }
        return member_node(type_name, &id.name);
    }
    // v0.52: the multi-actor sum binder is not a runtime local — the resolved
    // actor is threaded through `deps.who` at the boundary wrapper, so the
    // binder ident lowers to it (the tagged union the body `match`es).
    if cx.actor_sum_binder() == Some(id.name.as_str())
        && matches!(cx.commons().expr_ty(e.id).as_deref(), Some(Ty::ActorSum(_)))
    {
        return member_node("deps", "who");
    }
    // #908: a re-`let` of this name (in this or an enclosing block) renamed
    // its emitted binding — resolve to it rather than the natural
    // (now-shadowed) `ts_ident` name.
    ident_node(
        &cx.resolved_local_name(&id.name)
            .unwrap_or_else(|| ts_ident(&id.name)),
    )
}

/// #2045: on `workers`, a variant of a consumed context's sum built inline —
/// `({ tag: "Held", reason: x } as Status)` — rather than through the callee's
/// companion (`Status.Held(x)`), which would make the callee's module a value
/// import of this Worker (#661). The companion's constructors are pure data,
/// so the literal is the same value; the cast keeps the tag narrow, through
/// the type-only import of `Status`. `None` for any other sum.
fn consumed_variant_literal(
    e: &Expr,
    sum: &str,
    tag: &str,
    args: Vec<bynk_ts::TsExpr>,
    cx: &LowerCtx,
) -> Result<bynk_ts::TsExpr, Vec<bynk_ts::TsExpr>> {
    let shape = (|| {
        if !cx.is_consumed_context_type(sum) {
            return None;
        }
        let decl = cx.commons().types.get(sum)?;
        let TypeBody::Sum(s) = &decl.body else {
            return None;
        };
        let variant = s.variants.iter().find(|v| v.name.name == tag)?;
        if variant.payload.len() != args.len() {
            return None;
        }
        let ty = cx.commons().expr_types.get(&e.id)?.ty;
        let props: Vec<String> = variant
            .payload
            .iter()
            .map(|p| crate::emitter::payload_prop(&p.name.name))
            .collect();
        Some((
            crate::emitter::ts_types::ts_ty_to_ts_type(ty, cx.ty_render()),
            props,
        ))
    })();
    let Some((ty, props)) = shape else {
        return Err(args);
    };
    Ok({
        let c = args;
        let mut entries = vec![bynk_ts::TsObjectEntry::prop(
            "tag".to_string(),
            str_node(tag),
        )];
        entries.extend(
            props
                .iter()
                .zip(c)
                .map(|(p, a)| bynk_ts::TsObjectEntry::prop(p.clone(), a)),
        );
        bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
            expr: Box::new(bynk_ts::TsExpr::object_entries(entries)),
            ty,
        }))
    })
}

/// Lower a variant call's arguments. #2341 (L4): a `RawError`'s first argument
/// is an `ErrorStatus` name the checker resolved, so it lowers to its number
/// (`Forbidden` → `403`), not to the `HttpResult` value of that name.
pub(super) fn lower_http_variant_args(
    e: &Expr,
    args: &[Expr],
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> Vec<bynk_ts::TsExpr> {
    // 1A.33: the variant is the one the checker resolved, not the call's spelling.
    let raw_error = matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic {
            op: StaticOp::HttpResult(v),
            ..
        }) if v.payload == bynk_syntax::ast::HttpVariantPayload::RawError
    );
    args.iter()
        .enumerate()
        .map(|(i, a)| {
            if raw_error
                && i == 0
                && let Some(status) = bynk_syntax::ast::raw_error_status(a)
            {
                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(status.to_string()))
            } else {
                pre.lower_node(a, cx)
            }
        })
        .collect()
}

/// #2719 (G): which helper a test `case` that calls a function in the
/// needs-context set directly settles the call with: on virtual time it is
/// driven (auto-advance) and then settled, otherwise the sends it handed the
/// harness's execution context are awaited, so the case's `expect`s see their
/// effects. `None` outside a test case, or when nothing needs settling.
pub(super) fn settle_helper(cx: &LowerCtx) -> Option<&'static str> {
    if !cx.in_test_case() {
        None
    } else if cx.runtime_use().virtual_time() {
        Some("__bynkSettleVirtual")
    } else if cx.test_settles_sends() {
        Some("__bynkSettle")
    } else {
        None
    }
}

/// [`settle_helper`] over a built call: `helper(deps, call)`.
pub(super) fn settle_built(call: bynk_ts::TsExpr, cx: &LowerCtx) -> bynk_ts::TsExpr {
    settle_with(settle_helper(cx), call)
}

/// [`settle_built`] with the helper already read, for a caller that lowers its
/// arguments between reading the settle state and settling (the test-case
/// service call reads it first, as it always has).
pub(super) fn settle_with(helper: Option<&'static str>, call: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    match helper {
        None => call,
        Some(helper) => bynk_ts::TsExpr::Call {
            callee: Box::new(ident_node(helper)),
            args: vec![ident_node("deps"), call],
        },
    }
}

fn lower_call(e: &Expr, name: &Ident, args: &[Expr], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    // Bare variant constructor with payload → qualify.
    let mut args_lowered = lower_http_variant_args(e, args, &mut pre, cx);
    // #2620: `unreachable(msg)` / `todo(msg)` lower to a call of the runtime's
    // `never`-returning `__fault`, which throws — valid in any expression
    // position, and inside an async body it rejects the effect.
    if let Some(Callee::Fault { kind }) = cx.commons().callee(e.id) {
        // The location is a string-literal node: the printer escapes it with
        // the same `escape_ts_literal` the text form used.
        let location = cx
            .assert_loc()
            .map(|_| expect_location(cx, e.span.start))
            .or_else(|| cx.fault_location(e.span.start))
            .unwrap_or_else(|| format!("offset {}", e.span.start));
        cx.note_fault();
        args_lowered.insert(0, str_node(*kind));
        args_lowered.push(str_node(location));
        return pre.finish(call_built(ident_node("__fault"), args_lowered));
    }
    // #3051: `TestConnection[F]()` — a case's recording handle for a websocket
    // connection.
    if let Some(Callee::Intrinsic {
        ns: IntrinsicNs::TestConnection,
        ..
    }) = cx.commons().callee(e.id)
    {
        cx.runtime_use().note_test_connection();
        let frame = match cx.commons().expr_ty(e.id).as_deref() {
            Some(Ty::TestConnection(f)) => {
                crate::emitter::ts_types::ts_ty_to_ts_type(*f, cx.ty_render())
            }
            _ => cx.missing_fact_ty(
                e.span,
                "`TestConnection[_]` type for a `TestConnection()` call",
            ),
        };
        // `new TestConnection<F>()`: a `New` over a `TypeApp` (1A.20, #3721).
        return pre.finish(bynk_ts::TsExpr::New {
            callee: Box::new(bynk_ts::TsExpr::TypeApp {
                callee: Box::new(ident_node("TestConnection")),
                type_args: vec![frame],
            }),
            args: vec![],
        });
    }
    // v0.9: HttpResult variant call.
    //
    // P6.21/P6.23 (review of #1251): reads `Callee::Intrinsic` instead of
    // the `Ty::HttpResult`/`Ty::QueueResult` type check plus a bare
    // keyword lookup — consistency with the bare-`Ident` sibling in
    // `lower_ident`, not a new defect closure.
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic {
            ns: IntrinsicNs::HttpResult,
            ..
        })
    ) {
        cx.note_http_result();
        let callee = bynk_ts::TsExpr::Member {
            object: Box::new(ident_node("HttpResult")),
            property: name.name.clone(),
        };
        return pre.finish(call_built(callee, args_lowered));
    }
    // v0.44: a QueueResult variant call (`Retry(reason)`) → `QueueResult.Retry(...)`.
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic {
            ns: IntrinsicNs::QueueResult,
            ..
        })
    ) {
        cx.note_queue_result();
        let callee = bynk_ts::TsExpr::Member {
            object: Box::new(ident_node("QueueResult")),
            property: name.name.clone(),
        };
        return pre.finish(call_built(callee, args_lowered));
    }
    // P6.21 (real P6.2 cutover, partial — #1143/#1140's `Callee` classification
    // read back instead of re-derived): agent instantiation `AgentName(key)`
    // lowers to the generated `__makeAgentName(key)` factory, which obtains
    // the instance for this key (lookup-or-create against the registry in
    // bundle mode, or a typed DO proxy in workers mode). Skipped when this
    // Call is the receiver of a MethodCall — that path folds construction
    // and the method invocation together. Previously guarded by
    // `cx.local_agents.contains(&name.name)`, a name-matched re-derivation
    // of what the checker's own `Callee::AgentInit` already resolved once —
    // the same defect class `block_writes_state`'s receiver-name matching
    // carried before #1196 converted it to read `Callee::Store` directly.
    if matches!(cx.commons().callee(e.id), Some(Callee::AgentInit(_))) {
        return pre.finish(lower_agent_init(name, args_lowered, cx));
    }
    // P6.21: sum-variant construction is `Callee::Ctor` — reads the sum
    // type's own name and the variant's own tag straight off the checker's
    // classification instead of `call_is_sum_variant`'s own name-matched
    // `sum_name`/`call_name` string comparison against the call's checked
    // type (the same re-derivation this whole `Callee` precedent, P6.0,
    // exists to close).
    if let Some(Callee::Ctor { sum, tag }) = cx.commons().callee(e.id) {
        let args_lowered = match consumed_variant_literal(e, &sum.name.name, tag, args_lowered, cx)
        {
            Ok(built) => return pre.finish(built),
            // Not an inline literal: the companion call below.
            Err(args) => args,
        };
        let callee = bynk_ts::TsExpr::Member {
            object: Box::new(ident_node(&sum.name.name)),
            property: tag.to_string(),
        };
        return pre.finish(call_built(callee, args_lowered));
    }
    // Constrained generics slice 1: the call's comparator witnesses follow the
    // ordinary arguments (an `Ord`-bounded callee's hidden trailing parameters).
    let mut needs_exec = false;
    if let Some(Callee::Fn(decl, witnesses)) = cx.commons().callee(e.id) {
        args_lowered.extend(cx.witness_args(decl, witnesses));
        // #2868: a function that declares `given` takes the caller's `deps`,
        // which the checker has made carry its capabilities.
        if !decl.given.is_empty() {
            args_lowered.push(cx.cap_deps_node());
        }
        // #2719: a function in the needs-context set takes the context last.
        if cx.commons().exec_plan.fn_needs(decl) {
            args_lowered.push(cx.exec_node());
            needs_exec = true;
        }
    }
    let mut built = call_built(ident_node(&ts_ident(&name.name)), args_lowered);
    if needs_exec {
        built = settle_built(built, cx);
    }
    pre.finish(built)
}

/// `true` / `false` as a node.
fn bool_node(value: bool) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(value))
}

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

fn lower_bin_op(op: BinOp, lhs: &Expr, rhs: &Expr, cx: &mut LowerCtx) -> Lowered {
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
                Lowered::node(bool_node(otherwise == "true")),
                cx,
            );
            return pre.finish(short_circuit(op, lhs_expr, ident_node(&slot)));
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
                BinOp::And => hoist_bool_if(&mut pre, l, r, Lowered::node(bool_node(false)), cx),
                BinOp::Or => hoist_bool_if(&mut pre, l, Lowered::node(bool_node(true)), r, cx),
                BinOp::Implies => hoist_bool_if(
                    &mut pre,
                    negated_paren(l),
                    Lowered::node(bool_node(true)),
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
            return pre.finish(ident_node(&value));
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
pub(super) fn build_value_bin_op(
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
            cx.note_int();
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
        if op == BinOp::Eq { call } else { negated(call) }
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
fn plain_binary(op: BinOp, children: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    let c = children;
    let (l, r) = two(c);
    bynk_ts::TsExpr::Binary {
        op: value_binary_op(op),
        left: Box::new(l),
        right: Box::new(r),
    }
}

/// `helper(l, r)` over a runtime helper's genuine identifier.
fn helper_call(helper: &'static str, children: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    let c = children;
    let (l, r) = two(c);
    bynk_ts::TsExpr::Call {
        callee: Box::new(ident_node(helper)),
        args: vec![l, r],
    }
}

/// `!call`: a `Unary(Not)` node over a call node, or today's `!{call}` text.
fn negated(call: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Unary {
        op: bynk_ts::TsUnaryOp::Not,
        expr: Box::new(call),
    }
}

/// `cmp(l, r) <op> 0` for a non-base `Ord` type. The comparator is a node, so a
/// composite comparator (`__bynkCmpList(__bynkCmpInt)`) is a callee like any
/// other (1A.16 closes the text path 1A.5 left for it).
fn ordered_comparison(
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
/// type other than `Bytes` (a `Uint8Array`), or a refined type over one. Every
/// other `Ord` type goes through a comparator.
fn is_bare_ordered(ty: TyId, tys: &Types) -> bool {
    matches!(
        ty.base(tys),
        Some(
            BaseType::Int
                | BaseType::Float
                | BaseType::String
                | BaseType::Duration
                | BaseType::Instant
        )
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
        Some(EqualityLowering::Structural) | None => {
            cx.note_eq();
            Some("__bynkEq")
        }
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

/// How one object-literal field is written: `key: <child>` (taking the next
/// child), or the bare shorthand `key`.
enum FieldSlot {
    Prop(String),
    Shorthand(String),
}

/// The fields of a record literal as `(slot, child)` pairs, lowering each
/// value through `pre`. A shorthand `{ x }` references the binding `x`; if the
/// binding was renamed (a reserved word) it expands to `x: __id_x` — the key is
/// wire format and must keep the source spelling.
fn field_inits(
    fields: &[FieldInit],
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> (Vec<FieldSlot>, Vec<bynk_ts::TsExpr>) {
    let mut slots = Vec::new();
    let mut children = Vec::new();
    for f in fields {
        match &f.value {
            Some(v) => {
                children.push(pre.lower_node(v, cx));
                slots.push(FieldSlot::Prop(f.name.name.clone()));
            }
            None => {
                let v = ts_ident(&f.name.name);
                if v == f.name.name {
                    slots.push(FieldSlot::Shorthand(f.name.name.clone()));
                } else {
                    children.push(ident_node(&v));
                    slots.push(FieldSlot::Prop(f.name.name.clone()));
                }
            }
        }
    }
    (slots, children)
}

/// `{ k: v, … }` (or, with `spread`, `{ ...base, k: v, … }`): an `Object` node.
/// `children` holds the spread base first (when present), then
/// one child per [`FieldSlot::Prop`], in order.
fn object_built(
    has_spread: bool,
    slots: Vec<FieldSlot>,
    children: Vec<bynk_ts::TsExpr>,
) -> bynk_ts::TsExpr {
    let nodes = children;
    let mut nodes = nodes.into_iter();
    let mut entries = Vec::new();
    if has_spread && let Some(base) = nodes.next() {
        entries.push(bynk_ts::TsObjectEntry::Spread(base));
    }
    for slot in &slots {
        entries.push(match slot {
            FieldSlot::Prop(key) => match nodes.next() {
                Some(v) => bynk_ts::TsObjectEntry::prop(key.clone(), v),
                None => unreachable!("one child per `Prop` slot"),
            },
            FieldSlot::Shorthand(key) => bynk_ts::TsObjectEntry::Shorthand(key.clone()),
        });
    }
    bynk_ts::TsExpr::object_entries(entries)
}

/// #3759: the slot a defaulted field's value fills, `record[field]`, so a refined value in it is
/// cast to its position's type (`default_value_node`). Indexed from the record's type as the
/// checker typed this construction: the refined type may not be nameable here (the record can
/// come from a unit this one imports without its field types), but the record is, since this
/// literal names it. Test scaffolding names no declared type, so it brands as it brands a refined
/// literal.
fn default_slot(e: &Expr, type_name: &Ident, field: &str, cx: &LowerCtx) -> Slot {
    if cx.in_test_scaffold() {
        return Slot::Scaffold;
    }
    let record = match cx.commons().expr_types.get(&e.id) {
        Some(te) => ts_ty_to_ts_type(te.ty, cx.ty_render()),
        None => bynk_ts::TsType::named(cx.consumed_type_name(&type_name.name)),
    };
    Slot::Typed(bynk_ts::TsType::Indexed {
        object: Box::new(record),
        index: Box::new(bynk_ts::TsType::StringLiteral(field.to_string())),
    })
}

fn lower_record_construction(
    e: &Expr,
    type_name: &Ident,
    fields: &[FieldInit],
    cx: &mut LowerCtx,
) -> Lowered {
    let mut pre = Pre::new();
    let (mut slots, mut children) = field_inits(fields, &mut pre, cx);
    // #2607 (Decision C): each declared field the literal leaves out takes its
    // default, written as a self-contained value (`field_default_value`, built as a node by
    // `default_value_node`) so it needs nothing imported into this module. The checker has
    // already required every field without a default, and rejected any default that has no
    // such value. A default is a node (1A.31, #3732), so a record that takes one stays one.
    let types = &cx.commons().types;
    if let Some(decl) = types.get(&type_name.name)
        && let TypeBody::Record(r) = &decl.body
    {
        for declared in &r.fields {
            let Some(init) = &declared.init else {
                continue;
            };
            if fields.iter().any(|f| f.name.name == declared.name.name) {
                continue;
            }
            match bynk_check::wire_default::field_default_value(init, &declared.type_ref, types) {
                Ok(v) => {
                    slots.push(FieldSlot::Prop(declared.name.name.clone()));
                    // #3759: a refined value in it is cast to its position's type.
                    let slot = match v.holds_refined() {
                        true => default_slot(e, type_name, &declared.name.name, cx),
                        false => Slot::Unbranded,
                    };
                    let node: bynk_ts::TsExpr = default_value_node(&v, &slot);
                    children.push(node);
                }
                // The checker rejected any default that has no value: a missing
                // fact (1A.34), reported at the construction.
                Err(_) => {
                    slots.push(FieldSlot::Prop(declared.name.name.clone()));
                    let node: bynk_ts::TsExpr = cx.missing_fact(
                        type_name.span,
                        format_args!("default value for field `{}`", declared.name.name),
                    );
                    children.push(node);
                }
            }
        }
    }
    let built = object_built(false, slots, children);
    pre.finish(built)
}

fn lower_field_access(e: &Expr, receiver: &Expr, field: &Ident, cx: &mut LowerCtx) -> Lowered {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    // v0.158 (ADR 0184): `<map>.entries` / `.keys` / `.values` on a `store
    // Map[K, V]` field — the key-exposing lazy queries. Like every store-map
    // query, they lower to a deferred thunk over the working record. `.values`
    // scans `Object.values` (the existing value lift); `.keys` scans
    // `Object.keys`; `.entries` zips both into `{ key, value }` records
    // (`MapEntry[K, V]`). A persisted key is a JS object key (a string), so an
    // `Int`-typed key is decoded back with `Number(...)`; a `String`-typed key
    // is already correct; a refined or opaque key is asserted to its branded
    // type (#1982).
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.is_agent_store_map(&id.name)
        && !cx.is_agent_held_map(&id.name)
        && matches!(
            field.name.as_str(),
            map_query::ENTRIES | map_query::KEYS | map_query::VALUES
        )
    {
        let m = member_node(cx.agent_store_var(), &id.name);
        let key = |cx: &LowerCtx| {
            decode_map_key(
                map_key_ty(e, cx),
                ident_node("__k"),
                tys,
                cx.fault_site(e.span),
            )
        };
        let is_identity =
            |k: &bynk_ts::TsExpr| matches!(k, bynk_ts::TsExpr::Ident(n) if n == "__k");
        let values = |call: &str| bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(m.clone()),
                property: call.to_string(),
            }),
            args: vec![],
        };
        let arrow = |params: Vec<&str>, body: bynk_ts::TsExpr| bynk_ts::TsExpr::Arrow {
            params: params
                .into_iter()
                .map(|p| bynk_ts::TsParam::new(p.to_string(), None))
                .collect(),
            is_async: false,
            generics: vec![],
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(body))),
        };
        let call = |f: &str, args: Vec<bynk_ts::TsExpr>| bynk_ts::TsExpr::Call {
            callee: Box::new(ident_node(f)),
            args,
        };
        let built: bynk_ts::TsExpr = match field.name.as_str() {
            map_query::VALUES => store_thunk(values("values")),
            map_query::KEYS => {
                let decoded = key(cx);
                if is_identity(&decoded) {
                    store_thunk(values("keys"))
                } else {
                    let map = bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Member {
                            object: Box::new(ident_node("__ks")),
                            property: "map".to_string(),
                        }),
                        args: vec![arrow(vec!["__k"], decoded)],
                    };
                    store_thunk(call(
                        "__bynkThen",
                        vec![values("keys"), arrow(vec!["__ks"], map)],
                    ))
                }
            }
            // entries: each `[__k, __v]` pair zipped into a `{ key, value }` record.
            _ => {
                let record = bynk_ts::TsExpr::Object {
                    entries: vec![
                        bynk_ts::TsObjectEntry::prop("key", key(cx)),
                        bynk_ts::TsObjectEntry::prop("value", ident_node("__v")),
                    ],
                    multiline: false,
                    end_span: None,
                };
                let pair = bynk_ts::TsExpr::Arrow {
                    params: vec![bynk_ts::TsParam::array_pattern(
                        vec!["__k".to_string(), "__v".to_string()],
                        None,
                    )],
                    is_async: false,
                    generics: vec![],
                    return_type: None,
                    body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(record))),
                };
                let zipped = bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(ident_node("__es")),
                        property: "map".to_string(),
                    }),
                    args: vec![pair],
                };
                store_thunk(call(
                    "__bynkThen",
                    vec![values("entries"), arrow(vec!["__es"], zipped)],
                ))
            }
        };
        return pre.finish(built);
    }
    // v0.9: `HttpResult.Variant` (nullary).
    //
    // P6.21/P6.23 (review of #1251): reads `Callee::Intrinsic` instead of
    // `id.name == HTTP_RESULT` — see the `MethodCall` sibling above (this
    // module's own qualified-with-args `HttpResult.Variant(args)` branch)
    // for the full reasoning.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic {
                ns: IntrinsicNs::HttpResult,
                ..
            })
        )
    {
        cx.note_http_result();
        return pre.finish(member_node("HttpResult", &field.name));
    }
    // #2073 (with #2045): a qualified nullary variant of a consumed context's
    // sum (`Region.Domestic`), on `workers`, is built inline as its bare form
    // is, so the callee's module stays a type-only import.
    if let ExprKind::Ident(id) = &receiver.kind
        && !cx.is_local(&id.name)
        && let Ok(built) = consumed_variant_literal(e, &id.name, &field.name, Vec::new(), cx)
    {
        return pre.finish(built);
    }
    // Agent-handler `self.<key>` rewrite.
    if cx.in_agent_handler()
        && let ExprKind::Ident(id) = &receiver.kind
        && id.name == "self"
        && let Some(k) = cx.agent_key_field()
        && field.name == *k
    {
        // #1818: the key this instance was addressed by. It used to read
        // `this.state.id.toString()` (the Durable Object's hex id on workers,
        // "[object Object]" on bundle) cast to the key's *name*.
        return pre.finish(member_node("this", "__key"));
    }
    // v0.45: `<binder>.identity` on a verified actor binding. The binder is not
    // a runtime value; the identity is minted at the verification seam. For the
    // zero-crypto schemes the sealed identity carries no payload, so it lowers
    // to the unit value (`undefined`). Authenticated identities (Bearer/
    // Signature) and the calling-context value arrive with their later slices.
    //
    // v0.47: a Bearer handler's identity is minted at the verification seam and
    // threaded through `deps`, so `<binder>.identity` reads `deps.identity`.
    // Other (unit) identities — `None`/`Internal` actors — carry no payload and
    // stay the unit value `undefined`.
    if field.name == "identity"
        && matches!(
            cx.commons().expr_ty(receiver.id).as_deref(),
            Some(Ty::Actor(_))
        )
    {
        if let (Some(binder), ExprKind::Ident(id)) = (cx.deps_identity_binder(), &receiver.kind)
            && id.name == binder
        {
            return pre.finish(member_node("deps", "identity"));
        }
        return pre.finish(ident_node("undefined"));
    }
    // #2380 (L5): an `Oidc` binder's `.claims` reads `deps.claims`, which its
    // seam filled from the verified token.
    if field.name == "claims"
        && matches!(
            cx.commons().expr_ty(receiver.id).as_deref(),
            Some(Ty::Actor(_))
        )
        && let (Some(binder), ExprKind::Ident(id)) = (cx.deps_identity_binder(), &receiver.kind)
        && id.name == binder
    {
        return pre.finish(member_node("deps", "claims"));
    }
    let r = pre.lower_node(receiver, cx);
    // `.raw` on an opaque value compiles to a TypeScript type
    // assertion back to the base type. The checker has already
    // verified that the receiver is opaque and the call site is
    // inside the defining commons.
    if field.name == RAW
        && let Some(Ty::Named {
            kind: NamedKind::Opaque(base),
            ..
        }) = cx.commons().expr_ty(receiver.id).as_deref()
    {
        let base = ts_base(*base);
        let built = {
            let c = vec![r];
            bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
                expr: Box::new(one(c)),
                ty: bynk_ts::TsType::named(base),
            }))
        };
        return pre.finish(built);
    }
    let built = {
        let c = vec![r];
        bynk_ts::TsExpr::Member {
            object: Box::new(one(c)),
            property: field.name.clone(),
        }
    };
    pre.finish(built)
}

/// v0.158 (ADR 0184): the key type `K` of a map `.entries`/`.keys` query, read
/// off the field-access result type — `Query[MapEntry[K, V]]` for `.entries`,
/// `Query[K]` for `.keys`. Used to decode persisted string object-keys back to
/// `K`. `None` only when the accessor's type was not recorded — which the
/// checker never leaves unset for a well-typed `.entries`/`.keys` (it stamps the
/// result type at `e.span`), so a `None` here is a checker/emitter invariant
/// break, not a runtime input. The `debug_assert!` in [`decode_map_key`] pins
/// that: the fallback identity decode must never be reached for an `Int` key,
/// which would silently emit a string where a `number` is expected.
fn map_key_ty(e: &Expr, cx: &LowerCtx) -> Option<TyId> {
    let tys = cx.ty_render();
    let Ty::Query(inner) = &*tys.get(cx.commons().expr_types.get(&e.id).map(|te| te.ty)?) else {
        return None;
    };
    if let Ty::Named { name, args, .. } = &*tys.get(*inner)
        && name == MAP_ENTRY
        && !args.is_empty()
    {
        Some(args[0])
    } else {
        Some(*inner)
    }
}

/// v0.158 (ADR 0184): decode a persisted map key (a JS object key, always a
/// string) back to its bynk type. Value-keyable keys are `Int`/`String` and
/// refinements/opaques over them (ADR 0110 D5); an `Int`-based key erases to
/// `number`, so it is parsed with `Number(...)`, while a `String`-based key is
/// already correct and passes through unchanged. A refined or opaque key is
/// then asserted back to its branded type (#1982).
///
/// A key type the checker did not record, or one that is not value-keyable, is a
/// missing fact (1A.34): the identity pass-through that used to stand in would
/// silently emit a string for an `Int` key.
fn decode_map_key(
    k: Option<TyId>,
    raw: bynk_ts::TsExpr,
    tys: TyRender<'_>,
    at: FaultSite<'_>,
) -> bynk_ts::TsExpr {
    let (base, branded) = match k.map(|t| tys.get(t)).as_deref() {
        Some(Ty::Base(b)) => (Some(*b), false),
        Some(Ty::Named {
            kind: NamedKind::Refined(b) | NamedKind::Opaque(b),
            ..
        }) => (Some(*b), true),
        _ => (None, false),
    };
    // #1982: a refined or opaque key is a branded TS type (`Sku = string & {
    // readonly __brand: "Sku" }`), which neither the raw string nor its
    // `Number(...)` is. The key was that type when it was put, and its storage
    // is the base value, so asserting it back is sound.
    let brand = |decoded: bynk_ts::TsExpr| match (branded, k) {
        (true, Some(k)) => bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
            expr: Box::new(decoded),
            ty: ts_ty_to_ts_type(k, tys),
        })),
        _ => decoded,
    };
    match base {
        Some(BaseType::Int) => brand(bynk_ts::TsExpr::Call {
            callee: Box::new(member_node("globalThis", "Number")),
            args: vec![raw],
        }),
        Some(BaseType::String) => brand(raw),
        // #70/#71 review: `resolver.rs`'s `check_map_key_keyable` and
        // `validate.rs`'s `@indexed`-field check both gate value-keyability
        // (ADR 0110 D5) on exactly `String`/`Int` — the same pair this match
        // now names explicitly. If either of those independent copies ever
        // widens the accepted set without this one being taught the new
        // type's wire shape, silently falling through to bare `raw` would
        // *miscompile* rather than reject — e.g. a `Bool` key needs
        // `raw === "true"`, not the raw string. Fail loudly instead.
        // Fail loudly instead (1A.34: report it).
        Some(other) => at.missing_fact(format_args!(
            "value-keyable map key type: {other:?} is not (ADR 0110 D5)"
        )),
        None => at.missing_fact("value-keyable map key type for `.entries`/`.keys`"),
    }
}

#[cfg(test)]
mod decode_map_key_tests {
    use super::*;
    use crate::emitter::ts_types::no_spellings;

    /// One table for this module's fixtures — `Types` is `Send + Sync`
    /// (T3.6b), so a `LazyLock` static serves every test without threading.
    static TYS: std::sync::LazyLock<Arc<Types>> =
        std::sync::LazyLock::new(|| Arc::new(Types::new()));

    fn decoded(k: Option<TyId>) -> (String, Vec<crate::emitter::InternalFault>) {
        let ru = RuntimeUse::default();
        let at = FaultSite {
            sink: &ru,
            span: bynk_syntax::span::Span::new(3, 9),
        };
        let node = decode_map_key(k, ident_node("k"), TyRender::new(&TYS, no_spellings()), at);
        (bynk_ts::print_expr(&node), ru.take_faults())
    }

    fn print_decoded(k: Option<TyId>) -> String {
        let (text, faults) = decoded(k);
        assert!(faults.is_empty(), "{faults:?}");
        text
    }

    #[test]
    fn int_key_is_parsed_with_number() {
        let int = TYS.intern(Ty::Base(BaseType::Int));
        assert_eq!(print_decoded(Some(int)), "globalThis.Number(k)");
    }

    #[test]
    fn string_key_passes_through() {
        let string = TYS.intern(Ty::Base(BaseType::String));
        assert_eq!(print_decoded(Some(string)), "k");
    }

    #[test]
    fn refined_int_key_is_parsed_with_number() {
        let refined = TYS.intern(Ty::Named {
            name: "UserId".to_string(),
            kind: NamedKind::Refined(BaseType::Int),
            args: Vec::new(),
        });
        assert_eq!(
            print_decoded(Some(refined)),
            "(globalThis.Number(k) as UserId)"
        );
    }

    #[test]
    fn opaque_string_key_is_asserted_to_its_brand() {
        // #1982: `Object.keys` yields `string`, not the branded `Slug`.
        let opaque = TYS.intern(Ty::Named {
            name: "Slug".to_string(),
            kind: NamedKind::Opaque(BaseType::String),
            args: Vec::new(),
        });
        assert_eq!(print_decoded(Some(opaque)), "(k as Slug)");
    }

    // #70/#71 review: a non-`String`/`Int` base reaching here means the
    // value-keyable set widened elsewhere (`resolver.rs`/`validate.rs`)
    // without teaching this decode the new type's wire shape — it must be
    // reported, not silently emit a wrong pass-through (1A.34).
    #[test]
    fn a_widened_non_string_int_base_is_reported_instead_of_silently_passing_through() {
        let b = TYS.intern(Ty::Base(BaseType::Bool));
        let (text, faults) = decoded(Some(b));
        assert_eq!(text, "undefined");
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert!(
            faults[0].what.contains("is not (ADR 0110 D5)"),
            "{faults:?}"
        );
    }

    /// The key type is always recorded for a well-typed `.entries`/`.keys`; one that is not is a
    /// missing fact at the call's span, and no identity decode stands in for it.
    #[test]
    fn an_unrecorded_key_type_is_reported_at_the_call() {
        let (text, faults) = decoded(None);
        assert_eq!(text, "undefined");
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert_eq!(faults[0].span, bynk_syntax::span::Span::new(3, 9));
    }
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

fn lower_lambda(e: &Expr, lambda: &LambdaExpr, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let tys = cx.ty_render();
    let is_async = matches!(
        cx.commons().expr_ty(e.id).as_deref(),
        Some(bynk_check::checker::Ty::Fn { ret, .. }) if ret.is_effect(tys.types())
    );
    // #2712: a record-pattern parameter has no annotation to print; its type is
    // the checked function type's, so a lambda with nothing to type it from
    // (held in a `let`) still compiles under `tsc --strict`. #3228: the same
    // holds for any unannotated parameter, which TypeScript can type from
    // context only when the lambda is handed straight to a typed parameter: a
    // spread element (`[...acc, (x) => …]`) or an immediately invoked arrow
    // (`((x) => …)(__x)`) gives it none, and `--strict` rejects the implicit
    // `any` (TS7006). So every parameter is annotated with its checked type.
    let fn_params = match cx.commons().expr_ty(e.id).as_deref() {
        Some(bynk_check::checker::Ty::Fn { params, .. }) => params.clone(),
        _ => Vec::new(),
    };
    let params: Vec<bynk_ts::TsParam> = lambda
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ty = match (&p.type_ref, &p.pattern, fn_params.get(i)) {
                (Some(tr), _, _) => Some(ts_type_ref_to_ts_type(tr, None)),
                (None, _, Some(ty)) => Some(ts_ty_to_ts_type(*ty, tys)),
                _ => None,
            };
            bynk_ts::TsParam {
                name: ts_ident(&p.name.name),
                ty,
                optional: false,
                rest: false,
                array_pattern: None,
                default: None,
            }
        })
        .collect();
    // #908: a param's name is the lambda body's own binding — push a frame and
    // register each so a body read resolves to the param rather than an
    // outer `let` rename, popped once the body is lowered so the
    // registration doesn't leak past the lambda.
    cx.shadow_scopes.push(HashMap::new());
    cx.lambda_depth += 1;
    for p in &lambda.params {
        cx.declare_binder(&p.name.name);
    }
    // v0.154 (ADR 0178): a lambda is its own return scope — a `return` in its
    // body (from a `?`) exits the arrow, not the enclosing function. Rebind
    // `return_ty` to the lambda's own return (its `Fn` type's `ret`) so an
    // embedding `?` anywhere in the body — block-bodied or expression-bodied —
    // uses that, not the outer one; restore after either form.
    let lam_ret = match cx.commons().expr_ty(e.id).as_deref() {
        Some(bynk_check::checker::Ty::Fn { ret, .. }) => Some(*ret),
        _ => None,
    };
    let saved = cx.return_ty.take();
    cx.return_ty = lam_ret;
    // #1750: the lambda's own tails `return` from the lambda.
    let saved_slot = cx.tail_slot.take();
    // #1764 review: an enclosing short-circuit's temp hoist stops here. A
    // receiver temp inside the lambda is read only inside it, and hoisting its
    // declaration out would share one slot between the lambda's invocations.
    let saved_hoist = cx.is_temp_hoist.take();
    // A block body or an expression body: one shape tested, nothing to decide per construct.
    let result: bynk_ts::TsExpr = if let ExprKind::Block(b) = &lambda.body.kind {
        // The body keeps its marks (1A.15, Decision D).
        let body = block_body(b, cx, is_async);
        lambda_node(
            params,
            is_async,
            bynk_ts::TsArrowBody::Multiline(body.into_stmts()),
        )
    } else {
        let lowered = lower_expr(&lambda.body, cx);
        let hoisted = lowered.pre;
        let value = lowered.expr;
        let body = if hoisted.is_empty() {
            // An object-literal body is bracketed by the printer.
            bynk_ts::TsArrowBody::Expr(Box::new(value))
        } else {
            let mut stmts: Vec<bynk_ts::TsStmt> = hoisted.into_iter().collect();
            stmts.push(bynk_ts::TsStmt::return_stmt(Some(value), None));
            bynk_ts::TsArrowBody::Multiline(stmts)
        };
        lambda_node(params, is_async, body)
    };
    cx.is_temp_hoist = saved_hoist;
    cx.tail_slot = saved_slot;
    cx.return_ty = saved;
    cx.lambda_depth -= 1;
    cx.shadow_scopes.pop();
    result
}

/// A lambda as an arrow node (1A.12).
fn lambda_node(
    params: Vec<bynk_ts::TsParam>,
    is_async: bool,
    body: bynk_ts::TsArrowBody,
) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Arrow {
        params,
        is_async,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(body),
    }
}

fn lower_record_spread(base: &Expr, overrides: &[FieldInit], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    let mut children = vec![pre.lower_node(base, cx)];
    let (slots, field_children) = field_inits(overrides, &mut pre, cx);
    children.extend(field_children);
    let built = object_built(true, slots, children);
    pre.finish(built)
}

fn lower_block_as_expr(b: &Block, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    // #1750: the arrow's own tails `return` from the arrow.
    let saved_slot = cx.tail_slot.take();
    // IIFE is a synchronous arrow function; the surrounding expression context
    // expects a concrete value, so `Effect.pure(...)` must still wrap as
    // `Promise.resolve(...)`.
    // v0.154 (ADR 0178): a `return` here exits the arrow, not the function, so
    // clear `return_ty`. #1750: a block holding a `?` no longer reaches here
    // (`hoist_value_as_statement`).
    let saved = cx.return_ty.take();
    // The body keeps its marks (1A.15, Decision D).
    let body = block_body(b, cx, false);
    cx.return_ty = saved;
    cx.tail_slot = saved_slot;
    iife_call(false, body.into_stmts())
}

/// #1 review: the discriminant's hoisted statements (`pre`) are the
/// caller's own — pushed straight into whatever scope is lowering this
/// `match` expression, exactly like every other `ExprKind` arm in
/// `lower_expr_into`. This used to lower the discriminant into a *local*,
/// throwaway vector and — when non-empty — splice it into a freshly
/// synthesised wrapper arrow via `(() => { ...stmts; return inner; })()`. A
/// `?` in the discriminant hoists `if (__rN.tag === "Err") return __rN;`; a
/// `return` inside that wrapper arrow exits the wrapper, not the enclosing
/// function, so the early return's `Err` silently became the whole match
/// expression's *value* instead of propagating out — a miscompile with no
/// diagnostic and no assertion, unlike its two ternary-path siblings (see
/// `lower_if`/`lower_tail_expr`, which at least `debug_assert!`ed the
/// analogous case before this same class of fix).
fn lower_match_as_iife(discriminant: &Expr, arms: &[MatchArm], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    let disc_ty = cx
        .commons()
        .expr_types
        .get(&discriminant.id)
        .map(|te| te.ty);
    let disc = pre.lower_node(discriminant, cx);
    // v0.154 (ADR 0178): a value-position `match` lowers to an IIFE, whose
    // `return`s exit the arrow, so clear `return_ty`. #1750: a `match` whose
    // arms hold a `?` no longer reaches here (`hoist_value_as_statement`).
    let saved = cx.return_ty.take();
    // #1750: the arrow's own tails `return` from the arrow.
    let saved_slot = cx.tail_slot.take();
    // T2.2 (R6.4): isolate the effectfulness flag to just this IIFE's own
    // body — `disc` was already lowered above, so any await it needed landed
    // in `pre`, not here. Restore as `saved_await || needs_async` so an
    // `await (async …)()` this call produces (if it does) still marks the
    // *caller's* scope — the propagation the old text scan got for free from
    // the substring surviving in the returned string.
    let saved_await = std::mem::take(&mut cx.emitted_await);
    // The arrow's body keeps its marks (1A.15, Decision D).
    let mut body = Body::new();
    match_dispatch(&mut body, &PatPath::ident("__d"), &disc_ty, arms, cx, false);
    let needs_async = cx.emitted_await;
    cx.emitted_await = saved_await || needs_async;
    cx.return_ty = saved;
    cx.tail_slot = saved_slot;
    let params = vec![bynk_ts::TsParam::new("__d".to_string(), None)];
    pre.finish(arrow_call(
        params,
        vec![disc],
        needs_async,
        body.into_stmts(),
    ))
}

/// A literal pattern's value as a node (1A.13, Decision C of #3714): the twin of
/// [`literal_case_label`], which prints byte for byte the same (`TsLit::Str`
/// shares `escape_ts_string`'s escaper, `TsLit::Num` is verbatim).
pub(super) fn literal_label_node(value: &LiteralValue) -> bynk_ts::TsExpr {
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
pub(super) struct PatPath {
    node: bynk_ts::TsExpr,
}

impl PatPath {
    /// A bare local.
    pub(super) fn ident(name: &str) -> Self {
        PatPath {
            node: ident_node(name),
        }
    }

    /// The path a lowered scrutinee is read at.
    pub(super) fn from_lowered(value: bynk_ts::TsExpr) -> Self {
        PatPath { node: value }
    }

    pub(super) fn node(&self) -> &bynk_ts::TsExpr {
        &self.node
    }

    /// `path.property`.
    pub(super) fn member(&self, property: &str) -> Self {
        PatPath {
            node: member_node_of(&self.node, property),
        }
    }

    /// `path[i]`.
    pub(super) fn index(&self, i: usize) -> Self {
        PatPath {
            node: bynk_ts::TsExpr::Index {
                object: Box::new(self.node.clone()),
                index: Box::new(num_lit(i.to_string())),
            },
        }
    }
}

/// `path <op> rhs`.
fn path_test(path: &PatPath, op: bynk_ts::TsBinaryOp, rhs: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Binary {
        op,
        left: Box::new(path.node.clone()),
        right: Box::new(rhs),
    }
}

/// `a && b && …`: a left-leaning `And` chain. `tests` is not empty.
pub(super) fn and_join(tests: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
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
pub(super) fn pattern_tests(
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
            for p in &predicate.predicates {
                cx.note_predicate(&p.kind);
            }
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
) -> Option<String> {
    let pattern = pattern?;
    let tests: Vec<String> = pattern
        .fields
        .iter()
        .map(|(name, value)| match value {
            EventPatternValueIr::Const(ConstVal::Int(n)) => format!("{path}.{name} === {n}"),
            EventPatternValueIr::Const(ConstVal::Str(s)) => {
                format!("{path}.{name} === \"{}\"", escape_ts_string(s))
            }
            EventPatternValueIr::Const(ConstVal::Bool(b)) => format!("{path}.{name} === {b}"),
            EventPatternValueIr::Const(
                ConstVal::Float(_) | ConstVal::DurationMillis(_) | ConstVal::Unit,
            ) => {
                unreachable!(
                    "EventPatternValueIr::Const only ever holds Int/Str/Bool (its own doc \
                     comment) — the checker's own check_event_pattern already restricted a \
                     from Events(...) filter to that closed set before this pattern could exist"
                )
            }
            EventPatternValueIr::Variant { tag } => format!("{path}.{name}.tag === \"{tag}\""),
        })
        .collect();
    Some(tests.join(" && "))
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
            event_pattern_guard_ir("e", Some(&pattern)),
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

/// Emit `const` declarations binding the names in `pattern` from runtime `path`,
/// recursing through nested payloads (ADR 0169). Each is a `Const` node when
/// the path is one (1A.13, Decision C of #3714), else its text line; an
/// or-pattern's `let` and `if`/`else` dispatch stays text lines.
pub(super) fn emit_pattern_bindings(
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
                        bool_node(true)
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
fn or_pattern_decl(names: Vec<String>) -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::let_list(
        names.into_iter().map(bynk_ts::TsDeclarator::new).collect(),
        None,
    )
}

/// One alternative of an or-pattern's dispatch: its test (none for the last, the bare
/// `else`) and the shared names it assigns, each from the path the alternative binds it at.
struct OrArm {
    cond: Option<bynk_ts::TsExpr>,
    assigns: Vec<(String, bynk_ts::TsExpr)>,
}

/// `recv.property` over a node receiver.
fn member_node_of(recv: &bynk_ts::TsExpr, property: &str) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Member {
        object: Box::new(recv.clone()),
        property: property.to_string(),
    }
}

/// `if (c0) { a = p; } else if (c1) { … } else { … }`.
fn or_dispatch_stmt(arms: Vec<OrArm>) -> bynk_ts::TsStmt {
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
fn list_rest_value(path: &PatPath, n: usize) -> bynk_ts::TsExpr {
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

fn lower_is(value: &Expr, pattern: &Pattern, cx: &mut LowerCtx) -> Lowered {
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
        for p in refinement.iter().flat_map(|r| &r.predicates) {
            cx.note_predicate(&p.kind);
        }
        return pre.finish(refined_check_as_bool(recv, base, refinement.as_ref()));
    }
    let path = PatPath::from_lowered(pre.absorb_node(cx.is_receiver_ref(value)));
    let scrut_ty = cx.commons().expr_types.get(&value.id).map(|te| te.ty);
    let tested: bynk_ts::TsExpr = match pattern {
        // A bare name binding after `is` matches anything over a sum (ADR 0169).
        Pattern::Wildcard(_) | Pattern::Binding(_) => bool_node(true),
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
                bool_node(true)
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
                body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(ident_node("__c")))),
            }))),
            args: vec![tested],
        });
    }
    pre.finish(tested)
}

/// v0.13: render a refined type's predicates as a single boolean expression over
/// `recv`, for `value is RefinedType`. Mirrors `emit_pred_check`'s per-predicate
/// logic but as `&&`-joined terms instead of `Result`-returning statements.
fn refined_check_as_bool(
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
                PredKind::InRange(..) | PredKind::InRangeF(..) if base != BaseType::Decimal => {
                    TsExpr::Paren(Box::new(cond))
                }
                PredKind::Matches(..)
                | PredKind::InRange(..)
                | PredKind::InRangeF(..)
                | PredKind::InRangeD(..)
                | PredKind::MinLength(..)
                | PredKind::MaxLength(..)
                | PredKind::Length(..)
                | PredKind::NonNegative
                | PredKind::Positive
                | PredKind::NonEmpty
                | PredKind::IsoDate => cond,
            });
        }
    }
    let mut terms = terms.into_iter();
    let Some(first) = terms.next() else {
        return bool_node(true);
    };
    let joined = terms.fold(first, |left, right| TsExpr::Binary {
        op: TsBinaryOp::And,
        left: Box::new(left),
        right: Box::new(right),
    });
    TsExpr::Paren(Box::new(joined))
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
mod idempotency_scoping_tests {
    use super::*;
    use crate::emitter::ctx::{BodyMode, HandlerShared, ModuleCtx};
    use crate::emitter::lower_call::platform::scope_idempotency_key;
    use bynk_check::checker::TypedCommons;

    // P6.38 (design/tracks/the-ir.md §6a): reads `TypedCommons::empty()`
    // (bynk-check) instead of hand-building an empty `Commons` here — this
    // module's own last `bynk_syntax::ast` dependency, now gone.
    fn empty_commons() -> TypedCommons {
        TypedCommons::empty()
    }

    /// #934 (Decision C): a first-party `Idempotency.dedup`/`remember` call
    /// reaching `scope_idempotency_key` with no `handler_scope` set means some
    /// emission site populated `capabilities` without also wiring the scope —
    /// a compiler bug. This must fail loudly, not silently ship an unscoped
    /// key; nothing in the fixture suite exercises a missing scope (every real
    /// site sets it), so this is the only proof the guard actually fires.
    #[test]
    #[should_panic(expected = "handler_scope")]
    fn missing_handler_scope_panics() {
        let commons = empty_commons();
        let cross_context = bynk_check::resolver::CrossContextInfo::default();
        let runtime_use = RuntimeUse::default();
        let cx = LowerCtx::new(
            ModuleCtx::new(
                &commons,
                &cross_context,
                &runtime_use,
                UnitFacts::detached(),
            ),
            BodyMode::Method,
        );
        let mut args = vec![ident_node("orderId")];
        scope_idempotency_key(true, "dedup", &mut args, &cx);
    }

    /// The mirror case: a real scope present, the call proceeds and the key
    /// is prefixed — confirms the panic in the test above is really guarding
    /// the `None` case and not firing unconditionally.
    #[test]
    fn present_handler_scope_prefixes_the_key() {
        let commons = empty_commons();
        let cross_context = bynk_check::resolver::CrossContextInfo::default();
        let runtime_use = RuntimeUse::default();
        let cx = LowerCtx::new(
            ModuleCtx::new(
                &commons,
                &cross_context,
                &runtime_use,
                UnitFacts::detached(),
            ),
            BodyMode::ServiceHandler {
                handler: HandlerShared {
                    handler_scope: Some("shop.reserve.ordering.call".to_string()),
                    ..HandlerShared::default()
                },
                deps_identity_binder: None,
                actor_sum_binder: None,
            },
        );
        let mut args = vec![ident_node("orderId")];
        scope_idempotency_key(true, "dedup", &mut args, &cx);
        assert_eq!(
            args[0].clone().printed(),
            "`shop.reserve.ordering.call::${orderId}`"
        );
    }

    /// `claim` takes its key first, like `dedup` and `remember`, so it is
    /// scoped to the calling handler the same way.
    #[test]
    fn claim_is_scoped_like_dedup() {
        let commons = empty_commons();
        let cross_context = bynk_check::resolver::CrossContextInfo::default();
        let runtime_use = RuntimeUse::default();
        let cx = LowerCtx::new(
            ModuleCtx::new(
                &commons,
                &cross_context,
                &runtime_use,
                UnitFacts::detached(),
            ),
            BodyMode::ServiceHandler {
                handler: HandlerShared {
                    handler_scope: Some("shop.hooks.api.POST".to_string()),
                    ..HandlerShared::default()
                },
                deps_identity_binder: None,
                actor_sum_binder: None,
            },
        );
        let mut args = vec![ident_node("id"), num_lit("3600000".to_string())];
        scope_idempotency_key(true, "claim", &mut args, &cx);
        assert_eq!(args[0].clone().printed(), "`shop.hooks.api.POST::${id}`");
        assert_eq!(args[1].clone().printed(), "3600000");
    }

    /// The declaration node prints the very text it replaced (1A.35a part B), token for token
    /// and in its one-line layout.
    #[test]
    fn the_claim_scope_declaration_node_prints_its_old_text() {
        assert_eq!(
            bynk_ts::print_stmt(&claim_scope_declaration(), 0),
            format!("{CLAIM_SCOPE_DECLARATION}\n")
        );
    }

    /// A provider with no `__track` gets a scope that takes a claim for its
    /// whole window, so nothing is cut to the lease with no settle to extend it.
    #[test]
    fn a_provider_without_a_scope_takes_the_full_window() {
        let before = CLAIM_SCOPE_DECLARATION;
        assert!(before.contains("lease: (t: number) => t"), "{before}");
        assert!(before.contains("__track?.() ??"), "{before}");
    }

    /// A capability merely named `"Idempotency"` from some other adapter or
    /// context (not first-party `bynk`) must pass through untouched even with
    /// no scope set — `is_first_party: false` short-circuits before the
    /// `handler_scope` check, so this must NOT panic.
    #[test]
    fn non_first_party_capability_is_not_scoped() {
        let commons = empty_commons();
        let cross_context = bynk_check::resolver::CrossContextInfo::default();
        let runtime_use = RuntimeUse::default();
        let cx = LowerCtx::new(
            ModuleCtx::new(
                &commons,
                &cross_context,
                &runtime_use,
                UnitFacts::detached(),
            ),
            BodyMode::Method,
        );
        let mut args = vec![ident_node("orderId")];
        scope_idempotency_key(false, "dedup", &mut args, &cx);
        assert_eq!(args[0].clone().printed(), "orderId");
    }
}

/// T2.2 (#1018, R6.4): `maybe_async_iife`'s `if !iife.contains("await ")`
/// replaced by `LowerCtx::emitted_await`, a flag set at the two statement
/// sites that emit a literal `await` (`EffectLet`, `Do`) and read-and-reset
/// around a value-position `match`/`if` IIFE's own body construction — see
/// `finish_async_iife`. Same precedent as `conditional_runtime_import_tests`
/// (`body.contains("__bynkBytes")` → `RuntimeUse`): the scan and the flag
/// agree on every case except where the scan's substring match wasn't really
/// about *this* scope's own effectfulness.
#[cfg(test)]
mod async_iife_effectfulness_tests {
    use crate::testkit::emit_source;

    /// The over-match this slice closes: a `Query`/broadcast iterator terminal
    /// (`forEach`) lowers to a self-contained, always-`async () => {...}` IIFE
    /// — a plain `Effect`-typed *value*, not something this arm itself awaits.
    /// The old scan saw the literal `"await "` inside that embedded string and
    /// wrongly wrapped the enclosing `match` arrow as `async` too, producing a
    /// spurious `await await (async (__d) => …)()`. Neither arm needs the
    /// switch's own arrow to be async, so it must stay a plain `(__d) => {…}`,
    /// invoked with a single `await` from the enclosing `let r <- …`.
    #[test]
    fn match_arm_iterator_terminal_does_not_force_the_switch_arrow_async() {
        let ts = emit_source(
            "commons t\n\n\
             fn noop(n: Int) -> Effect[()] {\n  Effect.pure(())\n}\n\n\
             fn run(names: List[Int], flag: Bool) -> Effect[()] {\n  \
             let r <- match flag {\n    \
             true => names.forEach(noop)\n    \
             false => Effect.pure(())\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("const r = await ((__d) => {"),
            "the switch arrow must stay synchronous, awaited once by the `let <-`: {ts}"
        );
        assert!(!ts.contains("await await"), "{ts}");
    }

    /// The same over-match, reached through the `if`-IIFE path (an `is`-binding
    /// forces the IIFE form rather than a ternary) instead of `match`.
    #[test]
    fn if_arm_iterator_terminal_does_not_force_the_iife_arrow_async() {
        let ts = emit_source(
            "commons t\n\n\
             fn noop(n: Int) -> Effect[()] {\n  Effect.pure(())\n}\n\n\
             fn run(o: Option[Int], names: List[Int]) -> Effect[()] {\n  \
             let r <- if o is Some(n) {\n    \
             names.forEach(noop)\n  \
             } else {\n    \
             Effect.pure(())\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("const r = await (() => {"),
            "the if-IIFE arrow must stay synchronous, awaited once by the `let <-`: {ts}"
        );
        assert!(!ts.contains("await await"), "{ts}");
    }

    /// Non-regression: the defect `#2 review` fixed in `8068c0db` (a genuinely
    /// effectful `is`-binding `if` in value position) must still emit a real
    /// `async` IIFE — this is not an over-match, so it keeps needing the wrap
    /// that closed the original hard `SyntaxError`.
    #[test]
    fn if_arm_genuine_effect_still_forces_the_iife_arrow_async() {
        let ts = emit_source(
            "commons t\n\n\
             fn fetch(n: Int) -> Effect[Int] {\n  Effect.pure(n)\n}\n\n\
             fn run(o: Option[Int], c: Bool) -> Effect[Int] {\n  \
             let r <- if o is Some(n) && c {\n    \
             let y <- fetch(n)\n    \
             Effect.pure(y + 1)\n  \
             } else {\n    \
             Effect.pure(0)\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("async () => {") && ts.contains("await fetch(n)"),
            "a real await inside the IIFE must still force the async wrap: {ts}"
        );
    }

    /// Non-regression: a genuinely effectful *nested* `match` (embedded
    /// directly as an outer arm's value, not via an intermediate `let <-`)
    /// must still propagate its own async-wrap need to the enclosing switch
    /// arrow — the flag's `saved || needs_async` restore is what replaces the
    /// old scan's implicit propagation (the substring surviving in the
    /// returned string).
    #[test]
    fn nested_match_effectfulness_propagates_to_the_outer_switch_arrow() {
        let ts = emit_source(
            "commons t\n\n\
             fn fetch(n: Int) -> Effect[Int] {\n  Effect.pure(n)\n}\n\n\
             fn run(a: Bool, b: Bool) -> Effect[Int] {\n  \
             let r <- match a {\n    \
             true => match b {\n      \
             true => {\n        let x <- fetch(1)\n        Effect.pure(x + 1)\n      }\n      \
             false => Effect.pure(2)\n    \
             }\n    \
             false => Effect.pure(3)\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("const r = await await (async (__d) => {"),
            "the outer switch arrow must also become async: {ts}"
        );
    }

    /// Non-regression, mirroring the `EffectLet` case above for the *other*
    /// write site: an arm whose only effect is a `do` (no binder) must still
    /// force its switch arrow async. The old scan was blind to which
    /// statement produced the `await`, so `Do` came along for free; the flag
    /// has to name it explicitly at its own site (`emit_statement`'s
    /// `Statement::Do` arm) and nothing else pins that it does.
    #[test]
    fn match_arm_do_only_effect_still_forces_the_switch_arrow_async() {
        let ts = emit_source(
            "commons t\n\n\
             fn noop(n: Int) -> Effect[()] {\n  Effect.pure(())\n}\n\n\
             fn run(flag: Bool) -> Effect[()] {\n  \
             let r <- match flag {\n    \
             true => {\n      do noop(1)\n      Effect.pure(())\n    }\n    \
             false => Effect.pure(())\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("const r = await await (async (__d) => {"),
            "a do-only effectful arm must also force the switch arrow async: {ts}"
        );
    }
}

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
            pre: vec![bynk_ts::TsStmt::expr_stmt(ident_node("t"), None)],
            expr: ident_node("x"),
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

    fn id(n: &str) -> bynk_ts::TsExpr {
        bynk_ts::TsExpr::Ident(n.to_string())
    }

    fn bin(op: bynk_ts::TsBinaryOp, l: &str, r: &str) -> bynk_ts::TsExpr {
        bynk_ts::TsExpr::Binary {
            op,
            left: Box::new(id(l)),
            right: Box::new(id(r)),
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
        let not_not = t_unary_op(UnaryOp::Not, t_unary_op(UnaryOp::Not, node(id("x"))));
        assert_eq!(not_not.printed(), "!!x");
        let neg_not = t_unary_op(UnaryOp::Neg, t_unary_op(UnaryOp::Not, node(id("x"))));
        assert_eq!(neg_not.printed(), "-!x");
        let neg_neg = t_unary_op(UnaryOp::Neg, t_unary_op(UnaryOp::Neg, node(id("x"))));
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
        let ok = t_runtime_ctor_call(None, "Ok", node(num_lit("1".into())));
        assert_eq!(ok.printed(), "Ok(1)");
        let http = t_runtime_ctor_call(Some("HttpResult"), "Ok", node(id("v")));
        let bynk_ts::TsExpr::Call { callee, .. } = &http else {
            panic!("expected a call node");
        };
        assert!(matches!(**callee, bynk_ts::TsExpr::Member { .. }));
        assert_eq!(http.printed(), "HttpResult.Ok(v)");
        assert_eq!(
            t_runtime_ctor_call(None, "Err", node(id("e"))).printed(),
            "Err(e)"
        );
        assert_eq!(
            t_runtime_ctor_call(None, "Some", node(id("s"))).printed(),
            "Some(s)"
        );
        let list = array(vec![node(id("a")), node(num_lit("2".into()))]);
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
        let n = |s: &str| node(id(s));
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
        let ne = super::negated(super::helper_call("__bynkEq", vec![n("a"), n("b")]));
        assert!(matches!(
            &ne,
            bynk_ts::TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Not,
                ..
            }
        ));
        assert_eq!(ne.printed(), "!__bynkEq(a, b)");
        // `Ord`: `cmp(l, r) < 0`.
        let ord = super::ordered_comparison(BinOp::Lt, id("__cmp_P"), vec![n("a"), n("b")]);
        assert!(matches!(&ord, bynk_ts::TsExpr::Binary { .. }));
        assert_eq!(ord.printed(), "__cmp_P(a, b) < 0");
    }

    #[test]
    fn a_composite_comparator_is_a_node_over_nodes() {
        // 1A.16: the comparator is itself a node, so a composite one is an
        // ordinary callee (the 1A.5 text path is closed).
        let cmp = bynk_ts::TsExpr::Call {
            callee: Box::new(id("__bynkCmpList")),
            args: vec![id("__bynkCmpInt")],
        };
        let got = super::ordered_comparison(BinOp::Lt, cmp, vec![node(id("a")), node(id("b"))]);
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
        assert_eq!(
            bynk_ts::print_expr(&ident_node("__bynkRunId")),
            "__bynkRunId"
        );
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
            let node = str_node(raw);
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
            ident_node("c"),
            bynk_ts::TsStmt::block(
                vec![bynk_ts::TsStmt::assign(
                    ident_node("__r1"),
                    ident_node("a"),
                    None,
                )],
                None,
            ),
            bynk_ts::TsStmt::block(
                vec![bynk_ts::TsStmt::assign(
                    ident_node("__r1"),
                    ident_node("b"),
                    None,
                )],
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

#[cfg(test)]
mod compact_iife_tests {
    use super::*;

    fn multi_line_stmt() -> bynk_ts::TsStmt {
        bynk_ts::TsStmt::if_stmt(
            ident_node("flag"),
            bynk_ts::TsStmt::block(
                vec![bynk_ts::TsStmt::expr_stmt(ident_node("done"), None)],
                None,
            ),
            None,
        )
    }

    /// A hoisted statement that prints over several lines cannot sit in the compact `Block`
    /// body (the printer refuses it); the arrow takes the multi-line body instead.
    #[test]
    fn a_multi_line_hoist_takes_the_multi_line_body() {
        let iife = compact_iife(vec![multi_line_stmt()], ident_node("r"));
        let printed = bynk_ts::print_expr(&iife);
        assert!(printed.starts_with("(() => {\n"), "{printed}");
        assert!(printed.contains("if (flag) {"), "{printed}");
        assert!(printed.contains("return r;"), "{printed}");
    }

    /// One-line statements keep the compact one-line form the corpus has always had.
    #[test]
    fn one_line_hoists_stay_compact() {
        let stmt = bynk_ts::TsStmt::expr_stmt(ident_node("a"), None);
        let iife = compact_iife(vec![stmt], ident_node("r"));
        assert_eq!(bynk_ts::print_expr(&iife), "(() => { a; return r; })()");
    }
}

/// 1A.18a (#3719): the `List` kernel's expression-bodied arms build nodes.
#[cfg(test)]
mod list_kernel_tests {
    use crate::testkit::emit_source;

    /// The emitted TypeScript for a one-function commons.
    fn emitted(body: &str) -> String {
        emit_source(&format!("commons lk\n{body}\n"))
    }

    /// The returned expression of `export function f`.
    fn returned(ts: &str) -> String {
        let at = ts.find("export function f(").expect("f");
        let rest = &ts[at..];
        let ret = rest.find("  return ").expect("a return") + "  return ".len();
        let end = rest[ret..].find(";\n}\n").expect("end");
        rest[ret..ret + end].to_string()
    }

    /// Each arm over node operands prints exactly today's text.
    #[test]
    fn expression_arms_over_nodes_print_as_before() {
        for (body, want) in [
            ("fn f(xs: List[Int]) -> Int { xs.length() }", "(xs).length"),
            ("fn f(xs: List[Int]) -> Int { xs.count() }", "(xs).length"),
            (
                "fn f(xs: List[Int], i: Int) -> Option[Int] { xs.get(i) }",
                "((__xs: readonly number[], __i: number) => __i >= 0 && __i < __xs.length ? Some(__xs[__i] as number) : None)(xs, i)",
            ),
            (
                "fn f(xs: List[Int], y: List[Int]) -> List[Int] { xs.concat(y).append(1).prepend(0) }",
                "[0, ...[...[...xs, ...y], 1]]",
            ),
            (
                "fn f(xs: List[Int]) -> List[Int] { xs.map((x) => x + 1).filter((x) => x > 1).take(2).skip(1) }",
                "((((xs).map((__x: number) => ((x: number) => x + 1)(__x))).filter((__x: number) => ((x: number) => x > 1)(__x))).slice(0, globalThis.Math.max(0, 2))).slice(globalThis.Math.max(0, 1))",
            ),
            (
                "fn f(xs: List[Int]) -> Bool { xs.any((x) => x > 1) && xs.all((x) => x > 0) }",
                "(xs).some((__x: number) => ((x: number) => x > 1)(__x)) && (xs).every((__x: number) => ((x: number) => x > 0)(__x))",
            ),
            (
                "fn f(xs: List[Int]) -> List[Int] { xs.flatMap((x) => [x, x]) }",
                "(xs).flatMap((__x: number) => ((x: number) => [x, x])(__x))",
            ),
            (
                "fn f(xs: List[Int]) -> Option[Int] { xs.first() }",
                "((__xs: readonly number[]) => __xs.length > 0 ? Some(__xs[0]) : None)(xs)",
            ),
            (
                "fn f(xs: List[Int]) -> Int { xs.firstOrElse(0) }",
                "((__xs: readonly number[], __d: number) => __xs.length > 0 ? __xs[0] : __d)(xs, 0)",
            ),
            (
                "fn f(xs: List[Int]) -> Int { xs.sum((x) => x) }",
                "(xs).reduce((__s: number, __x: number) => __s + ((x: number) => x)(__x), 0)",
            ),
            (
                "fn f(xs: List[Int]) -> List[Int] { xs.distinct() }",
                "[...new globalThis.Set(xs)]",
            ),
            (
                "fn f(xs: List[Bytes]) -> List[Bytes] { xs.distinct() }",
                "__bynkDistinct(xs)",
            ),
            (
                "fn f(xs: List[Float]) -> List[Float] { xs.sortBy((x) => x) }",
                "[...xs].sort((__a: number, __b: number) => __bynkCmpFloat(((x: number) => x)(__a), ((x: number) => x)(__b)))",
            ),
            (
                "fn f(xs: List[Float]) -> List[Float] { xs.sortByDescending((x) => x) }",
                "[...xs].sort((__a: number, __b: number) => __bynkCmpFloat(((x: number) => x)(__b), ((x: number) => x)(__a)))",
            ),
        ] {
            let ts = emitted(body);
            assert_eq!(returned(&ts), want, "{body}");
        }
    }

    /// The `Decimal` sum starts from the zero decimal and notes its helper.
    #[test]
    fn a_decimal_sum_is_a_node_and_imports_its_helper() {
        let ts = emitted("fn f(xs: List[Decimal]) -> Decimal { xs.sum((x) => x) }");
        assert!(ts.contains("__bynkDecimalAdd(__s, ((x: { readonly coefficient: bigint; readonly scale: number }) => x)(__x)), { coefficient: 0n, scale: 0 })"), "{ts}");
        assert!(ts.contains("import { __bynkDecimalAdd }"), "{ts}");
    }

    /// The one returned expression of `f`, async or not.
    fn returned_any(ts: &str) -> String {
        let at = ts.find("function f(").expect("f");
        let rest = &ts[at..];
        let ret = rest.find("  return ").expect("a return") + "  return ".len();
        let end = rest[ret..].find(";\n}\n").expect("end");
        rest[ret..ret + end].to_string()
    }

    fn arm_source(elem: &str, ret: &str, extra: &str, expr: &str) -> String {
        let param = format!("xs: List[{elem}]");
        let body = expr.replace("{R}", "xs");
        format!("fn f({param}{extra}) -> {ret} {{ {body} }}")
    }

    /// The layout of the statement arms: a block-bodied arrow, one statement per
    /// line, an unbraced loop body inline, `} else {` on one line, and the
    /// accumulator's function type keeps its parameter names.
    #[test]
    fn statement_arms_print_as_multi_line_arrows() {
        for (elem, ret, extra, expr, want) in [
            (
                "Int",
                "Effect[Int]",
                ", g: (Int, Int) -> Effect[Int]",
                "{R}.foldEff(0, g)",
                "(async (__xs: readonly number[], __acc: number, __f: (acc: number, x: number) => globalThis.Promise<number>) => {
    for (const __x of __xs) __acc = await __f(__acc, __x);
    return __acc;
  })(xs, 0, g)",
            ),
            (
                "Result[Int, String]",
                "Result[List[Int], List[String]]",
                "",
                "{R}.sequenceAll()",
                "((__xs: readonly Result<number, string>[]) => {
    const __ok: number[] = [];
    const __errs: string[] = [];
    for (const __r of __xs) {
      if (__r.tag === \"Err\") {
        __errs.push(__r.error);
      } else {
        __ok.push(__r.value);
      }
    }
    return __errs.length > 0 ? Err(__errs) : Ok(__ok);
  })(xs)",
            ),
            (
                "Int",
                "List[Int]",
                "",
                "{R}.sortByDescending((x) => x)",
                "[...xs].sort((__a: number, __b: number) => {
    const __ka = ((x: number) => x)(__a);
    const __kb = ((x: number) => x)(__b);
    return __ka < __kb ? 1 : __ka > __kb ? -1 : 0;
  })",
            ),
            (
                "Int",
                "List[Int]",
                ", ys: List[Int]",
                "{R}.joinOn(ys, (x) => x, (y) => y, (x, y) => x + y)",
                "(() => {
    const __h: globalThis.Record<string, number[]> = globalThis.Object.create(null);
    for (const __u of ys) {
      const __k = String(((y: number) => y)(__u));
      (__h[__k] = __h[__k] ?? []).push(__u);
    }
    return (xs).flatMap((__t: number) => {
      const __m = __h[String(((x: number) => x)(__t))] ?? [];
      return __m.map((__u: number) => ((x: number, y: number) => x + y)(__t, __u));
    });
  })()",
            ),
        ] {
            let ts = emitted(&arm_source(elem, ret, extra, expr));
            assert_eq!(returned_any(&ts), want, "{expr}");
        }
    }

    /// The async arms stay async by construction and still import the settle
    /// helper that `par*` calls: `note_settle_all` fires as it did.
    #[test]
    fn par_arms_import_their_settle_helper() {
        let ts = emitted(&arm_source(
            "Int",
            "Effect[()]",
            ", g: (Int) -> Effect[()]",
            "{R}.parTraverse(g)",
        ));
        assert!(ts.contains("__bynkSettleAll"), "{ts}");
        assert!(
            ts.contains("(async (__xs: readonly number[]) => {\n"),
            "{ts}"
        );
    }

    /// Hoisted effects keep source order: the receiver's `?` lowers before the
    /// argument's for `append`/`concat`, after it for `prepend`.
    #[test]
    fn hoisted_operands_keep_source_order() {
        let src = "commons lk\n\
            fn p(x: Int) -> Result[Int, String] { Ok(x) }\n\
            fn f(xs: List[Int]) -> Result[List[Int], String] { Ok(xs.append(p(1)?).concat([p(2)?])) }\n\
            fn g(xs: List[Int]) -> Result[List[Int], String] { Ok(xs.prepend(p(3)?)) }\n";
        let ts = emit_source(src);
        let at = |n: &str| ts.find(n).unwrap_or_else(|| panic!("{n}\n{ts}"));
        let f = at("export function f(");
        let one = ts[f..].find("p(1)").unwrap();
        let two = ts[f..].find("p(2)").unwrap();
        assert!(one < two, "{ts}");
        assert!(
            ts.contains("return Ok([...[...xs, __r0.value], ...[__r1.value]]);"),
            "{ts}"
        );
        assert!(ts.contains("return Ok([__r0.value, ...xs]);"), "{ts}");
    }

    /// Decision G: the imports derive from the printed output, so a module using
    /// `get`, `first`, `sequenceAll` and `traverseTry` imports what it uses: the
    /// `Result` and `Option` names, and nothing from `errors` (1A.36b-1).
    #[test]
    fn imports_follow_the_printed_kernel_output() {
        let src = "commons lk\n\
            fn a(xs: List[Int]) -> Option[Int] { xs.get(0) }\n\
            fn a2(xs: List[Int]) -> Option[Int] { xs.first() }\n\
            fn b(xs: List[Result[Int, String]]) -> Result[List[Int], List[String]] { xs.sequenceAll() }\n\
            fn c(xs: List[Int]) -> Effect[Result[List[Int], String]] {\n\
              xs.traverseTry((x) => { let r: Result[Int, String] = Ok(x)\n  Effect.pure(r) })\n\
            }\n";
        let ts = emit_source(src);
        assert!(
            ts.contains(
                "import { Ok, Err, Some, None, type Result, type Option } from \"./__bynk_runtime/result.js\";"
            ),
            "{ts}"
        );
        assert_eq!(ts.matches("import ").count(), 1, "{ts}");
    }
}

#[cfg(test)]
mod leaf_node_tests {
    use super::*;
    use bynk_ts::{TsExpr, print_expr, print_stmt};

    fn ident(n: &str) -> TsExpr {
        TsExpr::Ident(n.to_string())
    }

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
