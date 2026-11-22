//! 1D.1e (#3929): AI Search, each `AiSearch` instance, as one [`ResourceKind`].
//!
//! An `AiSearch` instance (#3480) queries an account's AI Search instance. The record is the instance's name only.
//!
//! Linked, never created: the deploy looks it up before the run confirms, records what it was linked
//! to before the push (one write for the Worker), and never orphans or prunes it. Every deploy
//! refreshes the record.

use super::*;

/// The AI Search kind.
pub(crate) struct AiSearch;

/// [`AiSearch`]'s records: `Environment::ai_search_links`, by Worker and binding.
pub(crate) struct AiSearchTable;

impl LedgerTable for AiSearchTable {
    type Record = AiSearchLink;

    fn get<'l>(env: &'l Environment, worker: &str, binding: &str) -> Option<&'l AiSearchLink> {
        env.ai_search_links
            .get(worker)
            .and_then(|links| links.get(binding))
    }

    fn insert(env: &mut Environment, worker: &str, binding: &str, record: AiSearchLink) {
        env.ai_search_links
            .entry(worker.to_string())
            .or_default()
            .insert(binding.to_string(), record);
    }

    fn remove(env: &mut Environment, worker: &str, binding: &str) {
        if let Some(links) = env.ai_search_links.get_mut(worker) {
            links.remove(binding);
            if links.is_empty() {
                env.ai_search_links.remove(worker);
            }
        }
    }

    fn entries(env: &Environment) -> Vec<(&str, &str, &AiSearchLink)> {
        env.ai_search_links
            .iter()
            .flat_map(|(worker, links)| {
                links
                    .iter()
                    .map(move |(binding, link)| (worker.as_str(), binding.as_str(), link))
            })
            .collect()
    }
}

impl ResourceKind for AiSearch {
    const ID: KindId = KindId::AiSearch;

    type Decl<'r> = &'r AiSearchDecl;
    type Record = AiSearchLink;
    type Table = AiSearchTable;
    type Line<'r> = PlanAiSearch<'r>;
    type Orphan = NoOrphan;
    type Live<'a> = ();
    /// Nothing: the config binds the instance by the name it already carries.
    type Bound = ();

    fn declared(r: &Resources) -> Vec<&AiSearchDecl> {
        r.ai_search.iter().collect()
    }

    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str {
        &decl.binding
    }

    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [PlanAiSearch<'r>] {
        &context.ai_search
    }

    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<PlanAiSearch<'r>> {
        &mut context.ai_search
    }

    fn live_in<'a>(_lives: Lives<'a>) -> Self::Live<'a> {}

    fn plan_line<'r>(
        _cx: &PlanCx<'_>,
        decl: Self::Decl<'r>,
        _live: Self::Live<'_>,
    ) -> PlanAiSearch<'r> {
        PlanAiSearch {
            binding: &decl.binding,
            instance: &decl.instance,
        }
    }

    /// Linked, not provisioned: the account owns it. The plan is built offline (#3494), so it has
    /// not looked the instance up: it says "will link", never "linked", and only a real deploy
    /// checks that the instance exists. The same line prints on a real run, before that lookup, so
    /// it is worded for both.
    fn short_line(search: &PlanAiSearch<'_>, out: &mut String) {
        out.push_str(&format!(
            "will link ai-search {} ({}; not created; not checked in a dry run)\n",
            search.instance, search.binding
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

    /// One link alone, recorded at once. The deploy links a Worker's together
    /// ([`ResourceKind::provision_all`]).
    fn provision(
        cx: &mut ApplyCx<'_, '_, AiSearch>,
        decl: &AiSearchDecl,
    ) -> Result<Provisioned<()>, DeployFailure> {
        let worker = cx.worker;
        let (link, bound) = link(cx, decl)?;
        if record_link(cx, &decl.binding, link) {
            cx.commit(&format!(
                "linked `{worker}`'s AI Search instances but could not record them"
            ))?;
        }
        Ok(Provisioned {
            bound: Some((decl.binding.clone(), bound)),
        })
    }

    /// Each link recorded, the ledger written once when any changed, before the push.
    fn provision_all(
        cx: &mut ApplyCx<'_, '_, AiSearch>,
        declared: &Resources,
    ) -> Result<BTreeMap<String, ()>, DeployFailure> {
        let worker = cx.worker;
        let mut bound = BTreeMap::new();
        let mut changed = false;
        for decl in &declared.ai_search {
            let (link, id) = link(cx, decl)?;
            changed |= record_link(cx, &decl.binding, link);
            bound.insert(decl.binding.clone(), id);
        }
        if changed {
            cx.commit(&format!(
                "linked `{worker}`'s AI Search instances but could not record them"
            ))?;
        }
        Ok(bound)
    }
}

/// Record `link` for this Worker's `binding` unless it is already what the ledger holds. Whether
/// the ledger changed.
fn record_link(cx: &mut ApplyCx<'_, '_, AiSearch>, binding: &str, link: AiSearchLink) -> bool {
    let worker = cx.worker;
    let env = cx.env_mut();
    if AiSearchTable::get(env, worker, binding) == Some(&link) {
        return false;
    }
    AiSearchTable::insert(env, worker, binding, link);
    true
}

/// What `decl` was linked to, as the ledger records it. Looked up before the run confirmed
/// ([`link_ai_search_for_run`]), which stops the run when one is missing.
fn link(
    _cx: &ApplyCx<'_, '_, AiSearch>,
    decl: &AiSearchDecl,
) -> Result<(AiSearchLink, ()), DeployFailure> {
    Ok((
        AiSearchLink {
            instance: decl.instance.clone(),
        },
        (),
    ))
}

/// #3480: one `AiSearch` instance's link. No action: `bynk deploy` neither
/// creates nor changes the instance.
#[derive(Debug, Serialize)]
pub(crate) struct PlanAiSearch<'a> {
    pub(crate) binding: &'a str,
    pub(crate) instance: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> AiSearchLink {
        AiSearchLink {
            instance: "support-docs".into(),
        }
    }

    /// Provision `declared`'s links of `shop` into `lock`, recording in `lock_path`.
    fn provision(
        lock: &mut DeployLock,
        lock_path: &Path,
        declared: &Resources,
        linked: &BTreeMap<(String, String), String>,
    ) -> Result<BTreeMap<String, ()>, DeployFailure> {
        let provenance = Provenance::Path("/nonexistent/wrangler".into());
        let project = BTreeMap::new();
        AiSearch::provision_all(
            &mut ApplyCx {
                environment: "production",
                worker: "shop",
                project: &project,
                lock,
                lock_path,
                live: AiSearch::live_in(Lives {
                    linked: Some(linked),
                    ..Lives::default()
                }),
                tools: Tools {
                    provenance: &provenance,
                    project_root: Path::new("/nonexistent"),
                },
            },
            declared,
        )
    }

    fn linked() -> BTreeMap<(String, String), String> {
        BTreeMap::from([(
            ("shop".to_string(), "AI_SEARCH_KB".to_string()),
            "unused".to_string(),
        )])
    }

    #[test]
    fn the_table_keeps_the_ledgers_shape() {
        let mut env = Environment::default();
        AiSearchTable::insert(&mut env, "shop", "AI_SEARCH_KB", link());
        assert_eq!(
            AiSearchTable::get(&env, "shop", "AI_SEARCH_KB"),
            Some(&link())
        );
        assert_eq!(
            AiSearchTable::entries(&env),
            vec![("shop", "AI_SEARCH_KB", &link())]
        );
        AiSearchTable::remove(&mut env, "shop", "AI_SEARCH_KB");
        assert!(env.ai_search_links.is_empty());
    }

    #[test]
    fn a_links_record_is_written_once_and_a_repeat_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("bynk-ai-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(LOCK_FILE);
        let mut lock = DeployLock::default();
        let bound = provision(
            &mut lock,
            &path,
            &Resources::default().ai_search(&[("AI_SEARCH_KB", "support-docs")]),
            &linked(),
        )
        .unwrap();
        assert_eq!(bound.len(), 1);
        assert_eq!(
            lock.recorded::<AiSearch>("production", "shop", "AI_SEARCH_KB"),
            Some(&link())
        );
        assert!(path.exists());
        std::fs::remove_file(&path).unwrap();
        provision(
            &mut lock,
            &path,
            &Resources::default().ai_search(&[("AI_SEARCH_KB", "support-docs")]),
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
            &Resources::default().ai_search(&[("AI_SEARCH_KB", "support-docs")]),
            &linked(),
        )
        .unwrap_err();
        assert!(
            e.message
                .starts_with("linked `shop`'s AI Search instances but could not record them in "),
            "{}",
            e.message
        );
    }

    #[test]
    fn the_short_line_says_it_is_linked_not_created() {
        let decls = Resources::default().ai_search(&[("AI_SEARCH_KB", "support-docs")]);
        let lock = DeployLock::default();
        let cx = PlanCx {
            ledger: LedgerView::of(&lock, "production"),
            worker: "shop",
        };
        let line = AiSearch::plan_line(&cx, AiSearch::declared(&decls)[0], ());
        let mut out = String::new();
        AiSearch::short_line(&line, &mut out);
        assert_eq!(
            out,
            "will link ai-search support-docs (AI_SEARCH_KB; not created; not checked in a dry run)\n"
        );
    }
}
