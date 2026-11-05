//! Keeps the generated CLI reference page in step with the clap definition.
//!
//! `site/src/content/docs/docs/cli.md` is rendered from `bynkc::cli` —
//! the very same command tree the binary parses with — so the page cannot
//! describe a CLI the binary does not have. (The page lives on the Developer
//! Documentation surface; its relative links still resolve against `reference/`,
//! where the generator authors them and where their targets remain.)
//!
//! Regenerate the docs page with:
//!     BYNK_BLESS=1 cargo test -p bynkc --test it cli_reference::

mod common;

use std::fs;
use std::path::PathBuf;

use bynkc::cli::{command, render_markdown};

#[test]
fn every_subcommand_is_documented() {
    let rendered = render_markdown();
    for sub in command().get_subcommands() {
        let name = sub.get_name();
        if name == "help" {
            continue;
        }
        assert!(
            rendered.contains(&format!("`bynkc {name}`")),
            "subcommand `{name}` is missing from the rendered CLI reference"
        );
    }
}

#[test]
fn generated_cli_page_is_up_to_date() {
    let page =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../site/src/content/docs/docs/cli.md");
    let rendered = common::to_site_page(&render_markdown(), "reference");

    if std::env::var_os("BYNK_BLESS").is_some() {
        fs::write(&page, &rendered).unwrap();
        return;
    }

    let current = fs::read_to_string(&page).unwrap_or_default();
    assert_eq!(
        current, rendered,
        "site/src/content/docs/docs/cli.md is out of date.\n\
         Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it cli_reference::"
    );
}
