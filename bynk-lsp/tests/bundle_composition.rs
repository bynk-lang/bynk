//! The bundle-composition form of `bynk.capability.not_provided` reaches the
//! editor. A bundle with a composition root (`compose.ts`, `composeApp`)
//! builds every context's deps itself, so a context that requires a
//! capability it declares but does not provide is refused at the `given`.
//! `bynk check` raised it from the driver's `run_checks` (#3558), and the
//! editor's analysis now raises it too, from the same predicate
//! (`bynk_check::project_model::composition_root_trigger`). Each test runs
//! both and requires the same diagnostics, at the same span, with the same
//! message and notes.

use std::fs;
use std::path::{Path, PathBuf};

const CATEGORY: &str = "bynk.capability.not_provided";

/// A project on disk, removed when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Scratch {
    fn new(test_name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bynk-lsp-bundle-composition-{test_name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Scratch(root.canonicalize().unwrap())
    }

    fn write(&self, rel: &str, src: &str) {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, src).unwrap();
    }

    /// Copy a checked-in fixture project into this one.
    fn copy_fixture(&self, fixture: &Path) {
        fn walk(from: &Path, to: &Path) {
            for entry in fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let target = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    fs::create_dir_all(&target).unwrap();
                    walk(&entry.path(), &target);
                } else {
                    fs::copy(entry.path(), target).unwrap();
                }
            }
        }
        walk(fixture, &self.0);
    }
}

/// One `bynk.capability.not_provided`, as a comparable value: the file, the
/// text under the span, its 1-based line and column, the message and notes.
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    file: PathBuf,
    at: String,
    line: usize,
    col: usize,
    message: String,
    notes: Vec<String>,
}

fn refusal(file: &Path, text: &str, error: &bynk_syntax::error::CompileError) -> Refusal {
    let before = &text[..error.span.start];
    let line = before.matches('\n').count() + 1;
    let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    Refusal {
        file: file.to_path_buf(),
        at: text[error.span.start..error.span.end].to_string(),
        line,
        col,
        message: error.message.clone(),
        notes: error.notes.clone(),
    }
}

/// The editor's analysis of the manifest-backed project at `root`, as the
/// language server runs it.
fn diagnose(root: &Path) -> bynk_ide::ProjectDiagnostics {
    let roots = bynk_ide::AnalysisRoots::Project(root.to_path_buf());
    let sources = bynk_testkit::read_project_sources(&roots);
    bynk_ide::diagnose_project_with(&roots, &sources, &bynk_ide::SchemaLock::Off)
}

/// What the editor reports for the project.
fn editor(root: &Path) -> Vec<Refusal> {
    let pd = diagnose(root);
    let mut out: Vec<Refusal> = pd
        .files
        .iter()
        .flat_map(|f| {
            f.diagnostics
                .iter()
                .filter(|d| d.error.category == CATEGORY)
                .map(|d| refusal(&f.source_path, &f.text, &d.error))
        })
        .collect();
    assert!(
        pd.unattributed.iter().all(|d| d.error.category != CATEGORY),
        "a refusal is attributed to the file of its `given`"
    );
    out.sort_by(|a, b| (&a.file, a.line, a.col).cmp(&(&b.file, b.line, b.col)));
    out
}

/// What `bynk check` reports for the project.
fn cli(root: &Path) -> Vec<Refusal> {
    let options = bynk_driver::project_options(root).expect("project options");
    let check = bynk_driver::project::check_project(&options);
    let mut out: Vec<Refusal> = check
        .errors
        .iter()
        .filter(|ae| ae.error.category == CATEGORY)
        .map(|ae| {
            let rel = ae.source_path.as_deref().expect("attributed to a file");
            let rel = rel.strip_prefix(root).unwrap_or(rel);
            let text = fs::read_to_string(root.join(rel)).unwrap();
            refusal(rel, &text, &ae.error)
        })
        .collect();
    out.sort_by(|a, b| (&a.file, a.line, a.col).cmp(&(&b.file, b.line, b.col)));
    out
}

/// Both surfaces, required to agree.
fn refusals(root: &Path) -> Vec<Refusal> {
    let editor = editor(root);
    assert_eq!(editor, cli(root), "the editor and `bynk check` disagree");
    editor
}

#[test]
fn the_editor_refuses_what_the_fixture_expects() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../bynk-tests/tests/fixtures/negative/1822_bundle_composition_capability_not_provided",
    );
    let scratch = Scratch::new("fixture");
    scratch.copy_fixture(&fixture);

    let found = refusals(&scratch.0);
    // Only `Warehouse` in `shop.orders`: the agent-only `shop.ledger`'s
    // `Clock` is left to the host, since the root builds no deps for it.
    assert_eq!(
        found,
        [Refusal {
            file: PathBuf::from("src/shop/orders.bynk"),
            at: "Warehouse".into(),
            line: 10,
            col: 66,
            message: "capability `Warehouse` is required here but has no provider in context `shop.orders`; this program's composition root builds every context, so nothing else can supply it".into(),
            notes: vec![
                "`shop.front` consumes `shop.stock`, which has services, so the bundle has a composition root (`composeApp`), which builds `shop.orders` with the providers it declares".into(),
                "add `provides Warehouse = …` to `shop.orders`; a test `stub` replaces it in the cases that stub it, but the program still needs one".into(),
            ],
        }],
    );
}

const ORDERS: &str = "context shop.orders\n\n\
capability Warehouse {\n  fn reserve(sku: String) -> Effect[String]\n}\n\n\
service orders {\n  on call(sku: String) -> Effect[String] given Warehouse {\n    Warehouse.reserve(sku)\n  }\n}\n";

const PUBLISHER: &str = "context shop.audit\n\n\
consumes bynk { Events }\n\n\
event Noted = { n: Int }\n\n\
service audit {\n  on call(n: Int) -> Effect[Int] given Events {\n    let _ <- Events.emit[Noted](Noted { n: n })\n    n\n  }\n}\n";

const MANIFEST: &str = "[project]\nname = \"bundle-composition\"\nversion = \"0.1.0\"\n";

/// The events trigger reads which units call `Events.emit`, from the callee
/// maps the analysis gathers as it checks each unit.
#[test]
fn a_context_that_emits_events_brings_the_root_in() {
    let scratch = Scratch::new("emits");
    scratch.write("bynk.toml", MANIFEST);
    scratch.write("src/shop/orders.bynk", ORDERS);
    scratch.write("src/shop/audit.bynk", PUBLISHER);

    let found = refusals(&scratch.0);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].file, Path::new("src/shop/orders.bynk"));
    assert_eq!(found[0].at, "Warehouse");
    assert_eq!(
        found[0].notes[0],
        "`shop.audit` emits events, so the bundle has a composition root (`composeApp`), which builds `shop.orders` with the providers it declares"
    );
}

/// Without a composition root the host supplies the capability through
/// `__makeSurface(deps)`, so nothing is refused.
#[test]
fn a_bundle_without_a_root_refuses_nothing() {
    let scratch = Scratch::new("no-root");
    scratch.write("bynk.toml", MANIFEST);
    scratch.write("src/shop/orders.bynk", ORDERS);

    assert_eq!(refusals(&scratch.0), []);
}

/// Like the driver, the check runs only on an otherwise-clean program: a unit
/// that failed to check has no callee map to read.
#[test]
fn an_error_elsewhere_holds_the_check_back() {
    let scratch = Scratch::new("unclean");
    scratch.write("bynk.toml", MANIFEST);
    scratch.write("src/shop/orders.bynk", ORDERS);
    scratch.write("src/shop/audit.bynk", PUBLISHER);
    scratch.write(
        "src/shop/broken.bynk",
        "commons shop.broken\n\nfn f() -> Int { \"not an int\" }\n",
    );

    assert_eq!(refusals(&scratch.0), []);
}

/// Raised in the editor, the refusal offers the provider-stub quick fix.
#[test]
fn the_refusal_offers_a_provider_stub() {
    let scratch = Scratch::new("quick-fix");
    scratch.write("bynk.toml", MANIFEST);
    scratch.write("src/shop/orders.bynk", ORDERS);
    scratch.write("src/shop/audit.bynk", PUBLISHER);

    let pd = diagnose(&scratch.0);
    let file = pd
        .files
        .iter()
        .find(|f| f.source_path == Path::new("src/shop/orders.bynk"))
        .expect("orders analysed");
    let given = file
        .diagnostics
        .iter()
        .find(|d| d.error.category == CATEGORY)
        .expect("refused")
        .error
        .span;
    let requests = bynk_lsp::provider_stub::requested(&file.text, &file.diagnostics, given);
    assert!(
        requests
            .iter()
            .any(|r| r.capability == "Warehouse" && r.quick_fix),
        "a quick fix for `Warehouse`"
    );
}
