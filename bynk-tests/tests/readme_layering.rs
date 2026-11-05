//! The crate-layering diagram drift guard.
//!
//! Every library crate's README carries the same "Where it sits" diagram: a
//! tree of the compiler's library crates in which each crate depends on its
//! parent, plus the crates listed after its `+`. A crate with no in-workspace
//! dependency (`bynk-std`) is drawn as a second root. It is duplicated by hand
//! across thirteen READMEs, and its predecessor drifted silently — it once placed
//! `bynk-ide` above `bynk-emit` (no such edge exists) and omitted six crates.
//! This test makes both properties a CI contract:
//!
//! - the diagram block is byte-identical in every library README, and
//! - it agrees with each crate's own `Cargo.toml` `[dependencies]`: the parent
//!   and every `+` crate are real dependencies, and every other in-workspace
//!   dependency is an ancestor in the tree (implied by the layering, so not
//!   restated). The tree names exactly the library crates, no more, no fewer.
//!
//! It reads `../bynk-*/README.md`, which lives OUTSIDE the `rust` CI path
//! filter, so a README-only PR would skip the main `test` job — the `drift`
//! job in `.github/workflows/ci.yml` runs this guard in exactly that case (the
//! same arrangement as `decisions_index`).

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// The library crates whose READMEs carry the diagram — every crate in the
/// tree. The front-ends (`bynkc`, `bynk`, `bynk-lsp`, `bynk-wasm`) sit on top
/// of it and are described in prose, not drawn.
const LIBRARY_CRATES: &[&str] = &[
    "bynk-syntax",
    "bynk-project",
    "bynk-ts",
    "bynk-render",
    "bynk-fmt",
    "bynk-check",
    "bynk-ir",
    "bynk-lower",
    "bynk-emit",
    "bynk-strip",
    "bynk-driver",
    "bynk-ide",
    "bynk-std",
];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The fenced `text` block whose first line starts with `bynk-syntax`.
fn diagram(krate: &str) -> String {
    let path = workspace_root().join(krate).join("README.md");
    let readme =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let start = readme
        .find("```text\nbynk-syntax ")
        .unwrap_or_else(|| panic!("{krate}/README.md has no crate-layering diagram"));
    let body = &readme[start + "```text\n".len()..];
    let end = body
        .find("```")
        .unwrap_or_else(|| panic!("{krate}/README.md: unterminated diagram"));
    body[..end].to_string()
}

/// One diagram row: the crate, its column (the tree depth), and its `+` crates.
struct Row {
    krate: String,
    column: usize,
    extras: BTreeSet<String>,
}

fn parse_rows(diagram: &str) -> Vec<Row> {
    diagram
        .lines()
        .map(|line| {
            let column = line.find("bynk-").expect("every diagram row names a crate");
            let krate: String = line[column..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            let extras = match line.find("+ ") {
                Some(at) => line[at + 2..]
                    .split(',')
                    .map(|short| format!("bynk-{}", short.trim()))
                    .collect(),
                None => BTreeSet::new(),
            };
            // Columns are counted in chars: the tree-drawing glyphs are multi-byte.
            let column = line[..column].chars().count();
            Row {
                krate,
                column,
                extras,
            }
        })
        .collect()
}

/// The `bynk-*` keys of a crate's `[dependencies]` table (not dev-dependencies).
fn bynk_dependencies(krate: &str) -> BTreeSet<String> {
    let path = workspace_root().join(krate).join("Cargo.toml");
    let manifest =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut in_dependencies = false;
    let mut deps = BTreeSet::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_dependencies = line == "[dependencies]";
        } else if in_dependencies && line.starts_with("bynk-") {
            let key: String = line
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            deps.insert(key);
        }
    }
    deps
}

#[test]
fn the_diagram_is_identical_in_every_library_readme() {
    let canonical = diagram(LIBRARY_CRATES[0]);
    let drifted: Vec<&str> = LIBRARY_CRATES[1..]
        .iter()
        .copied()
        .filter(|krate| diagram(krate) != canonical)
        .collect();
    assert!(
        drifted.is_empty(),
        "these READMEs' crate-layering diagram differs from bynk-syntax/README.md's \
         (the block must be byte-identical everywhere): {drifted:?}"
    );
}

#[test]
fn the_diagram_matches_every_crates_dependencies() {
    let rows = parse_rows(&diagram(LIBRARY_CRATES[0]));

    let drawn: BTreeSet<&str> = rows.iter().map(|r| r.krate.as_str()).collect();
    let expected: BTreeSet<&str> = LIBRARY_CRATES.iter().copied().collect();
    assert_eq!(
        drawn, expected,
        "the diagram must name exactly the library crates"
    );

    let mut problems = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        // Ancestors: walking up, each row at a strictly smaller column than the
        // last one taken. The first of them is the parent.
        let mut ancestors = Vec::new();
        let mut column = row.column;
        for above in rows[..i].iter().rev() {
            if above.column < column {
                ancestors.push(above.krate.clone());
                column = above.column;
            }
        }
        let deps = bynk_dependencies(&row.krate);
        if let Some(parent) = ancestors.first()
            && !deps.contains(parent)
        {
            problems.push(format!(
                "{}: drawn under {parent}, but does not depend on it",
                row.krate
            ));
        }
        for extra in &row.extras {
            if !deps.contains(extra) {
                problems.push(format!(
                    "{}: lists `+ {extra}`, but does not depend on it",
                    row.krate
                ));
            }
        }
        for dep in &deps {
            if !row.extras.contains(dep) && !ancestors.contains(dep) {
                problems.push(format!(
                    "{}: depends on {dep}, which is neither an ancestor in the tree nor listed after its `+`",
                    row.krate
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "the crate-layering diagram has drifted:\n  {}",
        problems.join("\n  ")
    );
}
