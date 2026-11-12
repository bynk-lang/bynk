//! #3319: `bynk dev` hands each `Postgres` instance its local connection string.
//!
//! `wrangler dev` does not reach Hyperdrive. It connects to a database of your
//! own, named by `CLOUDFLARE_HYPERDRIVE_LOCAL_CONNECTION_STRING_<BINDING>`.
//! `bynk.toml` says which environment variable holds that string for each
//! capability, never the string:
//!
//! ```toml
//! [dev.hyperdrive.Main]
//! connection_env = "MAIN_LOCAL_DATABASE_URL"
//! ```
//!
//! The value goes into the environment of that Worker's `wrangler dev` and
//! nowhere else (R-16): not argv, not a file under `out/`, not a log line.

use std::collections::BTreeMap;
use std::path::Path;

/// The prefix of the environment variable `wrangler dev` reads a Hyperdrive
/// binding's local connection string from.
pub(crate) const LOCAL_CONNECTION_STRING_PREFIX: &str =
    "CLOUDFLARE_HYPERDRIVE_LOCAL_CONNECTION_STRING_";

/// One environment variable to set on a `wrangler dev`. Its `Debug` hides the
/// value, so a stray `{:?}` cannot print a connection string.
pub(crate) struct LocalConnection {
    pub(crate) name: String,
    pub(crate) value: String,
}

impl std::fmt::Debug for LocalConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}=<redacted>", self.name)
    }
}

/// The variables to set on each Worker's `wrangler dev`, by Worker.
pub(crate) type LocalConnections = BTreeMap<String, Vec<LocalConnection>>;

/// The variables to set on each served Worker's `wrangler dev`, by Worker.
///
/// A binding whose capability `[dev.hyperdrive]` names, but whose variable is
/// not set, is an error naming both: wrangler's own would not. A binding with
/// no `[dev.hyperdrive]` entry is left to a
/// `CLOUDFLARE_HYPERDRIVE_LOCAL_CONNECTION_STRING_<BINDING>` already in the
/// environment (or a `localConnectionString` the author wrote), and a
/// warning is returned when neither is there.
pub(crate) fn local_connections(
    project_root: &Path,
    workers_dir: &Path,
    workers: &[String],
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(LocalConnections, Vec<String>), String> {
    let mut out = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut settings = None;
    for worker in workers {
        let config = std::fs::read_to_string(workers_dir.join(worker).join("wrangler.toml"))
            .unwrap_or_default();
        let bindings = bynk_emit::emitter::wrangler::hyperdrive_entries(&config)?;
        if bindings.is_empty() {
            continue;
        }
        if settings.is_none() {
            settings = Some(
                bynk_project::hyperdrive::try_read_hyperdrive_dev(project_root)
                    .map_err(|e| e.to_string())?,
            );
        }
        let settings = settings.as_ref().expect("read above");
        let mut vars = Vec::new();
        for (binding, _) in bindings {
            let name = format!("{LOCAL_CONNECTION_STRING_PREFIX}{binding}");
            let configured = settings.iter().find(|(capability, _)| {
                bynk_check::worker_bindings::instance_binding_name(
                    bynk_check::firstparty::POSTGRES_BINDING_PREFIX,
                    capability,
                ) == binding
            });
            match configured {
                Some((capability, variable)) => {
                    let Some(value) = lookup(variable).filter(|v| !v.trim().is_empty()) else {
                        return Err(format!(
                            "the environment variable `{variable}` is not set, and `[dev.hyperdrive.{capability}]` reads the local connection string of `{binding}` (`{worker}`) from it"
                        ));
                    };
                    vars.push(LocalConnection { name, value });
                }
                None if lookup(&name).is_none() => warnings.push(format!(
                    "`{binding}` of `{worker}` has no local connection string: add `[dev.hyperdrive.<Capability>]` with a `connection_env` to bynk.toml, or set {name}"
                )),
                None => {}
            }
        }
        out.insert(worker.clone(), vars);
    }
    Ok((out, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: &str = "postgres://dev:l0cal-Pw-77@127.0.0.1:5432/shop_dev";

    fn project(manifest: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bynk-dev-hyperdrive-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let worker = root.join("workers").join("shop");
        std::fs::create_dir_all(&worker).unwrap();
        std::fs::write(root.join("bynk.toml"), manifest).unwrap();
        std::fs::write(
            worker.join("wrangler.toml"),
            "name = \"shop\"\n\n[[hyperdrive]]\nbinding = \"HYPERDRIVE_MAIN\"\nid = \"<HYPERDRIVE_MAIN_ID>\"\n",
        )
        .unwrap();
        root
    }

    fn workers() -> Vec<String> {
        vec!["shop".to_string()]
    }

    #[test]
    fn each_binding_gets_its_local_string_from_the_variable_bynk_toml_names() {
        let root = project("[dev.hyperdrive.Main]\nconnection_env = \"MAIN_LOCAL_URL\"\n");
        let lookup = |name: &str| (name == "MAIN_LOCAL_URL").then(|| LOCAL.to_string());
        let (vars, warnings) =
            local_connections(&root, &root.join("workers"), &workers(), &lookup).unwrap();
        assert!(warnings.is_empty());
        let shop = &vars["shop"];
        assert_eq!(shop.len(), 1);
        assert_eq!(
            shop[0].name,
            "CLOUDFLARE_HYPERDRIVE_LOCAL_CONNECTION_STRING_HYPERDRIVE_MAIN"
        );
        assert_eq!(shop[0].value, LOCAL);
        assert!(
            !format!("{shop:?}").contains("l0cal-Pw-77"),
            "Debug hides it"
        );
    }

    #[test]
    fn a_named_but_unset_variable_fails_naming_it_and_the_capability_not_the_value() {
        let root = project("[dev.hyperdrive.Main]\nconnection_env = \"MAIN_LOCAL_URL\"\n");
        let e = local_connections(&root, &root.join("workers"), &workers(), &|_| None).unwrap_err();
        assert!(e.contains("`MAIN_LOCAL_URL` is not set"), "{e}");
        assert!(
            e.contains("`[dev.hyperdrive.Main]`") && e.contains("`HYPERDRIVE_MAIN`"),
            "{e}"
        );
    }

    #[test]
    fn an_unconfigured_binding_warns_unless_wrangler_has_the_variable_already() {
        let root = project("[project]\nname = \"p\"\n");
        let (vars, warnings) =
            local_connections(&root, &root.join("workers"), &workers(), &|_| None).unwrap();
        assert!(vars["shop"].is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("[dev.hyperdrive.<Capability>]"),
            "{warnings:?}"
        );

        let set = |name: &str| {
            (name == "CLOUDFLARE_HYPERDRIVE_LOCAL_CONNECTION_STRING_HYPERDRIVE_MAIN")
                .then(|| LOCAL.to_string())
        };
        let (_, warnings) =
            local_connections(&root, &root.join("workers"), &workers(), &set).unwrap();
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_worker_without_postgres_is_untouched() {
        let root = project("");
        std::fs::write(root.join("workers/shop/wrangler.toml"), "name = \"shop\"\n").unwrap();
        let (vars, warnings) =
            local_connections(&root, &root.join("workers"), &workers(), &|_| None).unwrap();
        assert!(vars.is_empty() && warnings.is_empty());
    }

    #[test]
    fn no_message_prints_the_local_string() {
        let root = project("[dev.hyperdrive.Main]\nconnection_env = \"MAIN_LOCAL_URL\"\n");
        let lookup = |_: &str| Some(LOCAL.to_string());
        let (_, warnings) =
            local_connections(&root, &root.join("workers"), &workers(), &lookup).unwrap();
        assert!(warnings.iter().all(|w| !w.contains("l0cal-Pw-77")));
    }
}
