//! #3562 (slice G1): the Worker groups of `bynk.toml`.
//!
//! Under the `workers` target a context is one Worker. A group puts several
//! contexts in one Worker, so a handful of contexts that belong together need
//! not each cost a Worker and a network hop:
//!
//! ```toml
//! [deploy.workers.core]
//! contexts = ["shop.orders", "shop.catalogue", "shop.pricing"]
//!
//! [deploy.workers.payments]
//! contexts = ["shop.payment"]
//! name = "acme-payments"          # the Cloudflare script name; default is the key
//! routes = ["pay.example.com/*"]
//! ```
//!
//! The **key** is the group's identity (its ledger entry, its `--worker`
//! selector, its `workers/<key>/` directory); `name` only overrides the
//! script's name on Cloudflare. A context named in no group stays one Worker
//! under its dashed name, so a project with no `[deploy.workers]` is unchanged.
//!
//! This module reads the table and checks everything that needs no project
//! model: shapes, the grammar of names, a context in two groups, two groups
//! sharing a script name, a route or domain claimed twice. What needs the
//! project's contexts (an unknown context, a context that is not deployable,
//! a key that collides with an implicit Worker's name) is checked against them
//! by [`validate_against`].

use crate::discovery::read_source;
use crate::paths::{ProjectPathsError, did_you_mean};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// One group's `[deploy.workers.<key>]` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerGroup {
    /// The members, as qualified context names (`shop.orders`), in the order
    /// the manifest lists them.
    pub contexts: Vec<String>,
    /// The Cloudflare script name; `None` means the group's key.
    pub name: Option<String>,
    /// Route patterns (`pay.example.com/*`) the Worker is attached to.
    pub routes: Vec<String>,
    /// Custom domains (`pay.example.com`) the Worker is attached to.
    pub custom_domains: Vec<String>,
}

impl WorkerGroup {
    /// The script's name on Cloudflare, given the group's key.
    pub fn script_name<'a>(&'a self, key: &'a str) -> &'a str {
        self.name.as_deref().unwrap_or(key)
    }
}

/// The `[deploy.workers]` groups, by key.
pub type WorkerGroups = BTreeMap<String, WorkerGroup>;

/// The keys of a `[deploy.workers.<key>]` entry.
pub const GROUP_KEYS: &[&str] = &["contexts", "name", "routes", "custom_domains"];

fn bad(key: &str, reason: String) -> ProjectPathsError {
    ProjectPathsError::BadWorkerGroup {
        key: key.to_string(),
        reason,
    }
}

/// Whether `name` can be a Worker's key or script name: Cloudflare's own
/// grammar, 1 to 63 lowercase letters, digits and `-`, not starting or ending
/// with one. It becomes a directory, a command-line argument and a ledger key,
/// so nothing else may reach them.
pub fn is_worker_name(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// Whether `name` is a qualified context name: dot-separated segments, each an
/// identifier (letters, digits and `_`, not starting with a digit).
pub fn is_qualified_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|seg| {
            let mut bytes = seg.bytes();
            bytes
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
}

/// Whether `host` is a hostname a custom domain can name: dot-separated
/// labels of letters, digits and `-`, at least two of them, no wildcard.
fn is_hostname(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').count() >= 2
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// Whether `route` is a route pattern: a host (a leading `*.` allowed), then
/// `/` and a path, with no scheme and no whitespace, as Cloudflare takes them.
fn is_route_pattern(route: &str) -> bool {
    let Some((host, _)) = route.split_once('/') else {
        return false;
    };
    let host = host.strip_prefix("*.").unwrap_or(host);
    is_hostname(host) && !route.bytes().any(|b| b.is_ascii_whitespace())
}

fn string_list(
    key: &str,
    field: &str,
    value: &toml::Value,
) -> Result<Vec<String>, ProjectPathsError> {
    let shape = || {
        bad(
            key,
            format!("`{field}` must be a list of strings, like `{field} = [\"…\"]`"),
        )
    };
    let list = value.as_array().ok_or_else(shape)?;
    list.iter()
        .map(|v| v.as_str().map(str::to_string).ok_or_else(shape))
        .collect()
}

/// A name that appears twice in `items`, if any.
fn first_duplicate(items: &[String]) -> Option<&str> {
    let mut seen = BTreeSet::new();
    items
        .iter()
        .find(|item| !seen.insert(item.as_str()))
        .map(String::as_str)
}

/// One `[deploy.workers.<key>]` entry.
fn group_from(key: &str, value: &toml::Value) -> Result<WorkerGroup, ProjectPathsError> {
    let entry = value
        .as_table()
        .ok_or_else(|| bad(key, "must be a table".to_string()))?;
    if !is_worker_name(key) {
        return Err(bad(
            key,
            "is not a valid Worker name: 1 to 63 lowercase letters, digits or `-`, not starting or ending with `-`"
                .to_string(),
        ));
    }
    for k in entry.keys() {
        if !GROUP_KEYS.contains(&k.as_str()) {
            let hint = did_you_mean(k, GROUP_KEYS.iter().copied())
                .map(|h| format!(" — did you mean `{h}`?"))
                .unwrap_or_default();
            return Err(bad(
                key,
                format!(
                    "has no key named `{k}`{hint} (its keys are `contexts`, `name`, `routes` and `custom_domains`)"
                ),
            ));
        }
    }
    let Some(contexts) = entry.get("contexts") else {
        return Err(bad(
            key,
            "needs `contexts`, like `contexts = [\"shop.orders\", \"shop.pricing\"]`".to_string(),
        ));
    };
    let contexts = string_list(key, "contexts", contexts)?;
    if contexts.is_empty() {
        return Err(bad(
            key,
            "bynk.deploy.empty_group: `contexts` names no context".to_string(),
        ));
    }
    if let Some(bad_name) = contexts.iter().find(|c| !is_qualified_name(c)) {
        return Err(bad(
            key,
            format!(
                "`contexts` holds `{bad_name}`, which is not a qualified context name like `shop.orders` (globs are not supported)"
            ),
        ));
    }
    if let Some(dup) = first_duplicate(&contexts) {
        return Err(bad(key, format!("lists context `{dup}` twice")));
    }
    let name = match entry.get("name") {
        None => None,
        Some(v) => {
            let n = v.as_str().filter(|n| is_worker_name(n)).ok_or_else(|| {
                bad(
                    key,
                    "`name` must be a Worker name: 1 to 63 lowercase letters, digits or `-`, not starting or ending with `-`"
                        .to_string(),
                )
            })?;
            Some(n.to_string())
        }
    };
    let routes = match entry.get("routes") {
        None => Vec::new(),
        Some(v) => string_list(key, "routes", v)?,
    };
    if let Some(r) = routes.iter().find(|r| !is_route_pattern(r)) {
        return Err(bad(
            key,
            format!(
                "`routes` holds `{r}`, which is not a route pattern like `pay.example.com/*` (a host, then `/` and a path, with no scheme)"
            ),
        ));
    }
    let custom_domains = match entry.get("custom_domains") {
        None => Vec::new(),
        Some(v) => string_list(key, "custom_domains", v)?,
    };
    if let Some(d) = custom_domains.iter().find(|d| !is_hostname(d)) {
        return Err(bad(
            key,
            format!(
                "`custom_domains` holds `{d}`, which is not a hostname like `pay.example.com` (no scheme, path or wildcard)"
            ),
        ));
    }
    Ok(WorkerGroup {
        contexts,
        name,
        routes,
        custom_domains,
    })
}

/// The rules across groups: a context in one group only, a script name used
/// once, and a route or custom domain claimed once.
fn check_across(groups: &WorkerGroups) -> Result<(), ProjectPathsError> {
    let mut context_owner: BTreeMap<&str, &str> = BTreeMap::new();
    let mut script_owner: BTreeMap<&str, &str> = BTreeMap::new();
    let mut route_owner: BTreeMap<&str, &str> = BTreeMap::new();
    let mut domain_owner: BTreeMap<&str, &str> = BTreeMap::new();
    for (key, group) in groups {
        for context in &group.contexts {
            if let Some(first) = context_owner.insert(context, key) {
                return Err(bad(
                    key,
                    format!(
                        "bynk.deploy.context_in_two_groups: context `{context}` is already in `[deploy.workers.{first}]`; a context is in one Worker only"
                    ),
                ));
            }
        }
        let script = group.script_name(key);
        if let Some(first) = script_owner.insert(script, key) {
            return Err(bad(
                key,
                format!(
                    "bynk.deploy.group_name_conflict: its Worker would be named `{script}`, as `[deploy.workers.{first}]`'s is"
                ),
            ));
        }
        for route in &group.routes {
            if let Some(first) = route_owner.insert(route, key) {
                return Err(bad(
                    key,
                    format!(
                        "bynk.deploy.duplicate_hostname: route `{route}` is also in `[deploy.workers.{first}]`"
                    ),
                ));
            }
        }
        for domain in &group.custom_domains {
            if let Some(first) = domain_owner.insert(domain, key) {
                return Err(bad(
                    key,
                    format!(
                        "bynk.deploy.duplicate_hostname: custom domain `{domain}` is also in `[deploy.workers.{first}]`"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// The `[deploy]` table's `workers` entries.
pub fn deploy_workers_from(deploy: &toml::Table) -> Result<WorkerGroups, ProjectPathsError> {
    let mut out = WorkerGroups::new();
    let Some(value) = deploy.get("workers") else {
        return Ok(out);
    };
    let all = value.as_table().ok_or_else(|| {
        bad(
            "…",
            "`[deploy.workers]` must be a table of Workers, like `[deploy.workers.core]`"
                .to_string(),
        )
    })?;
    for (key, value) in all {
        out.insert(key.clone(), group_from(key, value)?);
    }
    check_across(&out)?;
    Ok(out)
}

/// Read `bynk.toml`'s `[deploy.workers]` groups, honouring `overlay`. No
/// manifest, or none of the table, is empty.
pub fn try_read_worker_groups_with(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<WorkerGroups, ProjectPathsError> {
    let Ok(content) = read_source(&project_root.join("bynk.toml"), overlay) else {
        return Ok(WorkerGroups::new());
    };
    let doc = content
        .parse::<toml::Table>()
        .map_err(|_| ProjectPathsError::Malformed)?;
    match doc.get("deploy").and_then(|v| v.as_table()) {
        Some(t) => deploy_workers_from(t),
        None => Ok(WorkerGroups::new()),
    }
}

/// [`try_read_worker_groups_with`], reading `bynk.toml` from disk.
pub fn try_read_worker_groups(project_root: &Path) -> Result<WorkerGroups, ProjectPathsError> {
    let path = project_root.join("bynk.toml");
    let overlay = match std::fs::read_to_string(&path) {
        Ok(text) => HashMap::from([(path, text)]),
        Err(_) => HashMap::new(),
    };
    try_read_worker_groups_with(project_root, &overlay)
}

/// A rule the groups break against the project's contexts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupIssue {
    /// The diagnostic code, `bynk.deploy.*`.
    pub code: &'static str,
    /// The group's key the issue is in.
    pub group: String,
    /// What is wrong, and the fix.
    pub message: String,
}

/// The units of a project, by kind, as [`validate_against`] needs them.
pub struct ProjectUnits<'a> {
    /// Every context's qualified name.
    pub contexts: &'a BTreeSet<String>,
    /// Every other unit (commons, adapters): not deployable.
    pub other_units: &'a BTreeSet<String>,
}

/// Check `groups` against the project's units: every member exists and is a
/// context, and no group's key or script name is an implicit Worker's (a
/// context in no group is a Worker named by its dashed name).
pub fn validate_against(groups: &WorkerGroups, units: &ProjectUnits<'_>) -> Vec<GroupIssue> {
    let mut issues = Vec::new();
    let grouped: BTreeSet<&str> = groups
        .values()
        .flat_map(|g| g.contexts.iter().map(String::as_str))
        .collect();
    let implicit: BTreeMap<String, &str> = units
        .contexts
        .iter()
        .filter(|c| !grouped.contains(c.as_str()))
        .map(|c| (crate::paths::worker_dir_name(c), c.as_str()))
        .collect();
    for (key, group) in groups {
        for context in &group.contexts {
            if units.other_units.contains(context) {
                issues.push(GroupIssue {
                    code: "bynk.deploy.not_a_context",
                    group: key.clone(),
                    message: format!(
                        "`{context}` is not a `context`: only contexts are deployed as Workers, so it cannot be in `[deploy.workers.{key}]`"
                    ),
                });
            } else if !units.contexts.contains(context) {
                let hint = did_you_mean(context, units.contexts.iter().map(String::as_str))
                    .map(|c| format!(" — did you mean `{c}`?"))
                    .unwrap_or_default();
                issues.push(GroupIssue {
                    code: "bynk.deploy.unknown_context",
                    group: key.clone(),
                    message: format!(
                        "`[deploy.workers.{key}]` names context `{context}`, which this project does not declare{hint}"
                    ),
                });
            }
        }
        let named = group.name.as_deref().map(|n| ("name", n));
        for (what, name) in std::iter::once(("key", key.as_str())).chain(named) {
            if let Some(context) = implicit.get(name) {
                issues.push(GroupIssue {
                    code: "bynk.deploy.group_name_conflict",
                    group: key.clone(),
                    message: format!(
                        "`[deploy.workers.{key}]`'s {what} `{name}` is the Worker of context `{context}`, which is in no group; name it in a group, or choose another {what}"
                    ),
                });
            }
        }
    }
    issues
}

#[cfg(test)]
mod tests;
