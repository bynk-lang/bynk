//! The `bynk-emit` README's usage example: compile a project from in-memory sources through the
//! driver and read the emitted tree.
//!
//! 1B.2b (#4028): it was a doctest of `bynk-emit`, which needed a dev-dependency on `bynk-driver`.
//! It runs here instead, and `the_example_matches_the_readme` keeps the README's block and this
//! body from drifting apart.

// Verbatim from the README, which rustfmt would otherwise reflow.
#[rustfmt::skip]
#[test]
fn the_readme_example_compiles_and_emits() {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use bynk_driver::project::{
        BuildTarget, CompileOptions, ProjectFailure, ProjectOutput, compile_project,
    };

    // `sources` maps every `.bynk` file under `root` to its text. bynk-emit never
    // touches disk: without `.sources(...)` there is nothing to compile.
    fn build(root: PathBuf, sources: HashMap<PathBuf, String>) -> Result<ProjectOutput, ProjectFailure> {
        let options = CompileOptions::single(root).sources(sources).target(BuildTarget::Workers);
        compile_project(&options) // an in-memory TypeScript tree
    }

    let root = PathBuf::from("src");
    let sources = HashMap::from([(
        root.join("commerce/orders.bynk"),
        "context commerce.orders {}".to_string(),
    )]);
    let output = build(root, sources).map_err(ProjectFailure::flatten).unwrap();
    assert!(!output.artefacts.docs.is_empty());
}

/// Every code line of the README's `rust,ignore` block is a line of the example above, so the
/// README cannot show code that this test does not run.
#[test]
fn the_example_matches_the_readme() {
    let readme = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../bynk-emit/README.md"),
    )
    .expect("read bynk-emit/README.md");
    let block = readme
        .split("```rust,ignore\n")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("the README holds a `rust,ignore` example");
    // Only the body of the example test counts: a line elsewhere in this file (this test's own
    // asserts, say) must not satisfy a README line.
    let this = include_str!("emit_readme_example.rs");
    let start = this
        .find("fn the_readme_example_compiles_and_emits() {")
        .expect("the example test is in this file");
    let body = &this[start..];
    let body = &body[..body.find("\n}\n").expect("the example test closes")];
    let ours: std::collections::HashSet<&str> = body.lines().map(str::trim).collect();
    for line in block.lines().map(str::trim).filter(|l| !l.is_empty()) {
        assert!(
            ours.contains(line),
            "the README example has a line this test does not run: {line:?}"
        );
    }
}
