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
    self, AltKind, Callee, HandlerOwner, MessageKind, ParticipantKind, SequenceModel, TriggerKind,
};
use bynk_syntax::ast::{CommonsItem, Handler, SourceUnit};
use tower_lsp::lsp_types::{Location, Url};

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

// Model-diagrams slice 3 adds `trigger`, `WireParticipant::key`,
// `WireMessage::callee` and `WireAltBlock::depth`, and the `"Break"` block
// kind. All additive: an older client ignores the fields, and draws a `Break`
// as it draws any block it does not know.

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireSequenceModel {
    pub participants: Vec<WireParticipant>,
    pub messages: Vec<WireMessage>,
    pub blocks: Vec<WireAltBlock>,
    /// What starts the handler — see [`bynk_ide::sequence::Trigger`].
    pub trigger: WireTrigger,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireTrigger {
    /// `Request`, `Call`, `Schedule`, `Event`, `Message`, `Connection` or
    /// `Rejected`.
    pub kind: &'static str,
    pub label: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireParticipant {
    pub id: u32,
    pub kind: &'static str,
    pub name: String,
    pub range: Option<tower_lsp::lsp_types::Range>,
    /// An agent's key expression as called (`client`), or `…` when calls
    /// disagree; `null` otherwise. See [`bynk_ide::sequence::Participant::key`].
    pub key: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireMessage {
    pub from: u32,
    pub to: u32,
    pub kind: &'static str,
    pub label: String,
    pub range: tower_lsp::lsp_types::Range,
    pub block: Option<u32>,
    /// The handler a boundary call reaches; `null` for every other message.
    pub callee: Option<WireCallee>,
}

/// The handler a boundary call reaches: by name, and where it is when the
/// server can find it in the committed round (a client opens the callee's own
/// sequence diagram there).
#[derive(Debug, Clone, serde::Serialize)]
pub struct WireCallee {
    /// The consumed context; `null` for an agent of the caller's own context.
    pub unit: Option<String>,
    pub owner: String,
    /// The handler's method name; `null` for a service's unnamed `on call`.
    pub method: Option<String>,
    /// The handler's declaration; `null` when not found (a consumed context
    /// from a package, say, whose source is not in this project).
    pub location: Option<Location>,
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
    /// The block's nesting depth, 0 at the top level (ADR 0858).
    pub depth: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireBranch {
    pub label: String,
    #[serde(rename = "messageIds")]
    pub message_ids: Vec<usize>,
    /// The branch's rendered outcome (`Ok(view)`) — see
    /// [`bynk_ide::sequence::Branch::reply`]. `null` on the wire when absent.
    pub reply: Option<String>,
    /// The source range of the branch's arm — see
    /// [`bynk_ide::sequence::Branch::span`]. For the `else` of an `else if`,
    /// the range starts at the inner `if` keyword and covers the rest of the
    /// chain (#3599); the inner `if` is a nested block with its own ranges.
    pub range: tower_lsp::lsp_types::Range,
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
        AltKind::Break => "Break",
    }
}

fn trigger_kind_str(k: TriggerKind) -> &'static str {
    match k {
        TriggerKind::Request => "Request",
        TriggerKind::Call => "Call",
        TriggerKind::Schedule => "Schedule",
        TriggerKind::Event => "Event",
        TriggerKind::Message => "Message",
        TriggerKind::Connection => "Connection",
        TriggerKind::Rejected => "Rejected",
    }
}

/// Find the handler `callee` names among `files` (the callee unit's files:
/// URI and analysed text), and return its declaration. A service's handler is
/// its `on call` (named `method`, or unnamed when `method` is `None`); an
/// agent's is its `on call method`.
pub fn locate_callee<'a>(
    callee: &Callee,
    files: impl IntoIterator<Item = (Url, &'a str)>,
) -> Option<Location> {
    for (uri, text) in files {
        let Ok(tokens) = bynk_syntax::lexer::tokenize(text) else {
            continue;
        };
        let (unit, _errs) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, text);
        let items: &[CommonsItem] = match unit.as_ref() {
            Some(SourceUnit::Context(c)) => &c.items,
            Some(SourceUnit::Adapter(a)) => &a.items,
            _ => continue,
        };
        for item in items {
            let (name, handlers) = match item {
                CommonsItem::Service(s) => (&s.name.name, &s.handlers),
                CommonsItem::Agent(a) => (&a.name.name, &a.handlers),
                _ => continue,
            };
            if *name != callee.owner {
                continue;
            }
            let found = handlers.iter().find(|h| {
                matches!(h.kind, bynk_syntax::ast::HandlerKind::Call)
                    && h.method_name.as_ref().map(|m| m.name.as_str()) == callee.method.as_deref()
            });
            if let Some(h) = found {
                return Some(Location {
                    uri,
                    range: crate::position::span_to_range(text, h.span),
                });
            }
        }
    }
    None
}

/// The wire shape of `model`, with each callee located by `locate` (the
/// server passes a lookup over the committed round's files).
pub fn to_wire(
    model: &SequenceModel,
    text: &str,
    locate: impl Fn(&Callee) -> Option<Location>,
) -> WireSequenceModel {
    WireSequenceModel {
        trigger: WireTrigger {
            kind: trigger_kind_str(model.trigger.kind),
            label: model.trigger.label.clone(),
        },
        participants: model
            .participants
            .iter()
            .map(|p| WireParticipant {
                id: p.id,
                kind: participant_kind_str(p.kind),
                name: p.name.clone(),
                range: p.span.map(|s| crate::position::span_to_range(text, s)),
                key: p.key.clone(),
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
                callee: m.callee.as_ref().map(|c| WireCallee {
                    unit: c.unit.clone(),
                    owner: c.owner.clone(),
                    method: c.method.clone(),
                    location: locate(c),
                }),
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
                        range: crate::position::span_to_range(text, br.span),
                    })
                    .collect(),
                range: crate::position::span_to_range(text, b.span),
                parent: b.parent,
                parent_branch: b.parent_branch,
                depth: b.depth,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_ide::sequence::{AltBlock, Branch};
    use bynk_syntax::span::Span;

    /// #3584: a branch's span is lowered to a `range` on the wire.
    #[test]
    fn wire_branch_carries_a_range() {
        let text = "ab\ncd\n";
        let model = SequenceModel {
            blocks: vec![AltBlock {
                id: 0,
                kind: AltKind::If,
                branches: vec![Branch {
                    label: "x".into(),
                    message_ids: vec![],
                    reply: None,
                    span: Span::from(3..5),
                }],
                span: Span::from(0..5),
                parent: None,
                parent_branch: None,
                depth: 0,
            }],
            ..SequenceModel::default()
        };
        let json = serde_json::to_value(to_wire(&model, text, |_| None)).unwrap();
        let range = &json["blocks"][0]["branches"][0]["range"];
        assert_eq!(range["start"]["line"], 1);
        assert_eq!(range["start"]["character"], 0);
        assert_eq!(range["end"]["character"], 2);
    }

    const PLATFORM: &str = "context platform\n\nservice Pinger {\n  on call(n: Int) -> Effect[Int] by Caller {\n    n\n  }\n}\n";
    const LIMITS: &str = "context ratelimit\n\nagent Limiter {\n  key client: String\n\n  store count: Cell[Int]\n\n  on call reset() -> Effect[()] {\n    count := 0\n  }\n\n  on call hit(now: Int) -> Effect[Int] {\n    now\n  }\n}\n";

    fn uri(name: &str) -> Url {
        Url::parse(&format!("file:///p/{name}")).unwrap()
    }

    /// Model-diagrams slice 3: a callee is found by owner and method among
    /// its unit's files.
    #[test]
    fn locates_a_service_call_and_a_named_agent_handler() {
        let pinger = Callee {
            unit: Some("platform".into()),
            owner: "Pinger".into(),
            method: None,
        };
        let found = locate_callee(
            &pinger,
            [
                (uri("other.bynk"), LIMITS),
                (uri("platform.bynk"), PLATFORM),
            ],
        )
        .expect("the service's on call");
        assert_eq!(found.uri, uri("platform.bynk"));
        assert_eq!(found.range.start.line, 3, "the `on call` line");

        let hit = Callee {
            unit: None,
            owner: "Limiter".into(),
            method: Some("hit".into()),
        };
        let found = locate_callee(&hit, [(uri("limits.bynk"), LIMITS)]).expect("on call hit");
        assert_eq!(found.range.start.line, 11, "not `reset`, the first handler");
    }

    #[test]
    fn a_callee_that_is_not_there_has_no_location() {
        let missing = Callee {
            unit: None,
            owner: "Limiter".into(),
            method: Some("nope".into()),
        };
        assert_eq!(
            locate_callee(&missing, [(uri("limits.bynk"), LIMITS)]),
            None
        );
        let other = Callee {
            unit: None,
            owner: "Nobody".into(),
            method: None,
        };
        assert_eq!(
            locate_callee(
                &other,
                [
                    (uri("limits.bynk"), LIMITS),
                    (uri("bad.bynk"), "context {{{")
                ]
            ),
            None
        );
    }

    /// Model-diagrams slice 3: the additive wire fields.
    #[test]
    fn wire_carries_trigger_depth_key_and_located_callee() {
        let text = "x\n";
        let callee = Callee {
            unit: None,
            owner: "Limiter".into(),
            method: Some("hit".into()),
        };
        let model = SequenceModel {
            participants: vec![bynk_ide::sequence::Participant {
                id: 1,
                kind: ParticipantKind::Agent,
                name: "Limiter".into(),
                span: None,
                key: Some("client".into()),
            }],
            messages: vec![bynk_ide::sequence::Message {
                from: 0,
                to: 1,
                kind: MessageKind::Call,
                label: "hit(now)".into(),
                span: Span::from(0..1),
                block: Some(0),
                callee: Some(callee),
            }],
            blocks: vec![AltBlock {
                id: 0,
                kind: AltKind::Break,
                branches: vec![],
                span: Span::from(0..1),
                parent: None,
                parent_branch: None,
                depth: 3,
            }],
            trigger: bynk_ide::sequence::Trigger {
                kind: TriggerKind::Schedule,
                label: "cron \"0 * * * *\"".into(),
            },
        };
        let located = |_: &Callee| {
            Some(Location {
                uri: uri("limits.bynk"),
                range: Default::default(),
            })
        };
        let json = serde_json::to_value(to_wire(&model, text, located)).unwrap();
        assert_eq!(json["trigger"]["kind"], "Schedule");
        assert_eq!(json["trigger"]["label"], "cron \"0 * * * *\"");
        assert_eq!(json["participants"][0]["key"], "client");
        assert_eq!(json["blocks"][0]["kind"], "Break");
        assert_eq!(json["blocks"][0]["depth"], 3);
        let c = &json["messages"][0]["callee"];
        assert_eq!(c["owner"], "Limiter");
        assert_eq!(c["method"], "hit");
        assert!(c["unit"].is_null());
        assert_eq!(c["location"]["uri"], "file:///p/limits.bynk");
    }
}
