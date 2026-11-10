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

use super::lower::{Pre, emit_block_as_function_body_with_return, write_line};
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
    /// The `if` expressions with a tail self call in one of their branches, at
    /// any depth: they must be emitted as statements, never as a ternary.
    ifs: HashSet<ExprId>,
    /// The receiver (when the function has `self`) and the parameters, in
    /// argument order.
    slots: Vec<Slot>,
}

/// Decide whether `f` is rewritten. `None` when it has an `ensures` clause (a
/// loop has one frame, so the postcondition could only be checked for the
/// outermost call, which is weaker), or has no tail self call.
pub(crate) fn plan(f: &FnDecl, commons: &TypedCommons) -> Option<Rc<TcoPlan>> {
    if !f.ensures.is_empty() {
        return None;
    }
    let mut p = TcoPlan {
        calls: HashSet::new(),
        ifs: HashSet::new(),
        slots: Vec::new(),
    };
    walk_block(&f.body, f, commons, &mut p);
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
fn walk_expr(e: &Expr, f: &FnDecl, commons: &TypedCommons, p: &mut TcoPlan) -> bool {
    match &e.kind {
        ExprKind::Call { .. } | ExprKind::MethodCall { .. } if is_self_call(e, f, commons) => {
            p.calls.insert(e.id);
            true
        }
        ExprKind::If {
            then_block,
            else_block,
            ..
        } => {
            let a = walk_block(then_block, f, commons, p);
            let b = walk_block(else_block, f, commons, p);
            if a || b {
                p.ifs.insert(e.id);
            }
            a || b
        }
        ExprKind::Match { arms, .. } => {
            let mut any = false;
            for arm in arms {
                any |= match &arm.body {
                    MatchBody::Expr(e) => walk_expr(e, f, commons, p),
                    MatchBody::Block(b) => walk_block(b, f, commons, p),
                };
            }
            any
        }
        _ => false,
    }
}

fn walk_block(b: &Block, f: &FnDecl, commons: &TypedCommons, p: &mut TcoPlan) -> bool {
    walk_expr(&b.tail, f, commons, p)
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

/// The expressions a self call passes, in slot order (receiver first for an
/// instance method call), or `None` when `e` is not a self call or not at the
/// function's own instantiation.
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
fn self_call_args<'e>(e: &'e Expr, f: &FnDecl, commons: &'e TypedCommons) -> Option<Vec<&'e Expr>> {
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
        let tys = commons.tys();
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

fn is_self_call(e: &Expr, f: &FnDecl, commons: &TypedCommons) -> bool {
    self_call_args(e, f, commons).is_some()
}

impl TcoPlan {
    /// Whether `e` is one of the tail self calls.
    pub(crate) fn is_call(&self, e: &Expr) -> bool {
        self.calls.contains(&e.id)
    }

    /// Whether the `if` `e` has a tail self call in a branch.
    pub(crate) fn if_has_call(&self, e: &Expr) -> bool {
        self.ifs.contains(&e.id)
    }
}

/// Write `f`'s body as the loop: the loop variables, the per-iteration
/// bindings, `requires` (re-checked every iteration, so each recursive call's
/// arguments are still checked), then the ordinary body lowering, whose tail
/// self calls [`emit_continue`] turns into `continue`.
pub(crate) fn emit_loop_body(
    out: &mut String,
    f: &FnDecl,
    plan: &Rc<TcoPlan>,
    cx: &mut LowerCtx,
    async_tail: bool,
    base: usize,
    guards: impl FnOnce(&mut String, &mut LowerCtx, usize),
) {
    let init: Vec<String> = plan
        .slots
        .iter()
        .map(|s| format!("{} = {}", s.var, s.binder))
        .collect();
    write_line(out, base, &format!("let {};", init.join(", ")));
    write_line(out, base, &format!("{LABEL}: while (true) {{"));
    for s in &plan.slots {
        write_line(
            out,
            base + INDENT_STEP,
            &format!("const {} = {};", s.binder, s.var),
        );
    }
    guards(out, cx, base / INDENT_STEP + 1);
    let prev = cx.tco.replace(Rc::clone(plan));
    emit_block_as_function_body_with_return(
        out,
        &f.body,
        cx,
        base + INDENT_STEP,
        async_tail,
        Some(&f.return_type),
    );
    cx.tco = prev;
    write_line(out, base, "}");
}

/// Emit the tail self call `e` as a reassignment and `continue`. Returns
/// `false` (writing nothing) when `e` is not one, so the caller lowers it as it
/// always did.
pub(crate) fn emit_continue(out: &mut String, e: &Expr, cx: &mut LowerCtx, indent: usize) -> bool {
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
    let vals: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
    for s in pre.stmts() {
        write_line(out, indent, s);
    }
    // An argument that is the parameter itself needs no assignment.
    let changed: Vec<(&Slot, &String)> = plan
        .slots
        .iter()
        .zip(&vals)
        .filter(|(s, v)| s.binder != **v)
        .collect();
    if changed.len() == 1 {
        write_line(
            out,
            indent,
            &format!("{} = {};", changed[0].0.var, changed[0].1),
        );
    } else if changed.len() > 1 {
        // Every new argument is evaluated before any loop variable changes, so
        // `f(b, a)` swaps and `f(n - 1, acc + n)` reads the old `n`.
        let temps: Vec<String> = changed.iter().map(|_| cx.fresh()).collect();
        for ((_, v), t) in changed.iter().zip(&temps) {
            write_line(out, indent, &format!("const {t} = {v};"));
        }
        for ((s, _), t) in changed.iter().zip(&temps) {
            write_line(out, indent, &format!("{} = {t};", s.var));
        }
    }
    write_line(out, indent, &format!("continue {LABEL};"));
    true
}
