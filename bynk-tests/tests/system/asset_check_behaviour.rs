//! #1883: `asset` declarations through the real CLI.
//!
//! - A single file checks clean (`bynk check page.bynk`), but its build is
//!   refused with `bynk.asset.needs_project`: it has no project root and
//!   emits no Worker (review of #1903 for the check half).
//! - A project checks clean when the file is in `assets/`, and reports
//!   `bynk.asset.missing_file` when it is not.
//! - `bynk compile --target workers` writes the hashed file and `_headers`
//!   into the Worker's assets directory (slice 1b), read from the project's
//!   `assets/` folder by the driver.
//! - Only the files a declaration names are read: the folder is listed up
//!   front, and a file nothing declares — however large, or unreadable —
//!   never is (review of #1904).
//!
//! Each case works in its own directory under `CARGO_TARGET_TMPDIR`: the CLI
//! may write a schema lock beside its input.

use std::path::{Path, PathBuf};
use std::process::Command;

const PAGE: &str = "commons compat.page\n\nasset style = \"assets/board.css\"\n\nfn sheet() -> String {\n  style.url\n}\n";
// The context declares an asset of its own too (review of #1904): the
// Worker serves both its own and its used commons' files.
const BOARD: &str = "context compat.board\n\nuses compat.page\n\nasset icon = \"assets/icon.svg\"\n\nservice web from http {\n  on GET(\"/\") () -> Effect[HttpResult[String]] by Visitor {\n    Ok(\"\\(sheet())\\(icon.url)\")\n  }\n}\n";
const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
const CSS: &[u8] = b"body{}\n";

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn project(name: &str, with_file: bool) -> PathBuf {
    let dir = scratch(name);
    std::fs::create_dir_all(dir.join("src/compat")).unwrap();
    std::fs::write(dir.join("src/compat/page.bynk"), PAGE).unwrap();
    std::fs::write(dir.join("src/compat/board.bynk"), BOARD).unwrap();
    if with_file {
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/board.css"), CSS).unwrap();
        std::fs::write(dir.join("assets/icon.svg"), SVG).unwrap();
    }
    dir
}

fn bynk(args: &[&str], path: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(args)
        .arg(path)
        .output()
        .expect("run bynk");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

#[test]
fn a_single_file_with_an_asset_checks_clean_and_its_build_is_refused() {
    let dir = scratch("asset-check-single");
    let file = dir.join("page.bynk");
    std::fs::write(&file, PAGE).unwrap();

    let (ok, out) = bynk(&["check"], &file);
    assert!(ok, "`bynk check` must pass, got:\n{out}");

    let ts = dir.join("page.ts");
    let (ok, out) = bynk(&["compile", "--output", ts.to_str().unwrap()], &file);
    assert!(!ok, "`bynk compile` must refuse the asset, got:\n{out}");
    assert!(out.contains("bynk.asset.needs_project"), "{out}");
}

#[test]
fn a_project_checks_clean_with_its_file_and_reports_a_missing_one() {
    let (ok, out) = bynk(&["check"], &project("asset-check-present", true));
    assert!(ok, "`bynk check` on a project must pass, got:\n{out}");

    let (ok, out) = bynk(&["check"], &project("asset-check-absent", false));
    assert!(!ok, "a missing asset file must fail the check, got:\n{out}");
    assert!(out.contains("bynk.asset.missing_file"), "{out}");
}

#[test]
fn a_workers_build_writes_the_hashed_file_and_headers() {
    let dir = project("asset-build-workers", true);
    let out_dir = dir.join("out");
    let (ok, out) = bynk(
        &[
            "compile",
            "--target",
            "workers",
            "--output",
            out_dir.to_str().unwrap(),
        ],
        &dir,
    );
    assert!(ok, "the Workers build must pass, got:\n{out}");

    let assets = out_dir.join("workers/compat-board/assets");
    let mut served: Vec<PathBuf> = std::fs::read_dir(assets.join("_assets"))
        .expect("the Worker's assets directory")
        .map(|e| e.unwrap().path())
        .collect();
    served.sort();
    assert_eq!(
        served.len(),
        2,
        "the commons' file and the context's own: {served:?}"
    );
    let name = served[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with("board.") && name.ends_with(".css"),
        "hashed name: {name}"
    );
    assert_eq!(std::fs::read(&served[0]).unwrap(), CSS);
    assert_eq!(
        std::fs::read(&served[1]).unwrap(),
        SVG,
        "the context's own asset"
    );
    assert!(
        std::fs::read_to_string(assets.join("_headers"))
            .unwrap()
            .contains("immutable")
    );
    let page = std::fs::read_to_string(out_dir.join("compat/page.ts")).unwrap();
    assert!(
        page.contains(&format!("\"/_assets/{name}\"")),
        "the `.url` constant names the written file:\n{page}"
    );
    let wrangler =
        std::fs::read_to_string(out_dir.join("workers/compat-board/wrangler.toml")).unwrap();
    assert!(wrangler.contains("[assets]"), "{wrangler}");
}

/// The positive fixture `1883_asset_workers`, compiled in-process: what the
/// e2e golden files cannot show (they compare documents, never `assets`).
fn compile_fixture(
    target: bynk_driver::project::BuildTarget,
) -> bynk_driver::project::ProjectOutput {
    compile_named_fixture("1883_asset_workers", target)
}

fn compile_named_fixture(
    name: &str,
    target: bynk_driver::project::BuildTarget,
) -> bynk_driver::project::ProjectOutput {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive")
        .join(name);
    let paths =
        bynk_driver::project::try_read_project_paths(&root).expect("well-formed fixture manifest");
    bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_split(root, paths).target(target),
    )
    .unwrap_or_else(|f| panic!("compile failed: {} error(s)", f.errors.len()))
}

#[test]
fn a_workers_build_ships_the_transitive_closure_reproducibly() {
    let first = compile_fixture(bynk_driver::project::BuildTarget::Workers);
    let keys: Vec<String> = first
        .artefacts
        .assets
        .keys()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    // `compat.board` uses `compat.page`, which uses `compat.theme`: both files
    // ship; `compat.api` uses neither and ships nothing.
    assert_eq!(keys.len(), 3, "{keys:?}");
    assert!(
        keys.iter()
            .all(|k| k.starts_with("workers/compat-board/assets/")),
        "{keys:?}"
    );
    assert!(keys.iter().any(|k| k.ends_with("/_headers")), "{keys:?}");
    assert!(
        keys.iter()
            .any(|k| k.contains("/_assets/board.") && k.ends_with(".css")),
        "{keys:?}"
    );
    assert!(
        keys.iter()
            .any(|k| k.contains("/_assets/palette.") && k.ends_with(".css")),
        "{keys:?}"
    );

    let second = compile_fixture(bynk_driver::project::BuildTarget::Workers);
    assert_eq!(
        first.artefacts.assets, second.artefacts.assets,
        "reproducible"
    );
}

#[test]
fn a_bundle_build_ships_no_files() {
    let out = compile_fixture(bynk_driver::project::BuildTarget::Bundle);
    assert!(
        out.artefacts.assets.is_empty(),
        "only a Worker serves files"
    );
}

/// Review of #1904: a deployable Bundle build ships no files, so it warns
/// about each asset whose `.url` it emits; `bynk test`'s Bundle build (with
/// tests) does not.
#[test]
fn a_deployable_bundle_build_warns_that_its_assets_are_not_served() {
    let dir = project("asset-build-bundle", true);
    let (ok, out) = bynk(
        &["compile", "--output", dir.join("out").to_str().unwrap()],
        &dir,
    );
    assert!(ok, "a Bundle build still succeeds, got:\n{out}");
    assert!(out.contains("bynk.asset.not_served"), "{out}");

    let quiet = compile_fixture(bynk_driver::project::BuildTarget::Bundle);
    assert!(
        !quiet
            .warnings
            .iter()
            .any(|w| w.error.category == "bynk.asset.not_served"),
        "a build with tests (as `bynk test` compiles) does not warn"
    );
}

/// #1883, slice 2, on `1883_asset_fixed_paths_workers`: `compat.board`
/// consumes `compat.widgets` and serves its fragment, so it ships the widget's
/// content-hashed badge — but not the widget's fixed `/robots.txt`, which
/// stays at the root of the widget's own Worker. Its own `/favicon.ico` sits
/// at the root of its assets directory, outside `_assets/`.
#[test]
fn fixed_paths_stay_home_and_consumed_hashed_files_ship() {
    let out = compile_named_fixture(
        "1883_asset_fixed_paths_workers",
        bynk_driver::project::BuildTarget::Workers,
    );
    let keys = |worker: &str| -> Vec<String> {
        let prefix = format!("workers/{worker}/assets/");
        out.artefacts
            .assets
            .keys()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .filter_map(|k| k.strip_prefix(&prefix).map(str::to_string))
            .map(|k| match k.strip_prefix("_assets/badge.") {
                // Elide the content hash, which the fixture's bytes fix anyway.
                Some(_) => "_assets/badge.<hash>.svg".to_string(),
                None => k,
            })
            .collect()
    };
    assert_eq!(
        keys("compat-board"),
        vec!["_assets/badge.<hash>.svg", "_headers", "favicon.ico"]
    );
    assert_eq!(
        keys("compat-widgets"),
        vec!["_assets/badge.<hash>.svg", "_headers", "robots.txt"]
    );
}

/// The fixture's `assets/` folder, listed from disk, plus an undeclared
/// `assets/huge.bin` — read through a reader that records every key it is
/// asked for and refuses `huge.bin`, as an unreadable or enormous file would.
fn recording_folder(
    root: &Path,
    refuse: &'static str,
) -> (
    bynk_driver::project::assets::AssetFolder,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    use bynk_driver::project::assets::AssetFolder;
    let disk = AssetFolder::on_disk(root).unwrap();
    let mut files = disk.files().clone();
    files.insert("assets/huge.bin".to_string());
    let reads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = reads.clone();
    let folder = AssetFolder::new(
        files,
        std::sync::Arc::new(move |key: &str| {
            log.lock().unwrap().push(key.to_string());
            if key == refuse {
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            disk.read(key)
        }),
    );
    (folder, reads)
}

fn fixture_options(
    folder: bynk_driver::project::assets::AssetFolder,
) -> bynk_driver::project::CompileOptions {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/1883_asset_workers");
    let paths =
        bynk_driver::project::try_read_project_paths(&root).expect("well-formed fixture manifest");
    bynk_testkit::compile_options_split(root, paths)
        .target(bynk_driver::project::BuildTarget::Workers)
        .asset_files(folder)
}

/// Review of #1904: a file in `assets/` that no declaration names is listed
/// but never read, so a large media file there costs a build nothing — and an
/// unreadable one does not break it. Each declared file is read once.
#[test]
fn a_build_reads_only_the_declared_files() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/1883_asset_workers");
    let (folder, reads) = recording_folder(&root, "assets/huge.bin");
    let out = bynk_driver::project::compile_project(&fixture_options(folder))
        .unwrap_or_else(|f| panic!("compile failed: {} error(s)", f.errors.len()));
    assert_eq!(
        out.artefacts.assets,
        compile_fixture(bynk_driver::project::BuildTarget::Workers)
            .artefacts
            .assets,
        "the undeclared file changes nothing"
    );

    let mut reads = reads.lock().unwrap().clone();
    assert!(
        !reads.iter().any(|k| k == "assets/huge.bin"),
        "an undeclared file is never read: {reads:?}"
    );
    let total = reads.len();
    reads.sort();
    reads.dedup();
    assert_eq!(
        total,
        reads.len(),
        "each declared file is read once: {reads:?}"
    );
    assert_eq!(
        reads,
        vec!["assets/board.css", "assets/css/palette.css"],
        "exactly the declared files"
    );
}

/// A declared file the reader fails on is `bynk.asset.unreadable_file`, not a
/// panic or a missing file.
#[test]
fn an_unreadable_declared_file_is_reported() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/1883_asset_workers");
    let (folder, _) = recording_folder(&root, "assets/board.css");
    let Err(failure) = bynk_driver::project::compile_project(&fixture_options(folder)) else {
        panic!("an unreadable declared file must fail the build");
    };
    let codes: Vec<_> = failure.errors.iter().map(|e| e.error.category).collect();
    assert_eq!(codes, vec!["bynk.asset.unreadable_file"], "{codes:?}");
}

/// Through the real CLI: an undeclared file in `assets/` that cannot be read
/// does not affect the build. (Skipped where permissions do not bite, e.g.
/// as root.)
#[cfg(unix)]
#[test]
fn an_unreadable_undeclared_file_does_not_affect_the_cli_build() {
    use std::os::unix::fs::PermissionsExt;
    let dir = project("asset-build-unreadable-undeclared", true);
    let locked = dir.join("assets/locked.bin");
    std::fs::write(&locked, b"secret").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&locked).is_ok() {
        eprintln!(
            "skipped: permissions do not stop this user reading `{}`",
            locked.display()
        );
        return;
    }
    let out_dir = dir.join("out");
    let (ok, out) = bynk(
        &[
            "compile",
            "--target",
            "workers",
            "--output",
            out_dir.to_str().unwrap(),
        ],
        &dir,
    );
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(ok, "the undeclared file must not be read, got:\n{out}");
}

/// Review of #1905: two declarations of one file, in different units (one
/// spelled `./assets/…`), read it once; when that read fails, each
/// declaration is reported, the first with the I/O error and without a second
/// read for the other.
#[test]
fn a_file_declared_twice_is_read_once() {
    let dir = project("asset-build-declared-twice", true);
    std::fs::write(
        dir.join("src/compat/extra.bynk"),
        "commons compat.extra\n\nasset again = \"./assets/board.css\"\n",
    )
    .unwrap();
    let options = |refuse: &'static str| {
        let (folder, reads) = recording_folder(&dir, refuse);
        let paths = bynk_driver::project::ProjectPaths::conventional(&dir);
        let options = bynk_testkit::compile_options_split(dir.clone(), paths)
            .target(bynk_driver::project::BuildTarget::Workers)
            .asset_files(folder);
        (options, reads)
    };
    let board_reads = |reads: &std::sync::Mutex<Vec<String>>| {
        reads
            .lock()
            .unwrap()
            .iter()
            .filter(|k| *k == "assets/board.css")
            .count()
    };

    let (opts, reads) = options("assets/huge.bin");
    if let Err(f) = bynk_driver::project::compile_project(&opts) {
        panic!(
            "compile failed: {:?}",
            f.errors
                .iter()
                .map(|e| e.error.category)
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(board_reads(&reads), 1, "read once for two declarations");

    let (opts, reads) = options("assets/board.css");
    let Err(failure) = bynk_driver::project::compile_project(&opts) else {
        panic!("an unreadable declared file must fail the build");
    };
    assert_eq!(board_reads(&reads), 1, "a failed read is not retried");
    let unreadable: Vec<_> = failure
        .errors
        .iter()
        .filter(|e| e.error.category == "bynk.asset.unreadable_file")
        .collect();
    assert_eq!(unreadable.len(), 2, "one per declaration");
    let with_io_note = unreadable
        .iter()
        .filter(|e| e.error.notes.iter().any(|n| n.contains("the read failed")))
        .count();
    assert_eq!(with_io_note, 1, "the I/O error is noted once");
}
