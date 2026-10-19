//! #2138 (end-to-end testing track §3.5): `bynk test --against dev` runs the
//! `system` suites against a local `bynk dev` it starts itself — local
//! `wrangler dev`/workerd only, nothing reaches Cloudflare.
//!
//! The project is the `2134_suite_env` fixture: its `system` cases run with
//! two distinct `env` maps (the suite's, and one case's override), so the run
//! starts one `bynk dev` per map, each serving that map's variables and the
//! secret the Bearer actor signs with. Afterwards no `bynk dev` from the run
//! is left: each one's readiness file is gone, and nothing in its process
//! group survives.
//!
//! Skipped without `node` and `wrangler`/`npx`, or without the sibling
//! `bynkc` binary; a non-empty `BYNK_REQUIRE_WORKERD` turns the skip into a
//! failure.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn skip(reason: &str) -> bool {
    eprintln!("skipping: {reason}");
    if std::env::var("BYNK_REQUIRE_WORKERD").is_ok_and(|v| !v.is_empty()) {
        panic!("BYNK_REQUIRE_WORKERD is set but {reason}");
    }
    true
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "expected" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            copy_dir(&path, &to.join(&name));
        } else {
            std::fs::copy(&path, to.join(&name)).unwrap();
        }
    }
}

#[test]
fn system_suites_run_against_a_local_bynk_dev_one_per_env() {
    let bynk = PathBuf::from(env!("CARGO_BIN_EXE_bynk"));
    let bynkc = bynk.with_file_name("bynkc");
    if (!have("node") || !(have("wrangler") || have("npx")))
        && skip("`node`, and `wrangler` or `npx`, are not available")
    {
        return;
    }
    if !bynkc.exists() && skip("the `bynkc` binary is not built beside `bynk`") {
        return;
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bynkc/tests/fixtures/positive/2134_suite_env");
    let dir = std::env::temp_dir().join(format!("bynk-against-dev-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dir.join("proj");
    copy_dir(&fixture, &proj);
    let out = dir.join("run").join("out");

    let o = Command::new(&bynk)
        .args(["test"])
        .arg(&proj)
        .arg("--output")
        .arg(&out)
        .args(["--format", "json", "--against", "dev", "--allow-skew"])
        .env("BYNK_BYNKC", &bynkc)
        .output()
        .expect("run bynk test --against dev");
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stderr = String::from_utf8_lossy(&o.stderr);
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("not a JSON document ({e})\nstdout:\n{stdout}\nstderr:\n{stderr}")
    });
    assert!(
        o.status.success(),
        "the run passes\n{doc:#}\nstderr:\n{stderr}"
    );
    assert_eq!(doc["transport"], "remote:dev", "{doc}");
    // Only the four `system` cases run, across the two `env` groups.
    assert_eq!(doc["passed"], 4, "{doc:#}");
    assert_eq!(doc["failed"], 0, "{doc:#}");

    // Every `bynk dev` the run started is gone.
    for i in 0..2 {
        assert!(
            !out.join(format!(".bynk-dev-ready-{i}.json")).exists(),
            "the readiness file of `bynk dev` {i} is removed"
        );
    }
    let leftover = Command::new("ps")
        .args(["-eo", "args"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    assert!(
        !leftover
            .lines()
            .any(|l| l.contains(&*proj.to_string_lossy()) && l.contains("dev")),
        "no `bynk dev` for the project is left running:\n{leftover}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
