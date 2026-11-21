//! Self tail calls as loops (#3193, ADR pending).
//!
//! A direct self call in tail position does not grow the stack: the function
//! body is wrapped in a labelled `while (true)` and the call becomes a
//! parameter reassignment plus `continue`. The pieces:
//!
//! - [`plan`] decides, from the typed body alone, whether a function is
//!   rewritten and which expressions are its tail self calls. It is purely
//!   syntactic and conservative: a call it does not recognise stays an
//!   ordinary (recursive) call, which is always correct.
//! - [`emit_loop_body`] writes the loop around the ordinary body lowering.
//! - [`emit_continue`] is what the tail lowering calls, at the point where it
//!   would have written `return <call>;`, for an expression in the plan.
//!
//! Bindings are per iteration: the loop's reassigned variables (`__tcoN`) are
//! distinct from the names the body sees, which are fresh `const`s at the top
//! of each iteration, so a closure created in one iteration keeps that
//! iteration's values.

use std::collections::HashSet;
use std::rc::Rc;

use bynk_check::checker::{Callee, TypedCommons};
use bynk_syntax::ast::{Block, Expr, ExprId, ExprKind, FnDecl, FnName, MatchBody};

use super::lower::{LoweredExpr, Pre};
use super::lower_ctx::LowerCtx;
use super::lower_stmt::{Body, Item, assign_item, block_body_with_return, const_item};
use super::ts_types::{ts_ty, ts_type_ref};
use super::*;

/// The label of the loop; a `continue` to it passes through any emitted
/// `switch` or inlined block.
const LABEL: &str = "__tco";

/// One rebound parameter: the name the body sees and the loop variable that
/// carries its next value.
struct Slot {
    binder: String,
    var: String,
}

/// A function chosen for the loop rewrite.
pub(crate) struct TcoPlan {
    /// The tail self calls.
    calls: HashSet<ExprId>,
    /// The `if` and `match` expressions with a tail self call in one of their
    /// branches, at any depth: they must be emitted as statements, never as a
    /// ternary or an arrow, whose `continue` could not reach the loop.
    ifs: HashSet<ExprId>,
    /// The receiver (when the function has `self`) and the parameters, in
    /// argument order.
    slots: Vec<Slot>,
}

/// Decide whether `f` is rewritten. `None` when it has an `ensures` clause (a
/// loop has one frame, so the postcondition could only be checked for the
/// outermost call, which is weaker), or has no tail self call.
pub(crate) fn plan(f: &FnDecl, commons: &TypedCommons, tys: TyRender<'_>) -> Option<Rc<TcoPlan>> {
    if !f.ensures.is_empty() {
        return None;
    }
    let mut p = TcoPlan {
        calls: HashSet::new(),
        ifs: HashSet::new(),
        slots: Vec::new(),
    };
    walk_block(&f.body, f, commons, tys, &mut p);
    if p.calls.is_empty() {
        return None;
    }
    let mut binders: Vec<String> = Vec::new();
    if f.has_self {
        binders.push("self".to_string());
    }
    binders.extend(f.params.iter().map(|q| ts_ident(&q.name.name)));
    p.slots = binders
        .into_iter()
        .enumerate()
        .map(|(i, binder)| Slot {
            binder,
            var: format!("__tco{i}"),
        })
        .collect();
    Some(Rc::new(p))
}

/// Tail position is the body's final expression, propagated through `if`
/// branches, `match` arms and a block's final expression. Anything else (an
/// argument, `?`, a lambda, `par`, `let`, a wrapper) is not walked into.
/// Returns whether a tail self call was found below `e`.
fn walk_expr(
    e: &Expr,
    f: &FnDecl,
    commons: &TypedCommons,
    tys: TyRender<'_>,
    p: &mut TcoPlan,
) -> bool {
    match &e.kind {
        ExprKind::Call { .. } | ExprKind::MethodCall { .. } if is_self_call(e, f, commons, tys) => {
            p.calls.insert(e.id);
            true
        }
        ExprKind::If {
            then_block,
            else_block,
            ..
        } => {
            let a = walk_block(then_block, f, commons, tys, p);
            let b = walk_block(else_block, f, commons, tys, p);
            if a || b {
                p.ifs.insert(e.id);
            }
            a || b
        }
        ExprKind::Match { arms, .. } => {
            let mut any = false;
            for arm in arms {
                any |= match &arm.body {
                    MatchBody::Expr(e) => walk_expr(e, f, commons, tys, p),
                    MatchBody::Block(b) => walk_block(b, f, commons, tys, p),
                };
            }
            if any {
                p.ifs.insert(e.id);
            }
            any
        }
        _ => false,
    }
}

fn walk_block(
    b: &Block,
    f: &FnDecl,
    commons: &TypedCommons,
    tys: TyRender<'_>,
    p: &mut TcoPlan,
) -> bool {
    walk_expr(&b.tail, f, commons, tys, p)
}

fn same_fn(decl: &FnDecl, f: &FnDecl) -> bool {
    decl.span == f.span
        && match (&decl.name, &f.name) {
            (FnName::Free(a), FnName::Free(b)) => a.name == b.name,
            (
                FnName::Method {
                    type_name: ta,
                    method_name: ma,
                },
                FnName::Method {
                    type_name: tb,
                    method_name: mb,
                },
            ) => ta.name == tb.name && ma.name == mb.name,
            _ => false,
        }
}

/// A call's callee and the expressions it passes in slot order (the receiver
/// first for an instance method call), and its explicit type arguments.
fn call_parts<'e>(
    e: &'e Expr,
    commons: &'e TypedCommons,
) -> Option<(&'e Callee, Vec<&'e Expr>, &'e [bynk_syntax::ast::TypeRef])> {
    let (args, type_args, recv): (&Vec<Expr>, &[_], Option<&Expr>) = match &e.kind {
        ExprKind::Call {
            args, type_args, ..
        } => (args, type_args, None),
        ExprKind::MethodCall {
            receiver,
            args,
            type_args,
            ..
        } => (args, type_args, Some(receiver.as_ref())),
        _ => return None,
    };
    let callee = commons.callees.get(&e.id)?;
    let mut all: Vec<&Expr> = Vec::new();
    if matches!(callee, Callee::Method(..)) {
        all.push(recv?);
    }
    all.extend(args.iter());
    Some((callee, all, type_args))
}

/// The expressions a self call passes, in slot order, or `None` when `e` is
/// not a self call or not at the function's own instantiation.
fn self_call_args<'e>(
    e: &'e Expr,
    f: &FnDecl,
    commons: &'e TypedCommons,
    tys: TyRender<'_>,
) -> Option<Vec<&'e Expr>> {
    let (callee, all, type_args) = call_parts(e, commons)?;
    let (decl, witnesses) = match callee {
        Callee::Fn(d, w) | Callee::Static(d, w) | Callee::Method(d, w) => (d, w),
        _ => return None,
    };
    if !same_fn(decl, f) {
        return None;
    }
    let slots = usize::from(f.has_self) + f.params.len();
    if all.len() != slots {
        return None;
    }
    // A call at a different instantiation would need loop variables of a
    // different TypeScript type: leave it as a real call.
    let owner_params: Vec<String> = match &f.name {
        FnName::Method { type_name, .. } => commons
            .types
            .get(&type_name.name)
            .map(|t| t.type_params.iter().map(|p| p.name.name.clone()).collect())
            .unwrap_or_default(),
        FnName::Free(_) => Vec::new(),
    };
    let mut own: Vec<String> = owner_params.clone();
    own.extend(f.type_params.iter().map(|p| p.name.name.clone()));
    if !own.is_empty() {
        let rendered = |ex: &Expr| commons.expr_types.get(&ex.id).map(|te| ts_ty(te.ty, tys));
        // Explicit method-level type arguments must be the function's own.
        if !type_args.is_empty()
            && (type_args.len() != f.type_params.len()
                || type_args
                    .iter()
                    .zip(&f.type_params)
                    .any(|(a, p)| ts_type_ref(a) != p.name.name))
        {
            return None;
        }
        if witnesses.iter().any(|w| !own.contains(&ts_ty(*w, tys))) {
            return None;
        }
        let mut idx = 0;
        if f.has_self {
            let FnName::Method { type_name, .. } = &f.name else {
                return None;
            };
            let want = if owner_params.is_empty() {
                type_name.name.clone()
            } else {
                format!("{}<{}>", type_name.name, owner_params.join(", "))
            };
            if rendered(all[0])? != want {
                return None;
            }
            idx = 1;
        }
        for (a, p) in all[idx..].iter().zip(&f.params) {
            if rendered(a)? != ts_type_ref(&p.type_ref) {
                return None;
            }
        }
    }
    Some(all)
}

fn is_self_call(e: &Expr, f: &FnDecl, commons: &TypedCommons, tys: TyRender<'_>) -> bool {
    self_call_args(e, f, commons, tys).is_some()
}

impl TcoPlan {
    /// Whether `e` is one of the tail self calls.
    pub(crate) fn is_call(&self, e: &Expr) -> bool {
        self.calls.contains(&e.id)
    }

    /// Whether the `if` or `match` `e` has a tail self call in a branch.
    pub(crate) fn has_call_in(&self, e: &Expr) -> bool {
        self.ifs.contains(&e.id)
    }
}

/// `f`'s body as the loop: the loop variables, the per-iteration bindings,
/// `requires` (re-checked every iteration, so each recursive call's arguments
/// are still checked), then the ordinary body lowering, whose tail self calls
/// [`emit_continue`] turns into `continue`. The statements of a typed function
/// body (1A.15): the loop is a labelled `while` node (1A.35a part B) whose block holds the
/// body's statements, built one step deeper ([`Body::into_stmts`]). `guards` builds the
/// `requires` checks, inside the loop.
pub(crate) fn loop_body_stmts(
    f: &FnDecl,
    plan: &Rc<TcoPlan>,
    cx: &mut LowerCtx,
    async_tail: bool,
    base: usize,
    guards: impl FnOnce(&mut LowerCtx) -> Body,
) -> Vec<bynk_ts::TsStmt> {
    let mut inner = Body::new();
    for s in &plan.slots {
        inner.push(Item::Node(Box::new(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident(s.binder.clone()),
            None,
            bynk_ts::TsExpr::Ident(s.var.clone()),
            None,
        ))));
    }
    inner.append(guards(cx));
    let prev = cx.tco.replace(Rc::clone(plan));
    inner.append(block_body_with_return(
        &f.body,
        cx,
        base + INDENT_STEP,
        async_tail,
        Some(&f.return_type),
    ));
    cx.tco = prev;
    // `let __tcoN = <binder>, …;` then `__tco: while (true) { … }`, as nodes: the loop body is
    // one step deeper, so its statements are built at that column.
    let vars = plan
        .slots
        .iter()
        .map(|s| {
            bynk_ts::TsDeclarator::new(s.var.clone()).init(bynk_ts::TsExpr::Ident(s.binder.clone()))
        })
        .collect();
    let looped = bynk_ts::TsStmt::while_stmt(
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true)),
        bynk_ts::TsStmt::block(inner.into_stmts(base + INDENT_STEP), None),
        None,
    );
    vec![
        bynk_ts::TsStmt::let_list(vars, None),
        bynk_ts::TsStmt::labelled(LABEL, looped, None),
    ]
}

/// Emit the tail self call `e` as a reassignment and `continue`. Returns
/// `false` (writing nothing) when `e` is not one, so the caller lowers it as it
/// always did.
pub(super) fn emit_continue(body: &mut Body, e: &Expr, cx: &mut LowerCtx) -> bool {
    // A tail-slot sink is a value-position form, never a function tail.
    if cx.tail_slot.is_some() {
        return false;
    }
    let Some(plan) = cx.tco.clone() else {
        return false;
    };
    if !plan.is_call(e) {
        return false;
    }
    let Some((_, args, _)) = call_parts(e, cx.commons()) else {
        return false;
    };
    let mut pre = Pre::new();
    let vals: Vec<LoweredExpr> = args.iter().map(|a| pre.lower_node(a, cx)).collect();
    body.extend(pre.items());
    // Every slot is assigned, even where the argument looks like the parameter's
    // own name: a binder in the body can share a parameter's name, and the
    // assignment is the only thing that can tell them apart. Every new argument
    // is evaluated into a temporary before any loop variable changes, so
    // `f(b, a)` swaps and `f(n - 1, acc + n)` reads the old `n`.
    let temps: Vec<String> = vals.iter().map(|_| cx.fresh()).collect();
    for (v, t) in vals.into_iter().zip(&temps) {
        body.push(const_item(t, None, v, false, cx.fault_site(e.span)));
    }
    for (s, t) in plan.slots.iter().zip(&temps) {
        body.push(assign_item(
            bynk_ts::TsExpr::Ident(s.var.clone()),
            LoweredExpr::Node(bynk_ts::TsExpr::Ident(t.clone())),
            cx.fault_site(e.span),
        ));
    }
    body.push(Item::Node(Box::new(bynk_ts::TsStmt::continue_label_stmt(
        LABEL, None,
    ))));
    true
}
