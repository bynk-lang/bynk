//! #4555: every Wrangler a `bynk` crate test starts has its usage telemetry off, so a local run
//! never tries to reach Cloudflare's telemetry hosts. `bynk-tests` asserts the same of its own
//! harness (`system/wrangler_prewarm.rs`); both use the one `TELEMETRY_OFF` constant.
//!
//! The tests that start a Wrangler (`bynk dev`, `bynk test --against dev`, or one directly) build
//! their commands with `dev_child::{bynk_command, tool_command}`. The first test checks those
//! builders; the second keeps a spawn from bypassing them.

#![cfg(unix)]

#[path = "support/dev_child.rs"]
mod dev_child;

fn telemetry_setting(cmd: &std::process::Command) -> Option<String> {
    cmd.get_envs()
        .find(|(key, _)| *key == "WRANGLER_SEND_METRICS")
        .and_then(|(_, value)| value.map(|v| v.to_string_lossy().into_owned()))
}

/// The builders carry `WRANGLER_SEND_METRICS=false`: the `bynk` binary (whose `dev` starts a
/// Wrangler that inherits it), a `wrangler`, an `npx`, and the `--version` probe of either.
#[test]
fn every_wrangler_a_bynk_test_starts_has_telemetry_off() {
    for (what, cmd) in [
        ("the `bynk` binary", dev_child::bynk_command()),
        ("a `wrangler`", dev_child::tool_command("wrangler")),
        ("an `npx`", dev_child::tool_command("npx")),
    ] {
        assert_eq!(
            telemetry_setting(&cmd).as_deref(),
            Some("false"),
            "{what} must start with Wrangler telemetry off"
        );
    }
}

/// No test file builds a `bynk dev` / `bynk test --against dev` / Wrangler command by hand: each
/// goes through the builders above, which set the variable.
#[test]
fn no_bynk_test_spawns_a_wrangler_without_the_builders() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut bad = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read bynk/tests") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|e| e != "rs") || path.ends_with("wrangler_telemetry_off.rs")
        {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read a test file");
        let starts_wrangler = src.contains("\"dev\"")
            || src.contains("\"--against\"")
            || src.contains("Command::new(\"wrangler\")");
        if !starts_wrangler {
            continue;
        }
        for (n, line) in src.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if line.contains("Command::new(env!(\"CARGO_BIN_EXE_bynk\"))")
                || line.contains("Command::new(\"wrangler\")")
                || line.contains("Command::new(\"npx\")")
            {
                bad.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "start these through dev_child::bynk_command / tool_command (#4555):\n{}",
        bad.join("\n")
    );
}
