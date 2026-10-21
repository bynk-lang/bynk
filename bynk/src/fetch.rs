//! Packaging slice 2b (#2240): fetching git dependencies and writing
//! `bynk.lock` — the one place the toolchain touches the network (track
//! `design/tracks/packaging.md` §6.4, decision A).
//!
//! Before `check`, `test`, `dev` or `deploy` compiles, [`prepare`] resolves
//! the project's (or its workspace's) dependencies against `bynk.lock`:
//!
//! - a locked git dependency whose manifest entry is unchanged keeps its
//!   locked commit; one missing from the store is fetched at that commit;
//! - a new or changed one (or every one named by `bynk update`) is resolved
//!   from its `rev`, fetched, and hashed;
//! - every git package's store entry is re-hashed against the lock, so a
//!   tampered or corrupted entry fails the build (`checksum_mismatch`);
//! - the lock is rewritten when the graph changed — or, under `--locked`,
//!   the command fails instead (`lock_out_of_date`). Under `--offline`, any
//!   needed fetch fails.
//!
//! Git runs as the user's own `git`, hardened per track §6.3 (decision B):
//! only `https` and `ssh` transports (plus `file` from the project's own
//! manifests), no hooks, no submodules, `--` before positional arguments,
//! and no URL or revision that looks like an option. Bynk passes no
//! credentials: a private repository works through the user's git setup.
//!
//! The compile never fetches: `bynkc`, the language server and the
//! playground read the lock and the store ([`bynk_emit::project`]'s graph
//! resolution reports `not_fetched` for a missing entry).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use bynk_emit::project::{is_workspace_root, load_workspace};
use sha2::{Digest, Sha256};

use bynk_project::lock::{
    Lock, LockedPackage, bynk_home, checksum_hex, checksum_of, git_source, lock_root, parse_lock,
    path_source, store_dir, LOCK_FILE,
};
use bynk_project::manifest::{DependencySource, DependencySpec, dependencies_from, package_config_from};

/// How a `bynk` command may change the lock and the store.
#[derive(Debug, Clone, Default)]
pub struct FetchOptions {
    /// Fail instead of changing `bynk.lock`.
    pub locked: bool,
    /// Fail instead of fetching.
    pub offline: bool,
    /// `bynk update`: re-resolve these packages' revisions (`Some(empty)`
    /// means every git dependency).
    pub update: Option<Vec<String>>,
}

/// A failure, as one diagnostic line: its code and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error[{}]: {}", self.code, self.message)
    }
}

const NOT_FETCHED: &str = "bynk.package.not_fetched";
const LOCK_OUT_OF_DATE: &str = "bynk.package.lock_out_of_date";
const CHECKSUM_MISMATCH: &str = "bynk.package.checksum_mismatch";
const VERSION_CONFLICT: &str = "bynk.package.version_conflict";
const LOCK_INVALID: &str = "bynk.package.lock_invalid";
const GIT_FAILED: &str = "bynk.package.git_failed";
const UNSAFE_SOURCE: &str = "bynk.package.unsafe_source";
const PATH_OUTSIDE_ROOT: &str = "bynk.dependency.path_outside_root";

fn fail(code: &'static str, message: impl Into<String>) -> FetchError {
    FetchError {
        code,
        message: message.into(),
    }
}

/// What [`prepare`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prepared {
    /// Whether `bynk.lock` was written.
    pub wrote_lock: bool,
    /// The git packages fetched into the store, as `org/package@commit`.
    pub fetched: Vec<String>,
}

/// Resolve, fetch and lock the dependencies of the project at `project_dir`
/// (and of every member of its workspace, which shares one lock). A project
/// with no dependencies and no lock is untouched.
pub fn prepare(project_dir: &Path, opts: &FetchOptions) -> Result<Prepared, FetchError> {
    let root = lock_root(project_dir);
    let members: Vec<PathBuf> = if is_workspace_root(&root) {
        load_workspace(&root)
            .map(|w| w.members)
            .map_err(|e| fail(LOCK_INVALID, format!("the workspace at `{}`: {e}", root.display())))?
    } else {
        vec![project_dir.to_path_buf()]
    };
    let lock_path = root.join(LOCK_FILE);
    let existing = match std::fs::read_to_string(&lock_path) {
        Ok(text) => Some(parse_lock(&text).map_err(|e| fail(LOCK_INVALID, e.0))?),
        Err(_) => None,
    };
    let mut walk = Walk {
        root: root.clone(),
        opts,
        existing: existing.clone().unwrap_or_default(),
        home: None,
        resolved: BTreeMap::new(),
        prepared: Prepared::default(),
    };
    let mut any = false;
    for member in &members {
        let Some(doc) = read_manifest(member) else {
            continue;
        };
        let mut declared = specs(&doc, "dependencies");
        declared.extend(specs(&doc, "dev-dependencies"));
        any |= !declared.is_empty();
        for spec in declared {
            walk.resolve(member, Declarer::Own, &spec)?;
        }
    }
    if !any && existing.is_none() {
        return Ok(walk.prepared);
    }
    let lock = Lock {
        packages: walk.resolved.values().cloned().collect(),
    };
    let rendered = lock.render();
    let current = existing.as_ref().map(Lock::render);
    if current.as_deref() != Some(rendered.as_str()) {
        if opts.locked {
            return Err(fail(
                LOCK_OUT_OF_DATE,
                format!(
                    "`{}` is out of date with the manifests, and `--locked` was given",
                    lock_path.display()
                ),
            ));
        }
        write_atomically(&lock_path, &rendered)
            .map_err(|e| fail(LOCK_INVALID, format!("could not write `{}`: {e}", lock_path.display())))?;
        walk.prepared.wrote_lock = true;
    }
    Ok(walk.prepared)
}

/// Who declared a dependency: the project (or a workspace member, or a path
/// package — the author's own files), or a fetched package.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Declarer {
    Own,
    Fetched,
}

struct Walk<'o> {
    root: PathBuf,
    opts: &'o FetchOptions,
    existing: Lock,
    home: Option<PathBuf>,
    /// Resolved packages by name.
    resolved: BTreeMap<String, LockedPackage>,
    prepared: Prepared,
}

impl Walk<'_> {
    fn home(&mut self) -> Result<PathBuf, FetchError> {
        if let Some(h) = &self.home {
            return Ok(h.clone());
        }
        let h = bynk_home().ok_or_else(|| {
            fail(
                NOT_FETCHED,
                "can't find a cache directory for the store: set `BYNK_HOME`",
            )
        })?;
        self.home = Some(h.clone());
        Ok(h)
    }

    /// Resolve one dependency of a package in `dir`, then its own.
    fn resolve(
        &mut self,
        dir: &Path,
        declarer: Declarer,
        spec: &DependencySpec,
    ) -> Result<(), FetchError> {
        match &spec.source {
            DependencySource::Version(_) => Ok(()), // slice 4; the graph reports it
            DependencySource::Path(p) => {
                if declarer == Declarer::Fetched {
                    return Err(fail(
                        PATH_OUTSIDE_ROOT,
                        format!(
                            "the fetched package declaring `{}` names the path dependency `{p}`, and a fetched package's path dependencies are never honoured",
                            spec.alias
                        ),
                    ));
                }
                let target = dir.join(p);
                let Some(doc) = read_manifest(&target) else {
                    return Ok(()); // the graph reports a missing directory
                };
                let source = path_source(&relative(&self.root, &target));
                let version = package_version(&doc);
                let deps = specs(&doc, "dependencies");
                if !self.record(spec, source, None, version, &deps)? {
                    return Ok(());
                }
                for d in deps {
                    self.resolve(&target, Declarer::Own, &d)?;
                }
                Ok(())
            }
            DependencySource::Git { url, rev } => {
                check_source(url, rev, declarer)?;
                let allow_file = declarer == Declarer::Own;
                let refresh = self
                    .opts
                    .update
                    .as_ref()
                    .is_some_and(|names| names.is_empty() || names.iter().any(|n| *n == spec.alias || *n == spec.package));
                let locked = (!refresh)
                    .then(|| self.existing.git(&spec.package, url, rev))
                    .flatten()
                    .map(|(p, commit)| (p.checksum.clone(), commit));
                if let Some((_, commit)) = &locked
                    && !(commit.len() >= 40 && commit.chars().all(|c| c.is_ascii_hexdigit()))
                {
                    return Err(fail(
                        LOCK_INVALID,
                        format!("`bynk.lock` locks `{}` to `{commit}`, which isn't a commit id", spec.package),
                    ));
                }
                let home = self.home()?;
                let (commit, checksum) = match locked {
                    Some((Some(checksum), commit)) => {
                        let entry = store_dir(&home, &checksum)
                            .ok_or_else(|| fail(LOCK_INVALID, format!("`bynk.lock`'s checksum for `{}` is malformed", spec.package)))?;
                        if !entry.is_dir() {
                            if self.opts.offline {
                                return Err(fail(
                                    NOT_FETCHED,
                                    format!(
                                        "`{}` ({url} at {commit}) isn't in the store, and `--offline` was given",
                                        spec.package
                                    ),
                                ));
                            }
                            let got = fetch_into_store(&home, url, &commit, allow_file)?;
                            if got != checksum {
                                return Err(fail(
                                    CHECKSUM_MISMATCH,
                                    format!(
                                        "`{}` at {commit} hashes to {got}, but `bynk.lock` says {checksum}",
                                        spec.package
                                    ),
                                ));
                            }
                            self.prepared.fetched.push(format!("{}@{commit}", spec.package));
                        }
                        (commit, checksum)
                    }
                    _ => {
                        if self.opts.locked {
                            return Err(fail(
                                LOCK_OUT_OF_DATE,
                                format!(
                                    "`{}` ({url} at {rev}) isn't locked, and `--locked` was given",
                                    spec.package
                                ),
                            ));
                        }
                        if self.opts.offline {
                            return Err(fail(
                                NOT_FETCHED,
                                format!(
                                    "`{}` ({url} at {rev}) needs fetching, and `--offline` was given",
                                    spec.package
                                ),
                            ));
                        }
                        let commit = resolve_rev(&home, url, rev, allow_file)?;
                        let checksum = fetch_into_store(&home, url, &commit, allow_file)?;
                        self.prepared.fetched.push(format!("{}@{commit}", spec.package));
                        (commit, checksum)
                    }
                };
                let entry = store_dir(&home, &checksum).expect("a checksum this module wrote");
                verify(&entry, &checksum, &spec.package)?;
                let Some(doc) = read_manifest(&entry) else {
                    return Err(fail(
                        NOT_FETCHED,
                        format!("`{url}` at {commit} has no `bynk.toml`"),
                    ));
                };
                let version = package_version(&doc);
                let deps = specs(&doc, "dependencies");
                let source = git_source(url, rev, &commit);
                if !self.record(spec, source, Some(checksum), version, &deps)? {
                    return Ok(());
                }
                for d in deps {
                    self.resolve(&entry, Declarer::Fetched, &d)?;
                }
                Ok(())
            }
        }
    }

    /// Record a resolved package. `Ok(false)` when it was already resolved
    /// identically (its dependencies are walked once); a different source for
    /// the same package is a version conflict.
    fn record(
        &mut self,
        spec: &DependencySpec,
        source: String,
        checksum: Option<String>,
        version: Option<String>,
        deps: &[DependencySpec],
    ) -> Result<bool, FetchError> {
        if let Some(seen) = self.resolved.get(&spec.package) {
            if seen.source == source {
                return Ok(false);
            }
            return Err(fail(
                VERSION_CONFLICT,
                format!(
                    "the package `{}` is reached from two sources: `{}` and `{source}`",
                    spec.package, seen.source
                ),
            ));
        }
        let mut dependencies: Vec<String> = deps
            .iter()
            .filter(|d| !matches!(d.source, DependencySource::Version(_)))
            .map(|d| d.package.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        dependencies.sort();
        self.resolved.insert(
            spec.package.clone(),
            LockedPackage {
                name: spec.package.clone(),
                version,
                source,
                checksum,
                dependencies,
            },
        );
        Ok(true)
    }
}

/// Refuse a git source that could run a command or reach outside the
/// project (track §6.3): an option-shaped URL or revision, a transport that
/// isn't `https`/`ssh` (or `file`, from the author's own manifests).
fn check_source(url: &str, rev: &str, declarer: Declarer) -> Result<(), FetchError> {
    if url.starts_with('-') || rev.starts_with('-') {
        return Err(fail(
            UNSAFE_SOURCE,
            format!("the git source `{url}` at `{rev}` looks like a command-line option"),
        ));
    }
    let lower = url.to_ascii_lowercase();
    let scheme_ok = lower.starts_with("https://")
        || lower.starts_with("ssh://")
        || (lower.contains('@') && lower.contains(':') && !lower.contains("::"));
    let file = lower.starts_with("file://");
    if file && declarer == Declarer::Fetched {
        return Err(fail(
            PATH_OUTSIDE_ROOT,
            format!("a fetched package names the local git repository `{url}`, and a fetched package can't reach one"),
        ));
    }
    if !(scheme_ok || file) {
        return Err(fail(
            UNSAFE_SOURCE,
            format!("the git source `{url}` isn't an `https://`, `ssh://` or `user@host:path` URL"),
        ));
    }
    Ok(())
}

/// `git` with the hardening every invocation carries.
fn git(allow_file: bool) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(["-c", "protocol.allow=never"])
        .args(["-c", "protocol.https.allow=always"])
        .args(["-c", "protocol.ssh.allow=always"])
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(["-c", "core.autocrlf=false"])
        .args(["-c", "core.symlinks=false"])
        .args(["-c", "submodule.recurse=false"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
    if allow_file {
        cmd.args(["-c", "protocol.file.allow=always"]);
    }
    cmd
}

fn run_git(mut cmd: Command, what: &str) -> Result<String, FetchError> {
    let out = cmd.output().map_err(|e| {
        fail(
            GIT_FAILED,
            format!("couldn't run `git` to {what}: {e} — is git installed? (`bynk doctor` checks)"),
        )
    })?;
    if !out.status.success() {
        return Err(fail(
            GIT_FAILED,
            format!(
                "`git` failed to {what}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The bare clone of `url` in the cache, created or brought up to date.
fn bare_clone(home: &Path, url: &str, allow_file: bool) -> Result<PathBuf, FetchError> {
    let digest: [u8; 32] = Sha256::digest(url.as_bytes()).into();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let dir = home.join("git").join(hex);
    if dir.join("HEAD").is_file() {
        let mut cmd = git(allow_file);
        cmd.arg("--git-dir")
            .arg(&dir)
            .args(["fetch", "--quiet", "--tags", "--force", "--"])
            .arg(url)
            .arg("+refs/heads/*:refs/heads/*");
        run_git(cmd, &format!("fetch `{url}`"))?;
        return Ok(dir);
    }
    std::fs::create_dir_all(dir.parent().unwrap())
        .map_err(|e| fail(GIT_FAILED, format!("could not create the git cache: {e}")))?;
    let tmp = dir.with_extension("partial");
    let _ = std::fs::remove_dir_all(&tmp);
    let mut cmd = git(allow_file);
    cmd.args(["clone", "--bare", "--quiet", "--no-recurse-submodules", "--"])
        .arg(url)
        .arg(&tmp);
    run_git(cmd, &format!("clone `{url}`"))?;
    std::fs::rename(&tmp, &dir)
        .map_err(|e| fail(GIT_FAILED, format!("could not place the clone of `{url}`: {e}")))?;
    Ok(dir)
}

/// The commit `rev` names in `url`.
fn resolve_rev(home: &Path, url: &str, rev: &str, allow_file: bool) -> Result<String, FetchError> {
    let dir = bare_clone(home, url, allow_file)?;
    let mut cmd = git(allow_file);
    cmd.arg("--git-dir")
        .arg(&dir)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{rev}^{{commit}}"));
    run_git(cmd, &format!("find `{rev}` in `{url}`"))
        .map_err(|_| fail(GIT_FAILED, format!("`{url}` has no tag, branch or commit `{rev}`")))
}

/// Check `commit` of `url` out into the store, returning its checksum. An
/// entry already there is left alone.
fn fetch_into_store(
    home: &Path,
    url: &str,
    commit: &str,
    allow_file: bool,
) -> Result<String, FetchError> {
    let dir = bare_clone(home, url, allow_file)?;
    let staging = home.join("store").join("staging").join(commit);
    let _ = make_writable(&staging);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|e| fail(GIT_FAILED, format!("could not stage `{url}`: {e}")))?;
    let index = home.join("store").join("staging").join(format!("{commit}.index"));
    let mut read = git(allow_file);
    read.arg("--git-dir")
        .arg(&dir)
        .arg("--work-tree")
        .arg(&staging)
        .env("GIT_INDEX_FILE", &index)
        .args(["read-tree", commit]);
    run_git(read, &format!("read `{url}` at {commit}"))?;
    let mut checkout = git(allow_file);
    checkout
        .arg("--git-dir")
        .arg(&dir)
        .arg("--work-tree")
        .arg(&staging)
        .env("GIT_INDEX_FILE", &index)
        .args(["checkout-index", "--all", "--force"]);
    run_git(checkout, &format!("check out `{url}` at {commit}"))?;
    let _ = std::fs::remove_file(&index);
    let checksum = tree_checksum(&staging)
        .map_err(|e| fail(GIT_FAILED, format!("could not hash `{url}` at {commit}: {e}")))?;
    let entry = store_dir(home, &checksum).expect("a checksum this module wrote");
    if entry.is_dir() {
        let _ = std::fs::remove_dir_all(&staging);
    } else {
        std::fs::create_dir_all(entry.parent().unwrap())
            .map_err(|e| fail(GIT_FAILED, format!("could not create the store: {e}")))?;
        std::fs::rename(&staging, &entry)
            .map_err(|e| fail(GIT_FAILED, format!("could not place `{url}` in the store: {e}")))?;
        let _ = make_read_only(&entry);
    }
    Ok(checksum)
}

/// Re-hash a store entry against its locked checksum.
fn verify(entry: &Path, checksum: &str, package: &str) -> Result<(), FetchError> {
    let got = tree_checksum(entry)
        .map_err(|e| fail(CHECKSUM_MISMATCH, format!("could not read the store entry for `{package}`: {e}")))?;
    if got != checksum {
        return Err(fail(
            CHECKSUM_MISMATCH,
            format!(
                "the store entry for `{package}` (`{}`) hashes to {got}, but `bynk.lock` says {checksum} — delete it and run `bynk fetch`",
                entry.display()
            ),
        ));
    }
    Ok(())
}

/// The canonical-tree checksum (decision C): every regular file under `dir`
/// except `.git`, in byte-wise sorted `/`-separated path order, each
/// contributing `path \0 length \0 bytes`. Symbolic links aren't files and
/// contribute nothing.
pub fn tree_checksum(dir: &Path) -> std::io::Result<String> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect(dir, dir, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for (rel, path) in files {
        let bytes = std::fs::read(&path)?;
        hasher.update(rel.as_bytes());
        hasher.update([0]);
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(&bytes);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    Ok(checksum_of(&digest))
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if entry.file_name() == ".git" {
                continue;
            }
            collect(root, &path, out)?;
        } else if kind.is_file() {
            let rel: Vec<String> = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push((rel.join("/"), path));
        }
    }
    Ok(())
}

fn make_read_only(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            make_read_only(&path)?;
        } else {
            let mut perms = std::fs::metadata(&path)?.permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(&path, perms)?;
        }
    }
    Ok(())
}

#[allow(clippy::permissions_set_readonly_false)]
fn make_writable(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            make_writable(&path)?;
        } else {
            let mut perms = std::fs::metadata(&path)?.permissions();
            perms.set_readonly(false);
            std::fs::set_permissions(&path, perms)?;
        }
    }
    Ok(())
}

fn read_manifest(dir: &Path) -> Option<toml::Table> {
    std::fs::read_to_string(dir.join("bynk.toml"))
        .ok()?
        .parse()
        .ok()
}

fn specs(doc: &toml::Table, table: &'static str) -> Vec<DependencySpec> {
    doc.get(table)
        .and_then(|v| v.as_table())
        .and_then(|t| dependencies_from(table, t).ok())
        .unwrap_or_default()
}

fn package_version(doc: &toml::Table) -> Option<String> {
    doc.get("package")
        .and_then(|v| v.as_table())
        .and_then(|t| package_config_from(t).ok())
        .and_then(|c| c.version)
}

/// `target` relative to `base`, both made absolute first; `target` itself
/// when they share no root.
fn relative(base: &Path, target: &Path) -> PathBuf {
    let abs = |p: &Path| {
        let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        std::path::absolute(&p).unwrap_or(p)
    };
    let (b, t) = (abs(base), abs(target));
    let bc: Vec<_> = b.components().collect();
    let tc: Vec<_> = t.components().collect();
    let common = bc.iter().zip(&tc).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return t;
    }
    let mut out = PathBuf::new();
    for _ in common..bc.len() {
        out.push("..");
    }
    for c in &tc[common..] {
        out.push(c.as_os_str());
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("lock.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// The checksum's store directory name, for messages and tests.
pub fn store_hex(checksum: &str) -> Option<String> {
    checksum_hex(checksum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_shaped_and_exotic_sources_are_refused() {
        for (url, rev) in [
            ("-uhttps://x", "v1"),
            ("https://x", "--upload-pack=evil"),
            ("ext::sh -c touch% /tmp/pwned", "v1"),
            ("fd::17", "v1"),
            ("http://plain.example/x", "v1"),
        ] {
            assert_eq!(
                check_source(url, rev, Declarer::Own).unwrap_err().code,
                UNSAFE_SOURCE,
                "{url} {rev}"
            );
        }
        assert!(check_source("https://github.com/acme/x", "v1", Declarer::Own).is_ok());
        assert!(check_source("git@github.com:acme/x.git", "main", Declarer::Fetched).is_ok());
        assert!(check_source("file:///tmp/repo", "main", Declarer::Own).is_ok());
        assert_eq!(
            check_source("file:///tmp/repo", "main", Declarer::Fetched).unwrap_err().code,
            PATH_OUTSIDE_ROOT
        );
    }

    #[test]
    fn the_tree_checksum_is_pinned() {
        let dir = std::env::temp_dir().join(format!("bynk-tree-hash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("bynk.toml"), "[package]\nname = \"a/b\"\n").unwrap();
        std::fs::write(dir.join("src/x.bynk"), "commons x\n").unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ignored").unwrap();
        // sha256 of "bynk.toml\0" "25\0" "[package]\nname = \"a/b\"\n"
        //         "src/x.bynk\0" "10\0" "commons x\n"
        let mut h = Sha256::new();
        h.update(b"bynk.toml\x0025\x00[package]\nname = \"a/b\"\n");
        h.update(b"src/x.bynk\x0010\x00commons x\n");
        let want: [u8; 32] = h.finalize().into();
        assert_eq!(tree_checksum(&dir).unwrap(), checksum_of(&want));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
