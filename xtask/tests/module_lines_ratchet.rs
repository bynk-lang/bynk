//! The ratcheted module size (#4085): no back-half source file may grow past its committed
//! ceiling in `xtask/module-line-ceilings.txt`, none may newly reach the threshold, a ceiling
//! must be removed when its file drops below it, and a new entry may not reach the track's
//! exit cap. `cargo xtask module-lines` prints the measurement; `--apply` rewrites the file.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
fn no_module_grows_past_its_ceiling() {
    let root = repo_root();
    let measured = xtask::module_lines::measure(&root);
    let ceilings = xtask::module_lines::read_ceilings(&root);
    let mut problems = xtask::module_lines::violations(&measured, &ceilings);
    problems.extend(xtask::module_lines::stale_grandfathered(&ceilings));
    problems.extend(xtask::module_lines::malformed_ceiling_lines(&root));
    assert!(
        problems.is_empty(),
        "module-size ratchet (#4085):\n  {}\n\n`cargo xtask module-lines` lists every back-half file over {} production lines; \
         `--apply` rewrites {} from the tree (it also lowers ceilings after a file shrinks). \
         Prefer splitting the file along a seam to raising its ceiling.",
        problems.join("\n  "),
        xtask::module_lines::THRESHOLD,
        xtask::module_lines::CEILINGS_FILE,
    );
}

#[test]
fn every_ceiling_key_names_an_existing_file() {
    let root = repo_root();
    for (key, _) in xtask::module_lines::read_ceilings(&root) {
        assert!(!key.contains('\\'), "{key}: keys use `/` on every platform");
        assert!(root.join(&key).is_file(), "{key} names no file");
    }
}
