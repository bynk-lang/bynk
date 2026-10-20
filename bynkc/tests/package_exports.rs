//! `[package] exports` lists only commons and adapters the project declares
//! (packaging slice 1, #2207, decision D; track D1). The check needs the
//! project's units, so it runs at `check` time, and the build and the editor
//! read `[package]` the same way: this runs one project through both.

use bynkc::Roots;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::process::Command;

const MANIFEST: &str = "[package]\nname = \"acme/money\"\nversion = \"1.4.0\"\nexports = [\"money\", \"shop.checkout\", \"currncy\"]\n";

const COMMONS: &str = "commons money {\n\tfn dbl(n: Int) -> Int { n + n }\n}\n";
const CURRENCY: &str = "commons currency {\n\tfn one() -> Int { 1 }\n}\n";
const CONTEXT: &str = "context shop.checkout\n\nservice probe {\n\ton call() -> Effect[Int] {\n\t\tEffect.pure(1)\n\t}\n}\n";

/// The `exports` diagnostics, as `(code, entry)`.
fn exports(found: impl IntoIterator<Item = (String, String)>) -> BTreeSet<(String, String)> {
    found
        .into_iter()
        .filter(|(code, _)| code.starts_with("bynk.project.export_"))
        .map(|(code, message)| {
            let entry = message.split('`').nth(5).unwrap_or("").to_string();
            (code, entry)
        })
        .collect()
}

#[test]
fn the_build_and_the_editor_refuse_the_same_exports() {
    let root = std::env::temp_dir().join(format!("bynk-package-exports-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/shop")).unwrap();
    fs::write(root.join("bynk.toml"), MANIFEST).unwrap();
    fs::write(root.join("src/money.bynk"), COMMONS).unwrap();
    fs::write(root.join("src/currency.bynk"), CURRENCY).unwrap();
    fs::write(root.join("src/shop/checkout.bynk"), CONTEXT).unwrap();

    let checked = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["check", "--format", "short"])
        .arg(&root)
        .output()
        .expect("run bynkc");
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(
        !checked.status.success(),
        "bad exports fail `check`:\n{out}"
    );
    let build = exports(out.lines().filter_map(|line| {
        let (_, rest) = line.split_once('[')?;
        let (code, rest) = rest.split_once("]: ")?;
        Some((code.to_string(), rest.to_string()))
    }));

    let paths = bynkc::try_read_project_paths(&root).expect("the manifest reads");
    let overlay = HashMap::from([
        (root.join("bynk.toml"), MANIFEST.to_string()),
        (root.join("src/money.bynk"), COMMONS.to_string()),
        (root.join("src/currency.bynk"), CURRENCY.to_string()),
        (root.join("src/shop/checkout.bynk"), CONTEXT.to_string()),
    ]);
    let analysis = bynk_check::analysis::analyse_project(
        &Roots::Split {
            project_root: root.clone(),
            paths,
        },
        &overlay,
        &bynkc::SchemaLock::Off,
    );
    let editor = exports(
        analysis
            .errors
            .iter()
            .map(|a| (a.error.category.to_string(), a.error.message.clone())),
    );

    let expected = BTreeSet::from([
        (
            "bynk.project.export_context".to_string(),
            "shop.checkout".to_string(),
        ),
        (
            "bynk.project.export_unknown".to_string(),
            "currncy".to_string(),
        ),
    ]);
    assert_eq!(build, expected, "the build's report:\n{out}");
    assert_eq!(editor, expected, "the editor's analysis");

    // The same project with `exports` naming only shareable units checks clean.
    fs::write(
        root.join("bynk.toml"),
        "[package]\nname = \"acme/money\"\nexports = [\"money\", \"currency\"]\n",
    )
    .unwrap();
    let checked = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["check", "--format", "short"])
        .arg(&root)
        .output()
        .expect("run bynkc");
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let _ = fs::remove_dir_all(&root);
}
