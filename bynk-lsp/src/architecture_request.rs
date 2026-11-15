//! #851: `bynk/architectureModel` — the architecture-map custom LSP request.
//!
//! The third custom request in this server (after #846's `bynk/sequenceModel`
//! and #847's `bynk/documentationModel`), and the same on-demand posture: no
//! `workspace/*/refresh` nudge exists for a custom method and none is needed
//! — the client re-issues the request each time "Bynk: Show Architecture Map"
//! fires.
//!
//! Unlike both siblings, this request is **project-scoped**, not file-scoped:
//! the params carry a `textDocument` only to resolve which project's
//! committed round to read (the same `committed_analysis` gate every pull-
//! based request uses), never to restrict the result to that one file. The
//! wire model reflects that: every node/member/edge carries its own `uri` —
//! not just a `range` against the request's document, the convention
//! `sequence`/`documentation` can get away with because they're single-file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_ide::architecture::{self, ArchMember, ArchModel, CapabilityOrigin, NodeKind};
use bynk_ide::{DeliveryKind, MemberKind};

/// The `bynk/architectureModel` request payload — a bare text-document
/// identifier used only to resolve the owning project (Decision B: the
/// active file's nearest `bynk.toml`), never to scope the result to that file.
///
/// `rename_all = "camelCase"` is load-bearing — see `sequence_request`'s and
/// `documentation_request`'s own params docs for why (the client sends
/// `textDocument`; this shipped as a missing-field bug once already, in #846,
/// before #847 caught and fixed it).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchitectureModelParams {
    pub text_document: tower_lsp::lsp_types::TextDocumentIdentifier,
}

/// Build the project-wide architecture model from the round's own retained
/// tables. A thin wrapper around [`architecture::architecture_model`] so the
/// `lib.rs` handler stays a `committed_analysis` guard chain, like its two
/// siblings.
pub fn architecture_model_for(
    unit_sources: &HashMap<String, Vec<PathBuf>>,
    snapshots: &HashMap<PathBuf, String>,
    sequence_info: &HashMap<String, bynk_ide::ContextSequenceInfo>,
    facts: &bynk_ide::ArchitectureFacts,
) -> ArchModel {
    architecture::architecture_model(unit_sources, snapshots, sequence_info, facts)
}

// -- Wire shape: a plain serde mirror of `bynk_ide::architecture::ArchModel`.
// -- Each `Located` lowers to a `{uri, range}` pair computed against *its
// -- own* file's committed snapshot — never the request document's, since a
// -- node's members may span several files of a multi-file unit, and two
// -- nodes almost always live in different files entirely.

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireLoc {
    pub uri: tower_lsp::lsp_types::Url,
    pub range: tower_lsp::lsp_types::Range,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchModel {
    pub nodes: Vec<WireArchNode>,
    pub edges: Vec<WireArchEdge>,
    /// #3583: every declared event with its emitters and subscribers.
    pub events: Vec<WireArchEvent>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchNode {
    pub name: String,
    pub kind: &'static str,
    pub loc: WireLoc,
    pub capabilities: Vec<WireArchCapability>,
    pub providers: Vec<WireArchProvider>,
    pub services: Vec<WireArchService>,
    pub agents: Vec<WireArchAgent>,
    /// #3583: per handler, the capabilities it requires through `given`.
    pub uses: Vec<WireArchHandlerUses>,
}

/// #3583: the handler (or provider operation) an event site or a use belongs to.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchMember {
    pub unit: String,
    /// `"service"`, `"agent"` or `"provider"`.
    pub kind: &'static str,
    pub container: String,
    pub handler: String,
    pub loc: WireLoc,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchHandlerUses {
    pub member: WireArchMember,
    pub uses: Vec<WireArchUse>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchUse {
    pub capability: String,
    /// The unit that declares the capability: this unit, a consumed one, or
    /// `"bynk"`.
    pub from: Option<String>,
    /// Written on the service header (`default_given`), not on the handler.
    pub inherited: bool,
    pub loc: WireLoc,
    #[serde(rename = "stubbedIn")]
    pub stubbed_in: Vec<WireArchStub>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchStub {
    /// The suite file, project-relative with `/` separators.
    pub suite: String,
    /// The suite's first `stub` of the capability.
    pub loc: WireLoc,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchEvent {
    pub owner: String,
    pub name: String,
    pub loc: WireLoc,
    pub emitters: Vec<WireArchEmitter>,
    pub subscribers: Vec<WireArchSubscriber>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchEmitter {
    pub member: WireArchMember,
    /// The `Events.emit[E](…)` call.
    pub loc: WireLoc,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchSubscriber {
    pub member: WireArchMember,
    /// `"all"`, `"pattern"`, `"schema"` or `"patternSchema"`.
    pub delivery: &'static str,
    /// The `from Events(…)` header.
    pub loc: WireLoc,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchCapability {
    pub name: String,
    pub local: bool,
    /// The providing unit's qualified name, for a consumed (non-local)
    /// capability — `"bynk"` for the toolchain's built-in surface. `null` for
    /// a locally-declared capability.
    pub from: Option<String>,
    pub loc: WireLoc,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchProvider {
    pub capability: String,
    #[serde(rename = "providerName")]
    pub provider_name: String,
    pub external: bool,
    pub loc: WireLoc,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchService {
    pub name: String,
    #[serde(rename = "handlerCount")]
    pub handler_count: usize,
    pub loc: WireLoc,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchAgent {
    pub name: String,
    #[serde(rename = "handlerCount")]
    pub handler_count: usize,
    pub loc: WireLoc,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireArchEdge {
    pub from: String,
    pub to: String,
    /// Selected capability labels (braced `consumes` form); empty for a
    /// whole-unit consumes.
    pub capabilities: Vec<String>,
    pub loc: WireLoc,
}

fn kind_str(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Context => "Context",
        NodeKind::Adapter => "Adapter",
    }
}

fn member_kind_str(k: MemberKind) -> &'static str {
    match k {
        MemberKind::Service => "service",
        MemberKind::Agent => "agent",
        MemberKind::Provider => "provider",
    }
}

fn delivery_str(d: DeliveryKind) -> &'static str {
    match d {
        DeliveryKind::All => "all",
        DeliveryKind::Pattern => "pattern",
        DeliveryKind::Schema => "schema",
        DeliveryKind::PatternSchema => "patternSchema",
    }
}

fn wire_member(
    project_root: &Path,
    snapshots: &HashMap<PathBuf, String>,
    m: &ArchMember,
) -> Option<WireArchMember> {
    Some(WireArchMember {
        unit: m.unit.clone(),
        kind: member_kind_str(m.kind),
        container: m.container.clone(),
        handler: m.handler.clone(),
        loc: wire_loc(project_root, snapshots, &m.loc)?,
    })
}

/// Lower a `Located` to a `{uri, range}` pair. `None` only on the defensive
/// paths — a missing snapshot for `loc.file` (every `Located` here was built
/// from a file `architecture_model` itself read out of `snapshots`, so this
/// should not happen) or a path that cannot form a `file://` URI.
fn wire_loc(
    project_root: &Path,
    snapshots: &HashMap<PathBuf, String>,
    loc: &architecture::Located,
) -> Option<WireLoc> {
    let text = snapshots.get(&loc.file)?;
    let uri = tower_lsp::lsp_types::Url::from_file_path(project_root.join(&loc.file)).ok()?;
    Some(WireLoc {
        uri,
        range: crate::position::span_to_range(text, loc.span),
    })
}

/// Lower the whole model. A node (or member, or edge) whose own location
/// can't be resolved (see `wire_loc` above) is dropped rather than sent with
/// a bogus location — click-to-code must never open the wrong file.
pub fn to_wire(
    model: &ArchModel,
    project_root: &Path,
    snapshots: &HashMap<PathBuf, String>,
) -> WireArchModel {
    WireArchModel {
        nodes: model
            .nodes
            .iter()
            .filter_map(|n| {
                Some(WireArchNode {
                    name: n.name.clone(),
                    kind: kind_str(n.kind),
                    loc: wire_loc(project_root, snapshots, &n.loc)?,
                    capabilities: n
                        .capabilities
                        .iter()
                        .filter_map(|c| {
                            let (local, from) = match &c.origin {
                                CapabilityOrigin::Local => (true, None),
                                CapabilityOrigin::Consumed { from } => (false, Some(from.clone())),
                            };
                            Some(WireArchCapability {
                                name: c.name.clone(),
                                local,
                                from,
                                loc: wire_loc(project_root, snapshots, &c.loc)?,
                            })
                        })
                        .collect(),
                    providers: n
                        .providers
                        .iter()
                        .filter_map(|p| {
                            Some(WireArchProvider {
                                capability: p.capability.clone(),
                                provider_name: p.provider_name.clone(),
                                external: p.external,
                                loc: wire_loc(project_root, snapshots, &p.loc)?,
                            })
                        })
                        .collect(),
                    services: n
                        .services
                        .iter()
                        .filter_map(|s| {
                            Some(WireArchService {
                                name: s.name.clone(),
                                handler_count: s.handler_count,
                                loc: wire_loc(project_root, snapshots, &s.loc)?,
                            })
                        })
                        .collect(),
                    agents: n
                        .agents
                        .iter()
                        .filter_map(|a| {
                            Some(WireArchAgent {
                                name: a.name.clone(),
                                handler_count: a.handler_count,
                                loc: wire_loc(project_root, snapshots, &a.loc)?,
                            })
                        })
                        .collect(),
                    uses: wire_uses(&n.uses, project_root, snapshots),
                })
            })
            .collect(),
        events: wire_events(&model.events, project_root, snapshots),
        edges: model
            .edges
            .iter()
            .filter_map(|e| {
                Some(WireArchEdge {
                    from: e.from.clone(),
                    to: e.to.clone(),
                    capabilities: e.capabilities.clone(),
                    loc: wire_loc(project_root, snapshots, &e.loc)?,
                })
            })
            .collect(),
    }
}

fn wire_uses(
    handlers: &[architecture::ArchHandlerUses],
    project_root: &Path,
    snapshots: &HashMap<PathBuf, String>,
) -> Vec<WireArchHandlerUses> {
    handlers
        .iter()
        .filter_map(|h| {
            Some(WireArchHandlerUses {
                member: wire_member(project_root, snapshots, &h.member)?,
                uses: h
                    .uses
                    .iter()
                    .filter_map(|u| {
                        Some(WireArchUse {
                            capability: u.capability.clone(),
                            from: u.from.clone(),
                            inherited: u.inherited,
                            loc: wire_loc(project_root, snapshots, &u.loc)?,
                            stubbed_in: u
                                .stubbed_in
                                .iter()
                                .filter_map(|s| {
                                    Some(WireArchStub {
                                        suite: s.suite.to_string_lossy().replace('\\', "/"),
                                        loc: wire_loc(project_root, snapshots, &s.loc)?,
                                    })
                                })
                                .collect(),
                        })
                    })
                    .collect(),
            })
        })
        .collect()
}

fn wire_events(
    events: &[architecture::ArchEvent],
    project_root: &Path,
    snapshots: &HashMap<PathBuf, String>,
) -> Vec<WireArchEvent> {
    events
        .iter()
        .filter_map(|e| {
            Some(WireArchEvent {
                owner: e.owner.clone(),
                name: e.name.clone(),
                loc: wire_loc(project_root, snapshots, &e.loc)?,
                emitters: e
                    .emitters
                    .iter()
                    .filter_map(|x| {
                        Some(WireArchEmitter {
                            member: wire_member(project_root, snapshots, &x.member)?,
                            loc: wire_loc(project_root, snapshots, &x.loc)?,
                        })
                    })
                    .collect(),
                subscribers: e
                    .subscribers
                    .iter()
                    .filter_map(|x| {
                        Some(WireArchSubscriber {
                            member: wire_member(project_root, snapshots, &x.member)?,
                            delivery: delivery_str(x.delivery),
                            loc: wire_loc(project_root, snapshots, &x.loc)?,
                        })
                    })
                    .collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bynk-lsp-architecture-request-test-{test_name}-{}",
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

    const PROVIDER_SRC: &str = r#"context platformtime

exports capability { Clock }

capability Clock {
  fn now() -> Effect[Int]
}

provides Clock = SystemClock {
  fn now() -> Effect[Int] {
    0
  }
}
"#;
    const CONSUMER_SRC: &str = r#"context ops.jobs

consumes platformtime { Clock }

service run {
  on call() -> Effect[Int] given Clock {
    let now <- Clock.now()
    now
  }
}
"#;

    #[test]
    fn params_deserialize_from_camel_case_wire_json() {
        let json = serde_json::json!({
            "textDocument": { "uri": "file:///a/b.bynk" }
        });
        let params: ArchitectureModelParams =
            serde_json::from_value(json).expect("camelCase textDocument must deserialize");
        assert_eq!(params.text_document.uri.as_str(), "file:///a/b.bynk");
    }

    /// Regression (the exact bug the advisor flagged): a node's own file
    /// differs from the *other* node's file, and each member's `uri` must
    /// point at the file it was actually declared in — not the request
    /// document, not the first file the model happened to visit.
    #[test]
    fn each_node_uri_points_at_its_own_declaring_file_not_the_others() {
        let root = setup_project(
            "twofile",
            &[
                ("platformtime.bynk", PROVIDER_SRC),
                ("jobs.bynk", CONSUMER_SRC),
            ],
        );
        let diag = bynk_ide::diagnose_project(
            &root,
            &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
        );
        let snapshots: HashMap<PathBuf, String> = diag
            .files
            .iter()
            .map(|f| (f.source_path.clone(), f.text.clone()))
            .collect();
        let model = architecture_model_for(
            &diag.unit_sources,
            &snapshots,
            &diag.sequence_info,
            &diag.architecture_facts,
        );
        let wire = to_wire(&model, &root, &snapshots);

        assert_eq!(wire.nodes.len(), 2);
        let jobs = wire.nodes.iter().find(|n| n.name == "ops.jobs").unwrap();
        let platformtime = wire
            .nodes
            .iter()
            .find(|n| n.name == "platformtime")
            .unwrap();

        assert!(
            jobs.loc.uri.as_str().ends_with("jobs.bynk"),
            "got {}",
            jobs.loc.uri
        );
        assert!(
            platformtime.loc.uri.as_str().ends_with("platformtime.bynk"),
            "got {}",
            platformtime.loc.uri
        );
        assert_ne!(jobs.loc.uri, platformtime.loc.uri);

        // The consumed capability entry lives in the *consuming* file
        // (`jobs.bynk`'s `consumes` clause), even though the capability
        // itself is declared over in `platformtime.bynk`.
        assert_eq!(jobs.capabilities.len(), 1);
        assert!(jobs.capabilities[0].loc.uri.as_str().ends_with("jobs.bynk"));
        assert_eq!(jobs.capabilities[0].from.as_deref(), Some("platformtime"));

        assert_eq!(wire.edges.len(), 1);
        assert_eq!(wire.edges[0].from, "ops.jobs");
        assert_eq!(wire.edges[0].to, "platformtime");
        assert!(wire.edges[0].loc.uri.as_str().ends_with("jobs.bynk"));
    }
}
