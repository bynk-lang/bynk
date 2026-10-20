//! #2223 follow-up: hovering a `uses` target written `<alias>.<unit>` shows
//! the dependency unit's own module doc, and the package and alias it comes
//! through — not nothing, as when the written name matched no declaration.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use bynk_lsp::hover;
use tower_lsp::lsp_types::Url;

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn a_dependency_uses_target_hovers_with_its_package() {
    let ws = std::env::temp_dir().join(format!("bynk-lsp-dep-hover-{}", std::process::id()));
    let _ = fs::remove_dir_all(&ws);
    write(
        &ws.join("packages/money/bynk.toml"),
        "[package]\nname = \"acme/money\"\nexports = [\"currency\"]\n",
    );
    let currency =
        "--- Money in minor units.\ncommons currency\n\nfn double(c: Int) -> Int {\n  c * 2\n}\n";
    write(&ws.join("packages/money/src/currency.bynk"), currency);
    let rounding =
        "--- Rounding helpers.\ncommons rounding\n\nfn half(n: Int) -> Int {\n  n / 2\n}\n";
    write(&ws.join("packages/money/src/rounding.bynk"), rounding);
    let root = ws.join("shop");
    let manifest = "[project]\nname = \"shop\"\n\n[dependencies]\nmoney = { package = \"acme/money\", path = \"../packages/money\" }\n";
    write(&root.join("bynk.toml"), manifest);
    let text = "commons pricing\n\nuses money.currency\nuses money.rounding\n\nfn total(n: Int) -> Int {\n  double(n)\n}\n";
    write(&root.join("src/pricing.bynk"), text);

    let files = HashMap::from([
        (root.join("bynk.toml"), manifest.to_string()),
        (
            ws.join("packages/money/src/currency.bynk"),
            currency.to_string(),
        ),
        (
            ws.join("packages/money/src/rounding.bynk"),
            rounding.to_string(),
        ),
    ]);
    let uri = Url::from_file_path(root.join("src/pricing.bynk")).unwrap();
    let hover_at = |needle: &str| {
        let offset = text.find(needle).unwrap() + "uses money.".len();
        hover::hover_content(&hover::HoverInput {
            analysis: None,
            doc: Some((text, offset)),
            uri: &uri,
            files: Some(&files),
        })
    };

    let got = hover_at("uses money.currency").expect("a hover");
    assert!(got.contains("commons currency"), "{got}");
    assert!(got.contains("Money in minor units."), "{got}");
    assert!(
        got.contains("From dependency `acme/money` (alias `money`)."),
        "{got}"
    );

    let got = hover_at("uses money.rounding").expect("a hover");
    assert!(got.contains("Rounding helpers."), "{got}");
    assert!(
        got.contains("Not exported: `bynk check` refuses this reference."),
        "{got}"
    );
    let _ = fs::remove_dir_all(&ws);
}
