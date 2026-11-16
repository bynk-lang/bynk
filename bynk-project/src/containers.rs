//! #3477 (slice C2): the Container settings of `bynk.toml`.
//!
//! A `Container` capability (`capability Renderer = Container @build("…")`)
//! has its image built from the Dockerfile `@build` names, by `wrangler deploy`,
//! which needs Docker. A team that builds elsewhere, or deploys from a machine
//! without Docker, names a prebuilt image here instead, per deployment
//! environment:
//!
//! ```toml
//! [deploy.containers.Renderer]
//! image = { production = "registry.cloudflare.com/<ACCOUNT_ID>/renderer@sha256:<DIGEST>" }
//! ```
//!
//! The image has to be in Cloudflare's own registry and named by digest, not by
//! tag: Cloudflare takes no other registry for a container whose class runs
//! under the `durable_object` scheduling policy. An environment with an adopted
//! image is never built.

use crate::discovery::read_source;
use crate::paths::{ProjectPathsError, did_you_mean};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// One capability's `[deploy.containers.<Capability>]` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerDeploy {
    /// Per deployment environment (`default`, or an `--env` name): the
    /// digest-pinned image `bynk deploy` runs instead of building one.
    pub image: BTreeMap<String, String>,
}

/// The `[deploy.containers]` entries, by capability name.
pub type ContainerDeployConfig = BTreeMap<String, ContainerDeploy>;

/// The keys of a `[deploy.containers.<Capability>]` entry.
pub const DEPLOY_ENTRY_KEYS: &[&str] = &["image"];

/// The registry a `durable_object` container's image must be in.
pub const REGISTRY: &str = "registry.cloudflare.com";

fn bad(table: &'static str, capability: &str, reason: String) -> ProjectPathsError {
    ProjectPathsError::BadContainers {
        table,
        capability: capability.to_string(),
        reason,
    }
}

/// Whether `image` is `registry.cloudflare.com/<account>/<repository>@sha256:<64
/// hex digits>`: the registry, a path of at least two segments of lowercase
/// letters, digits and `._-`, and a digest. It goes into a configuration file
/// and a log line, so nothing else may reach them.
pub fn is_image_reference(image: &str) -> bool {
    let Some(rest) = image
        .strip_prefix(REGISTRY)
        .and_then(|r| r.strip_prefix('/'))
    else {
        return false;
    };
    let Some((path, digest)) = rest.split_once("@sha256:") else {
        return false;
    };
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && path.split('/').count() >= 2
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && segment.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
                })
        })
}

/// The `[deploy]` table's `containers` entries.
pub fn deploy_containers_from(
    deploy: &toml::Table,
) -> Result<ContainerDeployConfig, ProjectPathsError> {
    let mut out = ContainerDeployConfig::new();
    let Some(value) = deploy.get("containers") else {
        return Ok(out);
    };
    let table = "deploy.containers";
    let all = value.as_table().ok_or_else(|| {
        bad(
            "deploy",
            "…",
            "`containers` must be a table of capabilities, like `[deploy.containers.Renderer]`"
                .to_string(),
        )
    })?;
    for (capability, value) in all {
        let entry = value
            .as_table()
            .ok_or_else(|| bad(table, capability, "must be a table".to_string()))?;
        for key in entry.keys() {
            if !DEPLOY_ENTRY_KEYS.contains(&key.as_str()) {
                let hint = did_you_mean(key, DEPLOY_ENTRY_KEYS.iter().copied())
                    .map(|k| format!(" — did you mean `{k}`?"))
                    .unwrap_or_default();
                return Err(bad(
                    table,
                    capability,
                    format!("has no key named `{key}`{hint} (its key is `image`)"),
                ));
            }
        }
        let Some(image) = entry.get("image") else {
            return Err(bad(
                table,
                capability,
                "needs an `image`, like `image = { production = \"registry.cloudflare.com/<ACCOUNT_ID>/renderer@sha256:<DIGEST>\" }`".to_string(),
            ));
        };
        let images = image.as_table().ok_or_else(|| {
            bad(
                table,
                capability,
                "`image` must be a table of environment to image reference, like `image = { production = \"registry.cloudflare.com/<ACCOUNT_ID>/renderer@sha256:<DIGEST>\" }`".to_string(),
            )
        })?;
        let mut deploy = ContainerDeploy::default();
        for (environment, reference) in images {
            let reference = reference
                .as_str()
                .filter(|r| is_image_reference(r))
                .ok_or_else(|| {
                    bad(
                        table,
                        capability,
                        format!(
                            "`image.{environment}` must be an image in Cloudflare's registry, named by digest: `{REGISTRY}/<ACCOUNT_ID>/<REPOSITORY>@sha256:<64 hex digits>` (another registry, or a tag, is not supported for this kind of container)"
                        ),
                    )
                })?;
            deploy
                .image
                .insert(environment.clone(), reference.to_string());
        }
        if deploy.image.is_empty() {
            return Err(bad(
                table,
                capability,
                "`image` names no environment".to_string(),
            ));
        }
        out.insert(capability.clone(), deploy);
    }
    Ok(out)
}

/// Read `bynk.toml`'s `[deploy.containers]` entries, honouring `overlay`. No
/// manifest, or none of the table, is empty.
pub fn try_read_containers_deploy_with(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<ContainerDeployConfig, ProjectPathsError> {
    let Ok(content) = read_source(&project_root.join("bynk.toml"), overlay) else {
        return Ok(ContainerDeployConfig::new());
    };
    let doc = content
        .parse::<toml::Table>()
        .map_err(|_| ProjectPathsError::Malformed)?;
    match doc.get("deploy").and_then(|v| v.as_table()) {
        Some(t) => deploy_containers_from(t),
        None => Ok(ContainerDeployConfig::new()),
    }
}

/// [`try_read_containers_deploy_with`], reading `bynk.toml` from disk.
pub fn try_read_containers_deploy(
    project_root: &Path,
) -> Result<ContainerDeployConfig, ProjectPathsError> {
    let path = project_root.join("bynk.toml");
    let overlay = match std::fs::read_to_string(&path) {
        Ok(text) => HashMap::from([(path, text)]),
        Err(_) => HashMap::new(),
    };
    try_read_containers_deploy_with(project_root, &overlay)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn read(text: &str) -> Result<ContainerDeployConfig, ProjectPathsError> {
        let doc: toml::Table = text.parse().unwrap();
        deploy_containers_from(doc["deploy"].as_table().unwrap())
    }

    #[test]
    fn an_image_is_cloudflares_registry_by_digest() {
        let good = format!("registry.cloudflare.com/0123abcd/renderer@sha256:{DIGEST}");
        assert!(is_image_reference(&good));
        assert!(is_image_reference(&format!(
            "registry.cloudflare.com/acct/team/render-er.v2@sha256:{DIGEST}"
        )));
        for bad in [
            format!("docker.io/library/nginx@sha256:{DIGEST}"),
            "registry.cloudflare.com/acct/renderer:latest".to_string(),
            "registry.cloudflare.com/acct/renderer@sha256:abc".to_string(),
            format!("registry.cloudflare.com/renderer@sha256:{DIGEST}"),
            format!("registry.cloudflare.com/acct/Renderer@sha256:{DIGEST}"),
            format!("registry.cloudflare.com/acct/ren derer@sha256:{DIGEST}"),
            format!("registry.cloudflare.com//renderer@sha256:{DIGEST}"),
            format!(
                "registry.cloudflare.com/acct/renderer@sha256:{}",
                DIGEST.to_uppercase()
            ),
        ] {
            assert!(!is_image_reference(&bad), "{bad}");
        }
    }

    #[test]
    fn an_entry_names_an_image_per_environment() {
        let text = format!(
            "[deploy.containers.Renderer]\nimage = {{ production = \"registry.cloudflare.com/a1/r@sha256:{DIGEST}\" }}\n"
        );
        let config = read(&text).unwrap();
        assert_eq!(
            config["Renderer"].image["production"],
            format!("registry.cloudflare.com/a1/r@sha256:{DIGEST}")
        );
        assert!(read("[deploy]\n").unwrap().is_empty());
    }

    #[test]
    fn a_malformed_entry_is_refused_with_its_table_and_capability() {
        for (text, needle) in [
            (
                "[deploy.containers.R]\nimagee = {}\n",
                "did you mean `image`",
            ),
            ("[deploy.containers.R]\n", "needs an `image`"),
            ("[deploy.containers.R]\nimage = \"x\"\n", "must be a table"),
            (
                "[deploy.containers.R]\nimage = {}\n",
                "names no environment",
            ),
            (
                "[deploy.containers.R]\nimage = { production = \"docker.io/x/y:1\" }\n",
                "Cloudflare's registry",
            ),
        ] {
            let err = read(text).unwrap_err().to_string();
            assert!(err.contains("[deploy.containers.R]"), "{err}");
            assert!(err.contains(needle), "{needle}: {err}");
        }
    }
}
