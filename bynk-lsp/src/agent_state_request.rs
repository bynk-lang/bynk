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

use std::collections::HashMap;
use std::sync::Arc;

use bynk_ide::agent_state::{self, AgentStateModel, Target};
use bynk_syntax::ast::{AgentDecl, CommonsItem, SourceUnit, TypeDecl};
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
    pub truncated: Vec<String>,
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
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum WireTarget {
    State { state: String },
    Unknown,
    Unchanged,
}

pub fn to_wire(agent: &AgentDecl, model: &AgentStateModel, text: &str) -> WireAgentStateModel {
    let range = |s: Span| crate::position::span_to_range(text, s);
    WireAgentStateModel {
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
                    })
                    .collect(),
                reachable: m.reachable.clone(),
                truncated: m.truncated.clone(),
            })
            .collect(),
    }
}
