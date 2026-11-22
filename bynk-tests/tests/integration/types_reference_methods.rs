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
        .join("../site/src/content/docs/reference/types.md");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The part of `page` from the `heading` line to the next heading of the same level.
fn section<'a>(page: &'a str, heading: &str) -> &'a str {
    let start = page
        .find(&format!("\n{heading}\n"))
        .unwrap_or_else(|| panic!("reference/types.md has no `{heading}` heading"));
    let body = &page[start + 1..];
    let level = heading.split(' ').next().unwrap();
    let end = body[1..]
        .find(&format!("\n{level} "))
        .map_or(body.len(), |i| i + 1);
    &body[..end]
}

#[test]
fn the_types_reference_lists_every_kernel_method() {
    let page = types_reference();
    // Each receiver is checked only where its own methods are written, so a
    // method that several receivers share (`map`, `length`, `get`) is found on
    // the right row rather than on another receiver's. The tables write a
    // method on a named receiver (`s.trim()`, `xs.map(f)`); `Set` lists its
    // kernel in its own section as `insert(x)`.
    let set = section(&page, "## Set");
    let kernels: [(&str, &[KernelMethod], &[&str], &str); 6] = [
        ("String", StringOp::TABLE, &["`s."], &page),
        ("List", ListOp::TABLE, &["`xs.", "`rs."], &page),
        ("Map", MapOp::TABLE, &["`m."], &page),
        ("Set", SetOp::TABLE, &["`"], set),
        ("Option", OptionOp::TABLE, &["`o."], &page),
        ("Result", ResultOp::TABLE, &["`r."], &page),
    ];
    let missing: Vec<String> = kernels
        .iter()
        .flat_map(|(receiver, table, prefixes, text)| {
            table
                .iter()
                .filter(|m| {
                    !prefixes
                        .iter()
                        .any(|p| text.contains(&format!("{p}{}(", m.name)))
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
