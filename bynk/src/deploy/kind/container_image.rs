//! 1D.1e (#3929): container images, each `Container` capability's adopted image, as one
//! [`ResourceKind`].
//!
//! A container (#3477) is built from its `@build` directory unless an environment adopts a
//! digest-pinned image for it. The record is that adopted image, by Worker and capability: the
//! plan reads it to say a changed or dropped adoption "was adopted" before. Nothing is created on
//! the account here (the application is `wrangler deploy`'s to reconcile), and nothing is ever
//! orphaned or pruned. The images are named in the configuration
//! ([`materialise_adopted_containers`]) before they are recorded, and the ledger is written for
//! every Worker with a container, changed or not.

use super::*;

/// The container image kind.
pub(crate) struct ContainerImage;

/// [`ContainerImage`]'s records: `Environment::container_images`, by Worker and capability.
pub(crate) struct ContainerImageTable;

impl LedgerTable for ContainerImageTable {
    type Record = String;

    fn get<'l>(env: &'l Environment, worker: &str, capability: &str) -> Option<&'l String> {
        env.container_images
            .get(worker)
            .and_then(|images| images.get(capability))
    }

    fn insert(env: &mut Environment, worker: &str, capability: &str, image: String) {
        env.container_images
            .entry(worker.to_string())
            .or_default()
            .insert(capability.to_string(), image);
    }

    fn remove(env: &mut Environment, worker: &str, capability: &str) {
        if let Some(images) = env.container_images.get_mut(worker) {
            images.remove(capability);
            if images.is_empty() {
                env.container_images.remove(worker);
            }
        }
    }

    fn entries(env: &Environment) -> Vec<(&str, &str, &String)> {
        env.container_images
            .iter()
            .flat_map(|(worker, images)| {
                images
                    .iter()
                    .map(move |(capability, image)| (worker.as_str(), capability.as_str(), image))
            })
            .collect()
    }
}

impl ResourceKind for ContainerImage {
    const ID: KindId = KindId::ContainerImage;

    type Decl<'r> = &'r ContainerDecl;
    /// The adopted image reference, digest-pinned.
    type Record = String;
    type Table = ContainerImageTable;
    type Line<'r> = PlanContainer<'r>;
    type Orphan = NoOrphan;
    type Live<'a> = ();
    /// Nothing: the configuration names the image ([`materialise_adopted_containers`]).
    type Bound = ();
    type Change = TableChange<ContainerImageTable>;
    type Step<'r> = ContainerImageStep<'r>;

    fn declared(r: &Resources) -> Vec<&ContainerDecl> {
        r.containers.iter().collect()
    }

    /// A container is keyed by its capability, the name `[deploy.containers.<Cap>]` takes.
    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str {
        &decl.capability
    }

    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [PlanContainer<'r>] {
        &context.containers
    }

    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<PlanContainer<'r>> {
        &mut context.containers
    }

    fn live_in<'a>(_lives: Lives<'a>) -> Self::Live<'a> {}

    fn plan_line<'r>(cx: &PlanCx<'_>, decl: Self::Decl<'r>, _live: ()) -> PlanContainer<'r> {
        PlanContainer {
            class: &decl.class,
            capability: &decl.capability,
            build: &decl.build,
            adopted: decl.adopt.as_deref(),
            previously_adopted: cx
                .ledger
                .recorded::<ContainerImage>(cx.worker, &decl.capability)
                .filter(|was| Some(was.as_str()) != decl.adopt.as_deref())
                .cloned(),
            instance: decl.instance_type.as_deref().unwrap_or("lite"),
            reconciled_by: "Cloudflare",
        }
    }

    /// Where its image comes from and the instance type passed at start, so the size is visible
    /// before the confirmation. The rates are Cloudflare's to state.
    fn short_line(container: &PlanContainer<'_>, out: &mut String) {
        let mut image = match container.adopted {
            Some(reference) => format!("adopted {reference}"),
            None => format!("build from {}", container.build),
        };
        if let Some(was) = &container.previously_adopted {
            image.push_str(&format!(", was adopted {was}"));
        }
        out.push_str(&format!(
            "container {} (image: {image}; instance {}; reconciled by {})\n",
            container.class, container.instance, container.reconciled_by
        ));
    }

    fn shown_orphan(orphan: &NoOrphan, _environment: &str) -> NoOrphan {
        match *orphan {}
    }

    fn short_orphan(orphan: &NoOrphan, _out: &mut String) {
        match *orphan {}
    }

    fn orphan_worker(orphan: &NoOrphan) -> &str {
        match *orphan {}
    }

    /// One step for all the Worker's containers, when it has any.
    fn steps<'r>(
        _cx: &PlanCx<'_>,
        declared: &'r Resources,
        _live: (),
    ) -> Vec<ContainerImageStep<'r>> {
        if declared.containers.is_empty() {
            return Vec::new();
        }
        vec![ContainerImageStep {
            containers: declared.containers.iter().collect(),
        }]
    }

    fn steps_of<'p, 'r>(steps: &'p WorkerSteps<'r>) -> &'p [ContainerImageStep<'r>] {
        &steps.containers
    }

    fn steps_mut<'p, 'r>(steps: &'p mut WorkerSteps<'r>) -> &'p mut Vec<ContainerImageStep<'r>> {
        &mut steps.containers
    }

    /// Each adopted image recorded, and one whose adoption was dropped forgotten (its container
    /// is built again), then the ledger written once, before the push, whatever that changed.
    fn apply(
        cx: &StepCx<'_, '_, ContainerImage>,
        step: &ContainerImageStep<'_>,
    ) -> Result<Applied<ContainerImage>, DeployFailure> {
        Ok(Applied::nothing().with(record_images(cx.worker, step.containers.iter().copied())))
    }
}

/// A Worker's containers' step (1D.2b): each container it declares.
#[derive(Debug, Clone)]
pub(crate) struct ContainerImageStep<'r> {
    pub(crate) containers: Vec<&'r ContainerDecl>,
}

/// One write of `worker`'s container images, whatever it changes: each adopted image recorded, and
/// the one recorded for a container built instead forgotten (which leaves an environment the
/// ledger does not have alone).
fn record_images<'d>(
    worker: &str,
    containers: impl IntoIterator<Item = &'d ContainerDecl>,
) -> Commit<TableChange<ContainerImageTable>> {
    Commit::of(
        containers
            .into_iter()
            .map(|decl| match &decl.adopt {
                Some(image) => TableChange::insert(worker, &decl.capability, image.clone()),
                None => TableChange::remove(worker, &decl.capability),
            })
            .collect(),
        format!("could not record the container images of `{worker}`"),
    )
}

/// #3477: one container the push declares.
#[derive(Debug, Serialize)]
pub(crate) struct PlanContainer<'a> {
    /// The generated Durable Object class.
    pub(crate) class: &'a str,
    /// The capability's name, the key `[deploy.containers.<Cap>]` takes.
    pub(crate) capability: &'a str,
    /// The `@build` directory the image is built from when it is not adopted.
    pub(crate) build: &'a str,
    /// The digest-pinned image adopted for this environment, if any. When
    /// present nothing is built.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) adopted: Option<&'a str>,
    /// The image the ledger recorded this container as adopted when it was last
    /// pushed, when it is not the one in `adopted` (a changed or dropped
    /// adoption). Informational: the push starts new instances on the new image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) previously_adopted: Option<String>,
    /// The instance type passed to `start()`: `@instanceType`, else the
    /// platform's default (`lite`).
    pub(crate) instance: &'a str,
    /// Always `Cloudflare`: the application is `wrangler deploy`'s to create
    /// and update, not Bynk's, as for a Durable Object class.
    pub(crate) reconciled_by: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::containers::tests_support::decl;

    const IMAGE: &str = "registry.example/renderer@sha256:0123";

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bynk-container-image-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn record(
        lock: &mut DeployLock,
        lock_path: &Path,
        declared: &Resources,
        environment: &str,
    ) -> Result<(), DeployFailure> {
        let provenance = Provenance::Path("/nonexistent/wrangler".into());
        let project = BTreeMap::new();
        let site = Site {
            tools: Tools {
                api: &crate::cloudflare::Account::new(Wrangler::new(
                    &provenance,
                    Path::new("/nonexistent"),
                )),
                project_root: Path::new("/nonexistent"),
            },
            environment,
            worker: "shop-render",
            project: &project,
        };
        provision_kind::<ContainerImage>(
            site,
            declared,
            &mut LedgerWriter::new(lock, lock_path),
            Lives::default(),
        )
        .map(|_| ())
    }

    #[test]
    fn the_ledger_records_an_adopted_image_and_forgets_a_dropped_one() {
        let dir = scratch("adopt");
        let path = dir.join(LOCK_FILE);
        let mut lock = DeployLock::default();
        let adopted = Resources {
            containers: vec![decl("Renderer", None, Some(IMAGE))],
            ..Resources::default()
        };
        record(&mut lock, &path, &adopted, "production").unwrap();
        assert_eq!(
            lock.recorded::<ContainerImage>("production", "shop-render", "Renderer"),
            Some(&IMAGE.to_string())
        );
        assert_eq!(
            lock.recorded::<ContainerImage>("staging", "shop-render", "Renderer"),
            None
        );
        assert!(std::fs::read_to_string(&path).unwrap().contains(IMAGE));
        // Built again: the record goes, and the environment's entry with it.
        let built = Resources {
            containers: vec![decl("Renderer", None, None)],
            ..Resources::default()
        };
        record(&mut lock, &path, &built, "production").unwrap();
        assert_eq!(
            lock.recorded::<ContainerImage>("production", "shop-render", "Renderer"),
            None
        );
        assert!(lock.environments["production"].container_images.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_built_image_in_an_environment_the_ledger_lacks_adds_nothing() {
        let dir = scratch("built");
        let path = dir.join(LOCK_FILE);
        let mut lock = DeployLock::default();
        let built = Resources {
            containers: vec![decl("Renderer", None, None)],
            ..Resources::default()
        };
        record(&mut lock, &path, &built, "staging").unwrap();
        assert!(lock.environments.is_empty());
        // Written all the same: a Worker with a container always records.
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_worker_without_containers_writes_nothing() {
        let dir = scratch("none");
        let path = dir.join(LOCK_FILE);
        let mut lock = DeployLock::default();
        record(&mut lock, &path, &Resources::default(), "production").unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_says_the_container_images_of_the_worker() {
        let mut lock = DeployLock::default();
        let adopted = Resources {
            containers: vec![decl("Renderer", None, Some(IMAGE))],
            ..Resources::default()
        };
        let e = record(
            &mut lock,
            Path::new("/nonexistent/bynk.deploy.lock"),
            &adopted,
            "production",
        )
        .unwrap_err();
        assert!(
            e.message
                .starts_with("could not record the container images of `shop-render` in "),
            "{}",
            e.message
        );
    }

    #[test]
    fn the_plan_line_says_a_changed_adoption_was_adopted() {
        let mut lock = DeployLock::default();
        lock.record::<ContainerImage>("default", "shop", "Renderer", "old@sha256:1".to_string());
        let cx = PlanCx {
            ledger: LedgerView::of(&lock, "default"),
            worker: "shop",
        };
        let same = decl("Renderer", Some("standard"), Some("old@sha256:1"));
        let line = ContainerImage::plan_line(&cx, &same, ());
        assert_eq!(line.previously_adopted, None);
        assert_eq!(line.instance, "standard");
        let built = decl("Renderer", None, None);
        let line = ContainerImage::plan_line(&cx, &built, ());
        assert_eq!(line.previously_adopted.as_deref(), Some("old@sha256:1"));
        let mut out = String::new();
        ContainerImage::short_line(&line, &mut out);
        assert_eq!(
            out,
            "container __Container_Renderer (image: build from containers/render, was adopted old@sha256:1; instance lite; reconciled by Cloudflare)\n"
        );
    }
}
