//! v0.25 (ADR 0053): the project-wide binding index — binding-correct
//! use→def edges for every in-scope symbol kind, assembled on the v0.24
//! project pass. The fixture matrix exercises every must-capture kind from
//! the proposal: cross-file types (annotation and static-receiver position),
//! fns, capabilities (bare `given`, dotted `B.Cap`, flattened
//! `consumes U { Cap }`), clause-list occurrences (`exports`, `consumes`
//! selection), cross-context service calls, and test-unit references — and
//! proves same-named symbols in different units are not conflated.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_check::index::{ProjectIndex, SymbolKey, SymbolKind};

fn smoke_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/110_full_orders_payment_tests/src")
}

fn analyse(root: &Path) -> (ProjectIndex, HashMap<String, String>) {
    analyse_roots(&bynk_ide::AnalysisRoots::SingleTree(root.to_path_buf()))
}

fn analyse_roots(roots: &bynk_ide::AnalysisRoots) -> (ProjectIndex, HashMap<String, String>) {
    analyse_roots_expecting(roots, &[])
}

/// [`analyse_roots`] for a project that reports exactly `expected` (by
/// category, in order across files): the index is still built for a project
/// with errors, as the editor sees one mid-edit.
fn analyse_roots_expecting(
    roots: &bynk_ide::AnalysisRoots,
    expected: &[&str],
) -> (ProjectIndex, HashMap<String, String>) {
    let result = bynk_ide::diagnose_project_with(
        roots,
        &bynk_testkit::read_project_sources(roots),
        &bynk_ide::SchemaLock::Off,
    );
    let reported: Vec<&str> = result
        .files
        .iter()
        .flat_map(|f| f.diagnostics.iter().map(|d| d.error.category))
        .collect();
    assert_eq!(reported, expected, "diagnostics reported by the fixture");
    let snapshots = result
        .files
        .iter()
        .map(|f| {
            (
                f.source_path.to_string_lossy().replace('\\', "/"),
                f.text.clone(),
            )
        })
        .collect();
    (result.index, snapshots)
}

fn key(unit: &str, kind: SymbolKind, name: &str) -> SymbolKey {
    SymbolKey {
        unit: unit.to_string(),
        kind,
        name: name.to_string(),
    }
}

/// Every recorded span must cover exactly the symbol's name segment.
fn assert_sites_spell(
    index: &ProjectIndex,
    snapshots: &HashMap<String, String>,
    k: &SymbolKey,
    expected_at: &[&str],
) {
    let entry = index
        .symbols
        .get(k)
        .unwrap_or_else(|| panic!("symbol {k:?} missing from index"));
    let def = entry.def.as_ref().expect("def site");
    let mut seen_files: Vec<String> = Vec::new();
    for site in std::iter::once(def).chain(entry.refs.iter()) {
        let path = site.path.to_string_lossy().replace('\\', "/");
        let text = snapshots
            .get(&path)
            .unwrap_or_else(|| panic!("no snapshot for {path}"));
        let segment = &text[site.span.range()];
        assert_eq!(
            segment, k.name,
            "site span in {path} must cover the name segment only"
        );
        seen_files.push(path);
    }
    for f in expected_at {
        assert!(
            seen_files.iter().any(|s| s == f),
            "{k:?}: expected a site in {f}, saw {seen_files:?}"
        );
    }
}

fn matrix_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/index/refproject/src")
}

fn refs_in<'a>(
    index: &'a ProjectIndex,
    k: &SymbolKey,
    file: &str,
) -> Vec<&'a bynk_check::index::SiteRef> {
    index.symbols[k]
        .refs
        .iter()
        .filter(|s| s.path.to_string_lossy().replace('\\', "/") == file)
        .collect()
}

#[test]
fn cross_file_type_and_fn_resolve_to_one_definition() {
    let (index, snapshots) = analyse(&matrix_root());

    // Type: annotation position in its own file and cross-file (billing's
    // `fn fee(m: Money)`), static-receiver position (`Money.of`).
    let money = key("shop.util", SymbolKind::Type, "Money");
    assert_sites_spell(
        &index,
        &snapshots,
        &money,
        &["shop/util.bynk", "shop/billing.bynk"],
    );
    // Static-receiver: `Money.of(x)` records the receiver segment, so
    // util.bynk carries both the annotation and the receiver refs.
    assert!(
        refs_in(&index, &money, "shop/util.bynk").len() >= 2,
        "annotation + static-receiver refs in util.bynk"
    );

    // Qualified variant receiver: `Status.Open` references `Status`.
    assert_sites_spell(
        &index,
        &snapshots,
        &key("shop.util", SymbolKind::Type, "Status"),
        &["shop/util.bynk"],
    );

    // Fn: called cross-file (billing) and from the test unit's own
    // `double(2)` call. P5.4 (`design/tracks/semantics-in-the-checker.md`
    // §6): closed the residual gap's category 7 —
    // `bynk_check::analysis::analyse_project` (which `bynk-ide` now calls)
    // now runs `bynk_check::test_suites::phase_test_bodies` (the relocated
    // checking half of `process_tests`), so a `.bynk` suite body's bindings
    // reach `index` again too. Formerly asserted absent (P4.2, #1122,
    // `design/tracks/project-model.md` §3.3(a)); folded into the ordinary
    // `assert_sites_spell` site list now that the gap has closed.
    let double = key("shop.util", SymbolKind::Fn, "double");
    assert_sites_spell(
        &index,
        &snapshots,
        &double,
        &[
            "shop/util.bynk",
            "shop/billing.bynk",
            "tests/billing.test.bynk",
        ],
    );
}

#[test]
fn same_named_symbols_in_different_units_are_not_conflated() {
    let (index, _) = analyse(&matrix_root());

    // `other.util` declares its own `Money` and `double`; nothing imports
    // it, so neither carries any reference — and the `shop.util` symbols'
    // references never land in other/util.bynk.
    let other_money = key("other.util", SymbolKind::Type, "Money");
    let other_double = key("other.util", SymbolKind::Fn, "double");
    assert!(index.symbols[&other_money].refs.is_empty());
    assert!(index.symbols[&other_double].refs.is_empty());

    for k in [
        key("shop.util", SymbolKind::Type, "Money"),
        key("shop.util", SymbolKind::Fn, "double"),
    ] {
        assert!(
            refs_in(&index, &k, "other/util.bynk").is_empty(),
            "{k:?} must not pick up other.util's same-named declaration sites"
        );
    }
}

#[test]
fn capability_references_cover_every_clause_and_call_form() {
    let (index, snapshots) = analyse(&matrix_root());
    let pay = key("shop.billing", SymbolKind::Capability, "Pay");

    // Every site spells exactly `Pay` — dotted `shop.billing.Pay` and
    // flattened forms record the name segment only.
    assert_sites_spell(
        &index,
        &snapshots,
        &pay,
        &["shop/billing.bynk", "shop/checkout.bynk", "ops/audit.bynk"],
    );

    // Declaring context: `exports capability { Pay }`, `provides Pay`,
    // `given Pay`, `Pay.charge(...)`.
    assert_eq!(refs_in(&index, &pay, "shop/billing.bynk").len(), 4);
    // Flattening consumer: `consumes shop.billing { Pay }` selection,
    // `given Pay` (flattened bare), `Pay.charge(...)`.
    assert_eq!(refs_in(&index, &pay, "shop/checkout.bynk").len(), 3);
    // Dotted consumer: `given shop.billing.Pay`, `shop.billing.Pay.charge`.
    assert_eq!(refs_in(&index, &pay, "ops/audit.bynk").len(), 2);
}

#[test]
fn cross_context_service_call_and_type_export_clause_are_indexed() {
    let (index, snapshots) = analyse(&matrix_root());

    // `shop.billing.bill(3)` in ops/audit.bynk references the service, as
    // does the test body's `bill.call(d)` — same P5.4 closure (category 7)
    // as `double`'s call above; see that assertion's comment for the full
    // explanation.
    let bill = key("shop.billing", SymbolKind::Service, "bill");
    assert_sites_spell(
        &index,
        &snapshots,
        &bill,
        &[
            "shop/billing.bynk",
            "ops/audit.bynk",
            "tests/billing.test.bynk",
        ],
    );

    // `exports transparent { Receipt }` is a reference to the type.
    let receipt = key("shop.billing", SymbolKind::Type, "Receipt");
    assert_sites_spell(&index, &snapshots, &receipt, &["shop/billing.bynk"]);
    assert!(
        refs_in(&index, &receipt, "shop/billing.bynk").len() >= 3,
        "exports clause + return annotation + construction"
    );
}

#[test]
fn smoke_existing_fixture_indexes_cleanly() {
    let (index, snapshots) = analyse(&smoke_root());
    // The consumed context's exported type is referenced from the consuming
    // context (the `place` service's return type).
    assert_sites_spell(
        &index,
        &snapshots,
        &key("commerce.payment", SymbolKind::Type, "AuthId"),
        &["commerce/payment.bynk", "commerce/orders.bynk"],
    );
    // The capability declared and provided in the payment context.
    assert_sites_spell(
        &index,
        &snapshots,
        &key("commerce.payment", SymbolKind::Capability, "Logger"),
        &["commerce/payment.bynk"],
    );
}

/// #1883: an asset is a symbol like a free function — its declaration is the
/// def site, and each bare-name use is a ref, resolved through `uses` (so
/// rename, references and go-to-definition reach it across files).
#[test]
fn asset_references_resolve_to_their_declaration() {
    // Manifest-resolved roots, as the editor analyses a project: an asset
    // names a file in the `assets/` folder beside `bynk.toml`, not in `src/`.
    let (index, snapshots) = analyse_roots(&bynk_ide::AnalysisRoots::Project(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/1883_asset_workers"),
    ));

    let style = key("compat.page", SymbolKind::Asset, "style");
    assert_sites_spell(
        &index,
        &snapshots,
        &style,
        &["src/compat/page.bynk", "src/compat/board.bynk"],
    );
    assert_eq!(refs_in(&index, &style, "src/compat/board.bynk").len(), 1);

    let palette = key("compat.theme", SymbolKind::Asset, "palette");
    assert_sites_spell(
        &index,
        &snapshots,
        &palette,
        &["src/compat/theme.bynk", "src/compat/page.bynk"],
    );
}

/// #2387: an embed is a symbol like an asset — its declaration is the def site
/// and each bare-name use, across a `uses` edge too, is a ref.
#[test]
fn embed_references_resolve_to_their_declaration() {
    let (index, snapshots) = analyse_roots(&bynk_ide::AnalysisRoots::Project(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/2387_embed_unit"),
    ));
    let shell = key("t.page", SymbolKind::Embed, "shell");
    assert_sites_spell(
        &index,
        &snapshots,
        &shell,
        &["src/t/page.bynk", "src/t/web.bynk"],
    );
    assert_eq!(refs_in(&index, &shell, "src/t/web.bynk").len(), 1);
}

/// #1883 (review of #1924): an asset's def must not register as an *owner*.
/// Owners are keyed by (unit, name) alone, so an asset named like a service
/// in the same context would take over the service's owner entry, and every
/// ref inside the service's handlers would be re-attributed to the asset's
/// file — at offsets that belong to the service's.
///
/// Since #1925 the program is `bynk.resolve.name_conflict`, but the editor
/// indexes it as typed, so the refs must still land in the right file.
#[test]
fn an_asset_named_like_a_service_does_not_steal_its_refs() {
    let root =
        std::env::temp_dir().join(format!("bynk-refindex-asset-owner-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let files = [
        (
            "bynk.toml",
            "[project]\nname = \"owner\"\nversion = \"0.1.0\"\n",
        ),
        (
            "src/app/a_web.bynk",
            "context app\n\nfn greet() -> String {\n  \"hi\"\n}\n\nservice api from http {\n  on GET(\"/\") () -> Effect[HttpResult[String]] by Visitor {\n    Ok(greet())\n  }\n}\n",
        ),
        (
            "src/app/z_files.bynk",
            "context app\n\nasset api = \"assets/api.json\"\n",
        ),
        ("assets/api.json", "{}"),
    ];
    for (rel, text) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
    }
    let (index, snapshots) = analyse_roots_expecting(
        &bynk_ide::AnalysisRoots::Project(root.clone()),
        &["bynk.resolve.name_conflict"],
    );
    let _ = std::fs::remove_dir_all(&root);

    // `greet()` in the handler is a ref in the service's file, spelling `greet`.
    let greet = key("app", SymbolKind::Fn, "greet");
    assert_sites_spell(&index, &snapshots, &greet, &["src/app/a_web.bynk"]);
    assert_eq!(refs_in(&index, &greet, "src/app/a_web.bynk").len(), 1);
    // The asset is still a symbol in its own right.
    assert!(
        index
            .symbols
            .contains_key(&key("app", SymbolKind::Asset, "api"))
    );
}
