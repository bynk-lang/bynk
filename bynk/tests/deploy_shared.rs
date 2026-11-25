//! #3943: `bynk deploy --env <name> --dry-run` lists what the environment shares with
//! `default`. When `default` cannot be built the comparison cannot be made, which a
//! plain run says and `--strict-environments` refuses, rather than passing unchecked.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("deploy_shared")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A project with a foreign `Service @worker("legacy-billing")` (the same literal in
/// every environment) and a required constant. `vars` is the `bynk.toml` tail.
fn project(name: &str, vars: &str) -> PathBuf {
    let dir = scratch(name);
    write(
        &dir.join("bynk.toml"),
        &format!(
            "[project]\nname = \"shared\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n\n[unstable]\nallow = [\"bynk.cloudflare.Service\"]\n\n{vars}"
        ),
    );
    write(
        &dir.join("src/shop/billing.bynk"),
        "context shop.billing\n\nconsumes bynk\nconsumes bynk.cloudflare { Service }\n\nconst Origin: String = env(ORIGIN)\n\ncapability Billing = Service @worker(\"legacy-billing\")\n\nservice charge {\n\ton call(id: String) -> Effect[Int] by Caller given Billing {\n\t\tlet req = Request { method: Post, url: \"https://legacy-billing/charge\", contentType: None, authorization: None, headers: Map.empty(), body: Some(id) }\n\t\tlet res <- Billing.send(req)\n\t\tmatch res { Ok(r) => r.status, Err(_) => 0 }\n\t}\n}\n",
    );
    dir
}

fn dry_run(dir: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args(["deploy", "--env", "staging", "--dry-run"])
        .args(extra)
        .arg(dir)
        .env("BYNK_NO_INSTALL", "1")
        .output()
        .expect("runs bynk")
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn a_shared_foreign_worker_is_listed_and_strict_refuses_it() {
    // `ORIGIN` is set for both, so `default` builds; the Worker name is the same literal.
    let dir = project(
        "shared",
        "[env.default.vars]\nORIGIN = \"https://app\"\n\n[env.staging.vars]\nORIGIN = \"https://staging\"\n",
    );
    let plain = dry_run(&dir, &[]);
    assert!(plain.status.success(), "{}", err(&plain));
    let out = String::from_utf8_lossy(&plain.stdout).into_owned();
    assert!(
        out.contains("shared with default: SERVICE_BILLING → \"legacy-billing\""),
        "{out}"
    );
    let json = dry_run(&dir, &["--format", "json"]);
    let json = String::from_utf8_lossy(&json.stdout).into_owned();
    assert!(json.contains("\"shared_with_default\""), "{json}");
    let strict = dry_run(&dir, &["--strict-environments"]);
    assert!(!strict.status.success());
    assert!(
        err(&strict).contains("shares SERVICE_BILLING with `default`"),
        "{}",
        err(&strict)
    );
}

#[test]
fn an_unbuildable_default_is_said_and_strict_refuses_instead_of_passing() {
    // Only staging sets `ORIGIN`, so `default` cannot be built.
    let dir = project(
        "unbuildable",
        "[env.staging.vars]\nORIGIN = \"https://staging\"\n",
    );
    let plain = dry_run(&dir, &[]);
    assert!(plain.status.success(), "{}", err(&plain));
    let said = err(&plain);
    assert!(said.contains("`default` could not be built"), "{said}");
    assert!(
        !String::from_utf8_lossy(&plain.stdout).contains("shared with default"),
        "nothing was compared"
    );
    let strict = dry_run(&dir, &["--strict-environments"]);
    assert!(!strict.status.success(), "isolation was not checked");
    assert!(err(&strict).contains("was not checked"), "{}", err(&strict));
}
