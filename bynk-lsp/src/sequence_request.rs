//! #846: `bynk/sequenceModel` — the sequence-diagram custom LSP request.
//!
//! The first custom (non-standard) request in this server: no `workspace/*/
//! refresh` nudge exists for it (there is no generic "refresh a custom
//! method" in the LSP spec or in `tower_lsp::Client`), and none is needed —
//! Tier 1 is on-demand: the client re-issues the request each time the
//! command/lens fires, rather than the server pushing updates.
//!
//! Two responsibilities live here, out of `lib.rs`: locating the `Handler`
//! AST node enclosing a cursor position (by re-parsing the committed
//! snapshot, the same convention `identifier_at` uses in `lib.rs`), and the
//! wire shape sent to the client (a plain serde mirror of
//! [`bynk_ide::sequence::SequenceModel`], `Span` lowered to LSP `Range` —
//! same convention as `SerKey` in `lib.rs`).

use bynk_ide::sequence::{
    self, AltKind, HandlerOwner, MessageKind, ParticipantKind, SequenceModel,
};
use bynk_syntax::ast::{CommonsItem, Handler, SourceUnit};

/// Locate the `Handler` enclosing `offset` in `text` and build its sequence
/// model. `info` is the owning unit's cross-context/agent table — `None`
/// degrades classification to capabilities only (still correct; just unable
/// to recognise agent/cross-context lifelines), which happens for a unit
/// `sequence_info` has no entry for (a commons file, or one this round never
/// reached because the pipeline bailed before it).
pub fn sequence_model_at(
    text: &str,
    offset: usize,
    info: Option<&bynk_ide::ContextSequenceInfo>,
) -> Option<SequenceModel> {
    let tokens = bynk_syntax::lexer::tokenize(text).ok()?;
    let (unit, _errs) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, text);
    let items: &[CommonsItem] = match unit.as_ref()? {
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Commons(_) | SourceUnit::Suite(_) => return None,
    };
    for item in items {
        match item {
            CommonsItem::Service(s) => {
                if let Some(h) = handler_at(&s.handlers, offset) {
                    return Some(sequence::sequence_model(
                        h,
                        HandlerOwner::Service(&s.name.name),
                        // v0.155: a handler with no `given`/`by` of its own
                        // inherits the service-level default. See `sequence_model`.
                        &s.default_given,
                        s.default_by.as_ref(),
                        info,
                    ));
                }
            }
            CommonsItem::Agent(a) => {
                if let Some(h) = handler_at(&a.handlers, offset) {
                    return Some(sequence::sequence_model(
                        h,
                        HandlerOwner::Agent(&a.name.name),
                        // Agents have no service-level `given` default and no
                        // principal (`by`).
                        &[],
                        None,
                        info,
                    ));
                }
            }
            _ => {}
        }
    }
    None
}

fn handler_at(handlers: &[Handler], offset: usize) -> Option<&Handler> {
    handlers
        .iter()
        .find(|h| h.span.start <= offset && offset < h.span.end)
}

/// Every `on <kind>` handler declaration in `text`, for the per-handler
/// "Show Sequence" CodeLens. **Not** `index_queries::code_lenses` — that
/// walks `SymbolKind::Handler` sites, which only agent handlers get
/// (`bynk-check/src/index.rs`: "Service handlers have no per-handler name...
/// so only agent dispatch is covered"); reusing it as-is would silently drop
/// the lens for every service (non-agent) handler, which is most of them.
/// A direct AST walk covers both uniformly.
pub fn handler_lens_sites(text: &str) -> Vec<bynk_syntax::span::Span> {
    let Ok(tokens) = bynk_syntax::lexer::tokenize(text) else {
        return Vec::new();
    };
    let (unit, _errs) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, text);
    let Some(unit) = unit else {
        return Vec::new();
    };
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Commons(_) | SourceUnit::Suite(_) => return Vec::new(),
    };
    let mut sites = Vec::new();
    for item in items {
        match item {
            CommonsItem::Service(s) => sites.extend(s.handlers.iter().map(|h| h.span)),
            CommonsItem::Agent(a) => sites.extend(a.handlers.iter().map(|h| h.span)),
            _ => {}
        }
    }
    sites
}

/// The `bynk/sequenceModel` request payload — the same two-field
/// text-document + cursor-position shape every other cursor-anchored request
/// in this server uses (`HoverParams`, `SignatureHelpParams`, …).
///
/// #847: `rename_all = "camelCase"` added — the client sends the LSP wire names
/// `textDocument`/`position`, so the params must deserialize from camelCase.
/// Without it the request failed with a missing-field error the moment a real
/// client called it (the #846 in-crate tests drive `sequence_model_at` directly
/// and never deserialize these params, and the live VS Code path was not run —
/// so the mismatch shipped latent).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SequenceModelParams {
    pub text_document: tower_lsp::lsp_types::TextDocumentIdentifier,
    pub position: tower_lsp::lsp_types::Position,
}

// -- Wire shape: a plain serde mirror of `bynk_ide::sequence::SequenceModel`,
// -- `Span` lowered to `Range` (converted against the committed snapshot text
// -- the caller already has — this module stays position-map-agnostic).

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireSequenceModel {
    pub participants: Vec<WireParticipant>,
    pub messages: Vec<WireMessage>,
    pub blocks: Vec<WireAltBlock>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireParticipant {
    pub id: u32,
    pub kind: &'static str,
    pub name: String,
    pub range: Option<tower_lsp::lsp_types::Range>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireMessage {
    pub from: u32,
    pub to: u32,
    pub kind: &'static str,
    pub label: String,
    pub range: tower_lsp::lsp_types::Range,
    pub block: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireAltBlock {
    pub id: u32,
    pub kind: &'static str,
    pub branches: Vec<WireBranch>,
    pub range: tower_lsp::lsp_types::Range,
    pub parent: Option<u32>,
    #[serde(rename = "parentBranch")]
    pub parent_branch: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireBranch {
    pub label: String,
    #[serde(rename = "messageIds")]
    pub message_ids: Vec<usize>,
    /// The branch's rendered outcome (`Ok(view)`) — see
    /// [`bynk_ide::sequence::Branch::reply`]. `null` on the wire when absent.
    pub reply: Option<String>,
}

fn participant_kind_str(k: ParticipantKind) -> &'static str {
    match k {
        ParticipantKind::Entry => "Entry",
        ParticipantKind::Capability => "Capability",
        ParticipantKind::Context => "Context",
        ParticipantKind::Agent => "Agent",
        ParticipantKind::Actor => "Actor",
    }
}

fn message_kind_str(k: MessageKind) -> &'static str {
    match k {
        MessageKind::Call => "Call",
        MessageKind::Return => "Return",
        MessageKind::Send => "Send",
        MessageKind::StateWrite => "StateWrite",
    }
}

fn alt_kind_str(k: AltKind) -> &'static str {
    match k {
        AltKind::If => "If",
        AltKind::Match => "Match",
        AltKind::Collapsed => "Collapsed",
    }
}

pub fn to_wire(model: &SequenceModel, text: &str) -> WireSequenceModel {
    WireSequenceModel {
        participants: model
            .participants
            .iter()
            .map(|p| WireParticipant {
                id: p.id,
                kind: participant_kind_str(p.kind),
                name: p.name.clone(),
                range: p.span.map(|s| crate::position::span_to_range(text, s)),
            })
            .collect(),
        messages: model
            .messages
            .iter()
            .map(|m| WireMessage {
                from: m.from,
                to: m.to,
                kind: message_kind_str(m.kind),
                label: m.label.clone(),
                range: crate::position::span_to_range(text, m.span),
                block: m.block,
            })
            .collect(),
        blocks: model
            .blocks
            .iter()
            .map(|b| WireAltBlock {
                id: b.id,
                kind: alt_kind_str(b.kind),
                branches: b
                    .branches
                    .iter()
                    .map(|br| WireBranch {
                        label: br.label.clone(),
                        message_ids: br.message_ids.clone(),
                        reply: br.reply.clone(),
                    })
                    .collect(),
                range: crate::position::span_to_range(text, b.span),
                parent: b.parent,
                parent_branch: b.parent_branch,
            })
            .collect(),
    }
}
