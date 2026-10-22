//! Registry archives on disk (packaging slice 4a, #2274, decision B): the
//! filesystem edge of `bynk-registry-core`, which holds the format, the
//! hardening rules and the checksum. Here a package directory is read into a
//! [`Tree`] to pack, and an unpacked tree is written into a directory.
//!
//! Unpacking takes only regular files at plain relative paths: an absolute
//! path, a `..`, a `\`, an empty or repeated path, or a size past the caps is
//! refused (`bynk.registry.bad_archive`).

use std::path::Path;

use bynk_registry_core::{Tree, is_excluded};

use crate::fetch::{FetchError, fail};

pub(crate) use bynk_registry_core::BAD_ARCHIVE;

/// Pack the package at `dir`: the archive bytes and the tree checksum a
/// consumer will compute from them.
pub(crate) fn pack(dir: &Path) -> std::io::Result<(Vec<u8>, String)> {
    let mut tree = Tree::new();
    for (rel, path) in crate::fetch::tree_files(dir)?
        .into_iter()
        .filter(|(rel, _)| !is_excluded(rel))
    {
        tree.insert(rel, std::fs::read(path)?);
    }
    Ok(bynk_registry_core::pack(&tree))
}

/// Unpack `archive` into the empty directory `into`.
pub(crate) fn unpack(archive: &[u8], into: &Path) -> Result<(), FetchError> {
    let tree = bynk_registry_core::unpack(archive).map_err(|e| fail(BAD_ARCHIVE, e.message))?;
    for (rel, bytes) in tree.iter() {
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
