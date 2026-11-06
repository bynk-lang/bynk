//! #611: the enumerable storage-operation registry must not drift from the
//! checker's dispatch. Every operation the registry lists is driven through the
//! real checker on a `store` field of the matching kind; none may be rejected as
//! `bynk.store.unknown_op` (a phantom registry entry). Args are omitted, so a
//! recognised operation surfaces only as an arity/argument error — which is
//! fine; we assert solely on `unknown_op`.
//!
//! **The tooth bites one way only.** This catches an operation the registry
//! lists and the checker does not have; it cannot catch the converse — an
//! operation added to a `check_store_*_op` arm later, which this table will
//! silently under-list (hover then falls through, so the failure is a *missing*
//! hover, not a wrong one). Catching that needs the checker's arms to be
//! enumerable, which is the thing the registry exists to work around. Nor are
//! the signature *strings* checked: the checker never reads them, so no probe
//! can observe them. `kernel_registry.rs` has the same shape and the same
//! limits; `store_ops`'s module doc states both.
//!
//! Agents need a `context`, which the single-file `compile` API does not accept,
//! so the probe is compiled as a one-file project in a temp directory.

use std::fs;

use bynk_check::store_ops::{
    CELL_STORE_OPS, LOG_STORE_OPS, MAP_STORE_OPS, SET_STORE_OPS, StoreOp, ops_for,
};
use bynkc::{ProjectFailure, compile_project};

/// `(store field declaration, receiver name, ops)` — one `store` field per
/// dispatched storage kind. `Cache` shares `MAP_STORE_OPS` and is covered by the
/// `ops_for` mapping; it is listed separately here because it needs `@ttl`.
fn cases() -> Vec<(&'static str, &'static str, &'static [StoreOp])> {
    vec![
        ("store cel: Cell[Int] = 0", "cel", CELL_STORE_OPS),
        ("store mp: Map[String, Int]", "mp", MAP_STORE_OPS),
        ("store st: Set[String]", "st", SET_STORE_OPS),
        (
            "store ca: Cache[String, Int] @ttl(5.minutes)",
            "ca",
            ops_for("Cache"),
        ),
        ("store lg: Log[Int]", "lg", LOG_STORE_OPS),
    ]
}

/// Compile a probe agent whose handler calls each `(receiver, op)` in `calls`,
/// and return the diagnostic category codes.
fn probe_codes(tag: &str, calls: &[(&str, &str)]) -> Vec<String> {
    let mut src = String::from("context probe\n\nagent Probe {\n  key id: String\n");
    for (field, _, _) in cases() {
        src += &format!("  {field}\n");
    }
    // `given Clock` — the `Cache`/`Log` write ops read the clock; without it they
    // raise a capability diagnostic that would drown the signal we assert on.
    src += "\n  on call f() -> Effect[()] given Clock {\n";
    for (recv, op) in calls {
        src += &format!("    let _ = {recv}.{op}()\n");
    }
    src += "    Effect.pure(())\n  }\n}\n";

    let root = std::env::temp_dir().join(format!("bynk_store_ops_{}_{tag}", std::process::id()));
    let dir = root.join("src");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("probe.bynk"), &src).unwrap();
    // Content-ownership track (#1086) slice 3: bynk-testkit's complete
    // sources map, proved here as one of the representative call sites,
    // instead of relying on bynk-emit's disk fallback.
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
fn store_op_registry_pins_dispatch() {
    let cases = cases();
    let calls: Vec<(&str, &str)> = cases
        .iter()
        .flat_map(|(_, recv, ops)| ops.iter().map(move |o| (*recv, o.name)))
        .collect();
    let codes = probe_codes("pins", &calls);
    assert!(
        !codes.iter().any(|c| c == "bynk.store.unknown_op"),
        "registry lists operation(s) the checker rejects as unknown:\n{}",
        codes.join("\n")
    );
}

/// The negative control for the test above: without it, a probe that failed to
/// reach store-op dispatch at all (a parse error, an unregistered store field)
/// would report no `unknown_op` and pass vacuously. A bogus op on every kind
/// must raise it.
#[test]
fn the_probe_actually_reaches_store_op_dispatch() {
    for (_, recv, _) in cases() {
        let codes = probe_codes(&format!("control_{recv}"), &[(recv, "definitelyNotAnOp")]);
        assert!(
            codes.iter().any(|c| c == "bynk.store.unknown_op"),
            "`{recv}.definitelyNotAnOp()` should be an unknown store op, got:\n{}",
            codes.join("\n")
        );
    }
}
