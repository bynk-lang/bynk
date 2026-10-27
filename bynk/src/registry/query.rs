//! Read-only registry queries for editors and scripts (#2472): `bynk search`
//! and `bynk deps`, and the JSON documents `bynk add`, `bynk fetch` and
//! `bynk publish` print under `--format json`.
//!
//! The registry protocol is a static, per-package index (packaging §6.2): it
//! has no listing or free-text search, so `search` is an exact `org/package`
//! lookup through the same verifying `Client` a build uses.
//! Everything here goes through the driver so that routing, trust, tokens and
//! caching stay in one place (#2472, decision A).

use std::path::Path;

use bynk_project::lock::{parse_registry_source, read_lock};
use bynk_project::manifest::DependencySource;
use serde_json::{Value, json};

use super::publish::Published;
use crate::fetch::Prepared;

/// One version of a package, as `search` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub version: String,
    pub checksum: String,
    pub yanked: bool,
    pub withdrawn: Option<String>,
}

/// What `bynk search <package>` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub package: String,
    pub registry: String,
    /// Oldest first, as the index lists them.
    pub versions: Vec<Version>,
}

impl Found {
    /// The highest version that is neither yanked nor withdrawn (a
    /// pre-release only when nothing else is offered).
    pub fn latest(&self) -> Option<&str> {
        let usable = || {
            self.versions
                .iter()
                .filter(|v| !v.yanked && v.withdrawn.is_none())
                .filter_map(|v| semver::Version::parse(&v.version).ok().map(|s| (s, v)))
        };
        let stable = usable()
            .filter(|(s, _)| s.pre.is_empty())
            .max_by(|a, b| a.0.cmp(&b.0));
        stable
            .or_else(|| usable().max_by(|a, b| a.0.cmp(&b.0)))
            .map(|(_, v)| v.version.as_str())
    }

    pub fn to_json(&self) -> Value {
        json!({
            "package": self.package,
            "registry": self.registry,
            "latest": self.latest(),
            "versions": self.versions.iter().map(|v| json!({
                "version": v.version,
                "checksum": v.checksum,
                "yanked": v.yanked,
                "withdrawn": v.withdrawn,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Look `package` (`org/package`) up on the registry its organisation routes
/// to, from the project at `project`.
pub fn search(project: &Path, package: &str) -> Result<Found, String> {
    if let Some(problem) = bynk_project::manifest::package_name_problem(package) {
        return Err(format!("`{package}` {problem}"));
    }
    let org = package.split_once('/').map_or(package, |(o, _)| o);
    let root = bynk_project::lock::lock_root(project);
    let mut client = super::Client::new(super::commands::registries_of(&root), None);
    let registry = client.route(org);
    let lines = client.lines(&registry, package).map_err(|e| e.message)?;
    Ok(Found {
        package: package.to_string(),
        registry,
        versions: lines
            .into_iter()
            .map(|l| Version {
                version: l.version,
                checksum: l.checksum,
                yanked: l.yanked,
                withdrawn: l.withdrawn,
            })
            .collect(),
    })
}

/// One `[dependencies]` or `[dev-dependencies]` entry with what `bynk.lock`
/// resolved it to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub alias: String,
    pub package: String,
    pub dev: bool,
    /// `registry`, `git` or `path`.
    pub kind: &'static str,
    /// The version range, git revision or path the manifest asks for.
    pub requested: String,
    /// The registry name, git URL or path.
    pub source: String,
    /// The version `bynk.lock` pins, when it records one.
    pub locked: Option<String>,
}

/// The dependencies of the project at `project`, offline: the manifest's
/// entries joined to `bynk.lock`.
pub fn dependencies(project: &Path) -> Result<Vec<Dependency>, String> {
    let doc = crate::fetch::read_manifest(project)
        .ok_or_else(|| "there is no readable `bynk.toml` here".to_string())?;
    let root = bynk_project::lock::lock_root(project);
    let lock = read_lock(&root, &Default::default()).map_err(|e| e.to_string())?;
    let configs = super::commands::registries_of(&root);
    let mut out = Vec::new();
    for (table, dev) in [("dependencies", false), ("dev-dependencies", true)] {
        for spec in crate::fetch::specs(&doc, table) {
            let locked = lock.as_ref().and_then(|l| l.package(&spec.package));
            let (kind, requested, source) = match &spec.source {
                DependencySource::Version(range) => {
                    let org = spec.package.split_once('/').map_or("", |(o, _)| o);
                    let registry = locked
                        .and_then(|p| parse_registry_source(&p.source))
                        .map(str::to_string)
                        .or_else(|| {
                            configs
                                .iter()
                                .find(|c| c.organisations.iter().any(|o| o == org))
                                .map(|c| c.name.clone())
                        })
                        .unwrap_or_else(|| bynk_project::lock::FIRST_PARTY_REGISTRY.to_string());
                    ("registry", range.clone(), registry)
                }
                DependencySource::Git { url, rev } => ("git", rev.clone(), url.clone()),
                DependencySource::Path(p) => ("path", p.clone(), p.clone()),
            };
            out.push(Dependency {
                alias: spec.alias,
                package: spec.package,
                dev,
                kind,
                requested,
                source,
                locked: locked.and_then(|p| p.version.clone()),
            });
        }
    }
    Ok(out)
}

/// The `bynk deps --format json` document.
pub fn dependencies_json(deps: &[Dependency]) -> Value {
    json!({
        "dependencies": deps.iter().map(|d| json!({
            "alias": d.alias,
            "package": d.package,
            "dev": d.dev,
            "kind": d.kind,
            "requested": d.requested,
            "source": d.source,
            "locked": d.locked,
        })).collect::<Vec<_>>(),
    })
}

/// The `bynk add --format json` document: the manifest line written.
pub fn added_json(alias: &str, package: &str, version: &str) -> Value {
    json!({ "added": { "alias": alias, "package": package, "version": version } })
}

/// The `bynk fetch --format json` document.
pub fn fetched_json(done: &Prepared) -> Value {
    json!({
        "wrote_lock": done.wrote_lock,
        "fetched": done.fetched,
        "warnings": done.warnings,
        "pending_npm": done.pending.iter().map(|n| json!({
            "package": n.package,
            "range": n.range,
            "required_by": n.required_by,
        })).collect::<Vec<_>>(),
    })
}

/// The `bynk publish --format json` document (also for `--dry-run`).
pub fn published_json(p: &Published) -> Value {
    json!({
        "package": p.name,
        "version": p.version,
        "checksum": p.checksum,
        "registry": p.registry,
        "dry_run": p.dry_run,
        "files": p.files.iter().map(|(path, bytes)| json!({ "path": path, "bytes": bytes })).collect::<Vec<_>>(),
    })
}
