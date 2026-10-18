//! v0.102 — the held-resource linearity pass (real-time track slice 2; the spec's
//! §3 step 11, "Check linearity for held resources").
//!
//! A flow-sensitive post-pass over a handler/function body. It tracks each held
//! binding (`Connection[F]`, anything `Ty::is_held`) through a small ownership
//! state machine — **owned → borrowed → owned**, **owned → consumed** — and
//! enforces the §2.9 discipline: single-owner, mandatory disposal at scope exit,
//! branch unification, and the borrow rules.
//!
//! It runs after `type_of_block`, so `expr_types` is fully populated; the pass
//! reads it to learn a binding's type and a receiver's type, and never
//! re-type-checks. It is deliberately bounded — a fixed operation vocabulary over
//! three states, no general dataflow lattice (the reason §2.9 chose
//! API-discipline linearity over a general affine system).

use std::collections::HashMap;
use std::sync::Arc;

use bynk_syntax::ast::{
    Block, Expr, ExprId, ExprKind, Ident, MatchBody, Param, Pattern, Statement, TypeDecl,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use super::{Ty, TyId, TypedExpr, Types, resolve_type_ref};

/// The ownership state of a held binding on the current control-flow path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Held {
    /// The binding owns the value; it may be operated on or disposed.
    Owned,
    /// Lent to a borrowing operation's callback for that callback's scope;
    /// non-consuming ops (`send`) are admitted, consuming ones rejected.
    Borrowed,
    /// Disposed — consumed (`close`) or transferred (stored / passed to a
    /// consumer). Any later reference is a use-after-consume error.
    Disposed,
}

type State = HashMap<String, Held>;

struct Lin<'a> {
    /// T3.6b (R4.1): the unit's intern table — `expr_types` and
    /// `pattern_binding_types` hand out `TyId`s, and this pass resolves them
    /// to ask "is this held?".
    tys: &'a Types,
    expr_types: &'a HashMap<ExprId, TypedExpr>,
    /// T3.4: pattern-bound names' types, keyed by the binding `Ident`'s own
    /// span — deliberately not `expr_types`/`ExprId`-keyed (see `Ctx`'s field
    /// of the same name).
    pattern_binding_types: &'a HashMap<Span, TyId>,
    errors: &'a mut Vec<CompileError>,
}

/// True if a value type is (or wraps, through `Option`) a held resource — the
/// shape a `take(k) -> Option[Connection]` result or a held binding takes.
fn held_value(ty: TyId, tys: &Types) -> bool {
    match &*tys.get(ty) {
        t if t.is_held() => true,
        Ty::Option(inner) => held_value(*inner, tys),
        _ => false,
    }
}

/// Entry point: check the linearity discipline over `body`, seeding held
/// handler/function parameters as owned bindings.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check(
    body: &Block,
    params: &[Param],
    types: &HashMap<String, Arc<TypeDecl>>,
    expr_types: &HashMap<ExprId, TypedExpr>,
    pattern_binding_types: &HashMap<Span, TyId>,
    // v0.106 (slice 3b-iii): names of held params that are **borrowed**, not owned
    // — e.g. the firing `connection` of a `from websocket` `on message`/`on close`,
    // which the handler may `send` to but does not own/dispose. A borrowed binding
    // admits only non-consuming ops and carries no disposal obligation at scope
    // exit. (`on open`'s connection is owned — empty set — and must be disposed.)
    borrowed: &std::collections::HashSet<String>,
    errors: &mut Vec<CompileError>,
    tys: &Types,
) {
    let mut lin = Lin {
        tys,
        expr_types,
        pattern_binding_types,
        errors,
    };
    let mut state = State::new();
    let mut seeded = Vec::new();
    for p in params {
        if p.name.name == "_" {
            continue;
        }
        if let Some(ty) = resolve_type_ref(&p.type_ref, types, tys)
            && held_value(ty, tys)
        {
            if borrowed.contains(&p.name.name) {
                state.insert(p.name.name.clone(), Held::Borrowed);
            } else {
                state.insert(p.name.name.clone(), Held::Owned);
                seeded.push((p.name.name.clone(), p.name.span));
            }
        }
    }
    // A held parameter is owned for the whole body; it must be disposed before
    // the handler or function returns (the body block is its scope).
    lin.walk_block(body, &mut state);
    for (name, span) in seeded {
        if state.get(&name) == Some(&Held::Owned) {
            lin.leak(&name, span);
        }
    }
}

impl Lin<'_> {
    fn ty_of(&self, e: &Expr) -> Option<TyId> {
        self.expr_types.get(&e.id).map(|te| te.ty)
    }

    fn leak(&mut self, name: &str, span: Span) {
        self.errors.push(
            CompileError::new(
                "bynk.held.leak",
                span,
                format!(
                    "held value `{name}` is still owned at scope exit — it must be disposed (stored, closed, or transferred) before returning"
                ),
            )
            .with_note("store it (`<map>.put(k, conn)`), close it (`conn.close()`), or pass it to a function that consumes it"),
        );
    }

    /// Walk a block: process its statements in order, then its tail. Bindings
    /// introduced *inside* this block are leak-checked at block exit; bindings
    /// owned from an outer scope remain in `state` for the caller.
    fn walk_block(&mut self, block: &Block, state: &mut State) {
        let mut introduced: Vec<(String, Span)> = Vec::new();
        for stmt in &block.statements {
            self.walk_stmt(stmt, state, &mut introduced);
        }
        self.walk_expr(&block.tail, state);
        for (name, span) in introduced {
            if state.get(&name) == Some(&Held::Owned) {
                self.leak(&name, span);
            }
            state.remove(&name);
        }
    }

    fn walk_stmt(
        &mut self,
        stmt: &Statement,
        state: &mut State,
        introduced: &mut Vec<(String, Span)>,
    ) {
        match stmt {
            Statement::Let(l) | Statement::EffectLet(l) => {
                self.walk_expr(&l.value, state);
                // Does this binding name a held value? For an `EffectLet` the
                // value is `Effect[T]`; the binding is `T`. For a `Let` the
                // binding type is the value's type directly.
                let bound = self.ty_of(&l.value).and_then(|t| match stmt {
                    Statement::EffectLet(_) => match &*self.tys.get(t) {
                        Ty::Effect(inner) => Some(*inner),
                        _ => None,
                    },
                    Statement::Let(_) => Some(t),
                    _ => None,
                });
                if let Some(t) = bound
                    && held_value(t, self.tys)
                    && l.name.name != "_"
                {
                    // A `let` reusing a name this block already bound to a held
                    // value ends that earlier generation's scope right now —
                    // check-and-clear it here, the same way a borrow/match-arm
                    // scope checks-and-restores a shadowed entry at its own end.
                    // Left to the block's single end-of-scope sweep, only the
                    // latest state per name would ever be seen, silently losing
                    // an earlier generation's leak (or misattributing its
                    // disposal to this one).
                    if let Some(pos) = introduced.iter().position(|(n, _)| n == &l.name.name) {
                        let (prev_name, prev_span) = introduced.remove(pos);
                        if state.get(&prev_name) == Some(&Held::Owned) {
                            self.leak(&prev_name, prev_span);
                        }
                    }
                    state.insert(l.name.name.clone(), Held::Owned);
                    introduced.push((l.name.name.clone(), l.name.span));
                }
            }
            Statement::Expect(a) => self.walk_expr(&a.value, state),
            Statement::Send(s) => self.walk_expr(&s.value, state),
            Statement::Do(d) => self.walk_expr(&d.value, state),
            Statement::Assign(a) => self.walk_expr(&a.value, state),
            Statement::Advance(a) => self.walk_expr(&a.amount, state),
        }
    }

    /// Walk an expression, applying ownership transitions. Held idents that
    /// appear in *value* position (an argument, an assignment RHS) are
    /// transferred (consumed); a `send`/`close` on a held receiver is handled at
    /// the method-call site so the receiver is not double-counted.
    fn walk_expr(&mut self, e: &Expr, state: &mut State) {
        match &e.kind {
            ExprKind::Ident(id) => {
                // A bare held ident used as a value is a transfer (consume).
                self.use_held(&id.name, e.span, state);
            }
            ExprKind::MethodCall {
                receiver,
                method,
                args,
                ..
            } => self.walk_method_call(receiver, method.name.as_str(), method.span, args, state),
            ExprKind::Call { args, .. } => {
                for a in args {
                    self.walk_expr(a, state);
                }
            }
            // Slice C: `Wire(<String>)` — walk the raw inner expression.
            ExprKind::Wire(inner) => self.walk_expr(inner, state),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.walk_expr(cond, state);
                self.walk_branches(
                    &[Branch::block(then_block), Branch::block(else_block)],
                    e.span,
                    state,
                );
            }
            ExprKind::Match { discriminant, arms } => {
                self.walk_expr(discriminant, state);
                // Each arm's pattern may bind held resources out of the
                // discriminant (`Some(conn)` over `Option[Connection]`, `Ok(c)`
                // over `Result[Connection, _]`). Register those bindings as
                // owned for the arm and require the arm to dispose them —
                // consistent with `let`-binding handling. Without this the held
                // value escapes the pass entirely (#719). An arm's guard is
                // carried alongside its body and walked as a borrow scope over
                // the arm's state (#1769, see `walk_guard`).
                let branches: Vec<Branch> = arms
                    .iter()
                    .map(|a| {
                        let body = match &a.body {
                            MatchBody::Expr(ex) => BranchBody::Expr(ex),
                            MatchBody::Block(b) => BranchBody::Block(b),
                        };
                        Branch {
                            body,
                            guard: a.guard.as_ref(),
                            bindings: self.held_pattern_bindings(&a.pattern),
                        }
                    })
                    .collect();
                self.walk_branches(&branches, e.span, state);
            }
            ExprKind::Block(b) => self.walk_block(b, state),
            ExprKind::Paren(inner)
            | ExprKind::Question(inner)
            | ExprKind::Ok(inner)
            | ExprKind::Err(inner)
            | ExprKind::Some(inner)
            | ExprKind::UnaryOp(_, inner) => self.walk_expr(inner, state),
            ExprKind::BinOp(_, l, r) => {
                self.walk_expr(l, state);
                self.walk_expr(r, state);
            }
            ExprKind::FieldAccess { receiver, .. } => self.walk_expr(receiver, state),
            // Lambdas are walked at their borrowing-call site (forEach/update);
            // a bare lambda elsewhere introduces its own scope we do not track.
            ExprKind::Lambda(_) => {}
            // Everything else recurses over the total child iterator, so a
            // held value inside a list literal, record field, interpolation
            // hole, or `is` scrutinee is transferred like any other value
            // position — the old `_ => {}` silently skipped them all.
            ExprKind::IntLit { .. }
            | ExprKind::FloatLit { .. }
            | ExprKind::DurationLit { .. }
            | ExprKind::StrLit(_)
            | ExprKind::InterpStr(_)
            | ExprKind::BoolLit(_)
            | ExprKind::RecordConstruction { .. }
            | ExprKind::Is { .. }
            | ExprKind::None
            | ExprKind::UnitLit
            | ExprKind::RecordSpread { .. }
            | ExprKind::EffectPure(_)
            | ExprKind::Expect(_)
            | ExprKind::Val { .. }
            | ExprKind::ListLit(_)
            | ExprKind::Observation(_)
            | ExprKind::Faults(_)
            | ExprKind::Trace { .. } => {
                for c in bynk_syntax::ast::expr_children(e) {
                    self.walk_expr(c, state);
                }
            }
        }
    }

    /// A held binding referenced as a value — transfer ownership (consume), or
    /// report a use after it was already disposed.
    fn use_held(&mut self, name: &str, span: Span, state: &mut State) {
        match state.get(name).copied() {
            Some(Held::Owned) => {
                state.insert(name.to_string(), Held::Disposed);
            }
            Some(Held::Borrowed) => {
                // Transferring a borrowed value out is a consume on a borrow.
                self.errors.push(CompileError::new(
                    "bynk.held.consume_on_borrow",
                    span,
                    format!(
                        "held value `{name}` is borrowed here and cannot be transferred — a borrow admits only non-consuming operations like `send`"
                    ),
                ));
            }
            Some(Held::Disposed) => self.use_after_consume(name, span),
            None => {}
        }
    }

    fn use_after_consume(&mut self, name: &str, span: Span) {
        self.errors.push(CompileError::new(
            "bynk.held.use_after_consume",
            span,
            format!(
                "held value `{name}` is used after a consuming operation (`close`/`put`/`take`) ended its lifetime"
            ),
        ));
    }

    fn walk_method_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        method_span: Span,
        args: &[Expr],
        state: &mut State,
    ) {
        // Is the receiver a bare held binding we are tracking?
        let recv_held = match &receiver.kind {
            ExprKind::Ident(id) if state.contains_key(&id.name) => Some(id.name.clone()),
            _ => None,
        };
        if let Some(name) = recv_held {
            let st = state.get(&name).copied();
            match method {
                "send" => {
                    // Non-consuming: the binding stays owned (or borrowed). A
                    // `send` on a disposed value is a use-after-consume.
                    if st == Some(Held::Disposed) {
                        self.use_after_consume(&name, method_span);
                    }
                    for a in args {
                        self.walk_expr(a, state);
                    }
                }
                "close" => match st {
                    Some(Held::Owned) => {
                        state.insert(name, Held::Disposed);
                    }
                    Some(Held::Borrowed) => self.consume_on_borrow(&name, method_span),
                    Some(Held::Disposed) => self.use_after_consume(&name, method_span),
                    None => {}
                },
                _ => {
                    // Any other op on a held receiver is unknown; treat a bare
                    // reference conservatively (the checker already rejects the
                    // unknown method).
                    if st == Some(Held::Disposed) {
                        self.use_after_consume(&name, method_span);
                    }
                    for a in args {
                        self.walk_expr(a, state);
                    }
                }
            }
            return;
        }

        // The receiver is not a tracked held binding. It may be a held-bearing
        // storage collection whose borrowing methods (`forEach`/`parTraverse`/
        // `traverseAll`/`parTraverseAll`/`update`) lend a borrowed reference into
        // a closure (v0.107: `parTraverse` is the parallel broadcast form — its
        // closure borrows exactly as `forEach`; v0.148: the collect-all iterators
        // `traverseAll`/`parTraverseAll` lend the element identically).
        let recv_holds_held = self
            .ty_of(receiver)
            .is_some_and(|t| storage_value_is_held(t, self.tys));
        if recv_holds_held
            && matches!(
                method,
                "forEach"
                    | "parTraverse"
                    | "traverseAll"
                    | "parTraverseAll"
                    | "traverseTry"
                    | "parTraverseTry"
                    | "update"
            )
        {
            self.walk_borrowing_call(args, state);
            return;
        }

        // Otherwise: walk the receiver (a held ident here is a transfer) and
        // every argument (a held ident argument is a transfer / store).
        self.walk_expr(receiver, state);
        for a in args {
            self.walk_expr(a, state);
        }
    }

    fn consume_on_borrow(&mut self, name: &str, span: Span) {
        self.errors.push(CompileError::new(
            "bynk.held.consume_on_borrow",
            span,
            format!(
                "a consuming operation is called on the borrowed held value `{name}` — borrows admit only non-consuming operations like `send`"
            ),
        ));
    }

    /// A borrowing storage call (`forEach`/`update`) over a held-valued
    /// collection: the closure parameter naming the value is a *borrowed* held
    /// reference for the closure body. Walk that body with the parameter marked
    /// `Borrowed`; the borrow ends when the body returns (no leak).
    fn walk_borrowing_call(&mut self, args: &[Expr], state: &mut State) {
        for a in args {
            if let ExprKind::Lambda(lam) = &a.kind {
                // The held value is the closure's last parameter (`forEach((k, v))`
                // / `update((v))`): mark it borrowed for the body.
                let saved: Vec<(String, Option<Held>)> = lam
                    .params
                    .iter()
                    .filter(|p| p.name.name != "_")
                    .map(|p| (p.name.name.clone(), state.get(&p.name.name).copied()))
                    .collect();
                if let Some(last) = lam.params.iter().rev().find(|p| p.name.name != "_") {
                    state.insert(last.name.name.clone(), Held::Borrowed);
                }
                self.walk_expr(&lam.body, state);
                // Restore: the borrow ends; outer bindings of the same name are
                // unaffected (closure params shadow).
                for (name, prev) in saved {
                    match prev {
                        Some(p) => {
                            state.insert(name, p);
                        }
                        None => {
                            state.remove(&name);
                        }
                    }
                }
            } else {
                self.walk_expr(a, state);
            }
        }
    }

    /// Held-typed idents a pattern binds into an arm's scope — the payload
    /// bindings that name a held resource (recursing through nested payloads).
    /// Their type is read from `expr_types`, where the checker records each
    /// pattern binding's resolved type at its ident span.
    fn held_pattern_bindings<'p>(&self, pat: &'p Pattern) -> Vec<&'p Ident> {
        pat.bound_names()
            .into_iter()
            .filter(|id| {
                self.pattern_binding_types
                    .get(&id.span)
                    .is_some_and(|t| held_value(*t, self.tys))
            })
            .collect()
    }

    /// #1769: walk a `match` arm's `if` guard as a **borrow scope** (§2.9.5,
    /// "Guards"). A guard is evaluated before its arm is chosen, so it may run
    /// for an arm that is then not taken — a consume there would happen on a
    /// path the pass records as still owning the value. Every held binding the
    /// guard can see (the arm's own pattern bindings and any outer one) is
    /// therefore lent to it, exactly as a `forEach` closure borrows its
    /// element: a non-consuming use is admitted, a consuming one (a transfer, a
    /// `close`) is `bynk.held.consume_on_borrow`, and a value already disposed
    /// stays disposed, so a guard naming it is `bynk.held.use_after_consume`.
    ///
    /// The walk runs over a copy of `state` that is then dropped: a borrow ends
    /// with its scope, so the guard leaves no ownership change behind for the
    /// arm body, the next arm, or the unification after the `match`.
    fn walk_guard(&mut self, guard: &Expr, state: &State) {
        let mut lent: State = state
            .iter()
            .map(|(name, held)| {
                let held = match held {
                    Held::Owned => Held::Borrowed,
                    other => *other,
                };
                (name.clone(), held)
            })
            .collect();
        let first = self.errors.len();
        self.walk_expr(guard, &mut lent);
        for err in &mut self.errors[first..] {
            if err.category == "bynk.held.consume_on_borrow" {
                err.notes.push(
                    "a match-arm guard only borrows held values: it runs before its arm is chosen, so it may run for an arm that is not taken"
                        .to_string(),
                );
            }
        }
    }

    /// Walk a set of branch bodies from a shared pre-branch `state`, then unify:
    /// every outer held binding must end each branch in the same state, else the
    /// branches diverge. Branch-local bindings are leak-checked within each body.
    fn walk_branches(&mut self, branches: &[Branch], span: Span, state: &mut State) {
        let outer: Vec<String> = state.keys().cloned().collect();
        let mut ends: Vec<State> = Vec::new();
        for br in branches {
            let mut branch = state.clone();
            // Introduce this arm's held pattern bindings as owned, saving any
            // outer binding they shadow (shadowing is legal — resolver.rs). The
            // arm binding is scoped to the arm: it must be disposed by the arm's
            // end, and the shadowed outer binding is restored afterwards so a
            // consume of the inner value is not misattributed to the outer one.
            let saved: Vec<(String, Option<Held>)> = br
                .bindings
                .iter()
                .map(|id| (id.name.clone(), branch.get(&id.name).copied()))
                .collect();
            for id in &br.bindings {
                branch.insert(id.name.clone(), Held::Owned);
            }
            // #1769: the guard sees the arm's bindings, and runs before the arm
            // is chosen, so it is walked against the arm's entry state.
            if let Some(guard) = br.guard {
                self.walk_guard(guard, &branch);
            }
            match &br.body {
                BranchBody::Block(b) => self.walk_block(b, &mut branch),
                BranchBody::Expr(ex) => self.walk_expr(ex, &mut branch),
            }
            for id in &br.bindings {
                if branch.get(&id.name) == Some(&Held::Owned) {
                    self.leak(&id.name, id.span);
                }
            }
            for (name, prev) in saved {
                match prev {
                    Some(p) => {
                        branch.insert(name, p);
                    }
                    None => {
                        branch.remove(&name);
                    }
                }
            }
            ends.push(branch);
        }
        // Unify each outer binding across the branches.
        for name in &outer {
            let first = ends.first().and_then(|s| s.get(name).copied());
            let diverges = ends.iter().any(|s| s.get(name).copied() != first);
            if diverges {
                self.errors.push(
                    CompileError::new(
                        "bynk.held.branch_divergence",
                        span,
                        format!(
                            "branches leave held value `{name}` in inconsistent ownership states — one consumes or stores it, another leaves it owned"
                        ),
                    )
                    .with_note(format!("make every branch dispose `{name}` (or none do, deferring disposal past the conditional)")),
                );
                // Settle on Disposed to suppress a cascading leak diagnostic.
                state.insert(name.clone(), Held::Disposed);
            } else if let Some(s) = first {
                state.insert(name.clone(), s);
            }
        }
    }
}

/// True if a storage collection's *type* carries held values — `Map[K, Conn]`,
/// `Cell[Option[Conn]]`, or a `Query`/`List` view over connections.
fn storage_value_is_held(t: TyId, tys: &Types) -> bool {
    match &*tys.get(t) {
        Ty::Map(_, v) => held_value(*v, tys),
        Ty::Query(v) | Ty::List(v) => held_value(*v, tys),
        _ => false,
    }
}

enum BranchBody<'a> {
    Block(&'a Block),
    Expr(&'a Expr),
}

/// One arm of a branching construct: its body, its `match`-arm guard (#1769;
/// walked as a borrow scope, see `Lin::walk_guard`), plus the held-typed
/// pattern bindings it introduces into scope (an `if`/`else` block has no
/// guard and binds nothing).
struct Branch<'a> {
    body: BranchBody<'a>,
    guard: Option<&'a Expr>,
    bindings: Vec<&'a Ident>,
}

impl<'a> Branch<'a> {
    /// A branch whose body is a block and which binds nothing (an `if`/`else`).
    fn block(b: &'a Block) -> Self {
        Branch {
            body: BranchBody::Block(b),
            guard: None,
            bindings: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::analyse;

    fn room(handler: &str) -> crate::testkit::Analysed {
        analyse(&[(
            "room.bynk",
            &format!(
                "context room\n\ntype ServerFrame = {{ text: String }}\n\nagent Room {{\n  key id: String\n  store conns: Map[String, Connection[ServerFrame]]\n\n{handler}\n}}\n"
            ),
        )])
    }

    #[test]
    fn a_guard_that_consumes_a_held_value_is_a_consume_on_borrow() {
        // #1769: a guard runs before its arm is chosen, so it only borrows.
        room(
            "  on call f(conn: Connection[ServerFrame], n: Int) -> Effect[()] {\n    match n {\n      0 if Some(conn) is Some(_) => conn.close(),\n      _ => conn.close()\n    }\n  }",
        )
        .assert_reports("bynk.held.consume_on_borrow");
    }

    #[test]
    fn a_guard_that_only_reads_a_held_value_is_fine() {
        let a = room(
            "  on call f(conn: Connection[ServerFrame], n: Int) -> Effect[()] {\n    match n {\n      0 if n > 0 => conn.close(),\n      _ => conn.close()\n    }\n  }",
        );
        a.assert_clean();
    }
}
