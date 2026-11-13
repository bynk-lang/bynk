//! #3478: the `[dev]` acknowledgements of remote-only bindings.
//!
//! Some Cloudflare bindings have no local simulation: under `bynk dev`, a call
//! runs on the developer's Cloudflare account and may incur charges. `bynk dev`
//! does not block such a call. It warns once, on the first one, unless the
//! project has acknowledged it in `bynk.toml`:
//!
//! ```toml
//! [dev]
//! ai = "allow"
//! ```
//!
//! One mechanism serves every such binding kind: a [`RemoteKind`] names the
//! `[dev]` key and the variable `bynk dev` hands to `wrangler dev` (as
//! `--var`) for an acknowledged kind, which the kind's provider reads to stay
//! quiet. Vectorize and AI Search add one entry each. No tooling here states
//! a price, an allowance or which operations are billed: the warning links to
//! Cloudflare's own pages.

use crate::discovery::read_source;
use crate::paths::{ProjectPathsError, did_you_mean};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// A binding kind with no local simulation under `bynk dev`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteKind {
    /// The `[dev]` key that acknowledges it (`ai`).
    pub key: &'static str,
    /// The variable `bynk dev` sets to `allow` on an acknowledged kind's
    /// Workers, which the kind's provider reads (`BYNK_DEV_ACK_AI`).
    pub var: &'static str,
}

/// The remote-only binding kinds, by `[dev]` key.
pub const REMOTE_KINDS: &[RemoteKind] = &[
    RemoteKind {
        key: "ai",
        var: "BYNK_DEV_ACK_AI",
    },
    // #3480: AI Search is its own kind, not covered by `ai`.
    RemoteKind {
        key: "ai-search",
        var: "BYNK_DEV_ACK_AI_SEARCH",
    },
];

/// The variable `bynk dev` sets to `1` on every Worker it serves, so a
/// provider can tell a development session from a deployment. A deployed
/// Worker has neither this nor any acknowledgement variable.
pub const DEV_VAR: &str = "BYNK_DEV";

/// The one value an acknowledgement takes.
pub const ALLOW: &str = "allow";

fn bad(key: &str, reason: String) -> ProjectPathsError {
    ProjectPathsError::BadDevAck {
        key: key.to_string(),
        reason,
    }
}

/// The remote-only kinds `[dev]` acknowledges. Any other value than
/// `"allow"`, for a known key, is an error.
pub fn dev_acks_from(dev: &toml::Table) -> Result<BTreeSet<&'static str>, ProjectPathsError> {
    let mut out = BTreeSet::new();
    for kind in REMOTE_KINDS {
        let Some(value) = dev.get(kind.key) else {
            continue;
        };
        match value.as_str() {
            Some(ALLOW) => {
                out.insert(kind.key);
            }
            Some(other) => {
                let hint = did_you_mean(other, [ALLOW])
                    .map(|s| format!(" (did you mean `{s}`?)"))
                    .unwrap_or_default();
                return Err(bad(
                    kind.key,
                    format!(
                        "must be `\"allow\"`, the only acknowledgement, not `\"{other}\"`{hint}"
                    ),
                ));
            }
            None => {
                return Err(bad(
                    kind.key,
                    "must be the string `\"allow\"`, like `ai = \"allow\"`".to_string(),
                ));
            }
        }
    }
    Ok(out)
}

/// Read `bynk.toml`'s `[dev]` acknowledgements, honouring `overlay`. No
/// manifest, or no `[dev]` table, acknowledges nothing.
pub fn try_read_dev_acks_with(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<BTreeSet<&'static str>, ProjectPathsError> {
    let Ok(content) = read_source(&project_root.join("bynk.toml"), overlay) else {
        return Ok(BTreeSet::new());
    };
    let doc = content
        .parse::<toml::Table>()
        .map_err(|_| ProjectPathsError::Malformed)?;
    match doc.get("dev").and_then(|v| v.as_table()) {
        Some(dev) => dev_acks_from(dev),
        None => Ok(BTreeSet::new()),
    }
}

/// [`try_read_dev_acks_with`], reading `bynk.toml` from disk.
pub fn try_read_dev_acks(project_root: &Path) -> Result<BTreeSet<&'static str>, ProjectPathsError> {
    let path = project_root.join("bynk.toml");
    let overlay = match std::fs::read_to_string(&path) {
        Ok(text) => HashMap::from([(path, text)]),
        Err(_) => HashMap::new(),
    };
    try_read_dev_acks_with(project_root, &overlay)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acks(text: &str) -> Result<Vec<&'static str>, String> {
        let doc: toml::Table = text.parse().unwrap();
        dev_acks_from(doc["dev"].as_table().unwrap())
            .map(|s| s.into_iter().collect())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn allow_acknowledges_a_kind() {
        assert_eq!(acks("[dev]\nai = \"allow\"\n"), Ok(vec!["ai"]));
        // #3480: AI Search is acknowledged on its own, not by `ai`.
        assert_eq!(
            acks("[dev]\nai-search = \"allow\"\n"),
            Ok(vec!["ai-search"])
        );
        assert_eq!(
            acks("[dev.hyperdrive.Main]\nconnection_env = \"X\"\n[dev]\n"),
            Ok(vec![])
        );
    }

    #[test]
    fn nothing_else_acknowledges() {
        let e = acks("[dev]\nai = \"alow\"\n").unwrap_err();
        assert!(
            e.contains("`[dev]` `ai`") && e.contains("did you mean `allow`"),
            "{e}"
        );
        let e = acks("[dev]\nai = true\n").unwrap_err();
        assert!(e.contains("must be the string"), "{e}");
    }

    #[test]
    fn every_kind_has_its_own_key_and_variable() {
        let keys: BTreeSet<_> = REMOTE_KINDS.iter().map(|k| k.key).collect();
        let vars: BTreeSet<_> = REMOTE_KINDS.iter().map(|k| k.var).collect();
        assert_eq!(keys.len(), REMOTE_KINDS.len());
        assert_eq!(vars.len(), REMOTE_KINDS.len());
        for k in REMOTE_KINDS {
            assert!(k.var.starts_with("BYNK_DEV_ACK_"), "{}", k.var);
        }
    }
}
