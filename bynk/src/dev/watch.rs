use super::serving::Serving;
use crate::cloudflare::CloudflareApi;
use crate::probe::Provenance;
use bynk_driver::emsg;
use bynk_driver::project::ProjectPaths;
use bynk_driver::project::try_read_project_paths;
use std::path::Path;
use std::path::PathBuf;

/// #524: a change fingerprint over the project's watched inputs — every
/// `.bynk` file under the `[paths] include` roots (author `exclude` subtrees
/// and tool/VCS directories skipped), `bynk.toml` itself, and (#1883, slice
/// 1b) every file in the project's `assets/` folder, so editing a stylesheet
/// rebuilds and rehashes it. Hashes each
/// file's path, mtime, and length, so an edit, add, delete, or rename all
/// change the fingerprint. I/O errors skip the entry rather than aborting the
/// watch.
pub(super) fn watch_fingerprint(project_root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    // The watch loop's own fingerprint has no error channel to surface a
    // malformed `bynk.toml` through — the CLI's real build/check paths
    // (`bynk-driver::project_options`) already do that, via
    // `try_read_project_paths_with`. This falls back to the conventional
    // layout on any error, same as `read_project_paths`'s deleted total form
    // (R3.8, #1113) always did for this one call site.
    let paths = try_read_project_paths(project_root)
        .unwrap_or_else(|_| ProjectPaths::conventional(project_root));
    let excludes: Vec<PathBuf> = paths.exclude.iter().map(|e| project_root.join(e)).collect();
    let mut entries: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();
    let record = |path: &Path, entries: &mut Vec<(PathBuf, std::time::SystemTime, u64)>| {
        if let Ok(meta) = std::fs::metadata(path)
            && let Ok(mtime) = meta.modified()
        {
            entries.push((path.to_path_buf(), mtime, meta.len()));
        }
    };
    record(&project_root.join("bynk.toml"), &mut entries);
    for root in &paths.include {
        collect_bynk_files(&project_root.join(root), &excludes, &mut |p| {
            record(p, &mut entries)
        });
    }
    if let Ok(assets) = bynk_driver::project::assets::list_asset_files(project_root) {
        for rel in assets {
            record(&project_root.join(rel), &mut entries);
        }
    }
    // #2387, slice 2: and every file in `embeds/`, so editing a template
    // rebuilds the code that embeds it.
    if let Ok(embeds) = bynk_driver::project::assets::list_embed_files(project_root) {
        for rel in embeds {
            record(&project_root.join(rel), &mut entries);
        }
    }
    // #2841: and every file in `fixtures/`, so editing a suite's fixture
    // re-runs the tests that read it.
    if let Ok(fixtures) = bynk_driver::project::assets::list_fixture_files(project_root) {
        for rel in fixtures {
            record(&project_root.join(rel), &mut entries);
        }
    }
    // P4b (#2608): and every D1 migration, so a new one is applied.
    if let Ok(migrations) = bynk_driver::project::assets::list_migration_files(project_root) {
        for rel in migrations {
            record(&project_root.join(rel), &mut entries);
        }
    }
    entries.sort();
    let mut hasher = std::hash::DefaultHasher::new();
    for (path, mtime, len) in &entries {
        path.hash(&mut hasher);
        mtime.hash(&mut hasher);
        len.hash(&mut hasher);
    }
    hasher.finish()
}

/// P4b (#2608): each served migration file's content hash as last applied,
/// keyed by its path in the build, so an edit to one wrangler has already
/// applied can be pointed out: wrangler applies a file once, by name.
#[derive(Default)]
pub(super) struct MigrationsSeen(std::collections::BTreeMap<PathBuf, u64>);

/// The bindings of `config`'s `[[d1_databases]]` that name a
/// `migrations_dir`, with that directory.
fn migrating_databases(config: &str) -> Vec<(String, String)> {
    let Ok(doc) = config.parse::<toml::Table>() else {
        return Vec::new();
    };
    doc.get(bynk_project::wrangler_keys::WranglerKey::D1Databases.top_key())
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|db| {
            Some((
                db.get("binding")?.as_str()?.to_string(),
                db.get("migrations_dir")?.as_str()?.to_string(),
            ))
        })
        .collect()
}

/// The `--persist-to` among `bynk dev`'s passthrough arguments, as
/// `["--persist-to", dir]`, so migrations land in the database being served.
pub(super) fn persist_to_args(wrangler_args: &[String]) -> Vec<String> {
    let mut args = wrangler_args.iter();
    while let Some(arg) = args.next() {
        if arg == "--persist-to" {
            return args
                .next()
                .map(|dir| vec![arg.clone(), dir.clone()])
                .unwrap_or_default();
        }
        if let Some(dir) = arg.strip_prefix("--persist-to=") {
            return vec!["--persist-to".to_string(), dir.to_string()];
        }
    }
    Vec::new()
}

/// P4b (#2608): `wrangler d1 migrations apply <binding> --local` for each
/// served Worker's databases with migrations, from the Worker's directory
/// (where `wrangler dev` keeps its local state). Stdin is closed, so wrangler
/// doesn't ask. A failure is reported and the session keeps serving: the
/// developer is mid-edit.
pub(super) fn apply_local_migrations(
    provenance: &Provenance,
    workers_dir: &Path,
    serving: &[Serving],
    wrangler_args: &[String],
    seen: &mut MigrationsSeen,
) {
    for s in serving {
        let worker_dir = workers_dir.join(&s.worker);
        let Ok(config) = std::fs::read_to_string(worker_dir.join("wrangler.toml")) else {
            continue;
        };
        for (binding, dir) in migrating_databases(&config) {
            let api = crate::cloudflare::Account::new(crate::cloudflare::Wrangler::new(
                provenance,
                &worker_dir,
            ));
            match api.d1_migrations_apply_local(
                &worker_dir,
                &binding,
                &persist_to_args(wrangler_args),
            ) {
                // Only an applied file is remembered: one fixed after a
                // failed apply was never applied, so it gets no note.
                Ok(()) => {
                    for file in edited_migrations(&worker_dir.join(&dir), seen) {
                        emsg!(
                            "bynk dev: note: `{file}` changed after it was applied; wrangler applies a migration once, by name, so add a new one to change the schema"
                        );
                    }
                }
                Err(e) if e.is_missing() => {
                    emsg!("bynk: warning: wrangler not found (run `bynk doctor --only deploy`)");
                    return;
                }
                Err(e) if e.ran_and_failed() => emsg!(
                    "bynk dev: warning: could not apply `{}`'s D1 migrations for `{binding}`: {}",
                    s.worker,
                    e.stderr_else_stdout()
                ),
                Err(e) => emsg!(
                    "bynk dev: warning: could not run wrangler to apply `{}`'s D1 migrations: {e}",
                    s.worker
                ),
            }
        }
    }
}

/// The files in `dir` whose content changed since they were last applied,
/// by name, sorted; every file's content is remembered as applied.
fn edited_migrations(dir: &Path, seen: &mut MigrationsSeen) -> Vec<String> {
    use std::hash::{Hash, Hasher};
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut edited = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let mut hasher = std::hash::DefaultHasher::new();
        bytes.hash(&mut hasher);
        let hash = hasher.finish();
        if seen
            .0
            .insert(path.clone(), hash)
            .is_some_and(|before| before != hash)
        {
            edited.push(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    edited.sort();
    edited
}

/// Walk `dir` recursively, calling `visit` for each `.bynk` file. Skips the
/// author `exclude` subtrees and the tool/VCS directories a source walk never
/// wants (`.bynk` build dir, `.git`, `node_modules`, `target`).
fn collect_bynk_files(dir: &Path, excludes: &[PathBuf], visit: &mut dyn FnMut(&Path)) {
    const SKIP_DIRS: [&str; 4] = [".bynk", ".git", "node_modules", "target"];
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            if SKIP_DIRS.iter().any(|s| name == *s) {
                continue;
            }
            if excludes.iter().any(|e| path.starts_with(e)) {
                continue;
            }
            collect_bynk_files(&path, excludes, visit);
        } else if path.extension().is_some_and(|e| e == "bynk") {
            visit(&path);
        }
    }
}

#[cfg(test)]
mod watch_tests {
    use super::*;

    /// #2387, slice 2: an edit to a file in `embeds/` changes the fingerprint
    /// too, so `bynk dev` rebuilds the code that embeds it.
    #[test]
    fn an_embed_edit_changes_the_watch_fingerprint() {
        let root =
            std::env::temp_dir().join(format!("bynk-dev-watch-embed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("embeds")).unwrap();
        std::fs::write(root.join("src/a.bynk"), "commons a\n").unwrap();
        std::fs::write(root.join("embeds/page.html"), "<p>").unwrap();
        let before = watch_fingerprint(&root);
        std::fs::write(root.join("embeds/page.html"), "<p>changed").unwrap();
        let after = watch_fingerprint(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_ne!(before, after, "an embed edit must trigger a rebuild");
    }

    /// #2841: an edit to a file in `fixtures/` changes the fingerprint too, so
    /// `bynk dev` re-runs the tests that read it.
    #[test]
    fn a_fixture_edit_changes_the_watch_fingerprint() {
        let root =
            std::env::temp_dir().join(format!("bynk-dev-watch-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("fixtures")).unwrap();
        std::fs::write(root.join("src/a.bynk"), "commons a\n").unwrap();
        std::fs::write(root.join("fixtures/push.json"), "{}").unwrap();
        let before = watch_fingerprint(&root);
        std::fs::write(root.join("fixtures/push.json"), "{\"ref\":1}").unwrap();
        let after = watch_fingerprint(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_ne!(before, after, "a fixture edit must trigger a re-run");
    }

    /// P4b (#2608): a new migration changes the fingerprint, so `bynk dev`
    /// rebuilds and applies it.
    #[test]
    fn a_new_migration_changes_the_watch_fingerprint() {
        let root = std::env::temp_dir().join(format!("bynk-dev-watch-d1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("migrations/shop-orders")).unwrap();
        std::fs::write(root.join("src/a.bynk"), "commons a\n").unwrap();
        let before = watch_fingerprint(&root);
        std::fs::write(
            root.join("migrations/shop-orders/0001_create.sql"),
            "SELECT 1;",
        )
        .unwrap();
        let after = watch_fingerprint(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_ne!(before, after, "a new migration must trigger a rebuild");
    }

    #[test]
    fn only_databases_with_migrations_are_applied_into_the_served_state() {
        let config = "name = \"shop\"\n\n[[d1_databases]]\nbinding = \"D1_ORDERS\"\ndatabase_name = \"shop-orders\"\ndatabase_id = \"<D1_ORDERS_ID>\"\nmigrations_dir = \"migrations/shop-orders\"\n\n[[d1_databases]]\nbinding = \"D1_PLAIN\"\ndatabase_name = \"shop-plain\"\ndatabase_id = \"<D1_PLAIN_ID>\"\n";
        assert_eq!(
            migrating_databases(config),
            [(
                "D1_ORDERS".to_string(),
                "migrations/shop-orders".to_string()
            )]
        );
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            persist_to_args(&args(&["--port", "1", "--persist-to", ".state"])),
            ["--persist-to", ".state"]
        );
        assert_eq!(
            persist_to_args(&args(&["--persist-to=.state"])),
            ["--persist-to", ".state"]
        );
        assert!(persist_to_args(&args(&["--port", "1"])).is_empty());
    }

    /// P4b (#2608): only a file that was applied, then edited, is pointed
    /// out. One fixed after a failed apply was never remembered (the caller
    /// remembers only after an apply succeeds), so it gets no note.
    #[test]
    fn an_applied_migration_edited_afterwards_is_pointed_out() {
        let dir = std::env::temp_dir().join(format!("bynk-dev-edited-d1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut seen = MigrationsSeen::default();
        std::fs::write(dir.join("0001_create.sql"), "CREATE TABLE t (id TEXT);").unwrap();
        assert!(edited_migrations(&dir, &mut seen).is_empty(), "first apply");
        // A new file, fixed after a failed apply: never remembered, so new.
        std::fs::write(dir.join("0002_more.sql"), "ALTER TABLE t ADD COLUMN n INT;").unwrap();
        assert!(edited_migrations(&dir, &mut seen).is_empty(), "a new file");
        std::fs::write(dir.join("0001_create.sql"), "CREATE TABLE t (id INT);").unwrap();
        let edited = edited_migrations(&dir, &mut seen);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(edited, ["0001_create.sql"]);
    }

    /// #1883, slice 1b: an edit to a file in `assets/` changes the
    /// fingerprint, so `bynk dev` rebuilds and the `.url` hash moves with it.
    #[test]
    fn an_asset_edit_changes_the_watch_fingerprint() {
        let root = std::env::temp_dir().join(format!("bynk-dev-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("src/a.bynk"), "commons a\n").unwrap();
        std::fs::write(root.join("assets/board.css"), "a{}").unwrap();
        let before = watch_fingerprint(&root);
        std::fs::write(root.join("assets/board.css"), "a{color:red}").unwrap();
        let after = watch_fingerprint(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_ne!(before, after, "an asset edit must trigger a rebuild");
    }
}
