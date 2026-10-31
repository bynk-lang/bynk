//! #1883 (static assets), slice 1b: the project's assets folder — the one
//! directory an `asset name = "path"` declaration may name a file in — and
//! the walk that reads it.
//!
//! Assets are public by construction, so they come only from
//! `<project root>/assets/` (decided on #1883 after the review of #1903): a
//! declaration can never publish `.env` or a `.bynk` source by mistake. The
//! checker states the rule (`bynk.asset.outside_assets_dir`); the driver and
//! the test harnesses list the folder as an [`AssetFolder`], so `bynk-emit`
//! and `bynk-check` stay disk-free and read the bytes of only the files a
//! declaration names, and the LSP lists it with [`list_asset_files`] for the
//! existence check.

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The project's assets folder, relative to the project root.
pub const PROJECT_ASSETS_DIR: &str = "assets";

/// `path` as the key [`list_asset_files`] files it under: separators made
/// `/`, and empty and `.` segments dropped, so `./assets//board.css` and
/// `assets\board.css` both become `assets/board.css`. A `..` segment is kept
/// (the checker rejects it separately, `bynk.asset.unsafe_path`).
pub fn normalise_asset_path(path: &str) -> String {
    path.split(['/', '\\'])
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether the normalised `path` names a file inside the assets folder —
/// `assets/<at least one more segment>`.
pub fn is_in_assets_dir(path: &str) -> bool {
    let normalised = normalise_asset_path(path);
    let mut segs = normalised.split('/');
    segs.next() == Some(PROJECT_ASSETS_DIR) && segs.next().is_some()
}

/// The project's embedded-text folder, relative to the project root (#2387).
/// Unlike [`PROJECT_ASSETS_DIR`] nothing in it is served: an `embed` compiles a
/// file's text into the code. It is listed and read as an [`AssetFolder`]
/// ([`EmbedFolder`]).
pub const PROJECT_EMBEDS_DIR: &str = "embeds";

/// The project's `embeds/` folder as the compiler sees it: an [`AssetFolder`]
/// over `embeds/` instead of `assets/` (#2387).
pub type EmbedFolder = AssetFolder;

/// Whether the normalised `path` names a file inside the embeds folder —
/// `embeds/<at least one more segment>`.
pub fn is_in_embeds_dir(path: &str) -> bool {
    let normalised = normalise_asset_path(path);
    let mut segs = normalised.split('/');
    segs.next() == Some(PROJECT_EMBEDS_DIR) && segs.next().is_some()
}

/// Standard-library P4b (#2608): the project's D1 migrations folder,
/// relative to the project root. Each `D1` instance's migrations are
/// `migrations/<database>/NNNN_<name>.sql`; it is listed and read as an
/// [`AssetFolder`].
pub const PROJECT_MIGRATIONS_DIR: &str = "migrations";

/// Reads one listed asset file's bytes, given its normalised key.
pub type AssetReader = dyn Fn(&str) -> io::Result<Vec<u8>> + Send + Sync;

/// The project's `assets/` folder as the compiler sees it: the files in it,
/// listed up front, and a reader the compiler calls for the bytes of only
/// those a declaration names. A large file nothing declares is never read.
///
/// The caller decides where the bytes come from: [`AssetFolder::on_disk`]
/// for a real build, [`AssetFolder::new`] with its own reader otherwise.
/// `bynk-emit` only ever calls [`AssetFolder::read`], so it stays disk-free.
#[derive(Clone)]
pub struct AssetFolder {
    files: BTreeSet<String>,
    reader: Arc<AssetReader>,
}

impl AssetFolder {
    /// A folder of `files` (normalised keys, `assets/board.css`) whose bytes
    /// come from `reader`. The compiler calls `reader` only with a key in
    /// `files`, at most once per build.
    pub fn new(files: BTreeSet<String>, reader: Arc<AssetReader>) -> Self {
        Self { files, reader }
    }

    /// `<project_root>/assets/`, listed now ([`list_asset_files`]: regular
    /// files only, symbolic links not followed, a missing folder empty) and
    /// read later, file by file, as the compiler asks. The folder's real
    /// location is fixed now too, so a read whose path resolves anywhere else
    /// — a directory swapped for a symbolic link after the listing — is
    /// refused rather than published.
    pub fn on_disk(project_root: &Path) -> io::Result<Self> {
        Self::on_disk_in(project_root, PROJECT_ASSETS_DIR)
    }

    /// [`AssetFolder::on_disk`] for the project's `embeds/` folder (#2387):
    /// the same listing and the same refusals, a different directory.
    pub fn embeds_on_disk(project_root: &Path) -> io::Result<Self> {
        Self::on_disk_in(project_root, PROJECT_EMBEDS_DIR)
    }

    /// [`AssetFolder::on_disk`] for the project's `migrations/` folder
    /// (#2608): the same listing and the same refusals.
    pub fn migrations_on_disk(project_root: &Path) -> io::Result<Self> {
        Self::on_disk_in(project_root, PROJECT_MIGRATIONS_DIR)
    }

    fn on_disk_in(project_root: &Path, folder: &str) -> io::Result<Self> {
        let files = list_files_in(project_root, folder)?;
        if files.is_empty() {
            // Nothing listed, so `read` never reaches the reader.
            return Ok(Self::new(files, Arc::new(|_: &str| Err(not_listed()))));
        }
        let root: PathBuf = project_root.to_path_buf();
        let dir = std::fs::canonicalize(root.join(folder))?;
        Ok(Self::new(
            files,
            Arc::new(move |key: &str| read_listed_file(&root, &dir, key)),
        ))
    }

    /// The files in the folder, by normalised key.
    pub fn files(&self) -> &BTreeSet<String> {
        &self.files
    }

    /// The bytes of the listed file `key`. A key the listing does not hold is
    /// `NotFound` without consulting the reader, so no caller can steer it
    /// outside the folder (`normalise_asset_path` keeps `..` segments).
    pub fn read(&self, key: &str) -> io::Result<Vec<u8>> {
        if !self.files.contains(key) {
            return Err(not_listed());
        }
        (self.reader)(key)
    }
}

impl fmt::Debug for AssetFolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetFolder")
            .field("files", &self.files)
            .finish_non_exhaustive()
    }
}

fn not_listed() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "not a listed asset file")
}

/// Read the listed file `key` under `root`, refusing it if it has become a
/// symbolic link since the folder was listed, or if its path now resolves
/// outside `dir` (the assets folder's canonical path, fixed at listing) —
/// a folder on the way swapped for a link — so no link is ever followed.
fn read_listed_file(root: &Path, dir: &Path, key: &str) -> io::Result<Vec<u8>> {
    let path = root.join(key);
    if !std::fs::symlink_metadata(&path)?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let resolved = std::fs::canonicalize(&path)?;
    if !resolved.starts_with(dir) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "resolves outside the assets folder",
        ));
    }
    std::fs::read(resolved)
}

/// Every regular file under `<project_root>/assets/`, by its normalised
/// project-relative path (`assets/board.css`), without reading any bytes.
/// Symbolic links are not followed, so a link cannot pull a file in from
/// outside the folder; a missing folder is an empty set.
pub fn list_asset_files(project_root: &Path) -> io::Result<BTreeSet<String>> {
    list_files_in(project_root, PROJECT_ASSETS_DIR)
}

/// [`list_asset_files`] for the project's `embeds/` folder (#2387).
pub fn list_embed_files(project_root: &Path) -> io::Result<BTreeSet<String>> {
    list_files_in(project_root, PROJECT_EMBEDS_DIR)
}

/// [`list_asset_files`] for the project's `migrations/` folder (#2608).
pub fn list_migration_files(project_root: &Path) -> io::Result<BTreeSet<String>> {
    list_files_in(project_root, PROJECT_MIGRATIONS_DIR)
}

fn list_files_in(project_root: &Path, folder: &str) -> io::Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    walk(project_root, folder, &mut |rel| {
        out.insert(rel);
    })?;
    Ok(out)
}

fn walk(project_root: &Path, folder: &str, visit: &mut dyn FnMut(String)) -> io::Result<()> {
    let dir = project_root.join(folder);
    if !dir.is_dir() {
        return Ok(());
    }
    walk_dir(&dir, folder, visit)
}

fn walk_dir(dir: &Path, rel: &str, visit: &mut dyn FnMut(String)) -> io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        // `DirEntry::file_type` does not follow a symlink, so a link is
        // neither a file nor a directory here and is skipped.
        let ty = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = format!("{rel}/{name}");
        if ty.is_dir() {
            walk_dir(&entry.path(), &child, visit)?;
        } else if ty.is_file() {
            visit(child);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_and_folder_membership() {
        assert_eq!(
            normalise_asset_path("./assets//board.css"),
            "assets/board.css"
        );
        assert_eq!(
            normalise_asset_path("assets\\img\\a.png"),
            "assets/img/a.png"
        );
        assert!(is_in_assets_dir("assets/board.css"));
        assert!(is_in_assets_dir("./assets/.well-known/security.txt"));
        assert!(!is_in_assets_dir("assets"));
        assert!(!is_in_assets_dir("assets/"));
        assert!(!is_in_assets_dir(".env"));
        assert!(!is_in_assets_dir("src/app/page.bynk"));
        assert!(!is_in_assets_dir("public/assets/x.css"));
    }

    /// #2387: `embeds/` is listed and read like `assets/`, and the two folders
    /// do not see each other's files.
    #[test]
    fn the_embeds_folder_is_listed_apart_from_assets() {
        assert!(is_in_embeds_dir("embeds/page.html"));
        assert!(is_in_embeds_dir("./embeds//a/b.txt"));
        assert!(!is_in_embeds_dir("embeds"));
        assert!(!is_in_embeds_dir("assets/page.html"));
        assert!(!is_in_assets_dir("embeds/page.html"));

        let root = std::env::temp_dir().join(format!("bynk-project-embeds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("embeds/mail")).unwrap();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("embeds/page.html"), b"<p>").unwrap();
        std::fs::write(root.join("embeds/mail/welcome.txt"), b"hi").unwrap();
        std::fs::write(root.join("assets/board.css"), b"a{}").unwrap();

        let embeds = AssetFolder::embeds_on_disk(&root).unwrap();
        assert_eq!(
            embeds.files().iter().cloned().collect::<Vec<_>>(),
            vec!["embeds/mail/welcome.txt", "embeds/page.html"]
        );
        assert_eq!(embeds.read("embeds/page.html").unwrap(), b"<p>");
        // The assets folder's file is not in the embeds listing, so it cannot
        // be read through it.
        assert_eq!(
            embeds.read("assets/board.css").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            list_asset_files(&root)
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["assets/board.css"]
        );
        assert!(
            AssetFolder::embeds_on_disk(&root.join("nowhere"))
                .unwrap()
                .files()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_walk_reads_regular_files_and_skips_links() {
        let root = std::env::temp_dir().join(format!("bynk-project-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets/img")).unwrap();
        std::fs::write(root.join("assets/board.css"), b"body{}").unwrap();
        std::fs::write(root.join("assets/img/a.png"), [0x89, 0x00]).unwrap();
        std::fs::write(root.join("secret.txt"), b"no").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("secret.txt"), root.join("assets/leak.txt")).unwrap();

        let folder = AssetFolder::on_disk(&root).unwrap();
        assert_eq!(
            folder.files().iter().cloned().collect::<Vec<_>>(),
            vec!["assets/board.css", "assets/img/a.png"]
        );
        assert_eq!(folder.read("assets/img/a.png").unwrap(), vec![0x89, 0x00]);
        // Only listed keys are read: a link, or a `..` escape, is refused
        // by name, whatever the reader would do with it.
        #[cfg(unix)]
        assert!(folder.read("assets/leak.txt").is_err());
        assert_eq!(
            folder.read("assets/../secret.txt").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(
            AssetFolder::on_disk(&root.join("nowhere"))
                .unwrap()
                .files()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Review of #1905: a directory inside `assets/` swapped for a symbolic
    /// link between the listing and the read does not pull a file in from
    /// outside the folder, though the file at the end is a regular one.
    #[cfg(unix)]
    #[test]
    fn a_folder_swapped_for_a_link_after_listing_is_not_followed() {
        let root =
            std::env::temp_dir().join(format!("bynk-project-assets-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets/css")).unwrap();
        std::fs::write(root.join("assets/css/palette.css"), b"a{}").unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        std::fs::write(root.join("outside/palette.css"), b"secret").unwrap();

        let folder = AssetFolder::on_disk(&root).unwrap();
        assert_eq!(folder.read("assets/css/palette.css").unwrap(), b"a{}");
        std::fs::remove_dir_all(root.join("assets/css")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("assets/css")).unwrap();
        let err = folder.read("assets/css/palette.css").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
