//! Packaging slice 1 (#2207, decision C): workspaces.
//!
//! A workspace root is a directory whose `bynk.toml` holds `[workspace]` (and
//! may hold `[registries]`) and nothing else: it is never a project itself, the
//! Cargo "virtual manifest". Its members are the directories `members` matches,
//! minus those `exclude` matches, each holding its own `bynk.toml`.
//!
//! A pattern is a relative path whose components may use `*` (any run of
//! characters) and `?` (one character); each `*` matches within one directory
//! level. A hidden directory is matched only by a component that itself starts
//! with `.`.
//!
//! A project finds the workspace it belongs to by walking up for a `[workspace]`
//! whose members include it ([`find_parent_workspace`]). A parent `bynk.toml`
//! that doesn't parse, has an unknown table, or doesn't list the project counts
//! as absent, so one broken unrelated manifest can't break every project nested
//! under it.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::manifest::{WorkspaceConfig, workspace_config_from};
use crate::paths::{ProjectPathsError, check_manifest_str};

/// A loaded workspace root and its members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The root directory, as the caller named it.
    pub root: PathBuf,
    pub config: WorkspaceConfig,
    /// Each member's directory relative to `root`, sorted, so every command
    /// runs over them in the same order.
    pub members: Vec<PathBuf>,
}

/// Whether `dir`'s `bynk.toml` parses and holds a `[workspace]` table. A
/// manifest that doesn't parse isn't a workspace root: the command that reads
/// it next reports it as malformed.
pub fn is_workspace_root(dir: &Path) -> bool {
    fs::read_to_string(dir.join("bynk.toml"))
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .is_some_and(|doc| doc.contains_key("workspace"))
}

/// Load the workspace rooted at `root`: check its manifest, refuse sources at
/// the root, and resolve its members. A workspace whose patterns match no
/// member is an error, as an unknown table is (#1665).
pub fn load_workspace(root: &Path) -> Result<Workspace, ProjectPathsError> {
    let text =
        fs::read_to_string(root.join("bynk.toml")).map_err(|_| ProjectPathsError::Malformed)?;
    check_manifest_str(&text)?;
    let doc = text
        .parse::<toml::Table>()
        .map_err(|_| ProjectPathsError::Malformed)?;
    let Some(table) = doc.get("workspace").and_then(|v| v.as_table()) else {
        return Err(ProjectPathsError::BadWorkspace {
            reason: "is missing".to_string(),
        });
    };
    let config = workspace_config_from(table)?;
    if root.join("src").is_dir() || has_bynk_file(root) {
        return Err(ProjectPathsError::WorkspaceRootSources);
    }
    let members = expand_members(root, &config)?;
    if members.is_empty() {
        return Err(ProjectPathsError::BadWorkspace {
            reason: format!(
                "matches no member: no directory under the root matches {}",
                quoted_list(&config.members)
            ),
        });
    }
    Ok(Workspace {
        root: root.to_path_buf(),
        config,
        members,
    })
}

/// The workspace root above `project_root` whose members include it, if any.
pub fn find_parent_workspace(project_root: &Path) -> Option<PathBuf> {
    let me = project_root.canonicalize().ok()?;
    let mut dir = me.parent();
    while let Some(candidate) = dir {
        if lists(candidate, &me) {
            return Some(candidate.to_path_buf());
        }
        dir = candidate.parent();
    }
    None
}

/// Whether `root`'s manifest is a well-formed workspace that lists `member`
/// (canonical). Anything short of that is "not listed".
fn lists(root: &Path, member: &Path) -> bool {
    let Ok(text) = fs::read_to_string(root.join("bynk.toml")) else {
        return false;
    };
    if check_manifest_str(&text).is_err() {
        return false;
    }
    let Some(config) = text
        .parse::<toml::Table>()
        .ok()
        .and_then(|doc| doc.get("workspace")?.as_table().cloned())
        .and_then(|t| workspace_config_from(&t).ok())
    else {
        return false;
    };
    let Ok(members) = expand_members(root, &config) else {
        return false;
    };
    members
        .iter()
        .any(|m| root.join(m).canonicalize().is_ok_and(|c| c == member))
}

/// The members `config` resolves to under `root`: every directory a `members`
/// pattern matches that no `exclude` pattern matches, relative and sorted. A
/// literal entry that doesn't exist, or a member without a `bynk.toml`, is an
/// error; a wildcard entry that matches nothing contributes nothing.
fn expand_members(
    root: &Path,
    config: &WorkspaceConfig,
) -> Result<Vec<PathBuf>, ProjectPathsError> {
    let bad = |reason: String| ProjectPathsError::BadWorkspace { reason };
    let mut found: BTreeSet<PathBuf> = BTreeSet::new();
    for pattern in &config.members {
        let parts = components(pattern);
        let matches = matching_dirs(root, &parts);
        if matches.is_empty() && !parts.iter().any(|p| is_wildcard(p)) {
            return Err(bad(format!(
                "`members` lists `{pattern}`, which isn't a directory under the workspace root"
            )));
        }
        found.extend(matches);
    }
    let mut members = Vec::new();
    for rel in found {
        let excluded = config
            .exclude
            .iter()
            .any(|ex| excludes(&components(ex), &rel));
        if excluded {
            continue;
        }
        let shown = rel.to_string_lossy().replace('\\', "/");
        if rel.as_os_str().is_empty() {
            return Err(bad(
                "`members` matches the workspace root itself, which is never a member".to_string(),
            ));
        }
        if !root.join(&rel).join("bynk.toml").is_file() {
            return Err(bad(format!(
                "member `{shown}` has no `bynk.toml` — add one, or `exclude` it"
            )));
        }
        members.push(rel);
    }
    Ok(members)
}

/// A pattern's normal components (`./` and empty segments dropped).
fn components(pattern: &str) -> Vec<String> {
    Path::new(pattern)
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn is_wildcard(part: &str) -> bool {
    part.contains(['*', '?'])
}

/// The directories under `root` that `parts` match, relative to `root`.
fn matching_dirs(root: &Path, parts: &[String]) -> Vec<PathBuf> {
    let mut current = vec![PathBuf::new()];
    for part in parts {
        let mut next = Vec::new();
        for base in &current {
            if is_wildcard(part) {
                let Ok(entries) = fs::read_dir(root.join(base)) else {
                    continue;
                };
                let mut names: Vec<String> = entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| part.starts_with('.') || !n.starts_with('.'))
                    .filter(|n| wildcard_match(part, n))
                    .collect();
                names.sort();
                next.extend(names.into_iter().map(|n| base.join(n)));
            } else if root.join(base).join(part).is_dir() {
                next.push(base.join(part));
            }
        }
        current = next;
    }
    current
}

/// Whether the exclude pattern `parts` matches `member` or a directory above it.
fn excludes(parts: &[String], member: &Path) -> bool {
    let names: Vec<String> = member
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.len() <= names.len() && parts.iter().zip(&names).all(|(p, n)| wildcard_match(p, n))
}

/// `*` and `?` matching over one path component.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Whether `dir` holds a `.bynk` file directly.
fn has_bynk_file(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|e| e.path().extension().is_some_and(|x| x == "bynk") && e.path().is_file())
    })
}

fn quoted_list(items: &[String]) -> String {
    if items.is_empty() {
        return "nothing (`members` is empty)".to_string();
    }
    items
        .iter()
        .map(|i| format!("`{i}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_match_within_a_component() {
        assert!(wildcard_match("*", "money"));
        assert!(wildcard_match("m*y", "money"));
        assert!(wildcard_match("mon?y", "money"));
        assert!(!wildcard_match("m*x", "money"));
        assert!(wildcard_match("money", "money"));
        assert!(!wildcard_match("mone", "money"));
    }

    #[test]
    fn an_exclude_covers_a_member_and_everything_under_it() {
        let ex = components("packages/scratch");
        assert!(excludes(&ex, Path::new("packages/scratch")));
        assert!(excludes(&ex, Path::new("packages/scratch/inner")));
        assert!(!excludes(&ex, Path::new("packages/money")));
        assert!(excludes(
            &components("packages/s*"),
            Path::new("packages/scratch")
        ));
    }
}
