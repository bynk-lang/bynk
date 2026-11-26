//! #3797: `bynk deploy --dry-run --format json` always prints a document, a refused plan
//! included: the refusal is an entry in `refusals`, with the facts of its subject and the flag that
//! overrides it, and the exit status says the run could not go ahead.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("deploy_plan_json")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A project whose context `shop.orders` hosts an agent, and whose ledger says that context was
/// last pushed on the script `acme-core`: the plan would move it to `shop-orders`.
fn moved_agent_project(name: &str) -> PathBuf {
    let dir = scratch(name);
    write(
        &dir.join("bynk.toml"),
        "[project]\nname = \"moved\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n",
    );
    write(
        &dir.join("src/shop/orders.bynk"),
        "context shop.orders\n\nagent Counter {\n\tkey id: String\n\n\tstore count: Cell[Int]\n\n\ton call current() -> Effect[Int] {\n\t\tcount\n\t}\n}\n",
    );
    write(
        &dir.join("bynk.deploy.lock"),
        "version = 1\n\n[environments.default.workers.acme-core]\ndeployed = true\ncontexts = [\"shop.orders\"]\nscript = \"acme-core\"\n",
    );
    dir
}

fn deploy(dir: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args(["deploy", "--dry-run"])
        .args(extra)
        .arg(dir)
        .env("BYNK_NO_INSTALL", "1")
        .output()
        .expect("runs bynk")
}

fn stdout_json(o: &Output) -> Value {
    let text = String::from_utf8_lossy(&o.stdout);
    serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!(
            "stdout is not one JSON document ({e}):\n{text}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&o.stderr)
        )
    })
}

#[test]
fn a_refused_agent_move_is_a_json_document_with_the_refusal_in_it() {
    let dir = moved_agent_project("refused");
    let out = deploy(&dir, &["--format", "json"]);
    assert!(!out.status.success(), "a refused plan must exit non-zero");
    let doc = stdout_json(&out);
    assert_eq!(doc["environment"], "default");
    let refusals = doc["refusals"].as_array().expect("refusals is an array");
    assert_eq!(refusals.len(), 1, "{doc}");
    let r = &refusals[0];
    assert_eq!(r["code"], "bynk.deploy.agent_move_refused");
    assert_eq!(r["reason"], "not_accepted");
    assert_eq!(r["context"], "shop.orders");
    assert_eq!(r["agents"], serde_json::json!(["Counter"]));
    assert_eq!(r["from"], "acme-core");
    assert_eq!(r["to"], "shop-orders");
    assert_eq!(r["override"], "--accept-fresh-agent shop.orders");
    assert!(
        r["message"]
            .as_str()
            .unwrap()
            .contains("would move from Worker script"),
        "{r}"
    );
    assert_eq!(doc["warnings"], serde_json::json!([]));
    // What the plan knew when it stopped: the upload order.
    assert_eq!(doc["order"], serde_json::json!(["shop-orders"]));
    // The human line is still on stderr, as it always was.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bynk.deploy.agent_move_refused: context `shop.orders`"),
        "{stderr}"
    );
}

#[test]
fn an_accepted_agent_move_warns_in_the_plan_and_refuses_nothing() {
    let dir = moved_agent_project("accepted");
    let out = deploy(
        &dir,
        &["--format", "json", "--accept-fresh-agent", "shop.orders"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc = stdout_json(&out);
    assert_eq!(doc["refusals"], serde_json::json!([]));
    let warnings = doc["warnings"].as_array().expect("warnings is an array");
    assert_eq!(warnings.len(), 1, "{doc}");
    assert_eq!(warnings[0]["code"], "bynk.deploy.agent_starts_fresh");
    assert_eq!(warnings[0]["context"], "shop.orders");
    assert_eq!(warnings[0]["from"], "acme-core");
    assert!(warnings[0].get("override").is_none(), "{}", warnings[0]);
    // The plan itself is there too.
    assert!(
        doc["contexts"].as_array().is_some_and(|c| !c.is_empty()),
        "{doc}"
    );
}

#[test]
fn a_name_that_matches_no_move_is_a_refusal_entry() {
    let dir = moved_agent_project("unused");
    let out = deploy(
        &dir,
        &[
            "--format",
            "json",
            "--accept-fresh-agent",
            "shop.orders",
            "--accept-fresh-agent",
            "shop.typo",
        ],
    );
    assert!(!out.status.success());
    let doc = stdout_json(&out);
    let refusals = doc["refusals"].as_array().unwrap();
    assert_eq!(refusals.len(), 1, "{doc}");
    assert_eq!(refusals[0]["code"], "bynk.deploy.accept_fresh_agent_unused");
    assert_eq!(refusals[0]["name"], "shop.typo");
    // A refused run goes ahead with nothing, so it has no warnings, as on stderr: `shop.orders`
    // was accepted, but it is not happening.
    assert_eq!(doc["warnings"], serde_json::json!([]), "{doc}");
}

#[test]
fn the_short_format_prints_no_document_for_a_refusal() {
    let dir = moved_agent_project("short");
    let out = deploy(&dir, &[]);
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A project with one plain context and no ledger.
fn plain_project(name: &str) -> PathBuf {
    let dir = scratch(name);
    write(
        &dir.join("bynk.toml"),
        "[project]\nname = \"plain\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n",
    );
    write(
        &dir.join("src/shop/orders.bynk"),
        "context shop.orders\n\nservice ping {\n\ton call() -> Effect[Int] by Caller {\n\t\t1\n\t}\n}\n",
    );
    dir
}

/// The one refusal of a stopped run's document: its code and the whole entry.
fn only_refusal(o: &Output) -> Value {
    assert!(!o.status.success(), "a refused run must exit non-zero");
    let doc = stdout_json(o);
    assert_eq!(doc["environment"], "default", "{doc}");
    assert_eq!(doc["warnings"], serde_json::json!([]), "{doc}");
    let refusals = doc["refusals"].as_array().expect("refusals is an array");
    assert_eq!(refusals.len(), 1, "{doc}");
    refusals[0].clone()
}

#[test]
fn a_context_the_build_did_not_produce_is_a_refusal_entry() {
    let dir = plain_project("context_not_found");
    let r = only_refusal(&deploy(
        &dir,
        &["--format", "json", "--context", "shop.nowhere"],
    ));
    assert_eq!(r["code"], "bynk.deploy.context_not_found");
    assert_eq!(r["requested"], "shop.nowhere");
    assert!(r.get("override").is_none(), "{r}");
}

#[test]
fn an_unreadable_ledger_is_a_refusal_entry_with_its_path() {
    let dir = plain_project("ledger_unreadable");
    write(&dir.join("bynk.deploy.lock"), "this is = not [valid toml\n");
    let r = only_refusal(&deploy(&dir, &["--format", "json"]));
    assert_eq!(r["code"], "bynk.deploy.ledger_unreadable");
    assert!(
        r["path"].as_str().unwrap().ends_with("bynk.deploy.lock"),
        "{r}"
    );
}

#[test]
fn an_unreadable_secrets_file_is_a_refusal_entry() {
    let dir = plain_project("secrets_source");
    let r = only_refusal(&deploy(
        &dir,
        &["--format", "json", "--secrets-file", "no-such-secrets.env"],
    ));
    assert_eq!(r["code"], "bynk.deploy.secrets_source_unreadable");
}

#[test]
fn a_second_env_flag_after_the_separator_is_a_refusal_entry() {
    let dir = plain_project("env_conflict");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args([
            "deploy",
            "--dry-run",
            "--format",
            "json",
            "--env",
            "staging",
        ])
        .arg(&dir)
        .args(["--", "--env", "other"])
        .env("BYNK_NO_INSTALL", "1")
        .output()
        .expect("runs bynk");
    assert!(!out.status.success());
    let doc = stdout_json(&out);
    assert_eq!(doc["environment"], "staging", "{doc}");
    let r = &doc["refusals"][0];
    assert_eq!(r["code"], "bynk.deploy.env_passthrough_conflict");
    assert_eq!(r["environment"], "staging");
    assert_eq!(r["flag"], "--env");
}

#[test]
fn a_deploy_workers_table_that_does_not_read_is_a_refusal_entry() {
    let dir = plain_project("undeployable");
    write(
        &dir.join("bynk.toml"),
        "[project]\nname = \"plain\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n\n[deploy.workers]\nbroken = 3\n",
    );
    let r = only_refusal(&deploy(&dir, &["--format", "json"]));
    assert_eq!(r["code"], "bynk.deploy.project_undeployable");
}

#[test]
fn a_stop_that_is_not_a_refusal_prints_no_document() {
    // A project that does not build stops with the compiler's diagnostics, not a refusal.
    let dir = plain_project("build_error");
    write(
        &dir.join("src/shop/orders.bynk"),
        "context shop.orders\n\nthis is not bynk\n",
    );
    let out = deploy(&dir, &["--format", "json"]);
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn a_context_bound_to_a_worker_never_deployed_is_a_refusal_entry_naming_both() {
    let dir = plain_project("dependency");
    write(
        &dir.join("src/shop/orders.bynk"),
        "context shop.orders\n\nconsumes shop.billing as Billing\n\nservice place {\n\ton call(n: Int) -> Effect[Int] by Caller {\n\t\tlet r <- Billing.charge(n)\n\t\tr\n\t}\n}\n",
    );
    write(
        &dir.join("src/shop/billing.bynk"),
        "context shop.billing\n\nservice charge {\n\ton call(n: Int) -> Effect[Int] by Caller {\n\t\tn\n\t}\n}\n",
    );
    let out = deploy(&dir, &["--format", "json", "--context", "shop.orders"]);
    let r = only_refusal(&out);
    assert_eq!(r["code"], "bynk.deploy.dependency_not_deployed");
    assert_eq!(r["worker"], "shop-orders");
    assert_eq!(r["dependencies"], serde_json::json!(["shop-billing"]));
}
