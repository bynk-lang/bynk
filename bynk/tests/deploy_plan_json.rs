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
    // #3565: the flag that lets the move through keeps the state; the message also names the
    // one that discards it.
    assert_eq!(r["override"], "--move-agent shop.orders");
    let message = r["message"].as_str().unwrap();
    assert!(message.contains("would move from Worker script"), "{r}");
    assert!(message.contains("--accept-fresh-agent shop.orders"), "{r}");
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

#[test]
fn a_project_with_no_contexts_is_not_an_unknown_context() {
    let dir = scratch("no_workers");
    write(
        &dir.join("bynk.toml"),
        "[project]\nname = \"empty\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n",
    );
    write(
        &dir.join("src/shop/types.bynk"),
        "commons shop.types\n\nfn one() -> Int { 1 }\n",
    );
    let r = only_refusal(&deploy(&dir, &["--format", "json"]));
    assert_eq!(r["code"], "bynk.deploy.no_workers_built");
    assert!(r.get("requested").is_none(), "{r}");
}

#[test]
fn an_unreadable_pin_file_is_a_refusal_entry_with_its_path() {
    let dir = plain_project("pins_unreadable");
    write(&dir.join("bynk.foreign.toml"), "this is = = not toml\n");
    let out = deploy(&dir, &["--format", "json"]);
    let r = only_refusal(&out);
    assert_eq!(r["code"], "bynk.deploy.pins_unreadable");
    assert_eq!(r["path"], "bynk.foreign.toml");
    // The run had its upload order by then, and the document says so.
    let order = stdout_json(&out)["order"].as_array().unwrap().clone();
    assert_eq!(order.len(), 1, "{order:?}");
}

#[test]
fn an_adoption_entry_that_names_no_capability_is_a_refusal_entry() {
    let dir = plain_project("adoption_unmatched");
    let manifest = std::fs::read_to_string(dir.join("bynk.toml")).unwrap();
    let image = format!("registry.cloudflare.com/acct/web@sha256:{}", "a".repeat(64));
    write(
        &dir.join("bynk.toml"),
        &format!("{manifest}\n[deploy.containers.Web]\nimage = {{ production = \"{image}\" }}\n"),
    );
    let out = deploy(&dir, &["--format", "json"]);
    let r = only_refusal(&out);
    assert_eq!(r["code"], "bynk.deploy.adoption_unmatched");
    assert_eq!(r["table"], "containers");
    assert_eq!(r["capability"], "Web");
    assert_eq!(stdout_json(&out)["order"].as_array().unwrap().len(), 1);
}

/// A project whose context declares a `Kv` instance, which a plan with no account credentials
/// cannot check.
fn kv_project(name: &str) -> PathBuf {
    let dir = plain_project(name);
    write(
        &dir.join("src/shop/orders.bynk"),
        "context shop.orders\n\nconsumes bynk.cloudflare { Kv }\n\ncapability Sessions = Kv\n\nservice ping {\n\ton call() -> Effect[Int] by Caller {\n\t\t1\n\t}\n}\n",
    );
    dir
}

#[test]
fn a_plan_that_could_not_be_checked_against_the_account_says_so_in_its_warnings() {
    let dir = kv_project("account_note");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args(["deploy", "--dry-run", "--format", "json"])
        .arg(&dir)
        .env("BYNK_NO_INSTALL", "1")
        .env_remove("CLOUDFLARE_API_TOKEN")
        .env_remove("CLOUDFLARE_ACCOUNT_ID")
        .output()
        .expect("runs bynk");
    assert!(out.status.success(), "a dry run is not refused");
    let doc = stdout_json(&out);
    let warnings = doc["warnings"].as_array().expect("warnings is an array");
    assert_eq!(warnings.len(), 1, "{doc}");
    assert_eq!(warnings[0]["code"], "bynk.deploy.account_note");
    // The same note is on stderr, as it always was.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not checked against the account"),
        "{stderr}"
    );
}

#[test]
fn an_adoption_with_nothing_for_this_environment_is_a_warning_in_the_plan() {
    let dir = plain_project("adoption_partial");
    write(
        &dir.join("src/shop/orders.bynk"),
        "context shop.orders\n\nconsumes bynk.cloudflare { Vectorize }\n\ncapability Docs = Vectorize @dimensions(768) @metric(\"cosine\")\n\nservice ping {\n\ton call() -> Effect[Int] by Caller {\n\t\t1\n\t}\n}\n",
    );
    let manifest = std::fs::read_to_string(dir.join("bynk.toml")).unwrap();
    write(
        &dir.join("bynk.toml"),
        &format!(
            "{manifest}\n[unstable]\nallow = [\"bynk.cloudflare.Vectorize\"]\n\n[deploy.vectorize.Docs]\nindex = {{ production = \"docs-prod\" }}\n"
        ),
    );
    let out = deploy(&dir, &["--format", "json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc = stdout_json(&out);
    let adopted = doc["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["code"] == "bynk.deploy.adoption_partial")
        .unwrap_or_else(|| panic!("no adoption_partial in {doc}"));
    assert_eq!(adopted["table"], "vectorize");
    assert_eq!(adopted["capability"], "Docs");
    assert_eq!(adopted["environment"], "default");
}

#[test]
fn a_refused_run_keeps_the_warnings_stderr_printed_before_it() {
    // The adoption note is printed, then the unmatched container entry refuses: the document has both.
    let dir = plain_project("refused_keeps_warnings");
    write(
        &dir.join("src/shop/orders.bynk"),
        "context shop.orders\n\nconsumes bynk.cloudflare { Vectorize }\n\ncapability Docs = Vectorize @dimensions(768) @metric(\"cosine\")\n\nservice ping {\n\ton call() -> Effect[Int] by Caller {\n\t\t1\n\t}\n}\n",
    );
    let manifest = std::fs::read_to_string(dir.join("bynk.toml")).unwrap();
    let image = format!("registry.cloudflare.com/acct/web@sha256:{}", "a".repeat(64));
    write(
        &dir.join("bynk.toml"),
        &format!(
            "{manifest}\n[unstable]\nallow = [\"bynk.cloudflare.Vectorize\"]\n\n[deploy.vectorize.Docs]\nindex = {{ production = \"docs-prod\" }}\n\n[deploy.containers.Web]\nimage = {{ production = \"{image}\" }}\n"
        ),
    );
    let out = deploy(&dir, &["--format", "json"]);
    assert!(!out.status.success());
    let doc = stdout_json(&out);
    assert_eq!(
        doc["refusals"][0]["code"], "bynk.deploy.adoption_unmatched",
        "{doc}"
    );
    assert_eq!(
        doc["warnings"][0]["code"], "bynk.deploy.adoption_partial",
        "{doc}"
    );
}

// #3565 (agent-context moves, slice 3): `--move-agent` plans a move with `--dry-run`.

/// A context's source: one that hosts agent `class` (with `prefix` before it, a `replaces`
/// for instance), or a plain service when `class` is empty.
fn context_source(context: &str, class: &str) -> String {
    if class.is_empty() {
        format!(
            "context {context}\n\nservice ping {{\n\ton call() -> Effect[Int] by Caller {{\n\t\t1\n\t}}\n}}\n"
        )
    } else {
        format!(
            "context {context}\n\nagent {class} {{\n\tkey id: String\n\n\tstore count: Cell[Int]\n\n\ton call current() -> Effect[Int] {{\n\t\tcount\n\t}}\n}}\n"
        )
    }
}

/// A project of `contexts` (`(context, agent class or "")`), with `manifest` after its
/// `[project]` table and `lock` as its ledger.
fn moving_project(name: &str, contexts: &[(&str, &str)], manifest: &str, lock: &str) -> PathBuf {
    let dir = scratch(name);
    write(
        &dir.join("bynk.toml"),
        &format!(
            "[project]\nname = \"moving\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n{manifest}"
        ),
    );
    for (context, class) in contexts {
        let file = context.replace('.', "/");
        write(
            &dir.join(format!("src/{file}.bynk")),
            &context_source(context, class),
        );
    }
    write(
        &dir.join("bynk.deploy.lock"),
        &format!("version = 1\n{lock}"),
    );
    dir
}

/// The ledger record of a deployed Worker with these members.
fn record(worker: &str, contexts: &[&str]) -> String {
    let members = contexts
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\n[environments.default.workers.{worker}]\ndeployed = true\ncontexts = [{members}]\nscript = \"{worker}\"\n"
    )
}

/// The plan of a `--move-agent` run that goes ahead: no refusals, and the `moves` array.
fn planned_moves(o: &Output) -> (Value, Value) {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let doc = stdout_json(o);
    assert_eq!(doc["refusals"], serde_json::json!([]), "{doc}");
    let moves = doc["moves"].clone();
    (doc, moves)
}

#[test]
fn a_named_move_out_of_a_dissolved_worker_is_planned_with_its_phases() {
    let dir = moved_agent_project("move_planned");
    let out = deploy(&dir, &["--format", "json", "--move-agent", "shop.orders"]);
    let (doc, moves) = planned_moves(&out);
    assert_eq!(
        moves,
        serde_json::json!([{
            "moves": [{
                "context": "shop.orders",
                "agents": ["Counter"],
                "from": "acme-core",
                "to": "shop-orders",
            }],
            "receive": ["shop-orders"],
            "rebind": [],
            "commit": ["acme-core"],
            "go_live": ["shop-orders"],
        }]),
        "{doc}"
    );
    let warnings = doc["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{doc}");
    assert_eq!(warnings[0]["code"], "bynk.deploy.agent_move_window");
    assert_eq!(warnings[0]["contexts"], serde_json::json!(["shop.orders"]));
}

#[test]
fn the_text_plan_lists_each_move_set_with_its_phases() {
    let dir = moved_agent_project("move_text");
    let out = deploy(&dir, &["--move-agent", "shop.orders"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    for line in [
        "move set 1: `shop.orders` (Counter) from `acme-core` to `shop-orders`",
        "  1 receive: shop-orders (forwarding the moving contexts)",
        "  2 commit: acme-core",
        "  3 go live: shop-orders",
    ] {
        assert!(text.contains(line), "{line}\n{text}");
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("about one push"), "{stderr}");
}

/// Without `--dry-run`, a supported move is still refused: this release only plans it.
#[test]
fn a_named_move_outside_a_dry_run_is_refused_as_not_applied_yet() {
    let dir = moved_agent_project("move_not_applied");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args(["deploy", "--format", "json", "--move-agent", "shop.orders"])
        .arg(&dir)
        .env("BYNK_NO_INSTALL", "1")
        .output()
        .expect("runs bynk");
    let r = only_refusal(&out);
    assert_eq!(r["code"], "bynk.deploy.agent_move_refused");
    assert_eq!(r["reason"], "apply_not_yet");
    assert_eq!(r["contexts"], serde_json::json!(["shop.orders"]));
    assert!(r["message"].as_str().unwrap().contains("--dry-run"), "{r}");
}

#[test]
fn a_move_agent_name_that_matches_no_move_is_a_refusal_entry() {
    let dir = moved_agent_project("move_unused");
    let out = deploy(
        &dir,
        &[
            "--format",
            "json",
            "--move-agent",
            "shop.orders",
            "--move-agent",
            "shop.typo",
        ],
    );
    let r = only_refusal(&out);
    assert_eq!(r["code"], "bynk.deploy.move_agent_unused");
    assert_eq!(r["name"], "shop.typo");
}

#[test]
fn moving_and_starting_fresh_the_same_context_is_refused() {
    let dir = moved_agent_project("move_both");
    let out = deploy(
        &dir,
        &[
            "--format",
            "json",
            "--move-agent",
            "shop.orders",
            "--accept-fresh-agent",
            "shop.orders",
        ],
    );
    let r = only_refusal(&out);
    assert_eq!(r["reason"], "both");
    assert_eq!(r["context"], "shop.orders");
}

/// Fan-in: two Workers of their own joining one group are one set with two sources.
#[test]
fn two_contexts_joining_a_group_are_one_set() {
    let dir = moving_project(
        "move_fan_in",
        &[("shop.orders", "Cart"), ("shop.pay", "Wallet")],
        "\n[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.pay\"]\n",
        &(record("shop-orders", &["shop.orders"]) + &record("shop-pay", &["shop.pay"])),
    );
    let out = deploy(
        &dir,
        &[
            "--format",
            "json",
            "--move-agent",
            "shop.orders",
            "--move-agent",
            "shop.pay",
        ],
    );
    let (doc, moves) = planned_moves(&out);
    assert_eq!(moves.as_array().unwrap().len(), 1, "{doc}");
    assert_eq!(moves[0]["receive"], serde_json::json!(["core"]));
    assert_eq!(
        moves[0]["commit"],
        serde_json::json!(["shop-orders", "shop-pay"])
    );
}

/// Fan-out: a group split into two Workers of their own is one set with two receivers.
#[test]
fn a_group_split_in_two_is_one_set() {
    let dir = moving_project(
        "move_fan_out",
        &[("shop.orders", "Cart"), ("shop.pay", "Wallet")],
        "",
        &record("core", &["shop.orders", "shop.pay"]),
    );
    let out = deploy(
        &dir,
        &[
            "--format",
            "json",
            "--move-agent",
            "shop.orders",
            "--move-agent",
            "shop.pay",
        ],
    );
    let (doc, moves) = planned_moves(&out);
    assert_eq!(moves.as_array().unwrap().len(), 1, "{doc}");
    assert_eq!(
        moves[0]["receive"],
        serde_json::json!(["shop-orders", "shop-pay"])
    );
    assert_eq!(moves[0]["commit"], serde_json::json!(["core"]));
}

/// A context leaving a group that keeps running, once refused even with `--accept-fresh-agent`
/// (`old_script_stays`), is movable: the group gives the class up with a `transferred`
/// tombstone. And between groups likewise.
#[test]
fn moves_out_of_a_staying_group_and_between_groups_are_planned() {
    let out_of = moving_project(
        "move_out_of_staying",
        &[("shop.orders", "Cart"), ("shop.pay", "")],
        "\n[deploy.workers.core]\ncontexts = [\"shop.pay\"]\n",
        &record("core", &["shop.orders", "shop.pay"]),
    );
    let (doc, moves) = planned_moves(&deploy(
        &out_of,
        &["--format", "json", "--move-agent", "shop.orders"],
    ));
    assert_eq!(
        moves[0]["receive"],
        serde_json::json!(["shop-orders"]),
        "{doc}"
    );
    assert_eq!(moves[0]["commit"], serde_json::json!(["core"]), "{doc}");

    let between = moving_project(
        "move_between_groups",
        &[("shop.orders", "Cart"), ("shop.pay", "")],
        "\n[deploy.workers.hub]\ncontexts = [\"shop.orders\"]\n\n[deploy.workers.core]\ncontexts = [\"shop.pay\"]\n",
        &record("core", &["shop.orders", "shop.pay"]),
    );
    let (doc, moves) = planned_moves(&deploy(
        &between,
        &["--format", "json", "--move-agent", "shop.orders"],
    ));
    assert_eq!(moves[0]["receive"], serde_json::json!(["hub"]), "{doc}");
    assert_eq!(moves[0]["commit"], serde_json::json!(["core"]), "{doc}");
}

/// The shapes v1 still refuses, each with its reason.
#[test]
fn a_set_through_a_partial_run_or_a_routed_worker_is_refused() {
    // `--context` leaves out the group the context leaves, which the set must push.
    let partial = moving_project(
        "move_partial",
        &[("shop.orders", "Cart"), ("shop.pay", "")],
        "\n[deploy.workers.core]\ncontexts = [\"shop.pay\"]\n",
        &record("core", &["shop.orders", "shop.pay"]),
    );
    let r = only_refusal(&deploy(
        &partial,
        &[
            "--format",
            "json",
            "--context",
            "shop.orders",
            "--move-agent",
            "shop.orders",
        ],
    ));
    assert_eq!(r["reason"], "partial_selection", "{r}");
    assert_eq!(r["workers"], serde_json::json!(["core"]), "{r}");

    // A receiver with routes (D12: S9 was not run).
    let routed = moving_project(
        "move_routes",
        &[("shop.orders", "Cart")],
        "\n[deploy.workers.core]\ncontexts = [\"shop.orders\"]\nroutes = [\"shop.example.com/*\"]\n",
        &record("shop-orders", &["shop.orders"]),
    );
    let r = only_refusal(&deploy(
        &routed,
        &["--format", "json", "--move-agent", "shop.orders"],
    ));
    assert_eq!(r["reason"], "routes", "{r}");
    assert_eq!(r["worker"], "core", "{r}");
    assert_eq!(r["key"], "routes", "{r}");
}

/// Two groups swapping members: each Worker is both a source and a receiver.
#[test]
fn a_worker_that_gives_and_receives_in_one_run_is_chained() {
    let dir = moving_project(
        "move_chained",
        &[("shop.orders", "Cart"), ("shop.pay", "Wallet")],
        "\n[deploy.workers.g1]\ncontexts = [\"shop.pay\"]\n\n[deploy.workers.g2]\ncontexts = [\"shop.orders\"]\n",
        &(record("g1", &["shop.orders"]) + &record("g2", &["shop.pay"])),
    );
    let out = deploy(
        &dir,
        &[
            "--format",
            "json",
            "--move-agent",
            "shop.orders",
            "--move-agent",
            "shop.pay",
        ],
    );
    let r = only_refusal(&out);
    assert_eq!(r["reason"], "chained", "{r}");
}

/// The maintainer's ruling on #3565: a context carrying its own class tombstones is not moved in
/// v1.
#[test]
fn a_context_carrying_class_tombstones_is_refused() {
    let dir = moved_agent_project("move_tombstones");
    write(
        &dir.join("src/shop/orders.bynk"),
        "context shop.orders\n\nagent Counter replaces Tally {\n\tkey id: String\n\n\tstore count: Cell[Int]\n\n\ton call current() -> Effect[Int] {\n\t\tcount\n\t}\n}\n",
    );
    // The rename names a class the schema lock records.
    write(
        &dir.join("bynk.schema.lock"),
        "version = 2\n\n[events]\n\n[agents.\"shop.orders.Tally\"]\nversion = 1\nfloor = 1\n\n[[agents.\"shop.orders.Tally\".shapes]]\nversion = 1\nadditive = false\n\n[[agents.\"shop.orders.Tally\".shapes.fields]]\nname = \"count\"\nkind = \"Cell\"\ntype = \"Int\"\nrefinement = \"Int\"\n\n[[agents.\"shop.orders.Tally\".shapes.fields.paths]]\npath = \"count\"\ntype = \"Int\"\n",
    );
    let r = only_refusal(&deploy(
        &dir,
        &["--format", "json", "--move-agent", "shop.orders"],
    ));
    assert_eq!(r["reason"], "tombstones", "{r}");
    assert_eq!(r["tombstones"], serde_json::json!(["Tally"]), "{r}");
}
