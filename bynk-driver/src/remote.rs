//! #2138 (end-to-end testing track §3.5, §3.7, §3.8): `test --against`, a run
//! of the `system` suites against a real runtime.
//!
//! This module owns the **safety** of that run, before any request is made:
//!
//! - `--against` takes only a name. `dev` is reserved for the local `bynk dev`
//!   the runner starts itself; anything else must be a target named under
//!   `[test.targets]` in `bynk.toml`. A bare URL is refused.
//! - A target marked `production = true` is refused.
//! - A target's secrets are read only from the runner's environment, by the
//!   names it lists, and each must be set.
//! - The target and each Worker's URL are printed before the first request
//!   ([`RemoteRun::banner`]).

use std::collections::BTreeMap;
use std::path::Path;

use bynk_emit::project::{RemoteSkip, TestTarget};

/// The reserved `--against` value that starts a local `bynk dev`.
pub const DEV: &str = "dev";

/// A remote run that passed every safety check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRun {
    /// The target's name, as passed to `--against`.
    pub target: String,
    /// Each Worker's base URL, keyed by worker-directory name.
    pub urls: BTreeMap<String, String>,
    /// The non-secret values the target was deployed with.
    pub vars: BTreeMap<String, String>,
    /// The names of the target's secrets.
    pub secrets: Vec<String>,
    /// The `system` cases the build skipped (`--skip-unsupported`), with the
    /// suite each belongs to.
    pub skipped: Vec<(String, RemoteSkip)>,
}

impl RemoteRun {
    /// `BYNK_TEST_TARGETS`: the URLs as the emitted remote harness reads them.
    pub fn targets_json(&self) -> String {
        serde_json::to_string(&self.urls).expect("a string map serialises")
    }

    /// The `transport` the JSON document reports.
    pub fn transport(&self) -> String {
        format!("remote:{}", self.target)
    }

    /// The lines printed before the first request: the target and each
    /// Worker's URL, so a run against the wrong deployment is visible at once.
    pub fn banner(&self) -> String {
        let mut out = format!(
            "running the `system` suites against target `{}`:\n",
            self.target
        );
        let width = self.urls.keys().map(String::len).max().unwrap_or(0);
        for (worker, url) in &self.urls {
            out.push_str(&format!("  {worker:<width$}  {url}\n"));
        }
        out
    }
}

/// Why `--against` was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// `--against dev` from a front end that can't start `bynk dev`, or
    /// before that path exists.
    Dev(String),
    /// A bare URL instead of a target name.
    BareUrl(String),
    /// No `[test.targets.<name>]` of that name.
    Unknown { name: String, known: Vec<String> },
    /// The target is marked `production = true`.
    Production(String),
    /// Secrets the target lists that the environment doesn't set.
    MissingSecrets { target: String, names: Vec<String> },
    /// `bynk.toml` couldn't be read.
    Manifest(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Dev(why) => write!(f, "`--against dev` {why}"),
            Refusal::BareUrl(url) => write!(
                f,
                "`--against {url}` names a URL; a run against a deployment must use a target named under `[test.targets]` in `bynk.toml`, so a URL can't be pointed at by accident"
            ),
            Refusal::Unknown { name, known } if known.is_empty() => write!(
                f,
                "`--against {name}` names no target: `bynk.toml` has no `[test.targets]`. Add `[test.targets.{name}]` with each Worker's `url`, or use `--against dev`"
            ),
            Refusal::Unknown { name, known } => write!(
                f,
                "`--against {name}` names no target under `[test.targets]`; the targets are {}",
                known
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Refusal::Production(name) => write!(
                f,
                "target `{name}` is marked `production = true`; the test runner never runs against production"
            ),
            Refusal::MissingSecrets { target, names } => write!(
                f,
                "target `{target}` needs the secret{} {} from the environment, and {} not set",
                if names.len() == 1 { "" } else { "s" },
                names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                if names.len() == 1 {
                    "it is"
                } else {
                    "they are"
                }
            ),
            Refusal::Manifest(e) => write!(f, "{e}"),
        }
    }
}

/// Whether `value` reads as a URL rather than a target name.
fn looks_like_url(value: &str) -> bool {
    value.contains("://")
        || value.starts_with("localhost")
        || value.contains('/')
        || value.parse::<std::net::SocketAddr>().is_ok()
        || value.contains(':')
}

/// Check `--against <value>` for a project rooted at `project_root`, against
/// the environment `env` (by name), and resolve the target it names.
/// `dev` is refused here with `dev_refusal`: the caller that can start `bynk
/// dev` handles it before calling this.
pub fn resolve(
    value: &str,
    project_root: &Path,
    env: impl Fn(&str) -> Option<String>,
    dev_refusal: &str,
) -> Result<RemoteRun, Refusal> {
    if value == DEV {
        return Err(Refusal::Dev(dev_refusal.to_string()));
    }
    if looks_like_url(value) {
        return Err(Refusal::BareUrl(value.to_string()));
    }
    let targets = bynk_emit::project::try_read_test_targets(project_root)
        .map_err(|e| Refusal::Manifest(e.to_string()))?;
    let Some(target) = targets.get(value) else {
        return Err(Refusal::Unknown {
            name: value.to_string(),
            known: targets.keys().cloned().collect(),
        });
    };
    check_target(value, target, env)
}

/// The checks on a named target: not production, every listed secret set.
pub fn check_target(
    name: &str,
    target: &TestTarget,
    env: impl Fn(&str) -> Option<String>,
) -> Result<RemoteRun, Refusal> {
    if target.production {
        return Err(Refusal::Production(name.to_string()));
    }
    let missing: Vec<String> = target
        .secrets
        .iter()
        .filter(|s| env(s).is_none_or(|v| v.is_empty()))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(Refusal::MissingSecrets {
            target: name.to_string(),
            names: missing,
        });
    }
    Ok(RemoteRun {
        target: name.to_string(),
        urls: target.urls.clone(),
        vars: target.vars.clone(),
        secrets: target.secrets.clone(),
        skipped: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(production: bool, secrets: &[&str]) -> TestTarget {
        TestTarget {
            urls: BTreeMap::from([("shop-orders".to_string(), "http://127.0.0.1:9".to_string())]),
            secrets: secrets.iter().map(|s| s.to_string()).collect(),
            production,
            vars: BTreeMap::new(),
        }
    }

    fn manifest(text: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bynk-remote-{}-{}", std::process::id(), text.len()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bynk.toml"), text).unwrap();
        dir
    }

    #[test]
    fn a_bare_url_is_refused() {
        for url in [
            "https://shop.example.workers.dev",
            "localhost:8787",
            "127.0.0.1:8787",
            "shop.example/x",
        ] {
            assert!(
                matches!(
                    resolve(url, Path::new("/nonexistent"), |_| None, "x"),
                    Err(Refusal::BareUrl(_))
                ),
                "{url}"
            );
        }
    }

    #[test]
    fn dev_is_reserved() {
        let r = resolve("dev", Path::new("/nonexistent"), |_| None, "is not here");
        assert_eq!(r, Err(Refusal::Dev("is not here".to_string())));
    }

    #[test]
    fn an_unknown_name_lists_the_targets() {
        let dir = manifest("[test.targets.preview]\nurl.shop-orders = \"http://127.0.0.1:9\"\n");
        let r = resolve("staging", &dir, |_| None, "x");
        assert_eq!(
            r,
            Err(Refusal::Unknown {
                name: "staging".to_string(),
                known: vec!["preview".to_string()]
            })
        );
        let ok = resolve("preview", &dir, |_| None, "x").expect("preview resolves");
        assert_eq!(ok.urls["shop-orders"], "http://127.0.0.1:9");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_production_target_is_refused() {
        assert_eq!(
            check_target("prod", &target(true, &[]), |_| None),
            Err(Refusal::Production("prod".to_string()))
        );
    }

    #[test]
    fn secrets_come_from_the_environment_and_must_be_set() {
        let t = target(false, &["AUTH_JWT_SECRET", "HOOK_SECRET"]);
        let r = check_target("preview", &t, |n| {
            (n == "AUTH_JWT_SECRET").then(|| "s".to_string())
        });
        assert_eq!(
            r,
            Err(Refusal::MissingSecrets {
                target: "preview".to_string(),
                names: vec!["HOOK_SECRET".to_string()]
            })
        );
        assert!(check_target("preview", &t, |_| Some("s".to_string())).is_ok());
    }

    #[test]
    fn the_banner_names_the_target_and_each_url() {
        let run = check_target("preview", &target(false, &[]), |_| None).unwrap();
        let banner = run.banner();
        assert!(banner.contains("target `preview`"), "{banner}");
        assert!(
            banner.contains("shop-orders  http://127.0.0.1:9"),
            "{banner}"
        );
    }
}
