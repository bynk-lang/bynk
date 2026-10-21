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

/// #2274: the first-party registry's name in routing and in a lock source.
pub const FIRST_PARTY_REGISTRY: &str = "bynk";

/// #2274: the first-party registry's index URL and pinned key
/// (`ed25519:<base64>`), once the hosted service (packaging slice 4b) is live.
/// Until then, an organisation no `[registries]` entry lists resolves nowhere
/// (`bynk.registry.unavailable`).
pub const FIRST_PARTY: Option<(&str, &str)> = None;

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

/// #2255: one npm package a dependency's adapters may bring into the build,
/// at one range. Two dependencies requiring one package at two ranges are two
/// rows; the compile refuses the conflict (`bynk.requires.conflicting_range`)
/// if both are reached.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockedNpm {
    pub package: String,
    pub range: String,
    /// The adapters requiring it, as `org/package:adapter`, sorted.
    pub required_by: Vec<String>,
}

/// #2274: a registry a build used, and the key first trusted for it — what
/// makes trust on first use stick across fresh clones (track §6.2).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockedRegistry {
    /// The `[registries.<name>]` it was routed through (`bynk` for the
    /// first-party registry).
    pub name: String,
    /// The `id` its `config.json` declares, which its signed lines bind.
    pub id: String,
    /// `ed25519-fp:<base64>` — the trusted key's fingerprint.
    pub key: String,
}

/// A parsed `bynk.lock`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lock {
    pub packages: Vec<LockedPackage>,
    /// #2274: the registries resolution used, with their trusted keys.
    pub registries: Vec<LockedRegistry>,
    /// #2255: every npm package a dependency's adapters may require — what
    /// *could* enter a build, recorded at resolve time; `out/package.json`
    /// holds what a build's reached adapters actually require.
    pub npm: Vec<LockedNpm>,
}

impl Lock {
    /// The locked git package for `name` requested as `url` at `rev`.
    pub fn git(&self, name: &str, url: &str, rev: &str) -> Option<(&LockedPackage, String)> {
        self.packages.iter().find_map(|p| {
            let (u, r, commit) = parse_git_source(&p.source)?;
            (p.name == name && u == url && r == rev).then_some((p, commit))
        })
    }

    /// The lock as `bynk.lock` text: packages sorted by name then source,
    /// each key in a fixed order.
    pub fn render(&self) -> String {
        let mut packages = self.packages.clone();
        packages.sort();
        let mut out =
            String::from("# bynk.lock — written by `bynk`; commit it. Do not edit by hand.\n");
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
        let mut registries = self.registries.clone();
        registries.sort();
        for r in &registries {
            out.push_str("\n[[registry]]\n");
            out.push_str(&format!("name = {}\n", toml_string(&r.name)));
            out.push_str(&format!("id = {}\n", toml_string(&r.id)));
            out.push_str(&format!("key = {}\n", toml_string(&r.key)));
        }
        let mut npm = self.npm.clone();
        npm.sort();
        for n in &npm {
            out.push_str("\n[[npm]]\n");
            out.push_str(&format!("package = {}\n", toml_string(&n.package)));
            out.push_str(&format!("range = {}\n", toml_string(&n.range)));
            let by: Vec<String> = n.required_by.iter().map(|d| toml_string(d)).collect();
            out.push_str(&format!("required-by = [{}]\n", by.join(", ")));
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
        Some(_) => {
            return Err(LockError(
                "`bynk.lock`'s `package` must be an array of tables".into(),
            ));
        }
    };
    for entry in entries {
        let Some(t) = entry.as_table() else {
            return Err(LockError(
                "a `[[package]]` in `bynk.lock` isn't a table".into(),
            ));
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
            .map(|a| {
                a.iter()
                    .filter_map(|d| d.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        packages.push(LockedPackage {
            name,
            version: text("version"),
            source,
            checksum: text("checksum"),
            dependencies,
        });
    }
    let mut npm = Vec::new();
    let rows = match doc.get("npm") {
        None => Vec::new(),
        Some(toml::Value::Array(items)) => items.clone(),
        Some(_) => {
            return Err(LockError(
                "`bynk.lock`'s `npm` must be an array of tables".into(),
            ));
        }
    };
    for row in rows {
        let Some(t) = row.as_table() else {
            return Err(LockError(
                "an `[[npm]]` in `bynk.lock` isn't a table".into(),
            ));
        };
        let text = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_owned);
        let (Some(package), Some(range)) = (text("package"), text("range")) else {
            return Err(LockError(
                "an `[[npm]]` in `bynk.lock` lacks `package` or `range`".into(),
            ));
        };
        let required_by = match t.get("required-by") {
            None => Vec::new(),
            Some(toml::Value::Array(items)) => items
                .iter()
                .map(|d| d.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    LockError(
                        "an `[[npm]]`'s `required-by` in `bynk.lock` must list strings".into(),
                    )
                })?,
            Some(_) => {
                return Err(LockError(
                    "an `[[npm]]`'s `required-by` in `bynk.lock` must be a list".into(),
                ));
            }
        };
        npm.push(LockedNpm {
            package,
            range,
            required_by,
        });
    }
    let mut registries = Vec::new();
    let rows = match doc.get("registry") {
        None => Vec::new(),
        Some(toml::Value::Array(items)) => items.clone(),
        Some(_) => {
            return Err(LockError(
                "`bynk.lock`'s `registry` must be an array of tables".into(),
            ));
        }
    };
    for row in rows {
        let t = row.as_table();
        let text = |k: &str| t.and_then(|t| t.get(k)).and_then(|v| v.as_str()).map(str::to_owned);
        let (Some(name), Some(id), Some(key)) = (text("name"), text("id"), text("key")) else {
            return Err(LockError(
                "a `[[registry]]` in `bynk.lock` needs `name`, `id` and `key`".into(),
            ));
        };
        registries.push(LockedRegistry { name, id, key });
    }
    Ok(Lock {
        packages,
        registries,
        npm,
    })
}

/// #2274: a registry package's `source`: `registry+<name>`.
pub fn registry_source(name: &str) -> String {
    format!("registry+{name}")
}

/// The registry name of a `registry+<name>` source.
pub fn parse_registry_source(source: &str) -> Option<&str> {
    source.strip_prefix("registry+")
}

impl Lock {
    /// The locked package called `name`, whatever its source.
    pub fn package(&self, name: &str) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// The locked trust record for registry `name`.
    pub fn registry(&self, name: &str) -> Option<&LockedRegistry> {
        self.registries.iter().find(|r| r.name == name)
    }
}

/// #2255: the npm packages an adapter source file's `binding … requires { … }`
/// names, with the adapter's declared name: `None` when the file isn't an
/// adapter with a binding. Recovery-parsed, so a file with an error elsewhere
/// still yields its clause; the compile reports the error.
pub fn adapter_requires(source: &str) -> Option<(String, Vec<(String, String)>)> {
    let tokens = bynk_syntax::lexer::tokenize(source).ok()?;
    let (unit, _errors) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, source);
    let bynk_syntax::ast::SourceUnit::Adapter(a) = unit? else {
        return None;
    };
    let binding = a.binding.as_ref()?;
    Some((
        a.name.joined(),
        binding
            .requires
            .iter()
            .map(|d| (d.package.clone(), d.range.clone()))
            .collect(),
    ))
}

/// Where the lock for the project at `project_root` lives: its workspace's
/// root when it is a member (one lock per workspace, track §4.2), else its own
/// root.
pub fn lock_root(project_root: &Path) -> PathBuf {
    // A relative root (`.`) has no ancestors to search, so the workspace is
    // looked for from its real path.
    let real = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    crate::workspace::find_parent_workspace(&real).unwrap_or_else(|| project_root.to_path_buf())
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

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
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

pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
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
            registries: vec![LockedRegistry {
                name: "acme-internal".into(),
                id: "acme".into(),
                key: "ed25519-fp:abc".into(),
            }],
            npm: vec![LockedNpm {
                package: "jose".into(),
                range: "^5.2.0".into(),
                required_by: vec!["acme/ledger:auth".into()],
            }],
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
        assert!(text.contains("[[npm]]\npackage = \"jose\"\nrange = \"^5.2.0\"\nrequired-by = [\"acme/ledger:auth\"]\n"));
        assert!(text.find("[[npm]]").unwrap() > text.find("[[package]]").unwrap());
        assert!(text.contains("[[registry]]\nname = \"acme-internal\"\nid = \"acme\"\nkey = \"ed25519-fp:abc\"\n"));
        assert_eq!(back.registry("acme-internal").unwrap().id, "acme");
        let (p, commit) = back
            .git("acme/ledger", "https://x/ledger", "v0.3.0")
            .unwrap();
        assert_eq!(commit, "9f1c");
        assert_eq!(p.version.as_deref(), Some("0.3.0"));
        assert!(
            back.git("acme/ledger", "https://x/ledger", "main")
                .is_none()
        );
    }

    #[test]
    fn a_malformed_required_by_is_refused() {
        let row = |by: &str| {
            format!(
                "version = 1\n\n[[npm]]\npackage = \"jose\"\nrange = \"^5\"\nrequired-by = {by}\n"
            )
        };
        assert!(
            parse_lock(&row("\"a/b:c\""))
                .unwrap_err()
                .0
                .contains("must be a list")
        );
        assert!(
            parse_lock(&row("[1]"))
                .unwrap_err()
                .0
                .contains("must list strings")
        );
        assert_eq!(
            parse_lock(&row("[\"a/b:c\"]")).unwrap().npm[0].required_by,
            ["a/b:c"]
        );
    }

    #[test]
    fn a_lock_without_a_version_is_refused() {
        assert!(
            parse_lock("[[package]]\nname = \"a/b\"\nsource = \"path+x\"\n")
                .unwrap_err()
                .0
                .contains("no `version`")
        );
        assert!(
            parse_lock("version = 2\n")
                .unwrap_err()
                .0
                .contains("version 2")
        );
        assert_eq!(parse_lock("version = 1\n").unwrap(), Lock::default());
    }
}

#[cfg(test)]
mod requires_tests {
    use super::adapter_requires;

    #[test]
    fn an_adapters_requires_are_read() {
        let src = "adapter tokens {\n  binding \"./t.binding.ts\" requires { \"jose\": \"^5\", \"zod\": \"3.23.8\" }\n}\n";
        assert_eq!(
            adapter_requires(src),
            Some((
                "tokens".to_string(),
                vec![
                    ("jose".into(), "^5".into()),
                    ("zod".into(), "3.23.8".into())
                ]
            ))
        );
        assert_eq!(adapter_requires("commons x\n"), None);
    }
}
