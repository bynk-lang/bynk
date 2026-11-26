//! #3883: the inbound-email settings of `bynk.toml`.
//!
//! Wrangler's `addresses` is **top-level only** and applies to every environment of a Worker, so
//! `bynk deploy --env staging` cannot reuse the source's addresses: they are production's, and
//! naming them again would take production's routing rules over. A non-default environment
//! therefore claims no address unless it is given its own here:
//!
//! ```toml
//! [deploy.email."support@shop.example"]
//! address = { staging = "support@staging.shop.example" }
//! ```
//!
//! The key is an address a `from email` service names. A mapped environment claims the mapped
//! address in its place, and its Worker accepts mail to it. `default` is never mapped: its
//! addresses are the source's.
//!
//! This module reads the shape. Whether a key names a `from email` address and whether an
//! address is well formed need the project's sources and the checker's address rule, so the
//! deploy checks them (`bynk/src/deploy`).

use crate::discovery::read_source;
use crate::paths::{ProjectPathsError, did_you_mean};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// The environment a `from email` address belongs to when it is not mapped.
pub const DEFAULT_ENVIRONMENT: &str = "default";

/// One `[deploy.email."<address>"]` entry: the address each environment claims in its place.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmailDeploy {
    /// Per environment (an `--env` name; never `default`): the address it claims.
    pub address: BTreeMap<String, String>,
}

/// The `[deploy.email]` entries, by the source address (lower case) they replace.
pub type EmailDeployConfig = BTreeMap<String, EmailDeploy>;

/// The keys of a `[deploy.email."<address>"]` entry.
pub const DEPLOY_ENTRY_KEYS: &[&str] = &["address"];

fn bad(address: &str, reason: String) -> ProjectPathsError {
    ProjectPathsError::BadEmailDeploy {
        address: address.to_string(),
        reason,
    }
}

/// The `[deploy]` table's `email` entries.
pub fn deploy_email_from(deploy: &toml::Table) -> Result<EmailDeployConfig, ProjectPathsError> {
    let mut out = EmailDeployConfig::new();
    let Some(value) = deploy.get("email") else {
        return Ok(out);
    };
    let all = value.as_table().ok_or_else(|| {
        bad(
            "",
            "must be a table of addresses, like `[deploy.email.\"support@shop.example\"]`"
                .to_string(),
        )
    })?;
    for (address, value) in all {
        let entry = value
            .as_table()
            .ok_or_else(|| bad(address, "must be a table".to_string()))?;
        for key in entry.keys() {
            if !DEPLOY_ENTRY_KEYS.contains(&key.as_str()) {
                let hint = did_you_mean(key, DEPLOY_ENTRY_KEYS.iter().copied())
                    .map(|k| format!(" — did you mean `{k}`?"))
                    .unwrap_or_default();
                return Err(bad(
                    address,
                    format!("has no key named `{key}`{hint} (its key is `address`)"),
                ));
            }
        }
        let example = "like `address = { staging = \"support@staging.shop.example\" }`";
        let Some(mapped) = entry.get("address") else {
            return Err(bad(address, format!("needs an `address`, {example}")));
        };
        let mapped = mapped.as_table().ok_or_else(|| {
            bad(
                address,
                format!("`address` must be a table of environment to address, {example}"),
            )
        })?;
        let mut deploy = EmailDeploy::default();
        for (environment, claimed) in mapped {
            if environment == DEFAULT_ENVIRONMENT {
                return Err(bad(
                    address,
                    "`address.default` cannot be mapped: the default environment claims the address as the source writes it".to_string(),
                ));
            }
            let claimed = claimed.as_str().filter(|a| !a.is_empty()).ok_or_else(|| {
                bad(
                    address,
                    format!("`address.{environment}` must be an address, like \"support@staging.shop.example\""),
                )
            })?;
            deploy
                .address
                .insert(environment.clone(), claimed.to_ascii_lowercase());
        }
        if deploy.address.is_empty() {
            return Err(bad(address, "`address` names no environment".to_string()));
        }
        if out.insert(address.to_ascii_lowercase(), deploy).is_some() {
            return Err(bad(
                address,
                "is the same address as an earlier entry, ignoring case; the first would be silently replaced".to_string(),
            ));
        }
    }
    Ok(out)
}

/// Read `bynk.toml`'s `[deploy.email]` entries, honouring `overlay`. No manifest, or none of the
/// table, is empty.
pub fn try_read_email_deploy_with(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<EmailDeployConfig, ProjectPathsError> {
    // The overlay first (an open, unsaved manifest), then the disk: a build reads the manifest
    // from disk, as `[deploy.workers]` does.
    let manifest = project_root.join("bynk.toml");
    let content = match read_source(&manifest, overlay) {
        Ok(text) => text,
        Err(_) => match std::fs::read_to_string(&manifest) {
            Ok(text) => text,
            Err(_) => return Ok(EmailDeployConfig::new()),
        },
    };
    let doc = content
        .parse::<toml::Table>()
        .map_err(|_| ProjectPathsError::Malformed)?;
    match doc.get("deploy").and_then(|v| v.as_table()) {
        Some(t) => deploy_email_from(t),
        None => Ok(EmailDeployConfig::new()),
    }
}

/// [`try_read_email_deploy_with`], reading `bynk.toml` from disk.
pub fn try_read_email_deploy(project_root: &Path) -> Result<EmailDeployConfig, ProjectPathsError> {
    let path = project_root.join("bynk.toml");
    let overlay = match std::fs::read_to_string(&path) {
        Ok(text) => HashMap::from([(path, text)]),
        Err(_) => HashMap::new(),
    };
    try_read_email_deploy_with(project_root, &overlay)
}

/// What `environment` claims, given the addresses the source's `from email` services name:
/// the source's own for `default`, and for any other environment only what it is mapped to
/// (nothing when it has no mapping). Sorted and de-duplicated.
pub fn claimed_addresses(
    config: &EmailDeployConfig,
    source: &[String],
    environment: &str,
) -> Vec<String> {
    if environment == DEFAULT_ENVIRONMENT {
        let mut own = source.to_vec();
        own.sort();
        own.dedup();
        return own;
    }
    let mapped: BTreeSet<&String> = source
        .iter()
        .filter_map(|a| config.get(&a.to_ascii_lowercase()))
        .filter_map(|entry| entry.address.get(environment))
        .collect();
    mapped.into_iter().cloned().collect()
}

/// Every address an environment is mapped to, by the source address it stands in for: the
/// addresses a Worker's `email()` accepts besides the source's own.
pub fn mapped_addresses(config: &EmailDeployConfig, source: &str) -> Vec<String> {
    let mut all: Vec<String> = config
        .get(&source.to_ascii_lowercase())
        .map(|entry| entry.address.values().cloned().collect())
        .unwrap_or_default();
    all.sort();
    all.dedup();
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<EmailDeployConfig, String> {
        let doc: toml::Table = text.parse().unwrap();
        deploy_email_from(doc["deploy"].as_table().unwrap()).map_err(|e| e.to_string())
    }

    const MAPPED: &str = "[deploy.email.\"Support@Shop.example\"]\naddress = { staging = \"Support@Staging.shop.example\" }\n";

    #[test]
    fn an_entry_is_read_and_lower_cased() {
        let config = parse(MAPPED).unwrap();
        assert_eq!(
            config["support@shop.example"].address["staging"],
            "support@staging.shop.example"
        );
    }

    #[test]
    fn no_table_is_no_mapping() {
        assert!(
            parse("[deploy.hyperdrive.Main]\nconfig_id = \"x\"\n")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_shape_is_checked() {
        let cases = [
            ("[deploy]\nemail = 3\n", "must be a table of addresses"),
            ("[deploy.email]\n\"a@b.example\" = 3\n", "must be a table"),
            (
                "[deploy.email.\"a@b.example\"]\naddres = {}\n",
                "did you mean `address`?",
            ),
            ("[deploy.email.\"a@b.example\"]\n", "needs an `address`"),
            (
                "[deploy.email.\"a@b.example\"]\naddress = \"x\"\n",
                "`address` must be a table",
            ),
            (
                "[deploy.email.\"a@b.example\"]\naddress = {}\n",
                "names no environment",
            ),
            (
                "[deploy.email.\"a@b.example\"]\naddress = { staging = 3 }\n",
                "`address.staging` must be an address",
            ),
            (
                "[deploy.email.\"a@b.example\"]\naddress = { default = \"c@d.example\" }\n",
                "`address.default` cannot be mapped",
            ),
            (
                "[deploy.email.\"A@b.example\"]\naddress = { staging = \"c@d.example\" }\n[deploy.email.\"a@b.example\"]\naddress = { qa = \"e@f.example\" }\n",
                "same address as an earlier entry",
            ),
        ];
        for (text, expected) in cases {
            let message = parse(text).unwrap_err();
            assert!(message.contains(expected), "{text}: {message}");
        }
    }

    #[test]
    fn default_claims_the_source_and_another_environment_only_its_mapping() {
        let config = parse(MAPPED).unwrap();
        let source = vec![
            "support@shop.example".to_string(),
            "*@shop.example".to_string(),
        ];
        assert_eq!(
            claimed_addresses(&config, &source, "default"),
            ["*@shop.example", "support@shop.example"]
        );
        assert_eq!(
            claimed_addresses(&config, &source, "staging"),
            ["support@staging.shop.example"]
        );
        assert!(claimed_addresses(&config, &source, "qa").is_empty());
        assert_eq!(
            mapped_addresses(&config, "support@shop.example"),
            ["support@staging.shop.example"]
        );
        assert!(mapped_addresses(&config, "*@shop.example").is_empty());
    }
}
