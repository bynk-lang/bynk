//! #2100 slice 5: the `source.organizeImports` computation end to end — a real
//! project on disk through `diagnose_project`, the binding index the unused
//! test reads, and the pure organise edit. Every positive case re-diagnoses
//! the edited project clean (what the handler's DECISION B gate checks) and
//! round-trips through `bynk-fmt` unchanged.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_lsp::organise::{self, Mode, Project};
use tower_lsp::lsp_types::*;

fn canon(src: &str) -> String {
    bynk_fmt::format_source(src, &bynk_fmt::FormatOptions::default()).expect("source formats")
}

fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-organise-{}-{}",
        test_name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create test root");
    for (rel, contents) in files {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).expect("create parent");
        fs::write(&p, contents).expect("write file");
    }
    root.canonicalize().unwrap_or(root)
}

/// A diagnosed project and the organise edit for `rel` under `mode`.
struct Run {
    root: PathBuf,
    result: bynk_ide::ProjectDiagnostics,
    snapshots: HashMap<PathBuf, String>,
}

impl Run {
    fn new(test_name: &str, files: &[(&str, &str)]) -> Self {
        let root = setup_project(test_name, files);
        let sources =
            bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
        let result = bynk_ide::diagnose_project(&root, &sources);
        let snapshots = result
            .files
            .iter()
            .map(|f| (f.source_path.clone(), f.text.clone()))
            .collect();
        Run {
            root,
            result,
            snapshots,
        }
    }

    fn text(&self, rel: &str) -> &str {
        &self.snapshots[Path::new(rel)]
    }

    fn organise(&self, rel: &str, mode: Mode) -> Option<String> {
        let project = Project {
            index: &self.result.index,
            snapshots: &self.snapshots,
            unit_sources: &self.result.unit_sources,
        };
        let text = self.text(rel);
        organise::organise(text, Path::new(rel), mode, &project)
            .map(|edit| organise::apply(text, &edit))
    }

    /// The baseline is clean, and `organised` keeps it clean and canonical.
    fn assert_clean(&self, rel: &str, organised: &str) {
        let diags = |r: &bynk_ide::ProjectDiagnostics| -> Vec<String> {
            r.files
                .iter()
                .flat_map(|f| {
                    f.diagnostics
                        .iter()
                        .map(|d| format!("{}: {}", f.source_path.display(), d.error.category))
                })
                .collect()
        };
        assert!(
            diags(&self.result).is_empty(),
            "baseline: {:?}",
            diags(&self.result)
        );
        let mut overlay = bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
            self.root.clone(),
        ));
        overlay.insert(self.root.join(rel), organised.to_string());
        let post = bynk_ide::diagnose_project(&self.root, &overlay);
        assert!(diags(&post).is_empty(), "organised: {:?}", diags(&post));
        assert_eq!(
            canon(organised),
            organised,
            "organised text is not fmt-canonical"
        );
    }
}

fn widget() -> String {
    canon(
        "commons shared.widget\n\ntype Widget = { size: Int }\n\nfn grow(w: Widget) -> Int {\n  w.size + 1\n}\n",
    )
}

fn other() -> String {
    canon("commons shared.other\n\ntype Other = { n: Int }\n")
}

fn clock() -> String {
    canon(
        "context svc.clock\n\nexports capability { Clock, Timer }\n\ncapability Clock {\n  fn now() -> Effect[Int]\n}\n\ncapability Timer {\n  fn tick() -> Effect[Int]\n}\n\nprovides Clock = Sys {\n  fn now() -> Effect[Int] {\n    1\n  }\n}\n\nprovides Timer = T {\n  fn tick() -> Effect[Int] {\n    1\n  }\n}\n",
    )
}

fn prov() -> String {
    canon("context svc.prov\n\nservice thing {\n  on call() -> Effect[Int] {\n    1\n  }\n}\n")
}

const BODY: &str = "service s {\n  on call(w: Widget) -> Effect[Int] given Clock {\n    let n <- Clock.now()\n    n + grow(w)\n  }\n}\n";

fn app(header: &str) -> String {
    canon(&format!("context app.a\n\n{header}\n\n{BODY}"))
}

fn project(test_name: &str, app_src: &str) -> Run {
    Run::new(
        test_name,
        &[
            ("shared/widget.bynk", widget().as_str()),
            ("shared/other.bynk", other().as_str()),
            ("svc/clock.bynk", clock().as_str()),
            ("svc/prov.bynk", prov().as_str()),
            ("app/a.bynk", app_src),
        ],
    )
}

#[test]
fn drops_unused_clauses_and_names_and_sorts_the_rest() {
    let src = app(
        "uses shared.widget\nuses shared.other\nconsumes svc.prov\nconsumes svc.clock { Timer, Clock }",
    );
    let run = project("drops", &src);
    let organised = run.organise("app/a.bynk", Mode::Full).expect("an edit");
    // `shared.other` and `svc.prov` are never referenced; `Timer` is named only
    // by its own `consumes` clause, which is not a use.
    assert_eq!(
        organised,
        app("uses shared.widget\nconsumes svc.clock { Clock }")
    );
    run.assert_clean("app/a.bynk", &organised);
}

#[test]
fn sort_only_keeps_every_clause_and_name() {
    let src = app(
        "uses shared.widget\nuses shared.other\nconsumes svc.prov\nconsumes svc.clock { Timer, Clock }",
    );
    let run = project("sortonly", &src);
    let organised = run.organise("app/a.bynk", Mode::SortOnly).expect("an edit");
    assert_eq!(
        organised,
        app(
            "uses shared.other\nuses shared.widget\nconsumes svc.clock { Clock, Timer }\nconsumes svc.prov"
        )
    );
    run.assert_clean("app/a.bynk", &organised);
}

#[test]
fn an_organised_header_offers_nothing() {
    let src = app("uses shared.widget\nconsumes svc.clock { Clock }");
    let run = project("noop", &src);
    assert_eq!(run.organise("app/a.bynk", Mode::Full), None);
    assert_eq!(run.organise("app/a.bynk", Mode::SortOnly), None);
}

#[test]
fn a_header_with_no_clauses_offers_nothing() {
    let run = Run::new(
        "noclauses",
        &[(
            "app/a.bynk",
            canon("commons app.a\n\nfn f() -> Int {\n  1\n}\n").as_str(),
        )],
    );
    assert_eq!(run.organise("app/a.bynk", Mode::Full), None);
}

#[test]
fn duplicates_are_dropped() {
    let src = app("uses shared.widget\nuses shared.widget\nconsumes svc.clock { Clock, Clock }");
    let run = project("dupes", &src);
    let organised = run.organise("app/a.bynk", Mode::SortOnly).expect("an edit");
    assert_eq!(
        organised,
        app("uses shared.widget\nconsumes svc.clock { Clock }")
    );
}

#[test]
fn dropping_every_clause_removes_the_header_block() {
    let src =
        canon("context app.a\n\nuses shared.other\nconsumes svc.prov\n\nfn f() -> Int {\n  1\n}\n");
    let run = project("dropall", &src);
    let organised = run.organise("app/a.bynk", Mode::Full).expect("an edit");
    assert_eq!(
        organised,
        canon("context app.a\n\nfn f() -> Int {\n  1\n}\n")
    );
    run.assert_clean("app/a.bynk", &organised);
}

#[test]
fn a_use_in_a_sibling_file_of_the_unit_keeps_the_clause() {
    // `uses` is unit-wide: `Widget` is referenced only from `more.bynk`, which
    // has no `uses` of its own, so `main.bynk`'s clause is load-bearing.
    let main = canon("context app.a\n\nuses shared.widget\n\nfn one() -> Int {\n  1\n}\n");
    let more = canon("context app.a\n\nfn area(w: Widget) -> Int {\n  w.size\n}\n");
    let run = Run::new(
        "sibling",
        &[
            ("shared/widget.bynk", widget().as_str()),
            ("app/a/main.bynk", main.as_str()),
            ("app/a/more.bynk", more.as_str()),
        ],
    );
    assert_eq!(run.organise("app/a/main.bynk", Mode::Full), None);
}

#[test]
fn a_first_party_selection_is_pruned_but_a_first_party_unit_is_kept() {
    // `bynk` is not a project unit: its `Fetch` is pruned against the
    // first-party capability references, and the whole-unit `uses bynk.list`
    // (nothing in the index can show it unused) stays.
    let src = canon(
        "context app.relay\n\nuses bynk.list\nconsumes bynk { Logger, Fetch }\n\nservice relay {\n  on call() -> Effect[Int] given Logger {\n    let _ <- Logger.info(\"hi\")\n    1\n  }\n}\n",
    );
    let run = Run::new("firstparty", &[("app/relay.bynk", src.as_str())]);
    let organised = run.organise("app/relay.bynk", Mode::Full).expect("an edit");
    assert_eq!(organised, src.replace("Logger, Fetch", "Logger"));
    run.assert_clean("app/relay.bynk", &organised);
}

#[test]
fn a_commented_clause_moves_with_its_comments_and_is_never_dropped() {
    let src = app(
        "uses shared.widget\n-- kept for the next slice\nuses shared.other  -- legacy\nconsumes svc.clock { Clock }",
    );
    let run = project("comments", &src);
    // Dropping `shared.other` would lose its comments: the full organise
    // declines.
    assert_eq!(run.organise("app/a.bynk", Mode::Full), None);
    // Sorting moves the clause with both of its comments.
    let organised = run.organise("app/a.bynk", Mode::SortOnly).expect("an edit");
    assert_eq!(
        organised,
        app(
            "-- kept for the next slice\nuses shared.other  -- legacy\nuses shared.widget\nconsumes svc.clock { Clock }"
        )
    );
    run.assert_clean("app/a.bynk", &organised);
}

#[test]
fn brace_form_keeps_its_indentation() {
    let src = canon(&format!(
        "context app.a {{\n  uses shared.widget\n  uses shared.other\n  consumes svc.clock {{ Timer, Clock }}\n\n{}}}\n",
        BODY.lines().map(|l| format!("  {l}\n")).collect::<String>()
    ));
    let run = project("brace", &src);
    let organised = run.organise("app/a.bynk", Mode::Full).expect("an edit");
    assert_eq!(
        organised,
        src.replace("\tuses shared.other\n", "")
            .replace("Timer, Clock", "Clock")
    );
    run.assert_clean("app/a.bynk", &organised);
}

#[test]
fn a_file_that_does_not_parse_offers_nothing() {
    let src = app("uses shared.other\nuses shared.widget");
    let run = project("broken", &src);
    let broken = format!("{}\nfn (", run.text("app/a.bynk"));
    let project = Project {
        index: &run.result.index,
        snapshots: &run.snapshots,
        unit_sources: &run.result.unit_sources,
    };
    assert_eq!(
        organise::organise(&broken, Path::new("app/a.bynk"), Mode::SortOnly, &project),
        None
    );
}

#[test]
fn clauses_split_by_another_header_clause_offer_nothing() {
    // Not one run: an `exports` between the `uses` and the `consumes` — the
    // action never moves a clause across other code. (Non-canonical text: the
    // formatter itself would reorder it.)
    let src = format!(
        "context app.a\n\nuses shared.widget\nexports capability {{ }}\nconsumes svc.clock {{ Clock }}\n\n{BODY}"
    );
    let run = project("split", &src);
    assert_eq!(run.organise("app/a.bynk", Mode::SortOnly), None);
}

#[test]
fn the_action_is_a_versioned_organize_imports_edit() {
    let src = app("uses shared.widget\nuses shared.other\nconsumes svc.clock { Clock }");
    let run = project("action", &src);
    let project = Project {
        index: &run.result.index,
        snapshots: &run.snapshots,
        unit_sources: &run.result.unit_sources,
    };
    let text = run.text("app/a.bynk");
    let edit = organise::organise(text, Path::new("app/a.bynk"), Mode::Full, &project).unwrap();
    let uri = Url::from_file_path(run.root.join("app/a.bynk")).unwrap();
    let CodeActionOrCommand::CodeAction(action) = organise::action(text, edit, &uri, Some(4))
    else {
        panic!("expected a CodeAction");
    };
    assert_eq!(action.kind, Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS));
    assert_eq!(action.title, "Organise `uses` and `consumes`");
    let Some(DocumentChanges::Edits(doc_edits)) = &action.edit.unwrap().document_changes else {
        panic!("expected versioned document edits");
    };
    assert_eq!(doc_edits[0].text_document.version, Some(4));
    assert_eq!(doc_edits[0].edits.len(), 1);
}

#[test]
fn a_use_in_a_suite_on_the_unit_keeps_the_clause() {
    // A suite resolves bare names through its target's namespace: `grow` is
    // referenced only from the suite, through `app.a`'s `uses shared.widget`.
    let app_src = canon("commons app.a\n\nuses shared.widget\n\nfn one() -> Int {\n  1\n}\n");
    let suite = canon(
        "suite app.a {\n  case \"grows\" {\n    expect grow(Widget { size: 1 }) == 2\n  }\n}\n",
    );
    let run = Run::new(
        "suite",
        &[
            ("shared/widget.bynk", widget().as_str()),
            ("app/a.bynk", app_src.as_str()),
            ("tests/a.test.bynk", suite.as_str()),
        ],
    );
    let diags: Vec<&str> = run
        .result
        .files
        .iter()
        .flat_map(|f| f.diagnostics.iter().map(|d| d.error.category))
        .collect();
    assert!(diags.is_empty(), "baseline: {diags:?}");
    assert!(
        run.snapshots.contains_key(Path::new("tests/a.test.bynk")),
        "the suite is analysed"
    );
    assert_eq!(run.organise("app/a.bynk", Mode::Full), None);
}
