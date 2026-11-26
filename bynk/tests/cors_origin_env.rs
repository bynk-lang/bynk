//! #4504: a `cors` origin is checked as the value of the environment being built.
//!
//! `env(NAME)` constants are rewritten to the literal `bynk.toml` gives them for the environment
//! the build selects (`[env.<name>.vars]`, then `[vars]`, then the default), before the checker
//! runs, so an origin a browser could never send is refused for the environment that carries it,
//! and a good one in the default environment does not hide it. Driven through the real binary:
//! the fixtures under `bynk-tests` build the default environment only.

use std::path::{Path, PathBuf};
use std::process::Command;

const API: &str = "\
context api

const AppOrigin: String = env(APP_ORIGIN)

service api from http {
  cors { origins: [AppOrigin] }

  on GET(\"/ping\") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok(\"pong\")
  }
}
";

/// A project whose default environment has a good origin and whose `staging` has a bad one.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        "[project]\nname = \"cors-env\"\nversion = \"0.1.0\"\n\n\
         [vars]\nAPP_ORIGIN = \"https://app.example.com\"\n\n\
         [env.staging.vars]\nAPP_ORIGIN = \"https://Staging.example.com/\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/api.bynk"), API).unwrap();
    dir
}

/// `bynk build <args>` in `dir`: (success, stdout + stderr).
fn build(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .arg("build")
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("run bynk");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn a_bad_origin_in_one_environment_is_refused_when_that_environment_is_built() {
    let dir = scratch("cors-origin-env");

    // The default environment's origin is fine, so a plain build passes.
    let (ok, text) = build(&dir, &[]);
    assert!(ok, "the default environment builds:\n{text}");

    // `staging` carries the bad one, refused at the constant's use in the `cors` block.
    let (ok, text) = build(&dir, &["--env", "staging"]);
    assert!(!ok, "the staging build is refused:\n{text}");
    assert!(text.contains("bynk.http.cors_invalid_origins"), "{text}");
    assert!(
        text.contains("`cors` origin `https://Staging.example.com/`"),
        "the message carries the value the environment gave: {text}"
    );
    assert!(
        text.contains("write `https://staging.example.com`"),
        "the corrected form is suggested: {text}"
    );
    assert!(text.contains("src/api.bynk:6"), "at the entry: {text}");
}
