//! #3580 (the dev request-traces track), slice 1a: a build that asks for traces
//! wraps the three traced sites and the Worker's `fetch`; every other build is
//! byte-identical to what it was, so a deploy build carries no recording.
//!
//! The fixture is `174_integration_full_checkout`: `shop.orders` makes a
//! cross-context call (`Pay.authorise`) and a let-bound agent call
//! (`ledger.tally`), and `shop.payment` makes a capability call (`Bank.charge`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use bynk_driver::project::{BuildTarget, CompileOptions};

fn fixture_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/174_integration_full_checkout/src")
}

/// Every emitted file of a Workers build, by its path, as text.
fn build(options: CompileOptions) -> BTreeMap<String, String> {
    let out =
        bynk_driver::project::compile_project(&options.target(BuildTarget::Workers).tests(false))
            .map_err(bynk_driver::project::ProjectFailure::flatten)
            .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
    out.artefacts
        .docs
        .iter()
        .map(|(path, doc)| (path.to_string_lossy().replace('\\', "/"), doc.text()))
        .collect()
}

fn find<'a>(files: &'a BTreeMap<String, String>, suffix: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p.ends_with(suffix))
        .unwrap_or_else(|| panic!("no emitted file ends {suffix:?}: {:?}", files.keys()))
        .1
}

#[test]
fn a_build_that_does_not_ask_for_traces_carries_none() {
    let files = build(bynk_testkit::compile_options_single(fixture_src()));
    for (path, text) in &files {
        assert!(
            !text.contains("__trace"),
            "{path} mentions the trace helpers"
        );
        assert!(
            !text.contains("__bynk_trace"),
            "{path} mentions the trace line"
        );
    }
    assert!(
        !files.keys().any(|p| p.ends_with("__bynk_runtime/trace.ts")),
        "the trace runtime module is emitted only for a traced build: {:?}",
        files.keys()
    );
    // Asking for no traces is the default, not a different build.
    let explicit = build(bynk_testkit::compile_options_single(fixture_src()).trace_requests(false));
    assert_eq!(files, explicit);
}

#[test]
fn a_traced_build_wraps_the_call_agent_and_capability_sites_with_their_spans() {
    let files = build(bynk_testkit::compile_options_single(fixture_src()).trace_requests(true));
    // The cross-context call, spanned at the call's method name in `orders.bynk`.
    let orders = find(&files, "shop-orders/handlers.ts");
    assert!(
        orders.contains(r#"__traceStep("call", "shop.payment.authorise", { file: "shop/orders.bynk", line: 23, col: 14 })(__callService("#),
        "{orders}"
    );
    // The let-bound agent call.
    assert!(
        orders.contains(r#"__traceStep("agent", "Ledger.tally", { file: "shop/orders.bynk", line: 27, col: 22 })("#),
        "{orders}"
    );
    // The capability op in `payment.bynk`.
    let payment = find(&files, "shop-payment/handlers.ts");
    assert!(
        payment.contains(r#"__traceStep("capability", "Bank.charge", { file: "shop/payment.bynk", line: 19, col: 14 })("#),
        "{payment}"
    );
    // Each handlers module imports what it wraps with.
    assert!(orders.contains("__traceStep"), "{orders}");
    // The entry serves each request under a record, and hands it a caught fault.
    let entry = find(&files, "shop-orders/index.ts");
    assert!(
        entry.contains("__traceFetch(\"shop.orders\", __bynkTracedFetch, request, env, ctx)"),
        "{entry}"
    );
    assert!(entry.contains("__traceFault(e)"), "{entry}");
    // The recorder is emitted, once the build reaches it.
    assert!(
        files.keys().any(|p| p.ends_with("__bynk_runtime/trace.ts")),
        "{:?}",
        files.keys()
    );
}

#[test]
fn a_traced_build_adds_only_the_recording_to_the_deploy_output() {
    let deploy = build(bynk_testkit::compile_options_single(fixture_src()));
    let traced = build(bynk_testkit::compile_options_single(fixture_src()).trace_requests(true));
    // The same files, plus the recorder's runtime module.
    let extra: Vec<&String> = traced.keys().filter(|p| !deploy.contains_key(*p)).collect();
    assert_eq!(extra.len(), 1, "{extra:?}");
    assert!(extra[0].ends_with("__bynk_runtime/trace.ts"), "{extra:?}");
    // The files that carry no traced site and no entry are byte-identical.
    for (path, text) in &deploy {
        if ["handlers.ts", "handlers.ts.map", "index.ts", "index.ts.map"]
            .iter()
            .any(|s| path.ends_with(s))
        {
            continue;
        }
        assert_eq!(
            Some(text),
            traced.get(path),
            "{path} changed in a traced build"
        );
    }
}
