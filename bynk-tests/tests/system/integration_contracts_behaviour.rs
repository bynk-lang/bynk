//! Regression for `bynkc test`'s second, workers-target compile silently
//! dropping `CompileOptions` fields the first compile carefully built.
//!
//! `run_test` compiles once (bundle target, `contracts: true`) and, when any
//! suite is an integration suite (drives real cross-context Workers), compiles
//! again at `BuildTarget::Workers` and writes everything outside `tests/` from
//! that second compile — the integration case imports the workers-mode module,
//! not the bundle one. If that second compile rebuilds `CompileOptions` from
//! scratch instead of cloning the first, it silently loses `contracts: true`
//! (and, under `--inspect`, `import_ext(Ts)`).
//!
//! This drives the real CLI with `--no-run` (a pure compile-and-write step, no
//! `tsc`/`node` dependency) and inspects the written workers-mode module for
//! the contract's call-site guard, rather than executing the integration
//! suite's own cross-context RPC — which needs a real Workers-ish runtime and
//! is orthogonal to what this regression is about.

use std::path::PathBuf;
use std::process::Command;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behaviour/integration_contracts")
}

#[test]
fn contracts_survive_the_integration_workers_recompile() {
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("integration-contracts-out")
        .join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(fixture())
        .arg("--output")
        .arg(&out_root)
        .arg("--no-run")
        .output()
        .expect("run bynkc test --no-run");
    assert!(
        out.status.success(),
        "bynkc test --no-run should succeed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // `shop.payment`'s `provides Bank` is not under `tests/`, so the
    // integration re-compile (`BuildTarget::Workers`) is what writes it —
    // this is the exact module the bug's from-scratch `CompileOptions`
    // rebuild would have compiled with `contracts: false`.
    let handlers = out_root
        .join("workers")
        .join("shop-payment")
        .join("handlers.ts");
    let ts = std::fs::read_to_string(&handlers)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", handlers.display()));
    // (Backtick-quoted in the emitted JS template literal, so checked as
    // separate substrings rather than one literal with escaped backticks.)
    assert!(
        ts.contains("BynkContractError") && ts.contains("postcondition") && ts.contains("halved"),
        "expected `rate`'s call-site guard in the workers-mode compile of \
         shop.payment (contracts must survive the integration recompile), got:\n{ts}"
    );
}
