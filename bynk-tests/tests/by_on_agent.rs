//! v0.99 (DECISION H): `by` is a service-edge clause. An agent `on call`
//! handler is reached across the agent boundary by the factory (`__makeAgent`),
//! never from an ingress, so it has no actor. The parser accepts a `by` clause
//! on any handler; the checker rejects it on an agent handler with
//! `bynk.actor.by_on_agent`, turning the deps-split taxonomy's "actor auth never
//! crosses the agent boundary" guarantee into an enforced invariant.
//!
//! Agents need a `context`, which the single-file `compile` API does not accept,
//! so each case is compiled as a one-file project in a temp directory.

use std::fs;

use bynkc::{ProjectFailure, compile_project};

/// Compile `bynk` as a one-file `context shop` and return its diagnostic
/// category codes (empty on success).
fn codes(tag: &str, bynk: &str) -> Vec<String> {
    let root = std::env::temp_dir().join(format!("bynk_byagent_{}_{tag}", std::process::id()));
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("shop.bynk"), bynk).unwrap();
    let out = match compile_project(&bynk_testkit::compile_options_single(src.clone())) {
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
fn by_on_agent_handler_is_rejected() {
    let src = "context shop\n\
        \n\
        actor Caller { auth = Bearer(secret = \"AUTH_JWT_SECRET\"), identity = UserId }\n\
        type UserId = String\n\
        \n\
        agent A {\n\
        \x20 key id: String\n\
        \x20 store count: Cell[Int] = 0\n\
        \x20 on call f(p: Int) -> Effect[()] by Caller {\n\
        \x20   Effect.pure(())\n\
        \x20 }\n\
        }\n";
    let cs = codes("reject", src);
    assert!(
        cs.iter().any(|c| c == "bynk.actor.by_on_agent"),
        "expected bynk.actor.by_on_agent, got {cs:?}"
    );
}

#[test]
fn agent_handler_without_by_compiles_cleanly() {
    let src = "context shop\n\
        \n\
        agent A {\n\
        \x20 key id: String\n\
        \x20 store count: Cell[Int] = 0\n\
        \x20 on call f(p: Int) -> Effect[()] {\n\
        \x20   Effect.pure(())\n\
        \x20 }\n\
        }\n";
    assert_eq!(codes("clean", src), Vec::<String>::new());
}
