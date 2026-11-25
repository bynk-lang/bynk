//! The Reference's marked lists stay alphabetical (#4206).
//!
//! A list opts in with `<!-- sorted: table -->` or `<!-- sorted: headings -->` ... `<!-- /sorted -->`;
//! `cargo xtask sorted-lists` runs the same check locally and in `cargo xtask ci --fast`. A pull
//! request that adds an entry out of order fails here with the entry and the one it should follow.
//! The pages are under `site/src/content/docs/reference`, which the `rust` CI filter covers.

use std::path::PathBuf;

#[test]
fn every_marked_list_is_in_alphabetical_order() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let problems = xtask::sorted_lists::check_site(&root);
    assert!(
        problems.is_empty(),
        "a list marked `<!-- sorted -->` is out of order. Move each whole entry (a table row, or a \
         heading with its section) to the place named; do not rewrite its text. Run \
         `cargo xtask sorted-lists` to check locally.\n  {}",
        problems.join("\n  ")
    );
}

/// The lists #4202 ordered are marked, so the check is guarding something. A page that loses its
/// markers would otherwise pass silently.
#[test]
fn the_ordered_reference_pages_carry_markers() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    for page in ["types.md", "glossary.md"] {
        let text = std::fs::read_to_string(root.join("site/src/content/docs/reference").join(page))
            .expect("read the page");
        assert!(
            text.contains("<!-- sorted: headings -->") && text.contains("<!-- /sorted -->"),
            "{page} lost its `<!-- sorted -->` markers"
        );
    }
}
