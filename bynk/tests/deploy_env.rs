//! #2028: `bynk deploy --dry-run --env staging` names what a staging deploy
//! creates. The push hands Wrangler `--env staging` and an `[env.staging]`
//! block, so each Worker deploys as `<name>-staging`, binds to `-staging`
//! targets and consumes `-staging` queues; the plan, in text and JSON, says so.
//!
//! Driven through the real binary over a two-context project: `shop.orders`
//! consumes `shop.payment` and a queue. `--dry-run` compiles, checks and prints
//! the plan without touching Cloudflare.

use std::path::{Path, PathBuf};
use std::process::Command;

const PAYMENT: &str = r#"context shop.payment

exports transparent { Receipt }

type Receipt = { id: String }

service authorise {
  on call(id: String) -> Effect[Result[Receipt, Int]] {
    Ok(Receipt { id: id })
  }
}
"#;

const ORDERS: &str = r#"context shop.orders

consumes shop.payment

type Job = { id: String }

service place {
  on call(id: String) -> Effect[Result[String, Int]] {
    let receipt <- shop.payment.authorise(id)
    match receipt {
      Ok(r) => Ok(r.id)
      Err(code) => Err(code)
    }
  }
}

service intake from queue("job-intake") {
  on message(message: Job) -> Effect[QueueResult] {
    Ack
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
    std::fs::write(dir.join("src/shop/payment.bynk"), PAYMENT).unwrap();
    std::fs::write(dir.join("src/shop/orders.bynk"), ORDERS).unwrap();
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

/// `bynk deploy --dry-run <args>` in `dir`: (success, stdout, stderr).
fn dry_run(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args(["deploy", "--dry-run"])
        .args(args)
        .current_dir(dir)
        .env_remove("CI")
        .output()
        .expect("run bynk");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_staging_plan_names_the_staging_workers_bindings_and_queues() {
    // `deploy`'s preflight needs a way to run wrangler, though a dry run never
    // calls it.
    if !on_path("wrangler") && !on_path("npx") {
        eprintln!("\n!!! DEPLOY ENV SKIPPED !!!\nneither `wrangler` nor `npx` is on PATH\n");
        return;
    }
    let dir = scratch("deploy-env-staging");

    let (ok, text, err) = dry_run(&dir, &["--env", "staging"]);
    assert!(ok, "{text}{err}");
    for line in [
        "deploy shop-payment-staging\n",
        "queue create job-intake-staging\n",
        "deploy shop-orders-staging\n",
        "order shop-payment-staging → shop-orders-staging\n",
    ] {
        assert!(text.contains(line), "missing `{}`:\n{text}", line.trim());
    }
    assert!(
        !text.contains("deploy shop-orders\n"),
        "an unsuffixed Worker:\n{text}"
    );

    let (ok, json, err) = dry_run(&dir, &["--env", "staging", "--format", "json"]);
    assert!(ok, "{json}{err}");
    let plan: serde_json::Value = serde_json::from_str(&json).expect("the plan is JSON");
    assert_eq!(plan["environment"], "staging");
    assert_eq!(
        plan["order"],
        serde_json::json!(["shop-payment-staging", "shop-orders-staging"])
    );
    let orders = &plan["contexts"][1];
    assert_eq!(orders["worker"], "shop-orders-staging");
    assert_eq!(
        orders["binds_to"],
        serde_json::json!(["shop-payment-staging"])
    );
    assert_eq!(orders["queues"][0]["queue"], "job-intake-staging");

    // The default environment is unchanged: bare names throughout.
    let (ok, text, err) = dry_run(&dir, &[]);
    assert!(ok, "{text}{err}");
    assert!(text.contains("queue create job-intake\n"), "{text}");
    assert!(
        text.contains("order shop-payment → shop-orders\n"),
        "{text}"
    );
}

/// #2038: `--context` refuses a context whose binding target has never been
/// deployed, and names both Workers as the plan does: `-staging` under
/// `--env staging`, bare in the default environment. The gate runs before the
/// dry run stops, so a fresh project with no ledger reaches it.
#[test]
fn a_context_refusal_names_the_deployed_workers() {
    if !on_path("wrangler") && !on_path("npx") {
        eprintln!("\n!!! DEPLOY ENV SKIPPED !!!\nneither `wrangler` nor `npx` is on PATH\n");
        return;
    }
    let dir = scratch("deploy-env-context-refusal");

    let (ok, text, err) = dry_run(&dir, &["--env", "staging", "--context", "shop.orders"]);
    assert!(!ok, "a never-deployed target refuses:\n{text}{err}");
    assert!(
        err.contains(
            "bynk: error: `shop-orders-staging` binds to `shop-payment-staging`, which has never been deployed"
        ),
        "{err}"
    );
    // #2047: the remedy deploys the environment that needs it.
    assert!(
        err.contains("Deploy the whole project once (`bynk deploy --env staging`)"),
        "{err}"
    );

    let (ok, text, err) = dry_run(&dir, &["--context", "shop.orders"]);
    assert!(!ok, "a never-deployed target refuses:\n{text}{err}");
    assert!(
        err.contains(
            "bynk: error: `shop-orders` binds to `shop-payment`, which has never been deployed"
        ),
        "{err}"
    );
}

/// The ledger of a project whose payment is live providing `authorise` at
/// `live`, and whose orders is live — compiled against `orders_expects` when
/// given, or recorded before expectations were (`None`), as an older lock is.
fn write_live_lock(dir: &Path, live: &str, orders_expects: Option<&str>) {
    let mut lock = format!(
        "version = 1\n\
         [environments.default.workers.shop-payment]\n\
         deployed = true\n\
         [environments.default.workers.shop-payment.contracts]\n\
         authorise = \"{live}\"\n\
         [environments.default.workers.shop-orders]\n\
         deployed = true\n\
         [environments.default.workers.shop-orders.contracts]\n\
         place = \"0000000000000000\"\n"
    );
    if let Some(hash) = orders_expects {
        lock.push_str(&format!(
            "[environments.default.workers.shop-orders.expects.\"shop.payment\"]\n\
             authorise = \"{hash}\"\n"
        ));
    }
    std::fs::write(dir.join("bynk.deploy.lock"), lock).unwrap();
}

/// #2077: `--context` refuses a callee whose new build no longer provides
/// the contract a live caller was compiled against, before the plan and so
/// under `--dry-run`, naming the caller, the contract and both hashes. A
/// whole-project run, which redeploys the caller too, is not refused; and a
/// caller recorded before expectations were is warned of, not refused.
#[test]
fn a_callee_push_that_breaks_a_live_caller_is_refused() {
    if !on_path("wrangler") && !on_path("npx") {
        eprintln!("\n!!! DEPLOY ENV SKIPPED !!!\nneither `wrangler` nor `npx` is on PATH\n");
        return;
    }
    let dir = scratch("deploy-env-callee-skew");
    let stale = "ffffffffffffffff";

    // Live orders stamps a hash this build of payment no longer provides.
    write_live_lock(&dir, stale, Some(stale));
    let (ok, text, err) = dry_run(&dir, &["--context", "shop.payment"]);
    assert!(!ok, "a callee skew refuses:\n{text}{err}");
    assert!(
        err.contains(
            "bynk: error: `shop-payment` no longer provides the contract its live callers were compiled against (bynk.deploy.contract_skew):"
        ),
        "{err}"
    );
    assert!(
        err.contains(&format!(
            "`shop-orders` calls shop.payment.authorise — compiled against {stale}, this build provides "
        )),
        "{err}"
    );
    assert!(text.is_empty(), "refused before the plan:\n{text}");

    // The hash this build provides, from its manifest.
    let manifest: serde_json::Value =
        serde_json::from_str(&built_manifest(&dir, "shop-payment")).unwrap();
    let provided = manifest["provides"]["authorise"]
        .as_str()
        .expect("payment provides `authorise`")
        .to_string();
    assert!(
        err.contains(&format!("this build provides {provided}\n")),
        "{err}"
    );

    // The whole project moves orders with payment: not refused.
    let (ok, text, err) = dry_run(&dir, &[]);
    assert!(ok, "a whole-project run is not refused:\n{text}{err}");
    assert!(text.contains("redeploy shop-payment\n"), "{text}");

    // A caller still on the hash this build provides passes.
    write_live_lock(&dir, &provided, Some(&provided));
    let (ok, text, err) = dry_run(&dir, &["--context", "shop.payment"]);
    assert!(ok, "an unchanged contract passes:\n{text}{err}");
    assert!(!err.contains("warning"), "{err}");

    // An older lock, with no `expects`: loads, warns, still plans.
    write_live_lock(&dir, stale, None);
    let (ok, text, err) = dry_run(&dir, &["--context", "shop.payment"]);
    assert!(ok, "an unrecorded caller is not refused:\n{text}{err}");
    assert!(
        err.contains(
            "bynk: warning: could not check that `shop-payment` still provides what the live `shop-orders` was compiled against"
        ),
        "{err}"
    );
    assert!(text.contains("redeploy shop-payment\n"), "{text}");
}
