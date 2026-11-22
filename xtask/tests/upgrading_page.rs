//! The Book's upgrading page (#3178) is written by the stamp; this keeps a hand
//! edit or a stamp bug from leaving it unordered, empty or ahead of the
//! changelog. Runs under `cargo test`, so CI enforces it with no workflow of
//! its own.

use std::path::Path;
use xtask::release_notes::{UPGRADING_PAGE, check_page};

#[test]
fn upgrading_page_is_consistent() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let page = std::fs::read_to_string(root.join(UPGRADING_PAGE)).unwrap();
    // The changelog is one page per release, plus Unreleased (#3868).
    let changelog = xtask::release_pages::all_rows(root).unwrap();
    let problems = check_page(&page, &changelog);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
