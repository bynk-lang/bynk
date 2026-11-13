//! #3479: the Vectorize settings of `bynk.toml`.
//!
//! A `Vectorize` capability (`capability Docs = Vectorize …`) names an index
//! the deploy creates. A team that already has one (made by hand, or shared
//! with another system) names it here instead, per deployment environment, as
//! Hyperdrive's `config_id` does:
//!
//! ```toml
//! [deploy.vectorize.Docs]
//! index = { production = "docs-prod" }
//! ```
//!
//! An adopted index is checked against the declaration for drift and is never
//! created, changed or deleted by `bynk deploy`.

use crate::discovery::read_source;
use crate::paths::{ProjectPathsError, did_you_mean};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// One capability's `[deploy.vectorize.<Capability>]` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VectorizeDeploy {
    /// Per deployment environment (`default`, or an `--env` name): the name of
    /// an index that already exists, which `bynk deploy` adopts as it is.
    pub index: BTreeMap<String, String>,
}

/// The `[deploy.vectorize]` entries, by capability name.
pub type VectorizeDeployConfig = BTreeMap<String, VectorizeDeploy>;

/// The keys of a `[deploy.vectorize.<Capability>]` entry.
pub const DEPLOY_ENTRY_KEYS: &[&str] = &["index"];

fn bad(table: &'static str, capability: &str, reason: String) -> ProjectPathsError {
    ProjectPathsError::BadVectorize {
        table,
        capability: capability.to_string(),
        reason,
    }
}

/// Whether `name` can be a Vectorize index name: kebab-cased (lowercase
/// letters, digits and hyphens, not starting or ending with one), 1 to 64
/// bytes. It goes into a command line, so nothing else may reach it.
pub fn is_index_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// The `[deploy]` table's `vectorize` entries.
pub fn deploy_vectorize_from(
    deploy: &toml::Table,
) -> Result<VectorizeDeployConfig, ProjectPathsError> {
    let mut out = VectorizeDeployConfig::new();
    let Some(value) = deploy.get("vectorize") else {
        return Ok(out);
    };
    let all = value.as_table().ok_or_else(|| {
        bad(
            "deploy",
            "…",
            "`vectorize` must be a table of capabilities, like `[deploy.vectorize.Docs]`"
                .to_string(),
        )
    })?;
    for (capability, value) in all {
        let table = "deploy.vectorize";
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
                    format!("has no key named `{key}`{hint} (its key is `index`)"),
                ));
            }
        }
        let Some(index) = entry.get("index") else {
            return Err(bad(
                table,
                capability,
                "needs an `index`, like `index = { production = \"docs-prod\" }`".to_string(),
            ));
        };
        let names = index.as_table().ok_or_else(|| {
            bad(
                table,
                capability,
                "`index` must be a table of environment to index name, like `index = { production = \"docs-prod\" }`"
                    .to_string(),
            )
        })?;
        let mut deploy = VectorizeDeploy::default();
        for (environment, name) in names {
            let name = name.as_str().filter(|n| is_index_name(n)).ok_or_else(|| {
                bad(
                    table,
                    capability,
                    format!(
                        "`index.{environment}` must be a Vectorize index name: 1 to 64 lowercase letters, digits or `-`, not starting or ending with `-`, as `wrangler vectorize list` shows"
                    ),
                )
            })?;
            deploy.index.insert(environment.clone(), name.to_string());
        }
        if deploy.index.is_empty() {
            return Err(bad(
                table,
                capability,
                "`index` names no environment".to_string(),
            ));
        }
        out.insert(capability.clone(), deploy);
    }
    Ok(out)
}

/// Read `bynk.toml`'s `[deploy.vectorize]` entries, honouring `overlay`. No
/// manifest, or none of the table, is empty.
pub fn try_read_vectorize_deploy_with(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<VectorizeDeployConfig, ProjectPathsError> {
    let Ok(content) = read_source(&project_root.join("bynk.toml"), overlay) else {
        return Ok(VectorizeDeployConfig::new());
    };
    let doc = content
        .parse::<toml::Table>()
        .map_err(|_| ProjectPathsError::Malformed)?;
    match doc.get("deploy").and_then(|v| v.as_table()) {
        Some(t) => deploy_vectorize_from(t),
        None => Ok(VectorizeDeployConfig::new()),
    }
}

/// [`try_read_vectorize_deploy_with`], reading `bynk.toml` from disk.
pub fn try_read_vectorize_deploy(
    project_root: &Path,
) -> Result<VectorizeDeployConfig, ProjectPathsError> {
    let path = project_root.join("bynk.toml");
    let overlay = match std::fs::read_to_string(&path) {
        Ok(text) => HashMap::from([(path, text)]),
        Err(_) => HashMap::new(),
    };
    try_read_vectorize_deploy_with(project_root, &overlay)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deploy(text: &str) -> Result<VectorizeDeployConfig, String> {
        let doc: toml::Table = text.parse().unwrap();
        deploy_vectorize_from(doc["deploy"].as_table().unwrap()).map_err(|e| e.to_string())
    }

    #[test]
    fn an_environment_names_an_existing_index() {
        let read = deploy(
            "[deploy.vectorize.Docs]\nindex = { production = \"docs-prod\", default = \"docs\" }\n",
        )
        .unwrap();
        assert_eq!(read["Docs"].index["production"], "docs-prod");
        assert_eq!(read["Docs"].index["default"], "docs");
        assert!(deploy("[deploy]\n").unwrap().is_empty());
    }

    #[test]
    fn a_malformed_entry_is_refused_naming_the_capability() {
        let err = |text: &str| deploy(text).unwrap_err();
        assert!(err("[deploy.vectorize.Docs]\n").contains("needs an `index`"));
        assert!(
            err("[deploy.vectorize.Docs]\nindex = \"docs\"\n").contains("table of environment")
        );
        assert!(err("[deploy.vectorize.Docs]\nindex = {}\n").contains("names no environment"));
        assert!(
            err("[deploy.vectorize.Docs]\nindex = { production = \"Docs_Prod\" }\n")
                .contains("`index.production` must be a Vectorize index name")
        );
        let unknown = err("[deploy.vectorize.Docs]\nindexes = { a = \"b\" }\n");
        assert!(
            unknown.contains("no key named `indexes`") && unknown.contains("did you mean `index`"),
            "{unknown}"
        );
        assert!(err("[deploy]\nvectorize = 3\n").contains("table of capabilities"));
    }

    #[test]
    fn an_index_name_is_kebab_case_of_at_most_64_bytes() {
        assert!(is_index_name("docs-prod"));
        assert!(is_index_name(&"a".repeat(64)));
        assert!(!is_index_name(&"a".repeat(65)));
        for bad in ["", "-a", "a-", "A", "a_b", "a b", "a/b", "a..b"] {
            assert!(!is_index_name(bad), "{bad}");
        }
    }
}
