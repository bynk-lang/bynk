//! 1B.2b (#4028): the contracts-lock tests (#2554) that build a project through the driver's
//! in-memory pipeline, moved from `bynk-emit` (`emitter/contracts.rs`).

#[path = "../support/emit_pipeline.rs"]
mod emit_pipeline;

use bynk_check::contract_compat::ServiceShape;
use bynk_driver::project::{BuildTarget, ProjectOutput, compile_files_in_memory_locked};
use bynk_project::contracts_lock::{self, ContractsLock};

fn shop(receipt: &str) -> String {
    format!(
        "context demo.shop\n\ntype Receipt = {receipt}\n\nservice calc {{\n  on call(n: Int) -> Effect[Receipt] {{\n    Effect.pure(Receipt {{ ok: true }})\n  }}\n}}\n"
    )
}

fn build(source: &str, lock: Option<&str>) -> Result<ProjectOutput, Vec<String>> {
    compile_files_in_memory_locked(
        &[("demo/shop.bynk", source)],
        BuildTarget::Workers,
        Default::default(),
        false,
        None,
        None,
        lock,
    )
    .map_err(|f| {
        f.errors
            .into_iter()
            .map(|e| format!("{}: {}", e.error.category, e.error.message))
            .collect()
    })
}

fn messages(out: &ProjectOutput) -> Vec<String> {
    out.warnings
        .iter()
        .map(|w| format!("{}: {}", w.error.category, w.error.message))
        .collect()
}

fn doc(out: &ProjectOutput, path: &str) -> String {
    out.artefacts.docs[std::path::Path::new(path)].text()
}

/// The shape text a build recorded for `calc`, undone from JSON.
fn recorded_shape(out: &ProjectOutput) -> String {
    let manifest = doc(out, "workers/demo-shop/bynk-contracts.json");
    let after = manifest
        .split("\"provides_shapes\"")
        .nth(1)
        .expect("shapes");
    let start = after.find("\"calc\": \"").expect("calc") + "\"calc\": \"".len();
    let mut text = String::new();
    let mut chars = after[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => text.push(chars.next().expect("escape")),
            c => text.push(c),
        }
    }
    text
}

fn lock_accepting(shape: &str) -> String {
    let mut lock = ContractsLock::new();
    lock.accept("demo.shop", "calc", shape);
    contracts_lock::serialize(&lock)
}

#[test]
fn a_callee_accepts_the_older_contract_its_lock_records() {
    let v1 = build(&shop("{ ok: Bool }"), None).expect("v1");
    let v1_shape = recorded_shape(&v1);
    let v1_hash = ServiceShape::from_text(&v1_shape).unwrap().hash();

    // v2 adds a response field: compatible for a caller on v1.
    let v2_source =
        shop("{ ok: Bool, at: Option[Int] }").replace("{ ok: true }", "{ ok: true, at: None }");
    let v2 = build(&v2_source, Some(&lock_accepting(&v1_shape))).expect("v2");
    let v2_hash = ServiceShape::from_text(&recorded_shape(&v2))
        .unwrap()
        .hash();
    assert_ne!(v1_hash, v2_hash);

    let index = doc(&v2, "workers/demo-shop/index.ts");
    assert!(
        index.contains(&format!(
            "__contract !== \"{v2_hash}\" && __contract !== \"{v1_hash}\""
        )),
        "{index}"
    );
    // The refusal still names the callee's own contract.
    assert!(
        index.contains("kind: \"ContractMismatch\", service: \"calc\""),
        "{index}"
    );
    let manifest = doc(&v2, "workers/demo-shop/bynk-contracts.json");
    assert!(
        manifest.contains(&format!("\"accepts\": {{\n    \"calc\": [\"{v1_hash}\"]")),
        "{manifest}"
    );
    assert!(v2.warnings.is_empty(), "{:?}", messages(&v2));
}

#[test]
fn without_a_lock_a_callee_accepts_only_its_own_contract() {
    let out = build(&shop("{ ok: Bool }"), None).expect("build");
    let index = doc(&out, "workers/demo-shop/index.ts");
    assert!(!index.contains("&& __contract !=="), "{index}");
    assert!(!doc(&out, "workers/demo-shop/bynk-contracts.json").contains("accepts"));
}

#[test]
fn an_older_contract_that_would_now_break_is_not_accepted() {
    let v1 = build(&shop("{ ok: Bool }"), None).expect("v1");
    let v1_shape = recorded_shape(&v1);
    // A retyped response field breaks a caller on v1.
    let v2_source = shop("{ ok: Int }").replace("{ ok: true }", "{ ok: 1 }");
    let v2 = build(&v2_source, Some(&lock_accepting(&v1_shape))).expect("v2");
    let index = doc(&v2, "workers/demo-shop/index.ts");
    assert!(!index.contains("&& __contract !=="), "{index}");
    assert!(!doc(&v2, "workers/demo-shop/bynk-contracts.json").contains("accepts"));
    let stale: Vec<_> = v2
        .warnings
        .iter()
        .filter(|w| w.error.category == "bynk.project.contracts_lock_stale")
        .collect();
    assert_eq!(stale.len(), 1, "{:?}", messages(&v2));
    assert!(
        stale[0].error.message.contains("demo.shop.calc"),
        "{}",
        stale[0].error.message
    );
}

#[test]
fn a_lock_entry_for_a_context_that_is_gone_is_a_warning() {
    let mut lock = ContractsLock::new();
    lock.accept("demo.gone", "calc", "Svc(\"calc\",Ret(U()))");
    let out = build(
        &shop("{ ok: Bool }"),
        Some(&contracts_lock::serialize(&lock)),
    )
    .expect("build");
    assert!(
        out.warnings
            .iter()
            .any(|w| w.error.category == "bynk.project.contracts_lock_stale"
                && w.error.message.contains("demo.gone.calc")),
        "{:?}",
        messages(&out)
    );
}

#[test]
fn a_corrupt_lock_fails_the_build() {
    let failure = build(&shop("{ ok: Bool }"), Some("not toml ["))
        .err()
        .expect("fails");
    assert!(
        failure
            .iter()
            .any(|e| e.starts_with("bynk.project.contracts_lock_corrupt")),
        "{failure:?}"
    );
}

#[test]
fn a_service_accepting_a_long_tail_of_older_contracts_is_warned() {
    // Receipts with 1..=9 optional fields, all compatible with one that has
    // ten: the callee accepts them all, and says that is a lot.
    let receipt = |n: usize| {
        let fields: Vec<String> = (1..=n).map(|i| format!("f{i}: Option[Int]")).collect();
        format!("{{ ok: Bool, {} }}", fields.join(", "))
    };
    let source = |n: usize| {
        let nones: Vec<String> = (1..=n).map(|i| format!("f{i}: None")).collect();
        shop(&receipt(n)).replace(
            "{ ok: true }",
            &format!("{{ ok: true, {} }}", nones.join(", ")),
        )
    };
    let mut lock = ContractsLock::new();
    for n in 1..=9 {
        let out = build(&source(n), None).expect("older build");
        lock.accept("demo.shop", "calc", &recorded_shape(&out));
    }
    let out = build(&source(10), Some(&contracts_lock::serialize(&lock))).expect("current build");
    let oversized: Vec<_> = out
        .warnings
        .iter()
        .filter(|w| w.error.category == "bynk.project.contracts_lock_oversized")
        .collect();
    assert_eq!(oversized.len(), 1, "{:?}", messages(&out));
    assert!(
        oversized[0]
            .error
            .message
            .contains("accepting 9 older contracts")
    );
    // All nine are still accepted: the warning does not drop any.
    let manifest = doc(&out, "workers/demo-shop/bynk-contracts.json");
    assert_eq!(manifest.matches("\"calc\": [").count(), 1, "{manifest}");
    assert_eq!(
        manifest
            .split("\"accepts\"")
            .nth(1)
            .unwrap()
            .matches(',')
            .count(),
        8,
        "{manifest}"
    );
}
