//! #851: the architecture-model query — a whole-project map of contexts and
//! adapters, their `consumes` edges, and the capabilities/providers/services/
//! agents each one binds, for the "Show Architecture Map" VS Code webview.
//! The macro counterpart to #846's per-handler sequence diagram: where that
//! query is file-scoped and cursor-anchored, this one is **project-scoped** —
//! every span here is paired with the project-relative file it belongs to
//! (a unit's declarations may span several files, per `unit_sources`), so a
//! wire layer can open the right file on click-to-code rather than assuming
//! the request's own document.
//!
//! Citation correction (found during implementation of #851; recorded in
//! `design/pending/architecture-map-851.md`): the issue proposed this as "a
//! pure, read-only query over the binding index and call graph"
//! (`bynk-check/src/index.rs`). That index has no `Context`/`Adapter` symbol
//! kind and its call graph does not carry `consumes` edges — its own comment
//! notes cross-context dispatch is "the one uncovered relation". The actual
//! source, used here exactly as `sequence::Builder::classify_cross_context`
//! already does, is [`ContextSequenceInfo::cross_context`] for resolved
//! consumed-context names (aliasing applied) plus a re-parse of each unit's
//! own snapshot text for its local declarations and raw `consumes` clauses —
//! there is no retained AST after a round, the same constraint
//! `sequence_request`/`documentation_request` re-parse against.
//!
//! Capability binding (Decision C: show binding, defer residency): a braced
//! `consumes U { Cap, … }` selection flattens `Cap` into the consumer's own
//! namespace, so each selected capability is recorded directly on the
//! consuming node (not only as an edge label) — this is what makes the
//! built-in `consumes bynk { Clock }` binding visible on a single-context
//! project even though the synthetic `bynk` unit has no project file and so
//! never becomes a node itself (an edge to it would dangle); see
//! [`architecture_model`]'s node-name filter. A whole-unit consumes (no
//! braces) grants qualified access to every exported capability but flattens
//! none of them, so it contributes an edge only, unlabelled — matching the
//! issue's "labelled with selected capabilities where braced".

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use bynk_check::analysis::ContextSequenceInfo;
use bynk_syntax::ast::*;
use bynk_syntax::span::Span;

/// A declaration site: which project-relative file it lives in, and its span
/// within that file's text. Project-scoped models (unlike the file-scoped
/// `sequence`/`documentation` ones) always need both — a node's members can
/// come from different files of the same multi-file unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub file: PathBuf,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Context,
    Adapter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityOrigin {
    /// A `capability { … }` declared directly in this unit.
    Local,
    /// Flattened in via a braced `consumes <from> { <name>, … }` selection.
    Consumed { from: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchCapability {
    pub name: String,
    pub origin: CapabilityOrigin,
    pub loc: Located,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchProvider {
    pub capability: String,
    pub provider_name: String,
    /// `provides Cap = Name` with no brace block — supplied by an adapter's
    /// external binding rather than a Bynk body (`ProviderDecl::external`).
    pub external: bool,
    pub loc: Located,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchService {
    pub name: String,
    pub handler_count: usize,
    pub loc: Located,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchAgent {
    pub name: String,
    pub handler_count: usize,
    pub loc: Located,
}

/// One context/adapter node — the "services in, agents below" grouping the
/// design notes describe, rendered as a single box with click-to-expand
/// members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchNode {
    /// The unit's qualified name (`demo.app`, `tokens`).
    pub name: String,
    pub kind: NodeKind,
    /// The unit-name declaration site — the node box's own click target.
    pub loc: Located,
    pub capabilities: Vec<ArchCapability>,
    pub providers: Vec<ArchProvider>,
    pub services: Vec<ArchService>,
    pub agents: Vec<ArchAgent>,
}

/// A directed `consumes` edge between two nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchEdge {
    pub from: String,
    pub to: String,
    /// Selected capability labels (braced form, in source order); empty for
    /// a whole-unit consumes.
    pub capabilities: Vec<String>,
    /// The `consumes` clause's own span, in the consuming (`from`) unit's file.
    pub loc: Located,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchModel {
    pub nodes: Vec<ArchNode>,
    pub edges: Vec<ArchEdge>,
}

/// Build the whole-project architecture model.
///
/// `unit_sources` and `snapshots` are `Analysis`/`ProjectDiagnostics`'s own
/// fields (project-relative file lists per unit, and each file's committed
/// text); `sequence_info` is the same table `bynk/sequenceModel` classifies
/// against. A unit with no `sequence_info` entry (a `commons`, or the
/// toolchain's synthetic `bynk` capability surface — excluded from
/// `unit_sources`, per its own doc) contributes no node; a `consumes` edge
/// that resolves to such a unit is dropped (nothing to draw an arrow to) but
/// its selected capabilities are still recorded on the consuming node — see
/// the module doc.
pub fn architecture_model(
    unit_sources: &HashMap<String, Vec<PathBuf>>,
    snapshots: &HashMap<PathBuf, String>,
    sequence_info: &HashMap<String, ContextSequenceInfo>,
) -> ArchModel {
    // Deterministic iteration order (ADR risk: "layout churn" — a `HashMap`
    // walk shuffles between rounds otherwise).
    let mut names: Vec<&String> = sequence_info
        .keys()
        .filter(|n| unit_sources.contains_key(n.as_str()))
        .collect();
    names.sort();
    let node_names: HashSet<&str> = names.iter().map(|s| s.as_str()).collect();

    let mut nodes = Vec::with_capacity(names.len());
    let mut edges = Vec::new();

    for name in names {
        let mut files = unit_sources[name].clone();
        files.sort();

        let mut kind = None;
        let mut loc = None;
        let mut capabilities = Vec::new();
        let mut providers = Vec::new();
        let mut services = Vec::new();
        let mut agents = Vec::new();

        let info = sequence_info.get(name);

        for file in &files {
            let Some(text) = snapshots.get(file) else {
                continue;
            };
            let Ok(tokens) = bynk_syntax::lexer::tokenize(text) else {
                continue;
            };
            let (unit_opt, _errs) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, text);
            let Some(unit) = unit_opt else { continue };
            let (this_kind, name_span, items, consumes): (
                NodeKind,
                Span,
                &[CommonsItem],
                &[ConsumesDecl],
            ) = match &unit {
                SourceUnit::Context(c) => (NodeKind::Context, c.name.span, &c.items, &c.consumes),
                SourceUnit::Adapter(a) => (NodeKind::Adapter, a.name.span, &a.items, &a.consumes),
                SourceUnit::Commons(_) | SourceUnit::Suite(_) => continue,
            };
            kind.get_or_insert(this_kind);
            loc.get_or_insert(Located {
                file: file.clone(),
                span: name_span,
            });

            for item in items {
                match item {
                    CommonsItem::Capability(c) => capabilities.push(ArchCapability {
                        name: c.name.name.clone(),
                        origin: CapabilityOrigin::Local,
                        loc: Located {
                            file: file.clone(),
                            span: c.name.span,
                        },
                    }),
                    CommonsItem::Provider(p) => providers.push(ArchProvider {
                        capability: p.capability.name.clone(),
                        provider_name: p.provider_name.name.clone(),
                        external: p.external,
                        loc: Located {
                            file: file.clone(),
                            span: p.provider_name.span,
                        },
                    }),
                    CommonsItem::Service(s) => services.push(ArchService {
                        name: s.name.name.clone(),
                        handler_count: s.handlers.len(),
                        loc: Located {
                            file: file.clone(),
                            span: s.name.span,
                        },
                    }),
                    CommonsItem::Agent(a) => agents.push(ArchAgent {
                        name: a.name.name.clone(),
                        handler_count: a.handlers.len(),
                        loc: Located {
                            file: file.clone(),
                            span: a.name.span,
                        },
                    }),
                    _ => {}
                }
            }

            for decl in consumes {
                let target = info
                    .and_then(|i| i.cross_context.resolve_prefix(&decl.target.joined()))
                    .unwrap_or_else(|| decl.target.joined());
                // #2765: the braces may also name an exported type
                // (`consumes bynk { TimedOut }`); only capabilities count here.
                let selected_caps: Vec<&Ident> = decl
                    .selected
                    .iter()
                    .flatten()
                    .filter(|id| {
                        info.is_none_or(|i| {
                            i.cross_context
                                .consumed_capabilities
                                .get(&target)
                                .is_some_and(|m| m.contains_key(&id.name))
                        })
                    })
                    .collect();
                let cap_labels: Vec<String> =
                    selected_caps.iter().map(|id| id.name.clone()).collect();

                // A selected capability flattens into the consumer's own
                // namespace regardless of whether `target` itself becomes a
                // node (the built-in `bynk` surface never does) — record the
                // binding on the node either way.
                {
                    for id in selected_caps {
                        capabilities.push(ArchCapability {
                            name: id.name.clone(),
                            origin: CapabilityOrigin::Consumed {
                                from: target.clone(),
                            },
                            loc: Located {
                                file: file.clone(),
                                span: id.span,
                            },
                        });
                    }
                }

                // The edge itself is only meaningful between two real nodes —
                // nothing to draw an arrow to for a synthetic/absent target.
                if node_names.contains(target.as_str()) {
                    edges.push(ArchEdge {
                        from: name.clone(),
                        to: target,
                        capabilities: cap_labels,
                        loc: Located {
                            file: file.clone(),
                            span: decl.span,
                        },
                    });
                }
            }
        }

        let (Some(kind), Some(loc)) = (kind, loc) else {
            continue;
        };
        // A multi-file unit can declare the same thing more than once across
        // its files in principle (a redundant `consumes X { Cap }` repeated
        // in two files of the same context, say) — dedupe by identity, not
        // by the whole struct (which would keep both copies apart on `loc`
        // alone), keeping the first (file-sorted, so deterministic) copy.
        capabilities.sort_by(|a, b| a.name.cmp(&b.name));
        capabilities.dedup_by(|a, b| a.name == b.name && a.origin == b.origin);
        providers.sort_by(|a, b| {
            (&a.capability, &a.provider_name).cmp(&(&b.capability, &b.provider_name))
        });
        providers
            .dedup_by(|a, b| a.capability == b.capability && a.provider_name == b.provider_name);
        services.sort_by(|a, b| a.name.cmp(&b.name));
        services.dedup_by(|a, b| a.name == b.name);
        agents.sort_by(|a, b| a.name.cmp(&b.name));
        agents.dedup_by(|a, b| a.name == b.name);

        nodes.push(ArchNode {
            name: name.clone(),
            kind,
            loc,
            capabilities,
            providers,
            services,
            agents,
        });
    }

    // A multi-file unit can carry more than one `consumes` clause naming the
    // same target across its files (e.g. `consumes X { A }` in one file and
    // `consumes X { B }` in another) — merge same-`(from, to)` edges into one
    // rather than drawing two arrows between the same pair of nodes, keeping
    // the first (file-sorted) edge's own location and the union of every
    // merged edge's capability labels.
    edges.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
    let mut merged_edges: Vec<ArchEdge> = Vec::with_capacity(edges.len());
    for edge in edges {
        match merged_edges
            .last_mut()
            .filter(|last: &&mut ArchEdge| last.from == edge.from && last.to == edge.to)
        {
            Some(last) => {
                for cap in edge.capabilities {
                    if !last.capabilities.contains(&cap) {
                        last.capabilities.push(cap);
                    }
                }
            }
            None => merged_edges.push(edge),
        }
    }
    for edge in &mut merged_edges {
        edge.capabilities.sort();
    }

    ArchModel {
        nodes,
        edges: merged_edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bynk-ide-architecture-test-{test_name}-{}",
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

    fn build_model(diag: &crate::ProjectDiagnostics) -> ArchModel {
        let snapshots: HashMap<PathBuf, String> = diag
            .files
            .iter()
            .map(|f| (f.source_path.clone(), f.text.clone()))
            .collect();
        architecture_model(&diag.unit_sources, &snapshots, &diag.sequence_info)
    }

    // -- Fixture: examples/rate-limiter's single context, self-contained (not
    // -- read from `examples/` — same convention as `sequence.rs`'s tests).
    // -- Its only consumes clause pulls a built-in capability (`Clock`) from
    // -- the synthetic `bynk` surface — the case with no edge, only a bound
    // -- capability.
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

    /// #2765: a type in the braces (`consumes bynk { Clock, TimedOut }`) is
    /// not a capability binding.
    #[test]
    fn a_selected_type_is_not_a_consumed_capability() {
        let src = RATELIMIT_SRC.replace(
            "consumes bynk { Clock }",
            "consumes bynk { Clock, TimedOut }",
        );
        let root = setup_project("type-selection", &[("ratelimit.bynk", &src)]);
        let diag = crate::testkit::diagnose_project(&root);
        let model = build_model(&diag);
        let names: Vec<&str> = model.nodes[0]
            .capabilities
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["Clock"]);
    }

    #[test]
    fn single_context_binds_a_builtin_capability_with_no_dangling_edge() {
        let root = setup_project("ratelimit", &[("ratelimit.bynk", RATELIMIT_SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let model = build_model(&diag);

        assert_eq!(
            model.nodes.len(),
            1,
            "the built-in bynk surface is not a node"
        );
        let node = &model.nodes[0];
        assert_eq!(node.name, "ratelimit");
        assert_eq!(node.kind, NodeKind::Context);
        assert_eq!(node.loc.file, PathBuf::from("ratelimit.bynk"));

        assert_eq!(node.agents.len(), 1);
        assert_eq!(node.agents[0].name, "Limiter");
        assert_eq!(node.agents[0].handler_count, 1);

        assert_eq!(node.services.len(), 1);
        assert_eq!(node.services[0].name, "api");
        assert_eq!(node.services[0].handler_count, 1);

        assert_eq!(
            node.capabilities,
            vec![ArchCapability {
                name: "Clock".to_string(),
                origin: CapabilityOrigin::Consumed {
                    from: "bynk".to_string()
                },
                loc: node.capabilities[0].loc.clone(),
            }],
            "Clock is bound via the built-in consumes, not locally declared"
        );

        assert!(
            model.edges.is_empty(),
            "no node exists for the synthetic bynk surface, so no edge is drawn to it"
        );
    }

    // -- Fixture: two real contexts, a braced selected-capability consumes —
    // -- the shape the issue's Done-when line calls for explicitly.
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
    fn two_context_project_gets_a_labelled_consumes_edge() {
        let root = setup_project(
            "twoctx",
            &[
                ("platformtime.bynk", PROVIDER_SRC),
                ("jobs.bynk", CONSUMER_SRC),
            ],
        );
        let diag = crate::testkit::diagnose_project(&root);
        let model = build_model(&diag);

        let names: Vec<&str> = model.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["ops.jobs", "platformtime"], "sorted by name");

        let provider_node = &model.nodes[1];
        assert_eq!(provider_node.capabilities.len(), 1);
        assert_eq!(provider_node.capabilities[0].name, "Clock");
        assert_eq!(
            provider_node.capabilities[0].origin,
            CapabilityOrigin::Local
        );
        assert_eq!(provider_node.providers.len(), 1);
        assert_eq!(provider_node.providers[0].provider_name, "SystemClock");
        assert!(!provider_node.providers[0].external);

        let consumer_node = &model.nodes[0];
        assert_eq!(
            consumer_node.capabilities,
            vec![ArchCapability {
                name: "Clock".to_string(),
                origin: CapabilityOrigin::Consumed {
                    from: "platformtime".to_string()
                },
                loc: consumer_node.capabilities[0].loc.clone(),
            }]
        );
        // Regression: this node's declaration lives in `jobs.bynk`, not
        // `platformtime.bynk` — a per-node uri computed from the wrong file
        // would silently open the other context on click-to-code.
        assert_eq!(consumer_node.loc.file, PathBuf::from("jobs.bynk"));

        assert_eq!(model.edges.len(), 1);
        let edge = &model.edges[0];
        assert_eq!(edge.from, "ops.jobs");
        assert_eq!(edge.to, "platformtime");
        assert_eq!(edge.capabilities, vec!["Clock".to_string()]);
        assert_eq!(edge.loc.file, PathBuf::from("jobs.bynk"));
    }

    // -- Regression: a multi-file unit's two files each `consumes` the same
    // -- target (a different braced capability each) — the two `ConsumesDecl`s
    // -- must merge into one edge (the union of both capability labels), not
    // -- draw two arrows between the same pair of nodes, and the node's own
    // -- bound-capability list must carry both without a stray duplicate.
    const TWO_CAP_PROVIDER_SRC: &str = r#"context platformtime

exports capability { Clock, Ping }

capability Clock {
  fn now() -> Effect[Int]
}

capability Ping {
  fn ping() -> Effect[Int]
}

provides Clock = SystemClock {
  fn now() -> Effect[Int] {
    0
  }
}

provides Ping = SystemPing {
  fn ping() -> Effect[Int] {
    1
  }
}
"#;
    const CONSUMER_FILE_A: &str = r#"context ops.jobs

consumes platformtime { Clock }

service run {
  on call() -> Effect[Int] given Clock {
    let now <- Clock.now()
    now
  }
}
"#;
    const CONSUMER_FILE_B: &str = r#"context ops.jobs

consumes platformtime { Ping }
"#;

    #[test]
    fn a_multi_file_unit_s_repeated_consumes_target_merges_into_one_edge() {
        let root = setup_project(
            "multifile_consumes",
            &[
                ("platformtime.bynk", TWO_CAP_PROVIDER_SRC),
                ("ops/jobs/a.bynk", CONSUMER_FILE_A),
                ("ops/jobs/b.bynk", CONSUMER_FILE_B),
            ],
        );
        let diag = crate::testkit::diagnose_project(&root);
        let model = build_model(&diag);

        assert_eq!(
            model.edges.len(),
            1,
            "both consumes clauses target platformtime — one edge, not two"
        );
        let edge = &model.edges[0];
        assert_eq!(edge.from, "ops.jobs");
        assert_eq!(edge.to, "platformtime");
        assert_eq!(
            edge.capabilities,
            vec!["Clock".to_string(), "Ping".to_string()]
        );

        let consumer = model.nodes.iter().find(|n| n.name == "ops.jobs").unwrap();
        let cap_names: Vec<&str> = consumer
            .capabilities
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            cap_names,
            vec!["Clock", "Ping"],
            "no duplicate capability rows"
        );
    }

    #[test]
    fn whole_unit_consumes_draws_an_unlabelled_edge_with_no_bound_capability() {
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
        let root = setup_project(
            "wholeunit",
            &[
                ("platform.bynk", PLATFORM_SRC),
                ("consumer.bynk", CONSUMER_SRC),
            ],
        );
        let diag = crate::testkit::diagnose_project(&root);
        let model = build_model(&diag);

        assert_eq!(model.edges.len(), 1);
        assert!(model.edges[0].capabilities.is_empty());
        let consumer = model.nodes.iter().find(|n| n.name == "consumer").unwrap();
        assert!(
            consumer.capabilities.is_empty(),
            "a whole-unit consumes flattens nothing into the local namespace"
        );
    }
}
