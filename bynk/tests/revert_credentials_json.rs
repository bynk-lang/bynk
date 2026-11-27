//! #3797: `bynk revert --format json` writes a structured `bynk.cloudflare.credentials_missing`
//! error when the Cloudflare credentials are not set, beside the `bynk: error:` line on stderr.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn project(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("revert_credentials_json")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        "[project]\nname = \"plain\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("bynk.deploy.lock"),
        "version = 1\n\n[environments.default.workers.shop-orders]\ndeployed = true\ncontexts = [\"shop.orders\"]\nscript = \"shop-orders\"\n",
    )
    .unwrap();
    dir
}

fn revert(dir: &PathBuf, token: bool, account: bool, extra: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk"));
    cmd.args(["revert", "--dry-run"])
        .args(extra)
        .arg(dir)
        .env("BYNK_NO_INSTALL", "1")
        .env_remove("CLOUDFLARE_API_TOKEN")
        .env_remove("CLOUDFLARE_ACCOUNT_ID");
    if token {
        cmd.env("CLOUDFLARE_API_TOKEN", "not-a-real-token");
    }
    if account {
        cmd.env("CLOUDFLARE_ACCOUNT_ID", "not-a-real-account");
    }
    cmd.output().expect("runs bynk")
}

#[test]
fn missing_credentials_are_a_structured_error_under_json() {
    let dir = project("both");
    let out = revert(&dir, false, false, &["--format", "json"]);
    assert!(!out.status.success());
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    assert_eq!(doc["error"]["code"], "bynk.cloudflare.credentials_missing");
    assert_eq!(
        doc["error"]["missing"],
        serde_json::json!(["CLOUDFLARE_API_TOKEN", "CLOUDFLARE_ACCOUNT_ID"])
    );
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(message.contains("CLOUDFLARE_API_TOKEN"), "{message}");
    // The same message is on stderr, as it always was.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(message), "{stderr}");
}

#[test]
fn only_the_unset_variable_is_named() {
    let dir = project("account_only");
    let out = revert(&dir, true, false, &["--format", "json"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        doc["error"]["missing"],
        serde_json::json!(["CLOUDFLARE_ACCOUNT_ID"])
    );
}

#[test]
fn the_short_format_prints_no_document() {
    let dir = project("short");
    let out = revert(&dir, false, false, &[]);
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}
