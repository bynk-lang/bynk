//! #3749: a build that fills `paths` into an existing `bynk.schema.lock`
//! says so. ADR 0682 C has an unchanged build record each stored field's
//! positions in a lock written before they existed, once. That rewrite was
//! silent, so a project built only in throwaway CI checkouts never committed
//! it, and found out only when an additive change was refused. `bynk build`
//! now names the agents it recorded positions for and asks for the lock to be
//! committed. Nothing else about the build changes.

use std::path::{Path, PathBuf};
use std::process::Command;

const CART: &str = "context shop\n\ntype Prefs = { theme: String }\n\nagent Cart {\n  key id: String\n\n  store byId: Map[String, Prefs]\n\n  on call ping() -> Effect[Int] {\n    1\n  }\n}\n";

fn build(root: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["build".as_ref(), root.as_os_str()])
        .output()
        .expect("run bynk build");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "`bynk build` should succeed:\n{s}");
    s
}

#[test]
fn a_build_that_fills_paths_into_the_lock_says_so() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("schema-lock-paths-note");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src/shop")).unwrap();
    std::fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/shop/cart.bynk"), CART).unwrap();
    let lock = root.join("bynk.schema.lock");

    // A fresh lock records paths from the start: there is nothing to say.
    let first = build(&root);
    assert!(!first.contains("stored positions"), "{first}");
    let recorded = std::fs::read_to_string(&lock).unwrap();
    let at = recorded
        .find("[[agents.\"shop.Cart\".shapes.fields.paths]]")
        .expect("the build records paths");

    // A lock from before ADR 0682: the same shape, no paths.
    std::fs::write(&lock, &recorded[..at]).unwrap();
    let filled = build(&root);
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        recorded,
        "an unchanged build fills the paths back in"
    );
    assert!(
        filled.contains("bynk.schema.lock: recorded stored positions for shop.Cart; commit it"),
        "the build should say it filled the lock's paths:\n{filled}"
    );

    // Once filled, a build is silent again.
    let again = build(&root);
    assert!(!again.contains("stored positions"), "{again}");
}
