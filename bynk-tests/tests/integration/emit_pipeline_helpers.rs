//! The helpers' own tests: a project with a suite emits its suite module, a Workers build emits
//! one directory per context, and a project that fails to compile panics with its diagnostics.
//!
//! 1B.2b (#4028): moved with the helpers from `bynk-emit`'s `testkit`.

#[path = "../support/emit_pipeline.rs"]
mod emit_pipeline;
use emit_pipeline::{emit_project, emit_workers};

const MATH: &str = "commons demo.math\n\nfn double(n: Int) -> Int { n * 2 }\n";

#[test]
fn a_workers_build_emits_one_worker_directory_per_context() {
    let shop = "context demo.shop\n\nservice api {\n  on call(n: Int) -> Effect[Int] by Caller {\n    Effect.pure(n)\n  }\n}\n";
    let out = emit_workers(&[("demo/shop.bynk", shop)]);
    for doc in ["index.ts", "compose.ts", "handlers.ts", "wrangler.toml"] {
        out.text(&format!("workers/demo-shop/{doc}"));
    }
}

#[test]
fn a_unit_with_a_suite_emits_the_suite_module() {
    let suite = "suite demo.math\n\ncase \"doubles\" {\n  expect double(2) == 4\n}\n";
    let out = emit_project(
        &[("demo/math.bynk", MATH), ("tests/math.bynk", suite)],
        bynk_check::project_model::BuildTarget::Bundle,
    );
    assert!(out.text("tests/demo_math.test.ts").contains("doubles"));
}

#[test]
#[should_panic(expected = "project fixture should compile")]
fn a_project_that_fails_to_compile_panics_with_its_diagnostics() {
    emit_project(
        &[(
            "demo/math.bynk",
            "commons demo.math\n\nfn f() -> Int { \"x\" }\n",
        )],
        bynk_check::project_model::BuildTarget::Bundle,
    );
}
