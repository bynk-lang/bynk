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

/// Internal: the `bynk` binary that can start `bynk dev`, set by `bynk test
/// --against dev` for the `bynkc` it delegates to. Without it, `bynkc test
/// --against dev` is refused.
pub const DEV_COMMAND_ENV: &str = "BYNK_DEV_COMMAND";

/// Internal: the URLs of a running `bynk dev`, by worker, for one group's run
/// (set by the `--against dev` orchestration). Every one must be loopback.
pub const DEV_URLS_ENV: &str = "BYNK_TEST_DEV_URLS";

/// Internal: the `env` map of the cases one `bynk dev` serves.
pub const DEV_ENV_ENV: &str = "BYNK_TEST_DEV_ENV";

/// Internal: the secret values one `bynk dev` serves, by name, which the
/// runner signs with. One JSON variable, so a name such as `PATH` never
/// becomes a real environment variable of the runner.
pub const DEV_SECRETS_ENV: &str = "BYNK_TEST_DEV_SECRETS";

/// The variable the remote harness reads every secret value from, by name.
pub const SECRETS_ENV: &str = "BYNK_TEST_SECRETS";

/// The variable `run.id` reads; unset in process, where it is `"local"`.
pub const RUN_ID_ENV: &str = "BYNK_TEST_RUN_ID";

/// A fresh `run.id` for a remote run: the time in milliseconds and the
/// process id, in hex, so two runs don't collide on the same deployment.
pub fn fresh_run_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("r{millis:x}{:x}", std::process::id())
}

/// `--settle`'s default: a failing `expect` in a remote run is retried for up
/// to five seconds (track §3.6).
pub const DEFAULT_SETTLE_MS: u64 = 5_000;

/// Parse a `--settle` duration (`5s`, `500ms`, `2m`, `0`) to milliseconds.
pub fn parse_settle(value: &str) -> Result<u64, String> {
    let bad =
        || format!("`--settle {value}` is not a duration; write one such as `5s`, `500ms` or `0`");
    let v = value.trim();
    if v == "0" {
        return Ok(0);
    }
    let (digits, factor) = if let Some(n) = v.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = v.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = v.strip_suffix('m') {
        (n, 60_000)
    } else {
        return Err(bad());
    };
    digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(factor))
        .ok_or_else(bad)
}

/// What `--against` asks for, once it passed the safety checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Against {
    /// Run against this target now.
    Run(RemoteRun),
    /// `--against dev` from `bynk test`: start `bynk dev` with this `bynk`
    /// binary, once per distinct `env`, and run each group against it.
    StartDev(std::path::PathBuf),
}

/// Whether `url` is plain HTTP on a loopback host, the only kind of URL a
/// `bynk dev` run accepts.
pub fn is_loopback_http(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let host = match authority.rsplit_once(':') {
        Some((h, port)) if !h.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    };
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

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
    /// Under `--against dev`, the `env` map of the cases this run covers.
    pub dev_env: Option<BTreeMap<String, String>>,
    /// The values the drivers sign with, by secret name: a named target's
    /// from the environment, a `bynk dev` group's from the orchestration.
    pub secret_values: BTreeMap<String, String>,
    /// `--settle`: how long a failing `expect` is retried, in milliseconds.
    pub settle_ms: u64,
    /// The `system` cases the build skipped (`--skip-unsupported`), with the
    /// suite each belongs to.
    pub skipped: Vec<(String, RemoteSkip)>,
}

impl RemoteRun {
    /// `BYNK_TEST_TARGETS`: the URLs as the emitted remote harness reads them.
    pub fn targets_json(&self) -> String {
        serde_json::to_string(&self.urls).expect("a string map serialises")
    }

    /// `BYNK_TEST_SECRETS`: the secret values as the remote harness reads them.
    pub fn secrets_json(&self) -> String {
        serde_json::to_string(&self.secret_values).expect("a string map serialises")
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
    /// `--against dev` from a front end that can't start `bynk dev`.
    Dev(String),
    /// A `bynk dev` URL that isn't plain HTTP on loopback.
    NotLoopback(String),
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
            Refusal::NotLoopback(url) => write!(
                f,
                "`--against dev` runs only against a local `bynk dev`, but was given `{url}`, which isn't plain HTTP on localhost"
            ),
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
/// the environment `env` (by name), and resolve what it asks for.
///
/// `dev` is a run against a local `bynk dev`: one group's run when
/// [`DEV_URLS_ENV`] names the running Workers (each URL must be loopback),
/// the orchestration when [`DEV_COMMAND_ENV`] names a `bynk` that can start
/// it, and otherwise refused with `dev_refusal`.
pub fn resolve(
    value: &str,
    project_root: &Path,
    env: impl Fn(&str) -> Option<String>,
    dev_refusal: &str,
) -> Result<Against, Refusal> {
    if value == DEV {
        if let Some(urls) = env(DEV_URLS_ENV) {
            let urls: BTreeMap<String, String> = serde_json::from_str(&urls).map_err(|e| {
                Refusal::Manifest(format!("{DEV_URLS_ENV} is not a JSON object: {e}"))
            })?;
            if let Some(bad) = urls.values().find(|u| !is_loopback_http(u)) {
                return Err(Refusal::NotLoopback(bad.clone()));
            }
            let json_map = |name: &str| -> Result<BTreeMap<String, String>, Refusal> {
                match env(name) {
                    Some(e) => serde_json::from_str(&e).map_err(|e| {
                        Refusal::Manifest(format!("{name} is not a JSON object: {e}"))
                    }),
                    None => Ok(BTreeMap::new()),
                }
            };
            let dev_env = json_map(DEV_ENV_ENV)?;
            let secret_values = json_map(DEV_SECRETS_ENV)?;
            return Ok(Against::Run(RemoteRun {
                target: DEV.to_string(),
                urls,
                vars: BTreeMap::new(),
                secrets: Vec::new(),
                dev_env: Some(dev_env),
                secret_values,
                settle_ms: DEFAULT_SETTLE_MS,
                skipped: Vec::new(),
            }));
        }
        if let Some(bynk) = env(DEV_COMMAND_ENV).filter(|p| !p.is_empty()) {
            return Ok(Against::StartDev(bynk.into()));
        }
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
    check_target(value, target, env).map(Against::Run)
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
    let mut secret_values = BTreeMap::new();
    let mut missing: Vec<String> = Vec::new();
    for name in &target.secrets {
        match env(name).filter(|v| !v.is_empty()) {
            Some(value) => {
                secret_values.insert(name.clone(), value);
            }
            None => missing.push(name.clone()),
        }
    }
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
        dev_env: None,
        secret_values,
        settle_ms: DEFAULT_SETTLE_MS,
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
    fn settle_durations_parse() {
        assert_eq!(parse_settle("0"), Ok(0));
        assert_eq!(parse_settle("500ms"), Ok(500));
        assert_eq!(parse_settle("5s"), Ok(5_000));
        assert_eq!(parse_settle("2m"), Ok(120_000));
        for bad in ["5", "fast", "-1s", "1.5s", "ms"] {
            assert!(parse_settle(bad).is_err(), "{bad}");
        }
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
    fn dev_urls_must_be_loopback() {
        let env = |urls: &'static str| move |n: &str| (n == DEV_URLS_ENV).then(|| urls.to_string());
        let ok = resolve(
            "dev",
            Path::new("/x"),
            env(r#"{"a":"http://localhost:8787","b":"http://127.0.0.1:9"}"#),
            "x",
        );
        assert!(
            matches!(ok, Ok(Against::Run(ref r)) if r.dev_env.is_some()),
            "{ok:?}"
        );
        for bad in [
            r#"{"a":"https://localhost:8787"}"#,
            r#"{"a":"http://shop.example.workers.dev"}"#,
            r#"{"a":"http://localhost.example.com:80"}"#,
        ] {
            assert!(
                matches!(
                    resolve("dev", Path::new("/x"), env(bad), "x"),
                    Err(Refusal::NotLoopback(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_dev_group_s_secrets_arrive_in_one_json_variable() {
        let env = |n: &str| match n {
            DEV_URLS_ENV => Some(r#"{"a":"http://localhost:8787"}"#.to_string()),
            DEV_SECRETS_ENV => Some(r#"{"PATH":"not-a-path","AUTH":"s"}"#.to_string()),
            _ => None,
        };
        let Ok(Against::Run(run)) = resolve("dev", Path::new("/x"), env, "x") else {
            panic!("a dev group run resolves");
        };
        // A secret named `PATH` is only ever a value for the drivers.
        assert_eq!(run.secret_values["PATH"], "not-a-path");
        assert!(run.secrets_json().contains("\"AUTH\":\"s\""));
    }

    #[test]
    fn dev_starts_only_with_a_bynk_that_can() {
        let r = resolve(
            "dev",
            Path::new("/x"),
            |n| (n == DEV_COMMAND_ENV).then(|| "/bin/bynk".to_string()),
            "x",
        );
        assert_eq!(r, Ok(Against::StartDev("/bin/bynk".into())));
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
        let Ok(Against::Run(ok)) = resolve("preview", &dir, |_| None, "x") else {
            panic!("preview resolves");
        };
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
