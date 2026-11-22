//! The structural guard of Decision I of #3928 (back-half paydown 1B.1c): the
//! event failure-isolation code has one home.
//!
//! ADR 0284: a subscriber's throw is logged and does not stop its siblings or
//! reach the committed publisher. Both targets build that statement with
//! `bynk-emit`'s `compose::events::isolated_delivery`. The duplicate came back
//! once after #1327, as a hand-written copy in the Bundle root that "mirrored"
//! the Durable Object's, so this counts the one string only that statement
//! logs, in production Rust source: more than one occurrence means a second copy
//! of the isolation code was written.

use std::path::{Path, PathBuf};

use crate::greenfield_status::{in_test_range, test_mod_ranges};

/// The message the isolation statement logs. Spelled in two halves so this file
/// is not itself an occurrence.
pub fn marker() -> String {
    ["EventsFanout delivery", " failed"].concat()
}

/// Every line of production Rust source under each crate's `src` (this crate
/// aside, which only names the marker) that holds `needle`, outside
/// `#[cfg(test)]` modules, as `path:line`.
pub fn occurrences(root: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    let mut crates: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("src").is_dir() && p.file_name().is_some_and(|n| n != "xtask"))
        .collect();
    crates.sort();
    for krate in crates {
        scan(&krate.join("src"), root, needle, &mut found);
    }
    found
}

fn scan(dir: &Path, root: &Path, needle: &str, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            scan(&path, root, needle, found);
        } else if path.extension().is_some_and(|e| e == "rs")
            && let Ok(source) = std::fs::read_to_string(&path)
        {
            let rel = path.strip_prefix(root).unwrap_or(&path).display();
            found.extend(
                occurrences_in(&source, needle)
                    .into_iter()
                    .map(|line| format!("{rel}:{line}")),
            );
        }
    }
}

/// The 1-based lines of `source` that hold `needle` outside `#[cfg(test)]` modules.
pub fn occurrences_in(source: &str, needle: &str) -> Vec<usize> {
    let lines: Vec<&str> = source.lines().collect();
    let tests = test_mod_ranges(&lines);
    lines
        .iter()
        .enumerate()
        .filter(|(i, l)| l.contains(needle) && !in_test_range(*i, &tests))
        .map(|(i, _)| i + 1)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_module_copy_is_not_an_occurrence_but_a_production_one_is() {
        let needle = marker();
        let source = format!(
            "fn a() {{ log(\"{needle}\"); }}\n#[cfg(test)]\nmod tests {{\n    fn t() {{ log(\"{needle}\"); }}\n}}\nfn b() {{ log(\"{needle}\"); }}\n"
        );
        assert_eq!(occurrences_in(&source, &needle), [1, 6]);
    }
}
