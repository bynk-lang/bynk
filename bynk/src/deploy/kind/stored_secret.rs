//! 1D.1e (#3929): stored secrets, each `StoredSecret` instance, as one [`ResourceKind`].
//!
//! A `StoredSecret` instance (#3301) reads a secret from a Secrets Store. The record is the store's name and id and the secret's name, never a value: the deploy never asks for one. It is what `bynk dev -- --remote` binds by.
//!
//! Linked, never created: the deploy looks it up before the run confirms, records what it was linked
//! to before the push (one write for the Worker), and never orphans or prunes it. Every deploy
//! refreshes the record.

use super::*;

/// The stored secret kind.
pub(crate) struct StoredSecret;

/// [`StoredSecret`]'s records: `Environment::stored_secret_links`, by Worker and binding.
pub(crate) struct StoredSecretTable;

impl LedgerTable for StoredSecretTable {
    type Record = StoredSecretLink;

    fn get<'l>(env: &'l Environment, worker: &str, binding: &str) -> Option<&'l StoredSecretLink> {
        env.stored_secret_links
            .get(worker)
            .and_then(|links| links.get(binding))
    }

    fn insert(env: &mut Environment, worker: &str, binding: &str, record: StoredSecretLink) {
        env.stored_secret_links
            .entry(worker.to_string())
            .or_default()
            .insert(binding.to_string(), record);
    }

    fn remove(env: &mut Environment, worker: &str, binding: &str) {
        if let Some(links) = env.stored_secret_links.get_mut(worker) {
            links.remove(binding);
            if links.is_empty() {
                env.stored_secret_links.remove(worker);
            }
        }
    }

    fn entries(env: &Environment) -> Vec<(&str, &str, &StoredSecretLink)> {
        env.stored_secret_links
            .iter()
            .flat_map(|(worker, links)| {
                links
                    .iter()
                    .map(move |(binding, link)| (worker.as_str(), binding.as_str(), link))
            })
            .collect()
    }
}

impl ResourceKind for StoredSecret {
    const ID: KindId = KindId::StoredSecret;

    type Decl<'r> = &'r StoredSecretDecl;
    type Record = StoredSecretLink;
    type Table = StoredSecretTable;
    type Line<'r> = PlanStoredSecret<'r>;
    type Orphan = NoOrphan;
    /// The Secrets Stores the run linked each `(worker, binding)` to, by id ([`link_for_run`]).
    type Live<'a> = Option<&'a BTreeMap<(String, String), String>>;
    /// The store's id, which the config binds the secret by.
    type Bound = String;
    type Change = TableChange<StoredSecretTable>;
    type Step<'r> = StoredSecretStep<'r>;

    fn declared(r: &Resources) -> Vec<&StoredSecretDecl> {
        r.stored_secrets.iter().collect()
    }

    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str {
        &decl.binding
    }

    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [PlanStoredSecret<'r>] {
        &context.stored_secrets
    }

    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<PlanStoredSecret<'r>> {
        &mut context.stored_secrets
    }

    fn live_in<'a>(lives: Lives<'a>) -> Self::Live<'a> {
        lives.linked
    }

    fn plan_line<'r>(
        _cx: &PlanCx<'_>,
        decl: Self::Decl<'r>,
        _live: Self::Live<'_>,
    ) -> PlanStoredSecret<'r> {
        PlanStoredSecret {
            binding: &decl.binding,
            store: &decl.store,
            secret: &decl.secret,
        }
    }

    /// Linked, not provisioned: the account owns both.
    fn short_line(secret: &PlanStoredSecret<'_>, out: &mut String) {
        out.push_str(&format!(
            "stored secret {} {} in store {} (linked; not created)\n",
            secret.binding, secret.secret, secret.store
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

    /// One step for all the Worker's links, written once when any changed.
    fn steps<'r>(
        _cx: &PlanCx<'_>,
        declared: &'r Resources,
        _live: Self::Live<'_>,
    ) -> Vec<StoredSecretStep<'r>> {
        if declared.stored_secrets.is_empty() {
            return Vec::new();
        }
        vec![StoredSecretStep {
            decls: declared.stored_secrets.iter().collect(),
        }]
    }

    fn steps_of<'p, 'r>(steps: &'p WorkerSteps<'r>) -> &'p [StoredSecretStep<'r>] {
        &steps.stored_secrets
    }

    fn steps_mut<'p, 'r>(steps: &'p mut WorkerSteps<'r>) -> &'p mut Vec<StoredSecretStep<'r>> {
        &mut steps.stored_secrets
    }

    /// Each link recorded, the ledger written once when any changed, before the push.
    fn apply(
        cx: &StepCx<'_, '_, StoredSecret>,
        step: &StoredSecretStep<'_>,
    ) -> Result<Applied<StoredSecret>, DeployFailure> {
        apply_links(cx, &step.decls)
    }

    fn materialise(
        config: &Path,
        worker: &str,
        bound: &BTreeMap<String, String>,
    ) -> Result<(), DeployFailure> {
        if !bound.is_empty() && !materialise_stored_secret_stores(config, bound) {
            return Err(DeployFailure::driver(format!(
                "could not materialise the Secrets Store ids into `{worker}`'s generated configuration"
            )));
        }
        Ok(())
    }
}

/// [`StoredSecret`]'s step: each of the Worker's links, written once when any changed.
fn apply_links(
    cx: &StepCx<'_, '_, StoredSecret>,
    decls: &[&StoredSecretDecl],
) -> Result<Applied<StoredSecret>, DeployFailure> {
    let worker = cx.worker;
    let mut applied = Applied::nothing();
    let mut links = Links::new();
    for &decl in decls {
        let (link, id) = link(cx, decl)?;
        links.link(cx, &decl.binding, link);
        applied.bound.push((decl.binding.clone(), id));
    }
    if let Some(commit) = links.commit(
        worker,
        format!("linked `{worker}`'s Secrets Store secrets but could not record them"),
    ) {
        applied = applied.with(commit);
    }
    Ok(applied)
}

/// What `decl` was linked to, as the ledger records it, and the store id. Linked before the run
/// confirmed ([`link_for_run`]), so a binding with no id here is a bug, not a user error.
fn link(
    cx: &StepCx<'_, '_, StoredSecret>,
    decl: &StoredSecretDecl,
) -> Result<(StoredSecretLink, String), DeployFailure> {
    let worker = cx.worker;
    let Some(store_id) = cx
        .live
        .and_then(|linked| linked.get(&(worker.to_string(), decl.binding.clone())))
    else {
        return Err(DeployFailure::driver(format!(
            "`{}` of `{worker}` was not linked to a Secrets Store before the push",
            decl.binding
        )));
    };
    Ok((
        StoredSecretLink {
            store: decl.store.clone(),
            store_id: store_id.clone(),
            secret: decl.secret.clone(),
        },
        store_id.clone(),
    ))
}

/// #3301: one `StoredSecret` instance's link. No action: `bynk deploy`
/// neither creates nor changes the store or the secret.
#[derive(Debug, Serialize)]
pub(crate) struct PlanStoredSecret<'a> {
    pub(crate) binding: &'a str,
    pub(crate) store: &'a str,
    pub(crate) secret: &'a str,
}

/// A Worker's links' step (1D.2b): each declaration, linked to the id found before the run
/// confirmed.
#[derive(Debug, Clone)]
pub(crate) struct StoredSecretStep<'r> {
    pub(crate) decls: Vec<&'r StoredSecretDecl>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> StoredSecretLink {
        StoredSecretLink {
            store: "payments".into(),
            store_id: "store-1".into(),
            secret: "stripe-live".into(),
        }
    }

    /// Provision `declared`'s links of `shop` into `lock`, recording in `lock_path`.
    fn provision(
        lock: &mut DeployLock,
        lock_path: &Path,
        declared: &Resources,
        linked: &BTreeMap<(String, String), String>,
    ) -> Result<BTreeMap<String, String>, DeployFailure> {
        let provenance = Provenance::Path("/nonexistent/wrangler".into());
        let project = BTreeMap::new();
        provision_kind::<StoredSecret>(
            Site {
                tools: Tools {
                    api: &Wrangler::new(&provenance, Path::new("/nonexistent")),
                    provenance: &provenance,
                    project_root: Path::new("/nonexistent"),
                },
                environment: "production",
                worker: "shop",
                project: &project,
            },
            declared,
            &mut LedgerWriter::new(lock, lock_path),
            Lives {
                linked: Some(linked),
                ..Lives::default()
            },
        )
    }

    fn linked() -> BTreeMap<(String, String), String> {
        BTreeMap::from([(
            ("shop".to_string(), "SECRET_STRIPE_KEY".to_string()),
            "store-1".to_string(),
        )])
    }

    #[test]
    fn the_table_keeps_the_ledgers_shape() {
        let mut env = Environment::default();
        StoredSecretTable::insert(&mut env, "shop", "SECRET_STRIPE_KEY", link());
        assert_eq!(
            StoredSecretTable::get(&env, "shop", "SECRET_STRIPE_KEY"),
            Some(&link())
        );
        assert_eq!(
            StoredSecretTable::entries(&env),
            vec![("shop", "SECRET_STRIPE_KEY", &link())]
        );
        StoredSecretTable::remove(&mut env, "shop", "SECRET_STRIPE_KEY");
        assert!(env.stored_secret_links.is_empty());
    }

    #[test]
    fn a_links_record_is_written_once_and_a_repeat_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("bynk-stored-secret-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(LOCK_FILE);
        let mut lock = DeployLock::default();
        let bound = provision(
            &mut lock,
            &path,
            &Resources::default().stored_secrets(&[(
                "SECRET_STRIPE_KEY",
                "payments",
                "stripe-live",
            )]),
            &linked(),
        )
        .unwrap();
        assert_eq!(bound.len(), 1);
        assert_eq!(
            lock.recorded::<StoredSecret>("production", "shop", "SECRET_STRIPE_KEY"),
            Some(&link())
        );
        assert!(path.exists());
        std::fs::remove_file(&path).unwrap();
        provision(
            &mut lock,
            &path,
            &Resources::default().stored_secrets(&[(
                "SECRET_STRIPE_KEY",
                "payments",
                "stripe-live",
            )]),
            &linked(),
        )
        .unwrap();
        assert!(!path.exists(), "an unchanged link is not written again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_says_what_was_linked() {
        let e = provision(
            &mut DeployLock::default(),
            Path::new("/nonexistent/bynk.deploy.lock"),
            &Resources::default().stored_secrets(&[(
                "SECRET_STRIPE_KEY",
                "payments",
                "stripe-live",
            )]),
            &linked(),
        )
        .unwrap_err();
        assert!(
            e.message
                .starts_with("linked `shop`'s Secrets Store secrets but could not record them in "),
            "{}",
            e.message
        );
    }

    #[test]
    fn the_short_line_says_it_is_linked_not_created() {
        let decls = Resources::default().stored_secrets(&[(
            "SECRET_STRIPE_KEY",
            "payments",
            "stripe-live",
        )]);
        let lock = DeployLock::default();
        let cx = PlanCx {
            ledger: LedgerView::of(&lock, "production"),
            worker: "shop",
        };
        let line = StoredSecret::plan_line(
            &cx,
            StoredSecret::declared(&decls)[0],
            StoredSecret::live_in(Lives::default()),
        );
        let mut out = String::new();
        StoredSecret::short_line(&line, &mut out);
        assert_eq!(
            out,
            "stored secret SECRET_STRIPE_KEY stripe-live in store payments (linked; not created)\n"
        );
    }

    #[test]
    fn a_binding_the_run_did_not_link_is_a_bug_named() {
        let e = provision(
            &mut DeployLock::default(),
            Path::new("/nonexistent/bynk.deploy.lock"),
            &Resources::default().stored_secrets(&[(
                "SECRET_STRIPE_KEY",
                "payments",
                "stripe-live",
            )]),
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert_eq!(
            e.message,
            "`SECRET_STRIPE_KEY` of `shop` was not linked to a Secrets Store before the push"
        );
    }
}
