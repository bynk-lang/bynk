//! #3583: `bynk/architectureModel`'s `events` and `uses` sections, end to end
//! against a real analysed project: an event emitted in one context and
//! subscribed in two (one through an aliased `consumes`, with a payload typed by
//! a commons), a service whose `default_given` supplies its handlers' `given`,
//! and a capability a suite stubs.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use bynk_ide::ProjectDiagnostics;
use bynk_lsp::architecture_request::{self, WireArchModel};

const VALUES: &str = "commons ev.values\n\ntype Qty = Int where InRange(1, 100)\n";

const EMITTER: &str = r#"context ev.emitter

uses ev.values
consumes bynk { Clock, Events }
exports transparent { Pinged }

event Pinged = { n: Qty }

capability Audit {
  fn note() -> Effect[()]
}

provides Audit = Ledger given Clock, Events {
  fn note() -> Effect[()] {
    let _ <- Clock.now()
    let _ <- Events.emit(Pinged { n: 1 })
    Effect.pure(())
  }
}

service api from http by Visitor given Clock, Events {
  on POST("/ping") () -> Effect[HttpResult[String]] {
    let _ <- Clock.now()
    let _ <- Events.emit[Pinged](Pinged { n: 7 })
    Created("sent")
  }

  on GET("/quiet") () -> Effect[HttpResult[String]] given Clock {
    let _ <- Clock.now()
    Created("quiet")
  }
}
"#;

const LISTENER: &str = r#"context ev.listener

consumes ev.emitter

service OnPing from Events(Pinged) {
  on event(e: Pinged) -> Effect[()] { () }
}
"#;

const FILTERED: &str = r#"context ev.filtered

consumes ev.emitter

service Seven from Events(Pinged { n: 7, .. }) {
  on event(e: Pinged) -> Effect[()] { () }
}

service Versioned from Events(Pinged) via schema(1) {
  on event(e: Pinged) -> Effect[()] { () }
}

service Both from Events(Pinged { n: 7, .. }) via schema(1) {
  on event(e: Pinged) -> Effect[()] { () }
}
"#;

const AUDITOR: &str = r#"context ev.auditor

consumes ev.emitter as em

service OnPing from Events(Pinged) {
  on event(e: Pinged) -> Effect[()] { () }
}
"#;

const SUITE: &str = r#"suite ev.emitter

stub Clock.now() returns 7

case "ping" {
  expect true
}
"#;

fn project(name: &str) -> (PathBuf, ProjectDiagnostics, HashMap<PathBuf, String>) {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-arch-events-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    for (rel, text) in [
        ("ev/values.bynk", VALUES),
        ("ev/emitter.bynk", EMITTER),
        ("ev/listener.bynk", LISTENER),
        ("ev/auditor.bynk", AUDITOR),
        ("ev/filtered.bynk", FILTERED),
        ("ev/emitter.test.bynk", SUITE),
    ] {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let diag = bynk_ide::diagnose_project(&root, &sources);
    let snaps = diag
        .files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect();
    (root, diag, snaps)
}

fn wire(name: &str) -> (WireArchModel, serde_json::Value) {
    let (root, diag, snaps) = project(name);
    let model = architecture_request::architecture_model_for(
        &diag.unit_sources,
        &snaps,
        &diag.sequence_info,
        &diag.architecture_facts,
    );
    let wire = architecture_request::to_wire(&model, &root, &snaps);
    let json = serde_json::to_value(&wire).unwrap();
    (wire, json)
}

#[test]
fn event_lists_its_emitter_and_both_subscribers_across_units() {
    let (wire, _) = wire("subs");
    assert_eq!(wire.events.len(), 1);
    let ev = &wire.events[0];
    assert_eq!(
        (ev.owner.as_str(), ev.name.as_str()),
        ("ev.emitter", "Pinged")
    );

    // The `POST /ping` handler and the `Ledger.note` provider operation emit (the latter with
    // its event type inferred from the argument, #4113).
    assert_eq!(ev.emitters.len(), 2);
    let e = ev
        .emitters
        .iter()
        .find(|e| e.member.kind == "service")
        .unwrap();
    assert_eq!(e.member.unit, "ev.emitter");
    assert_eq!(e.member.kind, "service");
    assert_eq!(e.member.container, "api");
    assert_eq!(e.member.handler, "POST /ping");
    assert!(e.loc.uri.as_str().ends_with("ev/emitter.bynk"));

    // Both subscribers resolve to the one event: the plain `consumes` and the
    // aliased one (`consumes ev.emitter as em`).
    let subs: Vec<(&str, &str, &str)> = ev
        .subscribers
        .iter()
        .map(|s| {
            (
                s.member.unit.as_str(),
                s.member.container.as_str(),
                s.delivery,
            )
        })
        .collect();
    assert_eq!(
        subs,
        vec![
            ("ev.auditor", "OnPing", "all"),
            ("ev.filtered", "Both", "patternSchema"),
            ("ev.filtered", "Seven", "pattern"),
            ("ev.filtered", "Versioned", "schema"),
            ("ev.listener", "OnPing", "all"),
        ]
    );
    assert!(
        ev.subscribers[0]
            .loc
            .uri
            .as_str()
            .ends_with("ev/auditor.bynk")
    );
}

#[test]
fn service_default_given_is_each_handlers_resolved_given_and_marked_inherited() {
    // `api` names `given Clock, Events` on the service; `GET /quiet` overrides
    // with its own `given Clock`, `POST /ping` inherits the default.
    let (wire, _) = wire("default-given");
    let node = wire.nodes.iter().find(|n| n.name == "ev.emitter").unwrap();
    let handler = |h: &str| node.uses.iter().find(|u| u.member.handler == h).unwrap();

    let ping = handler("POST /ping");
    let caps: Vec<(&str, bool, Option<&str>)> = ping
        .uses
        .iter()
        .map(|u| (u.capability.as_str(), u.inherited, u.from.as_deref()))
        .collect();
    assert_eq!(
        caps,
        vec![
            ("Clock", true, Some("bynk")),
            ("Events", true, Some("bynk"))
        ]
    );

    let quiet = handler("GET /quiet");
    assert_eq!(quiet.uses.len(), 1);
    assert_eq!(quiet.uses[0].capability, "Clock");
    assert!(!quiet.uses[0].inherited);
}

#[test]
fn a_capability_a_suite_stubs_names_the_suite() {
    let (wire, json) = wire("stubs");
    let node = wire.nodes.iter().find(|n| n.name == "ev.emitter").unwrap();
    for h in &node.uses {
        for u in h.uses.iter().filter(|u| u.capability == "Clock") {
            assert_eq!(u.stubbed_in.len(), 1, "{}", h.member.handler);
            assert!(u.stubbed_in[0].suite.ends_with("ev/emitter.test.bynk"));
        }
        for u in h.uses.iter().filter(|u| u.capability == "Events") {
            assert!(u.stubbed_in.is_empty());
        }
    }
    // The wire form is camelCase, like the fields beside it.
    let first_use = &json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "ev.emitter")
        .unwrap()["uses"][0]["uses"][0];
    assert!(first_use.get("stubbedIn").is_some(), "{first_use}");
}

#[test]
fn existing_fields_are_unchanged_by_the_new_sections() {
    let (_, json) = wire("compat");
    for node in json["nodes"].as_array().unwrap() {
        for key in [
            "name",
            "kind",
            "loc",
            "capabilities",
            "providers",
            "services",
            "agents",
            "uses",
        ] {
            assert!(node.get(key).is_some(), "node lacks `{key}`");
        }
    }
    assert!(json["edges"].is_array() && json["events"].is_array());
}

#[test]
fn a_provider_operation_is_a_member_whose_uses_are_never_inherited() {
    let (wire, _) = wire("provider");
    let node = wire.nodes.iter().find(|n| n.name == "ev.emitter").unwrap();
    let op = node
        .uses
        .iter()
        .find(|u| u.member.kind == "provider")
        .expect("the provider operation is a member");
    assert_eq!(
        (op.member.container.as_str(), op.member.handler.as_str()),
        ("Ledger", "note")
    );
    let caps: Vec<(&str, bool)> = op
        .uses
        .iter()
        .map(|u| (u.capability.as_str(), u.inherited))
        .collect();
    assert_eq!(caps, vec![("Clock", false), ("Events", false)]);
}
