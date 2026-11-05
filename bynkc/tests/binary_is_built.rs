//! Keeps the `bynkc` binary built in every `cargo test --workspace` run.
//!
//! Cargo builds a package's binaries only when one of its integration tests
//! (or a dependent's) needs them. The big corpus moved to `bynk-tests`, which
//! drives a forwarding binary instead, so nothing would build `bynkc` itself.
//! `bynk`'s tests (`crypto_ed25519_cross_check`, `packages`, `registry`, …) find
//! it as a sibling of the `bynk` binary and fail (or skip) without it. This
//! test needs `CARGO_BIN_EXE_bynkc`, so cargo builds the binary, and it checks
//! the binary runs.

use std::process::Command;

#[test]
fn bynkc_binary_runs() {
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("--version")
        .output()
        .expect("run bynkc");
    assert!(out.status.success(), "bynkc --version failed: {out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("bynkc"),
        "unexpected --version output: {out:?}"
    );
}
