//! #3477 (slice C3): `bynk dev` and containers.
//!
//! A `Container` runs under Docker on the developer's machine, started by
//! `wrangler dev` when the Worker first asks, so there is no remote call, no
//! `[dev] <kind> = "allow"` acknowledgement and no cost warning of the kind the
//! remote-only bindings print. What `bynk dev` adds:
//!
//! - **`--remote` is refused** for a Worker that has a container: remote
//!   bindings are not documented for containers, so it would be a guess.
//! - **The wrangler floor.** A container Worker needs
//!   [`bynk_emit::WRANGLER_CONTAINERS_MIN`], the first wrangler that accepts the
//!   `durable_object` scheduling policy and runs it locally; an older one is
//!   refused with the version named. Every other Worker keeps the general floor.
//! - **Docker.** With a running Docker daemon the container runs for real, and
//!   the first start builds the image, which can take minutes, so a note says
//!   which container is building. Without one, `bynk dev` warns once, naming the
//!   containers, and starts the affected Workers with containers off
//!   (`wrangler dev --enable-containers=false`, which Wrangler 4.136.0 applies to
//!   the `durable_object` policy: `ctx.container` is then absent) and
//!   [`bynk_emit::CONTAINERS_UNAVAILABLE_VAR`] set, so a call to a container
//!   faults with a message that names Docker, rather than failing as an
//!   inscrutable start. Nothing else about the session changes.

use std::collections::BTreeMap;
use std::path::Path;

use crate::probe::{self, Toolbox};

/// The served Workers that have a container, by worker directory, each with its
/// containers' capability names (`Renderer`), read from the build's
/// `bynk-containers.json`.
pub(crate) type Containers = BTreeMap<String, Vec<String>>;

/// The capability names of the containers `worker` (a directory of
/// `workers_dir`) declares, or none when it has no manifest or the manifest
/// cannot be read as the layout this build writes.
fn capabilities_of(workers_dir: &Path, worker: &str) -> Vec<String> {
    let path = workers_dir
        .join(worker)
        .join(bynk_emit::emitter::wrangler::CONTAINERS_MANIFEST);
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let mut names: Vec<String> = json["containers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["capability"].as_str().map(str::to_string))
        .collect();
    names.sort();
    names
}

/// The served Workers that declare a container.
pub(crate) fn served_containers(workers_dir: &Path, serving: &[String]) -> Containers {
    serving
        .iter()
        .filter_map(|worker| {
            let names = capabilities_of(workers_dir, worker);
            (!names.is_empty()).then(|| (worker.clone(), names))
        })
        .collect()
}

/// `... \`Renderer\` of \`shop-render\`, \`Thumbs\` of \`shop-render\``.
fn describe(containers: &Containers) -> String {
    containers
        .iter()
        .flat_map(|(worker, names)| {
            names
                .iter()
                .map(move |name| format!("`{name}` of `{worker}`"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The refusal `--remote` gets when a served Worker has a container: remote
/// bindings are not documented for containers, so Bynk does not guess. `None`
/// without `--remote` or without a container.
pub(crate) fn remote_refusal(containers: &Containers, remote: bool) -> Option<String> {
    (remote && !containers.is_empty()).then(|| {
        format!(
            "bynk: `--remote` is not supported for a Worker with a container ({}): remote bindings are not documented for containers, and `bynk dev` runs them under Docker on this machine. Run `bynk dev` without `--remote`.",
            describe(containers)
        )
    })
}

/// The refusal a served container Worker gets below
/// [`bynk_emit::WRANGLER_CONTAINERS_MIN`], with the version named. `None` when
/// no served Worker has a container or the version cannot be judged without
/// running npx.
pub(crate) fn floor_refusal(
    probe: &probe::Probe,
    manager: bynk_driver::npm_install::PackageManager,
    containers: &Containers,
) -> Option<String> {
    let version = probe.version?;
    let min = probe::Version::parse(bynk_emit::WRANGLER_CONTAINERS_MIN)?;
    if (version.major, version.minor, version.patch) >= (min.major, min.minor, min.patch) {
        return None;
    }
    let worker = containers.keys().next()?;
    Some(format!(
        "bynk: wrangler {version} is older than {}, the first that runs the `durable_object` container policy `{worker}` is configured with under `wrangler dev`. Upgrade with `{}`.",
        bynk_emit::WRANGLER_CONTAINERS_MIN,
        crate::doctor::wrangler_upgrade_remedy(probe, manager)
    ))
}

/// Whether Docker is there to run a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Docker {
    Running,
    /// The CLI is installed but no daemon answers.
    Stopped,
    /// There is no `docker` on the `PATH` (or at `WRANGLER_DOCKER_BIN`).
    Missing,
}

/// Look for Docker the way `bynk doctor` does.
pub(crate) fn docker(tb: &dyn Toolbox) -> Docker {
    let bin = crate::deploy::containers::docker_binary(
        std::env::var("WRANGLER_DOCKER_BIN").ok().as_deref(),
    );
    match crate::deploy::containers::resolve_docker(tb, &bin) {
        None => Docker::Missing,
        Some(path) if tb.docker_daemon_running(&path) => Docker::Running,
        Some(_) => Docker::Stopped,
    }
}

/// The one warning a session prints when containers are off: which containers,
/// why, what a call to one does, and how to get them back. `None` when Docker is
/// running or nothing is served that needs it.
pub(crate) fn docker_warning(docker: Docker, containers: &Containers) -> Option<String> {
    if containers.is_empty() {
        return None;
    }
    let why = match docker {
        Docker::Running => return None,
        Docker::Stopped => "no Docker daemon is running",
        Docker::Missing => "`docker` is not installed",
    };
    Some(format!(
        "bynk dev: warning: {why}, so the containers {} will not start: those Workers run with containers off, and a call to one faults, naming Docker. Start Docker and restart `bynk dev` to run them.",
        describe(containers)
    ))
}

/// The note a session prints when Docker is running: the first start builds each
/// container's image, which takes a while, and rebuilding when a Dockerfile
/// changes is Wrangler's behaviour, not Bynk's.
pub(crate) fn building_note(containers: &Containers) -> Option<String> {
    (!containers.is_empty()).then(|| {
        format!(
            "bynk dev: the containers {} run under Docker. The first start builds each image, which can take minutes; Wrangler decides when a changed Dockerfile is built again.",
            describe(containers)
        )
    })
}

/// What `bynk dev` does about containers once wrangler is resolved: refuse a
/// wrangler below the containers floor, then look for Docker. `Ok(true)` when
/// containers are off for this session (the one warning is printed here),
/// `Ok(false)` when they will run (the build note is printed), and `Err` with
/// the refusal.
pub(crate) fn prepare(
    tb: &dyn Toolbox,
    probe: &probe::Probe,
    manager: bynk_driver::npm_install::PackageManager,
    containers: &Containers,
) -> Result<bool, String> {
    if let Some(refusal) = floor_refusal(probe, manager, containers) {
        return Err(refusal);
    }
    let state = docker(tb);
    if let Some(warning) = docker_warning(state, containers) {
        eprintln!("{warning}");
    } else if let Some(note) = building_note(containers) {
        eprintln!("{note}");
    }
    Ok(state != Docker::Running && !containers.is_empty())
}

/// The served Workers' containers, or `None` after printing why not: `--remote`
/// with a container is refused ([`remote_refusal`]).
pub(crate) fn for_session(
    workers_dir: &Path,
    serving: &[crate::dev::Serving],
    wrangler_args: &[String],
) -> Option<Containers> {
    let names: Vec<String> = serving.iter().map(|s| s.worker.clone()).collect();
    let containers = served_containers(workers_dir, &names);
    let remote = crate::dev::is_remote(wrangler_args);
    if let Some(refusal) = remote_refusal(&containers, remote) {
        eprintln!("{refusal}");
        return None;
    }
    Some(containers)
}

/// [`prepare`], printing a refusal; `None` after one.
pub(crate) fn session(
    tb: &dyn Toolbox,
    probe: &probe::Probe,
    manager: bynk_driver::npm_install::PackageManager,
    containers: &Containers,
) -> Option<bool> {
    prepare(tb, probe, manager, containers)
        .map_err(|refusal| eprintln!("{refusal}"))
        .ok()
}

/// [`off_args`] for `worker` when containers are off and it has one; none
/// otherwise.
pub(crate) fn off_args_for(off: bool, containers: &Containers, worker: &str) -> Vec<String> {
    if off && containers.contains_key(worker) {
        off_args()
    } else {
        Vec::new()
    }
}

/// The `wrangler dev` arguments for a Worker whose containers are off: the flag
/// that makes wrangler start without Docker, and the variable the generated class
/// reads to fault naming Docker.
pub(crate) fn off_args() -> Vec<String> {
    vec![
        "--enable-containers=false".to_string(),
        "--var".to_string(),
        format!("{}:docker", bynk_emit::CONTAINERS_UNAVAILABLE_VAR),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn containers() -> Containers {
        BTreeMap::from([(
            "shop-render".to_string(),
            vec!["Renderer".to_string(), "Thumbs".to_string()],
        )])
    }

    struct NoDocker;

    impl Toolbox for NoDocker {
        fn on_path(&self, _: &str) -> Option<PathBuf> {
            None
        }
        fn in_dir(&self, _: &Path, _: &str) -> Option<PathBuf> {
            None
        }
        fn version(&self, _: &Path) -> Option<probe::Version> {
            None
        }
        fn npx_available(&self) -> bool {
            false
        }
    }

    #[test]
    fn a_session_without_docker_turns_containers_off_for_the_container_worker_only() {
        let probe = probe::Probe {
            tool: "wrangler".to_string(),
            version: probe::Version::parse("4.140.0"),
            provenance: probe::Provenance::Path(PathBuf::from("/bin/wrangler")),
        };
        let manager = bynk_driver::npm_install::PackageManager::Npm;
        let with = containers();
        assert_eq!(session(&NoDocker, &probe, manager, &with), Some(true));
        assert!(!off_args_for(true, &with, "shop-render").is_empty());
        assert!(off_args_for(true, &with, "shop-other").is_empty());
        assert_eq!(
            session(&NoDocker, &probe, manager, &Containers::new()),
            Some(false)
        );
    }

    #[test]
    fn remote_is_recognised_in_its_alias_forms() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for yes in ["--remote", "-r", "--remote=true"] {
            assert!(crate::dev::is_remote(&args(&[yes])), "{yes}");
        }
        assert!(!crate::dev::is_remote(&args(&[
            "--port=1",
            "--remote=false"
        ])));
    }

    #[test]
    fn remote_is_refused_only_with_a_container() {
        let refusal = remote_refusal(&containers(), true).unwrap();
        assert!(refusal.contains("`--remote`") && refusal.contains("`Renderer` of `shop-render`"));
        assert_eq!(remote_refusal(&containers(), false), None);
        assert_eq!(remote_refusal(&Containers::new(), true), None);
    }

    #[test]
    fn docker_is_warned_about_once_and_only_when_it_is_needed() {
        let missing = docker_warning(Docker::Missing, &containers()).unwrap();
        assert!(missing.contains("`docker` is not installed"), "{missing}");
        assert!(missing.contains("`Thumbs` of `shop-render`"), "{missing}");
        assert!(missing.contains("naming Docker"), "{missing}");
        let stopped = docker_warning(Docker::Stopped, &containers()).unwrap();
        assert!(stopped.contains("no Docker daemon is running"), "{stopped}");
        assert_eq!(docker_warning(Docker::Running, &containers()), None);
        assert_eq!(docker_warning(Docker::Missing, &Containers::new()), None);
        // No price, allowance or remote-call wording: containers run locally.
        assert!(!missing.contains('$') && !missing.to_lowercase().contains("billed"));
    }

    #[test]
    fn a_running_docker_notes_the_first_build() {
        let note = building_note(&containers()).unwrap();
        assert!(note.contains("can take minutes"), "{note}");
        assert_eq!(building_note(&Containers::new()), None);
    }

    #[test]
    fn containers_off_is_a_flag_and_a_variable() {
        let args = off_args();
        assert_eq!(args[0], "--enable-containers=false");
        assert_eq!(args[1], "--var");
        assert_eq!(args[2], "BYNK_CONTAINERS_UNAVAILABLE:docker");
    }

    #[test]
    fn the_manifest_names_the_containers_a_worker_declares() {
        let dir = std::env::temp_dir().join(format!("bynk-dev-containers-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("shop-render")).unwrap();
        std::fs::create_dir_all(dir.join("shop-plain")).unwrap();
        std::fs::write(
            dir.join("shop-render")
                .join(bynk_emit::emitter::wrangler::CONTAINERS_MANIFEST),
            "{ \"version\": 1, \"containers\": [{ \"capability\": \"Thumbs\", \"class\": \"__Container_Thumbs\", \"build\": \"c\", \"instance_type\": null }, { \"capability\": \"Renderer\", \"class\": \"__Container_Renderer\", \"build\": \"c\", \"instance_type\": null }] }",
        )
        .unwrap();
        let found = served_containers(
            &dir,
            &[
                "shop-render".to_string(),
                "shop-plain".to_string(),
                "absent".to_string(),
            ],
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found["shop-render"], ["Renderer", "Thumbs"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
