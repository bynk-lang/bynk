//! #851: `bynk/architectureModel` — the whole-project architecture-map
//! query, end to end against a real analysed project. `bynk_lsp::architecture_request`'s
//! own in-crate tests cover the wire-lowering edge cases (per-node `uri`
//! correctness, camelCase params); this file covers the two fixtures the
//! issue's own "Done when" line calls for: a single-context project (the
//! real `examples/rate-limiter`, not a paraphrase — same convention
//! `document_link_fixture.rs` uses) and a two-context project with a braced
//! `consumes … { Cap }` edge.
//!
//! Reads `../examples/rate-limiter`, outside this crate's package — excluded
//! from the published tarball in `Cargo.toml`, same as `document_link_fixture.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_ide::ProjectDiagnostics;
use bynk_ide::architecture::{CapabilityOrigin, NodeKind};
use bynk_lsp::architecture_request;

fn diagnose(root: &Path) -> ProjectDiagnostics {
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources = bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
        root.to_path_buf(),
    ));
    bynk_ide::diagnose_project(root, &sources)
}

fn snapshots(diag: &ProjectDiagnostics) -> HashMap<PathBuf, String> {
    diag.files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect()
}

/// The issue's own worked example: a single-context project — the real
/// `examples/rate-limiter`, its `Clock` capability (consumed from the
/// built-in `bynk` surface, so no edge is drawn — see `bynk-ide`'s
/// `architecture.rs` module doc), an `api` service, and a `Limiter` agent.
#[test]
fn rate_limiter_is_one_node_with_a_bound_capability_and_no_dangling_edge() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/rate-limiter/src");
    let diag = diagnose(&root);
    let snaps = snapshots(&diag);
    let model = architecture_request::architecture_model_for(
        &diag.unit_sources,
        &snaps,
        &diag.sequence_info,
        &diag.architecture_facts,
    );

    assert_eq!(model.nodes.len(), 1);
    let node = &model.nodes[0];
    assert_eq!(node.name, "ratelimit");
    assert_eq!(node.kind, NodeKind::Context);
    assert_eq!(
        node.services
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        vec!["api"]
    );
    assert_eq!(
        node.agents
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Limiter"]
    );
    assert_eq!(node.capabilities.len(), 1);
    assert_eq!(node.capabilities[0].name, "Clock");
    assert_eq!(
        node.capabilities[0].origin,
        CapabilityOrigin::Consumed {
            from: "bynk".to_string()
        }
    );
    assert!(model.edges.is_empty());

    // The wire form: every location resolves to a real file URI under the
    // fixture's own `src`, and click-to-code targets a non-empty range.
    let wire = architecture_request::to_wire(&model, &root, &snaps);
    assert_eq!(wire.nodes.len(), 1);
    let wire_node = &wire.nodes[0];
    assert!(wire_node.loc.uri.as_str().ends_with("ratelimit.bynk"));
    let (s, e) = (wire_node.loc.range.start, wire_node.loc.range.end);
    assert!((s.line, s.character) < (e.line, e.character));
    assert_eq!(wire_node.capabilities.len(), 1);
    assert!(!wire_node.capabilities[0].local);
    assert_eq!(wire_node.capabilities[0].from.as_deref(), Some("bynk"));
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

fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-architecture-fixture-{test_name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create test root");
    for (rel, contents) in files {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&p, contents).expect("write file");
    }
    root
}

/// The two-context case the issue's Done-when line names explicitly: "a
/// two-context project with a `consumes` edge and a selected-capability
/// label."
#[test]
fn two_context_project_has_a_consumes_edge_with_a_selected_capability_label() {
    let root = setup_project(
        "twoctx",
        &[
            ("platformtime.bynk", PROVIDER_SRC),
            ("jobs.bynk", CONSUMER_SRC),
        ],
    );
    let diag = diagnose(&root);
    let snaps = snapshots(&diag);
    let model = architecture_request::architecture_model_for(
        &diag.unit_sources,
        &snaps,
        &diag.sequence_info,
        &diag.architecture_facts,
    );
    let wire = architecture_request::to_wire(&model, &root, &snaps);

    assert_eq!(wire.nodes.len(), 2);
    assert_eq!(wire.edges.len(), 1);
    let edge = &wire.edges[0];
    assert_eq!(edge.from, "ops.jobs");
    assert_eq!(edge.to, "platformtime");
    assert_eq!(edge.capabilities, vec!["Clock".to_string()]);
    assert!(edge.loc.uri.as_str().ends_with("jobs.bynk"));

    let platformtime = wire
        .nodes
        .iter()
        .find(|n| n.name == "platformtime")
        .expect("provider node");
    assert_eq!(platformtime.kind, "Context");
    assert_eq!(platformtime.providers.len(), 1);
    assert_eq!(platformtime.providers[0].provider_name, "SystemClock");
    assert!(!platformtime.providers[0].external);
    assert!(platformtime.loc.uri.as_str().ends_with("platformtime.bynk"));
}
