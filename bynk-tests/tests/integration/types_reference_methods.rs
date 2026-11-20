//! The types reference lists every built-in method (#3827).
//!
//! `reference/types.md` carries a method table for `String`, `List`, `Map`,
//! `Set`, `Option` and `Result`. This gate reads the checker's own kernel tables
//! (`bynk_check::kernel_op`) and fails when the reference misses one of their
//! methods, so a method added to the checker cannot go undocumented.

use std::fs;
use std::path::PathBuf;

use bynk_check::kernel_op::{KernelMethod, ListOp, MapOp, OptionOp, ResultOp, SetOp, StringOp};

fn types_reference() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../site/src/content/docs/book/reference/types.md");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_types_reference_lists_every_kernel_method() {
    let page = types_reference();
    let kernels: [(&str, &[KernelMethod]); 6] = [
        ("String", StringOp::TABLE),
        ("List", ListOp::TABLE),
        ("Map", MapOp::TABLE),
        ("Set", SetOp::TABLE),
        ("Option", OptionOp::TABLE),
        ("Result", ResultOp::TABLE),
    ];
    let missing: Vec<String> = kernels
        .iter()
        .flat_map(|(receiver, table)| {
            table
                .iter()
                // A method appears as code: `x.name(…)` in a table, or `name(…)`
                // in a list of them.
                .filter(|m| {
                    !page.contains(&format!(".{}(", m.name))
                        && !page.contains(&format!("`{}(", m.name))
                })
                .map(move |m| format!("{receiver}.{}", m.name))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "reference/types.md does not list these kernel methods: {}",
        missing.join(", ")
    );
}
