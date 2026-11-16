//! #3581 (the dev agent-inspection track), slice 0a: a build that asks for agent
//! inspection records each agent instance's key and answers a signed
//! `/_bynk/inspect/` route; every other build is byte-identical to what it was, so a
//! deploy build carries neither the route nor the registry.
//!
//! The fixture is `174_integration_full_checkout`: `shop.orders` hosts the `Ledger`
//! agent.

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

/// Decision D's golden: a deploy build has no inspection route, no registry and no
/// recording, and asking for none is the default and not a different build.
#[test]
fn a_deploy_build_carries_no_inspection_route_or_registry() {
    let files = build(bynk_testkit::compile_options_single(fixture_src()));
    for (path, text) in &files {
        for needle in [
            "_bynk/inspect",
            "BynkInspectRegistry",
            "__BYNK_INSPECT_REGISTRY",
            "__inspectRoute",
            "__inspectAgent",
            "__noteAgentInstance",
        ] {
            assert!(!text.contains(needle), "{path} mentions {needle}");
        }
    }
    assert!(
        !files
            .keys()
            .any(|p| p.ends_with("__bynk_runtime/inspect.ts")),
        "the inspection runtime module is emitted only for a dev build: {:?}",
        files.keys()
    );
    let explicit = build(bynk_testkit::compile_options_single(fixture_src()).inspect_agents(false));
    assert_eq!(files, explicit);
}

#[test]
fn a_dev_build_records_instances_and_answers_the_inspect_route() {
    let files = build(bynk_testkit::compile_options_single(fixture_src()).inspect_agents(true));
    // The factory hands its agent back through the recorder, with its key codec.
    let agents = find(&files, "shop-orders/handlers.ts");
    assert!(agents.contains("__noteAgentInstance(env?.__BYNK_INSPECT_REGISTRY, env?.LEDGER, \"Ledger\", key, __LedgerKey, __makeAgent("), "{agents}");
    // The agent's `fetch` answers `/_bynk/inspect` from its own storage, read-only.
    assert!(
        agents.contains("if (url.pathname === \"/_bynk/inspect\")"),
        "{agents}"
    );
    assert!(
        agents.contains("__inspectAgent(this.state, __entryFieldsOfLedger"),
        "{agents}"
    );
    // The Worker verifies and dispatches the signed route, and hosts the registry.
    let entry = find(&files, "shop-orders/index.ts");
    assert!(
        entry.contains("path.startsWith(\"/_bynk/inspect/\")"),
        "{entry}"
    );
    assert!(
        entry.contains("__inspectRoute(env, request, \"shop.orders\", { \"Ledger\": \"LEDGER\" })"),
        "{entry}"
    );
    assert!(entry.contains("export { BynkInspectRegistry }"), "{entry}");
    // A Worker that hosts no agent gets none of it.
    let payment = find(&files, "shop-payment/index.ts");
    assert!(!payment.contains("inspect"), "{payment}");
    assert!(
        files
            .keys()
            .any(|p| p.ends_with("__bynk_runtime/inspect.ts")),
        "{:?}",
        files.keys()
    );
}

#[test]
fn a_dev_build_adds_only_the_inspection_to_the_deploy_output() {
    let deploy = build(bynk_testkit::compile_options_single(fixture_src()));
    let dev = build(bynk_testkit::compile_options_single(fixture_src()).inspect_agents(true));
    let extra: Vec<&String> = dev.keys().filter(|p| !deploy.contains_key(*p)).collect();
    assert_eq!(extra.len(), 1, "{extra:?}");
    assert!(extra[0].ends_with("__bynk_runtime/inspect.ts"), "{extra:?}");
    // The Worker's config is the deploy build's: the dev copy is patched by `bynk dev`.
    for (path, text) in &deploy {
        if path.ends_with("wrangler.toml") {
            assert_eq!(Some(text), dev.get(path), "{path} changed in a dev build");
        }
    }
}
