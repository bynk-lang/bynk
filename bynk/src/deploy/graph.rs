use super::*;

/// The project's binding graph: worker → the workers it binds to.
pub(crate) fn service_graph(
    resources: &BTreeMap<String, Resources>,
) -> BTreeMap<String, Vec<String>> {
    resources
        .iter()
        .map(|(worker, r)| (worker.clone(), r.binds_to.clone()))
        .collect()
}

/// Why an order could not be produced.
#[derive(Debug, PartialEq, Eq)]
pub enum OrderError {
    /// A Service Binding cycle. A `consumes` cycle never reaches here —
    /// `bynkc` rejects one as `bynk.context.consumes_cycle` before emit — and
    /// a context is bound only when called (#1961), so the one cycle the
    /// compiler can produce runs through an events fan-out: the publisher
    /// binds to each subscriber to deliver, and a subscriber that also calls
    /// the publisher (directly or through others) binds back.
    /// [`cycle_publishers`] names that case. Refused rather than tolerated
    /// because Cloudflare resolves a binding at upload: whichever Worker goes
    /// first binds to one that does not exist yet.
    Cycle(Vec<String>),
}

impl std::fmt::Display for OrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderError::Cycle(path) => write!(
                f,
                "the generated Service Bindings form a cycle ({}) — it cannot be uploaded in dependency order",
                path.join(" → ")
            ),
        }
    }
}

/// The workers on `cycle` (a closed path, as [`OrderError::Cycle`] reports
/// it) that publish events — those declaring the events fan-out Durable
/// Object, whose delivery binds them to every subscriber. Empty for a cycle
/// the compiler cannot produce (a hand-edited build tree).
pub(crate) fn cycle_publishers(
    cycle: &[String],
    resources: &BTreeMap<String, Resources>,
) -> Vec<String> {
    let mut publishers: Vec<String> = cycle
        .iter()
        .filter(|worker| {
            resources.get(*worker).is_some_and(|r| {
                r.durable_objects
                    .iter()
                    .any(|d| d.class == bynk_emit::emitter::wrangler::EVENTS_FANOUT_CLASS_NAME)
            })
        })
        .cloned()
        .collect();
    publishers.sort();
    publishers.dedup();
    publishers
}

/// Order the workers so that **every binding target is uploaded before the
/// worker that binds to it** — dependencies first.
///
/// This is a correctness barrier, not a nicety: Cloudflare resolves a Service
/// Binding at **upload time**, and rejects a Worker whose bound target does not
/// yet exist ("deployment will fail, because Worker A declares a binding to
/// Worker B, which does not yet exist"). Uploading in a wrong order does not
/// merely open a transient half-wired window — it fails outright.
///
/// Depth-first post-order over the sorted worker list, so the result is
/// deterministic for a given project rather than dependent on map iteration
/// order. Pure, so the ordering contract is unit-tested without a build tree.
pub fn deploy_order(
    workers: &[String],
    graph: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<String>, OrderError> {
    let mut order = Vec::new();
    let mut done = BTreeSet::new();
    let mut path = Vec::new();
    for worker in workers {
        visit(worker, graph, &mut done, &mut path, &mut order)?;
    }
    Ok(order)
}

fn visit(
    worker: &str,
    graph: &BTreeMap<String, Vec<String>>,
    done: &mut BTreeSet<String>,
    path: &mut Vec<String>,
    order: &mut Vec<String>,
) -> Result<(), OrderError> {
    if done.contains(worker) {
        return Ok(());
    }
    if path.iter().any(|p| p == worker) {
        // Close the reported path onto the repeated node so the cycle reads as
        // a loop (`a → b → a`) rather than a bare list.
        let start = path.iter().position(|p| p == worker).unwrap_or(0);
        let mut cycle: Vec<String> = path[start..].to_vec();
        cycle.push(worker.to_string());
        return Err(OrderError::Cycle(cycle));
    }
    path.push(worker.to_string());
    for target in graph.get(worker).into_iter().flatten() {
        // A binding to something outside this project's build (an
        // externally-managed Worker) has no node to order against; leave it to
        // Cloudflare to accept or reject.
        if graph.contains_key(target) {
            visit(target, graph, done, path, order)?;
        }
    }
    path.pop();
    done.insert(worker.to_string());
    order.push(worker.to_string());
    Ok(())
}

/// The contexts `worker` binds to that the ledger has never recorded as
/// deployed — the D4 gate for `--context`. Deploying a Worker whose binding
/// target does not exist fails at upload, so naming the absent dependency beats
/// letting Cloudflare reject the push with its own vocabulary.
pub(crate) fn absent_dependencies(
    worker: &str,
    graph: &BTreeMap<String, Vec<String>>,
    lock: &DeployLock,
    environment: &str,
) -> Vec<String> {
    graph
        .get(worker)
        .into_iter()
        .flatten()
        .filter(|target| !lock.is_deployed(environment, target))
        .cloned()
        .collect()
}

/// One dependency service whose live contract is not the one `worker` was
/// compiled against.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ContractSkew {
    pub(crate) dependency: String,
    pub(crate) service: String,
    pub(crate) expected: String,
    pub(crate) live: String,
    /// #2485: whether the pushed caller still works against the live callee,
    /// when both sides' structure is on record. Information only: the gate
    /// refuses any skew (ADR 0491), because the runtime check compares hashes.
    pub(crate) compat: Option<Compatibility>,
}

/// v0.177 (#643): the D4 gate extended from *exists* to *matches*.
///
/// `deploy --context A` pushes A alone, against dependencies assumed already
/// live. D4 already refuses when a dependency has never been deployed. This adds
/// the other half: a dependency that exists but no longer provides the contract A
/// was compiled against. Without it, the push succeeds and the skew is
/// discovered by production traffic 409ing — which is better than the silent
/// misinterpretation that preceded this increment, but far worse than refusing.
///
/// **Silence is not a match.** A dependency the ledger has no contract record
/// for (deployed by a pre-v0.177 driver) yields no finding: the gate reports what
/// it *knows* is skewed, never what it merely cannot rule out. The runtime check
/// remains the backstop for exactly that case — and for a `wrangler` push behind
/// the driver's back, or a ledger that has drifted. This gate is an optimisation
/// over the runtime check, never a replacement for it.
pub(crate) fn contract_skews(
    expects: &BTreeMap<String, BTreeMap<String, String>>,
    lock: &DeployLock,
    worker_of: impl Fn(&str) -> String,
    environment: &str,
) -> Vec<ContractSkew> {
    let mut out = Vec::new();
    for (dependency, services) in expects {
        let worker = worker_of(dependency);
        // No record — a pre-v0.177 push, or never deployed. Silence is not a
        // match: report only what is *known* to be skewed. (An empty-but-present
        // record is different, and is checked: it means the callee is known to
        // provide nothing, so every expected service is absent.)
        let Some(live) = lock.live_contracts(environment, &worker) else {
            continue;
        };
        for (service, expected) in services {
            match live.get(service) {
                Some(actual) if actual != expected => out.push(ContractSkew {
                    dependency: dependency.clone(),
                    service: service.clone(),
                    expected: expected.clone(),
                    live: actual.clone(),
                    compat: None,
                }),
                // A service the live callee no longer provides at all: it was
                // removed or renamed, which is the most complete skew there is.
                None => out.push(ContractSkew {
                    dependency: dependency.clone(),
                    service: service.clone(),
                    expected: expected.clone(),
                    live: "<absent>".to_string(),
                    compat: None,
                }),
                Some(_) => {}
            }
        }
    }
    out
}

/// #2485: classify each caller-side skew as compatible or breaking, where the
/// pushed build's `expects` shapes and the ledger's `provides` shapes are both
/// on record and agree with their hashes. A skew with either side missing keeps
/// no classification: silence is not a verdict.
pub(crate) fn classify_contract_skews(
    skews: &mut [ContractSkew],
    expects_shapes: &BTreeMap<String, BTreeMap<String, String>>,
    lock: &DeployLock,
    worker_of: impl Fn(&str) -> String,
    environment: &str,
) {
    for s in skews {
        let Some(caller) = expects_shapes
            .get(&s.dependency)
            .and_then(|m| m.get(&s.service))
            .and_then(|text| ServiceShape::from_text_checked(text, &s.expected))
        else {
            continue;
        };
        let Some(callee) = lock.live_shape(environment, &worker_of(&s.dependency), &s.service)
        else {
            continue;
        };
        s.compat = Some(check_call(&caller, &callee));
    }
}

/// #2485: the callee half of [`classify_contract_skews`]: the live caller's
/// recorded `expects` shape against the pushed build's `provides` shape.
pub(crate) fn classify_callee_skews(
    skews: &mut [CalleeSkew],
    provides_shapes: &BTreeMap<String, String>,
    lock: &DeployLock,
    environment: &str,
) {
    for s in skews {
        let Some(callee) = provides_shapes
            .get(&s.service)
            .and_then(|text| ServiceShape::from_text_checked(text, &s.provided))
        else {
            continue;
        };
        let Some(caller) = lock.expected_shape(environment, &s.caller, &s.dependency, &s.service)
        else {
            continue;
        };
        s.compat = Some(check_call(&caller, &callee));
    }
}

/// One live caller service whose recorded contract the pushed callee's new
/// build no longer provides (#2077).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CalleeSkew {
    /// The live caller, by its logical Worker name (`shop-orders`).
    pub(crate) caller: String,
    /// The pushed callee as the caller names it: its qualified context.
    pub(crate) dependency: String,
    pub(crate) service: String,
    /// The hash the live caller stamps, as the ledger recorded it.
    pub(crate) expected: String,
    /// What the new build provides, `<absent>` for a removed service.
    pub(crate) provided: String,
    /// #2485: whether the live caller still works against the new build, when
    /// both sides' structure is on record. Information only, as for
    /// [`ContractSkew::compat`].
    pub(crate) compat: Option<Compatibility>,
}

/// #2077: the callee half of the #643 gate.
///
/// [`contract_skews`] holds the *pushed* Worker's `expects` to its live
/// dependencies' recorded `provides`. This holds the pushed Worker's new
/// `provides` to every *other* live Worker's recorded `expects` in
/// `environment`: pushing a callee whose contract changed under a live caller
/// ships a callee that 409s that caller on every call, exactly as pushing a
/// skewed caller does.
///
/// `worker_of` maps a qualified dependency name to its Worker, as in
/// [`contract_skews`]. The pushed Worker's own record is skipped (it is being
/// replaced). A caller whose record has no `expects` — pushed before they were
/// recorded — yields no finding: silence is not a match
/// ([`unrecorded_callers`] names those for a warning).
pub(crate) fn callee_contract_skews(
    worker: &str,
    provides: &BTreeMap<String, String>,
    lock: &DeployLock,
    worker_of: impl Fn(&str) -> String,
    environment: &str,
) -> Vec<CalleeSkew> {
    let mut out = Vec::new();
    for (caller, expects) in lock.live_expects(environment) {
        if caller == worker {
            continue;
        }
        let Some(expects) = expects else { continue };
        for (dependency, services) in expects {
            if worker_of(dependency) != worker {
                continue;
            }
            for (service, expected) in services {
                let provided = match provides.get(service) {
                    Some(actual) if actual == expected => continue,
                    Some(actual) => actual.clone(),
                    None => "<absent>".to_string(),
                };
                out.push(CalleeSkew {
                    caller: caller.to_string(),
                    dependency: dependency.clone(),
                    service: service.clone(),
                    expected: expected.clone(),
                    provided,
                    compat: None,
                });
            }
        }
    }
    out
}

/// #2077 review: split callee skews into those whose caller this build still
/// has (`graph` keys every project Worker) and those whose caller is an
/// orphan — a Worker of a context since removed from the project, still live
/// in the ledger. The remedies differ: a whole-project deploy moves a built
/// caller with its callee, but neither redeploys nor checks an orphan (nor
/// does `--prune`, which never deletes a Worker), so advising it for an
/// orphan would ship the very 409 the gate refused.
pub(crate) fn split_orphaned_callers(
    skews: Vec<CalleeSkew>,
    graph: &BTreeMap<String, Vec<String>>,
) -> (Vec<CalleeSkew>, Vec<CalleeSkew>) {
    skews
        .into_iter()
        .partition(|s| graph.contains_key(&s.caller))
}

/// #2077: the live Workers that bind to `worker` in this build's `graph` but
/// whose ledger record predates `expects`, so [`callee_contract_skews`] cannot
/// check them. A warning, never a refusal: the gate reports only what it
/// knows is skewed, and the next whole-project deploy records them.
pub(crate) fn unrecorded_callers(
    worker: &str,
    graph: &BTreeMap<String, Vec<String>>,
    lock: &DeployLock,
    environment: &str,
) -> Vec<String> {
    lock.live_expects(environment)
        .filter(|(caller, expects)| {
            *caller != worker
                && expects.is_none()
                && graph
                    .get(*caller)
                    .is_some_and(|targets| targets.iter().any(|t| t == worker))
        })
        .map(|(caller, _)| caller.to_string())
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::deploy::config::tests::project;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// A binding graph literal: `[("a", &["b"])]` = a binds to b.
    pub(crate) fn graph(edges: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        edges
            .iter()
            .map(|(from, to)| (from.to_string(), names(to)))
            .collect()
    }

    // ---- #601 slice 2: the deploy order --------------------------------

    #[test]
    fn a_binding_target_is_deployed_before_the_worker_that_binds_to_it() {
        // The defining contract. Cloudflare resolves a Service Binding at
        // upload and rejects a Worker whose target does not exist yet, so
        // this order is a correctness barrier, not a nicety.
        let g = graph(&[
            ("commerce-orders", &["commerce-payment"]),
            ("commerce-payment", &[]),
        ]);
        assert_eq!(
            deploy_order(&names(&["commerce-orders", "commerce-payment"]), &g),
            Ok(names(&["commerce-payment", "commerce-orders"])),
            "payment must be uploaded before the orders worker that binds to it"
        );
    }

    #[test]
    fn a_chain_deploys_from_the_far_end() {
        // a → b → c: c has no dependencies, so it goes first.
        let g = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &[])]);
        assert_eq!(
            deploy_order(&names(&["a", "b", "c"]), &g),
            Ok(names(&["c", "b", "a"]))
        );
    }

    #[test]
    fn a_diamond_deploys_the_shared_dependency_once_and_first() {
        //   a → b → d
        //   a → c → d
        let g = graph(&[("a", &["b", "c"]), ("b", &["d"]), ("c", &["d"]), ("d", &[])]);
        let order = deploy_order(&names(&["a", "b", "c", "d"]), &g).expect("a diamond is acyclic");
        assert_eq!(
            order.len(),
            4,
            "the shared dependency is deployed once: {order:?}"
        );
        let at = |w: &str| order.iter().position(|o| o == w).expect("present");
        assert!(at("d") < at("b") && at("d") < at("c"), "{order:?}");
        assert!(at("b") < at("a") && at("c") < at("a"), "{order:?}");
    }

    #[test]
    fn the_order_is_deterministic_for_a_given_project() {
        // Two independent workers keep the discovered (sorted) order, so a
        // plan a reviewer approved is the plan that runs.
        let g = graph(&[("a", &[]), ("b", &[])]);
        assert_eq!(
            deploy_order(&names(&["a", "b"]), &g),
            Ok(names(&["a", "b"]))
        );
    }

    #[test]
    fn a_binding_outside_the_project_is_left_to_cloudflare() {
        // An externally-managed Worker has no node to order against; it must
        // not be invented into the order.
        let g = graph(&[("a", &["someone-elses-worker"])]);
        assert_eq!(deploy_order(&names(&["a"]), &g), Ok(names(&["a"])));
    }

    #[test]
    fn a_cycle_is_reported_rather_than_looping_forever() {
        // A hand-edited build tree, or the events case below: either way the
        // order must not hang or overflow the stack.
        let g = graph(&[("a", &["b"]), ("b", &["a"])]);
        let Err(OrderError::Cycle(path)) = deploy_order(&names(&["a", "b"]), &g) else {
            panic!("a cycle must be reported");
        };
        assert_eq!(
            path.first(),
            path.last(),
            "the path closes into a loop: {path:?}"
        );
        assert!(path.contains(&"a".to_string()) && path.contains(&"b".to_string()));
    }

    // ---- #1961: events and the deploy order ----------------------------

    #[test]
    fn a_subscriber_that_only_consumes_its_publisher_deploys_first() {
        // The issue's shape after the fix: the subscriber consumes the
        // publisher for the event type alone, so it binds to nothing, and the
        // publisher's fan-out binds to it. Subscriber first, then publisher.
        let g = graph(&[("ev-emitter", &["ev-listener"]), ("ev-listener", &[])]);
        assert_eq!(
            deploy_order(&names(&["ev-emitter", "ev-listener"]), &g),
            Ok(names(&["ev-listener", "ev-emitter"]))
        );
    }

    #[test]
    fn a_subscriber_that_calls_its_publisher_is_a_cycle_through_the_fan_out() {
        // The case still refused: the listener calls an emitter service (a
        // real binding) and the emitter delivers to the listener. The cycle
        // names the publisher, so the message can say why.
        let g = graph(&[
            ("ev-emitter", &["ev-listener"]),
            ("ev-listener", &["ev-emitter"]),
        ]);
        let Err(OrderError::Cycle(path)) = deploy_order(&names(&["ev-emitter", "ev-listener"]), &g)
        else {
            panic!("a subscriber calling its publisher is a binding cycle");
        };
        let resources = project(vec![
            (
                "ev-emitter",
                Resources::default()
                    .binds(&["ev-listener"])
                    .exports(&["__EventsFanout"]),
            ),
            (
                "ev-listener",
                Resources::default()
                    .binds(&["ev-emitter"])
                    .exports(&["Tally"]),
            ),
        ]);
        assert_eq!(cycle_publishers(&path, &resources), names(&["ev-emitter"]));
    }

    #[test]
    fn a_cycle_without_a_publisher_names_none() {
        let resources = project(vec![
            ("a", Resources::default().binds(&["b"])),
            ("b", Resources::default().binds(&["a"])),
        ]);
        assert!(cycle_publishers(&names(&["a", "b", "a"]), &resources).is_empty());
    }

    #[test]
    fn a_self_binding_is_reported_not_looped() {
        let g = graph(&[("a", &["a"])]);
        assert!(matches!(
            deploy_order(&names(&["a"]), &g),
            Err(OrderError::Cycle(_))
        ));
    }

    #[test]
    fn the_graph_is_the_binding_edges_of_the_resources_read() {
        let resources = project(vec![
            (
                "orders",
                Resources::default().binds(&["payment"]).consumes(&["q"]),
            ),
            ("payment", Resources::default().needs_kv()),
        ]);
        assert_eq!(
            service_graph(&resources),
            graph(&[("orders", &["payment"]), ("payment", &[])]),
            "the graph carries the binding edges and nothing else"
        );
    }
}
