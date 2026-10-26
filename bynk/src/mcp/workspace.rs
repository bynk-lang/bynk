//! What the MCP tools may read, and how a project is analysed from disk.
//!
//! The driver is the process edge, so it is where a project's files are read
//! (`bynk-ide` touches no disk): [`Workspace::analyse`] hands `bynk-ide` a
//! complete `(path, content)` map, as the language server's content sweep does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_ide::{AnalysisRoots, ProjectDiagnostics, SchemaLock};

/// Directories a project search never enters.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "out", "dist"];

/// How deep below the root a project search looks for a `bynk.toml`.
const SEARCH_DEPTH: usize = 4;

/// The directory the tools may read, canonicalised so that containment checks
/// are not fooled by `..` or a symlink.
pub struct Workspace {
    root: PathBuf,
}

/// One analysed project.
pub struct Project {
    /// The project root (a directory with a `bynk.toml`).
    pub root: PathBuf,
    pub analysis: ProjectDiagnostics,
}

impl Workspace {
    pub fn new(path: &Path) -> Result<Self, String> {
        let root = path
            .canonicalize()
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if !root.is_dir() {
            return Err(format!("{} is not a directory", path.display()));
        }
        Ok(Self { root })
    }

    /// Resolve a tool's `file` argument — an absolute path or one relative to
    /// the workspace root — to an existing file inside the workspace. A path
    /// that resolves elsewhere (`..`, a symlink out) is refused: the tools run
    /// without a confirmation, so they must not be steerable at other files.
    pub fn resolve_file(&self, file: &str) -> Result<PathBuf, String> {
        let given = Path::new(file);
        let joined = if given.is_absolute() {
            given.to_path_buf()
        } else {
            self.root.join(given)
        };
        let resolved = joined.canonicalize().map_err(|_| {
            format!(
                "No file `{file}` in this workspace. Give a path relative to the workspace \
                 root, or an absolute path."
            )
        })?;
        if !resolved.starts_with(&self.root) {
            return Err(format!("`{file}` is outside this workspace."));
        }
        Ok(resolved)
    }

    /// The path an agent sees: relative to the workspace root, `/`-separated.
    pub fn display(&self, path: &Path) -> String {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        let text = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if text.is_empty() {
            ".".to_string()
        } else {
            text
        }
    }

    /// The nearest enclosing project of `path`: the first directory at or above
    /// it, up to the workspace root, that holds a `bynk.toml` which is not a
    /// workspace manifest.
    pub fn project_root_for(&self, path: &Path) -> Option<PathBuf> {
        let mut dir = if path.is_dir() {
            Some(path)
        } else {
            path.parent()
        };
        while let Some(d) = dir {
            if !d.starts_with(&self.root) {
                return None;
            }
            if d.join("bynk.toml").is_file() && !bynk_ide::is_workspace_root(d) {
                return Some(d.to_path_buf());
            }
            dir = d.parent();
        }
        None
    }

    /// Every project under the workspace root, in path order.
    pub fn project_roots(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        self.collect_projects(&self.root, 0, &mut found);
        found.sort();
        found
    }

    fn collect_projects(&self, dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
        if dir.join("bynk.toml").is_file() && !bynk_ide::is_workspace_root(dir) {
            found.push(dir.to_path_buf());
            return;
        }
        if depth >= SEARCH_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            // `is_dir` follows symlinks; a link out of the workspace is skipped.
            let path = entry.path();
            if path.is_dir() && path.canonicalize().is_ok_and(|p| p.starts_with(&self.root)) {
                self.collect_projects(&path, depth + 1, found);
            }
        }
    }

    /// The project a tool call means: the one enclosing `file` if given, else
    /// the workspace's only project.
    pub fn pick_project(&self, file: Option<&str>) -> Result<PathBuf, String> {
        if let Some(file) = file {
            let path = self.resolve_file(file)?;
            return self.project_root_for(&path).ok_or_else(|| {
                format!(
                    "`{file}` is not inside a Bynk project (no `bynk.toml` at or above it \
                     within the workspace)."
                )
            });
        }
        let mut roots = self.project_roots();
        match roots.len() {
            0 => Err("No `bynk.toml` found in this workspace.".to_string()),
            1 => Ok(roots.remove(0)),
            _ => Err(format!(
                "This workspace holds {} Bynk projects ({}); pass `file` (any file in the one \
                 you mean).",
                roots.len(),
                roots
                    .iter()
                    .map(|r| format!("`{}`", self.display(r)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// Analyse the project at `root` from the files on disk, as the language
    /// server's round does (including the `bynk.schema.lock` reconciliation, so
    /// a non-additive event change is reported as `bynk check` reports it).
    pub fn analyse(&self, root: &Path) -> Project {
        let roots = AnalysisRoots::Project(root.to_path_buf());
        let mut content: HashMap<PathBuf, String> = HashMap::new();
        // The manifest first: it decides which trees `discover_files` walks.
        let manifest = root.join("bynk.toml");
        if let Ok(text) = std::fs::read_to_string(&manifest) {
            content.insert(manifest, text);
        }
        for path in bynk_ide::discover_files(&roots, &content) {
            // The manifest's `[paths]` and a symlinked directory can both lead
            // out of the workspace; a source that resolves elsewhere is not read,
            // so no tool can show an agent a file outside `PATH`.
            if !path.canonicalize().is_ok_and(|p| p.starts_with(&self.root)) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                content.insert(path, text);
            }
        }
        let lock = match std::fs::read_to_string(root.join("bynk.schema.lock")) {
            Ok(text) => SchemaLock::On {
                existing: Some(text),
            },
            Err(_) => SchemaLock::Off,
        };
        Project {
            root: root.to_path_buf(),
            analysis: bynk_ide::diagnose_project_with(&roots, &content, &lock),
        }
    }
}

impl Project {
    /// A project-relative source path as the agent sees it.
    pub fn display(&self, ws: &Workspace, rel: &Path) -> String {
        ws.display(&self.root.join(rel))
    }

    /// The project-relative form of a canonical path to an analysed file. A
    /// source reached through a symlinked directory is analysed under its
    /// literal path, so a plain prefix strip is tried first and the analysed
    /// files' canonical paths second.
    pub fn rel(&self, path: &Path) -> Option<PathBuf> {
        if let Ok(rel) = path.strip_prefix(&self.root)
            && self.text(rel).is_some()
        {
            return Some(rel.to_path_buf());
        }
        self.analysis
            .files
            .iter()
            .find(|f| {
                self.root
                    .join(&f.source_path)
                    .canonicalize()
                    .is_ok_and(|c| c == path)
            })
            .map(|f| f.source_path.clone())
    }

    /// The analysed text of a project-relative file.
    pub fn text(&self, rel: &Path) -> Option<&str> {
        self.analysis
            .files
            .iter()
            .find(|f| f.source_path == rel)
            .map(|f| f.text.as_str())
    }

    /// Every analysed file, project-relative path to text.
    pub fn snapshots(&self) -> HashMap<PathBuf, String> {
        self.analysis
            .files
            .iter()
            .map(|f| (f.source_path.clone(), f.text.clone()))
            .collect()
    }
}

/// `(line, column)`, both 1-based, of byte `offset` in `text`. Columns count
/// UTF-16 code units, as the editor tools (and LSP) do, so a location quoted
/// from either surface agrees.
pub fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..floor_boundary(text, offset)];
    let line = before.matches('\n').count() + 1;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = before[start..].encode_utf16().count() + 1;
    (line, column)
}

/// The byte offset of 1-based `line` and (optional) 1-based UTF-16 `column`;
/// with no column, the line's first non-blank character.
pub fn offset_of(text: &str, line: usize, column: Option<usize>) -> Result<usize, String> {
    let count = text.split('\n').count();
    if line < 1 || line > count {
        return Err(format!("Line {line} is outside the file (1–{count})."));
    }
    let start: usize = text.split('\n').take(line - 1).map(|l| l.len() + 1).sum();
    let line_text = text[start..].split('\n').next().unwrap_or("");
    let Some(column) = column else {
        let blank = line_text.len() - line_text.trim_start().len();
        return Ok(start + blank);
    };
    let mut units = 0;
    let want = column.saturating_sub(1);
    for (i, c) in line_text.char_indices() {
        if units >= want {
            return Ok(start + i);
        }
        units += c.len_utf16();
    }
    Ok(start + line_text.trim_end_matches('\r').len())
}

/// The byte offset of the first whole-word occurrence of `name` on 1-based `line`.
pub fn offset_of_name(text: &str, line: usize, name: &str) -> Result<usize, String> {
    let start = offset_of(text, line, Some(1))?;
    let line_text = text[start..].split('\n').next().unwrap_or("");
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(i) = line_text[from..].find(name) {
        let at = from + i;
        let end = at + name.len();
        let before_ok = !line_text[..at].chars().next_back().is_some_and(is_word);
        let after_ok = !line_text[end..].chars().next().is_some_and(is_word);
        if before_ok && after_ok {
            return Ok(start + at);
        }
        from = at + name.chars().next().map_or(1, char::len_utf8);
    }
    Err(format!("`{name}` does not appear on line {line}."))
}

fn floor_boundary(text: &str, mut i: usize) -> usize {
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_is_one_based_and_counts_utf16_units() {
        let text = "ab\n😀x\n";
        assert_eq!(line_col(text, 0), (1, 1));
        assert_eq!(line_col(text, 3), (2, 1));
        // After the emoji (4 bytes, 2 UTF-16 units) the column is 3, not 2.
        assert_eq!(line_col(text, 7), (2, 3));
    }

    #[test]
    fn offset_of_inverts_line_col() {
        let text = "ab\n😀x\n  y";
        for off in [0, 1, 3, 7, 10] {
            let (l, c) = line_col(text, off);
            assert_eq!(offset_of(text, l, Some(c)).unwrap(), off, "{l}:{c}");
        }
        assert_eq!(offset_of(text, 3, None).unwrap(), 11);
        assert!(offset_of(text, 9, None).is_err());
    }

    #[test]
    fn a_name_matches_whole_words_only() {
        let text = "let counter = Counter.make(counter)";
        assert_eq!(offset_of_name(text, 1, "Counter").unwrap(), 14);
        assert_eq!(offset_of_name(text, 1, "counter").unwrap(), 4);
        assert!(offset_of_name(text, 1, "count").is_err());
    }
}
