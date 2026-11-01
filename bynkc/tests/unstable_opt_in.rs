//! `bynk.toml`'s `[unstable] allow` is read the same way by the build and the
//! editor (standard-library S3, #1963, Decision E).
//!
//! The policy is built in one place (`bynk_check::stability::StabilityPolicy`)
//! from the project root and the overlay: the editor's overlay carries the open
//! `bynk.toml`, and a CLI build reads it from disk. This test runs one project
//! through both and requires the same diagnostics for its `[unstable]` entries.
//! Every entry here that names an item is stale: `bynk.list` was never
//! experimental, and `bynk.cloudflare.D1` graduated (P4d, #2652). The
//! experimental-use rule itself is tested end to end, under a
//! supplied policy, in `bynk-check/src/stability.rs`.

use bynkc::Roots;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::process::Command;

const MANIFEST: &str = "[project]\nname = \"opt-in\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.list\", \"bynk.cloudflare.D1\", \"bynk.cloudflare.Kvv\"]\n";

const SOURCE: &str = "context app.probe\n\nuses bynk.list\n\nservice probe {\n\ton call() -> Effect[Int] {\n\t\tEffect.pure(1)\n\t}\n}\n";

/// The `[unstable]` diagnostics: `bynk.project.unstable_*`, as `(code, entry)`.
fn unstable(found: impl IntoIterator<Item = (String, String)>) -> BTreeSet<(String, String)> {
    found
        .into_iter()
        .filter(|(code, _)| code.starts_with("bynk.project.unstable_"))
        .map(|(code, message)| {
            let entry = message.split('`').nth(5).unwrap_or("").to_string();
            (code, entry)
        })
        .collect()
}

#[test]
fn the_build_and_the_editor_report_the_same_unstable_diagnostics() {
    let root = std::env::temp_dir().join(format!("bynk-unstable-opt-in-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/app")).unwrap();
    fs::write(root.join("bynk.toml"), MANIFEST).unwrap();
    fs::write(root.join("src/app/probe.bynk"), SOURCE).unwrap();

    // The build: `bynkc check`, which reads `bynk.toml` from disk. `short`
    // prints `path:line:col: severity[code]: message`, one per line.
    let checked = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["check", "--format", "short"])
        .arg(&root)
        .output()
        .expect("run bynkc");
    let stdout = format!(
        "{}{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    let build = unstable(stdout.lines().filter_map(|line| {
        let (_, rest) = line.split_once('[')?;
        let (code, rest) = rest.split_once("]: ")?;
        Some((code.to_string(), rest.to_string()))
    }));
    assert!(
        !checked.status.success(),
        "an unknown `[unstable]` entry is an error:\n{stdout}"
    );

    // The editor: `analyse_project`, with `bynk.toml` in the overlay as the LSP
    // puts it there.
    let paths = bynkc::try_read_project_paths(&root).expect("the manifest reads");
    let overlay = HashMap::from([
        (root.join("bynk.toml"), MANIFEST.to_string()),
        (root.join("src/app/probe.bynk"), SOURCE.to_string()),
    ]);
    let analysis = bynk_check::analysis::analyse_project(
        &Roots::Split {
            project_root: root.clone(),
            paths,
        },
        &overlay,
        &bynkc::SchemaLock::Off,
    );
    let editor = unstable(
        analysis
            .errors
            .iter()
            .map(|a| (a.error.category.to_string(), a.error.message.clone())),
    );

    let expected = BTreeSet::from([
        (
            "bynk.project.unstable_not_experimental".to_string(),
            "bynk.list".to_string(),
        ),
        (
            "bynk.project.unstable_not_experimental".to_string(),
            "bynk.cloudflare.D1".to_string(),
        ),
        (
            "bynk.project.unstable_unknown_item".to_string(),
            "bynk.cloudflare.Kvv".to_string(),
        ),
    ]);
    assert_eq!(build, expected, "the build's report:\n{stdout}");
    assert_eq!(editor, expected, "the editor's analysis");
    let _ = fs::remove_dir_all(&root);
}
