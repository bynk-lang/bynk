//! Packaging slice 2b (#2240, track `design/tracks/packaging.md` §5.2, §6.4):
//! `bynk.lock` and the store.
//!
//! The lock pins a project's (or a workspace's) resolved package graph: each
//! git dependency's requested revision, the commit it resolved to and the
//! sha256 of its canonical tree, and each path dependency's directory (never
//! hashed — it is the author's working copy). Only the `bynk` CLI writes it
//! and fills the store; the compile, `bynkc` and the editor read both and
//! never fetch ([`crate::resolve_graph`]).
//!
//! The format is versioned and written in one deterministic order, and a file
//! with no `version` is refused rather than read as empty, as
//! `bynk.deploy.lock` refuses one: a truncated lock must not silently
//! re-resolve every dependency.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::discovery::read_source;

/// The lock's file name, beside `bynk.toml` (or the workspace root's).
pub const LOCK_FILE: &str = "bynk.lock";

/// The lock format this toolchain reads and writes.
pub const LOCK_VERSION: i64 = 1;

/// One locked package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockedPackage {
    /// `org/package`.
    pub name: String,
    /// Its `[package] version`, when it declares one.
    pub version: Option<String>,
    /// Where it came from: [`git_source`] or `path+<dir>`.
    pub source: String,
    /// `sha256-<base64>` of its canonical tree; `None` for a path package.
    pub checksum: Option<String>,
    /// The packages (`org/package`) it depends on, sorted.
    pub dependencies: Vec<String>,
}

/// A parsed `bynk.lock`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lock {
    pub packages: Vec<LockedPackage>,
}

impl Lock {
    /// The locked git package for `name` requested as `url` at `rev`.
    pub fn git(&self, name: &str, url: &str, rev: &str) -> Option<(&LockedPackage, String)> {
        self.packages.iter().find_map(|p| {
            let (u, r, commit) = parse_git_source(&p.source)?;
            (p.name == name && u == url && r == rev).then(|| (p, commit))
        })
    }

    /// The lock as `bynk.lock` text: packages sorted by name then source,
    /// each key in a fixed order.
    pub fn render(&self) -> String {
        let mut packages = self.packages.clone();
        packages.sort();
        let mut out = String::from(
            "# bynk.lock — written by `bynk`; commit it. Do not edit by hand.\n",
        );
        out.push_str(&format!("version = {LOCK_VERSION}\n"));
        for p in &packages {
            out.push_str("\n[[package]]\n");
            out.push_str(&format!("name = {}\n", toml_string(&p.name)));
            if let Some(v) = &p.version {
                out.push_str(&format!("version = {}\n", toml_string(v)));
            }
            out.push_str(&format!("source = {}\n", toml_string(&p.source)));
            if let Some(c) = &p.checksum {
                out.push_str(&format!("checksum = {}\n", toml_string(c)));
            }
            let deps: Vec<String> = p.dependencies.iter().map(|d| toml_string(d)).collect();
            out.push_str(&format!("dependencies = [{}]\n", deps.join(", ")));
        }
        out
    }
}

/// Why a `bynk.lock` couldn't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockError(pub String);

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Parse `bynk.lock`'s text.
pub fn parse_lock(text: &str) -> Result<Lock, LockError> {
    let doc: toml::Table = text
        .parse()
        .map_err(|_| LockError("`bynk.lock` is not valid TOML".to_string()))?;
    match doc.get("version").and_then(|v| v.as_integer()) {
        Some(LOCK_VERSION) => {}
        Some(v) => {
            return Err(LockError(format!(
                "`bynk.lock` is version {v}, which this toolchain doesn't read (it reads version {LOCK_VERSION})"
            )));
        }
        None => {
            return Err(LockError(
                "`bynk.lock` has no `version` — it may be truncated; delete it and run `bynk fetch`"
                    .to_string(),
            ));
        }
    }
    let mut packages = Vec::new();
    let entries = match doc.get("package") {
        None => Vec::new(),
        Some(toml::Value::Array(items)) => items.clone(),
        Some(_) => return Err(LockError("`bynk.lock`'s `package` must be an array of tables".into())),
    };
    for entry in entries {
        let Some(t) = entry.as_table() else {
            return Err(LockError("a `[[package]]` in `bynk.lock` isn't a table".into()));
        };
        let text = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_owned);
        let (Some(name), Some(source)) = (text("name"), text("source")) else {
            return Err(LockError(
                "a `[[package]]` in `bynk.lock` lacks `name` or `source`".into(),
            ));
        };
        let dependencies = t
            .get("dependencies")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|d| d.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        packages.push(LockedPackage {
            name,
            version: text("version"),
            source,
            checksum: text("checksum"),
            dependencies,
        });
    }
    Ok(Lock { packages })
}

/// Where the lock for the project at `project_root` lives: its workspace's
/// root when it is a member (one lock per workspace, track §4.2), else its own
/// root.
pub fn lock_root(project_root: &Path) -> PathBuf {
    crate::workspace::find_parent_workspace(project_root).unwrap_or_else(|| project_root.to_path_buf())
}

/// Read the lock at `lock_root`: the overlay first, then the disk. `Ok(None)`
/// when there is none.
pub fn read_lock(
    lock_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<Option<Lock>, LockError> {
    let path = lock_root.join(LOCK_FILE);
    let text = match read_source(&path, overlay) {
        Ok(text) => text,
        Err(_) => match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => return Ok(None),
        },
    };
    parse_lock(&text).map(Some)
}

/// A git package's `source`: `git+<url>?rev=<rev>#<commit>`.
pub fn git_source(url: &str, rev: &str, commit: &str) -> String {
    format!("git+{url}?rev={rev}#{commit}")
}

/// `(url, rev, commit)` of a git `source`, or `None` for any other source.
pub fn parse_git_source(source: &str) -> Option<(String, String, String)> {
    let rest = source.strip_prefix("git+")?;
    let (rest, commit) = rest.rsplit_once('#')?;
    let (url, rev) = rest.rsplit_once("?rev=")?;
    Some((url.to_owned(), rev.to_owned(), commit.to_owned()))
}

/// A path package's `source`: `path+<dir>`, `/`-separated.
pub fn path_source(dir: &Path) -> String {
    let parts: Vec<String> = dir
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    format!("path+{}", parts.join("/"))
}

/// The store's root: `$BYNK_HOME`, else `$XDG_CACHE_HOME/bynk`, else
/// `~/.cache/bynk`; `None` when none can be determined.
pub fn bynk_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("BYNK_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(home));
    }
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(cache).join("bynk"));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".cache").join("bynk"))
}

/// The store directory holding the tree whose checksum is `checksum`
/// (`sha256-<base64>`): `<home>/store/sha256/<hex>`.
pub fn store_dir(home: &Path, checksum: &str) -> Option<PathBuf> {
    let hex = checksum_hex(checksum)?;
    Some(home.join("store").join("sha256").join(hex))
}

/// `sha256-<base64>` for a 32-byte digest.
pub fn checksum_of(digest: &[u8; 32]) -> String {
    format!("sha256-{}", base64_encode(digest))
}

/// The lowercase hex of a `sha256-<base64>` checksum's digest.
pub fn checksum_hex(checksum: &str) -> Option<String> {
    let bytes = base64_decode(checksum.strip_prefix("sha256-")?)?;
    if bytes.len() != 32 {
        return None;
    }
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in text.bytes().filter(|&c| c != b'=') {
        let v = B64.iter().position(|&b| b == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// `s` as a TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_round_trip_through_base64() {
        let digest: [u8; 32] = std::array::from_fn(|i| (i * 7) as u8);
        let c = checksum_of(&digest);
        assert!(c.starts_with("sha256-") && c.ends_with('='));
        let hex = checksum_hex(&c).unwrap();
        assert_eq!(hex.len(), 64);
        assert_eq!(&hex[..6], "00070e");
        assert_eq!(checksum_hex("sha256-short"), None);
    }

    #[test]
    fn the_lock_round_trips_in_a_fixed_order() {
        let lock = Lock {
            packages: vec![
                LockedPackage {
                    name: "acme/money".into(),
                    version: None,
                    source: "path+../packages/money".into(),
                    checksum: None,
                    dependencies: vec![],
                },
                LockedPackage {
                    name: "acme/ledger".into(),
                    version: Some("0.3.0".into()),
                    source: git_source("https://x/ledger", "v0.3.0", "9f1c"),
                    checksum: Some(checksum_of(&[1; 32])),
                    dependencies: vec!["acme/money".into()],
                },
            ],
        };
        let text = lock.render();
        assert!(text.find("acme/ledger").unwrap() < text.find("acme/money").unwrap());
        let back = parse_lock(&text).unwrap();
        assert_eq!(back.render(), text);
        let (p, commit) = back.git("acme/ledger", "https://x/ledger", "v0.3.0").unwrap();
        assert_eq!(commit, "9f1c");
        assert_eq!(p.version.as_deref(), Some("0.3.0"));
        assert!(back.git("acme/ledger", "https://x/ledger", "main").is_none());
    }

    #[test]
    fn a_lock_without_a_version_is_refused() {
        assert!(parse_lock("[[package]]\nname = \"a/b\"\nsource = \"path+x\"\n")
            .unwrap_err()
            .0
            .contains("no `version`"));
        assert!(parse_lock("version = 2\n").unwrap_err().0.contains("version 2"));
        assert_eq!(parse_lock("version = 1\n").unwrap(), Lock::default());
    }
}
