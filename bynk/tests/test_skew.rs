//! #1675: `bynk test`, and `bynk check`/`bynk fmt` under a `BYNK_BYNKC`
//! override, act on the driver↔compiler skew they compute, end to end — a real
//! `bynk` run against a fake `bynkc` (via `BYNK_BYNKC`) that reports a chosen
//! version and echoes its arguments when it is run. The `dev`/`deploy` compile
//! is covered by `compile_once_warnings_behaviour.rs`.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Output};

/// A fake `bynkc` reporting `version` for `--version`, and printing a marker
/// (proof it was delegated to) for anything else.
fn fake_bynkc(name: &str, version: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("test-skew")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bynkc");
    write_stand_in(
        &path,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"bynkc {version}\"; exit 0; fi\necho \"FAKE BYNKC RAN: $*\"\n"
        ),
    );
    path
}

/// Write an executable stand-in at `path` through a child `sh`, never by
/// this process (#2064). A test thread that forks while this process holds
/// the script open for writing carries the descriptor into its child until
/// exec, and running the script in that window fails with ETXTBSY ("Text file
/// busy").
fn write_stand_in(path: &std::path::Path, script: &str) {
    let wrote = Command::new("sh")
        .arg("-c")
        .arg("printf '%s' \"$1\" > \"$2\" && chmod 755 \"$2\"")
        .arg("sh")
        .arg(script)
        .arg(path)
        .status()
        .unwrap();
    assert!(wrote.success(), "the stand-in is written");
}

fn bynk_test(bynkc: &PathBuf, extra: &[&str], allow_env: bool) -> Output {
    bynk(bynkc, "test", extra, allow_env)
}

fn bynk(bynkc: &PathBuf, command: &str, extra: &[&str], allow_env: bool) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk"));
    cmd.arg(command)
        .arg(".")
        .args(extra)
        .env("BYNK_BYNKC", bynkc);
    if allow_env {
        cmd.env("BYNK_ALLOW_SKEW", "1");
    } else {
        cmd.env_remove("BYNK_ALLOW_SKEW");
    }
    cmd.output().expect("run bynk test")
}

/// The driver's own version, as `MAJOR.MINOR.PATCH`.
fn driver() -> (u32, u32) {
    let v = env!("CARGO_PKG_VERSION");
    let mut it = v.split('.').map(|p| p.parse::<u32>().unwrap());
    (it.next().unwrap(), it.next().unwrap())
}

/// #2998: `bynk test` runs in-process, so a skewed `bynkc` named by
/// `BYNK_BYNKC` is neither consulted nor run, and nothing about it is reported.
#[test]
fn test_never_runs_or_gates_on_a_second_compiler() {
    let (major, _) = driver();
    let bynkc = fake_bynkc("major-test", &format!("{}.0.0", major + 1));
    let out = bynk_test(&bynkc, &["--no-run", "--format", "json"], false);
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !stdout.contains("FAKE BYNKC RAN") && !stderr.contains("major version"),
        "stdout: {stdout}\nstderr: {stderr}"
    );
}

/// `check` and `fmt` shell the override's `bynkc` too, so they are gated the
/// same way — but neither takes `--allow-skew`, so the refusal offers only the
/// variable, and the variable alone lets them run.
#[test]
fn overridden_check_and_fmt_are_gated() {
    let (major, _) = driver();
    let bynkc = fake_bynkc("major-check-fmt", &format!("{}.0.0", major + 1));
    for command in ["check", "fmt"] {
        let out = bynk(&bynkc, command, &[], false);
        let (stdout, stderr) = (
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(!out.status.success(), "{command}: a major skew fails");
        assert!(
            stderr.contains(&format!("bynk {command}:")) && stderr.contains("Refusing"),
            "{command}: explains: {stderr}"
        );
        assert!(
            !stderr.contains("--allow-skew"),
            "{command}: never advises a flag it rejects: {stderr}"
        );
        assert!(
            !stdout.contains("FAKE BYNKC RAN"),
            "{command}: never delegates: {stdout}"
        );

        let out = bynk(&bynkc, command, &[], true);
        assert!(
            String::from_utf8_lossy(&out.stdout).contains(&format!("FAKE BYNKC RAN: {command}")),
            "{command}: runs when allowed"
        );
    }
}
