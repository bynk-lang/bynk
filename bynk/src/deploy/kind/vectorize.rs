//! 1D.1c (#3929): Vectorize, each `Vectorize` instance's index (`capability Docs = Vectorize …`),
//! as one [`ResourceKind`].
//!
//! An index's environment-qualified name is its identity and its configuration is fixed at
//! creation (#3479), so the ledger records the name it was created or adopted under, in
//! `Environment::vectorize_instances`. An index already on the account, or one `bynk.toml` adopts,
//! is recorded as adopted and never deleted. One a changed name leaves behind is kept as a
//! superseded record and reported as an orphan, which `--prune` deletes, with its vectors, unless it
//! was adopted. The wrangler calls are in `deploy/vectorize.rs`, unchanged.

use super::*;

/// The Vectorize kind.
pub(crate) struct Vectorize;

/// [`Vectorize`]'s records: `Environment::vectorize_instances`, by Worker and binding.
pub(crate) struct VectorizeTable;

impl VectorizeRecord {
    /// The record of index `name`, adopted or made by the deploy, with nothing superseded yet.
    pub(crate) fn of(name: &str, adopted: bool) -> Self {
        VectorizeRecord {
            name: name.to_string(),
            adopted,
            superseded: Vec::new(),
        }
    }
}

impl VectorizeTable {
    /// Drop index `name` from `worker`'s `binding` once `--prune` deleted it, whether it was the
    /// live one (its Worker or capability is gone) or a superseded one, and the Worker's map once
    /// it is empty.
    pub(crate) fn forget_index(env: &mut Environment, worker: &str, binding: &str, name: &str) {
        let Some(instances) = env.vectorize_instances.get_mut(worker) else {
            return;
        };
        if instances.get(binding).is_some_and(|r| r.name == name) {
            instances.remove(binding);
        } else if let Some(record) = instances.get_mut(binding) {
            record.superseded.retain(|past| past.name != name);
        }
        if instances.is_empty() {
            env.vectorize_instances.remove(worker);
        }
    }
}

impl LedgerTable for VectorizeTable {
    type Record = VectorizeRecord;

    fn get<'l>(env: &'l Environment, worker: &str, binding: &str) -> Option<&'l VectorizeRecord> {
        env.vectorize_instances
            .get(worker)
            .and_then(|instances| instances.get(binding))
    }

    /// An index this replaces under another name is not forgotten: its vectors are still there,
    /// so it joins the new record's `superseded`, unless it was adopted (never the project's). A
    /// binding that moves back to an index it left (A, B, A) does not list the live index as an
    /// orphan: `--prune` would delete it.
    fn insert(env: &mut Environment, worker: &str, binding: &str, record: VectorizeRecord) {
        let instances = env
            .vectorize_instances
            .entry(worker.to_string())
            .or_default();
        let mut superseded = Vec::new();
        if let Some(old) = instances.remove(binding) {
            superseded = old.superseded;
            superseded.retain(|past| past.name != record.name);
            if old.name != record.name && !old.adopted {
                superseded.push(VectorizePast {
                    name: old.name,
                    adopted: old.adopted,
                });
            }
        }
        instances.insert(
            binding.to_string(),
            VectorizeRecord {
                superseded,
                ..record
            },
        );
    }

    fn remove(env: &mut Environment, worker: &str, binding: &str) {
        if let Some(instances) = env.vectorize_instances.get_mut(worker) {
            instances.remove(binding);
            if instances.is_empty() {
                env.vectorize_instances.remove(worker);
            }
        }
    }

    fn entries(env: &Environment) -> Vec<(&str, &str, &VectorizeRecord)> {
        env.vectorize_instances
            .iter()
            .flat_map(|(worker, instances)| {
                instances
                    .iter()
                    .map(move |(binding, index)| (worker.as_str(), binding.as_str(), index))
            })
            .collect()
    }
}

impl ResourceKind for Vectorize {
    const ID: KindId = KindId::Vectorize;

    type Decl<'r> = &'r VectorizeDecl;
    type Record = VectorizeRecord;
    type Table = VectorizeTable;
    type Line<'r> = PlanVectorize<'r>;
    type Orphan = OrphanVectorize;
    type Live<'a> = ();
    /// Nothing from provisioning: an adopted index's name is written into the config later in
    /// the deploy ([`materialise_adopted_vectorize`]), from the declarations.
    type Bound = ();

    fn declared(r: &Resources) -> Vec<&VectorizeDecl> {
        r.vectorize_indexes.iter().collect()
    }

    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str {
        &decl.binding
    }

    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [PlanVectorize<'r>] {
        &context.vectorize
    }

    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<PlanVectorize<'r>> {
        &mut context.vectorize
    }

    fn live_in<'a>(_lives: Lives<'a>) -> Self::Live<'a> {}

    fn plan_line<'r>(cx: &PlanCx<'_>, v: Self::Decl<'r>, _live: ()) -> PlanVectorize<'r> {
        let index = v.physical_name(cx.ledger.environment);
        let recorded = cx.ledger.recorded::<Vectorize>(cx.worker, &v.binding);
        PlanVectorize {
            binding: &v.binding,
            action: if v.adopt.is_some() {
                // Adopted stays `adopt` once recorded, as Hyperdrive's does.
                "adopt"
            } else if recorded.is_some_and(|r| r.name == index) {
                "reuse"
            } else {
                "create"
            },
            index,
            dimensions: v.dimensions,
            metric: &v.metric,
            indexed: &v.indexed,
        }
    }

    fn short_line(v: &PlanVectorize<'_>, out: &mut String) {
        out.push_str(&format!(
            "vectorize {} {} ({}, {}, {}",
            v.action, v.index, v.binding, v.dimensions, v.metric
        ));
        if !v.indexed.is_empty() {
            out.push_str(&format!("; metadata indexes {}", v.indexed.join(", ")));
        }
        out.push_str(")\n");
    }

    fn listing(v: &PlanVectorize<'_>) -> Option<on_account::Listing> {
        matches!(v.action, "create" | "create or reuse").then_some((Self::ID, None))
    }

    /// An index of its name the account already holds is `reuse` (#3494): the deploy adopts it.
    fn checked_against(
        v: &mut PlanVectorize<'_>,
        _shown_worker: &str,
        on: &on_account::OnAccount,
        _notes: &mut Vec<String>,
    ) {
        v.action = held_verb(v.action, on.holds(Self::ID, None, &v.index));
    }

    fn name_problem(cx: &ProjectCx<'_>) -> Option<String> {
        vectorize_index_problem(cx.resources, cx.ledger)
    }

    fn orphans_in(orphans: &Orphans) -> &[OrphanVectorize] {
        &orphans.vectorize
    }

    fn set_orphans(orphans: &mut Orphans, found: Vec<OrphanVectorize>) {
        orphans.vectorize = found;
    }

    /// An index with its Worker or on its own; one a changed name left behind is an orphan
    /// while its binding is live, too.
    fn orphans(cx: &OrphanCx<'_>) -> Vec<OrphanVectorize> {
        let resources = cx.resources;
        cx.env
            .vectorize_instances
            .iter()
            .flat_map(|(worker, instances)| {
                instances.iter().flat_map(move |(binding, record)| {
                    let live = resources
                        .get(worker)
                        .is_some_and(|r| r.vectorize_indexes.iter().any(|v| &v.binding == binding));
                    let main = (!live).then(|| OrphanVectorize {
                        worker: worker.clone(),
                        binding: binding.clone(),
                        index: record.name.clone(),
                        adopted: record.adopted,
                    });
                    main.into_iter()
                        .chain(record.superseded.iter().map(move |past| OrphanVectorize {
                            worker: worker.clone(),
                            binding: binding.clone(),
                            index: past.name.clone(),
                            adopted: past.adopted,
                        }))
                })
            })
            .collect()
    }

    fn shown_orphan(v: &OrphanVectorize, _environment: &str) -> OrphanVectorize {
        v.clone()
    }

    fn short_orphan(v: &OrphanVectorize, out: &mut String) {
        out.push_str(&format!(
            "orphan vectorize {} {} ({}{})\n",
            v.worker,
            v.binding,
            v.index,
            if v.adopted {
                "; adopted, never deleted"
            } else {
                "; deleted by --prune"
            }
        ));
    }

    fn orphan_worker(v: &OrphanVectorize) -> &str {
        &v.worker
    }

    /// An index the deploy made, with its vectors; an adopted one is not the project's to delete.
    fn prunable(v: &OrphanVectorize) -> bool {
        !v.adopted
    }

    fn prune_notice(v: &OrphanVectorize, _environment: &str) -> Option<Notice> {
        Some(Notice::Plain(format!(
            "bynk: will delete Vectorize index `{}` of `{}`'s `{}`, and every vector in it",
            v.index, v.worker, v.binding
        )))
    }

    /// Named as the account has it; gone already is success, as for a queue.
    fn prune(
        cx: &mut ApplyCx<'_, '_, Vectorize>,
        orphan: &OrphanVectorize,
    ) -> Result<(), DeployFailure> {
        delete_vectorize_index(cx.tools.provenance, &orphan.index, cx.tools.project_root).map_err(
            |e| {
                DeployFailure::driver(format!(
                    "could not delete the orphaned Vectorize index `{}` of `{}`'s `{}`: {e}",
                    orphan.index, orphan.worker, orphan.binding
                ))
            },
        )?;
        cx.change(
            |env| VectorizeTable::forget_index(env, &orphan.worker, &orphan.binding, &orphan.index),
            &format!("deleted the orphaned Vectorize index `{}`", orphan.index),
        )
    }

    fn provision(
        cx: &mut ApplyCx<'_, '_, Vectorize>,
        decl: &VectorizeDecl,
    ) -> Result<Provisioned<()>, DeployFailure> {
        provision_vectorize_index(cx, decl, METADATA_INDEX_PACING)?;
        Ok(Provisioned { bound: None })
    }
}

/// One recorded Vectorize index the build no longer declares (#3479): a
/// capability removed or renamed, an index replaced, or a whole Worker gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OrphanVectorize {
    pub(crate) worker: String,
    pub(crate) binding: String,
    /// The index's name on the account.
    pub(crate) index: String,
    /// An adopted index is not the project's to delete.
    pub(crate) adopted: bool,
}

/// #3479: one Vectorize index. The name is the identity and the configuration
/// is fixed at creation, so `reuse` states that an index of the name is there;
/// whether it still matches is checked by the deploy, which fails on a
/// mismatch.
#[derive(Debug, Serialize)]
pub(crate) struct PlanVectorize<'a> {
    /// `VECTORIZE_<SNAKE>`.
    pub(crate) binding: &'a str,
    /// `create`, `adopt` (an existing index `bynk.toml` names, which is never
    /// created, changed or deleted) or `reuse` (the one the ledger recorded, or
    /// #3494 one of its name the account already holds, which the deploy
    /// adopts).
    pub(crate) action: &'static str,
    /// The index's account-facing name, [`vectorize_index_name`]d.
    pub(crate) index: String,
    pub(crate) dimensions: i64,
    pub(crate) metric: &'a str,
    /// The metadata indexes created before any vector.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub(crate) indexed: &'a [String],
}

/// #3479: why the indexes the build declares cannot all be made in
/// `environment`, or `None`: a name Cloudflare would refuse (the environment's
/// suffix can push a derived name past 64 bytes), or one name for two
/// instances, which would silently share an index. Checked within the build and
/// against every index the ledger records in any environment.
pub(crate) fn vectorize_index_problem(
    resources: &BTreeMap<String, Resources>,
    ledger: LedgerView<'_>,
) -> Option<String> {
    let environment = ledger.environment;
    let mut seen: BTreeMap<String, (&str, &str, bool)> = BTreeMap::new();
    for (worker, declared) in resources {
        for index in &declared.vectorize_indexes {
            let name = index.physical_name(environment);
            if let Some(why) = vectorize_index_name_problem(&name) {
                // #3479: an adopted name is the developer's, so the remedy is
                // the `bynk.toml` entry, not the capability's name.
                let remedy = if index.adopt.is_some() {
                    "name an index whose name the platform takes in `[deploy.vectorize]`"
                } else {
                    "rename the capability or the context"
                };
                return Some(format!(
                    "Vectorize index `{name}` (`{}` of `{worker}`) cannot be used: {why}; {remedy}",
                    index.binding
                ));
            }
            let adopted = index.adopt.is_some();
            // Adopters may share an index (it is another system's, or
            // another environment's); a created index is one capability's.
            if let Some((other_worker, other_binding, other_adopted)) =
                seen.insert(name.clone(), (worker, &index.binding, adopted))
                && !(adopted && other_adopted)
            {
                return Some(format!(
                    "`{}` of `{worker}` and `{other_binding}` of `{other_worker}` would both be the Vectorize index `{name}`; index names are scoped to the account, so rename one capability, or adopt the index for both in `[deploy.vectorize]`",
                    index.binding
                ));
            }
            for (other_env, env) in ledger.all() {
                let clash = env
                    .vectorize_instances
                    .iter()
                    .flat_map(|(w, instances)| instances.iter().map(move |(b, r)| (w, b, r)))
                    .find(|(w, b, r)| {
                        r.name == name
                            && !(r.adopted && adopted)
                            && (other_env != environment
                                || w.as_str() != worker
                                || **b != index.binding)
                    });
                if let Some((other_worker, other_binding, _)) = clash {
                    return Some(format!(
                        "`{}` of `{worker}` would be the Vectorize index `{name}`, which `{other_binding}` of `{other_worker}` already holds in environment `{other_env}`; index names are scoped to the account, so rename one capability, or adopt the index for both in `[deploy.vectorize]`",
                        index.binding
                    ));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    // ---- 1D.1c (#3929): the `Vectorize` impl ----

    use super::*;
    use crate::deploy::config::tests::project;

    #[test]
    fn a_new_index_supersedes_one_it_replaces_unless_adopted_or_returned_to() {
        let mut env = Environment::default();
        VectorizeTable::insert(
            &mut env,
            "shop",
            "VECTORIZE_DOCS",
            VectorizeRecord::of("a", false),
        );
        VectorizeTable::insert(
            &mut env,
            "shop",
            "VECTORIZE_DOCS",
            VectorizeRecord::of("b", false),
        );
        assert_eq!(
            env.vectorize_instances["shop"]["VECTORIZE_DOCS"].superseded,
            vec![VectorizePast {
                name: "a".to_string(),
                adopted: false
            }]
        );
        // Back to `a`: the live index is never also an orphan.
        VectorizeTable::insert(
            &mut env,
            "shop",
            "VECTORIZE_DOCS",
            VectorizeRecord::of("a", false),
        );
        let record = VectorizeTable::get(&env, "shop", "VECTORIZE_DOCS").expect("recorded");
        assert_eq!(record.name, "a");
        assert_eq!(
            record
                .superseded
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b"]
        );
        // Moving off an adopted index leaves nothing to report.
        VectorizeTable::insert(
            &mut env,
            "shop",
            "VECTORIZE_X",
            VectorizeRecord::of("theirs", true),
        );
        VectorizeTable::insert(
            &mut env,
            "shop",
            "VECTORIZE_X",
            VectorizeRecord::of("ours", false),
        );
        assert!(
            env.vectorize_instances["shop"]["VECTORIZE_X"]
                .superseded
                .is_empty()
        );
    }

    #[test]
    fn forgetting_an_index_takes_the_live_or_the_superseded_one_and_then_the_map() {
        let mut env = Environment::default();
        VectorizeTable::insert(
            &mut env,
            "shop",
            "VECTORIZE_DOCS",
            VectorizeRecord::of("a", false),
        );
        VectorizeTable::insert(
            &mut env,
            "shop",
            "VECTORIZE_DOCS",
            VectorizeRecord::of("b", false),
        );
        VectorizeTable::forget_index(&mut env, "shop", "VECTORIZE_DOCS", "a");
        assert!(
            env.vectorize_instances["shop"]["VECTORIZE_DOCS"]
                .superseded
                .is_empty()
        );
        VectorizeTable::forget_index(&mut env, "shop", "VECTORIZE_DOCS", "b");
        assert_eq!(env, Environment::default());
    }

    #[test]
    fn a_plan_line_adopts_reuses_or_creates_and_is_checked_against_the_account() {
        let mut lock = DeployLock::default();
        lock.record::<Vectorize>(
            "default",
            "shop",
            "VECTORIZE_DOCS",
            VectorizeRecord::of("shop-docs", false),
        );
        let mut declared = Resources::default().vectorize_indexes(&[
            ("VECTORIZE_DOCS", "shop-docs", 768, "cosine", &["category"]),
            ("VECTORIZE_NEW", "shop-new", 384, "euclidean", &[]),
            ("VECTORIZE_THEIRS", "shop-theirs", 768, "cosine", &[]),
        ]);
        declared.vectorize_indexes[2].adopt = Some("shared-index".to_string());
        let cx = PlanCx {
            ledger: LedgerView::of(&lock, "default"),
            worker: "shop",
        };
        let mut lines: Vec<_> = Vectorize::declared(&declared)
            .into_iter()
            .map(|decl| Vectorize::plan_line(&cx, decl, ()))
            .collect();
        let mut out = String::new();
        for line in &lines {
            Vectorize::short_line(line, &mut out);
        }
        assert_eq!(
            out,
            "vectorize reuse shop-docs (VECTORIZE_DOCS, 768, cosine; metadata indexes category)\n\
             vectorize create shop-new (VECTORIZE_NEW, 384, euclidean)\n\
             vectorize adopt shared-index (VECTORIZE_THEIRS, 768, cosine)\n"
        );
        assert_eq!(
            Vectorize::listing(&lines[1]),
            Some((KindId::Vectorize, None))
        );
        assert_eq!(Vectorize::listing(&lines[2]), None);
        struct Holds;
        impl on_account::Listings for Holds {
            fn names(&self, _: KindId, _: Option<&str>) -> Result<BTreeSet<String>, String> {
                Ok(BTreeSet::from(["shop-new".to_string()]))
            }
        }
        let on = on_account::OnAccount::read(&BTreeSet::from([(KindId::Vectorize, None)]), &Holds);
        Vectorize::checked_against(&mut lines[1], "shop", &on, &mut Vec::new());
        assert_eq!(lines[1].action, "reuse");
    }

    #[test]
    fn orphans_are_undeclared_and_superseded_indexes_and_only_ours_are_pruned() {
        let mut lock = DeployLock::default();
        lock.record::<Vectorize>(
            "default",
            "shop",
            "VECTORIZE_DOCS",
            VectorizeRecord::of("shop-docs", false),
        );
        lock.record::<Vectorize>(
            "default",
            "shop",
            "VECTORIZE_DOCS",
            VectorizeRecord::of("shop-docs-v2", false),
        );
        lock.record::<Vectorize>(
            "default",
            "gone",
            "VECTORIZE_THEIRS",
            VectorizeRecord::of("shared", true),
        );
        let resources = project(vec![(
            "shop",
            Resources::default().vectorize_indexes(&[(
                "VECTORIZE_DOCS",
                "shop-docs-v2",
                768,
                "cosine",
                &[],
            )]),
        )]);
        let orphans = Vectorize::orphans(&OrphanCx {
            env: &lock.environments["default"],
            resources: &resources,
        });
        let mut out = String::new();
        for orphan in &orphans {
            Vectorize::short_orphan(orphan, &mut out);
        }
        assert_eq!(
            out,
            "orphan vectorize gone VECTORIZE_THEIRS (shared; adopted, never deleted)\n\
             orphan vectorize shop VECTORIZE_DOCS (shop-docs; deleted by --prune)\n"
        );
        assert!(!Vectorize::prunable(&orphans[0]));
        assert!(Vectorize::prunable(&orphans[1]));
        assert_eq!(
            Vectorize::prune_notice(&orphans[1], "default"),
            Some(Notice::Plain(
                "bynk: will delete Vectorize index `shop-docs` of `shop`'s `VECTORIZE_DOCS`, and every vector in it"
                    .to_string()
            ))
        );
        assert_eq!(
            recorded_bound_id(LedgerView::of(&lock, "default"), "shop", "VECTORIZE_DOCS"),
            None,
            "an index is bound by name"
        );
    }
}
