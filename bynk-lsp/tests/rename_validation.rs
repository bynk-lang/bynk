//! v0.25 (ADR 0053): the rename pipeline as pure functions — plan → apply →
//! re-analyse → validate — over real multi-file projects on disk. No
//! transport: these exercise exactly what the `rename` handler runs.

use bynk_lsp::index_queries;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

// The handler-side pure modules, included directly (bynk-lsp is a binary
// crate). Not all of their surface is exercised here — references/definition
// queries have their own unit tests inside the module. `position` is here
// because `index_queries` reaches it as `crate::position` (v0.28's
// semantic-tokens producer converts spans for the delta encoding).

fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-rename-{}-{}",
        test_name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create test root");
    for (rel, contents) in files {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&p, contents).expect("write file");
    }
    root.canonicalize().unwrap_or(root)
}

struct Analysed {
    index: bynk_check::index::ProjectIndex,
    snapshots: HashMap<PathBuf, String>,
    diags: Vec<(PathBuf, String)>,
}

fn analyse(root: &Path, overlay: &HashMap<PathBuf, String>) -> Analysed {
    let result = bynk_ide::diagnose_project(root, overlay);
    Analysed {
        index: result.index,
        snapshots: result
            .files
            .iter()
            .map(|f| (f.source_path.clone(), f.text.clone()))
            .collect(),
        diags: result
            .files
            .iter()
            .flat_map(|f| {
                f.diagnostics
                    .iter()
                    .map(|d| (f.source_path.clone(), d.error.category.to_string()))
            })
            .collect(),
    }
}

/// Run the full handler pipeline: plan at (file, offset), apply, re-analyse,
/// both validators. Returns the edited texts on success.
fn try_rename(
    root: &Path,
    pre: &Analysed,
    at: (&str, usize),
    new_name: &str,
) -> Result<HashMap<PathBuf, String>, String> {
    let mut plan = index_queries::plan_rename(&pre.index, Path::new(at.0), at.1, new_name)?;
    index_queries::narrow_qualified_edits(&mut plan, &pre.snapshots);
    let mut overlay = HashMap::new();
    let mut edited_texts = HashMap::new();
    for (rel, text) in &pre.snapshots {
        let edited = match plan.edits.get(rel) {
            Some(spans) => index_queries::apply_edits(text, spans, new_name),
            None => text.clone(),
        };
        overlay.insert(root.join(rel), edited.clone());
        edited_texts.insert(rel.clone(), edited);
    }
    let post = analyse(root, &overlay);
    index_queries::no_new_diagnostics(&pre.diags, &post.diags)?;
    if !index_queries::index_unchanged_modulo_rename(&pre.index, &post.index, &plan) {
        return Err("index changed beyond the rename (capture/escape)".to_string());
    }
    Ok(edited_texts)
}

/// Byte offset of the `n`-th occurrence of `needle` in `file`.
fn offset_of(snapshots: &HashMap<PathBuf, String>, file: &str, needle: &str, n: usize) -> usize {
    let text = &snapshots[Path::new(file)];
    let mut search_from = 0;
    let mut remaining = n;
    loop {
        let at = text[search_from..].find(needle).expect("needle present") + search_from;
        if remaining == 0 {
            return at;
        }
        remaining -= 1;
        search_from = at + 1;
    }
}

const UTIL: &str = "\
commons demo.util

type Money = Int where NonNegative

fn helper(x: Int) -> Int {
  x
}
";

const APP: &str = "\
context demo.app

uses demo.util

fn fee(m: Money) -> Int {
  helper(2)
}
";

#[test]
fn happy_path_rename_edits_def_and_all_references() {
    let root = setup_project("happy", &[("demo/util.bynk", UTIL), ("demo/app.bynk", APP)]);
    let pre = analyse(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );
    assert!(pre.diags.is_empty(), "fixture clean: {:?}", pre.diags);

    // Rename `Money` from its *reference* in app.bynk.
    let at = offset_of(&pre.snapshots, "demo/app.bynk", "Money", 0);
    let edited = try_rename(&root, &pre, ("demo/app.bynk", at), "Cash").expect("rename succeeds");
    assert!(edited[Path::new("demo/util.bynk")].contains("type Cash = Int"));
    assert!(edited[Path::new("demo/app.bynk")].contains("fn fee(m: Cash)"));
    assert!(!edited[Path::new("demo/util.bynk")].contains("Money"));
    assert!(!edited[Path::new("demo/app.bynk")].contains("Money"));
}

#[test]
fn colliding_rename_is_refused_by_reanalysis() {
    let root = setup_project(
        "collide",
        &[("demo/util.bynk", UTIL), ("demo/app.bynk", APP)],
    );
    let pre = analyse(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );

    // `helper` → `Money` collides with the type declared in the same unit.
    let at = offset_of(&pre.snapshots, "demo/util.bynk", "helper", 0);
    let err = try_rename(&root, &pre, ("demo/util.bynk", at), "Money")
        .expect_err("collision must refuse");
    assert!(
        err.contains("bynk.resolve.name_conflict") || err.contains("would introduce"),
        "refusal cites the introduced diagnostic: {err}"
    );
}

#[test]
fn capturing_rename_is_refused_by_index_equality() {
    // `use_local` calls its fn-typed *parameter* `shadow`. Renaming the
    // top-level `helper` to `shadow` makes the call site resolve to the
    // declared fn instead (call position prefers declared fns) — no new
    // diagnostic, silently re-bound. The index-equality validator refuses.
    let cap = "\
commons demo.cap

fn helper(x: Int) -> Int {
  x
}

fn use_local(shadow: Int -> Int, y: Int) -> Int {
  shadow(y)
}
";
    let root = setup_project("capture", &[("demo/cap.bynk", cap)]);
    let pre = analyse(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );
    assert!(pre.diags.is_empty(), "fixture clean: {:?}", pre.diags);

    let at = offset_of(&pre.snapshots, "demo/cap.bynk", "helper", 0);
    let err =
        try_rename(&root, &pre, ("demo/cap.bynk", at), "shadow").expect_err("capture must refuse");
    assert!(
        err.contains("capture/escape"),
        "refusal comes from the index-equality validator: {err}"
    );
}

#[test]
fn prepare_rename_refuses_locals_and_invalid_names_refuse() {
    let root = setup_project("locals", &[("demo/util.bynk", UTIL)]);
    let pre = analyse(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );

    // The parameter `x` is a local binding — not in the index, refused.
    let at = offset_of(&pre.snapshots, "demo/util.bynk", "x: Int", 0);
    assert!(index_queries::prepare_rename(&pre.index, Path::new("demo/util.bynk"), at).is_none());

    // Keyword / non-identifier new names refuse at planning.
    let at_money = offset_of(&pre.snapshots, "demo/util.bynk", "Money", 0);
    for bad in ["fn", "two words", "a.b", ""] {
        assert!(
            index_queries::plan_rename(&pre.index, Path::new("demo/util.bynk"), at_money, bad)
                .is_err(),
            "{bad:?} must refuse"
        );
    }
}

/// #4352: renaming a declaration a unit reads by several spellings rewrites every one of them,
/// and the renamed project still analyses clean.
#[test]
fn rename_reaches_every_spelling_of_a_used_declaration() {
    let lib = "commons lib.y\n\ntype Status = | On | Off\n";
    let app = "commons app.z\n\nuses lib.y\nuses lib.y as Y\n\n\
               fn a(s: Status) -> Y.Status { s }\n\nfn b(s: Y.Status) -> lib.y.Status { s }\n\n\
               fn c(s: lib.y.Status) -> Status { s }\n";
    let root = setup_project(
        "every-spelling",
        &[("lib/y.bynk", lib), ("app/z.bynk", app)],
    );
    let pre = analyse(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );
    assert!(pre.diags.is_empty(), "fixture clean: {:?}", pre.diags);

    let at = offset_of(&pre.snapshots, "lib/y.bynk", "Status", 0);
    let edited = try_rename(&root, &pre, ("lib/y.bynk", at), "State")
        .expect("renaming through every spelling is clean");
    let z = &edited[Path::new("app/z.bynk")];
    assert!(!z.contains("Status"), "every spelling renamed: {z}");
    for spelling in ["s: State", "Y.State", "lib.y.State"] {
        assert!(z.contains(spelling), "`{spelling}` in {z}");
    }
}
