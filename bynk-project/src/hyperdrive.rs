//! #3319: the Hyperdrive settings of `bynk.toml`.
//!
//! Two tables name a `Postgres` capability (`capability Main = Postgres`) and
//! say where its connection string comes from. Neither holds a connection
//! string: a string is a secret, and `bynk.toml` is committed. Each names an
//! environment variable to read it from.
//!
//! ```toml
//! # Deploy: `bynk deploy` makes or updates the Hyperdrive config from the
//! # variable, or adopts a config that already exists (no string needed).
//! [deploy.hyperdrive.Main]
//! connection_env = "MAIN_DATABASE_URL"
//! config_id = { production = "0123…" }
//!
//! # Development: `bynk dev` hands the variable's value to `wrangler dev`.
//! [dev.hyperdrive.Main]
//! connection_env = "MAIN_LOCAL_DATABASE_URL"
//! ```

use crate::discovery::read_source;
use crate::paths::{ProjectPathsError, did_you_mean};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// One capability's `[deploy.hyperdrive.<Capability>]` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HyperdriveDeploy {
    /// The environment variable `bynk deploy` reads the connection string
    /// from. Never the string itself.
    pub connection_env: Option<String>,
    /// Per deployment environment (`default`, or an `--env` name): the id of
    /// a Hyperdrive config that already exists. An environment listed here is
    /// adopted, and needs no connection string.
    pub config_id: BTreeMap<String, String>,
}

/// The `[deploy.hyperdrive]` entries, by capability name.
pub type HyperdriveDeployConfig = BTreeMap<String, HyperdriveDeploy>;

/// The `[dev.hyperdrive]` entries, by capability name: the environment
/// variable `bynk dev` reads each connection string from.
pub type HyperdriveDevConfig = BTreeMap<String, String>;

/// The keys of a `[deploy.hyperdrive.<Capability>]` entry.
pub const DEPLOY_ENTRY_KEYS: &[&str] = &["connection_env", "config_id"];

/// The keys of a `[dev.hyperdrive.<Capability>]` entry.
pub const DEV_ENTRY_KEYS: &[&str] = &["connection_env"];

fn bad(table: &'static str, capability: &str, reason: String) -> ProjectPathsError {
    ProjectPathsError::BadHyperdrive {
        table,
        capability: capability.to_string(),
        reason,
    }
}

/// Whether `name` can be an environment variable's name: a letter or `_`,
/// then letters, digits and `_`. A value that is not (`postgres://…`) is a
/// connection string written where a variable's name belongs, which the
/// reader refuses without ever echoing it.
pub fn is_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `connection_env`'s value, checked. The refusal never repeats the value: a
/// connection string pasted here would otherwise be printed by the error.
fn connection_env(
    table: &'static str,
    capability: &str,
    value: &toml::Value,
) -> Result<String, ProjectPathsError> {
    let Some(name) = value.as_str() else {
        return Err(bad(
            table,
            capability,
            "`connection_env` must be a string naming an environment variable".to_string(),
        ));
    };
    if !is_env_var_name(name) {
        return Err(bad(
            table,
            capability,
            "`connection_env` names the environment variable the connection string is read from, \
             not the string itself (a name is letters, digits and `_`)"
                .to_string(),
        ));
    }
    Ok(name.to_string())
}

/// The entries of `hyperdrive` (a table of capability name to entry), each
/// checked against `keys`. `table` is `deploy` or `dev`.
fn entries<'a>(
    table: &'static str,
    parent: &'a toml::Table,
) -> Result<Option<&'a toml::Table>, ProjectPathsError> {
    let Some(value) = parent.get("hyperdrive") else {
        return Ok(None);
    };
    value.as_table().map(Some).ok_or_else(|| {
        bad(
            table,
            "…",
            format!(
                "`hyperdrive` must be a table of capabilities, like `[{table}.hyperdrive.Main]`"
            ),
        )
    })
}

fn entry_table<'a>(
    table: &'static str,
    capability: &str,
    value: &'a toml::Value,
    keys: &[&str],
) -> Result<&'a toml::Table, ProjectPathsError> {
    let entry = value
        .as_table()
        .ok_or_else(|| bad(table, capability, "must be a table".to_string()))?;
    for key in entry.keys() {
        if !keys.contains(&key.as_str()) {
            let hint = did_you_mean(key, keys.iter().copied())
                .map(|k| format!(" — did you mean `{k}`?"))
                .unwrap_or_default();
            return Err(bad(
                table,
                capability,
                format!(
                    "has no key named `{key}`{hint} (its keys are {})",
                    keys.iter()
                        .map(|k| format!("`{k}`"))
                        .collect::<Vec<_>>()
                        .join(" and ")
                ),
            ));
        }
    }
    Ok(entry)
}

/// The `[deploy]` table's `hyperdrive` entries.
pub fn deploy_hyperdrive_from(
    deploy: &toml::Table,
) -> Result<HyperdriveDeployConfig, ProjectPathsError> {
    let mut out = HyperdriveDeployConfig::new();
    let Some(all) = entries("deploy", deploy)? else {
        return Ok(out);
    };
    for (capability, value) in all {
        let entry = entry_table("deploy.hyperdrive", capability, value, DEPLOY_ENTRY_KEYS)?;
        let mut deploy = HyperdriveDeploy::default();
        if let Some(v) = entry.get("connection_env") {
            deploy.connection_env = Some(connection_env("deploy.hyperdrive", capability, v)?);
        }
        if let Some(v) = entry.get("config_id") {
            let ids = v.as_table().ok_or_else(|| {
                bad(
                    "deploy.hyperdrive",
                    capability,
                    "`config_id` must be a table of environment to Hyperdrive config id, \
                     like `config_id = { default = \"…\" }`"
                        .to_string(),
                )
            })?;
            for (environment, id) in ids {
                let id = id.as_str().filter(|s| !s.is_empty()).ok_or_else(|| {
                    bad(
                        "deploy.hyperdrive",
                        capability,
                        format!("`config_id.{environment}` must be a non-empty string"),
                    )
                })?;
                deploy.config_id.insert(environment.clone(), id.to_string());
            }
        }
        if deploy.connection_env.is_none() && deploy.config_id.is_empty() {
            return Err(bad(
                "deploy.hyperdrive",
                capability,
                "needs a `connection_env`, a `config_id`, or both".to_string(),
            ));
        }
        out.insert(capability.clone(), deploy);
    }
    Ok(out)
}

/// The `[dev]` table's `hyperdrive` entries.
pub fn dev_hyperdrive_from(dev: &toml::Table) -> Result<HyperdriveDevConfig, ProjectPathsError> {
    let mut out = HyperdriveDevConfig::new();
    let Some(all) = entries("dev", dev)? else {
        return Ok(out);
    };
    for (capability, value) in all {
        let entry = entry_table("dev.hyperdrive", capability, value, DEV_ENTRY_KEYS)?;
        let Some(v) = entry.get("connection_env") else {
            return Err(bad(
                "dev.hyperdrive",
                capability,
                "needs a `connection_env`".to_string(),
            ));
        };
        out.insert(
            capability.clone(),
            connection_env("dev.hyperdrive", capability, v)?,
        );
    }
    Ok(out)
}

fn table_of(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
    name: &str,
) -> Result<Option<toml::Table>, ProjectPathsError> {
    let Ok(content) = read_source(&project_root.join("bynk.toml"), overlay) else {
        return Ok(None);
    };
    let doc = content
        .parse::<toml::Table>()
        .map_err(|_| ProjectPathsError::Malformed)?;
    Ok(doc.get(name).and_then(|v| v.as_table()).cloned())
}

/// Read `bynk.toml`'s `[deploy.hyperdrive]` entries, honouring `overlay`. No
/// manifest, or none of the table, is empty.
pub fn try_read_hyperdrive_deploy_with(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<HyperdriveDeployConfig, ProjectPathsError> {
    match table_of(project_root, overlay, "deploy")? {
        Some(t) => deploy_hyperdrive_from(&t),
        None => Ok(HyperdriveDeployConfig::new()),
    }
}

/// [`try_read_hyperdrive_deploy_with`], reading `bynk.toml` from disk.
pub fn try_read_hyperdrive_deploy(
    project_root: &Path,
) -> Result<HyperdriveDeployConfig, ProjectPathsError> {
    try_read_hyperdrive_deploy_with(project_root, &disk_overlay(project_root))
}

/// `bynk.toml` as the overlay a reader takes: [`read_source`] consults the
/// overlay, not the disk.
fn disk_overlay(project_root: &Path) -> HashMap<PathBuf, String> {
    let path = project_root.join("bynk.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => HashMap::from([(path, text)]),
        Err(_) => HashMap::new(),
    }
}

/// Read `bynk.toml`'s `[dev.hyperdrive]` entries, honouring `overlay`.
pub fn try_read_hyperdrive_dev_with(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<HyperdriveDevConfig, ProjectPathsError> {
    match table_of(project_root, overlay, "dev")? {
        Some(t) => dev_hyperdrive_from(&t),
        None => Ok(HyperdriveDevConfig::new()),
    }
}

/// [`try_read_hyperdrive_dev_with`], reading `bynk.toml` from disk.
pub fn try_read_hyperdrive_dev(
    project_root: &Path,
) -> Result<HyperdriveDevConfig, ProjectPathsError> {
    try_read_hyperdrive_dev_with(project_root, &disk_overlay(project_root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deploy(text: &str) -> Result<HyperdriveDeployConfig, String> {
        let doc: toml::Table = text.parse().unwrap();
        deploy_hyperdrive_from(doc["deploy"].as_table().unwrap()).map_err(|e| e.to_string())
    }

    fn dev(text: &str) -> Result<HyperdriveDevConfig, String> {
        let doc: toml::Table = text.parse().unwrap();
        dev_hyperdrive_from(doc["dev"].as_table().unwrap()).map_err(|e| e.to_string())
    }

    #[test]
    fn a_deploy_entry_names_a_variable_and_per_environment_ids() {
        let config = deploy(
            "[deploy.hyperdrive.Main]\nconnection_env = \"MAIN_DATABASE_URL\"\n\
             config_id = { production = \"abc\" }\n",
        )
        .unwrap();
        let main = &config["Main"];
        assert_eq!(main.connection_env.as_deref(), Some("MAIN_DATABASE_URL"));
        assert_eq!(main.config_id["production"], "abc");
    }

    #[test]
    fn an_entry_with_neither_a_variable_nor_an_id_is_refused() {
        let message = deploy("[deploy.hyperdrive.Main]\n").unwrap_err();
        assert!(message.contains("`[deploy.hyperdrive.Main]`"), "{message}");
        assert!(message.contains("connection_env"), "{message}");
    }

    #[test]
    fn an_unknown_key_suggests_the_nearest() {
        let message = deploy("[deploy.hyperdrive.Main]\nconnection_evn = \"X\"\n").unwrap_err();
        assert!(
            message.contains("did you mean `connection_env`?"),
            "{message}"
        );
    }

    /// R-16: a connection string written where a variable's name belongs is
    /// refused, and the refusal does not print it.
    #[test]
    fn a_connection_string_in_the_manifest_is_refused_without_being_echoed() {
        let secret = "postgres://app:hunter2-s3cret@db.example.com:5432/app";
        for text in [
            format!("[deploy.hyperdrive.Main]\nconnection_env = \"{secret}\"\n"),
            format!("[dev.hyperdrive.Main]\nconnection_env = \"{secret}\"\n"),
        ] {
            let doc: toml::Table = text.parse().unwrap();
            let message = if doc.contains_key("deploy") {
                deploy(&text)
            } else {
                dev(&text).map(|_| BTreeMap::new())
            }
            .unwrap_err();
            assert!(message.contains("not the string itself"), "{message}");
            assert!(
                !message.contains("hunter2") && !message.contains("db.example.com"),
                "{message}"
            );
        }
    }

    #[test]
    fn config_id_is_a_table_of_non_empty_strings() {
        assert!(
            deploy("[deploy.hyperdrive.Main]\nconfig_id = \"abc\"\n")
                .unwrap_err()
                .contains("must be a table")
        );
        assert!(
            deploy("[deploy.hyperdrive.Main]\nconfig_id = { staging = \"\" }\n")
                .unwrap_err()
                .contains("`config_id.staging` must be a non-empty string")
        );
    }

    #[test]
    fn a_dev_entry_names_a_variable() {
        assert_eq!(
            dev("[dev.hyperdrive.Main]\nconnection_env = \"LOCAL_URL\"\n").unwrap()["Main"],
            "LOCAL_URL"
        );
        assert!(
            dev("[dev.hyperdrive.Main]\n")
                .unwrap_err()
                .contains("needs a `connection_env`")
        );
        assert!(
            dev("[dev.hyperdrive.Main]\nconfig_id = { default = \"a\" }\n")
                .unwrap_err()
                .contains("has no key named `config_id`")
        );
    }

    #[test]
    fn the_disk_readers_read_bynk_toml() {
        let root = std::env::temp_dir().join(format!("bynk-hd-read-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("bynk.toml"),
            "[deploy.hyperdrive.Main]\nconnection_env = \"A\"\n\n[dev.hyperdrive.Main]\nconnection_env = \"B\"\n",
        )
        .unwrap();
        assert_eq!(
            try_read_hyperdrive_deploy(&root).unwrap()["Main"]
                .connection_env
                .as_deref(),
            Some("A")
        );
        assert_eq!(try_read_hyperdrive_dev(&root).unwrap()["Main"], "B");
        assert!(
            try_read_hyperdrive_dev(&root.join("missing"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn variable_names_are_checked() {
        assert!(is_env_var_name("MAIN_DATABASE_URL"));
        assert!(is_env_var_name("_x1"));
        assert!(!is_env_var_name(""));
        assert!(!is_env_var_name("1X"));
        assert!(!is_env_var_name("A-B"));
        assert!(!is_env_var_name("postgres://x"));
    }
}
