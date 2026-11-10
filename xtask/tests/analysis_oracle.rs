//! The differential oracle for the language server's analysis memo (#3243,
//! #1523 S2).
//!
//! A reused analysis round must answer exactly what a fresh one answers. This
//! drives `analyse_project_memoised` (the entry point reuse will sit behind) and
//! `analyse_project` side by side, over
//!
//! - every project under `examples/`, then again with the memo warm; and
//! - seeded random edit sequences on the benchmark generator's two project
//!   shapes plus a commons whose function body can gain a `~>` (the cross-unit
//!   body fact, #2719, that a signature-only key would miss),
//!
//! and asserts the two [`canonical_report`]s are identical after every step.
//! Edits go through the overlay the way open editor buffers do. A failure prints
//! the seed, the step and the edit, so it replays.
//!
//! Sizes are small enough for the unit tier; `BYNK_ORACLE_SEEDS` and
//! `BYNK_ORACLE_STEPS` raise them (a nightly can run many more).

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use bynk_check::analysis::{ProjectAnalysis, analyse_project};
use bynk_check::memo::{AnalysisMemo, analyse_project_memoised};
use bynk_check::oracle::canonical_report;
use bynk_project::{Roots, SchemaLock};
use xtask::bench::{Shape, edit_source, generate_project_shaped};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// splitmix64: small, seedable, and the same on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The first line where two reports differ, with a little context, for a failure message.
fn first_difference(fresh: &str, memoised: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (fresh.lines().collect(), memoised.lines().collect());
    for i in 0..a.len().max(b.len()) {
        if a.get(i) != b.get(i) {
            return format!(
                "line {i}:\n  fresh:    {:?}\n  memoised: {:?}",
                a.get(i),
                b.get(i)
            );
        }
    }
    "reports differ only in length".to_string()
}

/// One round both ways; the memoised answer must equal the fresh one, and the
/// fresh one must equal itself (so the report is deterministic).
fn check_round(
    roots: &Roots,
    overlay: &HashMap<PathBuf, String>,
    memo: &mut AnalysisMemo,
    ctx: &str,
) -> BTreeSet<&'static str> {
    let fresh: ProjectAnalysis = analyse_project(roots, overlay, &SchemaLock::Off);
    let memoised = analyse_project_memoised(roots, overlay, &SchemaLock::Off, memo);
    let (fresh_report, memo_report) = (canonical_report(&fresh), canonical_report(&memoised));
    assert!(
        fresh_report == memo_report,
        "memoised analysis differs from fresh ({ctx}): {}",
        first_difference(&fresh_report, &memo_report)
    );
    let again = canonical_report(&analyse_project(roots, overlay, &SchemaLock::Off));
    assert!(
        fresh_report == again,
        "the canonical report is not deterministic ({ctx}): {}",
        first_difference(&fresh_report, &again)
    );
    fresh.errors.iter().map(|e| e.error.category).collect()
}

// --- the corpus ---------------------------------------------------------------

#[test]
fn examples_agree_cold_and_warm() {
    let examples = xtask::repo_root().join("examples");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&examples)
        .expect("examples/")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("bynk.toml").is_file())
        .collect();
    dirs.sort();
    assert!(
        dirs.len() >= 10,
        "the examples corpus went missing: {dirs:?}"
    );
    for dir in dirs {
        let roots = Roots::Single(dir.clone());
        let overlay = HashMap::new();
        let mut memo = AnalysisMemo::new();
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        // Cold, then warm: the second round has a populated memo.
        check_round(
            &roots,
            &overlay,
            &mut memo,
            &format!("example {name}, cold"),
        );
        check_round(
            &roots,
            &overlay,
            &mut memo,
            &format!("example {name}, warm"),
        );
    }
}

// --- random edit sequences ----------------------------------------------------

/// The generated project, held the way the language server holds one: files on
/// disk, edits in an overlay.
struct Project {
    root: PathBuf,
    overlay: HashMap<PathBuf, String>,
    /// Overlay-only files added by an edit, so they can be removed again.
    added: Vec<PathBuf>,
}

impl Project {
    fn create(shape: Shape, contexts: usize, tag: &str) -> Project {
        let root = std::env::temp_dir().join(format!(
            "bynk-analysis-oracle-{}-{}-{tag}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mut files = generate_project_shaped(shape, contexts, 1);
        // A commons whose function body can gain a `~>`, and contexts that call
        // it from an agent handler: an edit to a body in one unit changes the
        // diagnostic in another (#2719).
        files.push((
            "src/util.bynk".to_string(),
            "commons util\n\nfn run(f: (Int) -> Effect[()], n: Int) -> Effect[Int] {\n\tlet _ <- f(n)\n\tEffect.pure(n)\n}\n".to_string(),
        ));
        for i in (0..contexts).step_by(3) {
            files.push((format!("src/leak{i}.bynk"), leak_context(i)));
        }
        let mut overlay = HashMap::new();
        for (rel, text) in &files {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            if rel.ends_with(".bynk") {
                overlay.insert(path, text.clone());
            }
        }
        Project {
            root,
            overlay,
            added: Vec::new(),
        }
    }

    fn bynk_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.overlay.keys().cloned().collect();
        v.sort();
        v
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn leak_context(i: usize) -> String {
    format!(
        "context leak{i}\n\nuses util\n\nagent Counter {{\n\tkey id: String\n\n\tstore hits: Cell[Int] = 0\n\n\
         \ton call bump() -> Effect[Int] {{\n\t\tlet r <- run((k: Int) => Effect.pure(()), 1)\n\t\tEffect.pure(r)\n\t}}\n}}\n"
    )
}

/// Swap `a` and `b` in `text` (whichever is present first), as an edit and its undo.
fn toggle(text: &str, a: &str, b: &str) -> Option<String> {
    if text.contains(a) {
        Some(text.replacen(a, b, 1))
    } else if text.contains(b) {
        Some(text.replacen(b, a, 1))
    } else {
        None
    }
}

const EXTRA_SERVICE: &str =
    "\n// extra\nservice extra {\n  on call() -> Effect[Int] {\n    Effect.pure(1)\n  }\n}\n";

/// Apply one random edit, returning what it did. An edit that finds nothing to
/// change in its target falls back to a leading-comment edit (which still shifts
/// every span in the file), so each step changes the project.
fn random_edit(project: &mut Project, rng: &mut Rng, step: usize) -> String {
    let files = project.bynk_files();
    let target = files[rng.below(files.len())].clone();
    let name = target.file_name().unwrap().to_string_lossy().into_owned();
    let text = project.overlay[&target].clone();
    let op = rng.below(9);
    // `Some(new text)` edits `target`; `None` means the op changed the project
    // some other way (another file, an added or removed context) and `target`
    // must be left exactly as it is, or its write would undo that.
    let edited: Option<(Option<String>, &str)> = match op {
        // A field's type becomes unknown, and back: an error appears and clears.
        0 => toggle(&text, "label: String,", "label: Strng,")
            .map(|t| (Some(t), "toggle a field to an unknown type")),
        // A numeric field becomes a String: the bodies that use it stop checking.
        1 => toggle(&text, "qty: Int,", "qty: String,")
            .or_else(|| toggle(&text, "count: Int,", "count: String,"))
            .map(|t| (Some(t), "change a record field's type")),
        // A body-only edit: a literal in a filter.
        2 => toggle(&text, "> 0)", "> 1)").map(|t| (Some(t), "change a literal in a body")),
        // A service's parameter type changes: every caller in another unit is affected.
        3 => toggle(&text, "on call(n: Int)", "on call(n: String)")
            .map(|t| (Some(t), "change a service signature")),
        // A whole declaration appears or disappears at the end of the file.
        4 => Some(if text.contains("// extra") {
            (
                Some(text.replacen(EXTRA_SERVICE, "", 1)),
                "remove a trailing service",
            )
        } else {
            (
                Some(format!("{text}{EXTRA_SERVICE}")),
                "add a trailing service",
            )
        }),
        // The body fact that crosses units: the commons function gains or loses a
        // `~>`. It edits `util.bynk` itself, whatever file was drawn as `target`.
        5 => {
            let util = project.root.join("src/util.bynk");
            let util_text = project.overlay[&util].clone();
            toggle(&util_text, "let _ <- f(n)", "~> f(n)").map(|t| {
                project.overlay.insert(util, t);
                (None, "toggle `~>` in the commons function body")
            })
        }
        // A comment before everything: every span in the file shifts.
        6 => Some((
            Some(format!("// shift {step}\n{text}")),
            "insert a leading comment",
        )),
        // A new overlay-only context appears.
        7 => {
            let path = project.root.join(format!("src/added{step}.bynk"));
            let body = format!(
                "context added{step}\n\ntype Tag{step} = String where NonEmpty\n\nagent Box{step} {{\n  key name: String\n\n  store tags: Map[String, Tag{step}]\n}}\n"
            );
            project.overlay.insert(path.clone(), body);
            project.added.push(path);
            Some((None, "add a context"))
        }
        // An added context is removed again.
        _ => project.added.pop().map(|path| {
            project.overlay.remove(&path);
            (None, "remove an added context")
        }),
    };
    // An op that found nothing to change falls back to a leading comment (which
    // still shifts every span in the file), so each step changes the project.
    let (new_text, what) = edited.unwrap_or_else(|| {
        (
            Some(format!("// fallback {step}\n{text}")),
            "insert a leading comment (fallback)",
        )
    });
    match new_text {
        Some(t) => {
            project.overlay.insert(target, t);
            format!("{what} in {name}")
        }
        None => what.to_string(),
    }
}

fn run_sequence(shape: Shape, seed: u64, steps: usize) -> BTreeSet<&'static str> {
    let contexts = 6;
    let mut project = Project::create(shape, contexts, &format!("{}-{seed}", shape.name()));
    let roots = Roots::Single(project.root.clone());
    let mut rng = Rng(seed ^ 0xB7_4E_D0_A1);
    let mut memo = AnalysisMemo::new();
    let mut seen = check_round(
        &roots,
        &project.overlay,
        &mut memo,
        &format!("{} seed {seed}, initial", shape.name()),
    );
    for step in 0..steps {
        let what = random_edit(&mut project, &mut rng, step);
        seen.extend(check_round(
            &roots,
            &project.overlay,
            &mut memo,
            &format!("{} seed {seed}, step {step}: {what}", shape.name()),
        ));
    }
    seen
}

#[test]
fn random_edit_sequences_agree_on_independent_contexts() {
    let (seeds, steps) = (
        env_usize("BYNK_ORACLE_SEEDS", 3),
        env_usize("BYNK_ORACLE_STEPS", 18),
    );
    let mut seen = BTreeSet::new();
    for seed in 0..seeds as u64 {
        seen.extend(run_sequence(Shape::Independent, seed, steps));
    }
    assert_reaches_the_interesting_diagnostics(&seen);
}

#[test]
fn random_edit_sequences_agree_on_linked_contexts() {
    let (seeds, steps) = (
        env_usize("BYNK_ORACLE_SEEDS", 3),
        env_usize("BYNK_ORACLE_STEPS", 18),
    );
    let mut seen = BTreeSet::new();
    for seed in 0..seeds as u64 {
        seen.extend(run_sequence(Shape::Linked, seed, steps));
    }
    assert_reaches_the_interesting_diagnostics(&seen);
}

/// The edits must actually move the project between clean and several kinds of
/// broken, including the cross-unit body fact, or agreeing proves nothing.
fn assert_reaches_the_interesting_diagnostics(seen: &BTreeSet<&'static str>) {
    assert!(
        seen.contains("bynk.exec.unavailable"),
        "no edit sequence made a commons body edit change another unit's diagnostic: {seen:?}"
    );
    assert!(
        seen.len() >= 3,
        "the edit mix reached too few kinds of diagnostic to be a useful oracle: {seen:?}"
    );
}

/// The oracle must be able to fail: two different projects give different reports.
#[test]
fn the_report_distinguishes_a_changed_diagnostic() {
    let mut project = Project::create(Shape::Independent, 3, "distinguish");
    let roots = Roots::Single(project.root.clone());
    let clean = canonical_report(&analyse_project(&roots, &project.overlay, &SchemaLock::Off));
    let ctx0 = project.root.join("src/ctx0.bynk");
    let broken = edit_source(&project.overlay[&ctx0], false);
    project.overlay.insert(ctx0, broken);
    let after = canonical_report(&analyse_project(&roots, &project.overlay, &SchemaLock::Off));
    assert_ne!(clean, after, "an error must change the report");
    // The commons edit changes another unit's diagnostics, and the report sees it.
    let util = project.root.join("src/util.bynk");
    let util_text = project.overlay[&util].clone();
    project
        .overlay
        .insert(util, util_text.replacen("let _ <- f(n)", "~> f(n)", 1));
    let leaked = canonical_report(&analyse_project(&roots, &project.overlay, &SchemaLock::Off));
    assert_ne!(
        after, leaked,
        "a body edit in util changes leak0's diagnostics"
    );
    assert!(leaked.contains("bynk.exec.unavailable"), "{leaked}");
}
