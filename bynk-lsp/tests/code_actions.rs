//! v0.26 (ADR 0054): the `codeAction` pipeline end-to-end on the v0.25
//! harness pattern — a real project on disk through `diagnose_project`, the
//! checker-authored suggestions riding the diagnostics, and the pure
//! quick-fix computation. No transport: this exercises exactly what the
//! `code_action` handler runs after position conversion.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_lsp::{capability_fixes, code_actions, position};
use tower_lsp::lsp_types::*;

// The handler-side pure modules, included directly (bynk-lsp is a binary
// crate). `code_actions` resolves `crate::position` against the include.

/// A source string in fmt-canonical form, so a header-insertion fix applied to
/// it can be asserted to round-trip through `fmt` unchanged (DECISION C).
fn canon(src: &str) -> String {
    bynk_fmt::format_source(src, &bynk_fmt::FormatOptions::default()).expect("source formats")
}

fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-code-actions-{}-{}",
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

const UNUSED_CAP: &str = "\
context billing.charge

capability Clock {
  fn now() -> Effect[Int]
}

provides Clock = FixedClock {
  fn now() -> Effect[Int] {
    42
  }
}

service charge {
  on call(cents: Int) -> Effect[Int] given Clock {
    cents
  }
}
";

/// Apply one file's `TextEdit`s (as `quick_fixes` emits them) to its text.
fn apply_text_edits(text: &str, edits: &[OneOf<TextEdit, AnnotatedTextEdit>]) -> String {
    let mut plain: Vec<&TextEdit> = edits
        .iter()
        .map(|e| match e {
            OneOf::Left(t) => t,
            OneOf::Right(_) => panic!("unexpected annotated edit"),
        })
        .collect();
    plain.sort_by_key(|e| (e.range.start.line, e.range.start.character));
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for e in plain {
        let start = position::position_to_offset(text, e.range.start).expect("start offset");
        let end = position::position_to_offset(text, e.range.end).expect("end offset");
        out.push_str(&text[last..start]);
        out.push_str(&e.new_text);
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

#[test]
fn unused_capability_quick_fix_round_trips() {
    let root = setup_project("unused", &[("billing/charge.bynk", UNUSED_CAP)]);
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let file = result
        .files
        .iter()
        .find(|f| f.source_path == Path::new("billing/charge.bynk"))
        .expect("context file analysed");
    let diag = file
        .diagnostics
        .iter()
        .find(|d| d.error.category == "bynk.given.unused_capability")
        .expect("the unused-capability diagnostic");

    // The request range sits on the squiggle (the diagnostic's span) — far
    // from where the edit lands (the `given` clause).
    let uri = Url::from_file_path(root.join("billing/charge.bynk")).unwrap();
    let actions = code_actions::quick_fixes(
        &file.text,
        &file.diagnostics,
        diag.error.span,
        &uri,
        Some(3),
    );
    assert_eq!(actions.len(), 1, "exactly one quick-fix offered");
    let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
        panic!("expected a CodeAction");
    };
    assert_eq!(action.title, "remove `Clock` from the `given` clause");
    assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));

    // The edit is versioned with the analysed document version.
    let Some(DocumentChanges::Edits(doc_edits)) = &action.edit.as_ref().unwrap().document_changes
    else {
        panic!("expected versioned document edits");
    };
    assert_eq!(doc_edits.len(), 1);
    assert_eq!(doc_edits[0].text_document.version, Some(3));
    assert_eq!(doc_edits[0].text_document.uri, uri);

    // Applying the WorkspaceEdit drops ` given Clock` exactly, and the
    // edited project re-diagnoses clean.
    let fixed = apply_text_edits(&file.text, &doc_edits[0].edits);
    assert_eq!(fixed, UNUSED_CAP.replace(" given Clock", ""));
    let abs = root.join("billing/charge.bynk");
    let canonical = abs.canonicalize().unwrap_or(abs);
    let mut overlay = HashMap::new();
    overlay.insert(canonical, fixed);
    let post = bynk_ide::diagnose_project(&root, &overlay);
    assert!(
        post.files.iter().all(|f| f.diagnostics.is_empty()),
        "applied fix re-diagnoses clean"
    );
}

#[test]
fn range_away_from_the_diagnostic_offers_nothing() {
    let root = setup_project("away", &[("billing/charge.bynk", UNUSED_CAP)]);
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let file = result
        .files
        .iter()
        .find(|f| f.source_path == Path::new("billing/charge.bynk"))
        .expect("context file analysed");
    let uri = Url::from_file_path(root.join("billing/charge.bynk")).unwrap();
    // A cursor at the top of the file intersects no diagnostic.
    let actions = code_actions::quick_fixes(
        &file.text,
        &file.diagnostics,
        bynk_syntax::span::Span::new(0, 0),
        &uri,
        None,
    );
    assert!(actions.is_empty());
}

const BYNK_LIST_DEPRECATED: &str = "\
commons demo

uses bynk.list

fn doubled(xs: List[Int]) -> List[Int] {
  map(xs, (x) => x * 2)
}

fn firstBig(xs: List[Int]) -> Option[Int] {
  find(xs, (x) => x > 10)
}
";

// -- #852: capability-aware header quick-fixes --

/// The `(title, first-edit-text, versioned document edits)` for a CodeAction.
fn action_parts(a: &CodeActionOrCommand) -> (String, Vec<OneOf<TextEdit, AnnotatedTextEdit>>) {
    let CodeActionOrCommand::CodeAction(action) = a else {
        panic!("expected a CodeAction");
    };
    assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
    let Some(DocumentChanges::Edits(doc_edits)) = &action.edit.as_ref().unwrap().document_changes
    else {
        panic!("expected versioned document edits");
    };
    assert_eq!(
        doc_edits[0].text_document.version,
        Some(7),
        "edit is versioned"
    );
    (action.title.clone(), doc_edits[0].edits.clone())
}

/// Overlay `fixed` on `rel` and assert the whole project re-diagnoses clean,
/// and that the fixed buffer round-trips through `fmt` (#852's acceptance).
///
/// Content-ownership track (#1086) slice 5: every *other* file the project
/// touches (a `uses`d commons, a consumed context) must still be readable —
/// `bynk-emit`'s disk-read fallback used to fill those in silently; now the
/// base sources map (literal, non-canonicalised keys, matching what
/// `discover_project_files` names) must be complete before `rel`'s fixed
/// content overlays on top.
fn assert_project_clean(root: &Path, rel: &str, fixed: &str) {
    let mut overlay = bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
        root.to_path_buf(),
    ));
    overlay.insert(root.join(rel), fixed.to_string());
    let post = bynk_ide::diagnose_project(root, &overlay);
    assert!(
        post.files.iter().all(|f| f.diagnostics.is_empty()),
        "applied fix re-diagnoses clean; residual: {:?}",
        post.files
            .iter()
            .flat_map(|f| f.diagnostics.iter().map(|d| d.error.category))
            .collect::<Vec<_>>()
    );
    // DECISION C: a new clause is inserted in fmt-stable position, so the fixed
    // buffer round-trips through the formatter unchanged.
    let opts = bynk_fmt::FormatOptions::default();
    let once = bynk_fmt::format_source(fixed, &opts).expect("fixed buffer formats");
    assert_eq!(once, fixed, "fixed buffer is not fmt-canonical");
}

fn header_actions_for(
    result: &bynk_ide::ProjectDiagnostics,
    rel: &str,
    category: &str,
    root: &Path,
) -> (String, Vec<CodeActionOrCommand>) {
    let file = result
        .files
        .iter()
        .find(|f| f.source_path == Path::new(rel))
        .expect("file analysed");
    let diag = file
        .diagnostics
        .iter()
        .find(|d| d.error.category == category)
        .unwrap_or_else(|| {
            panic!(
                "expected a `{category}` diagnostic; got {:?}",
                file.diagnostics
                    .iter()
                    .map(|d| d.error.category)
                    .collect::<Vec<_>>()
            )
        });
    let uri = Url::from_file_path(root.join(rel)).unwrap();
    let actions = capability_fixes::header_quick_fixes(
        &file.text,
        &file.diagnostics,
        diag.error.span,
        &uri,
        Some(7),
        &result.index,
    );
    (file.text.clone(), actions)
}

#[test]
fn auto_uses_brings_a_commons_type_into_scope() {
    let widget = canon("commons shared.widget\n\ntype Widget = { size: Int }\n");
    let use_src = canon("commons app.use\n\nfn area(w: Widget) -> Int {\n  w.size\n}\n");
    let root = setup_project(
        "autouses",
        &[
            ("shared/widget.bynk", widget.as_str()),
            ("app/use.bynk", use_src.as_str()),
        ],
    );
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let (text, actions) =
        header_actions_for(&result, "app/use.bynk", "bynk.resolve.unknown_type", &root);
    assert_eq!(actions.len(), 1, "one candidate commons → one `uses` fix");
    let (title, edits) = action_parts(&actions[0]);
    assert_eq!(title, "add `uses shared.widget`");
    let fixed = apply_text_edits(&text, &edits);
    assert!(
        fixed.contains("uses shared.widget"),
        "clause added:\n{fixed}"
    );
    assert_project_clean(&root, "app/use.bynk", &fixed);
}

#[test]
fn auto_uses_brings_a_commons_asset_into_scope() {
    // #1883: an asset is a commons value like a free fn — naming one without
    // its `uses` offers the clause that brings it into scope.
    let brand = canon("commons shared.brand\n\nasset logo = \"assets/logo.svg\"\n");
    let use_src = canon("commons app.use\n\nfn logoUrl() -> String {\n  logo.url\n}\n");
    let root = setup_project(
        "autousesasset",
        &[
            ("shared/brand.bynk", brand.as_str()),
            ("app/use.bynk", use_src.as_str()),
            ("assets/logo.svg", "<svg/>"),
        ],
    );
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let (text, actions) =
        header_actions_for(&result, "app/use.bynk", "bynk.resolve.unknown_name", &root);
    assert_eq!(actions.len(), 1, "one candidate commons → one `uses` fix");
    let (title, edits) = action_parts(&actions[0]);
    assert_eq!(title, "add `uses shared.brand`");
    let fixed = apply_text_edits(&text, &edits);
    assert_project_clean(&root, "app/use.bynk", &fixed);
}

#[test]
fn auto_uses_offers_one_action_per_ambiguous_candidate() {
    // `Widget` is declared in two commons — DECISION A: one action each, no guess.
    let root = setup_project(
        "ambiguous",
        &[
            ("a/w.bynk", "commons a.w\n\ntype Widget = { size: Int }\n"),
            ("b/w.bynk", "commons b.w\n\ntype Widget = { size: Int }\n"),
            (
                "app/use.bynk",
                "commons app.use\n\nfn area(w: Widget) -> Int {\n  w.size\n}\n",
            ),
        ],
    );
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let (_text, actions) =
        header_actions_for(&result, "app/use.bynk", "bynk.resolve.unknown_type", &root);
    let mut titles: Vec<String> = actions.iter().map(|a| action_parts(a).0).collect();
    titles.sort();
    assert_eq!(
        titles,
        vec!["add `uses a.w`".to_string(), "add `uses b.w`".to_string()]
    );
}

#[test]
fn add_consumes_repairs_an_unconsumed_cross_context_call() {
    let prov =
        canon("context svc.prov\n\nservice thing {\n  on call() -> Effect[Int] {\n    1\n  }\n}\n");
    let call = canon(
        "context app.call\n\nservice caller {\n  on call() -> Effect[Int] {\n    let x <- svc.prov.thing()\n    x\n  }\n}\n",
    );
    let root = setup_project(
        "consumes",
        &[
            ("svc/prov.bynk", prov.as_str()),
            ("app/call.bynk", call.as_str()),
        ],
    );
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let (text, actions) = header_actions_for(
        &result,
        "app/call.bynk",
        "bynk.resolve.unconsumed_context",
        &root,
    );
    assert_eq!(actions.len(), 1);
    let (title, edits) = action_parts(&actions[0]);
    assert_eq!(title, "add `consumes svc.prov`");
    let fixed = apply_text_edits(&text, &edits);
    assert!(
        fixed.contains("consumes svc.prov"),
        "clause added:\n{fixed}"
    );
    assert_project_clean(&root, "app/call.bynk", &fixed);
}

#[test]
fn add_consumes_builtin_capability_then_given_reaches_clean() {
    // `Logger` used with neither `consumes bynk` nor `given` → the auto-consumes
    // fix resolves the name; the existing add-`given` fix then reaches clean.
    let relay = canon(
        "context app.relay\n\nservice relay {\n  on call() -> Effect[Int] {\n    let _ <- Logger.info(\"hi\")\n    1\n  }\n}\n",
    );
    let root = setup_project("builtincap", &[("app/relay.bynk", relay.as_str())]);
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let (text, actions) = header_actions_for(
        &result,
        "app/relay.bynk",
        "bynk.resolve.unknown_name",
        &root,
    );
    let logger = actions
        .iter()
        .map(action_parts)
        .find(|(t, _)| t == "add `consumes bynk { Logger }`")
        .expect("consumes-bynk fix offered");
    let consumed = apply_text_edits(&text, &logger.1);
    assert!(
        consumed.contains("consumes bynk { Logger }"),
        "clause added:\n{consumed}"
    );

    // Re-diagnose the consumed buffer → now the `given` diagnostic, with its
    // own machine-applicable fix. Applying it reaches clean.
    let abs = root.join("app/relay.bynk");
    let canonical = abs.canonicalize().unwrap_or(abs.clone());
    let mut overlay = HashMap::new();
    overlay.insert(canonical, consumed.clone());
    let mid = bynk_ide::diagnose_project(&root, &overlay);
    let file = mid
        .files
        .iter()
        .find(|f| f.source_path == Path::new("app/relay.bynk"))
        .unwrap();
    let given = file
        .diagnostics
        .iter()
        .find(|d| d.error.category == "bynk.given.undeclared_capability")
        .expect("undeclared-capability now surfaces");
    let uri = Url::from_file_path(&abs).unwrap();
    let given_actions = code_actions::quick_fixes(
        &consumed,
        &file.diagnostics,
        given.error.span,
        &uri,
        Some(8),
    );
    let CodeActionOrCommand::CodeAction(ga) = &given_actions[0] else {
        panic!()
    };
    let Some(DocumentChanges::Edits(de)) = &ga.edit.as_ref().unwrap().document_changes else {
        panic!()
    };
    let clean = apply_text_edits(&consumed, &de[0].edits);
    assert_project_clean(&root, "app/relay.bynk", &clean);
}

#[test]
fn add_consumes_capability_extends_an_existing_braced_clause() {
    // DECISION C: `Fetch` used with `consumes bynk { Logger }` present → the
    // capability is added into the existing brace list, not a new clause.
    let root = setup_project(
        "extendbrace",
        &[(
            "app/relay.bynk",
            "context app.relay\n\nconsumes bynk { Logger }\n\nservice relay {\n  on call() -> Effect[Int] given Logger {\n    let r <- Fetch.send(\"u\")\n    1\n  }\n}\n",
        )],
    );
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let (text, actions) = header_actions_for(
        &result,
        "app/relay.bynk",
        "bynk.resolve.unknown_name",
        &root,
    );
    let (title, edits) = actions
        .iter()
        .map(action_parts)
        .find(|(t, _)| t.contains("Fetch"))
        .expect("Fetch fix offered");
    assert_eq!(title, "add `Fetch` to `consumes bynk`");
    let fixed = apply_text_edits(&text, &edits);
    assert!(
        fixed.contains("consumes bynk { Logger, Fetch }"),
        "extended in place:\n{fixed}"
    );
}

#[test]
fn add_consumes_into_an_empty_braced_clause_is_canonical() {
    // A `consumes bynk {  }` with non-canonical interior spacing → the clause is
    // rebuilt canonically rather than braces-inserted (the review's cosmetic
    // note). `Logger.info` needs the given too, but the clause edit is the point.
    let root = setup_project(
        "emptybrace",
        &[(
            "app/relay.bynk",
            "context app.relay\n\nconsumes bynk {  }\n\nservice relay {\n  on call() -> Effect[Int] given Logger {\n    let _ <- Logger.info(\"hi\")\n    1\n  }\n}\n",
        )],
    );
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let (text, actions) = header_actions_for(
        &result,
        "app/relay.bynk",
        "bynk.resolve.unknown_name",
        &root,
    );
    let (_title, edits) = actions
        .iter()
        .map(action_parts)
        .find(|(t, _)| t.contains("Logger"))
        .expect("Logger fix offered");
    let fixed = apply_text_edits(&text, &edits);
    assert!(
        fixed.contains("consumes bynk { Logger }") && !fixed.contains("{  }"),
        "empty braces rebuilt canonically:\n{fixed}"
    );
}

#[test]
fn bynk_list_deprecation_quick_fix_rewrites_to_method() {
    // ADR 0116 D6 / v0.91: the `bynk.list` free functions are deprecated with a
    // machine-applicable rewrite to the method form. Project mode (the only
    // place `uses bynk.list` resolves).
    let root = setup_project("bynklist", &[("demo/demo.bynk", BYNK_LIST_DEPRECATED)]);
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let file = result
        .files
        .iter()
        .find(|f| f.source_path == Path::new("demo/demo.bynk"))
        .expect("commons file analysed");
    let uri = Url::from_file_path(root.join("demo/demo.bynk")).unwrap();

    // Every deprecated call is a (non-failing) warning.
    let depr: Vec<_> = file
        .diagnostics
        .iter()
        .filter(|d| d.error.category == "bynk.list.deprecated_function")
        .collect();
    assert_eq!(depr.len(), 2, "map + find both flagged");

    // The `map(xs, f)` fix rewrites to `xs.map(f)`.
    let map_diag = depr
        .iter()
        .find(|d| d.error.message.contains("bynk.list.map"))
        .expect("map deprecation");
    let actions = code_actions::quick_fixes(
        &file.text,
        &file.diagnostics,
        map_diag.error.span,
        &uri,
        Some(1),
    );
    let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
        panic!("expected a CodeAction");
    };
    let Some(DocumentChanges::Edits(doc_edits)) = &action.edit.as_ref().unwrap().document_changes
    else {
        panic!("expected versioned document edits");
    };
    let fixed = apply_text_edits(&file.text, &doc_edits[0].edits);
    assert!(
        fixed.contains("xs.map((x) => x * 2)"),
        "map fix yields the method form; got:\n{fixed}"
    );

    // The `find(xs, p)` fix rewrites to `xs.filter(p).first()`.
    let find_diag = depr
        .iter()
        .find(|d| d.error.message.contains("bynk.list.find"))
        .expect("find deprecation");
    let actions = code_actions::quick_fixes(
        &file.text,
        &file.diagnostics,
        find_diag.error.span,
        &uri,
        Some(1),
    );
    let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
        panic!("expected a CodeAction");
    };
    let Some(DocumentChanges::Edits(doc_edits)) = &action.edit.as_ref().unwrap().document_changes
    else {
        panic!("expected versioned document edits");
    };
    let fixed = apply_text_edits(&file.text, &doc_edits[0].edits);
    assert!(
        fixed.contains("xs.filter((x) => x > 10).first()"),
        "find fix yields filter(...).first(); got:\n{fixed}"
    );
}

/// #2641: the two record-pattern parse errors each carry a machine-applicable
/// fix (#2654: and so does the irrefutable record `let … else`), offered as a quick fix and leaving the file clean once applied.
#[test]
fn record_pattern_parse_errors_offer_quick_fixes() {
    const PRELUDE: &str = "\
commons shop

type Status = enum { Open, Paid }
type Order = { id: String, status: Status, total: Int }

";
    let cases = [
        (
            "bynk.parse.record_pattern_without_rest",
            "add the trailing `..`",
            "fn f(o: Order) -> Int {\n  match o {\n    Order { total } => total\n  }\n}\n",
            "fn f(o: Order) -> Int {\n  match o {\n    Order { total, .. } => total\n  }\n}\n",
        ),
        (
            "bynk.parse.record_pattern_in_is_unparenthesised",
            "parenthesise the record pattern",
            "fn f(o: Order) -> Bool {\n  if o is Order { status: Paid, .. } { true } else { false }\n}\n",
            "fn f(o: Order) -> Bool {\n  if o is (Order { status: Paid, .. }) { true } else { false }\n}\n",
        ),
        // #2654: the same fixes fire in `let` position, and a record `let …
        // else` that cannot fail offers to drop its `else`.
        (
            "bynk.parse.record_pattern_without_rest",
            "add the trailing `..`",
            "fn f(o: Order) -> Int {\n  let Order { total } = o\n  total\n}\n",
            "fn f(o: Order) -> Int {\n  let Order { total, .. } = o\n  total\n}\n",
        ),
        (
            "bynk.types.unreachable_arm",
            "remove the `else`",
            "fn f(o: Order) -> Int {\n  let Order { total, .. } = o else { 0 }\n  total\n}\n",
            "fn f(o: Order) -> Int {\n  let Order { total, .. } = o\n  total\n}\n",
        ),
        (
            "bynk.types.unreachable_arm",
            "remove the `else`",
            "fn f(o: Order) -> Int {\n  let ((Order { total, .. })) = o else match {\n    _ => 0\n  }\n  total\n}\n",
            "fn f(o: Order) -> Int {\n  let Order { total, .. } = o\n  total\n}\n",
        ),
        // #2680: a `..` followed only by wildcards moves to the end.
        (
            "bynk.parse.list_rest_not_last",
            "move the `..` to the end",
            "fn f(xs: List[Int]) -> Int {\n  match xs {\n    [a, .., _, _] => a\n    _ => 0\n  }\n}\n",
            "fn f(xs: List[Int]) -> Int {\n  match xs {\n    [a, _, _, ..] => a\n    _ => 0\n  }\n}\n",
        ),
        // #2699: an inverted range offers to swap its bounds (`MaybeIncorrect`).
        (
            "bynk.types.empty_range_pattern",
            "swap the bounds",
            "fn f(n: Int) -> Int {\n  match n {\n    9..3 => 1\n    _ => 0\n  }\n}\n",
            "fn f(n: Int) -> Int {\n  match n {\n    3..9 => 1\n    _ => 0\n  }\n}\n",
        ),
    ];
    for (n, (code, title, body, fixed_body)) in cases.into_iter().enumerate() {
        let src = format!("{PRELUDE}{body}");
        let root = setup_project(&format!("record-pattern-{n}"), &[("shop.bynk", &src)]);
        let sources =
            bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
        let result = bynk_ide::diagnose_project(&root, &sources);
        let file = result
            .files
            .iter()
            .find(|f| f.source_path == Path::new("shop.bynk"))
            .expect("file analysed");
        let diag = file
            .diagnostics
            .iter()
            .find(|d| d.error.category == code)
            .unwrap_or_else(|| panic!("{code} not reported: {:?}", file.diagnostics.len()));
        let uri = Url::from_file_path(root.join("shop.bynk")).unwrap();
        let actions =
            code_actions::quick_fixes(&file.text, &file.diagnostics, diag.error.span, &uri, None);
        assert_eq!(actions.len(), 1, "{code}: exactly one quick fix");
        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected a CodeAction");
        };
        assert_eq!(action.title, title);
        let Some(DocumentChanges::Edits(doc_edits)) =
            &action.edit.as_ref().unwrap().document_changes
        else {
            panic!("expected document edits");
        };
        let fixed = apply_text_edits(&file.text, &doc_edits[0].edits);
        assert_eq!(fixed, format!("{PRELUDE}{fixed_body}"));
        let abs = root.join("shop.bynk");
        let mut overlay = HashMap::new();
        overlay.insert(abs.canonicalize().unwrap_or(abs), fixed);
        let post = bynk_ide::diagnose_project(&root, &overlay);
        assert!(
            post.files.iter().all(|f| f.diagnostics.is_empty()),
            "{code}: the applied fix re-diagnoses clean"
        );
    }
}

/// #2665 (DECISION G): precise coverage proves a refutable-looking record
/// `let … else` total, so its `else` is `unreachable_arm`, but the pattern
/// still needs the `else` to parse, so no machine-applicable "remove the
/// `else`" fix is offered.
#[test]
fn a_covering_but_refutable_record_let_else_offers_no_removal_fix() {
    const SRC: &str = "\
commons shop

type Only = enum { One }
type Holder = { only: Only, n: Int }

fn f(h: Holder) -> Int {
  let Holder { only: One, n, .. } = h else { 0 }
  n
}
";
    let root = setup_project("record-let-covering-else", &[("shop.bynk", SRC)]);
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let file = result
        .files
        .iter()
        .find(|f| f.source_path == Path::new("shop.bynk"))
        .expect("file analysed");
    let diag = file
        .diagnostics
        .iter()
        .find(|d| d.error.category == "bynk.types.unreachable_arm")
        .expect("the `else` is unreachable");
    assert!(
        diag.error.suggestions.is_empty(),
        "no fix: {:?}",
        diag.error.suggestions
    );
    let uri = Url::from_file_path(root.join("shop.bynk")).unwrap();
    let actions =
        code_actions::quick_fixes(&file.text, &file.diagnostics, diag.error.span, &uri, None);
    assert!(actions.is_empty(), "no quick fix is offered");
}

/// #2680: a non-exhaustive `match` on a list names its missing lengths as list
/// patterns, each carrying a fill-arm suggestion.
#[test]
fn list_witnesses_carry_fill_arm_suggestions() {
    const SRC: &str = "\
commons shop

fn count(xs: List[Int]) -> Int {
  match xs {
    [x] => x
  }
}

fn heads(xs: List[Option[Int]]) -> Int {
  match xs {
    [] => 0
    [Some(n), ..] => n
  }
}
";
    let root = setup_project("fill-arm-list-witnesses", &[("shop.bynk", SRC)]);
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let file = result
        .files
        .iter()
        .find(|f| f.source_path == Path::new("shop.bynk"))
        .expect("file analysed");
    let arms: Vec<String> = file
        .diagnostics
        .iter()
        .filter(|d| d.error.category == "bynk.types.non_exhaustive_match")
        .flat_map(|d| d.error.suggestions.iter())
        .flat_map(|s| s.edits.iter().map(|(_, text)| text.trim().to_string()))
        .collect();
    for want in ["[] => 0", "[_, _, ..] => 0", "[None, ..] => 0"] {
        assert!(arms.contains(&want.to_string()), "{want}: {arms:?}");
    }
}

/// #2699: a non-exhaustive `Int` `match` names the interval it misses, as a
/// range pattern carrying a fill-arm suggestion; single-value gaps are literals.
#[test]
fn range_witnesses_carry_fill_arm_suggestions() {
    const SRC: &str = "\
commons shop

fn bucket(age: Int) -> Int {
  match age {
    ..-1 => 0
    0..12 => 1
    18.. => 2
    14 => 3
  }
}
";
    let root = setup_project("fill-arm-range-witnesses", &[("shop.bynk", SRC)]);
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let file = result
        .files
        .iter()
        .find(|f| f.source_path == Path::new("shop.bynk"))
        .expect("file analysed");
    let arms: Vec<String> = file
        .diagnostics
        .iter()
        .filter(|d| d.error.category == "bynk.types.non_exhaustive_match")
        .flat_map(|d| d.error.suggestions.iter())
        .flat_map(|s| s.edits.iter().map(|(_, text)| text.trim().to_string()))
        .collect();
    for want in ["13 => 0", "15..17 => 0"] {
        assert!(arms.contains(&want.to_string()), "{want}: {arms:?}");
    }
    assert_eq!(arms.len(), 2, "{arms:?}");
}

/// #2665: a non-exhaustive multi-field or record `match` names concrete
/// witnesses, each carrying a fill-arm suggestion; a match with more than the
/// cap reports a count of the rest and offers none, since filling some of the
/// arms would leave it non-exhaustive.
#[test]
fn concrete_witnesses_carry_fill_arm_suggestions_until_capped() {
    let variants: Vec<String> = (1..=12).map(|i| format!("V{i}")).collect();
    let src = format!(
        "\
commons shop

type Pair = | Pair(left: Bool, right: Bool)
type Status = enum {{ Open, Paid, Cancelled }}
type Order = {{ id: String, status: Status, total: Int }}
type Big = enum {{ {} }}

fn pair(p: Pair) -> Int {{
  match p {{
    Pair(true, _) => 1
    Pair(false, true) => 2
  }}
}}

fn order(o: Order) -> Int {{
  match o {{
    Order {{ status: Open, .. }} => 1
  }}
}}

fn big(b: Big) -> Int {{
  match b {{
    V1 => 1
  }}
}}
",
        variants.join(", ")
    );
    let root = setup_project("fill-arm-witnesses", &[("shop.bynk", &src)]);
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let file = result
        .files
        .iter()
        .find(|f| f.source_path == Path::new("shop.bynk"))
        .expect("file analysed");
    let missing: Vec<_> = file
        .diagnostics
        .iter()
        .filter(|d| d.error.category == "bynk.types.non_exhaustive_match")
        .collect();
    let fills = |d: &&bynk_ide::Diagnostic| -> Vec<String> {
        d.error
            .suggestions
            .iter()
            .flat_map(|s| s.edits.iter().map(|(_, text)| text.trim().to_string()))
            .collect()
    };
    let arms: Vec<String> = missing.iter().flat_map(fills).collect();
    assert!(
        arms.contains(&"Pair(false, false) => 0".to_string()),
        "{arms:?}"
    );
    assert!(
        arms.contains(&"Order { status: Paid, .. } => 0".to_string()),
        "{arms:?}"
    );
    assert!(
        arms.contains(&"Order { status: Cancelled, .. } => 0".to_string()),
        "{arms:?}"
    );
    // Eleven variants are missing: eight are reported, with no fill, and the
    // last carries the count of the three left out.
    let big: Vec<_> = missing
        .iter()
        .filter(|d| d.error.message.contains("of `Big`"))
        .collect();
    assert_eq!(big.len(), 8);
    assert!(big.iter().all(|d| d.error.suggestions.is_empty()));
    assert!(
        big.last()
            .unwrap()
            .error
            .notes
            .iter()
            .any(|n| n.contains("and 3 more uncovered patterns")),
        "{:?}",
        big.last().unwrap().error.notes
    );
}
