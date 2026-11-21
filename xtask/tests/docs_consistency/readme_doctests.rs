//! The README-doctest drift guard (#1645).
//!
//! A crate README's `rust` examples are compiled as doctests: the crate's
//! `lib.rs` includes the README under `cfg(doctest)`, and the
//! `cargo readme-doctests` alias in `.cargo/config.toml` runs exactly the
//! crates that have one. This test keeps the three in step:
//!
//! - every workspace crate whose README has a `rust` block is in the alias, and
//!   its `lib.rs` includes the README; every crate in the alias has one; and
//! - no README opens an unlabelled fence, which rustdoc would compile as Rust.
//!
//! It reads `../*/README.md`, outside the `rust` CI path filter, so the `drift`
//! job runs it (and the alias) for a README-only PR, as for `readme_layering`.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The workspace members that carry a `README.md`.
fn members_with_readmes() -> Vec<String> {
    let manifest: toml::Table = read("Cargo.toml").parse().expect("Cargo.toml parses");
    manifest["workspace"]["members"]
        .as_array()
        .expect("workspace.members is an array")
        .iter()
        .map(|m| m.as_str().expect("a member is a string").to_string())
        .filter(|m| workspace_root().join(m).join("README.md").is_file())
        .collect()
}

/// Each fence-opening line's info string (`rust`, `text`, empty for a bare fence).
fn opening_fences(readme: &str) -> Vec<String> {
    let mut open = false;
    let mut infos = Vec::new();
    for line in readme.lines() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        if !open {
            infos.push(info.trim().to_string());
        }
        open = !open;
    }
    infos
}

fn is_rust(info: &str) -> bool {
    info.split(',').next().map(str::trim) == Some("rust")
}

/// The `-p` crates of the `readme-doctests` alias.
fn alias_crates() -> BTreeSet<String> {
    let config: toml::Table = read(".cargo/config.toml")
        .parse()
        .expect(".cargo/config.toml parses");
    let alias = config["alias"]["readme-doctests"]
        .as_str()
        .expect("the `readme-doctests` alias is a string");
    let words: Vec<&str> = alias.split_whitespace().collect();
    words
        .windows(2)
        .filter(|w| w[0] == "-p")
        .map(|w| w[1].to_string())
        .collect()
}

#[test]
fn every_readme_with_a_rust_example_is_doctested() {
    let with_rust: BTreeSet<String> = members_with_readmes()
        .into_iter()
        .filter(|m| {
            opening_fences(&read(&format!("{m}/README.md")))
                .iter()
                .any(|info| is_rust(info))
        })
        .collect();
    assert_eq!(
        with_rust,
        alias_crates(),
        "the `readme-doctests` alias in .cargo/config.toml must name exactly the \
         crates whose README has a `rust` block"
    );
    for krate in &with_rust {
        let lib = read(&format!("{krate}/src/lib.rs"));
        assert!(
            lib.contains("#[cfg(doctest)]\n#[doc = include_str!(\"../README.md\")]"),
            "{krate}/src/lib.rs must include its README under `cfg(doctest)` so \
             its `rust` examples compile"
        );
    }
}

#[test]
fn no_readme_opens_an_unlabelled_fence() {
    for member in members_with_readmes() {
        let infos = opening_fences(&read(&format!("{member}/README.md")));
        assert!(
            infos.iter().all(|info| !info.is_empty()),
            "{member}/README.md opens a fence with no language; rustdoc would \
             compile it as Rust, so label it (`text`, `sh`, …)"
        );
    }
}
