//! The ratcheted maximum function length (#3167): no non-test function may grow
//! past its committed ceiling in `xtask/fn-length-ceilings.txt`, none may newly
//! reach the threshold, and a ceiling must be removed when its function drops below
//! it. `cargo xtask fn-lengths` prints the measurement; `--apply` rewrites the file.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
fn no_function_grows_past_its_ceiling() {
    let root = repo_root();
    let measured = xtask::fn_length::measure(&root);
    let ceilings = xtask::fn_length::read_ceilings(&root);
    let problems = xtask::fn_length::violations(&measured, &ceilings);
    assert!(
        problems.is_empty(),
        "function-length ratchet (#3167):\n  {}\n\n`cargo xtask fn-lengths` lists every function over {} lines; \
         `--apply` rewrites {} from the tree (lowering ceilings only: do not raise one to make this pass)",
        problems.join("\n  "),
        xtask::fn_length::THRESHOLD,
        xtask::fn_length::CEILINGS_FILE,
    );
}
