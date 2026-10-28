//! #2554: `bynk deploy --context <callee> --accept-compatible-skew` deploys a
//! callee ahead of live callers still on an older, compatible contract.
//!
//! Driven through the real binary over the two-context project the other
//! deploy tests use. `--dry-run` compiles and checks without touching
//! Cloudflare; the live state is a `bynk.deploy.lock` seeded from the first
//! build's own manifests, as a real deploy would have recorded it.

use std::path::{Path, PathBuf};
use std::process::Command;

const PAYMENT_V1: &str = r#"context shop.payment

exports transparent { Receipt }

type Receipt = { id: String, extra: Option[String] }

service authorise {
  on call(id: String) -> Effect[Result[Receipt, Int]] {
    Ok(Receipt { id: id, extra: None })
  }
}
"#;

/// The receipt gains an optional field: compatible for a caller on V1.
const PAYMENT_V2: &str = r#"context shop.payment

exports transparent { Receipt }

type Receipt = { id: String, extra: Option[String], note: Option[String] }

service authorise {
  on call(id: String) -> Effect[Result[Receipt, Int]] {
    Ok(Receipt { id: id, extra: None, note: None })
  }
}
"#;

/// A field the caller decodes changes type: breaking for a caller on V1. The
/// caller never reads `extra`, so it still compiles.
const PAYMENT_BREAKING: &str = r#"context shop.payment

exports transparent { Receipt }

type Receipt = { id: String, extra: Option[Int] }

service authorise {
  on call(id: String) -> Effect[Result[Receipt, Int]] {
    Ok(Receipt { id: id, extra: None })
  }
}
"#;

const ORDERS: &str = r#"context shop.orders

consumes shop.payment

service place {
  on call(id: String) -> Effect[Result[String, Int]] {
    let receipt <- shop.payment.authorise(id)
    match receipt {
      Ok(r) => Ok(r.id)
      Err(code) => Err(code)
    }
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
    std::fs::write(dir.join("src/shop/payment.bynk"), PAYMENT_V1).unwrap();
    std::fs::write(dir.join("src/shop/orders.bynk"), ORDERS).unwrap();
    dir
}

fn on_path(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `bynk deploy --dry-run <args>` in `dir`: (success, stderr).
fn dry_run(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args(["deploy", "--dry-run"])
        .args(args)
        .current_dir(dir)
        .env_remove("CI")
        .output()
        .expect("run bynk");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn manifest(dir: &Path, worker: &str) -> serde_json::Value {
    let path = dir
        .join(".bynk/deploy/workers")
        .join(worker)
        .join("bynk-contracts.json");
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// Write the `bynk.deploy.lock` a real deploy of the project as it stands
/// would have recorded, from the build's own manifests.
fn seed_ledger(dir: &Path) {
    let payment = manifest(dir, "shop-payment");
    let orders = manifest(dir, "shop-orders");
    let hash = payment["provides"]["authorise"].as_str().unwrap();
    let shape = payment["provides_shapes"]["authorise"].as_str().unwrap();
    assert_eq!(
        orders["expects"]["shop.payment"]["authorise"].as_str(),
        Some(hash),
        "caller and callee agree at baseline"
    );
    let ledger = format!(
        "version = 1\n\
         [environments.default.workers.shop-payment]\n\
         deployed = true\n\
         [environments.default.workers.shop-payment.contracts]\n\
         authorise = \"{hash}\"\n\
         [environments.default.workers.shop-payment.shapes]\n\
         authorise = '{shape}'\n\
         [environments.default.workers.shop-orders]\n\
         deployed = true\n\
         [environments.default.workers.shop-orders.expects.\"shop.payment\"]\n\
         authorise = \"{hash}\"\n\
         [environments.default.workers.shop-orders.expects_shapes.\"shop.payment\"]\n\
         authorise = '{shape}'\n"
    );
    std::fs::write(dir.join("bynk.deploy.lock"), ledger).unwrap();
}

#[test]
fn a_compatible_callee_ahead_of_its_callers_needs_the_flag_and_a_breaking_one_is_never_let_through()
{
    if !on_path("wrangler") && !on_path("npx") {
        eprintln!(
            "\n!!! ACCEPT COMPATIBLE SKEW SKIPPED !!!\nneither `wrangler` nor `npx` is on PATH\n"
        );
        return;
    }
    let dir = scratch("accept-compatible-skew");
    // Baseline: both sides at V1, as deployed.
    let (ok, out) = dry_run(&dir, &[]);
    assert!(ok, "the baseline plans:\n{out}");
    seed_ledger(&dir);

    // Unchanged callee: no skew, nothing to accept.
    let (ok, out) = dry_run(&dir, &["--context", "shop.payment"]);
    assert!(ok, "an unchanged callee deploys:\n{out}");

    // The callee gains an optional response field.
    std::fs::write(dir.join("src/shop/payment.bynk"), PAYMENT_V2).unwrap();

    // Without the flag it is refused, and the refusal says it is compatible
    // and what would accept it.
    let (ok, out) = dry_run(&dir, &["--context", "shop.payment"]);
    assert!(!ok, "refused without the flag:\n{out}");
    assert!(out.contains("bynk.deploy.contract_skew"), "{out}");
    assert!(out.contains("compatible response"), "{out}");
    assert!(out.contains("--accept-compatible-skew"), "{out}");

    // With it, a dry run reports what it would record and writes nothing.
    let (ok, out) = dry_run(
        &dir,
        &["--context", "shop.payment", "--accept-compatible-skew"],
    );
    assert!(ok, "accepted with the flag:\n{out}");
    assert!(
        out.contains("will also answer 1 live caller contract"),
        "{out}"
    );
    assert!(out.contains("Nothing was written"), "{out}");
    assert!(
        !dir.join("bynk.contracts.lock").exists(),
        "a dry run records nothing"
    );

    // A breaking change is refused even with the flag.
    std::fs::write(dir.join("src/shop/payment.bynk"), PAYMENT_BREAKING).unwrap();
    let (ok, out) = dry_run(
        &dir,
        &["--context", "shop.payment", "--accept-compatible-skew"],
    );
    assert!(!ok, "a breaking change is never accepted:\n{out}");
    assert!(out.contains("breaking response"), "{out}");
    assert!(!out.contains("will also answer"), "{out}");
    assert!(!dir.join("bynk.contracts.lock").exists());
}

/// A `wrangler`/`npx` that reports a version and fails everything else, so a
/// real run passes its preflight and builds, and stops at the first upload
/// ("Cloudflare authentication is unavailable"): everything the gate does has
/// then been done.
#[cfg(unix)]
fn fake_wrangler(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("fakebin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = "#!/bin/sh\ncase \"$1$2\" in\n  --version|wrangler--version) echo \"4.50.0\";;\n  *) echo \"fake wrangler: $*\" >&2; exit 1;;\nesac\n";
    for name in ["wrangler", "npx"] {
        let path = bin.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

#[cfg(unix)]
#[test]
fn a_real_run_records_the_older_contract_and_rebuilds_the_callee_to_carry_it() {
    if !on_path("node") {
        eprintln!("\n!!! ACCEPT COMPATIBLE SKEW (REAL RUN) SKIPPED !!!\n`node` is not on PATH\n");
        return;
    }
    let dir = scratch("accept-compatible-skew-real");
    let bin = fake_wrangler(&dir);
    let (ok, out) = dry_run(&dir, &[]);
    assert!(ok, "the baseline plans:\n{out}");
    seed_ledger(&dir);
    let v1_hash = manifest(&dir, "shop-payment")["provides"]["authorise"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::write(dir.join("src/shop/payment.bynk"), PAYMENT_V2).unwrap();

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = |extra: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
            .args(["deploy", "--yes", "--context", "shop.payment"])
            .args(extra)
            .current_dir(&dir)
            .env("PATH", &path)
            .env_remove("CI")
            .output()
            .expect("run bynk");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };

    // Without the flag: refused, and nothing is recorded.
    let err = run(&[]);
    assert!(err.contains("bynk.deploy.contract_skew"), "{err}");
    assert!(!dir.join("bynk.contracts.lock").exists());

    // With it: the older contract is recorded, the callee rebuilt to carry it,
    // and the run goes on to the upload (which the fake refuses).
    let err = run(&["--accept-compatible-skew"]);
    assert!(err.contains("Recorded in bynk.contracts.lock"), "{err}");
    assert!(!err.contains("bynk.deploy.contract_skew"), "{err}");
    let lock = std::fs::read_to_string(dir.join("bynk.contracts.lock")).unwrap();
    assert!(lock.contains("[accepts.\"shop.payment\"]"), "{lock}");
    let rebuilt = manifest(&dir, "shop-payment");
    assert_eq!(
        rebuilt["accepts"]["authorise"][0].as_str(),
        Some(v1_hash.as_str()),
        "the rebuilt callee carries the caller's hash: {rebuilt}"
    );
    let index =
        std::fs::read_to_string(dir.join(".bynk/deploy/workers/shop-payment/index.ts")).unwrap();
    assert!(
        index.contains(&format!("&& __contract !== \"{v1_hash}\"")),
        "{index}"
    );

    // Again: nothing new to record, and the callee is still accepted.
    let err = run(&["--accept-compatible-skew"]);
    assert!(
        err.contains("will also answer 1 live caller contract"),
        "{err}"
    );
    assert!(!err.contains("Recorded in bynk.contracts.lock"), "{err}");
    assert_eq!(
        std::fs::read_to_string(dir.join("bynk.contracts.lock")).unwrap(),
        lock,
        "the lock is not rewritten"
    );
}
