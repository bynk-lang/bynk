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
    let changelog =
        std::fs::read_to_string(root.join("site/src/content/docs/book/reference/changelog.md"))
            .unwrap();
    let problems = check_page(&page, &changelog);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
