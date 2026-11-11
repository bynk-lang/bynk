//! #3319: `bynk deploy` and a `Postgres` instance's Hyperdrive config.
//!
//! Driven through the real binary over a project with
//! `capability Main = Postgres`. `--dry-run` compiles, checks and prints the
//! plan without touching Cloudflare, so what it shows, and where it fails, is
//! what a deploy decides before it asks the account for anything.
//!
//! The connection string is a secret (R-16): every case here checks that
//! nothing the run printed or wrote contains it.

use std::path::{Path, PathBuf};
use std::process::Command;

const ORDERS: &str = r#"context shop.orders

consumes bynk
consumes bynk.cloudflare { Postgres, PgError, PgValue, PgRun }

capability Main = Postgres

service api from http {
	on GET("/orders") () -> Effect[HttpResult[String]] by Visitor given Main {
		let rows <- Main.all("select id from orders where qty > $1", [PgInteger(0)])
		match rows {
			Ok(json) => Ok(json)
			Err(_) => Ok("failed")
		}
	}
}
"#;

/// A connection string whose every part is recognisable if it leaks.
const STRING: &str = "postgres://svc_user:Hunter2-s3cret@db-host.internal.example:6543/shopdb";
const LEAKS: [&str; 5] = [
    STRING,
    "Hunter2-s3cret",
    "db-host.internal.example",
    "svc_user",
    "shopdb",
];

fn scratch(name: &str, deploy_table: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/shop")).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        format!(
            "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.cloudflare.Postgres\"]\n\n{deploy_table}"
        ),
    )
    .unwrap();
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

/// `bynk deploy --dry-run <args>` in `dir` with `env` set: (ok, stdout, stderr).
fn dry_run(dir: &Path, env: &[(&str, &str)], args: &[&str]) -> (bool, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bynk"));
    command
        .args(["deploy", "--dry-run"])
        .args(args)
        .current_dir(dir)
        .env_remove("CI")
        .env_remove("MAIN_DATABASE_URL");
    for (k, v) in env {
        command.env(k, v);
    }
    let out = command.output().expect("run bynk");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn skip() -> bool {
    if !on_path("wrangler") && !on_path("npx") {
        eprintln!("\n!!! DEPLOY HYPERDRIVE SKIPPED !!!\nneither `wrangler` nor `npx` is on PATH\n");
        return true;
    }
    false
}

/// Every file under `dir`, read as text where it is text.
fn files(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, out);
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            out.push((path, text));
        }
    }
}

fn assert_clean(what: &str, text: &str) {
    for leak in LEAKS {
        assert!(!text.contains(leak), "{what} leaks `{leak}`:\n{text}");
    }
}

const SETTINGS: &str = "[deploy.hyperdrive.Main]\nconnection_env = \"MAIN_DATABASE_URL\"\n";

#[test]
fn the_plan_creates_the_config_and_prints_none_of_the_string() {
    if skip() {
        return;
    }
    let dir = scratch("deploy-hyperdrive-plan", SETTINGS);
    let (ok, text, err) = dry_run(&dir, &[("MAIN_DATABASE_URL", STRING)], &[]);
    assert!(ok, "{text}{err}");
    assert!(
        text.contains("hyperdrive create shop-orders-main (HYPERDRIVE_MAIN)\n"),
        "{text}"
    );
    assert_clean("the plan", &text);
    assert_clean("stderr", &err);

    let (ok, json, err) = dry_run(
        &dir,
        &[("MAIN_DATABASE_URL", STRING)],
        &["--format", "json"],
    );
    assert!(ok, "{json}{err}");
    let plan: serde_json::Value = serde_json::from_str(&json).expect("the plan is JSON");
    let line = &plan["contexts"][0]["hyperdrive"][0];
    assert_eq!(line["action"], "create");
    assert_eq!(
        line["connection_env"], "MAIN_DATABASE_URL",
        "a name, not a value"
    );
    assert_clean("the plan JSON", &json);

    // Nothing under the project, `out/` and the generated configs included,
    // holds the string; and a plan writes no ledger.
    let mut all = Vec::new();
    files(&dir, &mut all);
    assert!(!all.is_empty());
    for (path, text) in all {
        assert_clean(&path.display().to_string(), &text);
    }
}

#[test]
fn a_named_environment_plans_its_own_config() {
    if skip() {
        return;
    }
    let dir = scratch("deploy-hyperdrive-staging", SETTINGS);
    let (ok, text, err) = dry_run(
        &dir,
        &[("MAIN_DATABASE_URL", STRING)],
        &["--env", "staging"],
    );
    assert!(ok, "{text}{err}");
    assert!(
        text.contains("hyperdrive create shop-orders-main-staging (HYPERDRIVE_MAIN)\n"),
        "{text}"
    );
}

#[test]
fn a_missing_variable_fails_the_deploy_naming_the_variable_and_the_capability() {
    if skip() {
        return;
    }
    let dir = scratch("deploy-hyperdrive-missing", SETTINGS);
    let (ok, text, err) = dry_run(&dir, &[], &[]);
    assert!(!ok, "{text}{err}");
    assert!(err.contains("`MAIN_DATABASE_URL` is not set"), "{err}");
    assert!(err.contains("`HYPERDRIVE_MAIN` of `shop-orders`"), "{err}");
    assert!(
        !text.contains("hyperdrive create"),
        "no plan was printed:\n{text}"
    );
}

#[test]
fn a_string_that_is_not_a_postgres_url_is_refused_without_being_echoed() {
    if skip() {
        return;
    }
    let dir = scratch("deploy-hyperdrive-malformed", SETTINGS);
    let bad = "mysql://svc_user:Hunter2-s3cret@db-host.internal.example:6543/shopdb";
    let (ok, text, err) = dry_run(&dir, &[("MAIN_DATABASE_URL", bad)], &[]);
    assert!(!ok, "{text}{err}");
    assert!(err.contains("`MAIN_DATABASE_URL`"), "{err}");
    assert_clean("the refusal", &format!("{text}{err}"));
}

#[test]
fn a_config_id_adopts_the_config_and_needs_no_variable() {
    if skip() {
        return;
    }
    let dir = scratch(
        "deploy-hyperdrive-adopt",
        "[deploy.hyperdrive.Main]\nconnection_env = \"MAIN_DATABASE_URL\"\nconfig_id = { default = \"0123456789abcdef0123456789abcdef\" }\n",
    );
    // No `MAIN_DATABASE_URL`: the environment's own `config_id` adopts.
    let (ok, text, err) = dry_run(&dir, &[], &[]);
    assert!(ok, "{text}{err}");
    assert!(
        text.contains("hyperdrive adopt 0123456789abcdef0123456789abcdef (HYPERDRIVE_MAIN)\n"),
        "{text}"
    );
    // Another environment of the same capability still needs its variable.
    let (ok, _, err) = dry_run(&dir, &[], &["--env", "staging"]);
    assert!(!ok);
    assert!(err.contains("`MAIN_DATABASE_URL` is not set"), "{err}");
}

#[test]
fn a_capability_without_settings_is_refused_pointing_at_the_table() {
    if skip() {
        return;
    }
    let dir = scratch("deploy-hyperdrive-unset", "");
    let (ok, text, err) = dry_run(&dir, &[("MAIN_DATABASE_URL", STRING)], &[]);
    assert!(!ok, "{text}{err}");
    assert!(err.contains("[deploy.hyperdrive.<Capability>]"), "{err}");
    assert_clean("the refusal", &format!("{text}{err}"));
}

#[test]
fn a_connection_string_written_into_bynk_toml_is_refused_without_being_echoed() {
    if skip() {
        return;
    }
    let dir = scratch(
        "deploy-hyperdrive-in-manifest",
        &format!("[deploy.hyperdrive.Main]\nconnection_env = \"{STRING}\"\n"),
    );
    let (ok, text, err) = dry_run(&dir, &[], &[]);
    assert!(!ok, "{text}{err}");
    assert!(err.contains("not the string itself"), "{err}");
    assert_clean("the refusal", &format!("{text}{err}"));
}
