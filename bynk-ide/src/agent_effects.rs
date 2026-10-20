//! #2198: the per-handler effect summary — what each of an agent's handlers
//! writes, reads and calls, path by path (agent-state-views track, slice 4;
//! summary in `design/archive/retired-tracks.md`, §3.1).
//!
//! The state model (`agent_state`) answers "which state does each handler
//! move the agent to?" for a field of an enum or sum type. Most agents have no
//! such field — maps, counters, logs — and for them the useful question is
//! "what does each handler touch, under which conditions?". For every handler
//! this returns rows, one per merged path:
//!
//! - its **guards**, as source text (`found is Some`);
//! - the store fields it **writes**, and how (`:=`, `put`, `append`, …);
//! - the fields it only **reads** (a read of a field after the path writes
//!   it is not listed separately);
//! - the **agents it calls** (`Counter.bump`), classified as the sequence
//!   diagram classifies them;
//! - its **outcomes**.
//!
//! The walk follows the state model's conventions (#2198 DECISION A): the same
//! statement-level scope (ADR 0259), the same store-write recogniser, joins
//! that merge paths producing the same row (here: the same writes, reads and
//! calls, with guards combined and outcomes collected), and the same
//! [`PATH_BUDGET`] backstop — a handler past it is one truncated row. It is a
//! separate, smaller walker: it tracks no state field, so none of the state
//! model's narrowing applies.

use std::collections::{BTreeSet, HashMap, HashSet};

use bynk_syntax::ast::*;
use bynk_syntax::span::Span;

use crate::agent_state::{PATH_BUDGET, combine_guards};
use bynk_check::ast_text::{branch_outcome, pattern_summary, peel_paren};
use bynk_check::store_write::{store_kinds, store_write};

/// One handler's effects.
#[derive(Debug, Clone, PartialEq)]
pub struct HandlerEffects {
    pub handler: String,
    pub rows: Vec<EffectRow>,
    /// The handler exceeded [`PATH_BUDGET`]; `rows` is empty.
    pub truncated: bool,
}

/// One merged path through a handler.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectRow {
    /// Conditions on the path, as source text.
    pub guards: Vec<String>,
    /// Writes in source order.
    pub writes: Vec<EffectWrite>,
    /// Store fields read but not written on this path, sorted.
    pub reads: Vec<String>,
    /// Agent calls as `Agent.method`, sorted.
    pub calls: Vec<String>,
    /// The values the path yields; empty for unit.
    pub outcomes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectWrite {
    pub field: String,
    /// `:=` for a cell assignment, else the store operation (`put`).
    pub op: String,
    /// The written key or item when it is a plain name or literal
    /// (`items.put(id, …)` → `id`), for the label.
    pub key: Option<String>,
    pub span: Span,
}

/// The effects of every handler of `agent`. `agents` names the agents a
/// handler can call (the context's own, as `ContextSequenceInfo::agents`
/// lists them).
pub fn agent_effects(agent: &AgentDecl, agents: &HashSet<String>) -> Vec<HandlerEffects> {
    let store = store_kinds(agent);
    agent
        .handlers
        .iter()
        .map(|h| {
            let mut w = Walker {
                store: &store,
                agents,
                bound: h.params.iter().map(|p| p.name.name.clone()).collect(),
                truncated: false,
            };
            let paths = w.walk_block(&h.body, vec![Path::default()], true);
            let handler = h
                .method_name
                .as_ref()
                .map_or_else(|| "call".to_string(), |m| m.name.clone());
            HandlerEffects {
                handler,
                truncated: w.truncated,
                rows: if w.truncated {
                    Vec::new()
                } else {
                    paths.into_iter().map(Path::into_row).collect()
                },
            }
        })
        .collect()
}

#[derive(Debug, Clone, Default)]
struct Path {
    guards: BTreeSet<String>,
    writes: Vec<EffectWrite>,
    reads: BTreeSet<String>,
    calls: BTreeSet<String>,
    outcomes: BTreeSet<String>,
}

impl Path {
    fn written(&self, field: &str) -> bool {
        self.writes.iter().any(|w| w.field == field)
    }

    /// Paths with the same key produce the same row, guards and outcomes aside.
    /// A write's key is part of it: a row shows, and links to, its own writes.
    fn same_key(&self, other: &Path) -> bool {
        let ops = |p: &Path| -> Vec<(String, String, Option<String>)> {
            p.writes
                .iter()
                .map(|w| (w.field.clone(), w.op.clone(), w.key.clone()))
                .collect()
        };
        ops(self) == ops(other) && self.reads == other.reads && self.calls == other.calls
    }

    fn into_row(self) -> EffectRow {
        EffectRow {
            guards: self.guards.into_iter().collect(),
            writes: self.writes,
            reads: self.reads.into_iter().collect(),
            calls: self.calls.into_iter().collect(),
            outcomes: self.outcomes.into_iter().collect(),
        }
    }
}

struct Walker<'a> {
    store: &'a HashMap<String, String>,
    agents: &'a HashSet<String>,
    bound: Vec<String>,
    truncated: bool,
}

impl Walker<'_> {
    fn walk_block(&mut self, block: &Block, mut paths: Vec<Path>, tail: bool) -> Vec<Path> {
        let scope = self.bound.len();
        for stmt in &block.statements {
            if self.truncated {
                return Vec::new();
            }
            paths = match stmt {
                Statement::Let(l) | Statement::EffectLet(l) => {
                    let paths = self.walk_expr(&l.value, paths, false);
                    self.bound.push(l.name.name.clone());
                    paths
                }
                Statement::Do(d) => self.walk_expr(&d.value, paths, false),
                Statement::Expect(e) => self.walk_expr(&e.value, paths, false),
                Statement::Send(s) => self.walk_expr(&s.value, paths, false),
                Statement::Advance(a) => self.walk_expr(&a.amount, paths, false),
                Statement::Assign(a) => {
                    let mut paths = self.walk_expr(&a.value, paths, false);
                    for p in &mut paths {
                        p.writes.push(EffectWrite {
                            field: a.target.name.clone(),
                            op: ":=".to_string(),
                            key: None,
                            span: a.span,
                        });
                    }
                    paths
                }
            };
        }
        let paths = self.walk_expr(&block.tail, paths, tail);
        self.bound.truncate(scope);
        paths
    }

    fn walk_expr(&mut self, expr: &Expr, mut paths: Vec<Path>, tail: bool) -> Vec<Path> {
        if self.truncated || paths.is_empty() {
            return paths;
        }
        let inner = peel_paren(expr);
        match &inner.kind {
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.scan(cond, &mut paths);
                let text = bynk_fmt::expr_to_string(cond);
                let then_paths = guarded(paths.clone(), text.clone());
                let else_paths = guarded(paths, format!("!({text})"));
                let mut out = self.walk_block(then_block, then_paths, tail);
                out.extend(self.walk_block(else_block, else_paths, tail));
                self.join(out)
            }
            ExprKind::Match { discriminant, arms } => {
                self.scan(discriminant, &mut paths);
                let subject = bynk_fmt::expr_to_string(discriminant);
                let mut out = Vec::new();
                for arm in arms {
                    let mut arm_paths = guarded(
                        paths.clone(),
                        format!("{subject} is {}", pattern_summary(&arm.pattern)),
                    );
                    let scope = self.bound.len();
                    self.bound.extend(
                        arm.pattern
                            .bound_names()
                            .into_iter()
                            .map(|n| n.name.clone()),
                    );
                    if let Some(g) = &arm.guard {
                        self.scan(g, &mut arm_paths);
                        arm_paths = guarded(arm_paths, bynk_fmt::expr_to_string(g));
                    }
                    out.extend(match &arm.body {
                        MatchBody::Expr(e) => self.walk_expr(e, arm_paths, tail),
                        MatchBody::Block(b) => self.walk_block(b, arm_paths, tail),
                    });
                    self.bound.truncate(scope);
                }
                self.join(out)
            }
            ExprKind::Block(b) => self.walk_block(b, paths, tail),
            _ => {
                if let Some(w) = store_write(self.store, &self.bound, inner) {
                    for a in w.args {
                        self.scan(a, &mut paths);
                    }
                    let key = w.args.first().and_then(simple_text);
                    for p in &mut paths {
                        p.writes.push(EffectWrite {
                            field: w.field.name.clone(),
                            op: w.method.name.clone(),
                            key: key.clone(),
                            span: inner.span,
                        });
                    }
                    // A store write yields `()`: no outcome.
                } else {
                    self.scan(inner, &mut paths);
                    if tail && let Some(o) = branch_outcome(inner) {
                        for p in &mut paths {
                            p.outcomes.insert(o.clone());
                        }
                    }
                }
                paths
            }
        }
    }

    /// Record the reads and agent calls in `expr` (descending into every
    /// sub-expression, lambdas included) on each path.
    fn scan(&self, expr: &Expr, paths: &mut [Path]) {
        let mut reads = BTreeSet::new();
        let mut calls = BTreeSet::new();
        let mut shadow = self.bound.clone();
        self.collect(expr, &mut shadow, &mut reads, &mut calls);
        for p in paths {
            let fresh: Vec<String> = reads.iter().filter(|f| !p.written(f)).cloned().collect();
            p.reads.extend(fresh);
            p.calls.extend(calls.iter().cloned());
        }
    }

    /// `shadow` is the bound names in scope: the walker's, plus the
    /// parameters of the lambdas `expr` sits in.
    fn collect(
        &self,
        expr: &Expr,
        shadow: &mut Vec<String>,
        reads: &mut BTreeSet<String>,
        calls: &mut BTreeSet<String>,
    ) {
        if let ExprKind::Lambda(l) = &expr.kind {
            let scope = shadow.len();
            shadow.extend(l.params.iter().map(|p| p.name.name.clone()));
            self.collect(&l.body, shadow, reads, calls);
            shadow.truncate(scope);
            return;
        }
        // A write nested in an argument or lambda is out of scope (as in
        // every agent view): neither a write nor a read of its field.
        if let Some(w) = store_write(self.store, shadow, expr) {
            for a in w.args {
                self.collect(a, shadow, reads, calls);
            }
            return;
        }
        match &expr.kind {
            ExprKind::Ident(id)
                if self.store.contains_key(&id.name) && !shadow.contains(&id.name) =>
            {
                reads.insert(id.name.clone());
            }
            ExprKind::MethodCall {
                receiver, method, ..
            } => {
                if let ExprKind::Call { name, .. } = &receiver.kind
                    && self.agents.contains(&name.name)
                {
                    calls.insert(format!("{}.{}", name.name, method.name));
                }
            }
            _ => {}
        }
        for c in expr_children(expr) {
            self.collect(c, shadow, reads, calls);
        }
    }

    fn join(&mut self, paths: Vec<Path>) -> Vec<Path> {
        let mut out: Vec<Path> = Vec::new();
        for p in paths {
            match out.iter_mut().find(|q| q.same_key(&p)) {
                Some(q) => {
                    q.guards = combine_guards(&q.guards, &p.guards);
                    q.outcomes.extend(p.outcomes);
                }
                None => out.push(p),
            }
        }
        if out.len() > PATH_BUDGET {
            self.truncated = true;
            return Vec::new();
        }
        out
    }
}

fn guarded(mut paths: Vec<Path>, label: String) -> Vec<Path> {
    for p in &mut paths {
        p.guards.insert(label.clone());
    }
    paths
}

/// A plain name or literal argument's text — what a write's label shows.
fn simple_text(arg: &Expr) -> Option<String> {
    matches!(
        peel_paren(arg).kind,
        ExprKind::Ident(_) | ExprKind::IntLit { .. } | ExprKind::StrLit(_) | ExprKind::BoolLit(_)
    )
    .then(|| bynk_fmt::expr_to_string(arg))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effects(src: &str, agent: &str, agents: &[&str]) -> Vec<HandlerEffects> {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("tokenize");
        let (unit, errs) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, src);
        assert!(errs.is_empty(), "parse errors: {errs:?}");
        let Some(SourceUnit::Context(ctx)) = unit else {
            panic!("expected a context")
        };
        let a = ctx
            .items
            .iter()
            .find_map(|i| match i {
                CommonsItem::Agent(a) if a.name.name == agent => Some(a),
                _ => None,
            })
            .expect("agent");
        let names = agents.iter().map(|s| s.to_string()).collect();
        agent_effects(a, &names)
    }

    fn handler<'a>(all: &'a [HandlerEffects], name: &str) -> &'a HandlerEffects {
        all.iter().find(|h| h.handler == name).expect("handler")
    }

    /// `(guards, writes as field.op(key), reads, calls, outcomes)`.
    type Summary = (
        Vec<String>,
        Vec<String>,
        Vec<String>,
        Vec<String>,
        Vec<String>,
    );

    fn summary(h: &HandlerEffects) -> Vec<Summary> {
        h.rows
            .iter()
            .map(|r| {
                let writes = r
                    .writes
                    .iter()
                    .map(|w| match (&w.key, w.op.as_str()) {
                        (_, ":=") => format!("{} :=", w.field),
                        (Some(k), op) => format!("{}.{op}({k})", w.field),
                        (None, op) => format!("{}.{op}", w.field),
                    })
                    .collect();
                (
                    r.guards.clone(),
                    writes,
                    r.reads.clone(),
                    r.calls.clone(),
                    r.outcomes.clone(),
                )
            })
            .collect()
    }

    fn v(x: &[&str]) -> Vec<String> {
        x.iter().map(|s| s.to_string()).collect()
    }

    // examples/todo's agent, inline (bynk-ide tests stay self-contained).
    const TODOS: &str = r#"context todo

type UserId = String where NonEmpty
type Title = String where NonEmpty
type Stored = { seq: Int, title: Title, done: Bool }
type TodoItem = { id: String, title: Title, done: Bool }
type TodoError = enum { NotFound }

fn view(id: String, s: Stored) -> TodoItem {
  TodoItem { id: id, title: s.title, done: s.done }
}

agent Todos {
  key owner: UserId

  store items:   Map[String, Stored]
  store lastSeq: Cell[Int]

  on call add(title: Title) -> Effect[TodoItem] {
    let next = lastSeq + 1
    let id   = "\(next)"
    let item = Stored { seq: next, title: title, done: false }
    let _ <- items.put(id, item)
    lastSeq := next
    view(id, item)
  }

  on call pendingCount() -> Effect[Int] {
    items.values.filter((it) => it.done == false).count()
  }

  on call complete(id: String) -> Effect[Result[(), TodoError]] {
    let found <- items.get(id)
    match found {
      Some(it) => {
        let _ <- items.put(id, Stored { ...it, done: true })
        Ok(())
      }
      None => Err(NotFound)
    }
  }
}
"#;

    #[test]
    fn todos_add_writes_both_fields_and_reads_the_counter_first() {
        let all = effects(TODOS, "Todos", &[]);
        assert_eq!(
            summary(handler(&all, "add")),
            [(
                v(&[]),
                v(&["items.put(id)", "lastSeq :="]),
                v(&["lastSeq"]),
                v(&[]),
                v(&["view(id, item)"])
            )]
        );
    }

    #[test]
    fn todos_complete_writes_only_when_the_item_is_found() {
        let all = effects(TODOS, "Todos", &[]);
        assert_eq!(
            summary(handler(&all, "complete")),
            [
                (
                    v(&["found is Some"]),
                    v(&["items.put(id)"]),
                    v(&["items"]),
                    v(&[]),
                    v(&["Ok(())"])
                ),
                (
                    v(&["found is None"]),
                    v(&[]),
                    v(&["items"]),
                    v(&[]),
                    v(&["Err(NotFound)"])
                ),
            ]
        );
    }

    #[test]
    fn a_query_accessor_is_a_read() {
        let all = effects(TODOS, "Todos", &[]);
        let rows = summary(handler(&all, "pendingCount"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, v(&[]));
        assert_eq!(rows[0].2, v(&["items"]));
    }

    #[test]
    fn the_rate_limiter_reads_then_writes_both_cells() {
        let src = r#"context ratelimit
type RateView = { allowed: Bool }
agent Limiter {
  key client: String
  store windowStart: Cell[Int]
  store count:       Cell[Int]
  on call hit(now: Int) -> Effect[RateView] {
    let d = decide(windowStart, count, now)
    windowStart := d.windowStart
    count := d.count
    RateView { allowed: d.allowed }
  }
}
"#;
        let all = effects(src, "Limiter", &[]);
        let rows = summary(handler(&all, "hit"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, v(&["windowStart :=", "count :="]));
        assert_eq!(rows[0].2, v(&["count", "windowStart"]));
    }

    #[test]
    fn a_log_append_is_a_write_and_its_queries_are_reads() {
        let src = r#"context events
type Event = { kind: String }
agent Activity {
  key stream: String
  store history: Log[Event]
  on call add(event: Event) -> Effect[()] {
    let _ <- history.append(event)
    Effect.pure(())
  }
  on call countSince(t: Int) -> Effect[Int] {
    history.since(t).count()
  }
}
"#;
        let all = effects(src, "Activity", &[]);
        let add = summary(handler(&all, "add"));
        assert_eq!(add[0].1, v(&["history.append(event)"]));
        assert_eq!(add[0].2, v(&[]), "the write's receiver is not also a read");
        let count = summary(handler(&all, "countSince"));
        assert_eq!(count[0].1, v(&[]));
        assert_eq!(count[0].2, v(&["history"]));
    }

    #[test]
    fn agent_calls_are_listed_and_paths_that_agree_merge() {
        let src = r#"context shop
agent Counter {
  key id: String
  store n: Cell[Int]
  on call bump() -> Effect[Int] { n := n + 1
    n }
}
agent Basket {
  key id: String
  store total: Cell[Int]
  on call add(k: String, big: Bool) -> Effect[Int] {
    let c <- Counter(k).bump()
    let _ = if big { total := total + 10
      () } else { total := total + 1
      () }
    c
  }
}
"#;
        let all = effects(src, "Basket", &["Counter", "Basket"]);
        let rows = summary(handler(&all, "add"));
        // Both branches write `total` and read it, so they merge into one row
        // whose guard is either side's — no condition at all.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, v(&[]));
        assert_eq!(rows[0].1, v(&["total :="]));
        assert_eq!(rows[0].2, v(&["total"]));
        assert_eq!(rows[0].3, v(&["Counter.bump"]));
        assert_eq!(rows[0].4, v(&["c"]));
    }

    #[test]
    fn a_shadowed_field_is_neither_read_nor_written() {
        let src = r#"context demo
agent A {
  key id: String
  store items: Map[String, Int]
  on call go(items: List[Int]) -> Effect[Int] {
    items.size()
  }
}
"#;
        let all = effects(src, "A", &[]);
        let rows = summary(handler(&all, "go"));
        assert_eq!(rows[0].1, v(&[]));
        assert_eq!(rows[0].2, v(&[]));
    }

    #[test]
    fn writes_with_different_keys_stay_separate_rows() {
        let src = r#"context demo
agent A {
  key id: String
  store items: Map[String, Int]
  on call go(c: Bool, a: String, b: String, x: Int) -> Effect[()] {
    let _ = if c { let _ <- items.put(a, x)
      () } else { let _ <- items.put(b, x)
      () }
    Effect.pure(())
  }
}
"#;
        let all = effects(src, "A", &[]);
        let h = handler(&all, "go");
        let rows = summary(h);
        // Each row shows, and links to, its own write.
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1, v(&["items.put(a)"]));
        assert_eq!(rows[1].1, v(&["items.put(b)"]));
        assert_ne!(h.rows[0].writes[0].span, h.rows[1].writes[0].span);
    }

    #[test]
    fn a_lambda_parameter_shadows_a_field_and_a_nested_write_is_not_a_read() {
        let src = r#"context demo
agent A {
  key id: String
  store count: Cell[Int]
  store items: Map[String, Int]
  on call go(xs: List[Int], k: String) -> Effect[Int] {
    let ys = xs.map((count) => count + 1)
    let _ = ignore(items.put(k, 1))
    ys.size()
  }
}
"#;
        let all = effects(src, "A", &[]);
        let rows = summary(handler(&all, "go"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, v(&[]), "a nested write is out of scope");
        assert_eq!(rows[0].2, v(&[]), "neither `count` nor `items` is read");
    }

    #[test]
    fn a_handler_past_the_budget_is_truncated() {
        // Nine sequential choices between writing `a` or `b` give 2^9 = 512
        // distinct write sequences, which no join can merge.
        let choices: String = (0..9)
            .map(|i| {
                format!(
                    "    let _ = if c{i} {{ a := {i}\n      () }} else {{ b := {i}\n      () }}\n"
                )
            })
            .collect();
        let params: Vec<String> = (0..9).map(|i| format!("c{i}: Bool")).collect();
        let src = format!(
            "context demo\nagent A {{\n  key id: String\n  store a: Cell[Int]\n  store b: Cell[Int]\n  on call go({}) -> Effect[()] {{\n{choices}    ()\n  }}\n}}\n",
            params.join(", ")
        );
        let all = effects(&src, "A", &[]);
        let go = handler(&all, "go");
        assert!(go.truncated);
        assert!(go.rows.is_empty());
    }
}
