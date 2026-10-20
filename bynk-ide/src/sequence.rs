//! #846: the sequence-diagram query.
//!
//! Classifies a handler body's calls into runtime-participant lifelines —
//! consumed capabilities, calls into consumed contexts, and agents (including
//! same-context agents) — for the "Show Sequence Diagram" VS Code feature.
//! Everything else (commons fns, context-local fns, methods, constructors)
//! folds into the entry participant's own activation: no message is emitted
//! for the call, and — because this repo's resolver does not inline call
//! bodies either — a lifeline call written inside a commons `fn`'s own body is
//! invisible to this walk. That is a stated Tier-1 limitation, not a bug: see
//! `design/pending/sequence-diagram-846.md`.
//!
//! A pure, read-only IDE query: it never touches the checker's hot path
//! (`bynk_check::checker::check_handler_body`) and is built on the exhaustive
//! `expr_children`/`statement_exprs` walkers only insofar as this module's own
//! statement dispatch mirrors their coverage of [`Statement`] — a new
//! `Statement` variant is a compile error here, in `Builder::walk_block`'s
//! `match`, not a silent gap.
//!
//! #2194: in an agent's own handler, a write to the agent's state — a `:=`
//! or a mutating store operation on one of its `store` fields — is not a
//! lifeline either, but it is not folded away: it is a
//! [`MessageKind::StateWrite`] from the entry to itself, rendered as a note.
//!
//! Cross-context/agent calls are boundary-stop (Decision C): one `Call` +
//! one `Return` message: the callee's own body is never walked, even where
//! reachable (an agent's handlers are visible via [`ContextSequenceInfo::agents`]).

use std::collections::HashMap;

use bynk_check::analysis::ContextSequenceInfo;
use bynk_check::ast_text::{branch_outcome, pattern_summary, peel_paren};
use bynk_syntax::ast::*;
use bynk_syntax::span::Span;

/// Which declaration owns the handler being diagrammed.
#[derive(Debug, Clone, Copy)]
pub enum HandlerOwner<'a> {
    Service(&'a str),
    Agent(&'a str),
}

/// Nesting budget for rendered `if`/`match` blocks (issue #846: "~2 levels").
/// Beyond this depth the walk stops classifying calls and emits a single
/// [`AltKind::Collapsed`] marker instead of recursing further.
const MAX_BLOCK_DEPTH: u32 = 2;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SequenceModel {
    pub participants: Vec<Participant>,
    pub messages: Vec<Message>,
    pub blocks: Vec<AltBlock>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Participant {
    pub id: u32,
    pub kind: ParticipantKind,
    pub name: String,
    /// `None` for `Entry` — it has no single declaration site to jump to
    /// (it *is* the handler).
    pub span: Option<Span>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantKind {
    Entry,
    Capability,
    Context,
    Agent,
    /// The handler's **principal** — the `by <Actor>` role (v0.45) that
    /// originates the request. Rendered leftmost, as the sender of the initial
    /// inbound message into the entry, and the recipient of the handler's
    /// replies. Present only for a handler that declares (or inherits) a `by`
    /// clause — HTTP/WebSocket service handlers; agents have no principal.
    Actor,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub from: u32,
    pub to: u32,
    pub kind: MessageKind,
    pub label: String,
    pub span: Span,
    pub block: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Call,
    Return,
    /// `~>` fire-and-forget — no paired `Return`.
    Send,
    /// #2194: an agent handler's write to its own state — a `:=` or a
    /// mutating store operation (`items.put(…)`, `count.update(…)`). Not a
    /// message across a boundary (the store is the agent's own storage, not an
    /// ADR 0258 lifeline): `from == to ==` the entry, and a renderer draws it
    /// as a note over the entry lifeline, in source order within its branch.
    StateWrite,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AltBlock {
    pub id: u32,
    pub kind: AltKind,
    /// Empty for `Collapsed`.
    pub branches: Vec<Branch>,
    pub span: Span,
    pub parent: Option<u32>,
    /// Which of `parent`'s branches this block is nested under — `None` iff
    /// `parent` is `None`. Needed to render nesting correctly: a parent
    /// branch can be entirely empty of messages (the rate-limiter's `if`/
    /// `else` gating only a return, for one), so a renderer walking
    /// `Message.block` alone would have no way to place a nested block whose
    /// own branches are also message-free.
    pub parent_branch: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AltKind {
    If,
    Match,
    Collapsed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Branch {
    pub label: String,
    pub message_ids: Vec<usize>,
    /// The value the handler yields on this branch — its rendered tail
    /// expression (`Ok(view)`, `TooManyRequests(...)`). This is the "reply to
    /// its own caller" ADR 0260 names as always present, but which the
    /// original model never actually emitted: a return-gating `if`/`match`
    /// whose branches call no lifeline produced two empty branches, and an
    /// empty `alt` renders as a mangled zero-width box (issue: rate-limiter's
    /// `GET /check/:client`). Carrying the outcome here gives the block real
    /// content to render as a note over the entry lifeline.
    ///
    /// `None` when the tail carries no distinguishable signal: a unit `()`
    /// tail (an else-less `if`'s synthesised branch), or control flow
    /// (`if`/`match`/block) that is itself already rendered as nested
    /// structure — a note duplicating it would be noise.
    pub reply: Option<String>,
}

/// Whether an expression sits directly under an effect operator
/// (`<-`/`~>`/`do`) — only then can it be a lifeline call, since a bare
/// (pure) `let` cannot bind an `Effect[_]` value in this language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrow {
    /// `let x <- expr` / `do expr` — awaited; a Call+Return pair.
    Awaited,
    /// `~> expr` — fire-and-forget; a Send only.
    FireAndForget,
}

const ENTRY_ID: u32 = 0;

/// The block/branch a walk is currently nested inside — `None` at the
/// handler-body top level. Threaded through every walk method so a nested
/// block can record `parent_branch` and a message can record its `block`.
#[derive(Debug, Clone, Copy)]
struct BlockCtx {
    id: u32,
    branch: u32,
}

/// `default_given` is the owning service's service-level `given` default
/// (v0.155, `ServiceDecl.default_given`) — a handler that declares no `given`
/// of its own inherits it. This classifier walks a freshly-parsed AST that
/// has *not* been through `bynk-emit`'s `inject_service_defaults`
/// normalization pass (which is what mutates `handler.given` in the compile
/// pipeline), so the fallback has to be applied here or a handler relying on
/// the service default would drop every capability lifeline. Pass `&[]` when
/// there is no default (always the case for `HandlerOwner::Agent` — agents
/// have no service-level `given`).
/// `default_by` is the owning service's service-level `by` default (v0.155,
/// `ServiceDecl.default_by`) — a handler that declares no `by` of its own
/// inherits it, exactly as it does the `given` default. Pass `None` for an
/// agent (agents have no principal).
pub fn sequence_model(
    handler: &Handler,
    owner: HandlerOwner<'_>,
    default_given: &[CapRef],
    default_by: Option<&ByClause>,
    info: Option<&ContextSequenceInfo>,
) -> SequenceModel {
    let given = if handler.given.is_empty() {
        default_given
    } else {
        &handler.given
    };
    let by = handler.by_clause.as_ref().or(default_by);
    let mut b = Builder::new(entry_label(handler, owner, by.is_some()), given, info);
    if let HandlerOwner::Agent(name) = owner
        && let Some(agent) = info.and_then(|i| i.agents.get(name))
    {
        b.store = bynk_check::store_write::store_kinds(agent);
        // A parameter named like a store field shadows it for the whole body.
        b.bound = handler.params.iter().map(|p| p.name.name.clone()).collect();
    }
    if let Some(by) = by {
        // The principal originates the request: a leftmost actor lifeline that
        // sends the initial inbound message (the route/method) into the entry.
        // The entry label is bare (`api`) in this case — the request descriptor
        // rides on the message rather than doubling into the entry box.
        b.add_actor(actor_name(by), by.span);
        b.emit_request(discriminator(handler), handler.span);
    }
    // The whole body is in the handler's return position: its tail (and each
    // return-gating branch's tail) is what the handler replies to the actor.
    b.walk_block(&handler.body, None, 0, true);
    b.finish()
}

/// The handler's request descriptor — its method + path (`GET /check/:client`),
/// method name, or lifecycle kind. Labels the initial actor→entry message, and
/// (when there is no actor) the entry box's own discriminator suffix.
fn discriminator(handler: &Handler) -> String {
    match &handler.kind {
        HandlerKind::Call => match handler.method_name.as_ref() {
            Some(m) => m.name.clone(),
            None => "call".to_string(),
        },
        HandlerKind::Http { method, path } => format!("{} {}", method.as_str(), path),
        HandlerKind::Cron { expr } => format!("cron \"{expr}\""),
        HandlerKind::Message => "message".to_string(),
        HandlerKind::Open => "open".to_string(),
        HandlerKind::Close => "close".to_string(),
        HandlerKind::Event => "event".to_string(),
    }
}

/// The actor participant's name — the referenced actor contract(s). More than
/// one (`by who: A | B`, an ordered sum of peer actors) joins with `|`.
fn actor_name(by: &ByClause) -> String {
    by.actors
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The entry lifeline's label. With an actor present the request descriptor
/// moves onto the initial message, so the entry is just its owner name
/// (`api`); without one it keeps the self-describing combined form
/// (`api GET /check/:client`, `Limiter.hit`).
fn entry_label(handler: &Handler, owner: HandlerOwner<'_>, has_actor: bool) -> String {
    match owner {
        HandlerOwner::Service(name) if has_actor => name.to_string(),
        HandlerOwner::Agent(name) if has_actor => name.to_string(),
        HandlerOwner::Service(name) => format!("{name} {}", discriminator(handler)),
        HandlerOwner::Agent(name) => format!("{name}.{}", discriminator(handler)),
    }
}

struct Builder<'a> {
    given: &'a [CapRef],
    info: Option<&'a ContextSequenceInfo>,
    participants: Vec<Participant>,
    messages: Vec<Message>,
    blocks: Vec<AltBlock>,
    /// The principal's participant id, once [`Builder::add_actor`] has run.
    /// Its presence is also the switch for reply routing: with an actor, a
    /// return-position outcome is emitted as a `Return` message *to the actor*;
    /// without one it stays a note on the branch (`Branch::reply`).
    actor: Option<u32>,
    /// #2194: the owning agent's store fields (name → kind head, `"Map"`,
    /// `"Cell"`, …). Empty for a service, which emits no state writes.
    store: HashMap<String, String>,
    /// #2194: names bound in scope (handler parameters, `let`s, match-arm
    /// bindings) — a store field shadowed by one is not a store operation.
    bound: Vec<String>,
}

impl<'a> Builder<'a> {
    fn new(
        entry_label: String,
        given: &'a [CapRef],
        info: Option<&'a ContextSequenceInfo>,
    ) -> Self {
        Builder {
            given,
            info,
            participants: vec![Participant {
                id: ENTRY_ID,
                kind: ParticipantKind::Entry,
                name: entry_label,
                span: None,
            }],
            messages: Vec::new(),
            blocks: Vec::new(),
            actor: None,
            store: HashMap::new(),
            bound: Vec::new(),
        }
    }

    /// Add the principal as the leftmost participant. Inserted at index 0 so it
    /// renders left of the entry, while the entry keeps [`ENTRY_ID`] (`0`) —
    /// participant ids are stable labels, independent of array position, so
    /// every `from: ENTRY_ID` reference stays correct.
    fn add_actor(&mut self, name: String, span: Span) {
        let id = self.participants.len() as u32;
        self.participants.insert(
            0,
            Participant {
                id,
                kind: ParticipantKind::Actor,
                name,
                span: Some(span),
            },
        );
        self.actor = Some(id);
    }

    /// The initial inbound request: a lone `Call` from the actor to the entry
    /// (no paired `Return` — the handler's replies are the branch outcomes).
    /// Spanned at the handler declaration so it sorts ahead of every body
    /// message and click-navigates to the handler.
    fn emit_request(&mut self, label: String, handler_span: Span) {
        let Some(actor) = self.actor else { return };
        self.messages.push(Message {
            from: actor,
            to: ENTRY_ID,
            kind: MessageKind::Call,
            label,
            span: handler_span,
            block: None,
        });
    }

    /// A return-position outcome replied to the principal — a `Return` message
    /// from the entry to the actor. A no-op when there is no actor (the outcome
    /// stays a branch note instead).
    fn emit_reply(&mut self, outcome: String, span: Span, current_block: Option<BlockCtx>) {
        let Some(actor) = self.actor else { return };
        self.messages.push(Message {
            from: ENTRY_ID,
            to: actor,
            kind: MessageKind::Return,
            label: outcome,
            span,
            block: current_block.map(|c| c.id),
        });
    }

    fn finish(self) -> SequenceModel {
        SequenceModel {
            participants: self.participants,
            messages: self.messages,
            blocks: self.blocks,
        }
    }

    fn participant_id(&mut self, kind: ParticipantKind, name: &str, span: Span) -> u32 {
        if let Some(p) = self
            .participants
            .iter()
            .find(|p| p.kind == kind && p.name == name)
        {
            return p.id;
        }
        let id = self.participants.len() as u32;
        self.participants.push(Participant {
            id,
            kind,
            name: name.to_string(),
            span: Some(span),
        });
        id
    }

    /// Walk a block's statements, then its tail — the traversal spine every
    /// call site (top-level body, `if`/`match` branch) shares. `ret` is whether
    /// this block sits in the handler's **return position**: only then is its
    /// tail a value the handler replies to the caller. Statements are never in
    /// return position (their values are bound or discarded); only the tail
    /// inherits the block's `ret`.
    fn walk_block(
        &mut self,
        block: &Block,
        current_block: Option<BlockCtx>,
        depth: u32,
        ret: bool,
    ) {
        // A `let` binds for the rest of its own block only.
        let scope = self.bound.len();
        for stmt in &block.statements {
            match stmt {
                Statement::EffectLet(l) => {
                    self.walk_value(&l.value, current_block, depth, Some(Arrow::Awaited), false);
                    self.bound.push(l.name.name.clone());
                }
                Statement::Do(d) => {
                    self.walk_value(&d.value, current_block, depth, Some(Arrow::Awaited), false)
                }
                Statement::Send(s) => self.walk_value(
                    &s.value,
                    current_block,
                    depth,
                    Some(Arrow::FireAndForget),
                    false,
                ),
                Statement::Let(l) => {
                    self.walk_value(&l.value, current_block, depth, None, false);
                    self.bound.push(l.name.name.clone());
                }
                Statement::Expect(e) => {
                    self.walk_value(&e.value, current_block, depth, None, false)
                }
                Statement::Assign(a) => {
                    self.walk_value(&a.value, current_block, depth, None, false);
                    self.emit_assign(a, current_block);
                }
                Statement::Advance(a) => {
                    self.walk_value(&a.amount, current_block, depth, None, false)
                }
            }
        }
        self.walk_value(&block.tail, current_block, depth, None, ret);
        self.bound.truncate(scope);
    }

    /// Classify one expression reached directly under a statement (or a
    /// block's tail): a lifeline call (only possible when `arrow.is_some()`,
    /// since a pure `let`/tail cannot bind an `Effect[_]`), or `if`/`match`
    /// control flow to recurse into. Anything else — plain computation,
    /// constructors, nested calls buried in argument expressions — folds into
    /// the current activation with no further descent (Tier-1 scope: only
    /// handler-body-level control flow is diagrammed, not arbitrary
    /// expression-level branching).
    fn walk_value(
        &mut self,
        expr: &Expr,
        current_block: Option<BlockCtx>,
        depth: u32,
        arrow: Option<Arrow>,
        ret: bool,
    ) {
        let inner = peel_paren(expr);
        match &inner.kind {
            // Control flow in return position keeps the `ret` flag: each
            // branch's own tail is what the handler ultimately returns.
            ExprKind::If { .. } => self.walk_if(inner, current_block, depth, ret),
            ExprKind::Match { arms, .. } => {
                self.walk_match(inner.span, arms, current_block, depth, ret)
            }
            ExprKind::Block(b) => self.walk_block(b, current_block, depth, ret),
            ExprKind::Call { .. } | ExprKind::MethodCall { .. } => {
                // A write to the agent's own store is a note, not a lifeline —
                // under `<-`/`do`, or as a tail (`balance.update(…)` as a
                // handler's last expression, `Ok(a) => items.put(a, n)`). Its
                // value is `()`, so it is not also a reply.
                if self.emit_store_op(inner, current_block) {
                    return;
                }
                if let Some(arrow) = arrow {
                    self.classify_call(inner, arrow, current_block);
                }
                self.maybe_reply(inner, current_block, ret);
            }
            _ => self.maybe_reply(inner, current_block, ret),
        }
    }

    /// A value tail in return position is the handler's reply to the actor.
    /// A no-op off the return path, on a signal-free outcome (unit), or with
    /// no actor (`emit_reply` itself then does nothing).
    fn maybe_reply(&mut self, expr: &Expr, current_block: Option<BlockCtx>, ret: bool) {
        if !ret {
            return;
        }
        if let Some(outcome) = branch_outcome(expr) {
            self.emit_reply(outcome, expr.span, current_block);
        }
    }

    /// `if_expr` is the (paren-peeled) `ExprKind::If` — destructured here rather
    /// than in the caller to keep the argument list small.
    fn walk_if(&mut self, if_expr: &Expr, current_block: Option<BlockCtx>, depth: u32, ret: bool) {
        let ExprKind::If {
            cond,
            then_block,
            else_block,
        } = &if_expr.kind
        else {
            return;
        };
        let span = if_expr.span;
        if depth >= MAX_BLOCK_DEPTH {
            self.push_collapsed(span, current_block);
            return;
        }
        let id = self.blocks.len() as u32;
        self.blocks.push(AltBlock {
            id,
            kind: AltKind::If,
            branches: Vec::new(),
            span,
            parent: current_block.map(|c| c.id),
            parent_branch: current_block.map(|c| c.branch),
        });
        let then_start = self.messages.len();
        self.walk_block(then_block, Some(BlockCtx { id, branch: 0 }), depth + 1, ret);
        let then_ids = (then_start..self.messages.len()).collect();
        // The condition itself labels the `then` branch (`alt view.allowed`)
        // rather than a bare "then" — the branch predicate is the signal.
        let mut branches = vec![Branch {
            label: bynk_fmt::expr_to_string(cond),
            message_ids: then_ids,
            reply: self.branch_reply(&then_block.tail),
        }];
        // An else-less `if` has a synthesised `()` else block: no messages, no
        // nested blocks, no outcome. Rendering it as a second (empty) branch
        // gains nothing — a single-branch block renders as an `opt`, which is
        // exactly the right shape for a guard with no alternative. A written
        // `else` is always walked and always contributes its branch.
        if !else_block.is_synth_unit() {
            let else_start = self.messages.len();
            self.walk_block(else_block, Some(BlockCtx { id, branch: 1 }), depth + 1, ret);
            branches.push(Branch {
                label: "otherwise".to_string(),
                message_ids: (else_start..self.messages.len()).collect(),
                reply: self.branch_reply(&else_block.tail),
            });
        }
        self.blocks[id as usize].branches = branches;
    }

    /// A branch's `reply` note — its outcome, but only when there is no actor.
    /// With an actor the outcome is instead emitted as a `Return` message to it
    /// (by the return-position walk of the branch tail), so a note would double
    /// it.
    fn branch_reply(&self, tail: &Expr) -> Option<String> {
        if self.actor.is_some() || self.store_op_label(peel_paren(tail)).is_some() {
            // A tail store write is already a state-write note.
            None
        } else {
            branch_outcome(tail)
        }
    }

    fn walk_match(
        &mut self,
        span: Span,
        arms: &[MatchArm],
        current_block: Option<BlockCtx>,
        depth: u32,
        ret: bool,
    ) {
        if depth >= MAX_BLOCK_DEPTH {
            self.push_collapsed(span, current_block);
            return;
        }
        let id = self.blocks.len() as u32;
        self.blocks.push(AltBlock {
            id,
            kind: AltKind::Match,
            branches: Vec::new(),
            span,
            parent: current_block.map(|c| c.id),
            parent_branch: current_block.map(|c| c.branch),
        });
        let mut branches = Vec::with_capacity(arms.len());
        for (arm_index, arm) in arms.iter().enumerate() {
            let start = self.messages.len();
            let branch_ctx = Some(BlockCtx {
                id,
                branch: arm_index as u32,
            });
            // The arm's tail — the value it yields — is its outcome, whether
            // the body is a bare expression or a block. It is in the match's
            // own return position, so a return-position arm replies to the actor.
            let scope = self.bound.len();
            self.bound.extend(
                arm.pattern
                    .bound_names()
                    .into_iter()
                    .map(|n| n.name.clone()),
            );
            let reply = match &arm.body {
                MatchBody::Expr(e) => {
                    self.walk_value(e, branch_ctx, depth + 1, None, ret);
                    self.branch_reply(e)
                }
                MatchBody::Block(b) => {
                    self.walk_block(b, branch_ctx, depth + 1, ret);
                    self.branch_reply(&b.tail)
                }
            };
            self.bound.truncate(scope);
            branches.push(Branch {
                label: pattern_summary(&arm.pattern),
                message_ids: (start..self.messages.len()).collect(),
                reply,
            });
        }
        self.blocks[id as usize].branches = branches;
    }

    /// #2194: a `:=` in an agent handler is a state write. The checker only
    /// admits `:=` against a `store Cell` (`bynk.cell.invalid_target`), so the
    /// target needs no resolution here.
    fn emit_assign(&mut self, a: &AssignStmt, current_block: Option<BlockCtx>) {
        // Only a known store field — the checker guarantees this for checked
        // code, but a file with errors could carry any `:=`.
        if !self.store.contains_key(&a.target.name) {
            return;
        }
        let label = format!(
            "{} := {}",
            a.target.name,
            truncate(&bynk_fmt::expr_to_string(&a.value))
        );
        self.push_state_write(label, a.span, current_block);
    }

    /// #2194: emit a state write for a mutating store operation on one of the
    /// agent's own fields (`items.put(id, …)`) — a bare-name receiver that is
    /// a store field, not shadowed, calling an op the registry marks as
    /// mutating for that field's kind. Returns whether `expr` was one.
    fn emit_store_op(&mut self, expr: &Expr, current_block: Option<BlockCtx>) -> bool {
        let Some(label) = self.store_op_label(expr) else {
            return false;
        };
        self.push_state_write(label, expr.span, current_block);
        true
    }

    /// The state-write label for `expr` when it is a mutating store operation
    /// on one of the agent's own, unshadowed fields; `None` otherwise.
    fn store_op_label(&self, expr: &Expr) -> Option<String> {
        let w = bynk_check::store_write::store_write(&self.store, &self.bound, expr)?;
        Some(format!(
            "{}.{}",
            w.field.name,
            op_call_label(&w.method.name, w.args)
        ))
    }

    fn push_state_write(&mut self, label: String, span: Span, current_block: Option<BlockCtx>) {
        self.messages.push(Message {
            from: ENTRY_ID,
            to: ENTRY_ID,
            kind: MessageKind::StateWrite,
            label,
            span,
            block: current_block.map(|c| c.id),
        });
    }

    fn push_collapsed(&mut self, span: Span, current_block: Option<BlockCtx>) {
        let id = self.blocks.len() as u32;
        self.blocks.push(AltBlock {
            id,
            kind: AltKind::Collapsed,
            branches: Vec::new(),
            span,
            parent: current_block.map(|c| c.id),
            parent_branch: current_block.map(|c| c.branch),
        });
    }

    fn classify_call(&mut self, expr: &Expr, arrow: Arrow, current_block: Option<BlockCtx>) {
        let Some((target, label)) = self.classify_target(expr) else {
            // Local computation (commons/context-local fn, plain method,
            // constructor) — folds into the entry activation, no message.
            return;
        };
        let block = current_block.map(|c| c.id);
        match arrow {
            Arrow::FireAndForget => {
                self.messages.push(Message {
                    from: ENTRY_ID,
                    to: target,
                    kind: MessageKind::Send,
                    label,
                    span: expr.span,
                    block,
                });
            }
            Arrow::Awaited => {
                self.messages.push(Message {
                    from: ENTRY_ID,
                    to: target,
                    kind: MessageKind::Call,
                    label,
                    span: expr.span,
                    block,
                });
                self.messages.push(Message {
                    from: target,
                    to: ENTRY_ID,
                    kind: MessageKind::Return,
                    label: String::new(),
                    span: expr.span,
                    block,
                });
            }
        }
    }

    /// Resolve a call expression's target lifeline, per Decision A:
    /// consumed Capability > Agent > consumed Context. Returns the
    /// participant id and a rendered call label, or `None` when the call is
    /// local computation.
    ///
    /// `TypeName.method(args)` (`Clock.now()`, a local capability op) and
    /// `Consumed.service(args)` (a cross-context call) are syntactically
    /// identical to an ordinary instance method call — the parser has no
    /// static-vs-instance distinction at the receiver, so *every* qualified
    /// call parses uniformly as `ExprKind::MethodCall` with an `Ident`
    /// receiver (the resolver/checker make the static-vs-instance call from
    /// context, which this classifier reimplements against `given`/`agents`/
    /// `cross_context` instead). `Agent(key).method(args)` is the one
    /// receiver shape that differs structurally: the receiver is itself an
    /// `ExprKind::Call` (the agent construction), not a bare `Ident`.
    fn classify_target(&mut self, expr: &Expr) -> Option<(u32, String)> {
        match &expr.kind {
            ExprKind::MethodCall {
                receiver,
                method,
                args,
                ..
            } => match &receiver.kind {
                ExprKind::Call { name, .. }
                    if self.info.is_some_and(|i| i.agents.contains_key(&name.name)) =>
                {
                    let id = self.participant_id(ParticipantKind::Agent, &name.name, name.span);
                    Some((id, call_label(&method.name, args)))
                }
                ExprKind::Ident(id) => self.classify_static(&id.name, &method.name, args, id.span),
                _ => None,
            },
            _ => None,
        }
    }

    /// Classify a bare qualified call `Name.method(args)`: a local
    /// capability op when `Name` is in the handler's effective `given`,
    /// otherwise a cross-context call when `Name` resolves as a consumed
    /// context (or alias). Anything else (a static method on an ordinary
    /// type, a sum-type variant constructor) is local — `None`.
    fn classify_static(
        &mut self,
        name: &str,
        method: &str,
        args: &[Expr],
        span: Span,
    ) -> Option<(u32, String)> {
        if self.given.iter().any(|c| c.key() == name) {
            let id = self.participant_id(ParticipantKind::Capability, name, span);
            return Some((id, call_label(method, args)));
        }
        self.classify_cross_context(name, method, args, span)
    }

    fn classify_cross_context(
        &mut self,
        prefix: &str,
        method: &str,
        args: &[Expr],
        span: Span,
    ) -> Option<(u32, String)> {
        let ctx_name = self.info?.cross_context.resolve_prefix(prefix)?;
        let id = self.participant_id(ParticipantKind::Context, &ctx_name, span);
        Some((id, call_label(method, args)))
    }
}

/// The longest rendered value a state-write note shows before eliding.
const WRITE_LABEL_MAX: usize = 32;

/// Shorten a rendered value to [`WRITE_LABEL_MAX`] characters, ending in `…`.
fn truncate(text: &str) -> String {
    if text.chars().count() <= WRITE_LABEL_MAX {
        text.to_string()
    } else {
        let kept: String = text.chars().take(WRITE_LABEL_MAX - 1).collect();
        format!("{kept}…")
    }
}

/// A store operation's label (#2194, DECISION B): the first argument when it
/// is a plain name or literal — usually the key (`put(id, …)`) or the item
/// (`add(x)`) — and every other argument elided to `…`, since values and
/// combiner lambdas are long and the note only has to say *what* is written.
fn op_call_label(method: &str, args: &[Expr]) -> String {
    let mut rendered = Vec::with_capacity(args.len());
    for (i, arg) in args.iter().enumerate() {
        let simple = matches!(
            peel_paren(arg).kind,
            ExprKind::Ident(_)
                | ExprKind::IntLit { .. }
                | ExprKind::StrLit(_)
                | ExprKind::BoolLit(_)
        );
        if i == 0 && simple {
            rendered.push(truncate(&bynk_fmt::expr_to_string(arg)));
        } else {
            rendered.push("…".to_string());
            break;
        }
    }
    format!("{method}({})", rendered.join(", "))
}

fn call_label(method: &str, args: &[Expr]) -> String {
    let rendered: Vec<String> = args.iter().map(bynk_fmt::expr_to_string).collect();
    format!("{method}({})", rendered.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// Same convention as `symbols.rs`'s `setup_project`: a temp dir unique to
    /// the test name, populated with `(relative_path, contents)` files.
    /// Self-contained fixtures only (never `examples/`) — `bynk-ide` is
    /// published standalone, and a test reaching outside the crate would fail
    /// a `cargo test` on the released tarball.
    fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bynk-ide-sequence-test-{test_name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test root");
        for (rel, contents) in files {
            let p = root.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&p, contents).expect("write file");
        }
        root
    }

    fn parse_context(text: &str) -> Context {
        let tokens = bynk_syntax::lexer::tokenize(text).expect("tokenize");
        let (unit, errs) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, text);
        assert!(errs.is_empty(), "parse errors: {errs:?}");
        match unit.expect("parsed unit") {
            SourceUnit::Context(c) => c,
            _ => panic!("expected a context"),
        }
    }

    fn find_service<'a>(ctx: &'a Context, name: &str) -> &'a ServiceDecl {
        ctx.items
            .iter()
            .find_map(|i| match i {
                CommonsItem::Service(s) if s.name.name == name => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| panic!("service {name} not found"))
    }

    // -- Fixture 1: examples/rate-limiter's `GET /check/:client`, reproduced
    // -- self-contained (not read from `examples/` — see `setup_project`).
    // -- Capability (Clock) + Agent (Limiter) + a return-gating `if` whose
    // -- branches call nothing lifeline-worthy — the regression fixture for
    // -- the corrected AltBlock rule (see the plan's "Corrected extractor
    // -- rule" note): the issue's own worked example renders this block.
    const RATELIMIT_SRC: &str = r#"context ratelimit

consumes bynk { Clock }

type ClientId = String where NonEmpty

type RateView = {
  allowed:   Bool,
  remaining: Int,
  resetAt:   Int,
}

agent Limiter {
  key client: ClientId

  store count: Cell[Int]

  on call hit(now: Int) -> Effect[RateView] {
    let _ <- count.update((c) => c + 1)
    RateView { allowed: count < 10, remaining: 10 - count, resetAt: now }
  }
}

service api from http {
  on GET("/check/:client") (client: ClientId) -> Effect[HttpResult[RateView]] by Visitor given Clock {
    let now  <- Clock.now()
    let view <- Limiter(client).hit(now.toEpochMillis())
    if view.allowed {
      Ok(view)
    } else {
      TooManyRequests("rate limit exceeded")
    }
  }
}
"#;

    #[test]
    fn rate_limiter_get_check_client_classifies_capability_and_agent_and_gates_the_return() {
        let root = setup_project("ratelimit", &[("ratelimit.bynk", RATELIMIT_SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag
            .sequence_info
            .get("ratelimit")
            .expect("sequence_info entry for ratelimit");

        let ctx = parse_context(RATELIMIT_SRC);
        let svc = find_service(&ctx, "api");
        let handler = &svc.handlers[0];

        let model = sequence_model(
            handler,
            HandlerOwner::Service("api"),
            &svc.default_given,
            svc.default_by.as_ref(),
            Some(info),
        );

        // The handler's `by Visitor` principal is the leftmost participant and
        // originates the request; the entry box drops to the bare owner name
        // (`api`) since the route now rides on the inbound message.
        let kinds: Vec<(ParticipantKind, &str)> = model
            .participants
            .iter()
            .map(|p| (p.kind, p.name.as_str()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (ParticipantKind::Actor, "Visitor"),
                (ParticipantKind::Entry, "api"),
                (ParticipantKind::Capability, "Clock"),
                (ParticipantKind::Agent, "Limiter"),
            ]
        );
        let actor_id = model.participants[0].id;

        // The initial inbound request: Visitor -> api, labelled by the route.
        let req = &model.messages[0];
        assert_eq!(req.kind, MessageKind::Call);
        assert_eq!((req.from, req.to), (actor_id, ENTRY_ID));
        assert_eq!(req.label, "GET /check/:client");

        // now() + reply, hit(...) + reply, then a reply-to-actor per branch.
        assert_eq!(
            model.messages.len(),
            7,
            "request + Clock Call/Return + Limiter Call/Return + a reply-to-actor per branch"
        );

        assert_eq!(model.blocks.len(), 1);
        assert_eq!(model.blocks[0].kind, AltKind::If);
        assert_eq!(model.blocks[0].branches.len(), 2);
        // With an actor, each branch carries no *note* (`reply` is None) — its
        // outcome is instead a `Return` message back to the actor, which the
        // `then`/`else` branches own via their `message_ids`.
        let labels: Vec<&str> = model.blocks[0]
            .branches
            .iter()
            .map(|b| b.label.as_str())
            .collect();
        assert_eq!(labels, vec!["view.allowed", "otherwise"]);
        assert!(
            model.blocks[0].branches.iter().all(|b| b.reply.is_none()),
            "with an actor the outcome is a message, not a note"
        );
        let branch_replies: Vec<(MessageKind, u32, u32, &str)> = model.blocks[0]
            .branches
            .iter()
            .flat_map(|b| &b.message_ids)
            .map(|&i| {
                let m = &model.messages[i];
                (m.kind, m.from, m.to, m.label.as_str())
            })
            .collect();
        assert_eq!(
            branch_replies,
            vec![
                (MessageKind::Return, ENTRY_ID, actor_id, "Ok(view)"),
                (
                    MessageKind::Return,
                    ENTRY_ID,
                    actor_id,
                    "TooManyRequests(\"rate limit exceeded\")",
                ),
            ],
            "each branch replies its outcome to the actor"
        );
    }

    // -- Fixture 2: a consumed-context call — boundary-stop (Decision C).
    const PLATFORM_SRC: &str = r#"context platform

service Pinger {
  on call(n: Int) -> Effect[Int] {
    n
  }
}
"#;
    const CONSUMER_SRC: &str = r#"context consumer

consumes platform

service api {
  on call(n: Int) -> Effect[Int] {
    let v <- platform.Pinger(n)
    v
  }
}
"#;

    #[test]
    fn cross_context_call_is_boundary_stop() {
        let root = setup_project(
            "crossctx",
            &[
                ("platform.bynk", PLATFORM_SRC),
                ("consumer.bynk", CONSUMER_SRC),
            ],
        );
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag
            .sequence_info
            .get("consumer")
            .expect("sequence_info entry for consumer");

        let ctx = parse_context(CONSUMER_SRC);
        let svc = find_service(&ctx, "api");
        let handler = &svc.handlers[0];
        let model = sequence_model(
            handler,
            HandlerOwner::Service("api"),
            &svc.default_given,
            svc.default_by.as_ref(),
            Some(info),
        );

        assert_eq!(model.participants.len(), 2, "Entry + the consumed context");
        assert_eq!(model.participants[1].kind, ParticipantKind::Context);
        assert_eq!(model.participants[1].name, "platform");
        assert_eq!(
            model.messages.len(),
            2,
            "one Call + one Return — the consumed service's own body is never walked"
        );
        assert_eq!(model.messages[0].kind, MessageKind::Call);
        assert_eq!(model.messages[1].kind, MessageKind::Return);
    }

    // -- Fixtures 3-5: fire-and-forget send, a degenerate (local-only)
    // -- handler, and 3-level-nested `if` past the depth budget.
    const MISC_SRC: &str = r#"context misc

consumes bynk { Clock, Logger }

fn double(x: Int) -> Int {
  x * 2
}

service fireService {
  on call(n: Int) -> Effect[()] given Logger {
    ~> Logger.info("hi")
    Effect.pure(())
  }
}

service localService {
  on call(n: Int) -> Effect[Int] {
    double(n)
  }
}

service nestedService {
  on call(n: Int) -> Effect[Int] given Clock {
    let now <- Clock.now()
    if n > 0 {
      if n > 10 {
        if n > 100 {
          now.toEpochMillis()
        } else {
          1
        }
      } else {
        2
      }
    } else {
      3
    }
  }
}
"#;

    fn misc_info(diag: &crate::ProjectDiagnostics) -> bynk_check::analysis::ContextSequenceInfo {
        diag.sequence_info
            .get("misc")
            .cloned()
            .expect("sequence_info entry for misc")
    }

    #[test]
    fn fire_and_forget_send_has_no_paired_return() {
        let root = setup_project("misc-send", &[("misc.bynk", MISC_SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = misc_info(&diag);

        let ctx = parse_context(MISC_SRC);
        let svc = find_service(&ctx, "fireService");
        let handler = &svc.handlers[0];
        let model = sequence_model(
            handler,
            HandlerOwner::Service("fireService"),
            &svc.default_given,
            svc.default_by.as_ref(),
            Some(&info),
        );

        assert_eq!(model.participants.len(), 2);
        assert_eq!(model.participants[1].kind, ParticipantKind::Capability);
        assert_eq!(model.participants[1].name, "Logger");
        assert_eq!(model.messages.len(), 1, "a Send has no paired Return");
        assert_eq!(model.messages[0].kind, MessageKind::Send);
    }

    #[test]
    fn degenerate_handler_with_only_local_calls_has_no_lifelines() {
        let root = setup_project("misc-local", &[("misc.bynk", MISC_SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = misc_info(&diag);

        let ctx = parse_context(MISC_SRC);
        let svc = find_service(&ctx, "localService");
        let handler = &svc.handlers[0];
        let model = sequence_model(
            handler,
            HandlerOwner::Service("localService"),
            &svc.default_given,
            svc.default_by.as_ref(),
            Some(&info),
        );

        assert_eq!(model.participants.len(), 1);
        assert_eq!(model.participants[0].kind, ParticipantKind::Entry);
        assert!(model.messages.is_empty());
        assert!(model.blocks.is_empty());
    }

    #[test]
    fn nested_if_collapses_past_the_depth_budget() {
        let root = setup_project("misc-nested", &[("misc.bynk", MISC_SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = misc_info(&diag);

        let ctx = parse_context(MISC_SRC);
        let svc = find_service(&ctx, "nestedService");
        let handler = &svc.handlers[0];
        let model = sequence_model(
            handler,
            HandlerOwner::Service("nestedService"),
            &svc.default_given,
            svc.default_by.as_ref(),
            Some(&info),
        );

        // Depth 0 (`n > 0`) and depth 1 (`n > 10`) render as real blocks;
        // depth 2 (`n > 100`) is past `MAX_BLOCK_DEPTH` and collapses instead
        // of expanding further — click-to-code still works (the collapsed
        // marker's span points at the whole collapsed `if`), the diagram
        // just doesn't recurse into it.
        let kinds: Vec<AltKind> = model.blocks.iter().map(|b| b.kind).collect();
        assert_eq!(kinds, vec![AltKind::If, AltKind::If, AltKind::Collapsed]);
        assert!(
            model.blocks[2].branches.is_empty(),
            "a Collapsed block carries no branches"
        );
        // Regression: each nested block must record which branch of its
        // parent it sits under, not just the parent id — both the middle
        // and innermost `if` are nested in their parent's *first* ("then")
        // branch, and a renderer needs that even when the branch itself
        // carries no messages of its own (only the nested block does).
        assert_eq!(model.blocks[0].parent, None);
        assert_eq!(model.blocks[0].parent_branch, None);
        assert_eq!(model.blocks[1].parent, Some(0));
        assert_eq!(model.blocks[1].parent_branch, Some(0));
        assert_eq!(model.blocks[2].parent, Some(1));
        assert_eq!(model.blocks[2].parent_branch, Some(0));
    }

    // -- Fixture 6 (#861 review): a service declares a service-level `given`
    // -- default (v0.155) and the handler omits its own `given`, inheriting it.
    // -- The classifier walks a freshly-parsed AST that never ran
    // -- `inject_service_defaults`, so it must apply the fallback itself — or
    // -- the Clock capability lifeline is silently dropped.
    const SERVICE_GIVEN_SRC: &str = r#"context svcgiven

consumes bynk { Clock }

service api from http by Visitor given Clock {
  on GET("/now") () -> Effect[HttpResult[Int]] {
    let now <- Clock.now()
    Ok(now.toEpochMillis())
  }
}
"#;

    #[test]
    fn service_level_given_default_is_inherited_by_a_handler_without_its_own() {
        let root = setup_project("svcgiven", &[("svcgiven.bynk", SERVICE_GIVEN_SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag
            .sequence_info
            .get("svcgiven")
            .expect("sequence_info entry for svcgiven");

        let ctx = parse_context(SERVICE_GIVEN_SRC);
        let svc = find_service(&ctx, "api");
        let handler = &svc.handlers[0];
        // Precondition: the freshly-parsed handler carries no `given` of its
        // own — it relies entirely on the service-level default.
        assert!(
            handler.given.is_empty(),
            "fixture handler must omit its own `given`"
        );
        assert_eq!(svc.default_given.len(), 1, "service-level `given Clock`");
        assert!(
            handler.by_clause.is_none() && svc.default_by.is_some(),
            "fixture handler must inherit the service-level `by Visitor`"
        );

        let model = sequence_model(
            handler,
            HandlerOwner::Service("api"),
            &svc.default_given,
            svc.default_by.as_ref(),
            Some(info),
        );

        let kinds: Vec<(ParticipantKind, &str)> = model
            .participants
            .iter()
            .map(|p| (p.kind, p.name.as_str()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                // Both service-level defaults are inherited: `by Visitor` (the
                // actor) and `given Clock` (the capability lifeline).
                (ParticipantKind::Actor, "Visitor"),
                (ParticipantKind::Entry, "api"),
                (ParticipantKind::Capability, "Clock"),
            ],
            "Clock must classify via the inherited `given`, and Visitor via the \
             inherited `by` — with only `handler.given`/`handler.by_clause` both \
             would be dropped"
        );

        // A non-branching handler tail still replies to the actor: the last
        // message is the `Ok(...)` return, entry -> Visitor.
        let actor_id = model.participants[0].id;
        let last = model.messages.last().expect("at least one message");
        assert_eq!(last.kind, MessageKind::Return);
        assert_eq!((last.from, last.to), (ENTRY_ID, actor_id));
        assert_eq!(last.label, "Ok(now.toEpochMillis())");
    }

    // -- Fixture 7 (issue): branch outcomes + else-less-if shape. An else-less
    // -- `if` renders as an `opt` (one branch, not an empty second branch), and
    // -- a `match` captures each arm's yielded value as its reply — the content
    // -- that keeps a return-gating block from rendering as an empty box.
    const OUTCOME_SRC: &str = r#"context outcome

consumes bynk { Logger }

service guardSvc {
  on call(flag: Bool) -> Effect[()] given Logger {
    if flag {
      ~> Logger.info("hi")
    }
  }
}

service routeSvc {
  on call(x: Int) -> Effect[Int] {
    match x {
      0 => 100
      _ => x
    }
  }
}
"#;

    #[test]
    fn else_less_if_is_a_single_branch_opt_and_match_arms_capture_replies() {
        // `info` is `None` here: this fixture references only a capability
        // (`given Logger`) and control flow — no agent or cross-context call —
        // so classification needs no project table, and this stays a pure
        // parse-only test (the full project wouldn't type-check anyway).
        let ctx = parse_context(OUTCOME_SRC);
        let guard_svc = find_service(&ctx, "guardSvc");
        let route_svc = find_service(&ctx, "routeSvc");

        // `guard`: an else-less `if` — one branch (an `opt`), labelled by its
        // condition. Its `then` carries the Send but its tail is unit, so it
        // has no reply of its own.
        let guard = &guard_svc.handlers[0];
        let gm = sequence_model(
            guard,
            HandlerOwner::Service("guardSvc"),
            &guard_svc.default_given,
            guard_svc.default_by.as_ref(),
            None,
        );
        assert_eq!(gm.blocks.len(), 1);
        assert_eq!(gm.blocks[0].kind, AltKind::If);
        assert_eq!(
            gm.blocks[0].branches.len(),
            1,
            "an else-less `if` renders as an `opt`, not an `alt` with an empty second branch"
        );
        assert_eq!(gm.blocks[0].branches[0].label, "flag");
        assert_eq!(
            gm.blocks[0].branches[0].reply, None,
            "a unit tail has no reply"
        );
        assert_eq!(
            gm.blocks[0].branches[0].message_ids.len(),
            1,
            "the `~>` Send"
        );

        // `route`: a `match` whose arms yield distinct values — each captured
        // as that branch's reply.
        let route = &route_svc.handlers[0];
        let rm = sequence_model(
            route,
            HandlerOwner::Service("routeSvc"),
            &route_svc.default_given,
            route_svc.default_by.as_ref(),
            None,
        );
        assert_eq!(rm.blocks.len(), 1);
        assert_eq!(rm.blocks[0].kind, AltKind::Match);
        let replies: Vec<Option<&str>> = rm.blocks[0]
            .branches
            .iter()
            .map(|b| b.reply.as_deref())
            .collect();
        assert_eq!(replies, vec![Some("100"), Some("x")]);
    }

    // -- #2194: an agent's *own* handler — state writes as notes.
    const ORDER_SRC: &str = r#"context orders

type OrderStatus = enum { Pending, Placed, Cancelled }
type OrderError  = enum { AlreadyPlaced, AlreadyCancelled }

agent Counter {
  key id: String

  store total: Cell[Int]

  on call bump() -> Effect[Int] {
    total := total + 1
    total
  }
}

agent Order {
  key id: String

  store status: Cell[OrderStatus] = Pending
  store items:  Map[String, Int]
  store tally:  Cell[Int]

  on call place() -> Effect[Result[(), OrderError]] {
    match status {
      Pending => {
        status := Placed
        Ok(())
      }
      Placed    => Err(AlreadyPlaced)
      Cancelled => Err(AlreadyCancelled)
    }
  }

  on call add(sku: String, qty: Int) -> Effect[Bool] {
    let n <- Counter(sku).bump()
    let _ <- items.put(sku, qty + n)
    let _ <- tally.update((t) => t + qty)
    let known <- items.contains(sku)
    known
  }

  on call bumpTally(n: Int) -> Effect[()] {
    tally.update((t) => t + n)
  }

  on call putIfSome(k: String, found: Option[Int]) -> Effect[()] {
    match found {
      Some(v) => items.put(k, v),
      None    => Effect.pure(()),
    }
  }
}
"#;

    fn find_agent<'a>(ctx: &'a Context, name: &str) -> &'a AgentDecl {
        ctx.items
            .iter()
            .find_map(|i| match i {
                CommonsItem::Agent(a) if a.name.name == name => Some(a),
                _ => None,
            })
            .unwrap_or_else(|| panic!("agent {name} not found"))
    }

    fn agent_model(test_name: &str, handler_src: &str, agent: &str, method: &str) -> SequenceModel {
        let root = setup_project(test_name, &[("orders.bynk", ORDER_SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag
            .sequence_info
            .get("orders")
            .expect("sequence_info for orders");
        let ctx = parse_context(handler_src);
        let a = find_agent(&ctx, agent);
        let h = a
            .handlers
            .iter()
            .find(|h| h.method_name.as_ref().is_some_and(|m| m.name == method))
            .expect("handler");
        sequence_model(h, HandlerOwner::Agent(agent), &[], None, Some(info))
    }

    fn writes(model: &SequenceModel) -> Vec<(&str, Option<u32>)> {
        model
            .messages
            .iter()
            .filter(|m| m.kind == MessageKind::StateWrite)
            .map(|m| (m.label.as_str(), m.block))
            .collect()
    }

    #[test]
    fn an_assignment_in_an_agent_handler_is_a_state_write_inside_its_branch() {
        let model = agent_model("assign", ORDER_SRC, "Order", "place");
        assert_eq!(writes(&model), [("status := Placed", Some(0))]);
        let w = &model.messages[0];
        assert_eq!((w.from, w.to), (ENTRY_ID, ENTRY_ID));
        // The write is a member of the `Pending` branch, ahead of its reply.
        let pending = &model.blocks[0].branches[0];
        assert_eq!(pending.label, "Pending");
        assert_eq!(pending.message_ids, [0]);
        assert_eq!(pending.reply.as_deref(), Some("Ok(())"));
        // The refusing branches write nothing.
        assert!(model.blocks[0].branches[1].message_ids.is_empty());
    }

    #[test]
    fn mutating_store_ops_are_state_writes_in_order_and_reads_are_not() {
        let model = agent_model("storeops", ORDER_SRC, "Order", "add");
        let kinds: Vec<(MessageKind, &str)> = model
            .messages
            .iter()
            .map(|m| (m.kind, m.label.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                (MessageKind::Call, "bump()"),
                (MessageKind::Return, ""),
                (MessageKind::StateWrite, "items.put(sku, …)"),
                (MessageKind::StateWrite, "tally.update(…)"),
            ],
            "the agent call keeps its lifeline; `contains` is a read and shows nothing"
        );
    }

    #[test]
    fn an_assignment_in_another_agent_is_still_a_write_of_its_own() {
        let model = agent_model("counter", ORDER_SRC, "Counter", "bump");
        assert_eq!(writes(&model), [("total := total + 1", None)]);
    }

    #[test]
    fn a_store_field_shadowed_by_a_parameter_or_let_is_not_a_store_op() {
        // Parsed only — the store fields come from ORDER_SRC's checked `Order`.
        let shadowing = r#"context orders
agent Order {
  key id: String
  store items: Map[String, Int]
  on call byParam(items: List[Int]) -> Effect[()] {
    let _ <- items.put("a", 1)
    ()
  }
  on call byLet() -> Effect[()] {
    let tally = Thing()
    let _ <- tally.update((t) => t)
    let _ <- items.remove("a")
    ()
  }
}
"#;
        assert!(writes(&agent_model("shadow-param", shadowing, "Order", "byParam")).is_empty());
        assert_eq!(
            writes(&agent_model("shadow-let", shadowing, "Order", "byLet")),
            [("items.remove(\"a\")", None)],
            "only the unshadowed field's write is a note"
        );
    }

    #[test]
    fn a_long_right_hand_side_is_truncated() {
        assert_eq!(truncate("short"), "short");
        let long = "a".repeat(40);
        let t = truncate(&long);
        assert_eq!(t.chars().count(), WRITE_LABEL_MAX);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn a_service_handler_emits_no_state_writes() {
        let root = setup_project("svc-nowrites", &[("ratelimit.bynk", RATELIMIT_SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag.sequence_info.get("ratelimit").expect("info");
        let ctx = parse_context(RATELIMIT_SRC);
        let svc = find_service(&ctx, "api");
        let model = sequence_model(
            &svc.handlers[0],
            HandlerOwner::Service("api"),
            &svc.default_given,
            svc.default_by.as_ref(),
            Some(info),
        );
        assert!(writes(&model).is_empty());
    }

    #[test]
    fn the_rate_limiters_own_hit_handler_shows_its_cell_update() {
        let root = setup_project("limiter-hit", &[("ratelimit.bynk", RATELIMIT_SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag.sequence_info.get("ratelimit").expect("info");
        let ctx = parse_context(RATELIMIT_SRC);
        let a = find_agent(&ctx, "Limiter");
        let model = sequence_model(
            &a.handlers[0],
            HandlerOwner::Agent("Limiter"),
            &[],
            None,
            Some(info),
        );
        assert_eq!(writes(&model), [("count.update(…)", None)]);
    }

    #[test]
    fn a_store_write_in_tail_position_is_a_note_and_not_also_a_reply() {
        // `Wallet.topUp`'s shape: the write is the handler's last expression.
        let model = agent_model("tail-write", ORDER_SRC, "Order", "bumpTally");
        assert_eq!(writes(&model), [("tally.update(…)", None)]);
        assert_eq!(model.messages.len(), 1, "no reply duplicating the write");
    }

    #[test]
    fn a_store_write_as_an_expression_match_arm_is_a_note_in_that_arm() {
        // `Ledger.open`'s shape: `Ok(acct) => balances.put(acct, n)`.
        let model = agent_model("arm-write", ORDER_SRC, "Order", "putIfSome");
        assert_eq!(writes(&model), [("items.put(k, …)", Some(0))]);
        let arms = &model.blocks[0].branches;
        assert_eq!(arms[0].message_ids, [0]);
        assert_eq!(
            arms[0].reply, None,
            "the write is the arm's note, not its reply"
        );
        assert!(arms[1].message_ids.is_empty());
        assert_eq!(arms[1].reply.as_deref(), Some("Effect.pure(())"));
    }

    #[test]
    fn an_assignment_to_a_name_that_is_not_a_store_field_is_not_labelled() {
        // Parsed only: a file with errors can carry a `:=` the checker refuses.
        let src = r#"context orders
agent Order {
  key id: String
  store items: Map[String, Int]
  on call bad() -> Effect[()] {
    nope := 1
    ()
  }
}
"#;
        assert!(writes(&agent_model("bad-assign", src, "Order", "bad")).is_empty());
    }
}
