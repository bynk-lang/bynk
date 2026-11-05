//! Slice 2 (ADR 0104) end-to-end proof: a breakpoint set in a `.bynk` source
//! binds and pauses when the emitted test entry runs under Node's inspector.
//!
//! This is the productised slice-2 spike. It compiles a debug build (`.ts` import
//! specifiers + source maps), then drives a headless Chrome-DevTools-Protocol
//! session — exactly what `vscode-js-debug`/`pwa-node` does — to confirm the map
//! round-trip on a real `node --inspect-brk` process. The CDP client is a small
//! Node script (Node ships a global `WebSocket`), invoked from this test; the
//! test is skipped when Node is absent or too old for `.ts` type-stripping
//! (≥ 22.6), so it never fails CI on an old toolchain.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

/// Parse `node --version` (`vMAJOR.MINOR.PATCH`) into `(major, minor)`, or `None`
/// if Node is missing/unparseable.
fn node_version() -> Option<(u32, u32)> {
    let out = Command::new("node").arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout);
    let v = v.trim().trim_start_matches('v');
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Type-stripping of a `.ts` entry is unflagged from 23.6 and flagged
/// (`--experimental-strip-types`) from 22.6.
fn node_can_strip_types(v: (u32, u32)) -> bool {
    v.0 > 22 || (v.0 == 22 && v.1 >= 6)
}

/// The committed CDP harness (run with `node`).
fn harness() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("support")
        .join("debug_attach.mjs")
}

#[test]
fn breakpoint_in_bynk_binds_and_pauses_under_node_inspector() {
    let Some(v) = node_version() else {
        eprintln!("skipping: `node` not found on PATH");
        return;
    };
    if !node_can_strip_types(v) {
        eprintln!(
            "skipping: node v{}.{} < 22.6 (no .ts type-stripping)",
            v.0, v.1
        );
        return;
    }

    // A unique temp project: a commons with a multi-statement free fn, and a test
    // that calls it — so a breakpoint in the production `.bynk` is reached through
    // the test runner.
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "bynk_dbg_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let src = dir.join("src");
    let tests = dir.join("tests");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&tests).unwrap();
    std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"dbg\"\n").unwrap();
    std::fs::write(
        src.join("calc.bynk"),
        "commons calc {\n  fn dbl(n: Int) -> Int {\n    let doubled = n + n\n    let result = doubled + 0\n    result\n  }\n}\n",
    )
    .unwrap();
    std::fs::write(
        tests.join("calc.bynk"),
        "suite calc {\n  case \"doubles\" {\n    expect dbl(3) == 6\n  }\n}\n",
    )
    .unwrap();

    // Compile the debug build (`.ts` specifiers) and write it with maps — exactly
    // what `bynkc test --inspect` writes before launching Node.
    let out_root = dir.join("out");
    let opts = bynk_testkit::compile_options_split(
        dir.clone(),
        bynkc::try_read_project_paths(&dir).expect("well-formed fixture manifest"),
    )
    .import_ext(bynkc::ImportExt::Ts);
    let output = bynkc::compile_project(&opts)
        .map_err(bynkc::ProjectFailure::flatten)
        .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
    bynkc::write_output(&output, &out_root).unwrap();

    // The harness launches `node --inspect-brk out/tests/main.ts`, sets a
    // breakpoint at calc.bynk:3 (mapped through calc.ts.map), and asserts it binds
    // and pauses there. Args: <out_root> <calc.ts.map> <source_basename> <bynk_line>.
    let map = out_root.join("calc.ts.map");
    assert!(map.exists(), "debug build must emit calc.ts.map");
    let entry = out_root.join("tests").join("main.ts");

    let result = Command::new("node")
        .arg(harness())
        .arg(&entry)
        .arg(&map)
        .arg("3") // user breakpoint: calc.bynk:3 (`let doubled = n + n`)
        .output()
        .expect("run the CDP harness");

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        result.status.success(),
        "breakpoint did not bind/pause as expected\n--- harness stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("BIND OK"),
        "harness did not confirm bind\n{stdout}"
    );
}
