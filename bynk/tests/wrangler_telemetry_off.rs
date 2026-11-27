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

/// Programs a `bynk` test may start directly: none of them is, or starts, a Wrangler.
const OTHER_PROGRAMS: [&str; 3] = ["node", "ps", "sh"];

/// Files that name `"dev"` or `"--against"` and run the `bynk` binary without starting a Wrangler,
/// each with why: they need not include `dev_child`.
const START_NO_WRANGLER: [(&str, &str); 5] = [
    (
        "each.rs",
        "`test --each --against dev` is refused before anything starts",
    ),
    (
        "workspace.rs",
        "`bynk dev` at a workspace root is refused before anything starts",
    ),
    ("packages_git.rs", "`dev` is a scratch directory name"),
    ("driver_commands.rs", "`dev` is a build profile"),
    ("registry_query.rs", "`dev` is a dependency-kind JSON key"),
];

/// A test file that runs the `bynk` binary with `dev` or `--against` includes `dev_child`; in every
/// file that includes it, every
/// `Command::new(` is one of [`OTHER_PROGRAMS`], and `CARGO_BIN_EXE_bynk` is never named: the binary
/// and the tools come from `dev_child::{bynk_command, tool_command}`, which set the variable. So a
/// bypass through a local variable (`Command::new(&bynk)`, `Command::new(tool)`) fails here too.
#[test]
fn no_bynk_test_spawns_a_wrangler_without_the_builders() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut bad = Vec::new();
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("read bynk/tests") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|e| e != "rs") || path.ends_with("wrangler_telemetry_off.rs")
        {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read a test file");
        // A file that runs the `bynk` binary with `dev` or `--against` starts a Wrangler: it
        // includes `dev_child` for the builders.
        let runs_bynk_dev = src.contains("CARGO_BIN_EXE_bynk")
            && (src.contains("\"dev\"") || src.contains("\"--against\""));
        let uses_builders = src.contains("support/dev_child.rs");
        let exempt = START_NO_WRANGLER
            .iter()
            .any(|(name, _)| path.file_name().is_some_and(|n| n == *name));
        if runs_bynk_dev && !uses_builders && !exempt {
            bad.push(format!(
                "{}: runs `bynk dev` / `--against` but does not include support/dev_child.rs",
                path.display()
            ));
        }
        if !uses_builders {
            continue;
        }
        checked += 1;
        for (n, line) in src.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let spawns_other = OTHER_PROGRAMS
                .iter()
                .any(|p| line.contains(&format!("Command::new(\"{p}\")")));
            if (line.contains("Command::new(") && !spawns_other)
                || line.contains("CARGO_BIN_EXE_bynk")
            {
                bad.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(checked >= 6, "the guard selected only {checked} files");
    assert!(
        bad.is_empty(),
        "start these through dev_child::bynk_command / tool_command (#4555):\n{}",
        bad.join("\n")
    );
}
