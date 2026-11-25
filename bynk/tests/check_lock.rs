//! #4310: `bynk check` resolves dependencies first, so on a project with
//! dependencies it creates `bynk.lock` the first time (and says so), changes
//! nothing on a second run, and under `--locked` fails rather than write it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("check_lock")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A workspace with a `money` package and a `shop` project depending on it by
/// path; returns the project directory.
fn project(root: &Path) -> PathBuf {
    write(
        &root.join("bynk.toml"),
        "[workspace]\nmembers = [\"packages/*\", \"apps/*\"]\n",
    );
    write(
        &root.join("packages/money/bynk.toml"),
        "[package]\nname = \"acme-labs/money\"\nversion = \"1.0.0\"\nexports = [\"currency\"]\n",
    );
    write(
        &root.join("packages/money/src/currency.bynk"),
        "commons currency\n\nfn double(n: Int) -> Int {\n  n * 2\n}\n",
    );
    let shop = root.join("apps/shop");
    write(
        &shop.join("bynk.toml"),
        "[project]\nname = \"shop\"\n\n[dependencies]\nmoney = { package = \"acme-labs/money\", path = \"../../packages/money\" }\n",
    );
    write(
        &shop.join("src/pricing.bynk"),
        "commons pricing\n\nuses money.currency\n\nfn total(n: Int) -> Int {\n  double(n)\n}\n",
    );
    shop
}

/// Every file under `root`, by relative path.
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn check(cwd: &Path, extra: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .arg("check")
        .args(extra)
        .current_dir(cwd)
        .output()
        .expect("spawn bynk");
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn check_creates_only_the_lock_and_says_so_then_changes_nothing() {
    let root = scratch("creates");
    let shop = project(&root);
    let before = tree(&root);

    let (code, out) = check(&shop, &[]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("wrote bynk.lock"), "{out}");
    let after = tree(&root);
    let added: Vec<&PathBuf> = after.keys().filter(|p| !before.contains_key(*p)).collect();
    assert_eq!(added, [&PathBuf::from("bynk.lock")], "only the lock is new");
    for (path, bytes) in &before {
        assert_eq!(
            after.get(path),
            Some(bytes),
            "{} is unchanged",
            path.display()
        );
    }

    let (code, out) = check(&shop, &[]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("wrote bynk.lock"), "{out}");
    assert_eq!(tree(&root), after, "a second check changes nothing");
}

#[test]
fn check_locked_fails_instead_of_writing_the_lock() {
    let root = scratch("locked");
    let shop = project(&root);
    let before = tree(&root);
    let (code, out) = check(&shop, &["--locked"]);
    assert_ne!(code, 0, "{out}");
    assert!(out.contains("--locked"), "{out}");
    assert_eq!(tree(&root), before, "nothing was written");
}
