//! v0.132: `bynkc test` resolves a multi-file commons by emitting an aggregating
//! `out/<name>.ts` barrel (`export *` from each of the commons' files) so the test
//! module's `import * as ns from "./<name>.js"` resolves for the directory layout
//! exactly as it does for a single file. See the v0.132 proposal / ADR.
//!
//! The `positive/25{1,2}_multi_file_commons*` fixtures pin the *content* of the
//! barrel (golden snapshot) and that it type-checks under `tsc` strict. This test
//! pins the two behaviours a path-keyed golden snapshot cannot see:
//!
//! - **Dedup** — a multi-file commons imported by *two* test modules (once as the
//!   suite target, once through another unit's `uses`) is barrelled exactly once,
//!   not emitted twice at the same output path.
//! - **Test-only** — a non-test build of the same multi-file commons emits **no**
//!   barrel, so production output is untouched (DECISION B).

use std::path::Path;

fn compile(fixture: &str) -> bynkc::ProjectOutput {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/barrel")
        .join(fixture)
        .join("src");
    bynkc::compile_project(
        &bynk_testkit::compile_options_single(src)
            .target(bynkc::BuildTarget::Bundle)
            .platform(bynkc::Platform::Cloudflare),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .unwrap_or_else(|errors| {
        panic!(
            "fixture `{fixture}` failed to compile:\n{}",
            bynkc::render_project_errors(&errors)
        )
    })
}

/// Every emitted file whose output path is exactly `money.ts` — for the
/// multi-file commons `money`, that path can only be the aggregating barrel
/// (the real per-file outputs are `money/cents.ts` and `money/make.ts`).
fn money_barrels(out: &bynkc::ProjectOutput) -> Vec<&bynkc::Document> {
    out.artefacts
        .docs
        .iter()
        .filter(|(p, _)| p.as_path() == Path::new("money.ts"))
        .map(|(_, doc)| doc)
        .collect()
}

#[test]
fn barrel_emitted_once_for_a_commons_imported_by_two_test_modules() {
    let out = compile("dedup");
    let barrels = money_barrels(&out);
    assert_eq!(
        barrels.len(),
        1,
        "expected exactly one `money.ts` barrel across the two importing test \
         modules (target suite `money` + `wallet`'s `uses money`), found {}",
        barrels.len()
    );
    // It is the barrel, not a stray real output: it re-exports both files.
    let body = barrels[0].text();
    assert!(
        body.contains("export * from \"./money/cents.js\";")
            && body.contains("export * from \"./money/make.js\";"),
        "barrel should `export *` each of the commons' files, got:\n{body}"
    );
}

#[test]
fn no_barrel_in_a_non_test_build_of_the_same_multi_file_commons() {
    let out = compile("production");
    // The commons still emits per file...
    assert!(
        out.artefacts.docs.contains_key(Path::new("money/cents.ts")),
        "the multi-file commons should still emit its per-file outputs"
    );
    // ...but no aggregating barrel ships when nothing under test imports it.
    assert!(
        money_barrels(&out).is_empty(),
        "a non-test build must emit no `money.ts` barrel (barrel is test-only)"
    );
}
