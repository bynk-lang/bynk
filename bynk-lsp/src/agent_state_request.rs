//! #2196: `bynk/agentStateModel` — the agent state view's custom LSP request
//! (agent-state-views track, slice 2).
//!
//! Agent-scoped, not handler-scoped: the cursor anywhere inside an `agent`
//! declaration (its header, a `store` field, a handler body) selects that
//! agent, and the response is the whole agent's state model
//! ([`bynk_ide::agent_state`]). Served from the committed round by
//! re-parsing the snapshot, like `bynk/sequenceModel` (ADR 0301's posture); no
//! refresh nudge, since the client re-pulls (ADR 0504).
//!
//! Also here: the sites of the per-agent `Show State` lens, and the wire
//! shape — a serde mirror of the model with `Span`s lowered to LSP `Range`s,
//! plus each handler's name and range (the table's rows, and what the client
//! highlights for the handler under the cursor).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::wire_contract_request::{WcInst, WcRef, WcType, wc_ref};
use bynk_ide::agent_effects::{self, HandlerEffects, StoredType};
use bynk_ide::agent_state::{
    self, AgentInvariant, AgentStateModel, Certainty, InvariantKind, Target,
};
use bynk_syntax::ast::{AgentDecl, CommonsItem, SourceUnit, TypeDecl, TypeRef};
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::Range;

/// The `bynk/agentStateModel` request payload. `rename_all = "camelCase"` is
/// load-bearing — the client sends `textDocument`/`position` (see
/// `sequence_request::SequenceModelParams`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStateModelParams {
    pub text_document: tower_lsp::lsp_types::TextDocumentIdentifier,
    pub position: tower_lsp::lsp_types::Position,
}

/// The agents of `text`, re-parsed. Empty for a commons, a suite, or a file
/// that doesn't tokenize.
fn agents(text: &str) -> Vec<AgentDecl> {
    let Ok(tokens) = bynk_syntax::lexer::tokenize(text) else {
        return Vec::new();
    };
    let (unit, _errs) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, text);
    let items = match unit {
        Some(SourceUnit::Context(c)) => c.items,
        Some(SourceUnit::Adapter(a)) => a.items,
        _ => return Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|i| match i {
            CommonsItem::Agent(a) => Some(a),
            _ => None,
        })
        .collect()
}

/// The agent enclosing `offset`, with its model built against `types` (the
/// unit's combined type table). `None` outside every agent.
pub fn agent_state_at(
    text: &str,
    offset: usize,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<(AgentDecl, AgentStateModel)> {
    let agent = agents(text)
        .into_iter()
        .find(|a| a.span.start <= offset && offset < a.span.end)?;
    let model = agent_state::agent_state_model(&agent, types);
    Some((agent, model))
}

/// Every agent's name, for the per-agent `Show State` lens.
pub fn agent_lens_sites(text: &str) -> Vec<Span> {
    agents(text).iter().map(|a| a.name.span).collect()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireAgentStateModel {
    pub agent: String,
    pub range: Range,
    pub handlers: Vec<WireHandler>,
    pub machines: Vec<WireStateMachine>,
    /// #2198: what each handler writes, reads and calls, path by path.
    pub effects: Vec<WireHandlerEffects>,
    /// #3608: the agent's `invariant`s and `transition`s; omitted when it
    /// declares neither.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub invariants: Vec<WireInvariant>,
    /// #3803: the wire contract's type table for the agent's `store` types, which the writes'
    /// `valueRef`s and `fieldRef`s point into; omitted when the agent has no named or generic
    /// store type.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<crate::wire_contract_request::WcType>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub instantiations: Vec<crate::wire_contract_request::WcInst>,
}

/// #3608: one declared `invariant` or `transition`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WireInvariant {
    /// `"invariant"` or `"transition"`.
    pub kind: &'static str,
    pub name: String,
    /// The predicate's source text, printed by the expression printer.
    pub predicate: String,
    pub range: Range,
    /// The declaration's documentation text; omitted when it has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

fn wire_invariants(invariants: &[AgentInvariant], text: &str) -> Vec<WireInvariant> {
    invariants
        .iter()
        .map(|i| WireInvariant {
            kind: match i.kind {
                InvariantKind::Invariant => "invariant",
                InvariantKind::Transition => "transition",
            },
            name: i.name.clone(),
            predicate: i.predicate.clone(),
            range: crate::position::span_to_range(text, i.span),
            doc: i.doc.clone(),
        })
        .collect()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireHandlerEffects {
    pub handler: String,
    pub rows: Vec<WireEffectRow>,
    pub truncated: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireEffectRow {
    pub guards: Vec<String>,
    pub writes: Vec<WireEffectWrite>,
    pub reads: Vec<String>,
    pub calls: Vec<String>,
    pub outcomes: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireEffectWrite {
    pub field: String,
    pub op: String,
    pub key: Option<String>,
    pub range: Range,
    /// #3803: the type of the value the write stores, as source text (`List[Line]`): a cell's
    /// `T`, a map's `V`, an `append`ed or `add`ed element. Omitted for a write that stores none
    /// (`remove`) and for a field that is not a `store` field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_type: Option<String>,
    /// #3803: the field's type once the write has happened: the cell's `T`, or the collection
    /// (`Map[String, Order]`, `Log[Line]`), so an `append` or `put` reports the element and the
    /// collection it grew.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_type: Option<String>,
    /// #3803: [`Self::value_type`] as a reference into this model's `types` and `instantiations`,
    /// the table `bynk/wireContract` uses, when the type has a wire shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_ref: Option<crate::wire_contract_request::WcRef>,
    /// #3803: [`Self::field_type`] as a reference, for a cell, whose field type is a value type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_ref: Option<crate::wire_contract_request::WcRef>,
}

/// #2198: the effects of `agent`'s handlers, with `agents` naming the
/// agents a handler can call (the context's agent table).
pub fn effects_of(agent: &AgentDecl, agents: &HashSet<String>) -> Vec<HandlerEffects> {
    agent_effects::agent_effects(agent, agents)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireHandler {
    pub name: String,
    pub range: Range,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireStateMachine {
    pub field: String,
    pub field_range: Range,
    pub states: Vec<String>,
    pub initial: Option<String>,
    pub edges: Vec<WireEdge>,
    pub reachable: Vec<String>,
    /// #2219: the states the agent can be in after a commit — the only start
    /// states the overlay checks, since the genesis commit runs no
    /// `transition`.
    pub reachable_after_commit: Vec<String>,
    pub truncated: Vec<String>,
    /// #2197: `transition`s on the field the overlay could not evaluate.
    pub not_evaluated: Vec<WireNamed>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireNamed {
    pub name: String,
    pub range: Range,
}

/// #2197: a `transition` this edge can violate at commit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WireViolation {
    pub transition: String,
    pub range: Range,
    /// `"Known"` or `"Possible"`.
    pub certainty: &'static str,
    pub from: Vec<String>,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireEdge {
    pub handler: String,
    pub from: Vec<String>,
    pub guards: Vec<String>,
    /// `{ "kind": "State", "state": "Placed" }`, `{ "kind": "Unknown" }` or
    /// `{ "kind": "Unchanged" }`.
    pub to: WireTarget,
    pub outcome: Option<String>,
    pub range: Range,
    pub violations: Vec<WireViolation>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum WireTarget {
    State { state: String },
    Unknown,
    Unchanged,
}

/// A type as source text, each qualified key read as its declaration's name, as
/// `bynk/wireContract` shows its types.
fn type_text(t: &TypeRef) -> String {
    bynk_check::symbols::display_type_names_in(&bynk_ide::symbols::type_ref_str(t))
}

/// `t` as a reference into the wire contract's type table, when it has a wire shape: a function,
/// query, stream, connection or history type has none, and is never a stored value.
fn type_ref_of(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> Option<WcRef> {
    match t {
        TypeRef::Fn(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..) => None,
        _ => Some(wc_ref(&bynk_check::wire::wire_ref(t, types))),
    }
}

/// #3803: the type fields of one write, from what its field's declared kind stores.
fn stored_fields(
    stored: Option<&StoredType>,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> (Option<String>, Option<String>, Option<WcRef>, Option<WcRef>) {
    let Some(stored) = stored else {
        return (None, None, None, None);
    };
    let value = stored.value.as_ref();
    let cell_type = (stored.kind.head.name == "Cell")
        .then(|| stored.kind.args.first())
        .flatten();
    let field_type = match cell_type {
        Some(t) => type_text(t),
        None => bynk_check::symbols::display_type_names_in(&bynk_ide::symbols::store_kind_text(
            &stored.kind,
        )),
    };
    (
        value.map(type_text),
        Some(field_type),
        value.and_then(|t| type_ref_of(t, types)),
        cell_type.and_then(|t| type_ref_of(t, types)),
    )
}

/// [`to_wire_with_types`] with no type table: the writes still say what they store, as text.
pub fn to_wire(
    agent: &AgentDecl,
    model: &AgentStateModel,
    effects: &[HandlerEffects],
    text: &str,
) -> WireAgentStateModel {
    to_wire_with_types(
        agent,
        model,
        effects,
        text,
        &HashMap::new(),
        (Vec::new(), Vec::new()),
    )
}

/// `table` is the wire contract's type table for the agent's `store` types
/// ([`crate::wire_contract_request::store_type_table`]), which the writes' references point into;
/// `types` is the unit's combined type table.
pub fn to_wire_with_types(
    agent: &AgentDecl,
    model: &AgentStateModel,
    effects: &[HandlerEffects],
    text: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
    table: (Vec<WcType>, Vec<WcInst>),
) -> WireAgentStateModel {
    let range = |s: Span| crate::position::span_to_range(text, s);
    WireAgentStateModel {
        types: table.0,
        instantiations: table.1,
        invariants: wire_invariants(&model.invariants, text),
        effects: effects
            .iter()
            .map(|h| WireHandlerEffects {
                handler: h.handler.clone(),
                truncated: h.truncated,
                rows: h
                    .rows
                    .iter()
                    .map(|r| WireEffectRow {
                        guards: r.guards.clone(),
                        writes: r
                            .writes
                            .iter()
                            .map(|w| {
                                let (value_type, field_type, value_ref, field_ref) =
                                    stored_fields(w.stored.as_ref(), types);
                                WireEffectWrite {
                                    field: w.field.clone(),
                                    op: w.op.clone(),
                                    key: w.key.clone(),
                                    range: range(w.span),
                                    value_type,
                                    field_type,
                                    value_ref,
                                    field_ref,
                                }
                            })
                            .collect(),
                        reads: r.reads.clone(),
                        calls: r.calls.clone(),
                        outcomes: r.outcomes.clone(),
                    })
                    .collect(),
            })
            .collect(),
        agent: model.agent.clone(),
        range: range(agent.name.span),
        handlers: agent
            .handlers
            .iter()
            .map(|h| WireHandler {
                name: h
                    .method_name
                    .as_ref()
                    .map_or_else(|| "call".to_string(), |m| m.name.clone()),
                range: range(h.method_name.as_ref().map_or(h.span, |m| m.span)),
            })
            .collect(),
        machines: model
            .machines
            .iter()
            .map(|m| WireStateMachine {
                field: m.field.clone(),
                field_range: range(m.field_span),
                states: m.states.clone(),
                initial: m.initial.clone(),
                edges: m
                    .edges
                    .iter()
                    .map(|e| WireEdge {
                        handler: e.handler.clone(),
                        from: e.from.clone(),
                        guards: e.guards.clone(),
                        to: match &e.to {
                            Target::State(s) => WireTarget::State { state: s.clone() },
                            Target::Unknown => WireTarget::Unknown,
                            Target::Unchanged => WireTarget::Unchanged,
                        },
                        outcome: e.outcome.clone(),
                        range: range(e.span),
                        violations: e
                            .violations
                            .iter()
                            .map(|v| WireViolation {
                                transition: v.transition.clone(),
                                range: range(v.span),
                                certainty: match v.certainty {
                                    Certainty::Known => "Known",
                                    Certainty::Possible => "Possible",
                                },
                                from: v.from.clone(),
                                targets: v.targets.clone(),
                            })
                            .collect(),
                    })
                    .collect(),
                reachable: m.reachable.clone(),
                reachable_after_commit: m.reachable_after_commit.clone(),
                truncated: m.truncated.clone(),
                not_evaluated: m
                    .not_evaluated
                    .iter()
                    .map(|n| WireNamed {
                        name: n.transition.clone(),
                        range: range(n.span),
                    })
                    .collect(),
            })
            .collect(),
    }
}
