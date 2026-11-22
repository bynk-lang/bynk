//! Back-half paydown 1D.2 (#4001): the apply, and the one place it writes the ledger.
//!
//! A kind never writes `bynk.deploy.lock`. Its provisioning and pruning read the ledger through a
//! [`StepCx`] and return what they would record or forget as [`Commit`]s, and [`LedgerWriter`]
//! writes each, in order, before the kind is asked for anything else: so a record still lands
//! before the next account call and before the push (ADR 0180). A failed step returns no commit,
//! so its changes are never written. The post-push record goes through the same writer, and so
//! `write_lock` has one caller in the deploy.
//!
//! What one write covers is what it covered before 1D.2 (the table in #4001's "What exists
//! today"): one per created or changed instance of KV, R2, D1, Hyperdrive and Vectorize (an index
//! before its metadata indexes); one per Worker for the rate limiters and each linked kind, only
//! when something changed, and for container images whenever the Worker has a container; one per
//! queue, after every queue is made, only when it changed the ledger; one per deleted orphan; and
//! one per pushed Worker.

use super::*;

/// The ledger and where it lives: the one place the deploy writes it (1D.2a, #4001).
pub(crate) struct LedgerWriter<'l> {
    lock: &'l mut DeployLock,
    path: &'l Path,
}

impl<'l> LedgerWriter<'l> {
    pub(crate) fn new(lock: &'l mut DeployLock, path: &'l Path) -> Self {
        LedgerWriter { lock, path }
    }

    /// The ledger as it stands.
    pub(crate) fn lock(&self) -> &DeployLock {
        self.lock
    }

    /// Where it is written.
    pub(crate) fn path(&self) -> &Path {
        self.path
    }

    /// Make `commit`'s changes in `environment`, in order, and write the ledger once if any of
    /// them asks it to be. A failed write is `commit`'s failure, " in {path}: {error}".
    pub(crate) fn commit<C: LedgerChange>(
        &mut self,
        environment: &str,
        commit: Commit<C>,
    ) -> Result<(), DeployFailure> {
        let mut write = false;
        for change in commit.changes {
            write |= change.apply(self.lock, environment);
        }
        if !write {
            return Ok(());
        }
        self.write().map_err(|e| {
            DeployFailure::driver(format!(
                "{} in {}: {e}",
                commit.failure,
                self.path.display()
            ))
        })
    }

    /// Change the ledger with `change`, then write it, whatever `change` did.
    pub(crate) fn change(&mut self, change: impl FnOnce(&mut DeployLock)) -> Result<(), String> {
        change(self.lock);
        self.write()
    }

    fn write(&mut self) -> Result<(), String> {
        write_lock(self.path, self.lock)
    }
}

/// Where a kind is provisioned: one Worker of one environment, in the whole project, through
/// the run's tools.
#[derive(Clone, Copy)]
pub(crate) struct Site<'a> {
    pub(crate) tools: Tools<'a>,
    pub(crate) environment: &'a str,
    pub(crate) worker: &'a str,
    pub(crate) project: &'a BTreeMap<String, Resources>,
}

impl<'a> Site<'a> {
    /// What a kind reads at this site, with the ledger as `lock` holds it now.
    pub(crate) fn cx<'c, 'l, K: ResourceKind>(
        &self,
        lock: &'c DeployLock,
        live: K::Live<'l>,
    ) -> StepCx<'c, 'l, K>
    where
        'a: 'c,
    {
        StepCx {
            environment: self.environment,
            worker: self.worker,
            project: self.project,
            lock,
            live,
            tools: self.tools,
        }
    }
}

/// Write what `provisioned` records, then do what follows it. What it binds, by binding.
pub(crate) fn complete<K: ResourceKind>(
    site: Site<'_>,
    provisioned: Provisioned<'_, K>,
    writer: &mut LedgerWriter<'_>,
    live: K::Live<'_>,
) -> Result<Vec<(String, K::Bound)>, DeployFailure> {
    for commit in provisioned.commits {
        writer.commit(site.environment, commit)?;
    }
    if let Some(then) = provisioned.then {
        then(&site.cx::<K>(writer.lock(), live))?;
    }
    Ok(provisioned.bound)
}

/// Provision `decl` of `K` at `site` alone: its account calls, then its record written, before
/// anything else. What the push binds it to.
pub(crate) fn provision_one<K: ResourceKind>(
    site: Site<'_>,
    decl: K::Decl<'_>,
    writer: &mut LedgerWriter<'_>,
    lives: Lives<'_>,
) -> Result<Vec<(String, K::Bound)>, DeployFailure> {
    let live = K::live_in(lives);
    let provisioned = K::provision(&site.cx::<K>(writer.lock(), live), decl)?;
    complete(site, provisioned, writer, live)
}

/// Provision each of the declared instances of `K` at `site`, in order, or together where the
/// kind does ([`ResourceKind::provision_all`]). Each record is written before the next instance is
/// made (ADR 0180: an interrupted run never makes a second). What the push binds, by binding.
pub(crate) fn provision_kind<K: ResourceKind>(
    site: Site<'_>,
    declared: &Resources,
    writer: &mut LedgerWriter<'_>,
    lives: Lives<'_>,
) -> Result<BTreeMap<String, K::Bound>, DeployFailure> {
    let live = K::live_in(lives);
    if let Some(provisioned) = K::provision_all(&site.cx::<K>(writer.lock(), live), declared) {
        return Ok(complete(site, provisioned?, writer, live)?
            .into_iter()
            .collect());
    }
    let mut bound = BTreeMap::new();
    for decl in K::declared(declared) {
        bound.extend(provision_one::<K>(site, decl, writer, lives)?);
    }
    Ok(bound)
}

/// Delete `orphan` of `K` from the account in `environment`, and forget it, writing the ledger.
pub(crate) fn prune_one<K: ResourceKind>(
    tools: Tools<'_>,
    writer: &mut LedgerWriter<'_>,
    environment: &str,
    orphan: &K::Orphan,
    lives: Lives<'_>,
) -> Result<(), DeployFailure> {
    let project = BTreeMap::new();
    let site = Site {
        tools,
        environment,
        worker: K::orphan_worker(orphan),
        project: &project,
    };
    let commit = K::prune(&site.cx::<K>(writer.lock(), K::live_in(lives)), orphan)?;
    match commit {
        Some(commit) => writer.commit(environment, commit),
        None => Ok(()),
    }
}

/// Delete each kind's orphans `--prune` deletes, in the list's order, each forgotten and the
/// ledger written as it goes, so a prune interrupted half-way is resumed rather than repeated.
pub(crate) fn prune_kind_orphans(
    tools: Tools<'_>,
    writer: &mut LedgerWriter<'_>,
    environment: &str,
    orphans: &Orphans,
    lives: Lives<'_>,
) -> Result<(), DeployFailure> {
    struct Prune<'p, 'w, 'v> {
        tools: Tools<'p>,
        writer: &'p mut LedgerWriter<'w>,
        environment: &'p str,
        orphans: &'p Orphans,
        lives: Lives<'v>,
        result: Result<(), DeployFailure>,
    }
    impl KindVisitor for Prune<'_, '_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            for orphan in K::orphans_in(self.orphans)
                .iter()
                .filter(|o| K::prunable(o))
            {
                if self.result.is_err() {
                    return;
                }
                self.result = prune_one::<K>(
                    self.tools,
                    self.writer,
                    self.environment,
                    orphan,
                    self.lives,
                );
            }
        }
    }
    let mut prune = Prune {
        tools,
        writer,
        environment,
        orphans,
        lives,
        result: Ok(()),
    };
    for_each_kind(&mut prune);
    prune.result
}

/// A Worker whose push succeeded, as the ledger records it: its directory (for the contracts it
/// provides and expects), its group's members and script, and what it declared.
pub(crate) struct PushedWorker<'a> {
    pub(crate) worker: &'a str,
    pub(crate) dir: std::path::PathBuf,
    pub(crate) members: Vec<String>,
    pub(crate) script: String,
    pub(crate) declared: &'a Resources,
}

impl<'a> PushedWorker<'a> {
    /// `worker`, built in `workers_dir`, in the project's `groups`.
    pub(crate) fn of(
        worker: &'a str,
        workers_dir: &Path,
        groups: &GroupNames,
        declared: &'a Resources,
    ) -> Self {
        PushedWorker {
            worker,
            dir: workers_dir.join(worker),
            members: groups.members(worker),
            script: groups.script(worker),
            declared,
        }
    }
}

/// Record that `pushed` was pushed to `environment`, and write the ledger once: its contracts, its
/// class tombstones, each kind's change after a push (a dropped rate limiter's id is free again),
/// and then `moves` (the fresh agents the run accepted). The write's error, for the caller's
/// message.
pub(crate) fn record_pushed(
    writer: &mut LedgerWriter<'_>,
    environment: &str,
    pushed: &PushedWorker<'_>,
    moves: impl FnOnce(&mut DeployLock),
) -> Result<(), String> {
    writer.change(|lock| {
        record_pushed_contracts(
            lock,
            environment,
            (pushed.worker, &pushed.dir),
            (&pushed.members, &pushed.script),
        );
        record_pushed_tombstones(lock, environment, pushed.worker, pushed.declared);
        kinds_after_push(lock, environment, pushed.worker, pushed.declared);
        moves(lock);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bynk-1d2-writer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A change that asks for no write.
    struct Unchanged;

    impl LedgerChange for Unchanged {
        fn apply(self, _lock: &mut DeployLock, _environment: &str) -> bool {
            false
        }
    }

    #[test]
    fn a_commit_whose_changes_ask_for_no_write_writes_nothing() {
        let dir = scratch("unchanged");
        let path = dir.join(LOCK_FILE);
        let mut lock = DeployLock::default();
        LedgerWriter::new(&mut lock, &path)
            .commit("default", Commit::of(vec![Unchanged], "never".to_string()))
            .unwrap();
        assert!(!path.exists(), "nothing was written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_is_made_in_the_environment_and_written() {
        let dir = scratch("record");
        let path = dir.join(LOCK_FILE);
        let mut lock = DeployLock::default();
        LedgerWriter::new(&mut lock, &path)
            .commit(
                "staging",
                Commit::recorded(
                    TableChange::<KvTable>::insert("api", "KV", KvNamespace::of("id-1")),
                    "created it",
                ),
            )
            .unwrap();
        assert_eq!(
            lock.recorded::<Kv>("staging", "api", "KV")
                .map(|kv| kv.id.as_str()),
            Some("id-1")
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            toml::to_string_pretty(&lock).unwrap(),
            "and written"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_says_what_was_done_and_where_it_could_not_be_recorded() {
        let dir = scratch("unwritable");
        // A directory where the ledger should be: the rename over it fails.
        let path = dir.join(LOCK_FILE);
        std::fs::create_dir_all(path.join("occupied")).unwrap();
        let mut lock = DeployLock::default();
        let e = LedgerWriter::new(&mut lock, &path)
            .commit(
                "default",
                Commit::recorded(
                    TableChange::<KvTable>::insert("api", "KV", KvNamespace::of("id-1")),
                    "created the KV namespace",
                ),
            )
            .unwrap_err();
        assert!(
            e.message.starts_with(&format!(
                "created the KV namespace but could not record it in {}: ",
                path.display()
            )),
            "{}",
            e.message
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forgetting_in_an_environment_the_ledger_lacks_changes_nothing_but_still_writes() {
        let dir = scratch("forget");
        let path = dir.join(LOCK_FILE);
        let mut lock = DeployLock::default();
        LedgerWriter::new(&mut lock, &path)
            .commit(
                "default",
                Commit::recorded(TableChange::<KvTable>::remove("api", "KV"), "deleted it"),
            )
            .unwrap();
        assert_eq!(lock, DeployLock::default());
        assert!(path.exists(), "the ledger was written, as before");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
