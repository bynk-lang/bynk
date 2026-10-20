//! Path dependencies (packaging slice 2a, #2223): the build and the editor
//! resolve the same package graph and report the same diagnostics. One
//! consumer with a dependency, run through `bynkc check` and through the
//! editor's analysis over the content the language server would sweep
//! (`bynk_testkit::read_project_sources`, which walks the dependency's trees
//! as `bynk-lsp`'s sweep does).

use bynkc::Roots;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// `(code, message)` for every package diagnostic, and every diagnostic in a
/// dependency file.
fn package_diagnostics(
    found: impl IntoIterator<Item = (String, String)>,
) -> BTreeSet<(String, String)> {
    found
        .into_iter()
        .filter(|(code, _)| code.starts_with("bynk.package.") || code.starts_with("bynk.types."))
        .collect()
}

#[test]
fn the_build_and_the_editor_resolve_the_same_graph() {
    let ws = std::env::temp_dir().join(format!("bynk-package-deps-{}", std::process::id()));
    let _ = fs::remove_dir_all(&ws);
    write(
        &ws.join("packages/money/bynk.toml"),
        "[package]\nname = \"acme/money\"\nexports = [\"currency\"]\n",
    );
    // A type error inside the dependency is reported in both.
    write(
        &ws.join("packages/money/src/currency.bynk"),
        "commons currency\n\nfn double(c: Int) -> Int {\n  c + true\n}\n",
    );
    write(
        &ws.join("packages/money/src/rounding.bynk"),
        "commons rounding\n\nfn half(n: Int) -> Int {\n  n / 2\n}\n",
    );
    let root = ws.join("shop");
    write(
        &root.join("bynk.toml"),
        "[project]\nname = \"shop\"\n\n[dependencies]\nmoney = { package = \"acme/money\", path = \"../packages/money\" }\n",
    );
    write(
        &root.join("src/pricing.bynk"),
        "commons pricing\n\nuses money.currency\nuses money.rounding\n\nfn total(n: Int) -> Int {\n  n\n}\n",
    );

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
    assert!(!checked.status.success(), "{out}");
    let build = package_diagnostics(out.lines().filter_map(|line| {
        let (_, rest) = line.split_once("error[")?;
        let (code, rest) = rest.split_once("]: ")?;
        Some((code.to_string(), rest.to_string()))
    }));

    let paths = bynkc::try_read_project_paths(&root).expect("the manifest reads");
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::Project(root.clone()));
    assert!(
        sources
            .keys()
            .any(|p| p.ends_with("packages/money/src/currency.bynk")),
        "the sweep reads the dependency's files"
    );
    let analysis = bynk_check::analysis::analyse_project(
        &Roots::Split {
            project_root: root.clone(),
            paths,
        },
        &sources,
        &bynkc::SchemaLock::Off,
    );
    let editor = package_diagnostics(
        analysis
            .errors
            .iter()
            .map(|a| (a.error.category.to_string(), a.error.message.clone())),
    );

    assert_eq!(build, editor, "the build's report:\n{out}");
    let codes: BTreeSet<&str> = build.iter().map(|(c, _)| c.as_str()).collect();
    assert_eq!(
        codes,
        BTreeSet::from(["bynk.package.unit_not_exported", "bynk.types.type_mismatch"]),
        "{out}"
    );
    let _ = fs::remove_dir_all(&ws);
}
