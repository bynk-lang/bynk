//! #3796: `bynk deploy --dry-run` changes nothing in the project. It used to
//! reconcile `bynk.schema.lock` on disk, as a real build does, and write its
//! build output under `.bynk/deploy`, even when the plan it printed was then
//! refused. A preview of a deploy now builds into a scratch directory and
//! reconciles the lock in memory only.
//!
//! Driven through the real binary over a one-context project whose agent has
//! stored state, so a build has a schema to record.

use std::path::{Path, PathBuf};
use std::process::Command;

const COUNTER: &str = r#"context shop.counter

agent Counter {
  key id: String

  store hits: Map[String, Int]

  on call bump(name: String) -> Effect[Bool] {
    let _ <- hits.put(name, 1)
    true
  }
}

service api {
  on call(id: String) -> Effect[Bool] {
    Counter(id).bump("x")
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
    std::fs::write(dir.join("src/shop/counter.bynk"), COUNTER).unwrap();
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

fn bynk(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args(args)
        .current_dir(dir)
        .env_remove("CI")
        // #3494: with these, the plan reads the account.
        .env_remove("CLOUDFLARE_API_TOKEN")
        .env_remove("CLOUDFLARE_ACCOUNT_ID")
        .output()
        .expect("run bynk");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

#[test]
fn a_dry_run_writes_neither_the_schema_lock_nor_a_build() {
    // `deploy`'s preflight needs a way to run wrangler, though a dry run never
    // calls it.
    if !on_path("wrangler") && !on_path("npx") {
        eprintln!("\n!!! DEPLOY DRY RUN SKIPPED !!!\nneither `wrangler` nor `npx` is on PATH\n");
        return;
    }
    let dir = scratch("deploy-dry-run-writes-nothing");
    let lock = dir.join("bynk.schema.lock");

    // No lock yet: a real build would create one. A dry run, text or JSON,
    // must not.
    for args in [
        &["deploy", "--dry-run"][..],
        &["deploy", "--dry-run", "--format", "json"][..],
        &["deploy", "--plan"][..],
    ] {
        let (ok, log) = bynk(&dir, args);
        assert!(ok, "`bynk {}` should succeed:\n{log}", args.join(" "));
        assert!(
            !lock.exists(),
            "`bynk {}` wrote {}:\n{log}",
            args.join(" "),
            lock.display()
        );
        assert!(
            !dir.join(".bynk").join("deploy").exists(),
            "`bynk {}` wrote its build under `.bynk/deploy`:\n{log}",
            args.join(" ")
        );
    }

    // A committed lock the build would change is left byte for byte.
    let (ok, log) = bynk(&dir, &["build"]);
    assert!(ok, "`bynk build` should succeed:\n{log}");
    let recorded = std::fs::read_to_string(&lock).expect("`bynk build` records the schema");
    std::fs::write(
        dir.join("src/shop/counter.bynk"),
        COUNTER
            .replace("Map[String, Int]", "Map[String, String]")
            .replace("hits.put(name, 1)", "hits.put(name, \"1\")"),
    )
    .unwrap();
    let _ = bynk(&dir, &["deploy", "--dry-run"]);
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        recorded,
        "a dry run rewrote the committed {}",
        lock.display()
    );
}
