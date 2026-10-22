//! Registry archives (packaging slice 4a, #2274, decision B): a package's
//! files in transit, held in memory.
//!
//! An archive is the header `bynk-tree-v1\n` followed by the package's files,
//! each `path \0 length \0 bytes`, in byte-wise path order — the very stream
//! the canonical-tree checksum hashes. The checksum a registry signs is that
//! tree checksum, and the client admits an unpacked tree only when its own tree
//! checksum matches, so the archive is transport alone: no tar, no links, no
//! modes, nothing to canonicalise (and so no compression backend to carry).
//!
//! Unpacking takes only regular files at plain relative paths: an absolute
//! path, a `..`, a `\`, an empty or repeated path, or a size past the caps is
//! refused (`bynk.registry.bad_archive`).

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

pub const BAD_ARCHIVE: &str = "bynk.registry.bad_archive";

const MAGIC: &[u8] = b"bynk-tree-v1\n";

/// The most an archive may unpack to.
pub const MAX_UNPACKED: u64 = 500 * 1024 * 1024;
/// The most files an archive may hold.
const MAX_FILES: usize = 100_000;

/// Why an archive was refused: always `bynk.registry.bad_archive`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveError {
    pub message: String,
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// A package's files in memory: `/`-separated relative path to bytes, ordered
/// byte-wise by path — the order an archive and its checksum use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    files: BTreeMap<String, Vec<u8>>,
}

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        self.files.insert(path.into(), bytes);
    }

    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    /// The files in path order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.files.iter().map(|(p, b)| (p.as_str(), b.as_slice()))
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl FromIterator<(String, Vec<u8>)> for Tree {
    fn from_iter<I: IntoIterator<Item = (String, Vec<u8>)>>(iter: I) -> Self {
        Tree {
            files: iter.into_iter().collect(),
        }
    }
}

/// What a package archive leaves out: build output, npm's cache, and every
/// hidden file or directory (`.git`, `.env`, editor state) — a published
/// package carries its sources, never a secret beside them.
pub fn is_excluded(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or_default();
    first == "out" || first == "node_modules" || rel.split('/').any(|p| p.starts_with('.'))
}

/// Pack `tree` canonically: the archive bytes and the tree checksum a consumer
/// will compute from them. Excluded files ([`is_excluded`]) are left out.
pub fn pack(tree: &Tree) -> (Vec<u8>, String) {
    let mut out = MAGIC.to_vec();
    let mut hasher = Sha256::new();
    for (rel, bytes) in tree.iter().filter(|(rel, _)| !is_excluded(rel)) {
        for chunk in [
            rel.as_bytes(),
            b"\0",
            bytes.len().to_string().as_bytes(),
            b"\0",
            bytes,
        ] {
            out.extend_from_slice(chunk);
            hasher.update(chunk);
        }
    }
    let digest: [u8; 32] = hasher.finalize().into();
    (out, bynk_project::lock::checksum_of(&digest))
}

/// Unpack `archive` into a [`Tree`], applying the hardening rules above.
pub fn unpack(archive: &[u8]) -> Result<Tree, ArchiveError> {
    let bad = |why: &str| ArchiveError {
        message: format!("a registry archive is malformed: {why}"),
    };
    let mut rest = archive
        .strip_prefix(MAGIC)
        .ok_or_else(|| bad("it isn't a `bynk-tree-v1` archive"))?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut tree = Tree::new();
    let mut total: u64 = 0;
    let mut previous: Option<String> = None;
    while !rest.is_empty() {
        let nul = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| bad("a path isn't terminated"))?;
        let rel = std::str::from_utf8(&rest[..nul]).map_err(|_| bad("a path isn't UTF-8"))?;
        rest = &rest[nul + 1..];
        let nul = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| bad("a length isn't terminated"))?;
        let len: u64 = std::str::from_utf8(&rest[..nul])
            .ok()
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| bad("a length isn't a number"))?;
        rest = &rest[nul + 1..];
        if len > rest.len() as u64 {
            return Err(bad("a file is shorter than its length"));
        }
        let (bytes, tail) = rest.split_at(len as usize);
        rest = tail;
        check_path(rel).map_err(|why| bad(&format!("`{rel}` {why}")))?;
        if previous.as_deref().is_some_and(|p| p >= rel) || !seen.insert(rel.to_owned()) {
            return Err(bad(&format!("`{rel}` is out of order or repeated")));
        }
        previous = Some(rel.to_owned());
        total += len;
        if total > MAX_UNPACKED || seen.len() > MAX_FILES {
            return Err(bad("it unpacks past the size or file-count limit"));
        }
        tree.insert(rel, bytes.to_vec());
    }
    Ok(tree)
}

/// Why `rel` isn't a plain relative file path, if it isn't. Lexical only: no
/// `std::path`, so the answer is the same on every platform.
fn check_path(rel: &str) -> Result<(), &'static str> {
    if rel.is_empty() {
        return Err("is empty");
    }
    if rel.contains('\\') || rel.contains(':') {
        return Err("isn't a plain `/`-separated path");
    }
    if rel.starts_with('/') {
        return Err("is absolute");
    }
    if rel
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err("climbs out of the package (`.`, `..` or a root)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_round_trips_and_leaves_out_what_a_package_never_ships() {
        let mut tree = Tree::new();
        tree.insert("bynk.toml", b"[package]\n".to_vec());
        tree.insert("src/x.bynk", b"commons x\n".to_vec());
        tree.insert("out/x.ts", b"built".to_vec());
        tree.insert(".env", b"SECRET=1".to_vec());
        tree.insert("node_modules/a/b.js", b"x".to_vec());
        let (archive, checksum) = pack(&tree);
        let back = unpack(&archive).unwrap();
        let paths: Vec<_> = back.iter().map(|(p, _)| p).collect();
        assert_eq!(paths, ["bynk.toml", "src/x.bynk"]);
        assert_eq!(pack(&back), (archive, checksum));
    }

    #[test]
    fn hostile_archives_are_refused() {
        let cases: &[&[u8]] = &[
            b"tar\0",
            b"bynk-tree-v1\n/etc/passwd\x001\0x",
            b"bynk-tree-v1\n../x\x001\0x",
            b"bynk-tree-v1\na/../../x\x001\0x",
            b"bynk-tree-v1\nC:\\x\x001\0x",
            b"bynk-tree-v1\nb\x001\0xa\x001\0x",
            b"bynk-tree-v1\na\x0099\0x",
            b"bynk-tree-v1\na\x00-1\0x",
            b"bynk-tree-v1\n\x001\0x",
            b"bynk-tree-v1\na//b\x001\0x",
        ];
        for (i, case) in cases.iter().enumerate() {
            assert!(unpack(case).is_err(), "case {i}");
        }
    }
}
