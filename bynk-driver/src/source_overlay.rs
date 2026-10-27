//! #2471: an in-memory overlay of unsaved editor buffers for a build.
//!
//! `bynkc compile --sources-stdin` reads a JSON object `{ "<absolute path>":
//! "<text>" }` from stdin and layers it over the sources the build reads from
//! disk, so an editor can compile what is on screen without saving it. The
//! overlay is the `CompileOptions::sources` map `compile_project` already
//! accepts; this module only reconciles its keys with the shape discovery
//! produced.
//!
//! **Which paths are accepted.** A path naming a source the build already read
//! replaces that source's text (the overlay wins over disk). A path that names
//! a `.bynk` file inside the project root and no existing source is added: an
//! unsaved new file. Anything else (a path outside the project, a non-`.bynk`
//! file) is *rejected* rather than ignored: the caller would otherwise believe
//! its buffer was compiled. A rejected overlay builds nothing.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use bynk_emit::project::CompileOptions;

/// Why an overlay could not be applied.
#[derive(Debug, PartialEq, Eq)]
pub enum OverlayError {
    /// The stdin document is not a JSON object of strings.
    Malformed(String),
    /// The build has no in-memory sources to layer onto (it read none).
    NoSources,
    /// `path` lies outside the project root.
    OutsideProject(PathBuf),
    /// `path` is not a `.bynk` source.
    NotBynk(PathBuf),
}

impl fmt::Display for OverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OverlayError::Malformed(e) => write!(
                f,
                "the sources document must be a JSON object of path to text: {e}"
            ),
            OverlayError::NoSources => write!(f, "the project has no sources to overlay"),
            OverlayError::OutsideProject(p) => {
                write!(f, "`{}` is outside the project", p.display())
            }
            OverlayError::NotBynk(p) => write!(f, "`{}` is not a .bynk source", p.display()),
        }
    }
}

impl std::error::Error for OverlayError {}

/// Parse the `--sources-stdin` document.
pub fn parse_overlay(json: &str) -> Result<HashMap<PathBuf, String>, OverlayError> {
    let map: HashMap<String, String> =
        serde_json::from_str(json).map_err(|e| OverlayError::Malformed(e.to_string()))?;
    Ok(map
        .into_iter()
        .map(|(k, v)| (PathBuf::from(k), v))
        .collect())
}

/// Layer `overlay` over `options`' sources for the project rooted at `root`
/// (the directory the build was pointed at). See the module doc for which
/// paths are accepted.
pub fn apply_overlay(
    options: CompileOptions,
    root: &Path,
    overlay: HashMap<PathBuf, String>,
) -> Result<CompileOptions, OverlayError> {
    let mut sources = options.sources.clone().ok_or(OverlayError::NoSources)?;
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    // Existing keys by canonical path, so an overlay entry finds the key
    // discovery used however either side spells the path.
    let by_canonical: HashMap<PathBuf, PathBuf> = sources
        .keys()
        .map(|k| (k.canonicalize().unwrap_or_else(|_| k.clone()), k.clone()))
        .collect();
    let mut replacements = Vec::new();
    for (path, text) in overlay {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if let Some(key) = by_canonical.get(&canonical) {
            replacements.push((key.clone(), text));
            continue;
        }
        let Ok(rel) = canonical.strip_prefix(&canonical_root) else {
            return Err(OverlayError::OutsideProject(path));
        };
        if canonical.extension().and_then(|e| e.to_str()) != Some("bynk") {
            return Err(OverlayError::NotBynk(path));
        }
        replacements.push((root.join(rel), text));
    }
    for (key, text) in replacements {
        sources.insert(key, text);
    }
    Ok(options.sources(sources))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn project(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "bynk_driver_overlay_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"p\"\n").unwrap();
        std::fs::write(dir.join("src/a.bynk"), "context p.a\n").unwrap();
        Scratch(dir.canonicalize().unwrap())
    }

    fn text_of(options: &CompileOptions, file: &Path) -> Option<String> {
        let want = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        options
            .sources
            .as_ref()?
            .iter()
            .find(|(k, _)| k.canonicalize().unwrap_or_else(|_| (*k).clone()) == want)
            .map(|(_, v)| v.clone())
    }

    #[test]
    fn the_overlay_wins_over_disk() {
        let dir = project("wins");
        let options = crate::try_project_options(&dir.0).unwrap();
        let a = dir.0.join("src/a.bynk");
        let out = apply_overlay(
            options,
            &dir.0,
            HashMap::from([(a.clone(), "UNSAVED".into())]),
        )
        .unwrap();
        assert_eq!(text_of(&out, &a).as_deref(), Some("UNSAVED"));
        // Replaced in place: no second entry for the same file.
        let same = out
            .sources
            .as_ref()
            .unwrap()
            .keys()
            .filter(|k| k.ends_with("src/a.bynk"))
            .count();
        assert_eq!(same, 1);
    }

    #[test]
    fn a_new_bynk_file_inside_the_project_is_added() {
        let dir = project("new");
        let options = crate::try_project_options(&dir.0).unwrap();
        let b = dir.0.join("src/b.bynk");
        let out = apply_overlay(
            options,
            &dir.0,
            HashMap::from([(b.clone(), "context p.b\n".into())]),
        )
        .unwrap();
        assert_eq!(text_of(&out, &b).as_deref(), Some("context p.b\n"));
    }

    #[test]
    fn a_path_outside_the_project_is_rejected() {
        let dir = project("outside");
        let options = crate::try_project_options(&dir.0).unwrap();
        let elsewhere = std::env::temp_dir().join("elsewhere.bynk");
        let err = apply_overlay(
            options,
            &dir.0,
            HashMap::from([(elsewhere.clone(), String::new())]),
        )
        .err()
        .unwrap();
        assert_eq!(err, OverlayError::OutsideProject(elsewhere));
    }

    #[test]
    fn a_non_bynk_path_is_rejected() {
        let dir = project("notbynk");
        let options = crate::try_project_options(&dir.0).unwrap();
        let toml = dir.0.join("src/notes.txt");
        let err = apply_overlay(
            options,
            &dir.0,
            HashMap::from([(toml.clone(), String::new())]),
        )
        .err()
        .unwrap();
        assert_eq!(err, OverlayError::NotBynk(toml));
    }

    #[test]
    fn a_malformed_document_is_reported() {
        assert!(matches!(
            parse_overlay("[1]"),
            Err(OverlayError::Malformed(_))
        ));
        assert_eq!(
            parse_overlay(r#"{"/p/a.bynk":"x"}"#).unwrap(),
            HashMap::from([(PathBuf::from("/p/a.bynk"), "x".to_string())])
        );
    }
}
