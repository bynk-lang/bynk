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
    // A project with a manifest is analysed through it, as the server does: its include
    // roots (`src/`) decide where a unit's file should be.
    let roots = if root.join("bynk.toml").exists() {
        bynk_ide::AnalysisRoots::Project(root.to_path_buf())
    } else {
        bynk_ide::AnalysisRoots::SingleTree(root.to_path_buf())
    };
    let result = bynk_ide::diagnose_project_with(&roots, overlay, &bynk_ide::SchemaLock::Off);
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
    // #4455: the handler refuses a rename started on a use under an alias.
    if let Some(why) =
        index_queries::alias_rename_refusal(&pre.index, &pre.snapshots, Path::new(at.0), at.1)
    {
        return Err(why);
    }
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
    // #4183: a unit rename leaves its files' paths to the user; the handler does not
    // count the layout diagnostic that follows.
    let post_diags = index_queries::without_expected_diagnostics(&plan, post.diags.clone());
    index_queries::no_new_diagnostics(&pre.diags, &post_diags)?;
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

/// #4455: renaming a declaration a braced `uses` list selects rewrites the item inside the list,
/// so the rename is not refused (`bynk.uses.unknown_name`) and the renamed project analyses clean.
#[test]
fn rename_rewrites_the_item_in_a_braced_uses_list() {
    let lib =
        "commons lib.y\n\ntype Status = | On | Off\n\nfn describe(s: Status) -> String { \"y\" }\n";
    let app = "commons app.z\n\nuses lib.y { Status, describe }\n\n\
               fn a(s: Status) -> String { describe(s) }\n";
    let root = setup_project("braced-plain", &[("lib/y.bynk", lib), ("app/z.bynk", app)]);
    let pre = analyse(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );
    assert!(pre.diags.is_empty(), "fixture clean: {:?}", pre.diags);

    // A type: the item and its uses in the body.
    let at = offset_of(&pre.snapshots, "lib/y.bynk", "Status", 0);
    let edited = try_rename(&root, &pre, ("lib/y.bynk", at), "State")
        .expect("renaming a selected type is clean");
    let z = &edited[Path::new("app/z.bynk")];
    assert!(
        z.contains("uses lib.y { State, describe }") && z.contains("fn a(s: State)"),
        "the list item and the body are renamed: {z}"
    );

    // A function: the item in the list and its call.
    let at = offset_of(&pre.snapshots, "lib/y.bynk", "describe", 0);
    let edited = try_rename(&root, &pre, ("lib/y.bynk", at), "label")
        .expect("renaming a selected function is clean");
    let z = &edited[Path::new("app/z.bynk")];
    assert!(
        z.contains("uses lib.y { Status, label }") && z.contains("label(s)"),
        "the list item and the call are renamed: {z}"
    );
}

/// #4455: in an aliased item (`{ Status as S }`) only the source name changes: the alias is the
/// unit's own spelling, so its uses are left as they are.
#[test]
fn rename_changes_only_the_source_name_of_an_aliased_uses_item() {
    let lib = "commons lib.y\n\ntype Status = | On | Off\n";
    let app = "commons app.z\n\nuses lib.y { Status as S }\n\nfn a(s: S) -> S { s }\n";
    let root = setup_project(
        "braced-aliased",
        &[("lib/y.bynk", lib), ("app/z.bynk", app)],
    );
    let pre = analyse(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );
    assert!(pre.diags.is_empty(), "fixture clean: {:?}", pre.diags);

    let at = offset_of(&pre.snapshots, "lib/y.bynk", "Status", 0);
    let edited = try_rename(&root, &pre, ("lib/y.bynk", at), "State")
        .expect("renaming through an aliased item is clean");
    let z = &edited[Path::new("app/z.bynk")];
    assert!(
        z.contains("uses lib.y { State as S }") && z.contains("fn a(s: S) -> S"),
        "only the source name changes; the alias and its uses stay: {z}"
    );
}

/// #4455: the item of a braced `uses` list is an indexed reference, so go to definition from it
/// reaches the declaration in the commons and find references lists it. An aliased item is the
/// reference at its source name.
#[test]
fn a_braced_uses_item_is_an_indexed_reference() {
    let lib = "commons lib.y\n\ntype Status = | On | Off\n";
    let app = "commons app.z\n\nuses lib.y { Status as S }\n\nfn a(s: S) -> S { s }\n";
    let root = setup_project(
        "braced-indexed",
        &[("lib/y.bynk", lib), ("app/z.bynk", app)],
    );
    let pre = analyse(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );
    assert!(pre.diags.is_empty(), "fixture clean: {:?}", pre.diags);

    let item = offset_of(&pre.snapshots, "app/z.bynk", "Status", 0);
    let (key, def) = index_queries::definition_at(&pre.index, Path::new("app/z.bynk"), item)
        .expect("the item resolves to its declaration");
    assert_eq!(key.name, "Status");
    assert_eq!(def.path, Path::new("lib/y.bynk"));

    let decl = offset_of(&pre.snapshots, "lib/y.bynk", "Status", 0);
    let sites = index_queries::sites_for(&pre.index, Path::new("lib/y.bynk"), decl, true)
        .expect("the declaration has sites");
    let z = &pre.snapshots[Path::new("app/z.bynk")];
    assert!(
        sites
            .iter()
            .any(|s| s.path == Path::new("app/z.bynk") && &z[s.span.start..s.span.end] == "Status"),
        "the braced item is among the references: {sites:?}"
    );
}

/// #4455: a rename started on a use of a declaration under an alias (`S` of
/// `uses lib.y { Status as S }`) is refused, with the reason, instead of renaming the declaration
/// in the commons and leaving the spot the user is on; `prepareRename` declines there too.
#[test]
fn a_rename_started_on_an_alias_use_is_refused() {
    let lib = "commons lib.y\n\ntype Status = | On | Off\n";
    let app = "commons app.z\n\nuses lib.y { Status as S }\n\nfn a(s: S) -> S { s }\n";
    let root = setup_project(
        "braced-alias-start",
        &[("lib/y.bynk", lib), ("app/z.bynk", app)],
    );
    let pre = analyse(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );
    assert!(pre.diags.is_empty(), "fixture clean: {:?}", pre.diags);

    let on_alias = offset_of(&pre.snapshots, "app/z.bynk", "s: S", 0) + "s: ".len();
    let why = try_rename(&root, &pre, ("app/z.bynk", on_alias), "T")
        .expect_err("a rename started on the alias is refused");
    assert!(
        why.contains("`S` is `Status` under another name"),
        "the refusal says what the spot is: {why}"
    );
    assert!(
        index_queries::alias_rename_refusal(
            &pre.index,
            &pre.snapshots,
            Path::new("app/z.bynk"),
            on_alias
        )
        .is_some(),
        "prepareRename declines on the alias"
    );

    // The source name in the list, and the declaration, still rename.
    let item = offset_of(&pre.snapshots, "app/z.bynk", "Status", 0);
    assert!(try_rename(&root, &pre, ("app/z.bynk", item), "State").is_ok());
}

// ---- #4183 (decision B): renaming a context or commons name ----

const OPS_ADMIN: &str = "\
context ops.admin

service audit {
  on call(id: String) -> Effect[Int] by Caller {
    Effect.pure(1)
  }
}
";

const OPS_BILLING: &str = "\
context ops.billing

actor Admin = Caller where IsContext(ops.admin)

service refunds {
  on call(id: String) -> Effect[Int] by Admin {
    Effect.pure(1)
  }
}
";

const SHOP_CART: &str = "\
context shop

consumes ops.admin

service place {
  on call(id: String) -> Effect[Int] by Caller {
    ops.admin.audit(id)
  }
}
";

fn ops_project(test_name: &str) -> (PathBuf, Analysed) {
    let root = setup_project(
        test_name,
        &[
            ("ops/admin.bynk", OPS_ADMIN),
            ("ops/billing.bynk", OPS_BILLING),
            ("shop.bynk", SHOP_CART),
        ],
    );
    let pre = analyse(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );
    assert!(pre.diags.is_empty(), "fixture clean: {:?}", pre.diags);
    (root, pre)
}

#[test]
fn renaming_a_context_rewrites_every_place_its_name_is_written() {
    let (root, pre) = ops_project("unit-happy");
    // From the `consumes`, the `IsContext(…)` and the call's prefix: the same rename.
    for (file, anchor, n) in [
        ("shop.bynk", "consumes ops.admin", 0),
        ("ops/billing.bynk", "IsContext(ops.admin", 0),
        ("shop.bynk", "ops.admin.audit", 0),
        ("ops/admin.bynk", "context ops.admin", 0),
    ] {
        let start = offset_of(&pre.snapshots, file, anchor, n)
            + anchor.find("ops.admin").expect("name in anchor");
        let edited = try_rename(&root, &pre, (file, start), "ops.staff")
            .unwrap_or_else(|e| panic!("rename from {file} at `{anchor}`: {e}"));
        // Its header, the `consumes`, the call's prefix and the `IsContext` are all the new name.
        assert!(edited[Path::new("ops/admin.bynk")].contains("context ops.staff\n"));
        let shop = &edited[Path::new("shop.bynk")];
        assert!(shop.contains("consumes ops.staff\n"), "{shop}");
        assert!(shop.contains("ops.staff.audit(id)"), "{shop}");
        let billing = &edited[Path::new("ops/billing.bynk")];
        assert!(billing.contains("IsContext(ops.staff)"), "{billing}");
        for (path, text) in &edited {
            assert!(
                !text.contains("ops.admin"),
                "`ops.admin` is left in {}: {text}",
                path.display()
            );
        }
    }
}

#[test]
fn an_alias_of_the_context_keeps_its_own_spelling() {
    let cart = "\
context shop

consumes ops.admin as Admin

service place {
  on call(id: String) -> Effect[Int] by Caller {
    Admin.audit(id)
  }
}
";
    let root = setup_project(
        "unit-alias",
        &[("ops/admin.bynk", OPS_ADMIN), ("shop.bynk", cart)],
    );
    let pre = analyse(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone())),
    );
    assert!(pre.diags.is_empty(), "fixture clean: {:?}", pre.diags);
    let at = offset_of(&pre.snapshots, "shop.bynk", "ops.admin", 0);
    let edited = try_rename(&root, &pre, ("shop.bynk", at), "ops.staff").expect("renames");
    let shop = &edited[Path::new("shop.bynk")];
    assert!(shop.contains("consumes ops.staff as Admin"), "{shop}");
    assert!(
        shop.contains("Admin.audit(id)"),
        "the alias is the unit's own: {shop}"
    );
}

#[test]
fn a_unit_rename_onto_another_unit_is_refused() {
    let (root, pre) = ops_project("unit-collide");
    let at = offset_of(&pre.snapshots, "shop.bynk", "ops.admin", 0);
    let err = try_rename(&root, &pre, ("shop.bynk", at), "ops.billing")
        .expect_err("a rename onto an existing unit");
    assert!(err.contains("already a unit of this project"), "{err}");
    let err = try_rename(&root, &pre, ("shop.bynk", at), "ops.admin")
        .expect_err("a rename to its own name");
    assert!(err.contains("already the unit's name"), "{err}");
}

#[test]
fn a_unit_name_must_be_identifiers_joined_by_dots() {
    let (root, pre) = ops_project("unit-invalid");
    let at = offset_of(&pre.snapshots, "shop.bynk", "ops.admin", 0);
    for bad in [
        "",
        "ops..staff",
        ".staff",
        "ops.",
        "ops.1staff",
        "ops staff",
        "ops/staff",
        "fn.staff",
    ] {
        let err = try_rename(&root, &pre, ("shop.bynk", at), bad)
            .expect_err(&format!("`{bad}` is not a unit name"));
        assert!(err.contains("not a valid"), "`{bad}`: {err}");
    }
    // A single segment is a unit name too, and so is a long one.
    assert!(index_queries::validate_unit_name("staff").is_ok());
    assert!(index_queries::validate_unit_name("a.b.c.d").is_ok());
}

#[test]
fn a_unit_rename_warns_where_the_layout_no_longer_matches() {
    let (_, pre) = ops_project("unit-layout");
    let at = offset_of(&pre.snapshots, "ops/billing.bynk", "ops.admin", 0);
    let plan =
        index_queries::plan_rename(&pre.index, Path::new("ops/billing.bynk"), at, "ops.staff")
            .expect("plans");
    let warnings = index_queries::layout_warnings(&plan, &pre.snapshots);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let w = &warnings[0];
    assert!(
        w.contains("`ops/admin.bynk` is still where `ops.admin` was"),
        "{w}"
    );
    assert!(w.contains("ops/staff.bynk"), "{w}");
    assert!(w.contains("bynk.project.inconsistent_commons_name"), "{w}");

    // A rename to a name the file's path already matches has nothing to warn of: the file is
    // `ops/admin.bynk`, so a unit `ops.admin` is where it belongs.
    let mut ok = plan.clone();
    ok.new_name = "ops.admin".to_string();
    assert!(index_queries::layout_warnings(&ok, &pre.snapshots).is_empty());
}

#[test]
fn only_a_unit_rename_expects_the_layout_diagnostic() {
    let (_, pre) = ops_project("unit-expected");
    let at = offset_of(&pre.snapshots, "ops/billing.bynk", "refunds", 0);
    let plan =
        index_queries::plan_rename(&pre.index, Path::new("ops/billing.bynk"), at, "reimburse")
            .expect("plans a service rename");
    let diags = vec![(
        PathBuf::from("ops/billing.bynk"),
        "bynk.project.inconsistent_commons_name".to_string(),
    )];
    assert_eq!(
        index_queries::without_expected_diagnostics(&plan, diags.clone()),
        diags,
        "another rename's layout diagnostic is a collision as ever"
    );
    assert!(index_queries::layout_warnings(&plan, &pre.snapshots).is_empty());
}

// ---- #4183 review: the other places a unit name is written, and a `src/` layout ----

const TIME: &str = "\
context platform.time

exports capability { Clock }

capability Clock {
  fn now() -> Effect[Int]
}

provides Clock = SystemClock {
  fn now() -> Effect[Int] {
    0
  }
}
";

const STOCK_RESERVE: &str = "\
context shop.stock

service reserve {
  on call(sku: String) -> Effect[Result[(), String]] by Caller {
    Effect.pure(Ok(()))
  }
}
";

const STOCK_RELEASE: &str = "\
context shop.stock

service release {
  on call(sku: String) -> Effect[Int] by Caller {
    Effect.pure(1)
  }
}
";

const CATALOG_UNIT: &str = "\
commons lib.catalog

type Sku = String
";

const ORDERS: &str = "\
context shop.orders

uses lib.catalog
consumes platform.time
consumes shop.stock
consumes bynk { CallOutcome }

service place {
  on call(sku: String) -> Effect[Int] by Caller given platform.time.Clock {
    let t <- platform.time.Clock.now()
    let o <- shop.stock.reserve.outcome(sku)
    let _ <- shop.stock.release(sku)
    Effect.pure(t)
  }
}
";

const ORDERS_SUITE: &str = "\
suite shop.orders {
  case \"holds\" {
    expect 1 == 1
  }
}
";

/// A conventional `src/` project exercising every spelling the checker or the pass records.
fn src_project(test_name: &str) -> (PathBuf, Analysed) {
    let root = setup_project(
        test_name,
        &[
            (
                "bynk.toml",
                "[project]\nname = \"unit-rename\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n",
            ),
            ("src/platform/time.bynk", TIME),
            ("src/shop/stock/reserve.bynk", STOCK_RESERVE),
            ("src/shop/stock/release.bynk", STOCK_RELEASE),
            ("src/lib/catalog.bynk", CATALOG_UNIT),
            ("src/shop/orders.bynk", ORDERS),
            ("src/tests/orders.test.bynk", ORDERS_SUITE),
        ],
    );
    let pre = analyse(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::Project(root.clone())),
    );
    (root, pre)
}

fn renamed_from(
    test: &str,
    file: &str,
    anchor: &str,
    name: &str,
    new: &str,
) -> HashMap<PathBuf, String> {
    let (root, pre) = src_project(test);
    assert!(pre.diags.is_empty(), "fixture clean: {:?}", pre.diags);
    let start = offset_of(&pre.snapshots, file, anchor, 0) + anchor.find(name).unwrap();
    try_rename(&root, &pre, (file, start), new)
        .unwrap_or_else(|e| panic!("rename `{name}` from {file}: {e}"))
}

#[test]
fn renaming_a_context_rewrites_its_capability_call_and_given_spellings() {
    let edited = renamed_from(
        "unit-cap",
        "src/shop/orders.bynk",
        "consumes platform.time",
        "platform.time",
        "platform.clock",
    );
    let orders = &edited[Path::new("src/shop/orders.bynk")];
    assert!(orders.contains("consumes platform.clock\n"), "{orders}");
    assert!(orders.contains("platform.clock.Clock.now()"), "{orders}");
    assert!(orders.contains("given platform.clock.Clock"), "{orders}");
    assert!(!orders.contains("platform.time"), "{orders}");
}

#[test]
fn renaming_a_context_rewrites_an_outcome_call_prefix() {
    let edited = renamed_from(
        "unit-outcome",
        "src/shop/orders.bynk",
        "shop.stock.reserve.outcome",
        "shop.stock",
        "shop.inventory",
    );
    let orders = &edited[Path::new("src/shop/orders.bynk")];
    assert!(
        orders.contains("shop.inventory.reserve.outcome(sku)"),
        "{orders}"
    );
    assert!(orders.contains("shop.inventory.release(sku)"), "{orders}");
}

#[test]
fn renaming_a_split_context_rewrites_every_file_header_and_warns_per_file() {
    let (root, pre) = src_project("unit-split");
    assert!(pre.diags.is_empty(), "fixture clean: {:?}", pre.diags);
    let at = offset_of(
        &pre.snapshots,
        "src/shop/orders.bynk",
        "consumes shop.stock",
        0,
    ) + "consumes ".len();
    let edited =
        try_rename(&root, &pre, ("src/shop/orders.bynk", at), "shop.inventory").expect("renames");
    for file in ["src/shop/stock/reserve.bynk", "src/shop/stock/release.bynk"] {
        assert!(
            edited[Path::new(file)].starts_with("context shop.inventory\n"),
            "{file}"
        );
    }
    let plan = index_queries::plan_rename(
        &pre.index,
        Path::new("src/shop/orders.bynk"),
        at,
        "shop.inventory",
    )
    .expect("plans");
    let warnings = index_queries::layout_warnings(&plan, &pre.snapshots);
    assert_eq!(warnings.len(), 2, "one per file: {warnings:?}");
    for w in &warnings {
        // The include root is kept: moving the file out of `src/` would break the project.
        assert!(w.contains("src/shop/inventory.bynk"), "{w}");
        assert!(w.contains("src/shop/inventory/"), "{w}");
    }
}

#[test]
fn renaming_a_commons_through_a_uses_rewrites_its_header() {
    let edited = renamed_from(
        "unit-commons",
        "src/shop/orders.bynk",
        "uses lib.catalog",
        "lib.catalog",
        "lib.items",
    );
    assert!(edited[Path::new("src/lib/catalog.bynk")].starts_with("commons lib.items\n"));
    assert!(edited[Path::new("src/shop/orders.bynk")].contains("uses lib.items\n"));
}

#[test]
fn renaming_a_context_from_a_suite_target_rewrites_the_suite_and_the_header() {
    let edited = renamed_from(
        "unit-suite",
        "src/tests/orders.test.bynk",
        "suite shop.orders",
        "shop.orders",
        "shop.checkout",
    );
    assert!(edited[Path::new("src/tests/orders.test.bynk")].starts_with("suite shop.checkout {"));
    assert!(edited[Path::new("src/shop/orders.bynk")].starts_with("context shop.checkout\n"));
}

#[test]
fn a_layout_warning_is_silent_when_the_include_root_stripped_path_still_fits() {
    let (_, pre) = src_project("unit-layout-src");
    let at = offset_of(
        &pre.snapshots,
        "src/shop/orders.bynk",
        "consumes shop.stock",
        0,
    ) + "consumes ".len();
    // `shop.stock` in `src/shop/stock/*.bynk`: renaming to `shop.stock` is no change, but a
    // name the (root-stripped) directory already matches must not warn. Here the file
    // `src/platform/time.bynk` declares `platform.time`; asking about that name matches.
    let mut plan = index_queries::plan_rename(
        &pre.index,
        Path::new("src/shop/orders.bynk"),
        at,
        "shop.inventory",
    )
    .expect("plans");
    plan.new_name = "shop.stock".to_string();
    assert!(index_queries::layout_warnings(&plan, &pre.snapshots).is_empty());
}
