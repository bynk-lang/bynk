//! `bynk publish` and `bynk yank` (packaging slice 4c, #2287, decisions B and
//! F): the client side of the publish API.
//!
//! `publish` checks the package, packs it, uploads it to the registry its
//! organisation routes to, and then reads the new line back through the
//! ordinary verifying client — so a server that signed anything other than
//! what was sent is caught at once.

use std::path::Path;

use bynk_project::manifest::{DependencySource, package_config_from};

use super::Client;
use super::archive;
use crate::fetch::{FetchError, fail};

pub(crate) const PUBLISH_REFUSED: &str = "bynk.registry.publish_refused";

/// What `publish` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub name: String,
    pub version: String,
    pub checksum: String,
    pub registry: String,
    /// Nothing was uploaded (`--dry-run`).
    pub dry_run: bool,
}

/// Publish the package at `project` (which must already pass `bynk check`).
pub fn publish(project: &Path, dry_run: bool) -> Result<Published, FetchError> {
    let doc = crate::fetch::read_manifest(project)
        .ok_or_else(|| fail(PUBLISH_REFUSED, "there is no readable `bynk.toml` here"))?;
    let package = doc
        .get("package")
        .and_then(|v| v.as_table())
        .and_then(|t| package_config_from(t).ok())
        .ok_or_else(|| {
            fail(
                PUBLISH_REFUSED,
                "this project isn't a package: `bynk.toml` has no `[package]`",
            )
        })?;
    let version = package.version.clone().ok_or_else(|| {
        fail(
            PUBLISH_REFUSED,
            "`[package]` has no `version`, which publishing needs",
        )
    })?;
    for spec in crate::fetch::specs(&doc, "dependencies") {
        if !matches!(spec.source, DependencySource::Version(_)) {
            return Err(fail(
                PUBLISH_REFUSED,
                format!(
                    "the dependency `{}` is a path or git dependency, which a registry refuses — publish it, and depend on it by `version`",
                    spec.alias
                ),
            ));
        }
    }
    let name = format!("{}/{}", package.org, package.package);
    let (bytes, checksum) = archive::pack(project)
        .map_err(|e| fail(PUBLISH_REFUSED, format!("could not pack the package: {e}")))?;
    let (mut client, registry) = client_for(project, &package.org);
    let mut out = Published {
        name: name.clone(),
        version: version.clone(),
        checksum: checksum.clone(),
        registry: registry.clone(),
        dry_run,
    };
    if dry_run {
        return Ok(out);
    }
    let (status, body) =
        client.api_write(&registry, "PUT", &format!("{name}/{version}"), Some(&bytes))?;
    if status != 201 && status != 200 {
        return Err(refused(&registry, status, &body));
    }
    // Read it back as any consumer would.
    client.forget(&registry, &name);
    let lines = client.lines(&registry, &name)?;
    let line = lines.iter().find(|l| l.version == version).ok_or_else(|| {
        fail(
            PUBLISH_REFUSED,
            format!(
                "registry `{registry}` accepted `{name}` {version}, but its index doesn't list it"
            ),
        )
    })?;
    if line.checksum != checksum {
        return Err(fail(
            crate::fetch::CHECKSUM_MISMATCH,
            format!(
                "registry `{registry}` signed `{name}` {version} as the tree {}, but bynk sent {checksum}",
                line.checksum
            ),
        ));
    }
    out.checksum = line.checksum.clone();
    Ok(out)
}

/// Yank (or, with `undo`, un-yank) `package` `version` through its
/// registry's API, and confirm it by reading the index back.
pub fn yank(
    project: &Path,
    package: &str,
    version: &str,
    undo: bool,
) -> Result<String, FetchError> {
    let org = package.split('/').next().unwrap_or_default();
    let (mut client, registry) = client_for(project, org);
    let verb = if undo { "unyank" } else { "yank" };
    let (status, body) = client.api_write(
        &registry,
        "POST",
        &format!("{package}/{version}/{verb}"),
        None,
    )?;
    if !(200..300).contains(&status) {
        return Err(refused(&registry, status, &body));
    }
    client.forget(&registry, package);
    let lines = client.lines(&registry, package)?;
    match lines.iter().find(|l| l.version == version) {
        Some(l) if l.yanked != undo => Err(fail(
            PUBLISH_REFUSED,
            format!("registry `{registry}` accepted the {verb}, but its index doesn't show it"),
        )),
        Some(_) => Ok(registry),
        None => Err(fail(
            PUBLISH_REFUSED,
            format!("registry `{registry}` has no `{package}` {version}"),
        )),
    }
}

/// A client over the project's (or workspace's) `[registries]` and lock, and
/// the registry `org` routes to.
fn client_for(project: &Path, org: &str) -> (Client, String) {
    let root = bynk_project::lock::lock_root(project);
    let lock = std::fs::read_to_string(root.join(bynk_project::lock::LOCK_FILE))
        .ok()
        .and_then(|t| bynk_project::lock::parse_lock(&t).ok());
    let client = Client::new(super::commands::registries_of(&root), lock.as_ref());
    let registry = client.route(org);
    (client, registry)
}

/// A refusal from the API, with its JSON `error` and `message` when present.
fn refused(registry: &str, status: u16, body: &[u8]) -> FetchError {
    let parsed: Option<(String, String)> = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            Some((
                v.get("error")?.as_str()?.to_owned(),
                v.get("message")?.as_str()?.to_owned(),
            ))
        });
    match parsed {
        Some((code, message)) => fail(
            PUBLISH_REFUSED,
            format!("registry `{registry}` refused it (HTTP {status}, `{code}`): {message}"),
        ),
        None if status == 401 => fail(
            PUBLISH_REFUSED,
            format!(
                "registry `{registry}` needs a publish token (HTTP 401): set `{}`",
                bynk_project::manifest::registry_token_var(registry)
            ),
        ),
        None => fail(
            PUBLISH_REFUSED,
            format!("registry `{registry}` refused it (HTTP {status})"),
        ),
    }
}
