//! I4 (#1970, `design/archive/retired-tracks.md`, interop track D3, Q6): `bynk deploy` holds a
//! build to each declared foreign caller's pin (`bynk.foreign.toml`).
//!
//! Driven through `bynk deploy --dry-run`, which compiles, checks and prints
//! the plan without touching Cloudflare, over a project whose context declares
//! two foreign callers. No pin is reported with the lines to add; a matching
//! pin passes silently; a contract change that breaks a pin is refused before
//! the plan, and updating the pin is the override.

use std::path::{Path, PathBuf};
use std::process::Command;

const ORDERS_V1: &str = r#"context shop.orders

actor Legacy { auth = Foreign(secret = "LEGACY_CALL_KEY") }
actor Ledger { auth = Foreign(secret = "LEDGER_CALL_KEY") }

service status {
  on call(id: String) -> Effect[Result[String, String]] by who: Legacy | Ledger {
    match who {
      Legacy => Ok("\(id) for legacy")
      Ledger => Ok("\(id) for the ledger")
    }
  }
}

service ping {
  on call(n: Int) -> Effect[Int] by Legacy {
    n
  }
}
"#;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/shop")).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    dir
}

/// A Worker's contract manifest as the project builds now. A dry run leaves no
/// build behind (#3796), so this builds the Workers into `out/` itself.
fn built_manifest(dir: &Path, worker: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args(["build", "--target", "workers"])
        .current_dir(dir)
        .output()
        .expect("run bynk build");
    assert!(
        out.status.success(),
        "the Workers build:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read_to_string(
        dir.join("out/workers")
            .join(worker)
            .join("bynk-contracts.json"),
    )
    .expect("the Worker records its contracts")
}

fn on_path(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `bynk deploy --dry-run` in `dir`: (success, stderr).
fn dry_run(dir: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args(["deploy", "--dry-run"])
        .current_dir(dir)
        .env_remove("CI")
        .output()
        .expect("run bynk");
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// The hash `service`'s line carries in a report or a client.
fn hash_after(text: &str, line_start: &str) -> String {
    text.lines()
        .find_map(|l| l.trim().strip_prefix(line_start))
        .and_then(|rest| rest.trim().strip_prefix('"'))
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("no `{line_start}` line in:\n{text}"))
        .to_string()
}

#[test]
fn deploy_holds_a_build_to_each_foreign_callers_pin() {
    // `deploy`'s preflight needs a way to run wrangler, though a dry run never
    // calls it.
    if !on_path("wrangler") && !on_path("npx") {
        eprintln!("\n!!! FOREIGN PINS SKIPPED !!!\nneither `wrangler` nor `npx` is on PATH\n");
        return;
    }
    let dir = scratch("foreign-pins");
    std::fs::write(dir.join("src/shop/orders.bynk"), ORDERS_V1).unwrap();

    // No pin: reported, with the lines to add, and not refused.
    let (ok, out) = dry_run(&dir);
    assert!(
        ok,
        "an unpinned foreign caller must not refuse a deploy:\n{out}"
    );
    assert!(out.contains("no pin in bynk.foreign.toml"), "{out}");
    assert!(out.contains("[\"shop.orders\".Legacy]"), "{out}");
    let ping = hash_after(&out, "ping =");
    let status = hash_after(&out, "status =");

    // Pinned as reported: silent.
    let pins = format!(
        "[\"shop.orders\".Legacy]\nping = \"{ping}\"\nstatus = \"{status}\"\n\n[\"shop.orders\".Ledger]\nstatus = \"{status}\"\n"
    );
    std::fs::write(dir.join("bynk.foreign.toml"), &pins).unwrap();
    let (ok, out) = dry_run(&dir);
    assert!(ok, "matching pins must pass:\n{out}");
    assert!(!out.contains("no pin"), "{out}");

    // A signature change breaks both callers' pins on `status`: refused,
    // before the plan.
    std::fs::write(
        dir.join("src/shop/orders.bynk"),
        ORDERS_V1.replace("on call(id: String)", "on call(id: Int)"),
    )
    .unwrap();
    let (ok, out) = dry_run(&dir);
    assert!(!ok, "a build that breaks a pin must be refused:\n{out}");
    assert!(out.contains("bynk.deploy.foreign_contract_skew"), "{out}");
    assert!(out.contains("shop.orders.status as `Legacy`"), "{out}");
    assert!(out.contains("shop.orders.status as `Ledger`"), "{out}");
    assert!(
        !out.contains("deploy shop-orders"),
        "refused before the plan:\n{out}"
    );

    // Updating the pin is the override.
    let generated = built_manifest(&dir, "shop-orders");
    let new_status = generated
        .lines()
        .find_map(|l| l.trim().strip_prefix("\"status\": \""))
        .map(|rest| rest.trim_end_matches(['"', ',']).to_string())
        .expect("the build records `status`");
    assert_ne!(new_status, status);
    std::fs::write(
        dir.join("bynk.foreign.toml"),
        pins.replace(
            &format!("status = \"{status}\""),
            &format!("status = \"{new_status}\""),
        ),
    )
    .unwrap();
    let (ok, out) = dry_run(&dir);
    assert!(ok, "an updated pin must pass:\n{out}");

    // A caller the context stops declaring breaks its pins.
    std::fs::write(
        dir.join("src/shop/orders.bynk"),
        ORDERS_V1
            .replace("on call(id: String)", "on call(id: Int)")
            .replace("by Legacy {", "by Caller {"),
    )
    .unwrap();
    let (ok, out) = dry_run(&dir);
    assert!(!ok, "{out}");
    assert!(out.contains("shop.orders.ping as `Legacy`"), "{out}");
    assert!(out.contains("the caller may no longer call it"), "{out}");

    // A malformed pin file is an error, named with the file.
    std::fs::write(
        dir.join("bynk.foreign.toml"),
        "[\"shop.orders\".Legacy]\nping = 1\n",
    )
    .unwrap();
    let (ok, out) = dry_run(&dir);
    assert!(!ok && out.contains("bynk.foreign.toml"), "{out}");
}
