use super::diagnostics::ErrorSink;
use super::diagnostics::Mode;
use super::diagnostics::ProjectFailure;
use super::entry::sources_to_discovered;
use super::pipeline::{RunChecks, run_checks};
use super::unit::report_internal_faults;
use super::*;
use bynk_check::checker::Types;
use bynk_check::firstparty::Platform;
use bynk_check::project_model::{self, Visibility};
use bynk_check::symbols::FileDeclIndex;
use bynk_check::symbols::UnitTable;
use bynk_emit::emit_model::ImportExt;
use bynk_emit::emit_model::is_worker_asset_path;
use bynk_emit::emit_model::is_worker_migration_path;
use bynk_emit::emit_model::worker_assets_dir;
use bynk_emit::emitter;
use bynk_project::discovery::discover_project_files;
use bynk_project::discovery::parse_sources;
use bynk_project::paths::ProjectPaths;
use bynk_project::paths::try_read_project_paths;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 1A.34: a checker fact the emitter did not find reaches the user as a coded diagnostic
/// at the `.bynk` span of the construct, attributed to its file, naming where to report it,
/// and never as output or a panic. Every fault of the unit is reported, not only the first.
#[test]
fn an_internal_fault_reaches_the_user_as_a_coded_diagnostic_with_a_span() {
    let source = "context shop\n\nfn total(xs: List[Int]) -> Int { xs.fold(0, add) }\n";
    let fold = source.find("xs.fold").unwrap();
    let faults = vec![
        emitter::InternalFault {
            span: bynk_syntax::span::Span::new(fold, fold + "xs.fold(0, add)".len()),
            what: "type for a `fold` call".to_string(),
        },
        emitter::InternalFault {
            span: bynk_syntax::span::Span::new(0, 7),
            what: "default value for field `n`".to_string(),
        },
    ];
    let mut errors = ErrorSink::new();
    report_internal_faults(&mut errors, Path::new("src/shop.bynk"), faults);
    let all = errors.into_all();
    assert_eq!(all.len(), 2, "every fault is reported");
    for attributed in &all {
        assert_eq!(attributed.error.category, emitter::MISSING_CHECKER_FACT);
        assert_eq!(
            attributed.source_path.as_deref(),
            Some(Path::new("src/shop.bynk"))
        );
    }
    assert_eq!(
        (all[0].error.span.start, all[0].error.span.end),
        (fold, fold + "xs.fold(0, add)".len())
    );
    assert!(
        bynk_syntax::diagnostics::REGISTRY
            .iter()
            .any(|d| d.code == emitter::MISSING_CHECKER_FACT),
        "the code is registered"
    );
    let mut rendered = Vec::new();
    all[0]
        .error
        .report_plain_for("src/shop.bynk", source)
        .write(
            ("src/shop.bynk", ariadne::Source::from(source)),
            &mut rendered,
        )
        .unwrap();
    let rendered = String::from_utf8(rendered).unwrap();
    assert!(
        rendered.contains(emitter::MISSING_CHECKER_FACT),
        "{rendered}"
    );
    assert!(rendered.contains("bynk internal error"), "{rendered}");
    assert!(rendered.contains("type for a `fold` call"), "{rendered}");
    assert!(
        rendered.contains("github.com/bynk-lang/bynk/issues"),
        "{rendered}"
    );
    assert!(
        rendered.contains("xs.fold(0, add)"),
        "the source at the span is shown: {rendered}"
    );
}

/// #3562: a member of a group names its D1 database from the Worker (the
/// group's key), and the migrations staged for it sit where `wrangler.toml`
/// points, so `wrangler d1 migrations apply` finds them.
#[test]
fn a_group_members_d1_migrations_are_staged_where_wrangler_points() {
    use bynk_project::worker_groups::{WorkerGroup, WorkerGroups};
    let groups = WorkerGroups::from([(
        "core".to_string(),
        WorkerGroup {
            contexts: vec!["shop.orders".to_string(), "shop.pricing".to_string()],
            ..WorkerGroup::default()
        },
    )]);
    let layout = bynk_project::worker_layout::WorkerLayout::from_groups(&groups);
    let bindings = vec![emitter::bindings::WorkerBinding {
        name: "D1_ORDERS".to_string(),
        source: emitter::bindings::BindingSource::D1Database {
            capability: "Orders".to_string(),
        },
    }];
    let database = emitter::wrangler::d1_database_name(&layout.dir("shop.orders"), "D1_ORDERS");
    assert_eq!(database, "core-orders");
    let mut migrations = bynk_check::d1_migrations::D1Migrations::new();
    migrations.insert(
        database.clone(),
        vec![bynk_check::d1_migrations::MigrationFile {
            name: "0001_init.sql".to_string(),
            sql: "CREATE TABLE t (id TEXT);".to_string(),
        }],
    );
    let toml = emitter::toml_doc::print_toml_document(&emitter::wrangler::emit_wrangler_toml(
        "shop.orders",
        &layout,
        &bindings,
        &[],
        &[],
        &[],
        &[],
        None,
        &Default::default(),
        &migrations,
    ));
    assert!(toml.contains("database_name = \"core-orders\""), "{toml}");
    assert!(
        toml.contains("migrations_dir = \"migrations/core-orders\""),
        "{toml}"
    );
    let mut staged = Vec::new();
    emitter::workers_emit::stage_binding_files(
        &bindings,
        &layout,
        "shop.orders",
        &migrations,
        &mut staged,
    );
    let paths: Vec<_> = staged.iter().map(|f| f.output_path.clone()).collect();
    assert_eq!(
        paths,
        vec![PathBuf::from(
            "workers/core/migrations/core-orders/0001_init.sql"
        )]
    );
}

/// Review of #1904: the `_headers` glob must cover every served path, or
/// every asset silently loses its immutable cache lifetime. Both come
/// from `SERVED_PREFIX`; this pins that they still agree.
#[test]
fn the_headers_glob_covers_every_served_path() {
    let headers = bynk_emit::emitter::wrangler::asset_headers();
    let glob = headers.lines().next().expect("a path line");
    let prefix = glob.strip_suffix('*').expect("a prefix glob");
    for source in ["assets/board.css", "assets/css/a.min.js", "assets/LICENSE"] {
        let served = bynk_check::assets::served_path(source, b"x");
        assert!(served.starts_with(prefix), "{served} not under {glob}");
        let staged = worker_assets_dir("w").join(served.trim_start_matches('/'));
        assert!(
            is_worker_asset_path(&staged),
            "{} is owned",
            staged.display()
        );
    }
}

/// P4b (#2608): `.sql` files are pruned only inside a Worker's copied
/// migrations, never elsewhere in an output directory.
#[test]
fn is_worker_migration_path_matches_only_files_inside_a_worker_migrations_dir() {
    assert!(is_worker_migration_path(Path::new(
        "workers/shop/migrations/shop-orders/0001_create.sql"
    )));
    for p in [
        "workers/shop/migrations",
        "workers/shop/schema.sql",
        "migrations/shop-orders/0001_create.sql",
        "workers/shop/migrations/../../../x.sql",
    ] {
        assert!(!is_worker_migration_path(Path::new(p)), "{p}");
    }
}

/// #1883, slice 0: the ownership rule `write_output`'s reconciliation
/// prunes by. Only files *inside* a Worker's assets directory qualify —
/// not the directory itself, not a same-named directory at another depth,
/// and not a sibling of it in the Worker's own output.
#[test]
fn is_worker_asset_path_matches_only_files_inside_a_worker_assets_dir() {
    let owned = [
        "workers/compat-board/assets/_headers",
        "workers/compat-board/assets/_assets/board.3f9c2a.css",
    ];
    for p in owned {
        assert!(is_worker_asset_path(Path::new(p)), "{p} is owned");
    }
    let not_owned = [
        "workers/compat-board/assets",
        "workers/compat-board/wrangler.toml",
        "workers/compat-board/notes.css",
        "workers/assets/x.css",
        "assets/x.css",
        "compat/assets/x.css",
        "workers/compat-board/sub/assets/x.css",
        // Review of #1902: `components()` keeps `..`, and `dir.join`
        // would then write outside the output root entirely.
        "workers/compat-board/assets/../../../../etc/cron.d/x",
        "workers/compat-board/assets/_assets/../x.css",
        "workers/compat-board/assets/..",
    ];
    for p in not_owned {
        assert!(!is_worker_asset_path(Path::new(p)), "{p} is not owned");
    }
    assert_eq!(
        worker_assets_dir("compat-board").join("_headers"),
        PathBuf::from(owned[0]),
        "the producer's directory and the pruner's rule agree"
    );
}

/// Regression (code review of #1114): a `sources` key that matches none
/// of `trees`'s roots (shouldn't happen for a well-formed map — see
/// `sources_to_discovered`'s own doc) must fall back to the *last* tree,
/// matching the pre-R3.9 two-tree `partition`'s fallback, not silently
/// switch to the first.
#[test]
fn sources_to_discovered_unmatched_key_falls_back_to_the_last_tree() {
    let trees = vec![
        (PathBuf::from("/proj/src"), PathBuf::from("src")),
        (PathBuf::from("/proj/tests"), PathBuf::from("tests")),
    ];
    let mut sources = HashMap::new();
    sources.insert(PathBuf::from("/proj/src/a.bynk"), "commons a\n".to_string());
    sources.insert(
        PathBuf::from("/elsewhere/stray.bynk"),
        "commons stray\n".to_string(),
    );
    let (_, discovered) = sources_to_discovered(&sources, &trees);
    let buckets = discovered.expect("a sources map always yields Some(Discovered)");
    assert_eq!(buckets[0], vec![PathBuf::from("/proj/src/a.bynk")]);
    assert_eq!(
        buckets[1],
        vec![PathBuf::from("/elsewhere/stray.bynk")],
        "an unmatched key must land in the last tree, not the first"
    );
}

/// Content-ownership track (#1086) slice 4: this crate's own
/// `#[cfg(test)]` module can't depend on the cross-crate `bynk-testkit`
/// (that crate depends on `bynk-emit` — a cyclic dev-dependency, the
/// same class of issue slice 3 found for `bynk-ide`). Mirrors
/// `bynk-testkit::compile_options_split` in-crate instead, directly
/// against this crate's own `Roots`/`discover_project_files` — no second
/// resolution to drift from the first. Keyed by the literal discovered
/// path, not canonicalised, matching `bynk-testkit`'s own convention
/// (canonicalising broke a project-consistency check the hard way in
/// slice 3).
/// Content-ownership track (#1086) slice 5: this crate's own tests can't
/// depend on `bynk-testkit` (cyclic — `bynk-testkit` depends on
/// `bynk-emit`), so its handful of sites that build a `Roots` and need
/// real disk content for it mirror `bynk-testkit`'s own read, in-crate.
fn read_disk_sources(roots: &Roots) -> HashMap<PathBuf, String> {
    discover_project_files(roots)
        .into_iter()
        .filter_map(|p| {
            let content = std::fs::read_to_string(&p).ok()?;
            Some((p, content))
        })
        .collect()
}

fn compile_options_split_with_sources(
    project_root: PathBuf,
    paths: ProjectPaths,
) -> CompileOptions {
    let roots = Roots::Split {
        project_root: project_root.clone(),
        paths: paths.clone(),
    };
    let sources = read_disk_sources(&roots);
    CompileOptions::split(project_root, paths).sources(sources)
}

// -- Finding #55/#65: memoized first-party parse must not leak gating ----

/// The first-party parse cache (`firstparty_parsed`) is keyed per-source,
/// not per-project — `phase_parse`'s `consumes`/`uses` gating still runs
/// fresh for every project. Two in-memory projects that gate in
/// *different* first-party units, compiled back-to-back in the same
/// process (so both share the same cache), must each see exactly their
/// own gated-in set: `bynk.map` (which itself `uses bynk.list`) for the
/// first, `bynk.string` alone for the second — never the other's.
#[test]
fn firstparty_cache_does_not_leak_gating_across_projects() {
    let out = compile_in_memory(
        "commons app.only_map\n\nuses bynk.map\n\nfn f() -> Int { 1 }\n",
        BuildTarget::Bundle,
        Default::default(),
    )
    .unwrap_or_else(|_| panic!("`uses bynk.map` should compile"));
    let paths: Vec<String> = out
        .artefacts
        .docs
        .keys()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(paths.iter().any(|p| p == "bynk/map.ts"), "{paths:?}");
    assert!(
        paths.iter().any(|p| p == "bynk/list.ts"),
        "bynk.map itself uses bynk.list, so list must be injected too: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.starts_with("bynk/string")),
        "a project that never uses bynk.string must not gain it: {paths:?}"
    );

    let out2 = compile_in_memory(
        "commons app.only_string\n\nuses bynk.string\n\nfn f() -> String { \"x\" }\n",
        BuildTarget::Bundle,
        Default::default(),
    )
    .unwrap_or_else(|_| panic!("`uses bynk.string` should compile"));
    let paths2: Vec<String> = out2
        .artefacts
        .docs
        .keys()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(paths2.iter().any(|p| p == "bynk/string.ts"), "{paths2:?}");
    assert!(
        !paths2.iter().any(|p| p.starts_with("bynk/map")),
        "the shared first-party parse cache must not leak the first \
         project's gating into this one: {paths2:?}"
    );
}

// -- Finding #64: `check_project` must not bail past an earlier error --

/// `compile_project`'s `Mode::Build` bails at the first structural error
/// (here, `exports capability` naming an undeclared capability) and never
/// reaches the per-unit checking pass — so a completely separate file's
/// test-body type error is silently dropped. `check_project`'s
/// `Mode::Analyse` must report both.
#[test]
fn check_project_reports_a_test_body_error_past_an_earlier_structural_error() {
    let root = scratch_project(
        "check_past_error",
        &[
            ("bynk.toml", "[project]\nname = \"c\"\n"),
            (
                "src/greet.bynk",
                "context greet {\n  exports capability { Bogus }\n}\n",
            ),
            (
                "src/math.bynk",
                "commons math {\n  fn double(n: Int) -> Int { n * 2 }\n}\n",
            ),
            (
                "tests/math_test.bynk",
                "suite math\n\ncase \"broken\" {\n  let x: Int = \"not an int\"\n  expect x == 1\n}\n",
            ),
        ],
    );
    let options = compile_options_split_with_sources(
        root.to_path_buf(),
        try_read_project_paths(&root).expect("well-formed fixture manifest"),
    );

    let check = check_project(&options);
    assert!(check.has_errors());
    let categories: Vec<&str> = check.errors.iter().map(|ae| ae.error.category).collect();
    assert!(
        categories.contains(&"bynk.exports.undeclared_capability"),
        "{categories:?}"
    );
    assert!(
        categories.contains(&"bynk.types.let_annotation_mismatch"),
        "check_project must still report the test body's own type error \
         past the earlier structural error: {categories:?}"
    );

    // The contrast: `compile_project`'s bail-fast `Mode::Build` is the
    // defect `check_project` exists to route `bynk check` around.
    let failure = match compile_project(&options) {
        Err(f) => f,
        Ok(_) => panic!("the structural error must still fail a real build"),
    };
    let failure_categories: Vec<&str> = failure.errors.iter().map(|ae| ae.error.category).collect();
    assert!(
        !failure_categories.contains(&"bynk.types.let_annotation_mismatch"),
        "compile_project must still bail before the test-body check runs \
         (documents why check_project is a separate entry point): {failure_categories:?}"
    );
}

// -- #3162: a suite naming a declaration recovery skipped ----------------

/// Check a project whose `src/` file has a syntax error and whose suite
/// uses the names that file declares; the categories reported, in order.
fn categories_for_broken_target(tag: &str, src: &str, suite: &str) -> Vec<String> {
    let root = scratch_project(
        tag,
        &[
            ("bynk.toml", "[project]\nname = \"c\"\n"),
            ("src/m.bynk", src),
            ("tests/m_test.bynk", suite),
        ],
    );
    let options = compile_options_split_with_sources(
        root.to_path_buf(),
        try_read_project_paths(&root).expect("well-formed fixture manifest"),
    );
    let check = check_project(&options);
    check
        .errors
        .iter()
        .map(|ae| ae.error.category.to_string())
        .collect()
}

/// The report: a syntax error in an agent's handler (here a dangling
/// operator; the report's own `by` parameter name is legal since #3191)
/// drops the whole agent, and the suite constructing it used to add an
/// `unknown_function` per use.
#[test]
fn a_suite_constructing_an_agent_whose_body_failed_to_parse_reports_only_the_syntax_error() {
    let cats = categories_for_broken_target(
        "broken_agent_keyword",
        "context m\n\nagent Counter {\n  key name: String\n  store n: Cell[Int]\n  \
         on call bump(amount: Int) -> Effect[Int] {\n    n +\n  }\n}\n",
        "suite m\n\ncase \"bump\" {\n  let a <- Counter(\"x\").bump(2)\n  \
         expect a == 2\n}\n",
    );
    assert_eq!(cats, ["bynk.parse.expected_expression"], "{cats:?}");
}

/// The same, from an unrelated syntax error (a missing `->` type), and for
/// a function rather than an agent, so the fix is not tied to the repro.
#[test]
fn a_suite_calling_a_function_that_failed_to_parse_reports_only_the_syntax_error() {
    let cats = categories_for_broken_target(
        "broken_fn_other_error",
        "commons m\n\nfn double(n: Int) -> Int { n * }\n",
        "suite m\n\ncase \"d\" {\n  expect double(2) == 4\n}\n",
    );
    // Exactly one syntax error, and no resolve echo of the skipped `double`.
    assert_eq!(cats.len(), 1, "{cats:?}");
    assert!(cats[0].starts_with("bynk.parse."), "{cats:?}");
}

/// A suite may `use` a commons its target does not; that commons' skipped
/// declarations are as visible to the suite as the target's own.
#[test]
fn a_suite_using_a_commons_with_a_skipped_function_reports_only_the_syntax_error() {
    let root = scratch_project(
        "suite_uses_broken_commons",
        &[
            ("bynk.toml", "[project]\nname = \"c\"\n"),
            ("src/m.bynk", "commons m\n\nfn one() -> Int { 1 }\n"),
            (
                "src/util.bynk",
                "commons util\n\nfn helper(n: Int) -> Int { n * }\n",
            ),
            (
                "tests/m_test.bynk",
                "suite m\n\nuses util\n\ncase \"h\" {\n  expect helper(2) == one()\n}\n",
            ),
        ],
    );
    let options = compile_options_split_with_sources(
        root.to_path_buf(),
        try_read_project_paths(&root).expect("well-formed fixture manifest"),
    );
    let cats: Vec<String> = check_project(&options)
        .errors
        .iter()
        .map(|ae| ae.error.category.to_string())
        .collect();
    assert_eq!(cats.len(), 1, "{cats:?}");
    assert!(cats[0].starts_with("bynk.parse."), "{cats:?}");
}

/// The suppression is only for names recovery skipped: a mistyped agent is
/// still unknown, and its note no longer talks of a commons.
#[test]
fn a_suite_naming_an_undeclared_agent_still_gets_unknown_function() {
    let root = scratch_project(
        "undeclared_agent_note",
        &[
            ("bynk.toml", "[project]\nname = \"c\"\n"),
            (
                "src/m.bynk",
                "context m\n\nagent Counter {\n  key name: String\n  store n: Cell[Int]\n  \
                 on call bump(amount: Int) -> Effect[Int] {\n    n\n  }\n}\n",
            ),
            (
                "tests/m_test.bynk",
                "suite m\n\ncase \"bump\" {\n  let a <- Cunter(\"x\").bump(2)\n  \
                 expect a == 2\n}\n",
            ),
        ],
    );
    let options = compile_options_split_with_sources(
        root.to_path_buf(),
        try_read_project_paths(&root).expect("well-formed fixture manifest"),
    );
    let check = check_project(&options);
    let e = check
        .errors
        .iter()
        .find(|ae| ae.error.category == "bynk.resolve.unknown_function")
        .unwrap_or_else(|| panic!("expected unknown_function"));
    let notes = format!("{:?}", e.error.notes);
    assert!(notes.contains("constructs an agent"), "{notes}");
    assert!(!notes.contains("this commons"), "{notes}");
}

/// #3162, integration suites: a `system`-tier suite that constructs an
/// agent the parser skipped (a dangling operator in its handler) gets the
/// syntax error alone, as a unit suite does.
#[test]
fn a_system_suite_naming_a_skipped_agent_reports_only_the_syntax_error() {
    let root = scratch_project(
        "system_suite_skipped_agent",
        &[
            ("bynk.toml", "[project]\nname = \"c\"\n"),
            (
                "src/m.bynk",
                "context m\n\nservice api from http {\n  on GET(\"/\") () -> Effect[HttpResult[String]] by v: Visitor {\n    Ok(\"x\")\n  }\n}\n\n\
                 agent Counter {\n  key name: String\n  store n: Cell[Int]\n  \
                 on call bump(amount: Int) -> Effect[Int] {\n    n +\n  }\n}\n",
            ),
            (
                "tests/m_test.bynk",
                "suite m as system {\n  case \"bump\" {\n    let a <- Counter(\"x\").bump(2)\n    \
                 expect a == 2\n  }\n}\n",
            ),
        ],
    );
    let options = compile_options_split_with_sources(
        root.to_path_buf(),
        try_read_project_paths(&root).expect("well-formed fixture manifest"),
    );
    let cats: Vec<String> = check_project(&options)
        .errors
        .iter()
        .map(|ae| ae.error.category.to_string())
        .collect();
    assert_eq!(cats, ["bynk.parse.expected_expression"], "{cats:?}");
}

// -- Slice 0: file identity is not the unit-validation path ---------------

/// The defect, reproduced hermetically: two `include` roots each holding a
/// file of the same name. Before slice 0 this yielded
/// `["thing.bynk", "thing.bynk"]` — `parse_tree` stripped each tree's own
/// root, so the two were indistinguishable and any consumer mapping by that
/// key dropped one.
///
/// This is the measurement from the track doc's §3.1, inverted into an
/// assertion. It deliberately does **not** read `../examples/todo`:
/// `bynk-emit` is published without an `exclude` list, so a test reaching
/// outside the crate would fail a standalone `cargo test` on the released
/// tarball. That `examples/todo` itself resolves is #647's regression
/// fixture, where the LSP can actually observe it.
#[test]
fn split_roots_give_each_file_a_distinct_identity() {
    let root = scratch_project(
        "identity",
        &[
            ("bynk.toml", "[project]\nname = \"identity\"\n"),
            ("src/thing.bynk", "context thing\n"),
            ("tests/thing.bynk", "suite thing\n"),
        ],
    );
    let roots = Roots::Split {
        project_root: root.to_path_buf(),
        paths: try_read_project_paths(&root).expect("well-formed fixture manifest"),
    };
    let trees = roots.trees();
    assert_eq!(
        trees,
        vec![
            (root.join("src"), PathBuf::from("src")),
            (root.join("tests"), PathBuf::from("tests")),
        ],
        "the fixture must actually be two-rooted"
    );
    let sources = read_disk_sources(&roots);
    let run = run_checks(
        &trees,
        BuildTarget::Bundle,
        Platform::default(),
        ImportExt::Js,
        Mode::Analyse,
        &sources,
        &roots.excludes(),
        None,
        false,
        false,
        false,
        true,
        &SchemaLock::Off,
        None,
        roots.project_root(),
        None,
        None,
        None,
        None,
        None,
        false,
        &Arc::new(Types::new()),
        None,
    );
    let snapshots = match run {
        RunChecks::Bailed { snapshots, .. } => snapshots,
        RunChecks::Checked { snapshots, .. } => snapshots,
    };
    let mut keys: Vec<String> = snapshots
        .iter()
        .map(|(p, _)| p.to_string_lossy().replace('\\', "/"))
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["src/thing.bynk", "tests/thing.bynk"],
        "a file's identity must be project-relative and unique across include roots"
    );
}

/// Regression (code review of #1114): an adapter declared outside the
/// first `include` tree used to have its `binding` module resolved
/// against `trees[0]` unconditionally (`phase_group`'s old `src_root:
/// &Path` parameter) — a project with `include = ["src", "adapters",
/// "tests"]` and an adapter under `adapters/` would look for its binding
/// under `src/`, fail to find it, and report `bynk.adapter.no_binding`
/// even though the binding file exists right beside the adapter.
#[test]
fn adapter_binding_resolves_against_its_own_include_tree() {
    let root = scratch_project(
        "adapter_binding_tree",
        &[
            (
                "bynk.toml",
                "[project]\nname = \"a\"\n\n[paths]\ninclude = [\"src\", \"adapters\", \"tests\"]\n",
            ),
            (
                "src/math.bynk",
                "commons math {\n  fn double(n: Int) -> Int { n * 2 }\n}\n",
            ),
            (
                "adapters/payments.bynk",
                "adapter payments {\n  binding \"./payments.binding.ts\"\n\n  exports capability { Pay }\n\n  capability Pay {\n    fn charge(amount: Int) -> Effect[String]\n  }\n\n  provides Pay = RealPay\n}\n",
            ),
            (
                "adapters/payments.binding.ts",
                "import type { Pay } from \"./payments.js\";\n\nexport class RealPay implements Pay {\n  async charge(amount: number): globalThis.Promise<string> {\n    return \"ok\";\n  }\n}\n",
            ),
        ],
    );
    let options = compile_options_split_with_sources(
        root.to_path_buf(),
        try_read_project_paths(&root).expect("well-formed fixture manifest"),
    );
    let out = compile_project(&options).unwrap_or_else(|f| {
        panic!(
            "adapter with a binding in a non-first include tree must compile: {}",
            render(&f.errors)
        )
    });
    let names: Vec<String> = out
        .artefacts
        .docs
        .keys()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        names.contains(&"payments.binding.ts".to_string()),
        "expected the binding to be copied into the output among {names:?}"
    );
}

/// Regression (code review of the #1114 fix itself): `tree_root_for`
/// compared `pf.abs_path()` (always absolute, via `std::path::absolute`)
/// against `trees`' roots as-is — for the ordinary CLI shape (a relative
/// project root, e.g. `bynk compile .`), every tree root stays relative,
/// so `starts_with` never matched and this silently fell back to
/// `trees[0]` for every file, reproducing the exact bug the fix above
/// exists to close. `scratch_project`-based tests never caught this
/// because their project root is always an absolute temp path.
#[test]
fn tree_root_for_matches_against_a_relative_tree_root() {
    let trees = vec![
        (PathBuf::from("src"), PathBuf::from("src")),
        (PathBuf::from("adapters"), PathBuf::from("adapters")),
    ];
    let root = Path::new("adapters");
    // Relative, exactly as `discover_bynk_files`/`phase_parse` would pass
    // it when `Roots::Split.project_root` is itself relative.
    let rel_path = root.join("payments.bynk");
    let (parsed, _warnings) = parse_sources(
        root,
        Path::new("adapters"),
        &rel_path,
        "adapter payments {\n  binding \"./payments.binding.ts\"\n\n  exports capability { Pay }\n\n  capability Pay {\n    fn charge(amount: Int) -> Effect[String]\n  }\n\n  provides Pay = RealPay\n}\n".to_string(),
    )
    .expect("trivial adapter source must parse");
    assert_eq!(
        project_model::tree_root_for(&trees, &parsed[0]),
        Path::new("adapters"),
        "must resolve to the adapter's own (relative) tree root, not trees[0] (\"src\")"
    );
}

/// Regression (code review of #1114): the emitted test module's
/// discovered-case location used to key off `trees.get(1)`'s prefix
/// unconditionally (`tests_prefix` in `process_tests`/
/// `emit_test_module`) — a project with `include = ["src", "examples",
/// "tests"]` would prefix every discovered case's location with
/// `examples/` (the second tree) even though the suite actually lives
/// under `tests/` (the third).
#[test]
fn discovered_case_location_uses_the_suite_files_own_tree() {
    let root = scratch_project(
        "test_tree_prefix",
        &[
            (
                "bynk.toml",
                "[project]\nname = \"t\"\n\n[paths]\ninclude = [\"src\", \"examples\", \"tests\"]\n",
            ),
            (
                "src/math.bynk",
                "commons math {\n  fn double(n: Int) -> Int { n * 2 }\n}\n",
            ),
            (
                "tests/math_test.bynk",
                "suite math\n\ncase \"doubles\" {\n  expect double(2) == 4\n}\n",
            ),
        ],
    );
    let options = compile_options_split_with_sources(
        root.to_path_buf(),
        try_read_project_paths(&root).expect("well-formed fixture manifest"),
    );
    let out = compile_project(&options).unwrap_or_else(|f| {
        panic!(
            "a suite in the third include tree must compile: {}",
            render(&f.errors)
        )
    });
    let locations: Vec<String> = out
        .discovered
        .iter()
        .flat_map(|s| &s.cases)
        .filter_map(|c| c.location.as_ref())
        .map(|l| l.path.clone())
        .collect();
    assert!(
        locations
            .iter()
            .all(|p| p.starts_with("tests/") && !p.starts_with("examples/")),
        "case locations must key off the suite file's own tree (`tests/`), not the \
         second `include` tree (`examples/`): {locations:?}"
    );
}

/// #57 (testing track): a two-file, cross-referencing project compiled
/// entirely through the public `compile_project` API with no on-disk
/// tree at all — `CompileOptions::sources` replaces what
/// `scratch_project` below has to fake with real temp-directory I/O.
/// Before this seam, exercising `uses` across two units from inside
/// `bynk-emit`'s own tests meant either a `scratch_project` (real files,
/// cleaned up on drop) or `bynk`'s on-disk fixtures one crate up.
#[test]
fn compile_project_with_in_memory_sources_resolves_a_cross_unit_uses() {
    let mut sources = HashMap::new();
    sources.insert(
        PathBuf::from("shapes.bynk"),
        "commons shapes\n\ntype Circle = { radius: Int }\n".to_string(),
    );
    sources.insert(
        PathBuf::from("app.bynk"),
        "commons app\n\nuses shapes\n\nfn area(c: Circle) -> Int {\n  c.radius * c.radius\n}\n"
            .to_string(),
    );
    let options = CompileOptions::single(".").sources(sources);
    let out = compile_project(&options).unwrap_or_else(|f| {
        panic!(
            "in-memory sources project should compile: {:?}",
            ProjectFailure::flatten(f)
        )
    });
    let names: Vec<String> = out
        .artefacts
        .docs
        .keys()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        names.contains(&"shapes.ts".to_string()),
        "expected shapes.ts among {names:?}"
    );
    assert!(
        names.contains(&"app.ts".to_string()),
        "expected app.ts among {names:?}"
    );
    let app_ts = out.artefacts.docs.get(Path::new("app.ts")).unwrap().text();
    assert!(
        app_ts.contains("radius"),
        "app.ts should reference the cross-unit Circle field:\n{app_ts}"
    );
}

/// A throwaway on-disk project, removed on drop — including when the test
/// panics, which a trailing `remove_dir_all` would skip.
struct Scratch(PathBuf);
impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Build a throwaway on-disk project. The e2e fixture suite cannot express
/// these cases: `expected_error.txt` asserts *category strings only*, never
/// a path, so no fixture there can pin attribution — which is precisely why
/// the identity collision survived to slice 0.
fn scratch_project(tag: &str, files: &[(&str, &str)]) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "bynk_slice0_{tag}_{}_{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    for (rel, body) in files {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
    }
    Scratch(dir)
}

/// `AttributedError` is public API without a `Debug` impl; slice 0 is not
/// the increment to add one, so tests render it themselves.
fn render<'a>(errors: impl IntoIterator<Item = &'a AttributedError>) -> String {
    errors
        .into_iter()
        .map(|e| {
            format!(
                "{} @ {}",
                e.error.category,
                e.source_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|| "<unattributed>".into())
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn analyse_split(root: &Path) -> Vec<AttributedError> {
    let roots = Roots::Split {
        project_root: root.to_path_buf(),
        paths: try_read_project_paths(root).expect("well-formed fixture manifest"),
    };
    let trees = roots.trees();
    let sources = read_disk_sources(&roots);
    let run = run_checks(
        &trees,
        BuildTarget::Bundle,
        Platform::default(),
        ImportExt::Js,
        Mode::Analyse,
        &sources,
        &roots.excludes(),
        None,
        false,
        false,
        false,
        true,
        &SchemaLock::Off,
        None,
        roots.project_root(),
        None,
        None,
        None,
        None,
        None,
        false,
        &Arc::new(Types::new()),
        None,
    );
    match run {
        RunChecks::Bailed { errors, .. } => errors.into_all(),
        RunChecks::Checked { errors, .. } => errors.into_all(),
    }
}

/// The defect's user-visible half: a diagnostic in a secondary-root file
/// must be attributed to *that* file. Before slice 0 both roots' files were
/// named `thing.bynk`, so a consumer keying by the attributed path (the LSP
/// does) folded the two together and one file's diagnostics vanished.
#[test]
fn a_secondary_root_diagnostic_is_attributed_to_the_secondary_root_file() {
    let root = scratch_project(
        "attr",
        &[
            ("bynk.toml", "[project]\nname = \"attr\"\n"),
            ("src/thing.bynk", "context thing\n"),
            // Same basename as the src file, different root — the collision.
            //
            // A *parse* error, deliberately: `parse_tree` attributes it as
            // the file is read, which is the path slice 0 changed. (A
            // checker-level error would not do: test bodies are checked by
            // `process_tests` during emit, not in `Mode::Analyse` — which is
            // also why `bynk check` is silent on a broken `case`.)
            ("tests/thing.bynk", "suite thing\n\ncase {{{ \n"),
        ],
    );
    let errors = analyse_split(&root);
    let paths: Vec<String> = errors
        .iter()
        .filter_map(|e| e.source_path.as_ref())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        !paths.is_empty(),
        "the fixture must produce at least one attributed diagnostic; got [{}]",
        render(&errors),
    );
    assert!(
        paths.iter().all(|p| p == "tests/thing.bynk"),
        "a tests-root diagnostic must be attributed to `tests/thing.bynk`, \
         never the bare `thing.bynk` it shares with `src/` — got {paths:?}",
    );
}

/// The layout that ruled out the cheaper repair. Prefixing only the
/// secondary tree would have worked for a `tests/` tree of suites — but
/// ADR 0147 made test-ness *structural*, so `include[1]` may hold an
/// ordinary unit. `check_path_name_alignment` reads `source_path`
/// (tree-relative), so that unit must still validate: `spec/other.bynk`
/// declaring `context other` is aligned, and prefixing its
/// unit-validation path would have broken it.
#[test]
fn a_non_test_unit_in_the_secondary_root_still_validates() {
    let root = scratch_project(
        "nontest",
        &[
            (
                "bynk.toml",
                "[project]\nname = \"nontest\"\n\n[paths]\ninclude = [\"src\", \"spec\"]\n",
            ),
            ("src/thing.bynk", "context thing\n"),
            ("spec/other.bynk", "context other\n"),
        ],
    );
    let errors = analyse_split(&root);
    let alignment: Vec<&AttributedError> = errors
        .iter()
        .filter(|e| e.error.category == "bynk.project.inconsistent_commons_name")
        .collect();
    assert!(
        alignment.is_empty(),
        "a non-test unit in include[1] must still pass path/name alignment — \
         its unit-validation path stays tree-relative; got [{}]",
        render(alignment.iter().copied()),
    );
}

/// End-to-end over the flat layout `conventional()` actually produces, so
/// the normalisation is pinned where it is reachable and not only on the
/// accessor. No e2e fixture has this layout.
#[test]
fn a_flat_project_with_a_manifest_reports_unprefixed_paths() {
    let root = scratch_project(
        "flat",
        &[
            ("bynk.toml", "[project]\nname = \"flat\"\n"),
            // Parse error: `parse_tree` attributes it as the file is read.
            ("thing.bynk", "context thing\n\nfn {{{ \n"),
        ],
    );
    let paths = try_read_project_paths(&root).expect("well-formed fixture manifest");
    assert_eq!(
        paths.include,
        vec![PathBuf::from(".")],
        "the fixture must actually exercise the flat layout"
    );
    let errors = analyse_split(&root);
    let attributed: Vec<String> = errors
        .iter()
        .filter_map(|e| e.source_path.as_ref())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        !attributed.is_empty(),
        "the fixture must produce an attributed diagnostic; got [{}]",
        render(&errors),
    );
    assert!(
        attributed.iter().all(|p| p == "thing.bynk"),
        "a flat project's diagnostics must report `thing.bynk`, never \
         `./thing.bynk` — got {attributed:?}",
    );
}

/// v0.29.4: assembly yields exactly one `UnitInfo` per group, every facet
/// present, with `exports`/`aliases`/`flattened` defaulting to empty for a
/// unit absent from those (genuinely optional) producer maps — reproducing
/// the old `.unwrap_or(empty)` read semantics as a total field.
#[test]
fn assemble_unit_info_yields_one_record_per_group_with_all_facets() {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    groups.insert("a.commons".to_string(), vec![0, 1]);
    groups.insert("a.context".to_string(), vec![2]);

    let mut kinds: BTreeMap<String, UnitKind> = BTreeMap::new();
    kinds.insert("a.commons".to_string(), UnitKind::Commons);
    kinds.insert("a.context".to_string(), UnitKind::Context);

    let mut unit_tables: HashMap<String, UnitTable> = HashMap::new();
    unit_tables.insert("a.commons".to_string(), UnitTable::default());
    unit_tables.insert("a.context".to_string(), UnitTable::default());

    let mut unit_uses: HashMap<String, Vec<String>> = HashMap::new();
    unit_uses.insert("a.context".to_string(), vec!["a.commons".to_string()]);

    let mut unit_consumes: HashMap<String, Vec<String>> = HashMap::new();
    unit_consumes.insert("a.context".to_string(), vec![]);

    // The genuinely-optional maps deliberately omit `a.commons` so the test
    // pins the empty-default behaviour.
    let mut unit_flattened: HashMap<String, HashMap<String, String>> = HashMap::new();
    unit_flattened.insert("a.context".to_string(), HashMap::new());
    let unit_consumes_aliases: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut exports_visibility: HashMap<String, HashMap<String, Visibility>> = HashMap::new();
    exports_visibility.insert("a.context".to_string(), HashMap::new());

    let mut unit_file_index: HashMap<String, FileDeclIndex> = HashMap::new();
    unit_file_index.insert(
        "a.commons".to_string(),
        FileDeclIndex {
            types: HashMap::new(),
            fns: HashMap::new(),
            methods: HashMap::new(),
            agents: HashMap::new(),
            capabilities: HashMap::new(),
        },
    );
    // `a.context` is absent from the file index → its `file_index` defaults.

    let info = project_model::assemble_unit_info(
        &groups,
        &kinds,
        &unit_tables,
        &unit_uses,
        &unit_consumes,
        &unit_flattened,
        &unit_consumes_aliases,
        &exports_visibility,
        &unit_file_index,
    );

    // One record per group, no more.
    assert_eq!(info.len(), 2);
    assert!(info.contains_key("a.commons"));
    assert!(info.contains_key("a.context"));

    // `files` mirrors the `groups` indices.
    assert_eq!(info["a.commons"].files, vec![0, 1]);
    assert_eq!(info["a.context"].files, vec![2]);

    // Non-optional facets are filled from their producer maps.
    assert_eq!(info["a.commons"].kind, UnitKind::Commons);
    assert_eq!(info["a.context"].kind, UnitKind::Context);
    assert_eq!(info["a.context"].uses, vec!["a.commons".to_string()]);

    // Optional facets default to empty for the unit with no entry.
    assert!(info["a.commons"].exports.is_empty());
    assert!(info["a.commons"].aliases.is_empty());
    assert!(info["a.commons"].flattened.is_empty());
    // And the absent `file_index` is an empty index, not a panic.
    assert!(info["a.context"].file_index.types.is_empty());
    assert!(info["a.context"].file_index.fns.is_empty());
    assert!(info["a.context"].file_index.methods.is_empty());
}

// -- #397: analyse_in_memory_with_types exposes expr_types (ADR 0094) -----

#[test]
fn analyse_in_memory_with_types_reports_expr_types_for_clean_source() {
    let src = "commons app.demo\n\nfn good() -> Int {\n  42\n}\n";
    let out = analyse_in_memory_with_types(src, BuildTarget::Bundle, Platform::default());
    assert!(
        out.errors.is_empty(),
        "clean source should have no errors: {:?}",
        out.errors
            .iter()
            .map(|e| &e.error.message)
            .collect::<Vec<_>>()
    );
    let offset = src.find("42").expect("source mentions 42");
    let ty = bynk_check::expr_types::type_at_offset(&out.expr_types, offset, &out.ty_intern);
    assert_eq!(
        ty.map(|t| t.display(&out.ty_intern)),
        Some("Int".to_string())
    );
}

#[test]
fn analyse_in_memory_with_types_is_partial_under_a_sibling_error() {
    // ADR 0094: a function that types cleanly still contributes its
    // `expr_types` even though a *different* function in the same file
    // has an error — `check_record`'s pre-ADR-0094 all-or-nothing gate
    // applied per-file, not per-function, and this is the change that
    // relaxed it. Hover (#397) depends on this: it must not go blank
    // over a well-typed expression just because some other function in
    // the buffer is mid-edit and broken.
    let src = "commons app.demo\n\n\
        fn good() -> Int {\n  42\n}\n\n\
        fn bad() -> Int {\n  \"oops\"\n}\n";
    let out = analyse_in_memory_with_types(src, BuildTarget::Bundle, Platform::default());
    assert!(
        !out.errors.is_empty(),
        "the broken function must still be reported"
    );
    let offset = src.find("42").expect("source mentions 42");
    let ty = bynk_check::expr_types::type_at_offset(&out.expr_types, offset, &out.ty_intern);
    assert_eq!(
        ty.map(|t| t.display(&out.ty_intern)),
        Some("Int".to_string()),
        "the clean function's types must survive the sibling error"
    );
}

#[test]
fn analyse_in_memory_still_returns_exactly_the_typed_variants_errors() {
    // Pins the refactor: `analyse_in_memory` must keep delegating to
    // `analyse_in_memory_with_types` rather than drift into a second,
    // independently-maintained `run_checks` call.
    let src = "commons app.demo\n\nfn bad() -> Int {\n  \"oops\"\n}\n";
    let errs = analyse_in_memory(src, BuildTarget::Bundle, Platform::default());
    let typed = analyse_in_memory_with_types(src, BuildTarget::Bundle, Platform::default());
    assert_eq!(errs.len(), typed.errors.len());
    assert!(!errs.is_empty());
}

// -- T3.3b: `expr_types` is total (R4.3, R2.5, R4.9) -----------------

#[test]
fn a_diagnosed_resolution_failure_records_ty_error_instead_of_nothing() {
    // An empty list literal with no expected element type to infer from
    // (`bynk.types.uninferable_element_type`, `checker.rs`'s `type_of`
    // `ExprKind::ListLit` arm) is a genuine, diagnosed `type_of` failure
    // reachable from a plain `fn` — no resolver/handler-body plumbing
    // needed to reproduce it.
    let src = "commons app.demo\n\nfn bad() -> Int {\n  []\n}\n";
    let out = analyse_in_memory_with_types(src, BuildTarget::Bundle, Platform::default());
    assert!(
        out.errors
            .iter()
            .any(|e| e.error.category == "bynk.types.uninferable_element_type"),
        "expected the uninferable-element-type diagnostic: {:?}",
        out.errors
            .iter()
            .map(|e| &e.error.message)
            .collect::<Vec<_>>()
    );
    let start = src.find("[]").expect("source mentions []");
    let recorded = out
        .expr_types
        .iter()
        .find(|(span, _)| span.start == start && span.end == start + 2)
        .map(|(_, t)| t.display(&out.ty_intern));
    assert_eq!(
        recorded.as_deref(),
        Some("<type error>"),
        "T3.3b: a diagnosed type_of failure must record Ty::Error, not leave the span \
         unrecorded"
    );
    // An editor query reads that recorded `Ty::Error` as "no type".
    assert_eq!(
        bynk_check::expr_types::type_at_offset(&out.expr_types, start, &out.ty_intern),
        None
    );
}
