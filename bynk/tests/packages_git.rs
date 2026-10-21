//! Git dependencies, the store and `bynk.lock` (packaging slice 2b, #2240),
//! over the real binaries and local git repositories — no test touches the
//! network. Each test has its own `BYNK_HOME`, so stores never mix.
//!
//! Pinned here:
//!  - a git dependency is fetched, locked (commit + tree checksum) and
//!    compiled, and a suite runs against its code;
//!  - a moved branch keeps the locked commit until `bynk update`;
//!  - `--locked` refuses a lock change; `--offline` refuses a fetch; `bynkc`
//!    reports `not_fetched` for a cold store and never fetches;
//!  - a tampered store entry fails with `checksum_mismatch`;
//!  - unsafe sources (`ext::`, option-shaped) and a fetched package's path or
//!    local-git dependencies are refused;
//!  - a workspace has one lock, and two members reaching one package at two
//!    revisions conflict;
//!  - `[dev-dependencies]` reach only suites, and never a deployable build.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bin(name: &str) -> PathBuf {
    let bynk = PathBuf::from(env!("CARGO_BIN_EXE_bynk"));
    if name == "bynk" {
        bynk
    } else {
        bynk.parent().unwrap().join(name)
    }
}

fn have_bynkc() -> bool {
    bin("bynkc").exists()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("packages_git")
        .join(name);
    if dir.exists() {
        make_writable(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_writable(dir: &Path) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.is_dir() {
            make_writable(&p);
        } else if let Ok(meta) = std::fs::metadata(&p) {
            let mut perms = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(&p, perms);
        }
    }
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A git repository at `dir` holding the package `acme/ledger`, tagged
/// `v0.3.0`, on branch `main`. Returns its `file://` URL.
fn ledger_repo(dir: &Path, body: &str) -> String {
    write(
        &dir.join("bynk.toml"),
        "[package]\nname = \"acme/ledger\"\nversion = \"0.3.0\"\n",
    );
    write(&dir.join("src/entries.bynk"), body);
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "init"]);
    git(dir, &["tag", "v0.3.0"]);
    format!("file://{}", dir.display())
}

const POST_PLUS_ONE: &str = "commons entries\n\nfn post(n: Int) -> Int {\n  n + 1\n}\n";

/// Run `bin <args>` in `cwd` with `BYNK_HOME = home`.
fn run(bin_name: &str, cwd: &Path, home: &Path, args: &[&str]) -> (i32, String) {
    let mut cmd = Command::new(bin(bin_name));
    cmd.args(args)
        .current_dir(cwd)
        .env("BYNK_HOME", home)
        .env_remove("BYNK_BYNKC")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if bin_name == "bynk" && args.first() == Some(&"test") && have_bynkc() {
        cmd.env("BYNK_BYNKC", bin("bynkc"));
    }
    let out = cmd.output().expect("spawn");
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// A consumer at `dir` depending on `url` at `rev` as `ledger`, with a
/// commons using it.
fn consumer(dir: &Path, url: &str, rev: &str) {
    write(
        &dir.join("bynk.toml"),
        &format!(
            "[project]\nname = \"app\"\n\n[dependencies]\nledger = {{ package = \"acme/ledger\", git = \"{url}\", rev = \"{rev}\" }}\n"
        ),
    );
    write(
        &dir.join("src/books.bynk"),
        "commons books\n\nuses ledger.entries\n\nfn total(n: Int) -> Int {\n  post(n)\n}\n",
    );
}

fn lock(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("bynk.lock")).unwrap_or_default()
}

#[test]
fn a_git_dependency_is_fetched_locked_and_run() {
    let root = scratch("fetch");
    let url = ledger_repo(&root.join("ledger"), POST_PLUS_ONE);
    let app = root.join("app");
    consumer(&app, &url, "v0.3.0");
    write(
        &app.join("tests/books_test.bynk"),
        "suite books\n\ncase \"posts through the git dependency\" {\n  expect total(3) == 4\n}\n",
    );
    let home = root.join("home");
    let (code, out) = run("bynk", &app, &home, &["check"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("bynk: fetched acme/ledger@"), "{out}");
    let text = lock(&app);
    let commit = git(&root.join("ledger"), &["rev-parse", "HEAD"]);
    assert!(text.contains("name = \"acme/ledger\""), "{text}");
    assert!(text.contains("version = \"0.3.0\""), "{text}");
    assert!(text.contains(&format!("?rev=v0.3.0#{commit}\"")), "{text}");
    assert!(text.contains("checksum = \"sha256-"), "{text}");
    assert!(home.join("store/sha256").read_dir().unwrap().count() == 1);

    // Nothing to fetch the second time, and the lock is unchanged.
    let (code, out) = run("bynk", &app, &home, &["check", "--locked", "--offline"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("fetched"), "{out}");

    if have_bynkc() {
        let (code, out) = run("bynk", &app, &home, &["test"]);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("posts through the git dependency"), "{out}");
    }
}

#[test]
fn a_moved_branch_waits_for_bynk_update() {
    let root = scratch("branch");
    let repo = root.join("ledger");
    let url = ledger_repo(&repo, POST_PLUS_ONE);
    let first = git(&repo, &["rev-parse", "HEAD"]);
    let app = root.join("app");
    consumer(&app, &url, "main");
    let home = root.join("home");
    assert_eq!(run("bynk", &app, &home, &["fetch"]).0, 0);
    assert!(lock(&app).contains(&first));

    write(
        &repo.join("src/entries.bynk"),
        "commons entries\n\nfn post(n: Int) -> Int {\n  n + 2\n}\n",
    );
    git(&repo, &["commit", "-qam", "moved"]);
    let second = git(&repo, &["rev-parse", "HEAD"]);

    let (code, out) = run("bynk", &app, &home, &["check"]);
    assert_eq!(code, 0, "{out}");
    assert!(lock(&app).contains(&first), "the locked commit holds");

    let (code, out) = run("bynk", &app, &home, &["update", "ledger"]);
    assert_eq!(code, 0, "{out}");
    let text = lock(&app);
    assert!(text.contains(&second) && !text.contains(&first), "{text}");
}

#[test]
fn locked_and_offline_refuse_rather_than_repair() {
    let root = scratch("modes");
    let url = ledger_repo(&root.join("ledger"), POST_PLUS_ONE);
    let app = root.join("app");
    consumer(&app, &url, "v0.3.0");
    let home = root.join("home");

    // No lock yet: `--locked` refuses to write one; `--offline` refuses to fetch.
    let (code, out) = run("bynk", &app, &home, &["check", "--locked"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("error[bynk.package.lock_out_of_date]"),
        "{out}"
    );
    let (code, out) = run("bynk", &app, &home, &["check", "--offline"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[bynk.package.not_fetched]"), "{out}");
    assert!(!app.join("bynk.lock").exists());

    // `bynkc` never fetches: a cold store is one `not_fetched` diagnostic.
    if have_bynkc() {
        let (code, out) = run("bynkc", &app, &home, &["check", "--format", "short", "."]);
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("bynk.package.not_fetched"), "{out}");
        assert!(!out.contains("unknown_commons"), "{out}");
    }

    assert_eq!(run("bynk", &app, &home, &["fetch"]).0, 0);
    // A cold store with a lock: `--offline` still can't fetch it.
    let cold = root.join("cold-home");
    let (code, out) = run("bynk", &app, &cold, &["check", "--offline"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("isn't in the store, and `--offline` was given"),
        "{out}"
    );

    // Changing `rev` re-resolves — and `--locked` refuses that change.
    consumer(&app, &url, "main");
    let (code, out) = run("bynk", &app, &home, &["check", "--locked"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("isn't locked, and `--locked` was given"),
        "{out}"
    );
    let (code, out) = run("bynk", &app, &home, &["check"]);
    assert_eq!(code, 0, "{out}");
    assert!(lock(&app).contains("?rev=main#"), "{}", lock(&app));
}

#[test]
fn a_tampered_store_entry_fails() {
    let root = scratch("tamper");
    let url = ledger_repo(&root.join("ledger"), POST_PLUS_ONE);
    let app = root.join("app");
    consumer(&app, &url, "v0.3.0");
    let home = root.join("home");
    assert_eq!(run("bynk", &app, &home, &["fetch"]).0, 0);
    let entry = home
        .join("store/sha256")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    make_writable(&entry);
    write(
        &entry.join("src/entries.bynk"),
        "commons entries\n\nfn post(n: Int) -> Int {\n  n * 100\n}\n",
    );
    let (code, out) = run("bynk", &app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("error[bynk.package.checksum_mismatch]"),
        "{out}"
    );
    assert!(out.contains("delete it and run `bynk fetch`"), "{out}");
}

#[test]
fn unsafe_sources_and_a_fetched_packages_local_reach_are_refused() {
    let root = scratch("unsafe");
    let app = root.join("app");
    let home = root.join("home");
    consumer(&app, "ext::sh -c touch% pwned", "v1");
    let (code, out) = run("bynk", &app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[bynk.package.unsafe_source]"), "{out}");
    assert!(!app.join("pwned").exists());

    consumer(
        &app,
        "https://example.invalid/x",
        "--upload-pack=touch pwned",
    );
    let (code, out) = run("bynk", &app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("looks like a command-line option"), "{out}");

    // A git package naming a path dependency of its own.
    let repo = root.join("ledger");
    write(
        &repo.join("bynk.toml"),
        "[package]\nname = \"acme/ledger\"\n\n[dependencies]\nhome = { package = \"acme/home\", path = \"../../..\" }\n",
    );
    write(&repo.join("src/entries.bynk"), POST_PLUS_ONE);
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "init"]);
    consumer(&app, &format!("file://{}", repo.display()), "main");
    let (code, out) = run("bynk", &app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("error[bynk.dependency.path_outside_root]"),
        "{out}"
    );
    assert!(
        out.contains("a fetched package's path dependencies are never honoured"),
        "{out}"
    );
}

#[test]
fn a_workspace_has_one_lock_and_one_version_per_package() {
    let root = scratch("workspace");
    let repo = root.join("ledger");
    let url = ledger_repo(&repo, POST_PLUS_ONE);
    write(
        &root.join("ws/bynk.toml"),
        "[workspace]\nmembers = [\"apps/*\"]\n",
    );
    let a = root.join("ws/apps/a");
    let b = root.join("ws/apps/b");
    consumer(&a, &url, "v0.3.0");
    consumer(&b, &url, "v0.3.0");
    let home = root.join("home");
    let (code, out) = run("bynk", &a, &home, &["check"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        root.join("ws/bynk.lock").exists(),
        "the lock lives at the workspace root"
    );
    assert!(!a.join("bynk.lock").exists() && !b.join("bynk.lock").exists());

    // Member b moves to another revision of the same package.
    write(
        &repo.join("src/entries.bynk"),
        "commons entries\n\nfn post(n: Int) -> Int {\n  n + 2\n}\n",
    );
    git(&repo, &["commit", "-qam", "moved"]);
    consumer(&b, &url, "main");
    let (code, out) = run("bynk", &a, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("error[bynk.package.version_conflict]"),
        "{out}"
    );
}

#[test]
fn dev_dependencies_reach_only_suites_and_never_a_deployable_build() {
    let root = scratch("dev");
    let fixtures = root.join("fixtures");
    write(
        &fixtures.join("bynk.toml"),
        "[package]\nname = \"acme/fixtures\"\n",
    );
    write(
        &fixtures.join("src/sample.bynk"),
        "commons sample\n\nfn three() -> Int {\n  3\n}\n",
    );
    let app = root.join("app");
    write(
        &app.join("bynk.toml"),
        "[project]\nname = \"app\"\n\n[dev-dependencies]\nfx = { package = \"acme/fixtures\", path = \"../fixtures\" }\n",
    );
    write(
        &app.join("src/books.bynk"),
        "commons books\n\nfn twice(n: Int) -> Int {\n  n + n\n}\n",
    );
    // A suite may target a dev-dependency's unit through its alias.
    write(
        &app.join("tests/sample_test.bynk"),
        "suite fx.sample\n\ncase \"three is three\" {\n  expect three() == 3\n}\n",
    );
    let home = root.join("home");
    let (code, out) = run("bynk", &app, &home, &["check"]);
    assert_eq!(code, 0, "{out}");
    if have_bynkc() {
        let (code, out) = run("bynk", &app, &home, &["test"]);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("three is three"), "{out}");

        // A deployable build carries none of it.
        let with = root.join("out-with");
        let (code, out) = run(
            "bynkc",
            &app,
            &home,
            &["compile", ".", "-o", with.to_str().unwrap()],
        );
        assert_eq!(code, 0, "{out}");
        assert!(
            !with.join("deps").exists(),
            "no dev-dependency unit is emitted"
        );
    }

    // Production source can't reach a dev-only alias.
    write(
        &app.join("src/books.bynk"),
        "commons books\n\nuses fx.sample\n\nfn twice(n: Int) -> Int {\n  n + three()\n}\n",
    );
    let (code, out) = run("bynk", &app, &home, &["check", "--format", "short"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(
        "error[bynk.package.dev_dependency_in_source]: `fx.sample` reaches dependency `acme/fixtures` through `fx`, which is a `[dev-dependencies]` alias: only a suite may use it"
    ), "{out}");
}

#[test]
fn a_missing_revision_and_a_truncated_lock_are_reported() {
    let root = scratch("broken");
    let url = ledger_repo(&root.join("ledger"), POST_PLUS_ONE);
    let app = root.join("app");
    let home = root.join("home");
    consumer(&app, &url, "v9.9.9");
    let (code, out) = run("bynk", &app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[bynk.package.git_failed]"), "{out}");
    assert!(
        out.contains("has no tag, branch or commit `v9.9.9`"),
        "{out}"
    );

    consumer(&app, &url, "v0.3.0");
    write(
        &app.join("bynk.lock"),
        "[[package]]\nname = \"acme/ledger\"\nsource = \"path+x\"\n",
    );
    let (code, out) = run("bynk", &app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[bynk.package.lock_invalid]"), "{out}");
    assert!(
        out.contains("has no `version` — it may be truncated"),
        "{out}"
    );
}

/// A hook planted in the git cache never runs: `git fetch` would run a
/// `reference-transaction` hook, and every invocation sets
/// `core.hooksPath=/dev/null`.
#[cfg(unix)]
#[test]
fn hooks_in_the_git_cache_never_run() {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch("hooks");
    let url = ledger_repo(&root.join("ledger"), POST_PLUS_ONE);
    let app = root.join("app");
    consumer(&app, &url, "main");
    let home = root.join("home");
    assert_eq!(run("bynk", &app, &home, &["fetch"]).0, 0);
    let cache = home
        .join("git")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let marker = root.join("hook-ran");
    for hook in ["reference-transaction", "post-checkout", "post-merge"] {
        let path = cache.join("hooks").join(hook);
        write(&path, &format!("#!/bin/sh\ntouch '{}'\n", marker.display()));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (code, out) = run("bynk", &app, &home, &["update"]);
    assert_eq!(code, 0, "{out}");
    assert!(!marker.exists(), "a hook ran");
}
