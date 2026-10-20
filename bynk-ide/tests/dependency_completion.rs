//! #2223 follow-up: completion and hover know a project's path dependencies.
//! A dependency's units are offered only as `<alias>.<unit>`, only when
//! exported, and never by their bare names; a dependency adapter's
//! capabilities complete through its aliased name.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_ide::completion::complete;
use bynk_ide::packages::DependencyView;

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A workspace with `packages/money` (`acme/money`, exporting `currency` and
/// `tokens`, but not `rounding`) and `shop`, which depends on it as `money`.
/// Returns the shop's root and its content map (manifest, own sources, and the
/// dependency's sources — what the language server sweeps).
fn workspace(tag: &str) -> (PathBuf, HashMap<PathBuf, String>) {
    let ws = std::env::temp_dir().join(format!("bynk-ide-deps-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&ws);
    write(
        &ws.join("packages/money/bynk.toml"),
        "[package]\nname = \"acme/money\"\nexports = [\"currency\", \"tokens\"]\n",
    );
    let dep = [
        (
            "currency.bynk",
            "--- Money in minor units.\ncommons currency\n\nfn double(c: Int) -> Int {\n  c * 2\n}\n",
        ),
        (
            "rounding.bynk",
            "commons rounding\n\nfn half(n: Int) -> Int {\n  n / 2\n}\n",
        ),
        (
            "tokens.bynk",
            "adapter tokens {\n  binding \"./tokens.binding.ts\"\n\n  exports capability { Jwt }\n\n  capability Jwt {\n    fn sign(sub: String) -> Effect[String]\n  }\n\n  provides Jwt = JoseJwt\n}\n",
        ),
    ];
    let root = ws.join("shop");
    let manifest = "[project]\nname = \"shop\"\n\n[dependencies]\nmoney = { package = \"acme/money\", path = \"../packages/money\" }\n";
    write(&root.join("bynk.toml"), manifest);
    let pricing = "commons pricing\n\nfn total(n: Int) -> Int {\n  n\n}\n";
    write(&root.join("src/pricing.bynk"), pricing);
    let mut files = HashMap::from([
        (root.join("bynk.toml"), manifest.to_string()),
        (root.join("src/pricing.bynk"), pricing.to_string()),
    ]);
    for (name, text) in dep {
        let p = ws.join("packages/money/src").join(name);
        write(&p, text);
        files.insert(p, text.to_string());
    }
    (root, files)
}

fn labels(line: &str, doc: &str, files: &HashMap<PathBuf, String>) -> Vec<String> {
    complete(line, doc, Some(files))
        .into_iter()
        .map(|c| c.label)
        .collect()
}

#[test]
fn uses_offers_own_commons_and_exported_dependency_commons_by_alias() {
    let (_root, files) = workspace("uses");
    let got = labels("uses ", "context shop.checkout\n\nuses ", &files);
    assert!(got.contains(&"pricing".to_string()), "{got:?}");
    assert!(got.contains(&"money.currency".to_string()), "{got:?}");
    // Not exported, and never by a bare name.
    assert!(!got.contains(&"money.rounding".to_string()), "{got:?}");
    assert!(!got.contains(&"currency".to_string()), "{got:?}");
    assert!(!got.contains(&"rounding".to_string()), "{got:?}");
}

#[test]
fn consumes_offers_a_dependency_adapter_by_alias_and_its_capabilities() {
    let (_root, files) = workspace("consumes");
    let doc = "context shop.checkout\n\nconsumes ";
    let got = labels("consumes ", doc, &files);
    assert!(got.contains(&"money.tokens".to_string()), "{got:?}");
    assert!(!got.contains(&"tokens".to_string()), "{got:?}");

    let got = labels("consumes money.tokens { ", doc, &files);
    assert_eq!(got, ["Jwt"]);

    let doc = "context shop.checkout\n\nconsumes money.tokens\n\nservice s {\n  on call() -> Effect[String] given ";
    let got = labels("  on call() -> Effect[String] given ", doc, &files);
    assert!(got.contains(&"money.tokens.Jwt".to_string()), "{got:?}");
}

#[test]
fn the_view_resolves_an_alias_to_its_package_and_export_state() {
    let (root, files) = workspace("view");
    let view = DependencyView::from_files(&files).expect("the project has dependencies");
    let currency = view.resolve("money.currency").unwrap();
    assert_eq!(currency.package.name(), "acme/money");
    assert_eq!(currency.unit, "currency");
    assert_eq!(currency.canonical, "$acme_money.currency");
    assert!(currency.exported);
    assert!(!view.resolve("money.rounding").unwrap().exported);
    assert!(view.resolve("pricing").is_none());
    assert!(view.is_dependency_file(&root.join("../packages/money/src/currency.bynk")));
    assert!(!view.is_dependency_file(&root.join("src/pricing.bynk")));
    assert_eq!(
        bynk_ide::packages::hover_note(&currency),
        "From dependency `acme/money` (alias `money`)."
    );
}

#[test]
fn a_project_without_dependencies_completes_as_before() {
    let (root, mut files) = workspace("plain");
    files.insert(
        root.join("bynk.toml"),
        "[project]\nname = \"shop\"\n".to_string(),
    );
    assert!(DependencyView::from_files(&files).is_none());
    // With no graph, every file is the project's own, as it always was.
    let got = labels("uses ", "context shop.checkout\n\nuses ", &files);
    assert!(got.contains(&"currency".to_string()), "{got:?}");
}

#[test]
fn the_round_keys_a_dependency_unit_by_the_canonical_name_the_view_resolves() {
    // Go-to-definition on `uses money.currency` looks the written name up in
    // the round's `unit_sources`, then falls back to the view's canonical name.
    let (root, mut files) = workspace("definition");
    let books =
        "commons books\n\nuses money.currency\n\nfn twice(n: Int) -> Int {\n  double(n)\n}\n";
    write(&root.join("src/books.bynk"), books);
    files.insert(root.join("src/books.bynk"), books.to_string());
    let diags = bynk_ide::diagnose_project_with(
        &bynk_ide::AnalysisRoots::Project(root.clone()),
        &files,
        &bynk_project::SchemaLock::Off,
    );
    let view = DependencyView::for_root(&root, &files).unwrap();
    let target = view.resolve("money.currency").unwrap();
    assert!(!diags.unit_sources.contains_key("money.currency"));
    let rel = diags.unit_sources[&target.canonical].first().unwrap();
    assert_eq!(rel, &PathBuf::from("../packages/money/src/currency.bynk"));
    assert!(root.join(rel).exists());
    // The lookup go-to-definition makes: the written name, through the view.
    let found =
        bynk_ide::packages::unit_sources_for(&diags.unit_sources, Some(&view), "money.currency")
            .and_then(|v| v.first());
    assert_eq!(found, Some(rel));
    assert_eq!(
        bynk_ide::packages::unit_sources_for(&diags.unit_sources, None, "money.currency"),
        None
    );
    assert!(
        bynk_ide::packages::unit_sources_for(&diags.unit_sources, Some(&view), "books").is_some()
    );
}

#[test]
fn another_manifest_in_the_content_map_does_not_win() {
    // An open dependency manifest with dependencies of its own, which sorts
    // before the project's, and holds more sources.
    let (root, mut files) = workspace("rivals");
    let ws = root.parent().unwrap().to_path_buf();
    let money_manifest = "[package]\nname = \"acme/money\"\nexports = [\"currency\", \"tokens\"]\n\n[dependencies]\nfx = { package = \"acme/fx\", path = \"../fx\" }\n";
    write(&ws.join("packages/money/bynk.toml"), money_manifest);
    write(
        &ws.join("packages/fx/bynk.toml"),
        "[package]\nname = \"acme/fx\"\n",
    );
    write(&ws.join("packages/fx/src/rates.bynk"), "commons rates\n");
    files.insert(
        ws.join("packages/money/bynk.toml"),
        money_manifest.to_string(),
    );

    let view = DependencyView::from_files(&files).expect("the shop's view");
    assert!(view.resolve("money.currency").is_some());
    assert!(view.resolve("fx.rates").is_none());

    // With the root the language server knows, the same answer.
    let got: Vec<String> = bynk_ide::completion::complete_in(
        Some(&root),
        "uses ",
        "context shop.checkout\n\nuses ",
        Some(&files),
    )
    .into_iter()
    .map(|c| c.label)
    .collect();
    assert!(got.contains(&"money.currency".to_string()), "{got:?}");
    assert!(!got.iter().any(|l| l.starts_with("fx.")), "{got:?}");
}

#[test]
fn a_dotted_dependencies_key_is_recognised() {
    let (root, mut files) = workspace("dotted");
    // A top-level dotted key: it must precede the first table header.
    let manifest = "dependencies.money = { package = \"acme/money\", path = \"../packages/money\" }\n\n[project]\nname = \"shop\"\n";
    write(&root.join("bynk.toml"), manifest);
    files.insert(root.join("bynk.toml"), manifest.to_string());
    let view = DependencyView::from_files(&files).expect("a dotted key declares a dependency");
    assert!(view.resolve("money.currency").is_some());
}
