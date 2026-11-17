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
            "__historyBefore",
            "__historyRecord",
            "__bynk_dev/history",
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
    // Its commit records the transition, the handler's name riding with it.
    assert!(
        agents.contains(
            "const __before = await __historyBefore(this.state.storage, __entryFieldsOfLedger)"
        ),
        "{agents}"
    );
    assert!(
        agents.contains(
            "await __historyRecord(this.state.storage, __entryFieldsOfLedger, __before, __handler)"
        ),
        "{agents}"
    );
    assert!(
        agents.contains("commitState(__state, \"tally\")"),
        "{agents}"
    );
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

// -- Worker groups (#3562, G6) -------------------------------------------------

/// A grouped project on disk: `shop.orders` and `shop.payment` in group `core`,
/// each hosting an agent of its own name.
fn grouped_project(tag: &str) -> PathBuf {
    grouped_project_with(tag, false)
}

/// [`grouped_project`], optionally with `shop.audit`, a context that hosts no
/// agent, listed **first** in the group.
fn grouped_project_with(tag: &str, audit_first: bool) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("bynk-group-inspect-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src/shop")).unwrap();
    let contexts = if audit_first {
        "\"shop.audit\", \"shop.orders\", \"shop.payment\""
    } else {
        "\"shop.orders\", \"shop.payment\""
    };
    std::fs::write(
        root.join("bynk.toml"),
        format!(
            "[project]\nname = \"x\"\nversion = \"0.1.0\"\n\n[deploy.workers.core]\ncontexts = [{contexts}]\n"
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("src/shop/audit.bynk"),
        "context shop.audit\n\nservice probe_audit {\n\ton call() -> Effect[Int] {\n\t\tEffect.pure(1)\n\t}\n}\n",
    )
    .unwrap();
    for (context, file, agent) in [
        ("shop.orders", "orders", "Cart"),
        ("shop.payment", "payment", "Wallet"),
    ] {
        std::fs::write(
            root.join(format!("src/shop/{file}.bynk")),
            format!(
                "context {context}\n\nagent {agent} {{\n\tkey id: String\n\tstore n: Cell[Int] = 0\n\n\ton call bump() -> Effect[Int] {{ Effect.pure(1) }}\n}}\n\nservice probe_{file} {{\n\ton call() -> Effect[Int] {{\n\t\tEffect.pure(1)\n\t}}\n}}\n"
            ),
        )
        .unwrap();
    }
    root
}

fn build_grouped(root: &std::path::Path, inspect: bool) -> BTreeMap<String, String> {
    let paths = bynk_driver::project::try_read_project_paths(root).expect("the manifest reads");
    build(bynk_testkit::compile_options_split(root, paths).inspect_agents(inspect))
}

/// A grouped deploy build, like any deploy build, has no inspection route: not in
/// the group's entry that dispatches to the members, not in a member's, and no
/// registry or recording anywhere in the Worker's directory.
#[test]
fn a_deploy_build_of_a_group_has_no_inspection_route_anywhere() {
    let root = grouped_project("deploy");
    let files = build_grouped(&root, false);
    assert!(
        files.contains_key("workers/core/index.ts"),
        "{:?}",
        files.keys()
    );
    for (path, text) in files.iter().filter(|(p, _)| p.starts_with("workers/core/")) {
        for needle in [
            "_bynk/inspect",
            "BynkInspectRegistry",
            "__BYNK_INSPECT_REGISTRY",
            "__inspectRoute",
            "__inspectAgent",
            "__noteAgentInstance",
            "bynk.dev",
        ] {
            assert!(!text.contains(needle), "{path} mentions {needle}");
        }
    }
    assert!(
        !files
            .keys()
            .any(|p| p.ends_with("__bynk_runtime/inspect.ts") || p.ends_with("bynk-inspect.json")),
        "{:?}",
        files.keys()
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A grouped dev build gates the route per member, before any signature is read:
/// a member answers an agent of its own class, the first member that hosts an
/// agent also answers the listing and exports the registry once, and the group's
/// entry answers a request no member claims as a standalone dev Worker answers
/// an unsigned one.
#[test]
fn a_dev_build_of_a_group_gates_the_inspect_route_per_member() {
    let root = grouped_project("dev");
    let files = build_grouped(&root, true);
    let orders = find(&files, "workers/core/shop-orders.index.ts");
    let payment = find(&files, "workers/core/shop-payment.index.ts");
    let guard = |classes: &str| {
        format!(
            "path.startsWith(\"/_bynk/inspect/agent/\") && [{classes}].includes(path.slice(\"/_bynk/inspect/agent/\".length))"
        )
    };
    // The first agent-hosting member answers the listing and its own agents.
    assert!(
        orders.contains(&format!(
            "path.startsWith(\"/_bynk/inspect/\") && (path === \"/_bynk/inspect/agents\" || {})",
            guard("\"Cart\"")
        )),
        "{orders}"
    );
    // The other answers its own agents only, and does not export the registry.
    assert!(
        payment.contains(&format!(
            "path.startsWith(\"/_bynk/inspect/\") && ({})",
            guard("\"Wallet\"")
        )),
        "{payment}"
    );
    assert!(!payment.contains("/_bynk/inspect/agents"), "{payment}");
    // The registry class is exported once, by the group.
    let index = find(&files, "workers/core/index.ts");
    assert_eq!(index.matches("BynkInspectRegistry").count(), 1, "{index}");
    assert!(
        index.contains("export { Cart, BynkInspectRegistry } from \"./shop-orders.index.js\";"),
        "{index}"
    );
    // A request no member claims is answered as an unsigned one would be.
    assert!(
        index.contains("path.startsWith(\"/_bynk/inspect/\")"),
        "{index}"
    );
    assert!(index.contains("\"Method Not Allowed\""), "{index}");
    let _ = std::fs::remove_dir_all(&root);
}

/// The listing belongs to the first member that hosts an agent, not to the first
/// member: with an agent-less context listed first (which has no inspect route at
/// all), the next member still answers `/_bynk/inspect/agents` and the group still
/// exports the registry class, once, from it.
#[test]
fn the_listing_goes_to_the_first_member_that_hosts_an_agent() {
    let root = grouped_project_with("audit-first", true);
    let files = build_grouped(&root, true);
    let audit = find(&files, "workers/core/shop-audit.index.ts");
    assert!(!audit.contains("inspect"), "{audit}");
    let orders = find(&files, "workers/core/shop-orders.index.ts");
    assert!(
        orders.contains("path === \"/_bynk/inspect/agents\""),
        "{orders}"
    );
    let payment = find(&files, "workers/core/shop-payment.index.ts");
    assert!(!payment.contains("/_bynk/inspect/agents"), "{payment}");
    let index = find(&files, "workers/core/index.ts");
    assert_eq!(index.matches("BynkInspectRegistry").count(), 1, "{index}");
    assert!(
        index.contains("export { Cart, BynkInspectRegistry } from \"./shop-orders.index.js\";"),
        "{index}"
    );
    // The agent-less member exports no class, so the group re-exports nothing of it.
    assert!(
        !index
            .lines()
            .any(|l| l.starts_with("export") && l.contains("shop-audit")),
        "{index}"
    );
    // `bynk dev inspect` signs for the first agent-hosting member (the listing) and for
    // a class's owner (a read), so the build names them: the members that host an agent,
    // in the entry's order, and none that does not.
    let manifest: serde_json::Value =
        serde_json::from_str(find(&files, "workers/core/bynk-inspect.json")).unwrap();
    assert_eq!(
        manifest,
        serde_json::json!({ "members": [
            { "context": "shop.orders", "classes": ["Cart"] },
            { "context": "shop.payment", "classes": ["Wallet"] },
        ]}),
        "{manifest}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
