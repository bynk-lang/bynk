//! Packaging slice 1 (#2207, track `design/tracks/packaging.md` §3, §4, §6.2):
//! the `bynk.toml` tables that give a project a package identity and group
//! projects into a workspace — `[package]`, `[dependencies]`,
//! `[dev-dependencies]`, `[registries]` and `[workspace]`.
//!
//! Everything here is checked and nothing is resolved. `[package]` identity is
//! validated but not yet used, `[registries]` is inert configuration, and a
//! well-formed, non-empty `[dependencies]` is refused until slice 2 can resolve
//! it (decision A). `[workspace]` is the one table with behaviour now: the
//! `bynk` driver runs `check`, `fmt` and `test` across its members
//! ([`crate::workspace`]).
//!
//! The readers take a `toml::Table` and return typed configs, the way
//! `workers_config_from` and `test_targets_from` do; [`crate::check_manifest_str`]
//! calls them, and so refuses what they refuse.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::discovery::read_source;
use crate::paths::{ProjectPathsError, did_you_mean};

/// The longest an organisation, package or registry name segment may be.
pub const MAX_SEGMENT_LEN: usize = 64;

/// The organisation reserved for first-party packages (ADR 0012), and the
/// name `mirror-of` uses for the first-party registry.
pub const RESERVED: &str = "bynk";

/// The entry keys of a `[dependencies]` or `[dev-dependencies]` entry.
pub const DEPENDENCY_KEYS: &[&str] = &["package", "version", "git", "rev", "path"];

/// The entry keys of a `[registries.<name>]` entry.
pub const REGISTRY_KEYS: &[&str] = &["index", "organisations", "mirror-of", "key"];

/// `[package]`: a shareable project's identity (track §3.2, decision D).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageConfig {
    /// The organisation segment of `name` (`acme` in `acme/money`).
    pub org: String,
    /// The package segment of `name` (`money` in `acme/money`).
    pub package: String,
    /// Strict semver, when given. Publishing (slice 4) will require it.
    pub version: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub repository: Option<String>,
    /// The units the package shares, when listed. `None` means every commons
    /// and adapter it declares. Whether each names a commons or adapter the
    /// project declares is `bynk check`'s to say, once the units are known.
    pub exports: Option<Vec<String>>,
}

/// Where a declared dependency comes from: exactly one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencySource {
    /// A registry version range. Only its pin is checked here: validating the
    /// range syntax is slice 2's, together with the resolver that reads it.
    Version(String),
    /// A git repository at a revision.
    Git { url: String, rev: String },
    /// A directory, relative to the project.
    Path(String),
}

/// One `[dependencies]` or `[dev-dependencies]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencySpec {
    /// The entry's key: the name source reaches the package's units by.
    pub alias: String,
    /// The package, as `org/package`.
    pub package: String,
    pub source: DependencySource,
}

/// One `[registries.<name>]` entry (track §6.2, decision E). Inert until
/// slice 4: nothing fetches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryConfig {
    pub name: String,
    pub index: String,
    /// The organisations routed to this registry.
    pub organisations: Vec<String>,
    /// The registry this one mirrors: `bynk`, or another registry's name.
    pub mirror_of: Option<String>,
    /// The pinned origin key, `ed25519:<base64>`.
    pub key: Option<String>,
}

/// `[workspace]` (decision C): the member globs and the paths they skip.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceConfig {
    pub members: Vec<String>,
    pub exclude: Vec<String>,
}

/// Why a name in `bynk.toml` isn't well formed, as the tail of a message.
fn segment_problem(segment: &str, hyphens: bool) -> Option<String> {
    let allowed = if hyphens {
        "`[a-z][a-z0-9-]*`"
    } else {
        "`[a-z][a-z0-9_]*`"
    };
    if segment.is_empty() {
        return Some("is empty".to_string());
    }
    if segment.chars().count() > MAX_SEGMENT_LEN {
        return Some(format!("is longer than {MAX_SEGMENT_LEN} characters"));
    }
    let mut chars = segment.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    let rest_ok = chars.all(|c| {
        c.is_ascii_lowercase() || c.is_ascii_digit() || if hyphens { c == '-' } else { c == '_' }
    });
    if first_ok && rest_ok {
        return None;
    }
    if !hyphens && segment.contains('-') {
        let fixed = segment.replace('-', "_");
        if segment_problem(&fixed, false).is_none() {
            return Some(format!(
                "can't contain `-`, because it is also a name in Bynk source, where `-` is minus — did you mean `{fixed}`?"
            ));
        }
    }
    if hyphens && segment.contains('_') {
        let fixed = segment.replace('_', "-");
        if segment_problem(&fixed, true).is_none() {
            return Some(format!("can't contain `_` — did you mean `{fixed}`?"));
        }
    }
    Some(format!("must match {allowed}"))
}

/// Whether `word` is a reserved Bynk keyword, which no name source can write
/// may be.
fn is_keyword(word: &str) -> bool {
    bynk_syntax::keywords::KEYWORDS
        .iter()
        .any(|k| k.word == word)
}

/// The problem with an organisation segment (decision G: `[a-z][a-z0-9-]*`).
pub fn org_problem(org: &str) -> Option<String> {
    segment_problem(org, true)
}

/// The problem with a package segment or an alias (decision G:
/// `[a-z][a-z0-9_]*`, not a keyword): each is written in source as the first
/// segment of `<alias>.<unit>`.
pub fn identifier_problem(name: &str) -> Option<String> {
    if let Some(problem) = segment_problem(name, false) {
        return Some(problem);
    }
    is_keyword(name).then(|| "is a Bynk keyword".to_string())
}

/// The problem with a registry name (decision G: `[a-z][a-z0-9-]*`, not
/// `bynk`).
pub fn registry_name_problem(name: &str) -> Option<String> {
    if name == RESERVED {
        return Some(
            "is reserved: `mirror-of = \"bynk\"` already means the first-party registry"
                .to_string(),
        );
    }
    segment_problem(name, true)
}

/// The environment variable a registry's token is read from (track §6.1):
/// `BYNK_REGISTRY_<NAME>_TOKEN`, the name uppercased with `-` mapped to `_`.
/// A registry name holds no `_`, so two names never share a variable.
pub fn registry_token_var(name: &str) -> String {
    format!(
        "BYNK_REGISTRY_{}_TOKEN",
        name.to_ascii_uppercase().replace('-', "_")
    )
}

/// The problem with an `org/package` name, as `(segment, problem)`.
pub fn package_name_problem(name: &str) -> Option<String> {
    let Some((org, package)) = name.split_once('/') else {
        return Some(format!(
            "`{name}` must be `organisation/package`, like `acme/money`"
        ));
    };
    if package.contains('/') {
        return Some(format!(
            "`{name}` must be `organisation/package`, with one `/`"
        ));
    }
    if let Some(problem) = org_problem(org) {
        return Some(format!("names the organisation `{org}`, which {problem}"));
    }
    if let Some(problem) = identifier_problem(package) {
        return Some(format!("names the package `{package}`, which {problem}"));
    }
    None
}

/// Whether `version` is strict semver 2.0: `MAJOR.MINOR.PATCH`, no leading
/// zeros, with an optional `-prerelease` and `+build`.
pub fn is_semver(version: &str) -> bool {
    let (rest, build) = match version.split_once('+') {
        Some((rest, build)) => (rest, Some(build)),
        None => (version, None),
    };
    let (core, pre) = match rest.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (rest, None),
    };
    let numeric = |s: &str| {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
    };
    let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 || !parts.iter().all(|p| numeric(p)) {
        return false;
    }
    if let Some(pre) = pre
        && !pre
            .split('.')
            .all(|id| ident(id) && (!id.chars().all(|c| c.is_ascii_digit()) || numeric(id)))
    {
        return false;
    }
    if let Some(build) = build
        && !build.split('.').all(ident)
    {
        return false;
    }
    true
}

/// Whether `name` is shaped like a unit name: dot-separated identifiers.
fn is_unit_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|seg| {
            let mut chars = seg.chars();
            chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// The project's `[package]` as both the build and the editor see it, the
/// way [`crate::read_unstable_config`] reads `[unstable]`: the overlay's
/// `bynk.toml` when it holds one, otherwise the one on disk. `None` when there
/// is no manifest, no `[package]`, or one that isn't well formed: its shape
/// errors are [`crate::check_manifest`]'s to report.
pub fn read_package_config(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Option<PackageConfig> {
    let manifest = project_root.join("bynk.toml");
    let content = match read_source(&manifest, overlay) {
        Ok(text) => text,
        Err(_) => std::fs::read_to_string(&manifest).ok()?,
    };
    let doc = content.parse::<toml::Table>().ok()?;
    package_config_from(doc.get("package")?.as_table()?).ok()
}

/// A list of strings, or `None` when `value` is anything else.
fn string_list(value: &toml::Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// `[package]`'s typed content (decision D). An unknown key is
/// [`crate::check_manifest_str`]'s to refuse.
pub fn package_config_from(table: &toml::Table) -> Result<PackageConfig, ProjectPathsError> {
    let bad = |key: &'static str, reason: String| ProjectPathsError::BadPackage { key, reason };
    let text = |key: &'static str| -> Result<Option<String>, ProjectPathsError> {
        match table.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_str()
                .map(|s| Some(s.to_owned()))
                .ok_or_else(|| bad(key, "must be a string".to_string())),
        }
    };
    let Some(name) = text("name")? else {
        return Err(bad(
            "name",
            "is required: a package is named `organisation/package`, like `name = \"acme/money\"`"
                .to_string(),
        ));
    };
    if let Some(problem) = package_name_problem(&name) {
        return Err(bad("name", problem));
    }
    let (org, package) = name.split_once('/').expect("checked above");
    if org == RESERVED {
        return Err(bad(
            "name",
            format!(
                "names the organisation `{RESERVED}`, which is reserved for first-party packages"
            ),
        ));
    }
    let version = text("version")?;
    if let Some(v) = &version
        && !is_semver(v)
    {
        return Err(bad(
            "version",
            format!("`{v}` isn't a semver version, like `1.4.0`"),
        ));
    }
    let exports = match table.get("exports") {
        None => None,
        Some(v) => {
            let list = string_list(v).ok_or_else(|| {
                bad(
                    "exports",
                    "must be a list of unit names, like `exports = [\"money\"]`".to_string(),
                )
            })?;
            if let Some(entry) = list.iter().find(|e| !is_unit_name(e)) {
                return Err(bad(
                    "exports",
                    format!("lists `{entry}`, which isn't a unit name"),
                ));
            }
            Some(list)
        }
    };
    Ok(PackageConfig {
        org: org.to_owned(),
        package: package.to_owned(),
        version,
        description: text("description")?,
        license: text("license")?,
        repository: text("repository")?,
        exports,
    })
}

/// A `[dependencies]` or `[dev-dependencies]` table's entries, each checked
/// for shape (decision A). `table` is the table's name, for messages.
pub fn dependencies_from(
    table: &'static str,
    entries: &toml::Table,
) -> Result<Vec<DependencySpec>, ProjectPathsError> {
    let mut out = Vec::new();
    for (alias, entry) in entries {
        let bad = |reason: String| ProjectPathsError::BadDependency {
            table,
            alias: alias.clone(),
            reason,
        };
        if let Some(problem) = identifier_problem(alias) {
            return Err(bad(format!(
                "is an alias, written in source as `{alias}.<unit>`, so it {problem}"
            )));
        }
        if alias == RESERVED {
            return Err(bad(format!(
                "can't be an alias: `{RESERVED}` is reserved for the standard library"
            )));
        }
        let Some(fields) = entry.as_table() else {
            return Err(bad(
                "must be a table, like `{ package = \"acme/money\", version = \"^1\" }` — there is no bare-version shorthand"
                    .to_string(),
            ));
        };
        check_entry_keys(table, alias, fields, DEPENDENCY_KEYS)?;
        let text = |key: &str| -> Result<Option<String>, ProjectPathsError> {
            match fields.get(key) {
                None => Ok(None),
                Some(v) => match v.as_str() {
                    Some(s) if !s.trim().is_empty() => Ok(Some(s.to_owned())),
                    _ => Err(bad(format!("`{key}` must be a non-empty string"))),
                },
            }
        };
        let Some(package) = text("package")? else {
            return Err(bad(
                "needs `package`, the `organisation/package` it names".to_string()
            ));
        };
        if let Some(problem) = package_name_problem(&package) {
            return Err(bad(format!("`package` {problem}")));
        }
        let (version, git, rev, path) =
            (text("version")?, text("git")?, text("rev")?, text("path")?);
        let given: Vec<&str> = [("version", &version), ("git", &git), ("path", &path)]
            .iter()
            .filter(|(_, v)| v.is_some())
            .map(|(k, _)| *k)
            .collect();
        if given.len() != 1 {
            let found = if given.is_empty() {
                "none".to_string()
            } else {
                given
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(" and ")
            };
            return Err(bad(format!(
                "must give exactly one of `version`, `git` or `path` (it gives {found})"
            )));
        }
        if rev.is_some() && git.is_none() {
            return Err(bad("has a `rev`, which only goes with `git`".to_string()));
        }
        let source = if let Some(v) = version {
            if crate::paths::is_unpinned_range(&v) {
                return Err(bad(format!(
                    "`version = \"{v}\"` is unpinned — name a version, like `\"^1.4\"`"
                )));
            }
            DependencySource::Version(v)
        } else if let Some(url) = git {
            let Some(rev) = rev else {
                return Err(bad(
                    "has a `git` source with no `rev` — pin a tag or commit, like `rev = \"v0.3.0\"`"
                        .to_string(),
                ));
            };
            DependencySource::Git { url, rev }
        } else {
            DependencySource::Path(path.expect("exactly one source"))
        };
        out.push(DependencySpec {
            alias: alias.clone(),
            package,
            source,
        });
    }
    Ok(out)
}

/// Refuse a map-table entry key that isn't one of `keys`, with did-you-mean.
fn check_entry_keys(
    table: &'static str,
    entry: &str,
    fields: &toml::Table,
    keys: &'static [&'static str],
) -> Result<(), ProjectPathsError> {
    match fields.keys().find(|k| !keys.contains(&k.as_str())) {
        Some(key) => Err(ProjectPathsError::UnknownEntryKey {
            table,
            entry: entry.to_owned(),
            key: key.clone(),
        }),
        None => Ok(()),
    }
}

/// Whether an `index` URL is acceptable: `https://`, or `http://` to the
/// local machine only (decision E).
fn index_problem(index: &str) -> Option<String> {
    let Some((scheme, rest)) = index.split_once("://") else {
        return Some(format!("`index = \"{index}\"` must be an `https://` URL"));
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let bare_host = if host.starts_with('[') {
        host.split_once(']')
            .map_or(host, |(h, _)| h)
            .trim_start_matches('[')
    } else {
        host.split(':').next().unwrap_or_default()
    };
    if bare_host.is_empty() {
        return Some(format!("`index = \"{index}\"` names no host"));
    }
    match scheme {
        "https" => None,
        "http" if bare_host == "localhost" => None,
        "http" => Some(format!(
            "`index = \"{index}\"` must be `https://` — `http://` is allowed only for `localhost`"
        )),
        _ => Some(format!("`index = \"{index}\"` must be an `https://` URL")),
    }
}

/// Whether `key` is `ed25519:` and the base64 of a 32-byte key.
fn is_ed25519_key(key: &str) -> bool {
    let Some(b64) = key.strip_prefix("ed25519:") else {
        return false;
    };
    let body = b64.trim_end_matches('=');
    let pad = b64.len() - body.len();
    b64.len() % 4 == 0
        && pad <= 2
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
        && b64.len() / 4 * 3 - pad == 32
}

/// `[registries]`'s entries, each checked (decision E), and the rules across
/// them: an organisation is routed by one registry at most, and a mirror
/// mirrors `bynk` or a registry that isn't itself a mirror.
pub fn registries_from(entries: &toml::Table) -> Result<Vec<RegistryConfig>, ProjectPathsError> {
    let mut out: Vec<RegistryConfig> = Vec::new();
    for (name, entry) in entries {
        let bad = |reason: String| ProjectPathsError::BadRegistry {
            name: name.clone(),
            reason,
        };
        if let Some(problem) = registry_name_problem(name) {
            return Err(bad(format!("is a registry name, which {problem}")));
        }
        let Some(fields) = entry.as_table() else {
            return Err(bad(
                "must be a table, like `[registries.acme-internal]` with an `index`".to_string(),
            ));
        };
        // Credentials first: a `token` is never a typo for some other key.
        if fields.contains_key("token") {
            return Err(ProjectPathsError::RegistryCredential { name: name.clone() });
        }
        check_entry_keys("registries", name, fields, REGISTRY_KEYS)?;
        let index = match fields.get("index") {
            None => return Err(bad("needs an `index` URL".to_string())),
            Some(v) => v
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| bad("`index` must be a URL string".to_string()))?,
        };
        if let Some(problem) = index_problem(&index) {
            return Err(bad(problem));
        }
        let organisations = match fields.get("organisations") {
            None => Vec::new(),
            Some(v) => string_list(v).ok_or_else(|| {
                bad("`organisations` must be a list of organisation names".to_string())
            })?,
        };
        for org in &organisations {
            if org == RESERVED {
                return Err(bad(format!(
                    "lists the organisation `{RESERVED}`, which always resolves on the first-party registry"
                )));
            }
            if let Some(problem) = org_problem(org) {
                return Err(bad(format!(
                    "lists the organisation `{org}`, which {problem}"
                )));
            }
        }
        let mirror_of = match fields.get("mirror-of") {
            None => None,
            Some(v) => Some(
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| bad("`mirror-of` must be a registry name".to_string()))?,
            ),
        };
        if mirror_of.is_some() && fields.contains_key("organisations") {
            return Err(bad(
                "has both `mirror-of` and `organisations` — a mirror serves another registry's organisations and routes none of its own"
                    .to_string(),
            ));
        }
        let key = match fields.get("key") {
            None => None,
            Some(v) => match v.as_str() {
                Some(k) if is_ed25519_key(k) => Some(k.to_owned()),
                _ => {
                    return Err(bad(
                        "`key` must be `ed25519:` followed by the base64 of a 32-byte public key"
                            .to_string(),
                    ));
                }
            },
        };
        out.push(RegistryConfig {
            name: name.clone(),
            index,
            organisations,
            mirror_of,
            key,
        });
    }
    let mut routed: BTreeMap<&str, &str> = BTreeMap::new();
    for registry in &out {
        for org in &registry.organisations {
            if let Some(first) = routed.insert(org, &registry.name) {
                return Err(ProjectPathsError::BadRegistry {
                    name: registry.name.clone(),
                    reason: format!(
                        "lists the organisation `{org}`, which `[registries.{first}]` already lists — an organisation resolves on one registry"
                    ),
                });
            }
        }
    }
    for registry in &out {
        let Some(target) = &registry.mirror_of else {
            continue;
        };
        if target == RESERVED {
            continue;
        }
        let bad = |reason: String| ProjectPathsError::BadRegistry {
            name: registry.name.clone(),
            reason,
        };
        if target == &registry.name {
            return Err(bad("can't be a mirror of itself".to_string()));
        }
        match out.iter().find(|r| &r.name == target) {
            None => {
                let names = out.iter().map(|r| r.name.as_str()).chain([RESERVED]);
                let hint = match did_you_mean(target, names) {
                    Some(n) => format!(" — did you mean `{n}`?"),
                    None => format!(" — it must be `{RESERVED}` or a registry declared here"),
                };
                return Err(bad(format!(
                    "has `mirror-of = \"{target}\"`, which names no registry{hint}"
                )));
            }
            Some(other) if other.mirror_of.is_some() => {
                return Err(bad(format!(
                    "mirrors `{target}`, which is itself a mirror — a mirror names `{RESERVED}` or a registry that isn't one"
                )));
            }
            Some(_) => {}
        }
    }
    Ok(out)
}

/// Whether a workspace `members`/`exclude` entry names a directory under the
/// root: a relative path with no `..`, no root or drive anchor (`/packages`
/// on Windows has no prefix, so `is_relative` alone lets it through), and at
/// least one named component — `.` names the root itself, which is never a
/// member, and an `exclude = ["."]` would otherwise exclude every member.
fn is_under_root(pattern: &str) -> bool {
    use std::path::Component;
    let path = std::path::Path::new(pattern);
    path.is_relative()
        && path.components().all(|c| {
            !matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
        && path.components().any(|c| matches!(c, Component::Normal(_)))
}

/// `[workspace]`'s typed content (decision C). An unknown key is
/// [`crate::check_manifest_str`]'s to refuse; whether the globs match any
/// member is [`crate::workspace::load_workspace`]'s.
pub fn workspace_config_from(table: &toml::Table) -> Result<WorkspaceConfig, ProjectPathsError> {
    let bad = |reason: String| ProjectPathsError::BadWorkspace { reason };
    let list = |key: &str| -> Result<Vec<String>, ProjectPathsError> {
        match table.get(key) {
            None => Ok(Vec::new()),
            Some(v) => string_list(v).ok_or_else(|| {
                bad(format!(
                    "`{key}` must be a list of paths, like `{key} = [\"packages/*\"]`"
                ))
            }),
        }
    };
    if !table.contains_key("members") {
        return Err(bad(
            "needs `members`, the member directories, like `members = [\"packages/*\"]`"
                .to_string(),
        ));
    }
    let members = list("members")?;
    let exclude = list("exclude")?;
    for (key, entries) in [("members", &members), ("exclude", &exclude)] {
        if let Some(p) = entries.iter().find(|p| !is_under_root(p)) {
            return Err(bad(format!(
                "`{key}` lists `{p}`, which isn't a relative path under the workspace root"
            )));
        }
        if let Some(p) = entries.iter().find(|p| p.contains("**")) {
            return Err(bad(format!(
                "`{key}` lists `{p}` — a pattern matches one directory level per `*`; `**` isn't supported"
            )));
        }
    }
    Ok(WorkspaceConfig { members, exclude })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_is_strict() {
        for ok in [
            "0.0.0",
            "1.4.0",
            "10.20.30",
            "1.0.0-alpha.1",
            "1.0.0+build.5",
            "1.0.0-rc.1+b",
        ] {
            assert!(is_semver(ok), "{ok}");
        }
        for bad in [
            "1", "1.4", "01.0.0", "1.0.0-", "1.0.0-01", "v1.0.0", "1.0.0.0", "1.0.0+",
        ] {
            assert!(!is_semver(bad), "{bad}");
        }
    }

    #[test]
    fn the_token_variable_is_injective_over_registry_names() {
        assert_eq!(
            registry_token_var("acme-internal"),
            "BYNK_REGISTRY_ACME_INTERNAL_TOKEN"
        );
        // `acme_internal` is not a registry name, so it can't collide.
        assert!(registry_name_problem("acme_internal").is_some());
    }

    #[test]
    fn segments_follow_decision_g() {
        assert!(org_problem("acme-labs").is_none());
        assert!(identifier_problem("ledger_kit").is_none());
        assert!(
            identifier_problem("ledger-kit")
                .unwrap()
                .contains("did you mean `ledger_kit`?")
        );
        assert!(identifier_problem("context").unwrap().contains("keyword"));
        assert!(org_problem("Acme").is_some());
        assert!(org_problem(&"a".repeat(65)).unwrap().contains("64"));
        assert!(
            org_problem("acme_labs")
                .unwrap()
                .contains("did you mean `acme-labs`?")
        );
    }

    #[test]
    fn an_ed25519_key_is_32_bytes_of_base64() {
        let key = format!("ed25519:{}=", "A".repeat(43));
        assert!(is_ed25519_key(&key));
        assert!(!is_ed25519_key("ed25519:AAAA"));
        assert!(!is_ed25519_key(&key.replace("ed25519", "rsa")));
    }
}
