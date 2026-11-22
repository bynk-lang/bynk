//! Structural guard (Decision I of #3928, back-half paydown 1B.1c): the event
//! failure-isolation code appears exactly once in production code, and the two
//! compose renderers read the plan rather than re-deriving composition facts.
//! See `xtask/src/delivery_isolation.rs`.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
fn delivery_isolation_has_one_home() {
    let root = repo_root();
    let marker = xtask::delivery_isolation::marker();
    let found = xtask::delivery_isolation::occurrences(&root, &marker);
    assert_eq!(
        found.len(),
        1,
        "the failure-isolation statement (ADR 0284) must be written once, by \
         `bynk-emit`'s `compose::events::isolated_delivery`; its log message `{marker}` \
         occurs {} times in production code (back-half paydown 1B.1c, Decision I of \
         #3928): {found:?}",
        found.len(),
    );
    assert!(
        found[0].starts_with("bynk-emit/src/emitter/compose/events.rs:"),
        "the one copy is expected in `compose/events.rs`, found {found:?}"
    );
}

#[test]
fn the_compose_renderers_read_the_plan() {
    let root = repo_root();
    for renderer in ["bundle.rs", "workers.rs"] {
        let path = root.join("bynk-emit/src/emitter/compose").join(renderer);
        let source = std::fs::read_to_string(&path).expect("a compose renderer exists");
        for needle in [
            "discover_event_subscribers(",
            "handler_cross_caps(",
            ".exec_plan",
        ] {
            let lines = xtask::delivery_isolation::occurrences_in(&source, needle);
            assert!(
                lines.is_empty(),
                "compose/{renderer} line(s) {lines:?} read `{needle}` directly; the renderers \
                 read the `ComposePlan` (1B.1, Decision I of #3928)"
            );
        }
    }
}
