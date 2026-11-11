//! #2640 (concurrent effects, slice 0): checking a `par { … }` block.
//!
//! The block issues every binding's effect, then waits for them all
//! (ADR 0432-pending, `design/archive/retired-tracks.md` Q1). It holds only
//! `let x <- e` and `do e`; each is checked exactly as it would be written
//! sequentially, against the scope *outside* the block, and the bound names
//! enter the enclosing scope once every binding has been checked.

use std::collections::HashMap;

use super::*;

/// Check one `par` block. `scope_end` is where the enclosing block closes,
/// which is how far the bound names are visible.
pub(super) fn check_par(p: &ParStmt, scope_end: usize, ctx: &mut Ctx) {
    let tys = ctx.tys;
    if !ctx.effectful {
        ctx.errors.push(
            CompileError::new(
                "bynk.effect.bind_in_pure_context",
                p.span,
                "a `par` block can only be used inside an effectful body (one returning `Effect[T]`)",
            )
            .with_label(
                ctx.return_ty_span,
                format!("enclosing return type is `{}`", ctx.return_ty.display(tys)),
            )
            .with_note("change the enclosing function/handler's return type to `Effect[...]`"),
        );
    }
    if p.statements.is_empty() {
        ctx.errors.push(
            CompileError::new(
                "bynk.par.invalid_statement",
                p.span,
                "a `par` block needs at least one `let x <- e` or `do e`",
            )
            .with_note("a `par` block has nothing to run concurrently when it is empty"),
        );
    }

    // Every name a binding introduces, with the statement that introduces it.
    let mut owners: HashMap<&str, usize> = HashMap::new();
    for (i, st) in p.statements.iter().enumerate() {
        if let Statement::EffectLet(l) = st
            && l.name.name != "_"
            && owners.insert(l.name.name.as_str(), i).is_some()
        {
            ctx.errors.push(
                CompileError::new(
                    "bynk.par.sibling_name",
                    l.name.span,
                    format!(
                        "`{}` is bound twice in the same `par` block, so it would be unclear which result is in scope afterwards",
                        l.name.name
                    ),
                )
                .with_note("give each binding in a `par` block its own name"),
            );
        }
    }

    let mut bound: Vec<(&LetStmt, TyId)> = Vec::new();
    // A binding that failed to check still binds its name, to the error type, so
    // its later uses absorb instead of echoing as unknown names.
    let mut errored: Vec<&Ident> = Vec::new();
    let mut valid = 0usize;
    for (i, st) in p.statements.iter().enumerate() {
        match st {
            Statement::EffectLet(l) => {
                valid += 1;
                let clean = check_binding_value(&l.value, i, &owners, ctx);
                let ty = in_sibling_scope(i, &owners, ctx, |ctx| {
                    let ty = check_effect_let(l, false, ctx);
                    reject_store_write(&l.value, ctx);
                    ty
                });
                match ty {
                    Some(ty) if clean => bound.push((l, ty)),
                    _ => errored.push(&l.name),
                }
            }
            Statement::Do(d) => {
                valid += 1;
                check_binding_value(&d.value, i, &owners, ctx);
                in_sibling_scope(i, &owners, ctx, |ctx| {
                    check_do(d, false, ctx);
                    reject_store_write(&d.value, ctx);
                });
            }
            other => {
                let what = match other {
                    Statement::Let(_) => "a plain `let`, which has no effect to overlap",
                    Statement::Expect(_) => "`expect`",
                    Statement::Send(_) => "`~>`, which is fire-and-forget and has nothing to join",
                    Statement::Assign(_) => "`:=`, since `store` writes stay sequential",
                    Statement::Advance(_) => "`advance`",
                    Statement::Par(_) => "a nested `par`",
                    Statement::EffectLet(_) | Statement::Do(_) => unreachable!(),
                };
                ctx.errors.push(
                    CompileError::new(
                        "bynk.par.invalid_statement",
                        other.span(),
                        format!("a `par` block holds only `let x <- e` and `do e`, not {what}"),
                    )
                    .with_note("move this statement before or after the block"),
                );
                // Typed anyway, so the capabilities it uses still count as used
                // and the author is not told about those on top of this.
                let mut exprs = Vec::new();
                statement_exprs(other, &mut exprs);
                for e in exprs {
                    type_of(e, None, ctx);
                }
            }
        }
    }

    if valid == 1 && p.statements.len() == 1 {
        ctx.errors.push(
            CompileError::new(
                "bynk.par.single_binding",
                p.span,
                "a `par` block with one binding has nothing to overlap; it is a plain `let x <- e`",
            )
            .with_note("add the other independent effects to the block, or drop the `par`"),
        );
    }

    // The names enter the enclosing scope after the closing brace (DECISION D).
    for (l, ty) in bound {
        bind_effect_let(l, ty, p.span.end, scope_end, ctx);
    }
    for name in errored {
        if name.name != "_" {
            ctx.bind(name.name.clone(), tys.intern(Ty::Error));
        }
    }
}

/// Run `check` with the names the block's *other* bindings introduce bound to the
/// error type: a binding is checked against the scope outside the block, and a
/// sibling mention (already reported) absorbs instead of resolving to a stale
/// outer binding or to an unknown name.
fn in_sibling_scope<R>(
    own: usize,
    owners: &HashMap<&str, usize>,
    ctx: &mut Ctx,
    check: impl FnOnce(&mut Ctx) -> R,
) -> R {
    let error = ctx.tys.intern(Ty::Error);
    ctx.push_scope();
    for (name, &owner) in owners {
        if owner != own {
            ctx.bind((*name).to_string(), error);
        }
    }
    let out = check(ctx);
    ctx.pop_scope();
    out
}

/// The two rules that look at a binding's expression before it is typed: no
/// `?` (a binding's `Result` is unwrapped after the block, where the `?` lines
/// run in source order; track Q2, slice 1 dropped), and no mention of a name a
/// *different* binding of the block binds. Returns whether it is clean.
fn check_binding_value(
    value: &Expr,
    own: usize,
    owners: &HashMap<&str, usize>,
    ctx: &mut Ctx,
) -> bool {
    let mut clean = true;
    let mut visit = |e: &Expr, in_lambda: bool, shadowed: &[String], ctx: &mut Ctx| match &e.kind {
        // A `?` inside a lambda belongs to the lambda, not to the binding.
        ExprKind::Question(_) if !in_lambda => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.par.question_in_join",
                    e.span,
                    "`?` cannot appear inside a `par` block",
                )
                .with_note(
                    "bind the `Result` in the block and apply `?` after it (`let userR <- Users.get(id)` inside, `let user = userR?` after); the block has waited for every binding, so the first `Err` in source order propagates",
                )
                .with_note(
                    "a `?` on an argument can be computed before the block (`let key = parse(raw)?`, then `Orders.get(key)` inside)",
                ),
            );
            clean = false;
        }
        ExprKind::Ident(id)
            if owners.get(id.name.as_str()).is_some_and(|&o| o != own)
                && !shadowed.contains(&id.name) =>
        {
            ctx.errors.push(
                CompileError::new(
                    "bynk.par.sibling_name",
                    id.span,
                    format!(
                        "`{}` is bound by another binding in this `par` block, so this binding cannot use it",
                        id.name
                    ),
                )
                .with_note(
                    "the bindings in a `par` block run together; use the name after the block, or move this binding after it. The rule holds even where an outer `{}` exists, so a stale value is never picked up silently",
                ),
            );
            clean = false;
        }
        _ => {}
    };
    walk(value, false, &mut Vec::new(), &mut visit, ctx);
    clean
}

/// Visit `e` and every expression beneath it, lambda bodies included, saying
/// whether the expression is inside one (a sibling's name read in a lambda is
/// captured just the same, but a `?` there belongs to the lambda) and which names
/// the code around it has bound itself: a lambda parameter, a `match` arm's
/// pattern, a `let` in a block or an `is` pattern named like a sibling is that
/// binding, not the sibling.
fn walk(
    e: &Expr,
    in_lambda: bool,
    shadowed: &mut Vec<String>,
    visit: &mut impl FnMut(&Expr, bool, &[String], &mut Ctx),
    ctx: &mut Ctx,
) {
    visit(e, in_lambda, shadowed, ctx);
    let mark = shadowed.len();
    let names = |p: &Pattern| -> Vec<String> {
        p.bound_names()
            .into_iter()
            .map(|i| i.name.clone())
            .collect()
    };
    match &e.kind {
        ExprKind::Lambda(l) => {
            shadowed.extend(l.params.iter().map(|p| p.name.name.clone()));
            walk(&l.body, true, shadowed, visit, ctx);
        }
        ExprKind::Match { discriminant, arms } => {
            walk(discriminant, in_lambda, shadowed, visit, ctx);
            for arm in arms {
                let arm_mark = shadowed.len();
                shadowed.extend(names(&arm.pattern));
                if let Some(g) = &arm.guard {
                    walk(g, in_lambda, shadowed, visit, ctx);
                }
                match &arm.body {
                    MatchBody::Expr(x) => walk(x, in_lambda, shadowed, visit, ctx),
                    MatchBody::Block(b) => walk_block(b, in_lambda, shadowed, visit, ctx),
                }
                shadowed.truncate(arm_mark);
            }
        }
        ExprKind::Block(b) => walk_block(b, in_lambda, shadowed, visit, ctx),
        _ => {
            for child in expr_children(e) {
                walk(child, in_lambda, shadowed, visit, ctx);
                // An `is` test binds for what follows it (`x is Some(u) && u > 0`).
                if let ExprKind::Is { pattern, .. } = &child.kind {
                    shadowed.extend(names(pattern));
                }
            }
        }
    }
    shadowed.truncate(mark);
}

fn walk_block(
    b: &Block,
    in_lambda: bool,
    shadowed: &mut Vec<String>,
    visit: &mut impl FnMut(&Expr, bool, &[String], &mut Ctx),
    ctx: &mut Ctx,
) {
    let mark = shadowed.len();
    for st in &b.statements {
        let mut exprs = Vec::new();
        statement_exprs(st, &mut exprs);
        for x in exprs {
            walk(x, in_lambda, shadowed, visit, ctx);
        }
        if let Statement::Let(l) | Statement::EffectLet(l) = st {
            shadowed.push(l.name.name.clone());
        }
    }
    walk(&b.tail, in_lambda, shadowed, visit, ctx);
    shadowed.truncate(mark);
}

/// `:=` and `.update` keep `store` writes sequential (track Q4): a binding whose
/// expression is itself a mutating store operation is refused.
fn reject_store_write(value: &Expr, ctx: &mut Ctx) {
    if let Some(Callee::Store { field, op }) = ctx.callees.get(&value.id)
        && crate::store_ops::is_mutating_any(op)
    {
        let msg = format!(
            "a `par` block cannot hold the store write `{field}.{op}(…)`: store writes stay sequential"
        );
        ctx.errors.push(
            CompileError::new("bynk.par.invalid_statement", value.span, msg)
                .with_note("perform the write before or after the block"),
        );
    }
}

/// One store access found in a closure: where, which field, which operation
/// (`:=` for a `Cell` assignment), and whether it writes.
struct StoreAccess {
    span: Span,
    field: String,
    op: String,
    writes: bool,
}

/// #3298: every store call and `:=` under `e`, lambdas, blocks, `match` arms
/// and statements included. `expr_children` flattens statements to their
/// expressions, which drops a `:=` itself, so blocks are walked here. A name
/// `let`-bound to a lambda that writes a store counts as a write wherever it is
/// mentioned or called (the closure may call it, or hand it on), unless an inner
/// lambda parameter, `let` or pattern binds the same name.
fn scan_store_access(e: &Expr, ctx: &Ctx, shadow: &mut Vec<String>, out: &mut Vec<StoreAccess>) {
    if let Some(Callee::Store { field, op }) = ctx.callees.get(&e.id) {
        out.push(StoreAccess {
            span: e.span,
            field: field.clone(),
            op: op.clone(),
            writes: crate::store_ops::is_mutating_any(op),
        });
    }
    let named = |name: &Ident, shadow: &[String]| {
        (!shadow.contains(&name.name))
            .then(|| ctx.store_writing_lambdas.get(&name.name))
            .flatten()
    };
    let push_named = |name: &Ident, shadow: &[String], out: &mut Vec<StoreAccess>| {
        if let Some((field, op)) = named(name, shadow) {
            out.push(StoreAccess {
                span: name.span,
                field: field.clone(),
                op: op.clone(),
                writes: true,
            });
        }
    };
    let mark = shadow.len();
    match &e.kind {
        ExprKind::Ident(id) => push_named(id, shadow, out),
        ExprKind::Call { name, args, .. } => {
            push_named(name, shadow, out);
            for a in args {
                scan_store_access(a, ctx, shadow, out);
            }
        }
        ExprKind::Lambda(l) => {
            shadow.extend(l.params.iter().map(|p| p.name.name.clone()));
            scan_store_access(&l.body, ctx, shadow, out);
        }
        ExprKind::Block(b) => scan_block_access(b, ctx, shadow, out),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            scan_store_access(cond, ctx, shadow, out);
            scan_block_access(then_block, ctx, shadow, out);
            scan_block_access(else_block, ctx, shadow, out);
        }
        ExprKind::Match { discriminant, arms } => {
            scan_store_access(discriminant, ctx, shadow, out);
            for arm in arms {
                let arm_mark = shadow.len();
                shadow.extend(
                    arm.pattern
                        .bound_names()
                        .into_iter()
                        .map(|i| i.name.clone()),
                );
                if let Some(g) = &arm.guard {
                    scan_store_access(g, ctx, shadow, out);
                }
                match &arm.body {
                    MatchBody::Expr(x) => scan_store_access(x, ctx, shadow, out),
                    MatchBody::Block(b) => scan_block_access(b, ctx, shadow, out),
                }
                shadow.truncate(arm_mark);
            }
        }
        _ => {
            for child in expr_children(e) {
                scan_store_access(child, ctx, shadow, out);
            }
        }
    }
    shadow.truncate(mark);
}

fn scan_block_access(b: &Block, ctx: &Ctx, shadow: &mut Vec<String>, out: &mut Vec<StoreAccess>) {
    let mark = shadow.len();
    for st in &b.statements {
        if let Statement::Assign(a) = st {
            out.push(StoreAccess {
                span: a.span,
                field: a.target.name.clone(),
                op: ":=".to_string(),
                writes: true,
            });
        }
        let mut exprs = Vec::new();
        statement_exprs(st, &mut exprs);
        for x in exprs {
            scan_store_access(x, ctx, shadow, out);
        }
        if let Statement::Let(l) | Statement::EffectLet(l) = st {
            shadow.push(l.name.name.clone());
        }
    }
    scan_store_access(&b.tail, ctx, shadow, out);
    shadow.truncate(mark);
}

/// The first store write in a lambda expression's body, as `(field, op)`.
/// `None` for anything that is not a lambda, or whose body writes no store.
pub(super) fn lambda_store_write(e: &Expr, ctx: &Ctx) -> Option<(String, String)> {
    let mut e = e;
    while let ExprKind::Paren(inner) = &e.kind {
        e = inner;
    }
    if !matches!(e.kind, ExprKind::Lambda(_)) {
        return None;
    }
    let mut found = Vec::new();
    scan_store_access(e, ctx, &mut Vec::new(), &mut found);
    found
        .into_iter()
        .find(|a| a.writes)
        .map(|a| (a.field, a.op))
}

/// #3298 (extends track Q4 to the `parTraverse` family): the closure given to
/// `parTraverse`, `parTraverseAll` or `parTraverseTry` runs once per element,
/// concurrently, so it may not write a store: a `get` then a `put` in it loses
/// updates, because each `<-` is an await point. A single call such as `update`
/// is atomic, but it is refused too, exactly as `par { }` refuses it, so the
/// two rules stay equal. Reads, and a store as the combinator's *receiver*, are
/// untouched. The closure is the written lambda, or a name `let`-bound to one.
pub(super) fn reject_store_write_in_closure(method: &Ident, args: &[Expr], ctx: &mut Ctx) {
    use crate::builtin_names::methods::{PAR_TRAVERSE, PAR_TRAVERSE_ALL, PAR_TRAVERSE_TRY};
    let name = method.name.as_str();
    if args.len() != 1
        || (name != PAR_TRAVERSE && name != PAR_TRAVERSE_ALL && name != PAR_TRAVERSE_TRY)
    {
        return;
    }
    let mut inner = &args[0];
    while let ExprKind::Paren(i) = &inner.kind {
        inner = i;
    }
    let mut accesses = Vec::new();
    scan_store_access(inner, ctx, &mut Vec::new(), &mut accesses);
    let reads: Vec<String> = accesses
        .iter()
        .filter(|a| !a.writes)
        .map(|a| a.field.clone())
        .collect();
    for a in accesses.iter().filter(|a| a.writes) {
        let what = if a.op == ":=" {
            format!("`{} := …`", a.field)
        } else {
            format!("`{}.{}(…)`", a.field, a.op)
        };
        let mut err = CompileError::new(
            "bynk.par.store_write_in_closure",
            a.span,
            format!(
                "a closure passed to `{name}` cannot write the store field `{}` ({what}): its branches run concurrently, so store writes stay sequential",
                a.field
            ),
        )
        .with_label(method.span, format!("`{name}` runs the closure for every element at once"))
        .with_note(
            "run the loop with `forEach` instead (`traverseAll` or `traverseTry` for the collecting forms); a single-call `update` or `upsert` is atomic per key there, because each element finishes before the next starts",
        )
        .with_note(
            "or keep the concurrent part in the closure and do the writes after it: collect with `parTraverseAll`, then write in a `forEach`",
        );
        if reads.contains(&a.field) {
            err = err.with_note(
                "a `get` and a `put` are two awaited steps, so another branch can write between them and the update is lost; use `update` or `upsert` in a sequential loop",
            );
        }
        ctx.errors.push(err);
    }
}

/// #3298: after a `let` is typed, remember that its name is a lambda that writes
/// a store (or a bare alias of one), so using that name in a `parTraverse`
/// closure is refused. An entry is never removed: the record is by name, not by
/// scope, so a rebinding in an inner block cannot hide an outer writer. The cost
/// is a refusal that errs the safe way, when a name once bound to a writing
/// lambda is later rebound to something else in the same body and still used in
/// a closure.
pub(super) fn note_let_lambda(l: &LetStmt, ctx: &mut Ctx) {
    let write = lambda_store_write(&l.value, ctx).or_else(|| match &l.value.kind {
        ExprKind::Ident(id) => ctx.store_writing_lambdas.get(&id.name).cloned(),
        _ => None,
    });
    if let Some(write) = write {
        ctx.store_writing_lambdas.insert(l.name.name.clone(), write);
    }
}
