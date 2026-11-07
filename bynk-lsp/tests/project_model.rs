//! Slice A (#647): the LSP analyses the project `bynk` compiles.
//!
//! The defect: `bynk-lsp` reduced `bynk.toml`'s `[paths] include` to a single
//! `src_dir` string — first entry only, `exclude` ignored — and handed
//! `<root>/src` to the analyser *as* a project root. So a secondary `include`
//! tree was invisible, `exclude` did nothing, and a flat project failed.
//!
//! These fixtures measure the **file set**, which is the thing that was wrong.
//! They read sibling directories (`../examples`, `../bynk`) deliberately — the
//! point is agreement with the real compiler on real projects — so this file
//! rides `Cargo.toml`'s `exclude` list alongside `hover_references.rs` and
//! `declaration_spans.rs`, keeping it out of the published tarball where those
//! siblings do not exist.
//!
//! They go through `bynk_ide` rather than `Backend` because `bynk-lsp` has no
//! `[lib]` target (that is slice C). What `Backend` itself does with these roots
//! is pinned in-crate, hermetically, in `main.rs`'s test module.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn rel_files(root: &Path) -> Vec<String> {
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let roots = bynk_ide::AnalysisRoots::Project(root.to_path_buf());
    let sources = bynk_testkit::read_project_sources(&roots);
    let r = bynk_ide::diagnose_project_with(&roots, &sources, &bynk_ide::SchemaLock::Off);
    let mut v: Vec<String> = r
        .files
        .iter()
        .map(|f| f.source_path.to_string_lossy().replace('\\', "/"))
        .collect();
    v.sort();
    v
}

/// The regression this slice exists for. `examples/todo/bynk.toml` declares no
/// `[paths]`, so `ProjectPaths::conventional` yields `include = ["src",
/// "tests"]` — and `examples/todo/tests/todos.bynk` is a real `suite` the
/// compiler compiles. Before slice A the LSP could not see it at all: the
/// analyser was handed `<root>/src`.
///
/// Also pins ADR 0198's identity: the two files share the basename `todos.bynk`
/// and must not collide.
#[test]
fn the_todo_example_s_tests_tree_is_analysed() {
    let root = workspace().join("examples/todo");
    assert!(root.join("tests/todos.bynk").is_file(), "fixture present");
    assert!(
        !root
            .join("bynk.toml")
            .read_to_string_lossy()
            .contains("[paths]"),
        "the example must still declare no [paths] — that is what makes it the fixture",
    );

    let files = rel_files(&root);
    assert!(
        files.contains(&"tests/todos.bynk".to_string()),
        "the LSP must analyse the tests tree `bynk` compiles; got {files:?}",
    );
    assert!(
        files.contains(&"src/todos.bynk".to_string()),
        "…without losing the src tree; got {files:?}",
    );
}

trait ReadLossy {
    fn read_to_string_lossy(&self) -> String;
}
impl ReadLossy for PathBuf {
    fn read_to_string_lossy(&self) -> String {
        std::fs::read_to_string(self).unwrap_or_default()
    }
}

// -- the manifest matrix --------------------------------------------------
//
// Each case is a real project on disk, analysed through the roots the LSP now
// resolves. These are hermetic (no sibling reads) but live here beside the
// `examples/todo` regression because they are the same claim at different
// layouts: the LSP's file set is the manifest's, not a guess.

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str, files: &[(&str, &str)]) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "bynk_sliceA_{tag}_{}_{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, body) in files {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    }
    Scratch(dir)
}

const MANIFEST: &str = "[project]\nname = \"m\"\n";

/// A conventional `src/`-only project: one `include` root, discovered without
/// any `[paths]` at all.
#[test]
fn conventional_src_only() {
    let s = scratch(
        "srconly",
        &[("bynk.toml", MANIFEST), ("src/a.bynk", "context a\n")],
    );
    assert_eq!(rel_files(&s.0), vec!["src/a.bynk"]);
}

/// The layout `examples/todo` has: `conventional()` picks up `tests/` because
/// it exists. The primary tree alone is what the LSP used to see.
#[test]
fn conventional_src_and_tests() {
    let s = scratch(
        "srctests",
        &[
            ("bynk.toml", MANIFEST),
            ("src/a.bynk", "context a\n"),
            ("tests/a.bynk", "suite a\n"),
        ],
    );
    assert_eq!(rel_files(&s.0), vec!["src/a.bynk", "tests/a.bynk"]);
}

/// An explicit two-root `include` with names that are not `src`/`tests` — the
/// flat layout ADR 0147 settled, where test-ness is structural rather than
/// directory-based.
#[test]
fn explicit_two_root_include() {
    let s = scratch(
        "tworoot",
        &[
            (
                "bynk.toml",
                "[project]\nname = \"m\"\n\n[paths]\ninclude = [\"lib\", \"spec\"]\n",
            ),
            ("lib/a.bynk", "context a\n"),
            ("spec/a.bynk", "suite a\n"),
            // Not under any include root — must not be analysed.
            ("src/stray.bynk", "context stray\n"),
        ],
    );
    assert_eq!(rel_files(&s.0), vec!["lib/a.bynk", "spec/a.bynk"]);
}

/// Content-ownership track (#1086) slice 2: an unsaved edit to `bynk.toml`
/// itself — not just to the `.bynk` sources it names — changes which trees
/// `diagnose_project_with` walks, because `AnalysisRoots::lower()` now
/// threads the overlay into `bynk.toml`'s own read. On disk, `bynk.toml`
/// still declares only `include = ["src"]`; the overlay's (unsaved) version
/// adds a second root, `alt`, and only the overlay's answer is the correct
/// one for what the editor should show.
#[test]
fn an_unsaved_bynk_toml_edit_changes_the_resolved_roots() {
    let s = scratch(
        "toml_overlay",
        &[
            (
                "bynk.toml",
                "[project]\nname = \"m\"\n\n[paths]\ninclude = [\"src\"]\n",
            ),
            ("src/a.bynk", "context a\n"),
            ("alt/b.bynk", "context b\n"),
        ],
    );
    let toml_path = s.0.join("bynk.toml");
    // Content-ownership track (#1086) slice 5: the `.bynk` sources
    // themselves must be readable too — `bynk-emit`'s disk-read fallback
    // used to fill these in silently; the overlay must be complete now.
    let mut overlay = HashMap::new();
    overlay.insert(
        toml_path.canonicalize().unwrap_or(toml_path),
        "[project]\nname = \"m\"\n\n[paths]\ninclude = [\"src\", \"alt\"]\n".to_string(),
    );
    overlay.insert(s.0.join("src/a.bynk"), "context a\n".to_string());
    overlay.insert(s.0.join("alt/b.bynk"), "context b\n".to_string());
    let r = bynk_ide::diagnose_project_with(
        &bynk_ide::AnalysisRoots::Project(s.0.clone()),
        &overlay,
        &bynk_ide::SchemaLock::Off,
    );
    let mut files: Vec<String> = r
        .files
        .iter()
        .map(|f| f.source_path.to_string_lossy().replace('\\', "/"))
        .collect();
    files.sort();
    assert_eq!(
        files,
        vec!["alt/b.bynk", "src/a.bynk"],
        "the overlay's [paths] include, not the on-disk manifest's, must decide the roots"
    );
}

/// A flat project: `.bynk` at the root, no `src/`. `conventional()` yields
/// `include = ["."]`. The LSP used to look for a nonexistent `src/` and find
/// nothing; ADR 0198 normalises the `.` so paths stay unprefixed.
#[test]
fn flat_project_with_a_manifest() {
    let s = scratch(
        "flat",
        &[("bynk.toml", MANIFEST), ("a.bynk", "context a\n")],
    );
    assert_eq!(
        rel_files(&s.0),
        vec!["a.bynk"],
        "a flat project's paths are unprefixed — never `./a.bynk`"
    );
}

/// `exclude` is honoured. The LSP previously parsed it and threw it away
/// (`project.rs` said so in a comment), so a generated tree was analysed.
#[test]
fn exclude_is_honoured() {
    let s = scratch(
        "exclude",
        &[
            (
                "bynk.toml",
                "[project]\nname = \"m\"\n\n[paths]\ninclude = [\".\"]\nexclude = [\"generated\"]\n",
            ),
            ("a.bynk", "context a\n"),
            ("generated/gen.bynk", "context gen\n"),
        ],
    );
    assert_eq!(
        rel_files(&s.0),
        vec!["a.bynk"],
        "an excluded subtree must not be analysed"
    );
}

/// The tool's own caches are skipped even when `include` is the project root —
/// otherwise a flat project would sweep its own build output back in.
#[test]
fn out_and_node_modules_are_skipped() {
    let s = scratch(
        "caches",
        &[
            ("bynk.toml", MANIFEST),
            ("a.bynk", "context a\n"),
            ("out/emitted.bynk", "context emitted\n"),
            ("node_modules/dep/d.bynk", "context d\n"),
        ],
    );
    assert_eq!(rel_files(&s.0), vec!["a.bynk"]);
}

/// CI repro: the VS Code extension's fixture workspace. Its `bynk.toml` still
/// uses the **legacy** `[paths] src`/`tests` keys ADR 0147 retired, which
/// `read_project_paths` ignores → `conventional()` → `include = ["src"]`.
#[test]
fn the_vscode_fixture_workspace_layout() {
    let s = scratch(
        "vscode",
        &[
            (
                "bynk.toml",
                "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\n\n[paths]\nsrc = \"src\"\ntests = \"tests\"\n",
            ),
            (
                "src/text.bynk",
                "commons text\n\nfn shout(s: String) -> String {\n  s\n}\n",
            ),
        ],
    );
    let files = rel_files(&s.0);
    assert_eq!(
        files,
        vec!["src/text.bynk"],
        "the fixture's one file must be analysed"
    );
}

/// **The invariant.** Every path a round produces must name a file the round
/// analysed — i.e. must be a `snapshots` key.
///
/// This is what should have caught the CI failure that sent this slice back.
/// ADR 0198 split a file's identity from its unit-validation path, and the
/// keyed sinks were converted by *grepping for a pattern* (`&pf.source_path`).
/// That pattern missed `&parsed[i].source_path` in three `refs.enter_file`
/// calls, an error attribution, and — the ones that actually bit — three site
/// paths in a different file entirely (`project/symbols.rs`), which is where
/// the index's def/ref sites are built. All were no-ops while the prefix was
/// empty, and all broke the moment slice A made it non-empty. Exactly the
/// "fails quietly, same shape as the bug" the slice-0 review warned about.
///
/// A grep finds the sites you thought of. This asserts the property.
///
/// `examples/todo` is the fixture because it is genuinely two-rooted *and* has
/// a `suite`, so it reaches the test-file index paths a hermetic commons-only
/// project never would.
#[test]
fn every_path_a_round_produces_is_a_file_the_round_analysed() {
    let root = workspace().join("examples/todo");
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let roots = bynk_ide::AnalysisRoots::Project(root.clone());
    let sources = bynk_testkit::read_project_sources(&roots);
    let r = bynk_ide::diagnose_project_with(&roots, &sources, &bynk_ide::SchemaLock::Off);

    let known: std::collections::BTreeSet<PathBuf> =
        r.files.iter().map(|f| f.source_path.clone()).collect();
    assert!(
        known.len() > 1,
        "the fixture must be multi-rooted or this proves nothing; got {known:?}",
    );

    let mut stray: Vec<String> = Vec::new();
    let mut check = |sink: &str, p: &Path| {
        if !known.contains(p) {
            stray.push(format!("{sink}: {}", p.display()));
        }
    };

    for (key, entry) in &r.index.symbols {
        if let Some(def) = &entry.def {
            check(&format!("index def {}", key.name), &def.path);
        }
        for s in &entry.refs {
            check(&format!("index ref {}", key.name), &s.path);
        }
    }
    for fr in &r.index.foreign_refs {
        check("index foreign_ref", &fr.site.path);
    }
    for e in &r.index.calls {
        check("index call site", &e.site.path);
    }
    for e in &r.index.impls {
        check("index impl site", &e.site.path);
    }
    for (p, _) in r.hints.iter() {
        check("hints", p);
    }
    for (p, _) in r.locals.iter() {
        check("locals", p);
    }
    for (p, _) in r.expr_types.iter() {
        check("expr_types", p);
    }
    for (p, _) in r.requirements.iter() {
        check("requirements", p);
    }
    for (unit, paths) in &r.unit_sources {
        for p in paths {
            check(&format!("unit_sources[{unit}]"), p);
        }
    }

    assert!(
        stray.is_empty(),
        "every path a round produces must be one of its analysed files.\n\
         analysed: {known:?}\n\
         strays:\n  {}",
        stray.join("\n  "),
    );
}

/// Rename validation re-analyses the edited buffers and compares the result
/// against the round's baseline. It must re-analyse over the **same roots** the
/// round used, or the two file sets diverge and a valid rename is spuriously
/// refused. This pins the property the round and the validator must share: for
/// a project with an `exclude`d `.bynk` tree, single-tree analysis (what a bare
/// `diagnose_project(project_root)` gives) sees strictly more files than the
/// manifest-aware round — so the validator cannot use it.
#[test]
fn single_tree_and_project_analysis_disagree_when_exclude_is_in_play() {
    let s = scratch(
        "rename_excl",
        &[
            (
                "bynk.toml",
                "[project]\nname = \"m\"\n\n[paths]\ninclude = [\".\"]\nexclude = [\"generated\"]\n",
            ),
            ("a.bynk", "context a\n"),
            ("generated/gen.bynk", "context gen\n"),
        ],
    );

    // The round: manifest-aware, `exclude` honoured.
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let project_roots = bynk_ide::AnalysisRoots::Project(s.0.clone());
    let project_sources = bynk_testkit::read_project_sources(&project_roots);
    let project = bynk_ide::diagnose_project_with(
        &project_roots,
        &project_sources,
        &bynk_ide::SchemaLock::Off,
    );
    let project_files: std::collections::BTreeSet<String> = project
        .files
        .iter()
        .map(|f| f.source_path.to_string_lossy().replace('\\', "/"))
        .collect();

    // The straggler: `diagnose_project(project_root)` → single-tree, no exclude.
    let single_sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(s.0.clone()));
    let single = bynk_ide::diagnose_project(&s.0, &single_sources);
    let single_files: std::collections::BTreeSet<String> = single
        .files
        .iter()
        .map(|f| f.source_path.to_string_lossy().replace('\\', "/"))
        .collect();

    assert!(
        project_files.iter().all(|f| !f.contains("generated/")),
        "the round must exclude `generated/`; got {project_files:?}",
    );
    assert!(
        single_files.iter().any(|f| f.contains("gen.bynk")),
        "single-tree analysis sweeps the excluded tree — that is exactly why \
         rename must not use it; got {single_files:?}",
    );
    assert_ne!(
        project_files, single_files,
        "the two must differ, or this test proves nothing about the bug",
    );
}
