//! #2719: which functions need the execution context, and where one is not
//! available to give them.
//!
//! `~>`, `e.within(d)` and `a.race(b)` hand work to the execution context's
//! `waitUntil`. A service handler reaches it as `deps.__exec`; a function has
//! no `deps`, so a function whose body (or a function it calls, or names as a
//! value) uses one of the three takes it as a hidden trailing parameter. This
//! module computes that *needs-context set* (a fixed point over a unit's
//! functions, against the set its dependencies already settled) and refuses a
//! call that would bring the construct into a place with no context
//! (`bynk.exec.unavailable`).
//!
//! The set is built after type checking, from what the checker resolved: a
//! `within` or `race` counts only when the checker recorded it as the kernel
//! method on an `Effect` receiver (`Callee::Kernel`), never by name, so a user
//! type's own `race` method puts nothing in the set. A `~>` is a
//! [`Statement`] variant, so it cannot be shadowed and is read syntactically.
//!
//! Members are keyed by their declaration's [`Span`]: the file id is unique
//! across a build (dependency packages included), so a callee reached through
//! `uses`, a package alias or a sibling file names the same key as its
//! declaration.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bynk_syntax::ast::{
    Block, ExprId, ExprKind, FnDecl, Handler, HandlerKind, MatchBody, ServiceProtocol, Statement,
    TypeRef, expr_children,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::checker::{Callee, Ty, TypedCommons, Types};

/// Why a function is in the needs-context set.
#[derive(Debug, Clone)]
pub enum Why {
    /// Its own body uses the construct, at `span`.
    Construct { what: &'static str, span: Span },
    /// It calls (or names as a value) a function already in the set.
    Calls { callee: String, key: Span, at: Span },
}

/// The needs-context set for a build so far: every effectful function that
/// takes the hidden `__exec` parameter, and every service handler whose `deps`
/// must carry `__exec` (its body uses a construct, or calls or names a member).
///
/// Units are checked dependency-first, so a unit extends the set its
/// dependencies left and every [`TypedCommons`] of the unit then carries a
/// snapshot of it.
#[derive(Debug, Clone, Default)]
pub struct ExecPlan {
    fns: HashMap<Span, Why>,
    handlers: HashSet<Span>,
    /// The subset that reaches a `within` or `race` (not only a `~>`): what puts
    /// a test module on virtual time, because those register timers on the
    /// harness clock.
    timed_fns: HashSet<Span>,
    timed_handlers: HashSet<Span>,
}

impl ExecPlan {
    /// Whether `f` takes the hidden execution-context parameter.
    pub fn fn_needs(&self, f: &FnDecl) -> bool {
        self.fns.contains_key(&f.span)
    }

    /// Whether this service handler's `deps` carry `__exec`: its body uses
    /// `~>`, `within` or `race`, or calls or names a function that does.
    pub fn handler_needs(&self, h: &Handler) -> bool {
        self.handlers.contains(&h.span)
    }

    /// Whether this service handler reaches a `within` or `race`, directly or
    /// through the functions it calls or names.
    pub fn handler_timed(&self, h: &Handler) -> bool {
        self.timed_handlers.contains(&h.span)
    }

    /// Whether `f` reaches a `within` or `race`.
    pub fn fn_timed(&self, f: &FnDecl) -> bool {
        self.timed_fns.contains(&f.span)
    }

    /// Whether any function in the build takes the execution context.
    pub fn any_fn(&self) -> bool {
        !self.fns.is_empty()
    }

    /// Whether any function in the build reaches a `within` or `race`.
    pub fn any_timed_fn(&self) -> bool {
        !self.timed_fns.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.fns.is_empty() && self.handlers.is_empty()
    }

    /// The chain from `key` down to the construct, as `a → b → within`.
    fn chain(&self, key: Span) -> Option<(Vec<String>, Span)> {
        let mut names = Vec::new();
        let mut at = key;
        // A chain cannot revisit a member (each `Calls` points at a function
        // that joined the set strictly earlier), so this ends; the bound is a
        // belt on that reasoning.
        for _ in 0..=self.fns.len() {
            match self.fns.get(&at)? {
                Why::Construct { what, span } => {
                    names.push(format!("`{what}`"));
                    return Some((names, *span));
                }
                Why::Calls { callee, key, .. } => {
                    names.push(format!("`{callee}`"));
                    at = *key;
                }
            }
        }
        None
    }

    /// Extend the set with one unit's files and report the calls that would
    /// carry the execution context where none exists.
    ///
    /// `files` are all of the unit's checked files: a function in one file may
    /// call a function in a sibling, and a handler's callees are recorded on
    /// whichever file the unit-wide handler check ran against.
    pub fn extend(&mut self, files: &[&TypedCommons]) -> Vec<CompileError> {
        let Some(first) = files.first() else {
            return Vec::new();
        };
        let tys = &*first.ty_intern;
        let callees: HashMap<ExprId, &Callee> = files
            .iter()
            .flat_map(|f| f.callees.iter().map(|(id, c)| (*id, c)))
            .collect();

        struct Node<'a> {
            key: Span,
            facts: Facts,
            kind: NodeKind<'a>,
        }
        enum NodeKind<'a> {
            Fn,
            /// A service handler that has a context to give.
            Handler(&'a Handler),
            /// A body with none: an agent handler, a WebSocket handler or a
            /// provider operation. Only its calls matter, for the diagnostic.
            NoContext(&'static str),
        }

        let mut nodes: Vec<Node> = Vec::new();
        for file in files {
            for item in &file.commons.items {
                use bynk_syntax::ast::CommonsItem;
                match item {
                    CommonsItem::Fn(f) if matches!(f.return_type, TypeRef::Effect(..)) => {
                        nodes.push(Node {
                            key: f.span,
                            facts: scan(&f.body, &callees, tys),
                            kind: NodeKind::Fn,
                        });
                    }
                    CommonsItem::Service(s) => {
                        let ws = matches!(s.protocol, ServiceProtocol::WebSocket { .. });
                        for h in &s.handlers {
                            let facts = scan(&h.body, &callees, tys);
                            let websocket = ws
                                && matches!(
                                    h.kind,
                                    HandlerKind::Open | HandlerKind::Message | HandlerKind::Close
                                );
                            nodes.push(Node {
                                key: h.span,
                                facts,
                                kind: if websocket {
                                    NodeKind::NoContext("a WebSocket handler")
                                } else {
                                    NodeKind::Handler(h)
                                },
                            });
                        }
                    }
                    CommonsItem::Agent(a) => {
                        for h in &a.handlers {
                            nodes.push(Node {
                                key: h.span,
                                facts: scan(&h.body, &callees, tys),
                                kind: NodeKind::NoContext("an agent handler"),
                            });
                        }
                    }
                    CommonsItem::Provider(p) => {
                        for op in &p.ops {
                            nodes.push(Node {
                                key: op.span,
                                facts: scan(&op.body, &callees, tys),
                                kind: NodeKind::NoContext("a provider operation"),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        // The fixed point: a function joins when it uses a construct or has an
        // edge to a member. A member's reason is recorded once, when it joins.
        loop {
            let mut grew = false;
            for n in &nodes {
                if !matches!(n.kind, NodeKind::Fn) || self.fns.contains_key(&n.key) {
                    continue;
                }
                let why = if let Some((what, span)) = n.facts.direct {
                    Some(Why::Construct { what, span })
                } else {
                    n.facts
                        .edges
                        .iter()
                        .find(|e| self.fns.contains_key(&e.key))
                        .map(|e| Why::Calls {
                            callee: e.name.clone(),
                            key: e.key,
                            at: e.at,
                        })
                };
                if let Some(why) = why {
                    self.fns.insert(n.key, why);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }

        loop {
            let mut grew = false;
            for n in &nodes {
                if matches!(n.kind, NodeKind::Fn)
                    && !self.timed_fns.contains(&n.key)
                    && (n.facts.timed
                        || n.facts
                            .edges
                            .iter()
                            .any(|e| self.timed_fns.contains(&e.key)))
                {
                    self.timed_fns.insert(n.key);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }

        let mut errors = Vec::new();
        for n in &nodes {
            match &n.kind {
                NodeKind::Fn => {}
                NodeKind::Handler(h) => {
                    if n.facts.direct.is_some()
                        || n.facts.edges.iter().any(|e| self.fns.contains_key(&e.key))
                    {
                        self.handlers.insert(h.span);
                    }
                    if n.facts.timed
                        || n.facts
                            .edges
                            .iter()
                            .any(|e| self.timed_fns.contains(&e.key))
                    {
                        self.timed_handlers.insert(h.span);
                    }
                }
                NodeKind::NoContext(place) => {
                    for e in &n.facts.edges {
                        let Some((chain, construct)) = self.chain(e.key) else {
                            continue;
                        };
                        let mut path = vec![format!("`{}`", e.name)];
                        path.extend(chain);
                        errors.push(
                            CompileError::new(
                                "bynk.exec.unavailable",
                                e.at,
                                format!(
                                    "`{}` needs an execution context, which {place} does not have",
                                    e.name
                                ),
                            )
                            .with_label(construct, "the execution context is used here")
                            .with_note(format!(
                                "{}: the call hands work to the execution context (`waitUntil`), which only a service handler (not a WebSocket one) supplies; call it from a service handler, or from a function one calls",
                                path.join(" → "),
                            )),
                        );
                    }
                }
            }
        }
        errors
    }
}

/// What one body does with the execution context: a construct it uses itself,
/// and every function it calls or names as a value.
#[derive(Default)]
struct Facts {
    direct: Option<(&'static str, Span)>,
    /// The body uses `within` or `race` itself.
    timed: bool,
    edges: Vec<Edge>,
}

struct Edge {
    name: String,
    key: Span,
    at: Span,
}

fn scan(body: &Block, callees: &HashMap<ExprId, &Callee>, tys: &Types) -> Facts {
    let mut facts = Facts::default();
    visit_block(body, &mut |hit| match hit {
        Hit::Send(span) => {
            facts.direct.get_or_insert(("~>", span));
        }
        Hit::Expr(e) => match callees.get(&e.id) {
            Some(Callee::Kernel { recv, op }) if op == "within" || op == "race" => {
                if matches!(&*tys.get(*recv), Ty::Effect(_)) {
                    facts.timed = true;
                    facts
                        .direct
                        .get_or_insert((if op == "within" { "within" } else { "race" }, e.span));
                }
            }
            Some(
                Callee::Fn(d, _) | Callee::Method(d, _) | Callee::Static(d) | Callee::FnValue(d, _),
            ) => facts.edges.push(Edge {
                name: d.name.display(),
                key: d.span,
                at: e.span,
            }),
            _ => {}
        },
    });
    facts
}

enum Hit<'a> {
    Send(Span),
    Expr(&'a bynk_syntax::ast::Expr),
}

/// Every send statement and expression of `b`, lambdas included. A block
/// reached through [`expr_children`] would lose its statements' kind, so
/// `Block`, `If` and `Match` are descended by hand, as `block_needs_exec` does.
fn visit_block<'a>(b: &'a Block, f: &mut impl FnMut(Hit<'a>)) {
    for s in &b.statements {
        visit_stmt(s, f);
    }
    visit_expr(&b.tail, f);
}

fn visit_stmt<'a>(s: &'a Statement, f: &mut impl FnMut(Hit<'a>)) {
    match s {
        Statement::Send(send) => {
            f(Hit::Send(send.span));
            visit_expr(&send.value, f);
        }
        Statement::Let(l) | Statement::EffectLet(l) => visit_expr(&l.value, f),
        Statement::Expect(a) => visit_expr(&a.value, f),
        Statement::Do(d) => visit_expr(&d.value, f),
        Statement::Assign(a) => visit_expr(&a.value, f),
        Statement::Advance(a) => visit_expr(&a.amount, f),
        Statement::Par(p) => {
            for s in &p.statements {
                visit_stmt(s, f);
            }
        }
    }
}

fn visit_expr<'a>(e: &'a bynk_syntax::ast::Expr, f: &mut impl FnMut(Hit<'a>)) {
    f(Hit::Expr(e));
    match &e.kind {
        ExprKind::Block(b) => visit_block(b, f),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            visit_expr(cond, f);
            visit_block(then_block, f);
            visit_block(else_block, f);
        }
        ExprKind::Match { discriminant, arms } => {
            visit_expr(discriminant, f);
            for a in arms {
                if let Some(g) = &a.guard {
                    visit_expr(g, f);
                }
                match &a.body {
                    MatchBody::Expr(e) => visit_expr(e, f),
                    MatchBody::Block(b) => visit_block(b, f),
                }
            }
        }
        _ => {
            for c in expr_children(e) {
                visit_expr(c, f);
            }
        }
    }
}

/// A shared snapshot, as carried on a [`TypedCommons`].
pub type SharedExecPlan = Arc<ExecPlan>;
