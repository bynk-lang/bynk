//! v0.34 (ADR 0067): the call-hierarchy graph — caller→callee `CallEdge`s
//! assembled by preserving each `RefEdge`'s `owner` (resolved to the caller's
//! `SymbolKey`). `Fn`/`Method` callees; any indexed owner may be a caller.
//! v0.36 (ADR 0069): methods are now index symbols, so a method caller
//! (`Counter.bump`) records an edge too. The fixture exercises a free fn
//! caller, a service caller, and a method caller.
//! #304: `CapabilityOp` and agent `Handler` callees join the graph too — see
//! `803_capability_op_and_handler_call_hierarchy` below.

use std::path::{Path, PathBuf};

use bynk_check::index::{ProjectIndex, SymbolKey, SymbolKind};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/215_call_hierarchy/src")
}

fn capability_op_and_handler_fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/803_capability_op_and_handler_call_hierarchy/src")
}

fn analyse(root: &Path) -> ProjectIndex {
    let result = bynk_ide::diagnose_project(
        root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (root).to_path_buf(),
        )),
    );
    for f in &result.files {
        assert!(
            f.diagnostics.is_empty(),
            "fixture should be clean, got in {}: {:?}",
            f.source_path.display(),
            f.diagnostics
                .iter()
                .map(|d| d.error.category)
                .collect::<Vec<_>>()
        );
    }
    result.index
}

fn key(unit: &str, kind: SymbolKind, name: &str) -> SymbolKey {
    SymbolKey {
        unit: unit.to_string(),
        kind,
        name: name.to_string(),
    }
}

#[test]
fn call_graph_records_fn_service_and_method_callers() {
    let index = analyse(&fixture_root());

    let helper = key("callgraph", SymbolKind::Fn, "helper");
    let caller = key("callgraph", SymbolKind::Fn, "caller");
    let bump = key("callgraph", SymbolKind::Method, "Counter.bump");
    let api = key("callgraph", SymbolKind::Service, "api");

    // Incoming: `helper` is called by the free fn `caller` and by the method
    // `Counter.bump`. v0.36: methods are indexed, so the method caller now
    // records an edge too (sorted by caller def position: caller before bump).
    let into_helper: Vec<&SymbolKey> = index.calls_into(&helper).map(|e| &e.caller).collect();
    assert_eq!(
        into_helper,
        vec![&caller, &bump],
        "helper is called by the free fn `caller` and the method `Counter.bump`"
    );
    assert_eq!(index.calls_into(&helper).count(), 2);

    // Incoming: `caller` is called by the service handler `api`.
    let into_caller: Vec<&SymbolKey> = index.calls_into(&caller).map(|e| &e.caller).collect();
    assert_eq!(
        into_caller,
        vec![&api],
        "an indexed service owner is a valid caller"
    );

    // Outgoing mirrors incoming off the same table.
    let from_caller: Vec<&SymbolKey> = index.calls_from(&caller).map(|e| &e.callee).collect();
    assert_eq!(from_caller, vec![&helper]);
    let from_api: Vec<&SymbolKey> = index.calls_from(&api).map(|e| &e.callee).collect();
    assert_eq!(from_api, vec![&caller]);

    // `helper` calls nothing.
    assert!(index.calls_from(&helper).next().is_none());

    // Each call site spells the callee name (sanity: spans are name segments).
    for e in &index.calls {
        assert!(
            !e.site.span.range().is_empty(),
            "call-site span must be non-empty"
        );
    }
}

#[test]
fn unknown_key_has_no_calls() {
    let index = analyse(&fixture_root());
    let ghost = key("callgraph", SymbolKind::Fn, "nope");
    assert_eq!(index.calls_into(&ghost).count(), 0);
    assert_eq!(index.calls_from(&ghost).count(), 0);
}

/// #304: `CapabilityOp` (ADR 0069 scoped its call-graph edges out; reversed —
/// the reference-count/incoming-call mismatch it produced was more confusing
/// than the edge it withheld) and agent `Handler` (new this increment) join
/// the call graph. The fixture's `api` service handler calls a local
/// capability op (`Clock.now`) and dispatches an agent handler
/// (`Order.addItem`) — both must now show `api` as their caller.
#[test]
fn call_graph_records_capability_op_and_agent_handler_callees() {
    let index = analyse(&capability_op_and_handler_fixture_root());

    let clock_now = key("billing", SymbolKind::CapabilityOp, "Clock.now");
    let order_add_item = key("billing", SymbolKind::Handler, "Order.addItem");
    let api = key("billing", SymbolKind::Service, "api");

    let into_clock_now: Vec<&SymbolKey> = index.calls_into(&clock_now).map(|e| &e.caller).collect();
    assert_eq!(
        into_clock_now,
        vec![&api],
        "a local capability-op call is now a call-hierarchy edge"
    );

    let into_order_add_item: Vec<&SymbolKey> = index
        .calls_into(&order_add_item)
        .map(|e| &e.caller)
        .collect();
    assert_eq!(
        into_order_add_item,
        vec![&api],
        "an agent-handler dispatch call is now a call-hierarchy edge"
    );

    let from_api: Vec<&SymbolKey> = index.calls_from(&api).map(|e| &e.callee).collect();
    assert_eq!(
        from_api,
        vec![&clock_now, &order_add_item],
        "outgoing calls mirror incoming off the same table"
    );
}
