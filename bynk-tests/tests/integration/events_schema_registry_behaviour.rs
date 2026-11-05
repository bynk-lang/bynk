//! Events track, slice 3c (#980) — behavioural proof that the schema
//! registry (`bynk.schema.lock`) actually reaches the `Events.emit` mint
//! site, not just the reconciliation logic in isolation
//! (`bynk-emit::project::schema_registry`'s own unit tests cover that). Three
//! compiles of the same on-disk project, in order:
//!
//! 1. First compile (no prior registry): baselines at schema `1`, both in
//!    the written lock file and in the emitted `schemaVersion`.
//! 2. Second compile, after adding a defaulted field: the registry
//!    auto-bumps to `2` with no `@schema(N)` annotation required, and the
//!    *re-emitted* TypeScript reflects the bump — proving the registry's
//!    effective version, not just `EventDecl::schema_version()`'s `1`
//!    fallback, reaches emission.
//! 3. Third compile, after adding a field with no default (non-additive):
//!    the compile fails with `bynk.event.non_additive_schema_change`, and
//!    the registry file is left exactly as step 2 left it — a failed build
//!    must never perturb the committed registry.
//!
//! No `tsc`/`node` needed: the assertions are on the compiled TypeScript
//! string and the lock file's own content, not on running the emitted code
//! (`events_schema_version_behaviour.rs` already covers the runtime wire
//! value; this test is about the registry file, not the wire).

use bynkc::{BuildTarget, SchemaLock};
use std::fs;

const SOURCE_V1: &str = r#"context commerce.order

consumes bynk { Events }

event PaymentConfirmed = {
  orderId: String,
}

service markPaid {
  on call(orderId: String) -> Effect[()] given Events {
    Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId })
  }
}
"#;

const SOURCE_V2_ADDITIVE: &str = r#"context commerce.order

consumes bynk { Events }

event PaymentConfirmed = {
  orderId: String,
  region: String = "US",
}

service markPaid {
  on call(orderId: String) -> Effect[()] given Events {
    Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId, region: "US" })
  }
}
"#;

const SOURCE_V3_NON_ADDITIVE: &str = r#"context commerce.order

consumes bynk { Events }

event PaymentConfirmed = {
  orderId: String,
  region: String,
}

service markPaid {
  on call(orderId: String) -> Effect[()] given Events {
    Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId, region: "US" })
  }
}
"#;

#[test]
fn schema_registry_baselines_bumps_and_blocks_non_additive_changes() {
    let tmp = std::env::temp_dir().join(format!(
        "bynk-events-schema-registry-behaviour-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj/commerce");
    fs::create_dir_all(&proj).unwrap();
    let source_path = proj.join("order.bynk");
    let lock_path = tmp.join("proj/bynk.schema.lock");

    // -- 1. First compile: no prior registry, baselines at 1. -------------
    fs::write(&source_path, SOURCE_V1).unwrap();
    let out = compile(&tmp);
    assert!(
        lock_path.exists(),
        "a schema_registry-enabled compile must write bynk.schema.lock"
    );
    let lock_v1 = fs::read_to_string(&lock_path).unwrap();
    assert!(
        lock_v1.contains("schema = 1"),
        "a brand-new event must baseline at schema 1:\n{lock_v1}"
    );
    let ts_v1 = emitted_ts(&out);
    assert!(
        ts_v1.contains("schemaVersion: 1"),
        "the mint site must reflect the baselined version 1:\n{ts_v1}"
    );

    // -- 2. Second compile: an additive change (defaulted field) auto-bumps. --
    fs::write(&source_path, SOURCE_V2_ADDITIVE).unwrap();
    let out = compile(&tmp);
    let lock_v2 = fs::read_to_string(&lock_path).unwrap();
    assert!(
        lock_v2.contains("schema = 2"),
        "adding a field with a default must auto-bump the registry to 2, with \
         no `@schema(N)` annotation required:\n{lock_v2}"
    );
    let ts_v2 = emitted_ts(&out);
    assert!(
        ts_v2.contains("schemaVersion: 2"),
        "the re-emitted mint site must reflect the registry's bumped version, \
         not just `EventDecl::schema_version()`'s unconditional `1` fallback:\n{ts_v2}"
    );

    // -- 3. Third compile: a non-additive change must fail, and the
    //       registry on disk must be untouched by the failed attempt. -------
    //       #1078: this is now a *structural* guarantee, not an incidental
    //       one — `compile_project` returns `Err`, which carries no revised
    //       lock content at all, so there is nothing for a caller to write.
    fs::write(&source_path, SOURCE_V3_NON_ADDITIVE).unwrap();
    let options = bynk_testkit::compile_options_single(tmp.join("proj"))
        .target(BuildTarget::Bundle)
        .schema_registry(schema_lock_input(&tmp.join("proj")));
    match bynkc::compile_project(&options) {
        Ok(_) => panic!("a field added without a default must not compile"),
        Err(failure) => {
            let rendered = bynkc::render_project_errors(&failure.flatten());
            assert!(
                rendered.contains("bynk.event.non_additive_schema_change"),
                "expected the non-additive-change diagnostic:\n{rendered}"
            );
        }
    }
    let lock_after_failure = fs::read_to_string(&lock_path).unwrap();
    assert_eq!(
        lock_v2, lock_after_failure,
        "a failed build must never perturb the committed registry"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// #1078: `bynk-emit` no longer reads `bynk.schema.lock` itself — pre-read it
/// here exactly as `bynk-driver`'s two production wiring points
/// (`bynk::workers::compile_once`, `bynkc::main::run_compile`) do.
fn schema_lock_input(proj: &std::path::Path) -> SchemaLock {
    let existing = bynk_driver::schema_lock::read(proj)
        .expect("reading a scratch project's bynk.schema.lock must not fail");
    SchemaLock::On { existing }
}

/// #1078: mirrors the production read-before/write-after shape — `bynk-emit`
/// computes the reconciled content on `ProjectOutput::schema_lock`; this
/// helper does the disk write a real caller would, so the test's own
/// `fs::read_to_string(&lock_path)` assertions keep working unchanged.
fn compile(tmp: &std::path::Path) -> bynkc::ProjectOutput {
    let proj = tmp.join("proj");
    let options = bynk_testkit::compile_options_single(&proj)
        .target(BuildTarget::Bundle)
        .schema_registry(schema_lock_input(&proj));
    let out = match bynkc::compile_project(&options) {
        Ok(out) => out,
        Err(failure) => panic!(
            "compile the schema-registry project:\n{}",
            bynkc::render_project_errors(&failure.flatten())
        ),
    };
    if let Some(content) = &out.schema_lock {
        bynk_driver::schema_lock::write(&proj, content)
            .expect("writing a scratch project's bynk.schema.lock must not fail");
    }
    out
}

fn emitted_ts(out: &bynkc::ProjectOutput) -> String {
    out.artefacts
        .docs
        .values()
        .map(|doc| doc.text())
        .collect::<Vec<_>>()
        .join("\n")
}
