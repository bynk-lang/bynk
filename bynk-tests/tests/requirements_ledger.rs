//! v0.99 (DECISION C/D): the capability-requirement ledger records every
//! capability-consuming site — covered or not — with provenance derived from
//! the *source*, never from the capability. These tests drive `diagnose_project`
//! over one-file projects and inspect `ProjectDiagnostics::requirements`.

use std::fs;

use bynk_check::requirements::{Requirement, RequirementSource};
use bynk_ide::diagnose_project;

/// Analyse `bynk` as a one-file project and return its requirements (flattened
/// across files) plus the analysed text.
fn requirements_for(tag: &str, bynk: &str) -> Vec<Requirement> {
    let root = std::env::temp_dir().join(format!("bynk_reqledger_{}_{tag}", std::process::id()));
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("shop.bynk"), bynk).unwrap();
    let result = diagnose_project(
        &src,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            (src).to_path_buf(),
        )),
    );
    let reqs: Vec<Requirement> = result
        .requirements
        .values()
        .flat_map(|v| v.iter().cloned())
        .collect();
    let _ = fs::remove_dir_all(&root);
    reqs
}

/// An agent whose `Cache` store op needs `Clock`, but the handler declares it:
/// a **covered** `StoreOp` requirement, whose reason is the storage feature's
/// fragment — and no materialization (already covered).
#[test]
fn covered_cache_store_op_records_a_store_requirement() {
    let src = "context shop\n\
        \n\
        capability Clock { fn now() -> Effect[Int] }\n\
        provides Clock = C { fn now() -> Effect[Int] { 0 } }\n\
        \n\
        agent Sessions {\n\
        \x20 key id: String\n\
        \x20 store live: Cache[String, Int] @ttl(5.minutes)\n\
        \x20 on call put(t: String, u: Int) -> Effect[()] given Clock {\n\
        \x20   let _ <- live.put(t, u)\n\
        \x20   Effect.pure(())\n\
        \x20 }\n\
        }\n";
    let reqs = requirements_for("covered_store", src);
    let clock = reqs
        .iter()
        .find(|r| r.capability == "Clock")
        .expect("a Clock requirement is recorded");
    assert!(clock.covered, "the handler declares `given Clock`");
    assert!(
        clock.materialize.is_none(),
        "covered requirement offers no ghost `given`"
    );
    assert!(
        matches!(&clock.source, RequirementSource::StoreOp { op, .. } if op == "put"),
        "source is the Cache.put store op, got {:?}",
        clock.source
    );
    assert!(
        clock.source.reason("Clock").contains("reads the clock"),
        "store reason is the feature fragment"
    );
}

/// The same agent **without** the `given Clock` — an **uncovered** requirement
/// that carries the materialization edit driving the ghost `given` inlay hint.
#[test]
fn uncovered_cache_store_op_carries_materialization() {
    let src = "context shop\n\
        \n\
        agent Sessions {\n\
        \x20 key id: String\n\
        \x20 store live: Cache[String, Int] @ttl(5.minutes)\n\
        \x20 on call put(t: String, u: Int) -> Effect[()] {\n\
        \x20   let _ <- live.put(t, u)\n\
        \x20   Effect.pure(())\n\
        \x20 }\n\
        }\n";
    let reqs = requirements_for("uncovered_store", src);
    let clock = reqs
        .iter()
        .find(|r| r.capability == "Clock")
        .expect("a Clock requirement is recorded even though uncovered");
    assert!(!clock.covered);
    let m = clock
        .materialize
        .as_ref()
        .expect("uncovered requirement carries a materialization edit");
    assert_eq!(
        m.edit_text, " given Clock",
        "the edit synthesises the absent clause"
    );
}

/// A user-defined capability called directly: a `DirectCall` requirement whose
/// reason is just the call site — generated with **no** bespoke per-capability
/// text (DECISION C).
#[test]
fn direct_call_records_a_direct_requirement_with_no_bespoke_text() {
    let src = "context shop\n\
        \n\
        capability Payments {\n\
        \x20 fn authorise(amount: Int) -> Effect[Bool]\n\
        }\n\
        \n\
        provides Payments = StripePayments {\n\
        \x20 fn authorise(amount: Int) -> Effect[Bool] { true }\n\
        }\n\
        \n\
        agent Checkout {\n\
        \x20 key id: String\n\
        \x20 store total: Cell[Int] = 0\n\
        \x20 on call pay(amount: Int) -> Effect[Bool] given Payments {\n\
        \x20   let ok <- Payments.authorise(amount)\n\
        \x20   Effect.pure(ok)\n\
        \x20 }\n\
        }\n";
    let reqs = requirements_for("direct", src);
    let pay = reqs
        .iter()
        .find(|r| r.capability == "Payments")
        .expect("a Payments requirement is recorded");
    assert!(pay.covered);
    assert_eq!(pay.source.reason("Payments"), "calls `Payments.authorise`");
}
