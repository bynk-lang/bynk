//! The module-size ratchet (#4085, slice 2.0 of the back-half paydown track, spine #3676).
//!
//! Q5 of the track: no source file may *newly* cross [`THRESHOLD`] production lines, and an
//! existing one may not grow. This module measures production lines per `src/**/*.rs` file
//! (comments and blank lines count; `#[cfg(test)]` modules, whole-test files and files
//! declared away by `#[cfg(test)] mod name;` do not) and compares the back-half crates'
//! files ([`BACK_HALF_CRATES`]) at or over the threshold against the committed ceilings in
//! `xtask/module-line-ceilings.txt`. The `module_lines` trend probe reads the same
//! measure, workspace-wide, so the trend and the gate share one definition.
//!
//! The rule is a ratchet in the way the function-length ratchet is ([`crate::fn_length`]):
//!
//! 1. a file may not grow past its ceiling;
//! 2. a file not listed may not reach [`THRESHOLD`];
//! 3. an entry whose file is under [`THRESHOLD`], or gone, or renamed, must be deleted;
//! 4. a file that shrinks but stays over keeps its looser ceiling until
//!    `cargo xtask module-lines --apply` rewrites the file from the tree.
//!
//! The track's exit cap is [`EXIT_CAP`] production lines. It binds a *new* entry now: an
//! entry at or over it must be named in [`GRANDFATHERED`], the files that were already over
//! it when the ratchet landed. At track exit the list is empty and the constant goes.
//!
//! Keys are `crate/src/path.rs` from the repository root with `/` separators on every
//! platform ([`repo_rel_slash`]); Windows CI runs the gate.

use std::collections::BTreeSet;
use std::path::{Component, Path};

use crate::greenfield_status::{
    in_test_range, rust_files, strip_visibility, test_mod_ranges, top_level_crate_dirs,
};

/// Files of at least this many production lines are tracked.
pub const THRESHOLD: usize = 2000;

/// The track's exit target: no back-half file at or over this many production lines. It
/// binds new ceilings now and every ceiling at track exit.
pub const EXIT_CAP: usize = 3000;

/// The committed ceilings, relative to the repository root.
pub const CEILINGS_FILE: &str = "xtask/module-line-ceilings.txt";

/// Back-half files that were already at or over [`EXIT_CAP`] when the ratchet landed (23
/// November 2026), exempt from the cap until track exit. An entry leaves the list when its
/// ceiling falls under [`EXIT_CAP`] (the test fails while a listed file is under it); the
/// slice that takes the last one under deletes this constant and the rule that reads it.
pub const GRANDFATHERED: &[&str] = &[
    "bynk-emit/src/emitter/emit.rs",
    "bynk-emit/src/emitter/serialisation.rs",
    "bynk-emit/src/emitter/workers_entry.rs",
    "bynk-emit/src/emitter.rs",
];

/// One measured file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleLines {
    /// `crate/src/path.rs`, with forward slashes.
    pub file: String,
    pub lines: usize,
}

/// `path` relative to `root`, joined with `/` whatever the platform separator is. Built from
/// `components()`, not by rewriting `\\` in a string, so a literal backslash in a Unix file
/// name is left alone and `Path::display` on Windows cannot leak a `\\` into a key (#4026).
pub fn repo_rel_slash(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The crates of the "back half" (track §8): the ratchet's scope and the probe's back-half reading.
pub const BACK_HALF_CRATES: [&str; 6] = [
    "bynk-model",
    "bynk-emit",
    "bynk-ts",
    "bynk-driver",
    "bynk-strip",
    "bynk",
];

/// True for a file under a crate's `src/` that is wholly tests: `tests.rs`,
/// `*_tests.rs`, or anything beneath a `tests/` directory. `rel` is the path
/// relative to `src/`, with `/` separators.
pub(crate) fn is_whole_test_file(rel: &str) -> bool {
    let mut parts: Vec<&str> = rel.split('/').collect();
    let file = parts.pop().unwrap_or("");
    file == "tests.rs" || file.ends_with("_tests.rs") || parts.contains(&"tests")
}

/// Lines of `source` outside its `#[cfg(test)]` module ranges. Comments and blank
/// lines count; only each test module's own range is removed, wherever it sits.
pub(crate) fn production_lines(source: &str) -> usize {
    let lines: Vec<&str> = source.lines().collect();
    let ranges = test_mod_ranges(&lines);
    (0..lines.len())
        .filter(|&i| !in_test_range(i, &ranges))
        .count()
}

/// Names declared by `#[cfg(test)] [vis] mod name;` — an external-file module
/// *declaration* gated by its attribute, whose file is therefore wholly test code
/// (`bynk-check/src/testkit.rs`). The attribute must be the line directly above
/// (blank lines allowed), as [`test_mod_ranges`] reads it.
fn cfg_test_declared_mods(source: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim() != "#[cfg(test)]" {
            continue;
        }
        let Some(next) = lines[i + 1..].iter().find(|l| !l.trim().is_empty()) else {
            continue;
        };
        let t = next.trim();
        let t = strip_visibility(t);
        if let Some(name) = t
            .trim_start()
            .strip_prefix("mod ")
            .and_then(|r| r.strip_suffix(';'))
        {
            out.push(name.trim().to_string());
        }
    }
    out
}

/// The `src/`-relative stems (`testkit`, `emitter/testkit`) of files and
/// directories declared away as test-only by a `#[cfg(test)] mod name;` in some
/// other file. A module file `a/b.rs` owns directory `a/b/`, except `lib.rs`,
/// `main.rs` and `mod.rs`, which own their own directory.
fn declaration_gated_stems(files: &[(String, String)]) -> BTreeSet<String> {
    let mut stems = BTreeSet::new();
    for (rel, contents) in files {
        let (dir, file) = rel.rsplit_once('/').unwrap_or(("", rel.as_str()));
        let stem = file.strip_suffix(".rs").unwrap_or(file);
        let owned = match stem {
            "lib" | "main" | "mod" => dir.to_string(),
            _ if dir.is_empty() => stem.to_string(),
            _ => format!("{dir}/{stem}"),
        };
        for name in cfg_test_declared_mods(contents) {
            stems.insert(if owned.is_empty() {
                name
            } else {
                format!("{owned}/{name}")
            });
        }
    }
    stems
}

/// True when `rel` is one of the declaration-gated stems' files (`stem.rs`,
/// `stem/mod.rs`) or lies beneath such a module's directory.
fn is_declaration_gated(rel: &str, stems: &BTreeSet<String>) -> bool {
    let without_ext = rel.strip_suffix(".rs").unwrap_or(rel);
    stems
        .iter()
        .any(|s| without_ext == s || rel.starts_with(&format!("{s}/")))
}

/// `(path from the repo root, production lines)` for one crate's `src/**/*.rs`
/// files, given as `(src-relative path, contents)`. Whole-test files and files
/// whose `mod` declaration is `#[cfg(test)]` are skipped; paths read
/// `{crate}/src/{rel}`.
pub(crate) fn module_lines_for_crate(
    crate_name: &str,
    files: &[(String, String)],
) -> Vec<(String, usize)> {
    let gated = declaration_gated_stems(files);
    files
        .iter()
        .filter(|(rel, _)| !is_whole_test_file(rel) && !is_declaration_gated(rel, &gated))
        .map(|(rel, contents)| {
            (
                format!("{crate_name}/src/{rel}"),
                production_lines(contents),
            )
        })
        .collect()
}

/// `(path from the repo root, production lines)` for every `src/**/*.rs` file of every
/// top-level crate in the workspace (the trend probe's universe).
pub fn measure_workspace(root: &Path) -> Vec<(String, usize)> {
    let mut files = Vec::new();
    for krate in top_level_crate_dirs(root) {
        let crate_name = krate.file_name().unwrap().to_string_lossy().to_string();
        let src = krate.join("src");
        let rels: Vec<(String, String)> = rust_files(&src)
            .into_iter()
            .map(|(path, contents)| (repo_rel_slash(&path, &src), contents))
            .collect();
        files.extend(module_lines_for_crate(&crate_name, &rels));
    }
    files
}

/// The ratchet's universe: the back-half crates' files at or over [`THRESHOLD`], longest
/// first, ties by path.
pub fn measure(root: &Path) -> Vec<ModuleLines> {
    let mut v: Vec<ModuleLines> = measure_workspace(root)
        .into_iter()
        .filter(|(file, lines)| {
            *lines >= THRESHOLD
                && file
                    .split('/')
                    .next()
                    .is_some_and(|c| BACK_HALF_CRATES.contains(&c))
        })
        .map(|(file, lines)| ModuleLines { file, lines })
        .collect();
    v.sort_by(|a, b| b.lines.cmp(&a.lines).then_with(|| a.file.cmp(&b.file)));
    v
}

/// The committed ceilings: `file = lines`, one per line; `#` starts a comment.
pub fn read_ceilings(root: &Path) -> Vec<(String, usize)> {
    let text = std::fs::read_to_string(root.join(CEILINGS_FILE)).unwrap_or_default();
    parse_ceilings(&text)
}

/// Lines of the committed ceilings file that are neither blank, a comment nor `file = lines`
/// (`foo.rs = 10,717`), which [`read_ceilings`] would otherwise drop silently.
pub fn malformed_ceiling_lines(root: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(root.join(CEILINGS_FILE)).unwrap_or_default();
    unparseable_lines(&text)
}

fn unparseable_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .filter(|l| {
            l.split_once('=')
                .is_none_or(|(_, n)| n.trim().parse::<usize>().is_err())
        })
        .map(|l| format!("{CEILINGS_FILE}: unparseable line {l:?}: write `file = lines`"))
        .collect()
}

fn parse_ceilings(text: &str) -> Vec<(String, usize)> {
    text.lines()
        .filter_map(|l| {
            let l = l.split('#').next().unwrap_or("").trim();
            let (key, n) = l.split_once('=')?;
            Some((key.trim().to_string(), n.trim().parse().ok()?))
        })
        .collect()
}

/// The ceilings file's text for a measurement: every back-half file at or over the
/// threshold, at its current length.
pub fn render_ceilings(measured: &[ModuleLines]) -> String {
    let mut s = format!(
        "# Ratcheted maximum module size (#4085). `file = lines`, for every back-half source\n\
         # file (bynk-model, bynk-emit, bynk-ts, bynk-driver, bynk-strip, bynk) of at least\n\
         # {THRESHOLD} production lines: comments and blank lines count; `#[cfg(test)]` modules,\n\
         # whole-test files and `#[cfg(test)] mod x;` files do not. A file may not grow past its\n\
         # ceiling, none may newly reach {THRESHOLD}, and an entry whose file has dropped below\n\
         # {THRESHOLD} (or gone, or been renamed) must be removed. A new entry may not be\n\
         # {EXIT_CAP} or more: split along a seam first. Prefer splitting a file to raising its\n\
         # ceiling; a raise needs its justification in review. `cargo xtask module-lines --apply`\n\
         # rewrites this file from the tree (lowering ceilings after a file shrinks).\n"
    );
    for m in measured {
        s.push_str(&format!("{} = {}\n", m.file, m.lines));
    }
    s
}

/// What is wrong, if anything: a file past its ceiling, one that newly crossed the
/// threshold, an entry that no longer names a file over it, a new entry at or over
/// [`EXIT_CAP`], a grandfathered entry that has fallen under it, and a key with a `\\`.
pub fn violations(measured: &[ModuleLines], ceilings: &[(String, usize)]) -> Vec<String> {
    let mut out = Vec::new();
    for m in measured {
        match ceilings.iter().find(|(k, _)| *k == m.file) {
            None => out.push(format!(
                "{} is {} production lines, over the {THRESHOLD}-line threshold, and has no ceiling: \
                 split along a seam, or, if it must be this long, justify that in review and record \
                 it with `cargo xtask module-lines --apply`",
                m.file, m.lines
            )),
            Some((_, ceiling)) if m.lines > *ceiling => out.push(format!(
                "{} grew to {} production lines, past its ceiling of {ceiling}: split the new code \
                 into its own module, or, if the growth is warranted, justify it in review and \
                 record it with `cargo xtask module-lines --apply`",
                m.file, m.lines
            )),
            Some(_) => {}
        }
    }
    for (key, ceiling) in ceilings {
        if key.contains('\\') {
            out.push(format!(
                "{key} has a `\\` in its key: keys are `/`-separated on every platform"
            ));
        }
        if !measured.iter().any(|m| m.file == *key) {
            out.push(format!(
                "{key} has a ceiling of {ceiling} but is no longer over {THRESHOLD} production lines \
                 (or is gone or renamed): remove its entry from {CEILINGS_FILE}"
            ));
        }
        let grandfathered = GRANDFATHERED.contains(&key.as_str());
        if *ceiling >= EXIT_CAP && !grandfathered {
            out.push(format!(
                "{key} has a ceiling of {ceiling}, at or over the {EXIT_CAP}-line exit cap, and is not \
                 grandfathered: split it along a seam before it grows that far"
            ));
        }
        if *ceiling < EXIT_CAP && grandfathered {
            out.push(format!(
                "{key} is under the {EXIT_CAP}-line exit cap: remove it from GRANDFATHERED in \
                 xtask/src/module_lines.rs"
            ));
        }
    }
    out
}

/// Grandfathered files that no longer have a ceiling entry (a split took them under the
/// threshold and deleted it): remove them from [`GRANDFATHERED`]. Checked against the whole
/// committed file, so it is separate from [`violations`], which unit tests feed partial lists.
pub fn stale_grandfathered(ceilings: &[(String, usize)]) -> Vec<String> {
    let mut out = Vec::new();
    for g in GRANDFATHERED {
        if !ceilings.iter().any(|(k, _)| k == g) {
            out.push(format!(
                "{g} is in GRANDFATHERED but has no ceiling entry: remove it from GRANDFATHERED in \
                 xtask/src/module_lines.rs"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn lines_of(n: usize) -> String {
        "x\n".repeat(n)
    }

    #[test]
    fn module_lines_excludes_a_trailing_test_module() {
        let src = format!(
            "{}#[cfg(test)]\nmod tests {{\n{}}}\n",
            lines_of(5),
            lines_of(100)
        );
        // 5 production lines plus the attribute line, which sits outside the range.
        assert_eq!(production_lines(&src), 6);
    }

    #[test]
    fn module_lines_excludes_a_mid_file_test_module_but_keeps_what_follows() {
        let src = format!(
            "{}#[cfg(test)]\nmod tests {{\n{}}}\n{}",
            lines_of(3),
            lines_of(50),
            lines_of(4)
        );
        assert_eq!(production_lines(&src), 3 + 1 + 4);
    }

    #[test]
    fn module_lines_counts_comments_and_blank_lines() {
        assert_eq!(production_lines("// c\n\nfn a() {}\n"), 3);
    }

    #[test]
    fn module_lines_skips_whole_test_files() {
        assert!(is_whole_test_file("tests.rs"));
        assert!(is_whole_test_file("emitter/tests.rs"));
        assert!(is_whole_test_file("parser_tests.rs"));
        assert!(is_whole_test_file("tests/fixtures/a.rs"));
        assert!(!is_whole_test_file("emitter/emit.rs"));
        assert!(!is_whole_test_file("attests.rs"));
    }

    #[test]
    fn module_lines_paths_read_crate_src_rel() {
        let files = vec![("emitter/a.rs".to_string(), lines_of(7))];
        assert_eq!(
            module_lines_for_crate("bynk-emit", &files),
            vec![("bynk-emit/src/emitter/a.rs".to_string(), 7)]
        );
    }

    #[test]
    fn module_lines_skips_files_declared_by_cfg_test_mod() {
        let lib =
            "#[cfg(test)]\npub(crate) mod testkit;\n\n#[cfg(test)]\nmod fixtures;\npub mod real;\n";
        let files = vec![
            ("lib.rs".to_string(), lib.to_string()),
            ("testkit.rs".to_string(), lines_of(500)),
            ("fixtures/mod.rs".to_string(), lines_of(50)),
            ("fixtures/inner.rs".to_string(), lines_of(50)),
            ("real.rs".to_string(), lines_of(9)),
        ];
        let got = module_lines_for_crate("c", &files);
        let names: Vec<&str> = got.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, vec!["c/src/lib.rs", "c/src/real.rs"]);
    }

    #[test]
    fn module_lines_declaration_gate_is_scoped_to_the_declaring_directory() {
        // `emitter.rs` declares `emitter/testkit.rs`; a top-level `testkit.rs`
        // that no `#[cfg(test)]` declaration gates stays production code.
        let files = vec![
            (
                "emitter.rs".to_string(),
                "#[cfg(test)]\nmod testkit;\n".to_string(),
            ),
            ("emitter/testkit.rs".to_string(), lines_of(30)),
            ("testkit.rs".to_string(), lines_of(40)),
            ("lib.rs".to_string(), "mod testkit;\n".to_string()),
        ];
        let got = module_lines_for_crate("c", &files);
        assert!(got.iter().any(|(p, n)| p == "c/src/testkit.rs" && *n == 40));
        assert!(!got.iter().any(|(p, _)| p.contains("emitter/testkit")));
    }

    fn m(file: &str, lines: usize) -> ModuleLines {
        ModuleLines {
            file: file.into(),
            lines,
        }
    }

    fn c(file: &str, lines: usize) -> (String, usize) {
        (file.to_string(), lines)
    }

    #[test]
    fn growth_a_new_crossing_and_a_stale_entry_are_each_reported() {
        let measured = [
            m("a/src/grew.rs", 2400),
            m("a/src/fresh.rs", 2100),
            m("a/src/ok.rs", 2000),
        ];
        let ceilings = vec![
            c("a/src/grew.rs", 2300),
            c("a/src/ok.rs", 2200),
            c("a/src/gone.rs", 2500),
        ];
        let v = violations(&measured, &ceilings);
        assert!(v.iter().any(|x| x.contains("a/src/grew.rs grew")), "{v:?}");
        assert!(
            v.iter()
                .any(|x| x.contains("a/src/fresh.rs") && x.contains("no ceiling")),
            "{v:?}"
        );
        assert!(
            v.iter()
                .any(|x| x.contains("a/src/gone.rs") && x.contains("remove")),
            "{v:?}"
        );
    }

    #[test]
    fn a_renamed_file_is_a_stale_entry_and_a_new_crossing() {
        let v = violations(
            &[m("a/src/new_name.rs", 2100)],
            &[c("a/src/old_name.rs", 2100)],
        );
        assert!(
            v.iter()
                .any(|x| x.contains("old_name.rs") && x.contains("renamed"))
        );
        assert!(
            v.iter()
                .any(|x| x.contains("new_name.rs") && x.contains("no ceiling"))
        );
    }

    #[test]
    fn a_file_that_shrank_but_stays_over_is_not_reported() {
        assert!(violations(&[m("a/src/s.rs", 2100)], &[c("a/src/s.rs", 2500)]).is_empty());
    }

    #[test]
    fn a_new_entry_at_the_exit_cap_is_reported_but_a_grandfathered_one_is_not() {
        // 3,000 exactly is over the cap (the probe counts with `>=`).
        let v = violations(&[m("a/src/big.rs", 3000)], &[c("a/src/big.rs", 3000)]);
        assert!(v.iter().any(|x| x.contains("exit cap")), "{v:?}");
        let g = GRANDFATHERED[0];
        assert!(violations(&[m(g, 3500)], &[c(g, 3500)]).is_empty());
        // A grandfathered file that fell under the cap must leave the list.
        let v = violations(&[m(g, 2999)], &[c(g, 2999)]);
        assert!(v.iter().any(|x| x.contains("GRANDFATHERED")), "{v:?}");
    }

    #[test]
    fn a_grandfathered_file_without_an_entry_is_reported() {
        let all: Vec<(String, usize)> = GRANDFATHERED.iter().map(|g| c(g, 3500)).collect();
        assert!(stale_grandfathered(&all).is_empty());
        let v = stale_grandfathered(&all[1..]);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains(GRANDFATHERED[0]));
    }

    #[test]
    fn a_malformed_ceiling_line_is_reported() {
        let v = unparseable_lines("# c\n\na/src/x.rs = 2400\na/src/y.rs = 10,717\nnoequals\n");
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(v[0].contains("10,717"));
    }

    #[test]
    fn a_key_with_a_backslash_is_reported() {
        let v = violations(&[m("a\\src\\b.rs", 2100)], &[c("a\\src\\b.rs", 2100)]);
        assert!(v.iter().any(|x| x.contains("`\\`")), "{v:?}");
    }

    /// Built with `Path::join` per component, so on Windows the separator is `\\` and the key
    /// must still read `/` (#4026).
    #[test]
    fn repo_rel_slash_joins_components_with_a_forward_slash() {
        let root = PathBuf::from("repo");
        let path = root.join("a").join("src").join("b").join("c.rs");
        assert_eq!(repo_rel_slash(&path, &root), "a/src/b/c.rs");
    }

    #[test]
    fn ceilings_round_trip_through_render_and_parse() {
        let measured = [m("a/src/x.rs", 2400), m("b/src/y.rs", 2100)];
        let parsed = parse_ceilings(&render_ceilings(&measured));
        assert_eq!(parsed, vec![c("a/src/x.rs", 2400), c("b/src/y.rs", 2100)]);
    }

    #[test]
    fn the_grandfathered_list_has_no_duplicates() {
        let set: BTreeSet<&&str> = GRANDFATHERED.iter().collect();
        assert_eq!(set.len(), GRANDFATHERED.len());
    }
}
