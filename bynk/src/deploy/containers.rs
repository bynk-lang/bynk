//! #3477 (slice C2): Containers at deploy.
//!
//! A `Container` is a Durable Object class backed by an image, and the
//! application is created and updated by `wrangler deploy` from the
//! `[[containers]]` stanza the build wrote (Decision C): Bynk neither pushes the
//! image nor talks to Cloudflare's registry. What the deploy adds around that:
//!
//! - **Adoption.** `[deploy.containers.<Capability>] image = { production = "…" }`
//!   names a digest-pinned image in Cloudflare's registry that an environment
//!   runs instead of building one. The generated `[containers.images.<Cap>]`
//!   entry is rewritten to that `image`, so `wrangler` builds nothing for it,
//!   and the reference is recorded in the ledger.
//! - **Docker.** Building an image needs a running Docker daemon, so a deploy
//!   that has an image to build checks for one before it authenticates or asks
//!   for confirmation, and names the container, the missing tool and the two
//!   ways out. A deploy whose containers are all adopted needs none.
//! - **The floor.** A container Worker needs wrangler
//!   [`bynk_emit::WRANGLER_CONTAINERS_MIN`]; the floor everyone else needs
//!   ([`bynk_emit::WRANGLER_MIN`]) does not move.
//!
//! Bynk's `--dry-run` shows the plan and builds nothing: it never delegates to
//! `wrangler deploy --dry-run`, which builds a Dockerfile image.

use super::*;
use std::path::PathBuf;

/// Name what `bynk.toml` adopts for `environment` on the declarations: the
/// Vectorize indexes, then the container images.
pub(crate) fn apply_adoptions(
    project_root: &Path,
    resources: &mut BTreeMap<String, Resources>,
    environment: &str,
) -> Result<(), ()> {
    apply_vectorize_adoptions(project_root, resources, environment)?;
    apply_container_adoptions(project_root, resources, environment)
}

/// Name the images `bynk.toml` adopts for `environment` on the declarations
/// (`[deploy.containers.<Capability>] image`). A capability with no entry for
/// the environment is built from its Dockerfile. Prints why and fails when
/// `bynk.toml` cannot be read or an entry names no container of this project.
pub(crate) fn apply_container_adoptions(
    project_root: &Path,
    resources: &mut BTreeMap<String, Resources>,
    environment: &str,
) -> Result<(), ()> {
    if resources.values().all(|r| r.containers.is_empty()) {
        // An entry for a container the project no longer declares is not an
        // error when there are none at all: nothing would build either way.
        let settings = bynk_project::containers::try_read_containers_deploy(project_root)
            .map_err(|e| eprintln!("bynk: {e}"))?;
        if let Some(capability) = settings.keys().next() {
            eprintln!(
                "bynk: `[deploy.containers.{capability}]` in bynk.toml names no `Container` capability in this project"
            );
            return Err(());
        }
        return Ok(());
    }
    let settings = bynk_project::containers::try_read_containers_deploy(project_root)
        .map_err(|e| eprintln!("bynk: {e}"))?;
    // An entry that names no declared capability would be silently ignored,
    // and the deploy would build an image the team meant to adopt.
    let capabilities: Vec<&str> = resources
        .values()
        .flat_map(|r| r.containers.iter().map(|c| c.capability.as_str()))
        .collect();
    for capability in settings.keys() {
        if capabilities.contains(&capability.as_str()) {
            continue;
        }
        let hint = bynk_project::paths::did_you_mean(capability, capabilities.iter().copied())
            .map(|c| format!("; did you mean `{c}`?"))
            .unwrap_or_default();
        eprintln!(
            "bynk: `[deploy.containers.{capability}]` in bynk.toml names no `Container` capability in this project{hint}; an entry that matches nothing would have the deploy build an image you meant to adopt"
        );
        return Err(());
    }
    for (capability, entry) in &settings {
        if !entry.image.contains_key(environment) {
            let named: Vec<&str> = entry.image.keys().map(String::as_str).collect();
            eprintln!(
                "bynk: note: `[deploy.containers.{capability}]` adopts an image only for {}; under `{environment}` the deploy builds one",
                named.join(", ")
            );
        }
    }
    for declared in resources.values_mut() {
        for container in &mut declared.containers {
            container.adopt = settings
                .get(&container.capability)
                .and_then(|entry| entry.image.get(environment))
                .cloned();
        }
    }
    Ok(())
}

/// What a deploy checks for its containers before it authenticates: a wrangler
/// new enough for the policy ([`containers_floor_refusal`]), then Docker for an
/// image to build ([`docker_refusal`]). `None` when there is nothing to refuse.
pub(crate) fn container_refusals(
    tb: &dyn Toolbox,
    probe: &probe::Probe,
    manager: bynk_driver::npm_install::PackageManager,
    order: &[String],
    resources: &BTreeMap<String, Resources>,
) -> Option<String> {
    containers_floor_refusal(probe, manager, order, resources).or_else(|| {
        let bin = docker_binary(std::env::var("WRANGLER_DOCKER_BIN").ok().as_deref());
        let path = resolve_docker(tb, &bin);
        let docker = path
            .as_deref()
            .map(|path| (path, tb.docker_daemon_running(path)));
        docker_refusal(docker, order, resources)
    })
}

/// The container engine CLI wrangler will run: `WRANGLER_DOCKER_BIN` when it is
/// set and not blank (wrangler 4.136.0's own override, for Podman or a Docker
/// off the `PATH`), else `docker`.
fn docker_binary(over: Option<&str>) -> String {
    over.map(str::trim)
        .filter(|bin| !bin.is_empty())
        .unwrap_or("docker")
        .to_string()
}

/// Resolves the engine CLI: a path (anything with a separator) as given when it
/// exists, a bare name on the `PATH`.
fn resolve_docker(tb: &dyn Toolbox, bin: &str) -> Option<PathBuf> {
    if bin.contains(['/', '\\']) {
        return Path::new(bin).is_file().then(|| PathBuf::from(bin));
    }
    tb.on_path(bin)
}

/// The refusal a deploy gets when a container has an image to build and Docker
/// is not there to build it. `docker` is the resolved CLI and whether its daemon
/// answers: `None` when there is no `docker` on the `PATH`. Names the first
/// container that would be built, the tool and the two ways out. `None` when
/// every container is adopted, or Docker is running.
pub(crate) fn docker_refusal(
    docker: Option<(&Path, bool)>,
    order: &[String],
    resources: &BTreeMap<String, Resources>,
) -> Option<String> {
    let (worker, container) = order.iter().find_map(|worker| {
        resources[worker]
            .containers
            .iter()
            .find(|c| c.adopt.is_none())
            .map(|c| (worker, c))
    })?;
    let problem = match docker {
        Some((_, true)) => return None,
        Some((_, false)) => "no Docker daemon is running",
        None => "`docker` is not installed",
    };
    Some(format!(
        "bynk: `{worker}`'s container `{}` is built from `{}/Dockerfile` at deploy, which needs Docker, and {problem}.\n  Start Docker, or run a prebuilt image instead: push it to Cloudflare's registry and name it in bynk.toml,\n    [deploy.containers.{}]\n    image = {{ <environment> = \"registry.cloudflare.com/<ACCOUNT_ID>/<REPOSITORY>@sha256:<DIGEST>\" }}",
        container.capability, container.build, container.capability
    ))
}

/// The refusal a deploy gets when a Worker declares a container and the wrangler
/// that would push it is older than [`bynk_emit::WRANGLER_CONTAINERS_MIN`], which
/// refuses the `durable_object` scheduling policy the generated configuration
/// uses. Only a container Worker is held to it. `None` when no Worker has a
/// container, or when the version cannot be judged without running npx.
pub(crate) fn containers_floor_refusal(
    probe: &probe::Probe,
    manager: bynk_driver::npm_install::PackageManager,
    order: &[String],
    resources: &BTreeMap<String, Resources>,
) -> Option<String> {
    let version = probe.version?;
    let min = probe::Version::parse(bynk_emit::WRANGLER_CONTAINERS_MIN)?;
    if (version.major, version.minor, version.patch) >= (min.major, min.minor, min.patch) {
        return None;
    }
    let worker = order
        .iter()
        .find(|worker| !resources[*worker].containers.is_empty())?;
    Some(format!(
        "bynk: wrangler {version} is older than {}, the first that accepts the `durable_object` scheduling policy the container of `{worker}` is configured with; the push would fail. Upgrade with `{}`.",
        bynk_emit::WRANGLER_CONTAINERS_MIN,
        doctor::wrangler_upgrade_remedy(probe, manager)
    ))
}

/// An adopted image is named in the generated configuration for `environment`
/// (replacing the Dockerfile it would have built), so `wrangler` builds nothing
/// for it. Nothing is written when no container is adopted.
pub(crate) fn materialise_adopted_containers(
    config: &Path,
    worker: &str,
    declared: &Resources,
    environment: &str,
) -> Result<(), DeployFailure> {
    let adopted: BTreeMap<String, String> = declared
        .containers
        .iter()
        .filter_map(|c| c.adopt.clone().map(|image| (c.capability.clone(), image)))
        .collect();
    if adopted.is_empty() {
        return Ok(());
    }
    let text = std::fs::read_to_string(config).map_err(|e| {
        DeployFailure::driver(format!(
            "could not read `{worker}`'s generated configuration: {e}"
        ))
    })?;
    let patched = bynk_emit::emitter::wrangler::materialise_adopted_container_images(
        &text,
        environment,
        &adopted,
    )
    .map_err(|e| {
        DeployFailure::driver(format!(
            "could not name the adopted container images in `{worker}`'s configuration: {e}"
        ))
    })?;
    std::fs::write(config, patched).map_err(|e| {
        DeployFailure::driver(format!(
            "could not write `{worker}`'s adopted container images: {e}"
        ))
    })
}

/// An adopted container image replaces the Dockerfile `wrangler` would build,
/// in the configuration it is about to read, and is recorded before the push.
pub(crate) fn adopt_containers_for_push(
    config: &Path,
    worker: &str,
    declared: &Resources,
    lock: &mut DeployLock,
    lock_path: &Path,
    environment: &str,
) -> Result<(), DeployFailure> {
    materialise_adopted_containers(config, worker, declared, environment)?;
    if declared.containers.is_empty() {
        return Ok(());
    }
    record_container_images(lock, worker, declared, environment);
    write_lock(lock_path, lock).map_err(|e| {
        DeployFailure::driver(format!(
            "could not record the container images of `{worker}` in {}: {e}",
            lock_path.display()
        ))
    })
}

/// Record each adopted image of `worker` in the ledger, and forget one whose
/// adoption was dropped (its container is built again). Written before the push,
/// as the other provisioned facts are.
pub(crate) fn record_container_images(
    lock: &mut DeployLock,
    worker: &str,
    declared: &Resources,
    environment: &str,
) {
    for container in &declared.containers {
        lock.record_container_image(
            environment,
            worker,
            &container.capability,
            container.adopt.as_deref(),
        );
    }
}

/// Fixtures the plan's tests share.
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    pub(crate) fn decl(
        capability: &str,
        instance_type: Option<&str>,
        adopt: Option<&str>,
    ) -> ContainerDecl {
        ContainerDecl {
            capability: capability.to_string(),
            class: format!("__Container_{capability}"),
            build: "containers/render".to_string(),
            instance_type: instance_type.map(str::to_string),
            adopt: adopt.map(str::to_string),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::decl;
    use super::*;

    const IMAGE: &str = "registry.cloudflare.com/abc123/renderer@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn resources(containers: Vec<ContainerDecl>) -> BTreeMap<String, Resources> {
        BTreeMap::from([(
            "shop-render".to_string(),
            Resources {
                containers,
                ..Resources::default()
            },
        )])
    }

    fn order() -> Vec<String> {
        vec!["shop-render".to_string()]
    }

    fn project(manifest_tail: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bynk-containers-{}-{}",
            std::process::id(),
            manifest_tail.len()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("bynk.toml"),
            format!("[project]\nname = \"shop\"\nversion = \"0.1.0\"\n\n{manifest_tail}"),
        )
        .unwrap();
        dir
    }

    #[test]
    fn bynk_toml_adopts_per_environment_by_capability() {
        let dir = project(&format!(
            "[deploy.containers.Renderer]\nimage = {{ production = \"{IMAGE}\" }}\n"
        ));
        let mut all = resources(vec![
            decl("Renderer", None, None),
            decl("Thumbs", None, None),
        ]);
        apply_container_adoptions(&dir, &mut all, "production").unwrap();
        assert_eq!(
            all["shop-render"].containers[0].adopt.as_deref(),
            Some(IMAGE)
        );
        assert_eq!(all["shop-render"].containers[1].adopt, None);
        // Another environment builds, and says so.
        apply_container_adoptions(&dir, &mut all, "staging").unwrap();
        assert_eq!(all["shop-render"].containers[0].adopt, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_entry_that_names_no_container_is_refused() {
        let dir = project(&format!(
            "[deploy.containers.Rendrer]\nimage = {{ production = \"{IMAGE}\" }}\n"
        ));
        let mut all = resources(vec![decl("Renderer", None, None)]);
        assert!(apply_container_adoptions(&dir, &mut all, "production").is_err());
        // Nor is one tolerated in a project with no container at all.
        let mut none = resources(Vec::new());
        assert!(apply_container_adoptions(&dir, &mut none, "production").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn docker_is_needed_only_for_an_image_that_is_built() {
        let built = resources(vec![decl("Renderer", None, None)]);
        let docker = std::path::Path::new("/usr/bin/docker");
        assert_eq!(docker_refusal(Some((docker, true)), &order(), &built), None);
        let stopped = docker_refusal(Some((docker, false)), &order(), &built).unwrap();
        assert!(stopped.contains("no Docker daemon is running"), "{stopped}");
        assert!(
            stopped.contains("`shop-render`'s container `Renderer`"),
            "{stopped}"
        );
        assert!(
            stopped.contains("containers/render/Dockerfile"),
            "{stopped}"
        );
        let missing = docker_refusal(None, &order(), &built).unwrap();
        assert!(missing.contains("`docker` is not installed"), "{missing}");
        // Both ways out are named: start Docker, or adopt a prebuilt image.
        assert!(missing.contains("Start Docker"), "{missing}");
        assert!(
            missing.contains("[deploy.containers.Renderer]"),
            "{missing}"
        );
        // Adopting every container needs no Docker at all.
        let adopted = resources(vec![decl("Renderer", None, Some(IMAGE))]);
        assert_eq!(docker_refusal(None, &order(), &adopted), None);
        // And a Worker with no container never asks.
        assert_eq!(docker_refusal(None, &order(), &resources(Vec::new())), None);
        // A mix names the one that is built.
        let mixed = resources(vec![
            decl("Renderer", None, Some(IMAGE)),
            decl("Thumbs", None, None),
        ]);
        assert!(
            docker_refusal(None, &order(), &mixed)
                .unwrap()
                .contains("`Thumbs`")
        );
    }

    fn wrangler(version: Option<probe::Version>) -> probe::Probe {
        probe::Probe {
            tool: "wrangler".to_string(),
            version,
            provenance: Provenance::Path("/usr/bin/wrangler".into()),
        }
    }

    fn v(major: u32, minor: u32, patch: u32) -> probe::Version {
        probe::Version {
            major,
            minor,
            patch,
        }
    }

    #[test]
    fn a_container_worker_needs_the_containers_floor_and_no_other_does() {
        let manager = bynk_driver::npm_install::PackageManager::Npm;
        let with = resources(vec![decl("Renderer", None, None)]);
        let refusal =
            containers_floor_refusal(&wrangler(Some(v(4, 120, 1))), manager, &order(), &with)
                .unwrap();
        assert!(refusal.contains("4.136.0"), "{refusal}");
        assert!(refusal.contains("`shop-render`"), "{refusal}");
        assert!(refusal.contains("npm install -g wrangler@4"), "{refusal}");
        for ok in [v(4, 136, 0), v(4, 140, 2), v(5, 0, 0)] {
            assert_eq!(
                containers_floor_refusal(&wrangler(Some(ok)), manager, &order(), &with),
                None,
                "{ok}"
            );
        }
        // A Worker with no container is held to the lower floor only: the one the
        // `exports` check applies, not this one.
        let without = resources(Vec::new());
        assert_eq!(
            containers_floor_refusal(&wrangler(Some(v(4, 120, 1))), manager, &order(), &without),
            None
        );
        // A version that cannot be judged without running npx is not guessed.
        assert_eq!(
            containers_floor_refusal(&wrangler(None), manager, &order(), &with),
            None
        );
        assert_eq!(
            bynk_emit::WRANGLER_MIN,
            "4.120.1",
            "the general floor does not move"
        );
    }

    #[test]
    fn the_engine_cli_is_wranglers_override_or_docker() {
        assert_eq!(docker_binary(None), "docker");
        assert_eq!(docker_binary(Some("  ")), "docker");
        assert_eq!(docker_binary(Some(" podman ")), "podman");
    }

    struct Stub {
        daemon: bool,
    }

    impl Toolbox for Stub {
        fn on_path(&self, tool: &str) -> Option<PathBuf> {
            (tool == "docker" || tool == "podman").then(|| PathBuf::from(format!("/bin/{tool}")))
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
        fn docker_daemon_running(&self, _: &Path) -> bool {
            self.daemon
        }
    }

    #[test]
    fn a_stopped_daemon_refuses_a_built_container_but_not_an_adopted_one() {
        let manager = bynk_driver::npm_install::PackageManager::Npm;
        let probe = wrangler(Some(v(4, 140, 0)));
        let built = resources(vec![decl("Renderer", None, None)]);
        let adopted = resources(vec![decl("Renderer", None, Some(IMAGE))]);
        let stopped = Stub { daemon: false };
        let refusal = container_refusals(&stopped, &probe, manager, &order(), &built).unwrap();
        assert!(refusal.contains("Docker"), "{refusal}");
        assert_eq!(
            container_refusals(&stopped, &probe, manager, &order(), &adopted),
            None
        );
        let running = Stub { daemon: true };
        assert_eq!(
            container_refusals(&running, &probe, manager, &order(), &built),
            None
        );
        assert_eq!(
            resolve_docker(&running, "podman"),
            Some("/bin/podman".into())
        );
        assert_eq!(resolve_docker(&running, "/no/such/engine"), None);
    }

    #[test]
    fn the_ledger_records_an_adopted_image_and_forgets_a_dropped_one() {
        let mut lock = DeployLock::default();
        let adopted = Resources {
            containers: vec![decl("Renderer", None, Some(IMAGE))],
            ..Resources::default()
        };
        record_container_images(&mut lock, "shop-render", &adopted, "production");
        assert_eq!(
            recorded_container_image(&lock, "shop-render", "Renderer", "production"),
            Some(IMAGE)
        );
        assert_eq!(
            recorded_container_image(&lock, "shop-render", "Renderer", "staging"),
            None
        );
        // Built again: the record goes, and the environment's entry with it.
        let built = Resources {
            containers: vec![decl("Renderer", None, None)],
            ..Resources::default()
        };
        record_container_images(&mut lock, "shop-render", &built, "production");
        assert_eq!(
            recorded_container_image(&lock, "shop-render", "Renderer", "production"),
            None
        );
        assert!(lock.environments["production"].container_images.is_empty());
    }
}
