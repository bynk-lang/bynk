//! Drift guard: every file under `design/pending/` is valid against the format.
//!
//! This runs under `cargo nextest run --workspace` (the main `test` job, and the
//! local full gate) and under `cargo test -p xtask` (the `drift` job, for a
//! `design/pending/`-only PR that the `rust`-filtered `test` job would skip). So
//! a malformed pending file cannot reach `main` by either path.

#[test]
fn design_pending_files_are_valid() {
    match xtask::check_all() {
        Ok(_) => {}
        Err(errors) => panic!(
            "design/pending contains invalid pending-increment file(s):\n  {}\n\n\
             See design/pending/README.md for the format.",
            errors.join("\n  ")
        ),
    }
}

/// The committed ledger of stamped files (#4319) is well formed: sixteen-hex hashes, real
/// versions, and no file recorded twice. The stamp refuses a file only on a slug-and-hash match,
/// so a malformed line would silently stop protecting its slug.
#[test]
fn the_stamped_ledger_is_well_formed() {
    let root = xtask::repo_root();
    let text = std::fs::read_to_string(root.join(xtask::stamped::LEDGER_PATH))
        .expect("design/stamped.txt exists");
    let entries = xtask::stamped::parse(&text);
    let data_lines = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .count();
    assert_eq!(entries.len(), data_lines, "every ledger line parses");
    assert!(entries.len() > 1000, "the history was back-filled");
    let mut seen = std::collections::BTreeSet::new();
    for e in &entries {
        assert!(
            e.hash.len() == 16 && e.hash.chars().all(|c| c.is_ascii_hexdigit()),
            "{}: bad hash {:?}",
            e.slug,
            e.hash
        );
        assert!(
            xtask::stamp::Version::parse(&e.version).is_some(),
            "{}: bad version {:?}",
            e.slug,
            e.version
        );
        assert!(
            seen.insert((e.slug.clone(), e.hash.clone())),
            "{} is recorded twice",
            e.slug
        );
    }
}
