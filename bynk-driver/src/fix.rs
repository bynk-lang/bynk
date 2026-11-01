//! #2642: `check --fix` — apply the machine-applicable suggestions of a file or
//! project from the command line.
//!
//! The selection rule is [`bynk_syntax::fix`], the one the editor's
//! `source.fixAll.bynk` uses, so the two cannot disagree about what a pass
//! takes. This module adds what a one-shot invocation needs and the editor
//! leaves to the user pressing fix-all again:
//!
//! - **A fixpoint** (DECISION C). Re-check the edited text and re-apply until a
//!   pass takes no edits, capped at [`PASS_CAP`] passes. Past the cap two fixes
//!   are undoing each other: the run is refused.
//! - **A gate** (DECISION D). A pass is accepted only if every diagnostic it
//!   introduces carries a single machine-applicable fix, so the next pass can
//!   resolve it. Any other new diagnostic needs a human, and the run is
//!   refused. The editor's gate is stricter (no new diagnostic at all) because
//!   it applies once; a fixpoint has to allow the cascade it exists for.
//! - **Nothing written until the end** (DECISION D/E). Every pass runs in
//!   memory over the source texts; only files whose text changed are written,
//!   in place, with no formatter pass.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use bynk_emit::project;
use bynk_syntax::error::CompileError;
use bynk_syntax::fix::{apply, lone_fix, select};
use bynk_syntax::span::Span;

use crate::{atomic_write, shown, try_project_options};

/// The most passes that may take edits. A fix that has not settled by then is
/// two suggestions undoing each other.
pub const PASS_CAP: usize = 10;

type Sources = HashMap<PathBuf, String>;

/// One diagnostic of a pass: the key (in the [`Sources`] map) of the file it
/// is in, or `None` for a project-level one, with the error itself.
struct Found {
    file: Option<PathBuf>,
    error: CompileError,
}

/// A file the run changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedFile {
    /// The path as the sources were read, i.e. as `check` would name it.
    pub path: PathBuf,
    pub original: String,
    pub fixed: String,
    /// Edits taken across every pass.
    pub edits: usize,
    /// The edits the first pass took: what `source.fixAll.bynk` would apply
    /// to the same file.
    pub first_pass: Vec<(Span, String)>,
}

/// The outcome of a run that reached a fixpoint the gate accepted.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FixPlan {
    /// Changed files, sorted by path.
    pub files: Vec<FixedFile>,
    /// Passes that took edits.
    pub passes: usize,
}

/// Why a run writes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// [`PASS_CAP`] passes still took edits; the codes of the diagnostics
    /// whose fixes were taken along the way, sorted.
    Oscillates(Vec<&'static str>),
    /// A pass introduced a diagnostic with no single machine-applicable fix.
    NewUnfixable {
        file: Option<PathBuf>,
        category: &'static str,
        message: String,
    },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oscillates(codes) => write!(
                f,
                "fixes did not settle after {PASS_CAP} passes (still being applied: {})",
                codes.join(", ")
            ),
            Self::NewUnfixable {
                file,
                category,
                message,
            } => {
                let place = file.as_deref().map(shown).unwrap_or_default();
                let at = if place.is_empty() {
                    String::new()
                } else {
                    format!(" in {place}")
                };
                write!(
                    f,
                    "applying the fixes would introduce a problem that needs a human{at}: \
                     [{category}] {message}"
                )
            }
        }
    }
}

/// Run the fixpoint over `sources`, diagnosing each state with `diagnose`.
fn fixpoint(
    mut sources: Sources,
    diagnose: impl Fn(&Sources) -> Vec<Found>,
) -> Result<FixPlan, Refusal> {
    let original = sources.clone();
    let mut edit_counts: HashMap<PathBuf, usize> = HashMap::new();
    let mut first_pass: HashMap<PathBuf, Vec<(Span, String)>> = HashMap::new();
    let mut codes: BTreeSet<&'static str> = BTreeSet::new();
    let mut passes = 0;
    let mut previous: Option<Vec<Found>> = None;
    loop {
        let found = diagnose(&sources);
        if let Some(before) = &previous {
            gate(before, &found)?;
        }
        let mut by_file: BTreeMap<&PathBuf, Vec<&CompileError>> = BTreeMap::new();
        for f in &found {
            if let Some(path) = &f.file {
                by_file.entry(path).or_default().push(&f.error);
            }
        }
        let mut picks = Vec::new();
        for (path, errors) in by_file {
            if let Some(text) = sources.get(path) {
                let selection = select(text, errors);
                if !selection.edits.is_empty() {
                    picks.push((path.clone(), selection));
                }
            }
        }
        if picks.is_empty() {
            break;
        }
        codes.extend(picks.iter().flat_map(|(_, s)| s.categories.iter().copied()));
        if passes == PASS_CAP {
            return Err(Refusal::Oscillates(codes.into_iter().collect()));
        }
        passes += 1;
        for (path, selection) in picks {
            let text = &sources[&path];
            let fixed = apply(text, &selection.edits);
            *edit_counts.entry(path.clone()).or_default() += selection.edits.len();
            if passes == 1 {
                first_pass.insert(path.clone(), selection.edits);
            }
            sources.insert(path, fixed);
        }
        previous = Some(found);
    }
    let mut files: Vec<FixedFile> = sources
        .into_iter()
        .filter(|(path, fixed)| original[path] != *fixed)
        .map(|(path, fixed)| FixedFile {
            original: original[&path].clone(),
            edits: edit_counts.get(&path).copied().unwrap_or(0),
            first_pass: first_pass.remove(&path).unwrap_or_default(),
            fixed,
            path,
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(FixPlan { files, passes })
}

/// DECISION D: a pass may add diagnostics only if each carries a single
/// machine-applicable fix. Diagnostics are matched across the edit by
/// (file, category, message), since their spans move; a key with more fixless
/// diagnostics after the pass than before has gained one.
fn gate(before: &[Found], after: &[Found]) -> Result<(), Refusal> {
    type Key<'a> = (&'a Option<PathBuf>, &'static str, &'a str);
    fn fixless<'a>(found: &'a [Found]) -> HashMap<Key<'a>, usize> {
        let mut counts = HashMap::new();
        for f in found.iter().filter(|f| lone_fix(&f.error).is_none()) {
            *counts
                .entry((&f.file, f.error.category, f.error.message.as_str()))
                .or_default() += 1;
        }
        counts
    }
    let was = fixless(before);
    let mut now: Vec<_> = fixless(after).into_iter().collect();
    now.sort();
    for ((file, category, message), count) in now {
        if count > was.get(&(file, category, message)).copied().unwrap_or(0) {
            return Err(Refusal::NewUnfixable {
                file: file.clone(),
                category,
                message: message.to_string(),
            });
        }
    }
    Ok(())
}

/// `path` without `.` components, so `./src/a.bynk` and `src/a.bynk` match.
fn normal(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

/// Plan the fixes for a single `.bynk` file. `None` when the file cannot be
/// read: `check` reports that.
pub fn plan_file(input: &Path) -> Option<Result<FixPlan, Refusal>> {
    let text = std::fs::read_to_string(input).ok()?;
    let filename = shown(input);
    let key = input.to_path_buf();
    let sources = Sources::from([(key.clone(), text)]);
    Some(fixpoint(sources, |sources| {
        let (Ok(diagnostics) | Err(diagnostics)) =
            bynk_emit::check_with_warnings(&sources[&key], &filename);
        diagnostics
            .into_iter()
            .map(|error| Found {
                file: Some(key.clone()),
                error,
            })
            .collect()
    }))
}

/// Plan the fixes for a project rooted at `input`. `None` when the project
/// cannot be set up (a bad manifest, an unreadable lock): `check` reports that.
pub fn plan_project(input: &Path) -> Option<Result<FixPlan, Refusal>> {
    let options = try_project_options(input).ok()?;
    let options = options.schema_registry(crate::schema_lock::read_only(input).ok()?);
    let sources = options.sources.clone()?;
    let keys: HashMap<PathBuf, PathBuf> = sources.keys().map(|k| (normal(k), k.clone())).collect();
    Some(fixpoint(sources, |sources| {
        let check = project::check_project(&options.clone().sources(sources.clone()));
        let root = check.display_root;
        check
            .errors
            .into_iter()
            .map(|ae| Found {
                file: ae
                    .source_path
                    .and_then(|p| keys.get(&normal(&root.join(p))).cloned()),
                error: ae.error,
            })
            .collect()
    }))
}

/// Plan the fixes for one file or project. `None` for a setup problem
/// (unreadable input, bad manifest) that `check` reports.
fn plan_input(input: &Path) -> Option<Result<FixPlan, Refusal>> {
    if input.is_dir() {
        plan_project(input)
    } else {
        plan_file(input)
    }
}

/// `check --fix` over `inputs` (one file or project, or a workspace's
/// members): plan every input, then write. Nothing is written unless every
/// input's plan is accepted, so a refusal anywhere leaves the whole tree as it
/// was. `named` is how each input is named in a message. Returns `false` when
/// the run was refused or could not write; the caller's ordinary report
/// follows either way.
pub(crate) fn fix_all(prog: &str, inputs: &[(&Path, String)]) -> bool {
    let mut plans = Vec::new();
    let mut refused = false;
    for (input, named) in inputs {
        match plan_input(input) {
            Some(Ok(plan)) => plans.push(plan),
            Some(Err(refusal)) => {
                eprintln!("{named}: --fix: {refusal}");
                refused = true;
            }
            None => {}
        }
    }
    if refused {
        eprintln!("{prog}: --fix: nothing was written");
        return false;
    }
    let files = plans.iter().flat_map(|plan| &plan.files);
    // Fail before the first write, so a read-only file leaves the tree as it was.
    for file in files.clone() {
        if std::fs::metadata(&file.path).is_ok_and(|m| m.permissions().readonly()) {
            eprintln!(
                "{prog}: --fix: cannot write `{}`: file is read-only; nothing was written",
                shown(&file.path)
            );
            return false;
        }
    }
    for file in files {
        if let Err(e) = atomic_write(&file.path, &file.fixed) {
            eprintln!("{prog}: --fix: write `{}`: {e}", shown(&file.path));
            return false;
        }
        let n = file.edits;
        eprintln!(
            "fixed: {} ({n} edit{})",
            shown(&file.path),
            if n == 1 { "" } else { "s" }
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_syntax::error::Applicability;

    const M: Applicability = Applicability::MachineApplicable;

    /// A diagnostic that, on `from`, offers replacing the whole text.
    fn rewrite(code: &'static str, from: &str, to: &str) -> CompileError {
        CompileError::new(code, Span::new(0, from.len()), "msg").with_suggestion(
            "fix",
            vec![(Span::new(0, from.len()), to.to_string())],
            M,
        )
    }

    fn key() -> PathBuf {
        PathBuf::from("a.bynk")
    }

    fn run(text: &str, diagnose: impl Fn(&str) -> Vec<CompileError>) -> Result<FixPlan, Refusal> {
        fixpoint(Sources::from([(key(), text.to_string())]), |sources| {
            diagnose(&sources[&key()])
                .into_iter()
                .map(|error| Found {
                    file: Some(key()),
                    error,
                })
                .collect()
        })
    }

    #[test]
    fn a_fix_that_exposes_another_converges_in_more_than_one_pass() {
        let plan = run("x", |text| match text {
            "x" => vec![rewrite("t.one", "x", "y")],
            "y" => vec![rewrite("t.two", "y", "z")],
            _ => vec![],
        })
        .expect("accepted");
        assert_eq!(plan.passes, 2);
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].fixed, "z");
        assert_eq!(plan.files[0].original, "x");
        assert_eq!(plan.files[0].edits, 2);
        // Only the first pass is what the editor would have applied.
        assert_eq!(
            plan.files[0].first_pass,
            vec![(Span::new(0, 1), "y".into())]
        );
    }

    #[test]
    fn a_clean_file_is_not_a_changed_file() {
        let plan = run("x", |_| vec![]).expect("accepted");
        assert_eq!(plan, FixPlan::default());
    }

    #[test]
    fn fixes_that_undo_each_other_are_refused_naming_their_codes() {
        let refusal = run("a", |text| match text {
            "a" => vec![rewrite("t.flip", "a", "b")],
            _ => vec![rewrite("t.flop", "b", "a")],
        })
        .expect_err("never settles");
        assert_eq!(refusal, Refusal::Oscillates(vec!["t.flip", "t.flop"]));
        assert!(refusal.to_string().contains("t.flip, t.flop"));
    }

    #[test]
    fn a_pass_that_introduces_an_unfixable_diagnostic_is_refused() {
        let refusal = run("x", |text| match text {
            "x" => vec![rewrite("t.one", "x", "y")],
            // No suggestion: a human has to look at this.
            _ => vec![CompileError::new(
                "t.stuck",
                Span::new(0, 1),
                "needs a human",
            )],
        })
        .expect_err("refused");
        assert!(
            matches!(
                refusal,
                Refusal::NewUnfixable {
                    category: "t.stuck",
                    ..
                }
            ),
            "{refusal:?}"
        );
    }

    #[test]
    fn a_diagnostic_that_was_already_unfixable_does_not_refuse() {
        let plan = run("x", |text| {
            let mut found = vec![CompileError::new("t.old", Span::new(0, 1), "was here")];
            if text == "x" {
                found.push(rewrite("t.one", "x", "y"));
            }
            found
        })
        .expect("accepted");
        assert_eq!(plan.files[0].fixed, "y");
    }

    #[test]
    fn a_choice_between_candidates_is_left_alone() {
        let ambiguous = CompileError::new("t.pick", Span::new(0, 1), "which?")
            .with_suggestion("one", vec![(Span::new(0, 1), "p".into())], M)
            .with_suggestion("two", vec![(Span::new(0, 1), "q".into())], M);
        let plan = run("x", |_| vec![ambiguous.clone()]).expect("accepted");
        assert!(plan.files.is_empty());
    }
}
