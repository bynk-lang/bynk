//! The ratcheted maximum function length (#3167).
//!
//! A very long function is a place where shape and hygiene go unchecked: the
//! 2026-10-01 review measured `emit_agent` at 2,064 lines and `lower_method_call`
//! at 1,118. This module measures every non-test function in the workspace and
//! compares the ones over [`THRESHOLD`] lines against the committed ceilings in
//! `xtask/fn-length-ceilings.txt`.
//!
//! The rule is a ratchet, not a target: a function over the threshold may not grow
//! past its ceiling, no function may *newly* cross the threshold, and a ceiling
//! whose function has since shrunk below the threshold (or gone) must be removed.
//! A function that shrinks but stays over the threshold keeps its looser ceiling
//! until someone lowers it with `cargo xtask fn-lengths --apply`, which rewrites the
//! file from the tree. Lowering a ceiling is always allowed; raising one is the
//! review conversation this probe exists to force.
//!
//! The measurement reuses the greenfield probes' scanner, which skips braces inside
//! strings, character literals and comments, and the extents of `#[cfg(test)]`
//! modules.

use std::path::Path;

use crate::greenfield_status::{
    BraceScanState, brace_delta, in_test_range, rust_files, test_mod_ranges, top_level_crate_dirs,
};

/// Functions of at least this many lines (signature to closing brace) are tracked.
pub const THRESHOLD: usize = 300;

/// The committed ceilings, relative to the repository root.
pub const CEILINGS_FILE: &str = "xtask/fn-length-ceilings.txt";

/// One measured function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnLen {
    /// `crate/src/path.rs`, with forward slashes.
    pub file: String,
    pub name: String,
    pub lines: usize,
}

impl FnLen {
    /// The key a ceiling is recorded under: `file::name`.
    pub fn key(&self) -> String {
        format!("{}::{}", self.file, self.name)
    }
}

/// The name of the function a source line declares, if it declares one
/// (`pub(crate) async fn name<…>(…`), else `None`.
fn declared_fn_name(line: &str) -> Option<&str> {
    let mut rest = line.trim_start();
    loop {
        let before = rest;
        for prefix in ["pub ", "const ", "async ", "unsafe "] {
            if let Some(r) = rest.strip_prefix(prefix) {
                rest = r.trim_start();
            }
        }
        if let Some(r) = rest.strip_prefix("pub(")
            && let Some(end) = r.find(')')
        {
            rest = r[end + 1..].trim_start();
        }
        if rest == before {
            break;
        }
    }
    let after = rest.strip_prefix("fn ")?;
    let end = after
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(after.len());
    (end > 0).then(|| &after[..end])
}

/// Every function in `source` (non-test), as `(name, lines)`.
pub fn measure_source(source: &str) -> Vec<(String, usize)> {
    let lines: Vec<&str> = source.lines().collect();
    let tests = test_mod_ranges(&lines);
    struct Open {
        name: String,
        start: usize,
        depth_before: i32,
        opened: bool,
    }
    let mut open: Vec<Open> = Vec::new();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut state = BraceScanState::Normal;
    for (i, line) in lines.iter().enumerate() {
        let normal_at_start = state == BraceScanState::Normal;
        if normal_at_start
            && !in_test_range(i, &tests)
            && let Some(name) = declared_fn_name(line)
        {
            open.push(Open {
                name: name.to_string(),
                start: i,
                depth_before: depth,
                opened: false,
            });
        }
        let (delta, new_state) = brace_delta(line, state);
        state = new_state;
        depth += delta;
        let mut k = 0;
        while k < open.len() {
            let o = &mut open[k];
            if !o.opened && depth > o.depth_before {
                o.opened = true;
            }
            if o.opened && depth <= o.depth_before {
                let o = open.remove(k);
                out.push((o.name, i - o.start + 1));
                continue;
            }
            // A body opened and closed on one line (`fn f() {}`) never raises the depth
            // across the line.
            if !o.opened
                && depth == o.depth_before
                && line.contains('{')
                && line.trim_end().ends_with('}')
            {
                let o = open.remove(k);
                out.push((o.name, i - o.start + 1));
                continue;
            }
            // A declaration with no body (`fn f();`) never opens.
            if !o.opened && depth == o.depth_before && line.trim_end().ends_with(';') {
                open.remove(k);
                continue;
            }
            k += 1;
        }
    }
    out
}

/// Every non-test function in the workspace over [`THRESHOLD`] lines, longest first.
pub fn measure(root: &Path) -> Vec<FnLen> {
    let mut by_key: std::collections::BTreeMap<String, FnLen> = std::collections::BTreeMap::new();
    for krate in top_level_crate_dirs(root) {
        for (path, contents) in rust_files(&krate.join("src")) {
            let file = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            for (name, lines) in measure_source(&contents) {
                if lines < THRESHOLD {
                    continue;
                }
                let f = FnLen {
                    file: file.clone(),
                    name,
                    lines,
                };
                let slot = by_key.entry(f.key()).or_insert_with(|| f.clone());
                if f.lines > slot.lines {
                    *slot = f;
                }
            }
        }
    }
    let mut v: Vec<FnLen> = by_key.into_values().collect();
    v.sort_by(|a, b| b.lines.cmp(&a.lines).then_with(|| a.key().cmp(&b.key())));
    v
}

/// The committed ceilings: `file::name = lines`, one per line; `#` starts a comment.
pub fn read_ceilings(root: &Path) -> Vec<(String, usize)> {
    let text = std::fs::read_to_string(root.join(CEILINGS_FILE)).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let l = l.split('#').next().unwrap_or("").trim();
            let (key, n) = l.split_once('=')?;
            Some((key.trim().to_string(), n.trim().parse().ok()?))
        })
        .collect()
}

/// The ceilings file's text for a measurement: every function over the threshold,
/// at its current length.
pub fn render_ceilings(measured: &[FnLen]) -> String {
    let mut s = format!(
        "# Ratcheted maximum function length (#3167). `file::function = lines`, for every\n\
         # non-test function of at least {THRESHOLD} lines. A function may not grow past its\n\
         # ceiling, none may newly reach {THRESHOLD} lines, and an entry whose function has dropped\n\
         # below {THRESHOLD} (or gone) must be removed. Lower a ceiling after shrinking a function:\n\
         # `cargo xtask fn-lengths --apply` rewrites this file from the tree.\n"
    );
    for f in measured {
        s.push_str(&format!("{} = {}\n", f.key(), f.lines));
    }
    s
}

/// What is wrong, if anything: a function that grew past its ceiling, one that newly
/// crossed the threshold, and a ceiling that no longer names a function over it.
pub fn violations(measured: &[FnLen], ceilings: &[(String, usize)]) -> Vec<String> {
    let mut out = Vec::new();
    for f in measured {
        match ceilings.iter().find(|(k, _)| *k == f.key()) {
            None => out.push(format!(
                "{} is {} lines, over the {THRESHOLD}-line threshold, and has no ceiling: split it, \
                 or (if it must be this long) add it to {CEILINGS_FILE} and say why in review",
                f.key(),
                f.lines
            )),
            Some((_, ceiling)) if f.lines > *ceiling => out.push(format!(
                "{} grew to {} lines, past its ceiling of {ceiling}",
                f.key(),
                f.lines
            )),
            Some(_) => {}
        }
    }
    for (key, ceiling) in ceilings {
        if !measured.iter().any(|f| f.key() == *key) {
            out.push(format!(
                "{key} has a ceiling of {ceiling} but is no longer over {THRESHOLD} lines (or is gone): \
                 remove its entry from {CEILINGS_FILE}"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_function_is_measured_from_its_signature_to_its_closing_brace() {
        let src = "fn a() {\n    1\n}\n\npub(crate) fn b(\n    x: u8,\n) -> u8 {\n    x\n}\n";
        assert_eq!(
            measure_source(src),
            vec![("a".to_string(), 3), ("b".to_string(), 5)]
        );
    }

    #[test]
    fn braces_inside_strings_and_comments_do_not_end_a_function() {
        let src = "fn a() {\n    let s = \"}\";\n    // }\n    let c = '}';\n}\nfn b() {}\n";
        assert_eq!(
            measure_source(src),
            vec![("a".to_string(), 5), ("b".to_string(), 1)]
        );
    }

    #[test]
    fn a_nested_function_is_counted_by_itself_and_inside_its_parent() {
        let src = "fn outer() {\n    fn inner() {\n        1;\n    }\n}\n";
        let mut got = measure_source(src);
        got.sort();
        assert_eq!(
            got,
            vec![("inner".to_string(), 3), ("outer".to_string(), 5)]
        );
    }

    #[test]
    fn a_bodiless_declaration_and_a_test_module_are_ignored() {
        let src =
            "trait T {\n    fn f(&self);\n}\n#[cfg(test)]\nmod tests {\n    fn t() {\n    }\n}\n";
        assert!(measure_source(src).is_empty());
    }

    #[test]
    fn a_fn_inside_a_string_is_not_a_function() {
        let src = "const S: &str = \"\nfn not_real() {\n}\n\";\nfn real() {}\n";
        assert_eq!(measure_source(src), vec![("real".to_string(), 1)]);
    }

    #[test]
    fn growth_a_new_long_function_and_a_stale_entry_are_each_reported() {
        let m = |n: &str, l| FnLen {
            file: "a.rs".into(),
            name: n.into(),
            lines: l,
        };
        let measured = [m("grew", 400), m("fresh", 350), m("ok", 300)];
        let ceilings = vec![
            ("a.rs::grew".to_string(), 390),
            ("a.rs::ok".to_string(), 320),
            ("a.rs::gone".to_string(), 500),
        ];
        let v = violations(&measured, &ceilings);
        assert_eq!(v.len(), 3, "{v:?}");
        assert!(v.iter().any(|m| m.contains("a.rs::grew grew")));
        assert!(
            v.iter()
                .any(|m| m.contains("a.rs::fresh") && m.contains("no ceiling"))
        );
        assert!(
            v.iter()
                .any(|m| m.contains("a.rs::gone") && m.contains("remove"))
        );
        // A function that shrank but is still over the threshold is not a violation.
        assert!(violations(&[m("ok", 310)], &[("a.rs::ok".to_string(), 320)]).is_empty());
    }
}
