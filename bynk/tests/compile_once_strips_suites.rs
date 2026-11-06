//! #1821 (ADR 0147 D3): `dev::compile_once`, the build behind `bynk dev` and
//! `bynk deploy`, strips every `suite`. It used to emit integration suites into
//! the build directory's `tests/`, and type-check every suite, so a broken one
//! failed the build.

use std::path::PathBuf;

use bynk::dev::compile_once;

#[test]
fn the_dev_and_deploy_build_strips_every_suite() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("compile-once-strips-suites");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src/shop")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"s\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    // An atomic file: the context and its suite (ADR 0147 D5). The context
    // ships; the suite does not.
    std::fs::write(
        root.join("src/shop/one.bynk"),
        "context shop.one {\n  service ping {\n    on call() -> Effect[Bool] { true }\n  }\n}\n\n\
         suite shop.one {\n  case \"pings\" {\n    let ok <- ping.call()\n    expect ok\n  }\n}\n",
    )
    .unwrap();
    // A suite that does not type-check: the build never checks it.
    std::fs::write(
        root.join("tests/broken.bynk"),
        "suite shop.one {\n  case \"x\" { expect nosuch(1) }\n}\n",
    )
    .unwrap();

    let build_dir = root.join("build");
    assert!(
        compile_once(&root, &build_dir, false),
        "the build succeeds without checking the broken suite"
    );
    assert!(
        build_dir.join("workers/shop-one/handlers.ts").exists(),
        "the context is still built"
    );
    assert!(
        !build_dir.join("tests").exists(),
        "no suite reaches the build directory"
    );
}
