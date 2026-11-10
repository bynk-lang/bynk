//! #596: the `store Map` `.entries`/`.keys`/`.values` accessor registry must
//! not drift from the checker's dispatch. Every accessor it lists is driven
//! through the real checker on a `store Map` field; none may be rejected as
//! `bynk.store.unknown_map_accessor`.
//!
//! The `Query` *method* vocabulary needs no such test (#3164): the checker
//! matches on `KernelOp` exhaustively, so a listed method with no checker arm
//! (and the converse) is a compile error. The accessor registry
//! (`MAP_QUERY_ACCESSORS`) is a separate, hand-kept table, so it keeps its
//! test.
//!
//! A `store` field needs a `context`, so the probe is compiled as a one-file
//! project in a temp directory, the same shape `store_op_registry.rs` uses.

use std::fs;

use bynk_check::store_ops::MAP_QUERY_ACCESSORS;
use bynk_driver::project::ProjectFailure;
use bynk_driver::project::compile_project;

/// Compile a probe agent with `store items: Map[String, Int]` whose handler
/// evaluates each given expression (a `<recv>.<member>` access), and return
/// the diagnostic category codes.
fn probe_codes(tag: &str, exprs: &[String]) -> Vec<String> {
    let mut src = String::from(
        "context probe\n\nagent Probe {\n  key id: String\n  store items: Map[String, Int]\n\n  on call f() -> Effect[()] {\n",
    );
    for e in exprs {
        src += &format!("    let _ = {e}\n");
    }
    src += "    Effect.pure(())\n  }\n}\n";

    let root =
        std::env::temp_dir().join(format!("bynk_query_registry_{}_{tag}", std::process::id()));
    let dir = root.join("src");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("probe.bynk"), &src).unwrap();
    let out = match compile_project(&bynk_testkit::compile_options_single(dir)) {
        Ok(_) => Vec::new(),
        Err(f) => ProjectFailure::flatten(f)
            .iter()
            .map(|e| e.category.to_string())
            .collect(),
    };
    let _ = fs::remove_dir_all(&root);
    out
}

#[test]
fn map_query_accessor_registry_pins_dispatch() {
    let exprs: Vec<String> = MAP_QUERY_ACCESSORS
        .iter()
        .map(|a| format!("items.{}", a.name))
        .collect();
    let codes = probe_codes("accessors_pin", &exprs);
    assert!(
        !codes.iter().any(|c| c == "bynk.store.unknown_map_accessor"),
        "registry lists accessor(s) the checker rejects as unknown:\n{}",
        codes.join("\n")
    );
}

/// Negative control: without it, a probe that never reached accessor dispatch
/// (a parse error, an unregistered store field) would pass vacuously.
#[test]
fn the_probe_actually_reaches_map_accessor_dispatch() {
    let codes = probe_codes(
        "accessors_control",
        &["items.definitelyNotAnAccessor".to_string()],
    );
    assert!(
        codes.iter().any(|c| c == "bynk.store.unknown_map_accessor"),
        "`items.definitelyNotAnAccessor` should be an unknown map accessor, got:\n{}",
        codes.join("\n")
    );
}
