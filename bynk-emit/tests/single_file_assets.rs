//! #1883: the single-file path checks `asset` declarations with the same rules
//! as the project pipeline (review of #1903), and refuses to build one:
//! `bynk.asset.needs_project`, since a single file has no project root to find
//! `assets/` under and emits no Worker to serve it beside. `bynk check
//! page.bynk` goes through `check_with_warnings`; `bynk compile page.bynk`
//! through `compile_with_warnings`.

const PAGE: &str = "commons compat.page\n\nasset style = \"assets/board.css\"\n\nfn sheet() -> String {\n  style.url\n}\n";

fn categories(errors: &[bynk_syntax::CompileError]) -> Vec<&'static str> {
    errors.iter().map(|e| e.category).collect()
}

#[test]
fn a_single_file_check_accepts_an_asset_and_a_build_refuses_it() {
    let warnings = bynk_emit::check_with_warnings(PAGE, "page.bynk")
        .unwrap_or_else(|e| panic!("check should pass, got {:?}", categories(&e)));
    assert!(warnings.is_empty(), "{:?}", categories(&warnings));

    let errors = match bynk_emit::compile_with_warnings(PAGE, "page.bynk") {
        Ok(_) => panic!("a single-file build of an asset must be refused"),
        Err(e) => e,
    };
    assert_eq!(categories(&errors), vec!["bynk.asset.needs_project"]);
}

#[test]
fn a_single_file_check_applies_the_declaration_rules() {
    for (src, code) in [
        (
            "commons c\n\nasset style = \"assets/a.css\"\nasset style = \"assets/b.css\"\n",
            "bynk.asset.duplicate",
        ),
        (
            "commons c\n\nfn style() -> Int {\n  1\n}\n\nasset style = \"assets/a.css\"\n",
            "bynk.resolve.name_conflict",
        ),
        (
            "commons c\n\ntype Style = Int\n\nasset Style = \"assets/a.css\"\n",
            "bynk.resolve.name_conflict",
        ),
        (
            "commons c\n\nasset style = \"../x.css\"\n",
            "bynk.asset.unsafe_path",
        ),
        (
            "commons c\n\nasset env = \".env\"\n",
            "bynk.asset.outside_assets_dir",
        ),
    ] {
        let errors = match bynk_emit::check_with_warnings(src, "c.bynk") {
            Ok(_) => panic!("expected `{code}` for:\n{src}"),
            Err(e) => e,
        };
        assert!(
            categories(&errors).contains(&code),
            "expected `{code}`, got {:?} for:\n{src}",
            categories(&errors)
        );
        assert!(
            !categories(&errors).contains(&"bynk.asset.needs_project"),
            "a check must not report the build-only refusal"
        );
    }
}
