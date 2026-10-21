//! Registry archives (packaging slice 4a, #2274, decision B): a package's
//! files in transit.
//!
//! An archive is the header `bynk-tree-v1\n` followed by the package's files,
//! each `path \0 length \0 bytes`, in byte-wise path order — the very stream
//! the canonical-tree checksum hashes ([`crate::fetch::tree_checksum`]). The
//! checksum a registry signs is that tree checksum, and the client admits an
//! unpacked tree only when its own tree checksum matches, so the archive is
//! transport alone: no tar, no links, no modes, nothing to canonicalise.
//!
//! Unpacking takes only regular files at plain relative paths: an absolute
//! path, a `..`, a `\`, an empty or repeated path, or a size past the caps is
//! refused (`bynk.registry.bad_archive`).

use std::collections::BTreeSet;
use std::path::{Component, Path};

use sha2::{Digest, Sha256};

use crate::fetch::{FetchError, fail};

pub(crate) const BAD_ARCHIVE: &str = "bynk.registry.bad_archive";

const MAGIC: &[u8] = b"bynk-tree-v1\n";

/// The most an archive may unpack to.
pub(crate) const MAX_UNPACKED: u64 = 500 * 1024 * 1024;
/// The most files an archive may hold.
const MAX_FILES: usize = 100_000;

/// What a package archive leaves out: build output, npm's cache, and every
/// hidden file or directory (`.git`, `.env`, editor state) — a published
/// package carries its sources, never a secret beside them.
fn excluded(rel: &str) -> bool {
    let mut parts = rel.split('/');
    let first = parts.next().unwrap_or_default();
    first == "out"
        || first == "node_modules"
        || rel.split('/').any(|p| p.starts_with('.'))
}

/// Pack the package at `dir`: the archive bytes and the tree checksum a
/// consumer will compute from them.
pub(crate) fn pack(dir: &Path) -> std::io::Result<(Vec<u8>, String)> {
    let files = crate::fetch::tree_files(dir)?;
    let mut out = MAGIC.to_vec();
    let mut hasher = Sha256::new();
    for (rel, path) in files.iter().filter(|(rel, _)| !excluded(rel)) {
        let bytes = std::fs::read(path)?;
        for chunk in [
            rel.as_bytes(),
            b"\0",
            bytes.len().to_string().as_bytes(),
            b"\0",
            &bytes,
        ] {
            out.extend_from_slice(chunk);
            hasher.update(chunk);
        }
    }
    let digest: [u8; 32] = hasher.finalize().into();
    Ok((out, bynk_project::lock::checksum_of(&digest)))
}

/// Unpack `archive` into the empty directory `into`.
pub(crate) fn unpack(archive: &[u8], into: &Path) -> Result<(), FetchError> {
    let bad = |why: &str| fail(BAD_ARCHIVE, format!("a registry archive is malformed: {why}"));
    let mut rest = archive
        .strip_prefix(MAGIC)
        .ok_or_else(|| bad("it isn't a `bynk-tree-v1` archive"))?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut total: u64 = 0;
    let mut previous: Option<String> = None;
    while !rest.is_empty() {
        let nul = rest.iter().position(|&b| b == 0).ok_or_else(|| bad("a path isn't terminated"))?;
        let rel = std::str::from_utf8(&rest[..nul]).map_err(|_| bad("a path isn't UTF-8"))?;
        rest = &rest[nul + 1..];
        let nul = rest.iter().position(|&b| b == 0).ok_or_else(|| bad("a length isn't terminated"))?;
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
        let path = into.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| fail(BAD_ARCHIVE, format!("could not unpack `{rel}`: {e}")))?;
        }
        std::fs::write(&path, bytes)
            .map_err(|e| fail(BAD_ARCHIVE, format!("could not unpack `{rel}`: {e}")))?;
    }
    Ok(())
}

/// Why `rel` isn't a plain relative file path, if it isn't.
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
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(_) => {}
            _ => return Err("climbs out of the package (`.`, `..` or a root)"),
        }
    }
    if rel.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
        return Err("climbs out of the package (`.`, `..` or a root)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("bynk-archive-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_package_round_trips_and_its_checksum_is_the_tree_checksum() {
        let src = scratch("src");
        std::fs::create_dir_all(src.join("src")).unwrap();
        std::fs::create_dir_all(src.join("out")).unwrap();
        std::fs::write(src.join("bynk.toml"), "[package]\nname = \"a/b\"\n").unwrap();
        std::fs::write(src.join("src/x.bynk"), "commons x\n").unwrap();
        std::fs::write(src.join("out/x.ts"), "built").unwrap();
        std::fs::write(src.join(".env"), "SECRET=1").unwrap();
        let (archive, checksum) = pack(&src).unwrap();
        let dst = scratch("dst");
        unpack(&archive, &dst).unwrap();
        assert!(!dst.join("out").exists() && !dst.join(".env").exists());
        assert_eq!(crate::fetch::tree_checksum(&dst).unwrap(), checksum);
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
        ];
        for (i, case) in cases.iter().enumerate() {
            let dst = scratch(&format!("bad{i}"));
            let err = unpack(case, &dst).unwrap_err();
            assert_eq!(err.code, BAD_ARCHIVE, "case {i}: {}", err.message);
        }
    }
}
