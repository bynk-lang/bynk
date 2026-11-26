use super::Entry;
use crate::deploy::LOCK_FILE;
use crate::deploy::config::deploy_command;
use crate::deploy::config::deployed_worker_name;
use crate::deploy::graph::CalleeSkew;
use crate::deploy::graph::ContractSkew;
use crate::deploy::graph::split_orphaned_callers;
use crate::deploy::ledger::DeployLock;
use bynk_check::contract_compat::Compatibility;
use bynk_check::contract_compat::ServiceShape;
use bynk_driver::colour;
use std::collections::BTreeMap;
use std::path::Path;

/// #1662: the diagnostic code a contract skew is reported under — a literal so
/// the registry test sees it, and `bynk explain` knows it.
pub(super) const CONTRACT_SKEW: &str = "bynk.deploy.contract_skew";

/// #2485: the reasons behind one skew's verdict, indented under its line.
fn push_compat(out: &mut String, compat: Option<&Compatibility>) {
    if let Some(c) = compat {
        for line in c.lines() {
            out.push_str(&format!("      {line}\n"));
        }
    }
}

/// Which side of a call a refusal is for.
#[derive(Clone, Copy)]
enum SkewSide {
    /// The pushed Worker is the caller, ahead of its live callee.
    Caller,
    /// The pushed Worker is the callee, ahead of or behind its live callers.
    Callee,
}

/// #2485, #2554: when some skew is only a compatible change, say what that
/// means for this side.
///
/// A callee can be deployed ahead of its callers that stay compatible: it is
/// built to answer the older contract too (`--accept-compatible-skew`). A
/// caller ahead of its live callee cannot: the live callee refuses a hash it
/// has never heard of, whatever the verdict, so the callee goes first.
fn push_compat_note<'a>(
    out: &mut String,
    verdicts: impl Iterator<Item = Option<&'a Compatibility>>,
    side: SkewSide,
    dependencies: &[&str],
) {
    use bynk_check::contract_compat::Verdict;
    let compatible = verdicts
        .into_iter()
        .any(|c| c.is_some_and(|c| c.verdict == Verdict::Compatible));
    if !compatible {
        return;
    }
    match side {
        SkewSide::Caller => {
            let mut first: Vec<&str> = dependencies.to_vec();
            first.sort_unstable();
            first.dedup();
            out.push_str(
                "  A compatible change is still refused here: the live callee refuses a contract it was not built to accept, so the callee goes first.\n",
            );
            out.push_str(&format!(
                "  Deploy {} ahead of this caller with `--accept-compatible-skew`.\n",
                first
                    .iter()
                    .map(|d| format!("`{d}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        SkewSide::Callee => out.push_str(
            "  A compatible change can be accepted: `--accept-compatible-skew` records the older contract in bynk.contracts.lock, and this callee then answers its live callers until they are redeployed.\n",
        ),
    }
}

/// #2554: the callee-ahead skews the pushed build answers: compatible for the
/// caller, and the caller's recorded hash is one the build accepts
/// (`bynk-contracts.json`'s `accepts`). Returns `(accepted, refused)`.
pub(crate) fn partition_accepted(
    skews: Vec<CalleeSkew>,
    accepts: &BTreeMap<String, Vec<String>>,
) -> (Vec<CalleeSkew>, Vec<CalleeSkew>) {
    skews
        .into_iter()
        .partition(|s| skew_is_accepted(s, accepts))
}

fn skew_is_accepted(s: &CalleeSkew, accepts: &BTreeMap<String, Vec<String>>) -> bool {
    use bynk_check::contract_compat::Verdict;
    s.compat
        .as_ref()
        .is_some_and(|c| c.verdict == Verdict::Compatible)
        && accepts
            .get(&s.service)
            .is_some_and(|hashes| hashes.contains(&s.expected))
}

/// #2554: the older contracts `bynk.contracts.lock` must gain for the pushed
/// build to answer its compatible live callers: `(context, service, hash,
/// shape text)`, one per distinct contract. A skew with no verdict, a breaking
/// one, and one the build already accepts add nothing.
pub(crate) fn shapes_to_accept(
    skews: &[CalleeSkew],
    accepts: &BTreeMap<String, Vec<String>>,
    lock: &DeployLock,
    environment: &str,
) -> Vec<(String, String, String, String)> {
    use bynk_check::contract_compat::Verdict;
    let mut out: Vec<(String, String, String, String)> = Vec::new();
    for s in skews {
        let compatible = s
            .compat
            .as_ref()
            .is_some_and(|c| c.verdict == Verdict::Compatible);
        if !compatible || skew_is_accepted(s, accepts) {
            continue;
        }
        let Some(shape) = lock.expected_shape(environment, &s.caller, &s.dependency, &s.service)
        else {
            continue;
        };
        let entry = (
            s.dependency.clone(),
            s.service.clone(),
            s.expected.clone(),
            shape.to_text(),
        );
        // One entry per contract hash: the `D()` marker is not hashed, so two
        // shape texts can differ only in it (#2689).
        if !out
            .iter()
            .any(|e| e.0 == entry.0 && e.1 == entry.1 && e.2 == entry.2)
        {
            out.push(entry);
        }
    }
    out
}

/// #2554: add `wanted` (from [`shapes_to_accept`]) to `bynk.contracts.lock`.
pub(crate) fn record_accepted(
    project_root: &Path,
    wanted: &[(String, String, String, String)],
) -> Result<(), String> {
    let existing = bynk_driver::contracts_lock::read(project_root).map_err(|e| e.to_string())?;
    let mut lock = bynk_project::contracts_lock::parse(existing.as_deref(), project_root)?;
    for (context, service, _, shape) in wanted {
        lock.accept(context, service, shape);
    }
    bynk_driver::contracts_lock::write(
        project_root,
        &bynk_project::contracts_lock::serialize(&lock),
    )
    .map_err(|e| e.to_string())
}

/// What a push that would break live callers prints: a caller this build still has
/// gets the callee's refusal, and one it no longer builds its own remedy, since a
/// whole-project deploy would not move it (#3562: named by its script).
///
/// #3797: each refusal is also an entry (`side` says which), and they come back for the stop.
pub(crate) fn report_callee_skews(
    skews: Vec<CalleeSkew>,
    graph: &BTreeMap<String, Vec<String>>,
    (environment, worker): (&str, &str),
    (runner, lock): (&str, &DeployLock),
) -> Vec<Entry> {
    let (built, orphaned) = split_orphaned_callers(skews, graph);
    let mut entries = Vec::new();
    if !built.is_empty() {
        let printed = callee_contract_skew_refusal(environment, worker, &built);
        colour::eprint(&printed);
        entries.push(callee_skew_entry(
            "callee",
            &printed,
            (environment, worker),
            &built,
        ));
    }
    if !orphaned.is_empty() {
        let script_of = |c: &str| lock.script_of(environment, c);
        let printed =
            orphaned_caller_skew_refusal(environment, worker, &orphaned, (runner, &script_of));
        colour::eprint(&printed);
        entries.push(callee_skew_entry(
            "orphaned_caller",
            &printed,
            (environment, worker),
            &orphaned,
        ));
    }
    entries
}

/// #3797: the refusal of a Worker compiled against a contract its live dependencies no longer
/// provide, as an entry: `side` is `caller`, each skew a dependency, service and both hashes.
pub(crate) fn contract_skew_entry(
    environment: &str,
    worker: &str,
    skews: &[ContractSkew],
    printed: &str,
) -> Entry {
    Entry::from_printed(CONTRACT_SKEW, printed)
        .with("side", "caller")
        .with("worker", deployed_worker_name(environment, worker))
        .with(
            "skews",
            skews
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "dependency": s.dependency,
                        "service": s.service,
                        "expected": s.expected,
                        "live": s.live,
                    })
                })
                .collect::<Vec<_>>(),
        )
}

/// #3797: the refusal of a callee whose build breaks live callers, as an entry: `side` is
/// `callee` for callers this build still has and `orphaned_caller` for ones it no longer builds.
fn callee_skew_entry(
    side: &str,
    printed: &str,
    (environment, worker): (&str, &str),
    skews: &[CalleeSkew],
) -> Entry {
    Entry::from_printed(CONTRACT_SKEW, printed)
        .with("side", side)
        .with("worker", deployed_worker_name(environment, worker))
        .with(
            "skews",
            skews
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "caller": deployed_worker_name(environment, &s.caller),
                        "dependency": s.dependency,
                        "service": s.service,
                        "expected": s.expected,
                        "provided": s.provided,
                    })
                })
                .collect::<Vec<_>>(),
        )
}

/// #2554: the lock entries for `worker`'s context that no live caller in
/// `environment` was compiled against: `(context, service, hash)`.
pub(crate) fn unused_accepted(
    contracts: &bynk_project::contracts_lock::ContractsLock,
    (worker, dir_of): (&str, &dyn Fn(&str) -> String),
    lock: &DeployLock,
    environment: &str,
) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (context, services) in contracts.contexts() {
        if dir_of(context) != worker {
            continue;
        }
        for (service, shapes) in services {
            for text in shapes {
                let Some(shape) = ServiceShape::from_text(text) else {
                    continue;
                };
                let hash = shape.hash();
                let used = lock.live_expects(environment).any(|(caller, expects)| {
                    caller != worker
                        && expects
                            .and_then(|e| e.get(context))
                            .and_then(|m| m.get(service))
                            == Some(&hash)
                });
                if !used {
                    out.push((context.clone(), service.clone(), hash));
                }
            }
        }
    }
    out
}

/// #2554: tell the author which accepted contracts nothing live needs.
pub(crate) fn unused_accepted_notice(unused: &[(String, String, String)]) -> String {
    let mut out = format!(
        "bynk: note: bynk.contracts.lock accepts {} older contract{} that no live caller was compiled against:\n",
        unused.len(),
        if unused.len() == 1 { "" } else { "s" }
    );
    for (context, service, hash) in unused {
        out.push_str(&format!("  {context}.{service} — {hash}\n"));
    }
    out.push_str("  Remove them from the lock when you no longer need them.\n");
    out
}

/// #2554: said when the pushed callee will answer live callers still on an
/// older, compatible contract.
pub(crate) fn accepted_skew_notice(
    environment: &str,
    worker: &str,
    accepted: &[CalleeSkew],
    recorded: bool,
    dry_run: bool,
) -> String {
    let mut out = format!(
        "bynk: `{}` will also answer {} live caller contract{} that {} compatible:\n",
        deployed_worker_name(environment, worker),
        accepted.len(),
        if accepted.len() == 1 { "" } else { "s" },
        if accepted.len() == 1 { "is" } else { "are" },
    );
    for s in accepted {
        out.push_str(&format!(
            "  `{}` calls {}.{} — compiled against {}\n",
            deployed_worker_name(environment, &s.caller),
            s.dependency,
            s.service,
            s.expected,
        ));
        push_compat(&mut out, s.compat.as_ref());
    }
    if dry_run {
        out.push_str(
            "  A real run would record these in bynk.contracts.lock and rebuild the callee to carry them. Nothing was written.\n",
        );
    } else if recorded {
        out.push_str(
            "  Recorded in bynk.contracts.lock: commit it. The callee answers these callers until they are redeployed; remove an entry once no live caller was compiled against it.\n",
        );
    }
    out
}

/// v0.177 (#643): the `--context` refusal for a Worker compiled against a
/// contract its live dependencies no longer provide. The Worker by its
/// [`deployed_worker_name`], the remedy repeating the run's `--env` (#2047).
/// Each skew names a contract (`<context>.<service>`), not a Worker.
pub(crate) fn contract_skew_refusal(
    environment: &str,
    worker: &str,
    skews: &[ContractSkew],
) -> String {
    let mut out = format!(
        "bynk: error: `{}` was compiled against a contract its live dependencies no longer provide ({CONTRACT_SKEW}):\n",
        deployed_worker_name(environment, worker)
    );
    for s in skews {
        out.push_str(&format!(
            "  {}.{} — compiled against {}, live is {}\n",
            s.dependency, s.service, s.expected, s.live
        ));
        push_compat(&mut out, s.compat.as_ref());
    }
    push_compat_note(
        &mut out,
        skews.iter().map(|s| s.compat.as_ref()),
        SkewSide::Caller,
        &skews
            .iter()
            .map(|s| s.dependency.as_str())
            .collect::<Vec<_>>(),
    );
    out.push_str(
        "  Deploying this would ship a caller its callee rejects (409 ContractMismatch) on every call.\n",
    );
    out.push_str(&format!(
        "  Deploy the whole project (`{}`) so both sides move together.\n",
        deploy_command(environment)
    ));
    out
}

/// #2077: the `--context` refusal for a callee whose new build no longer
/// provides the contract its live callers were compiled against — the callee
/// half of [`contract_skew_refusal`], under the same code. Each live caller and
/// the callee by their [`deployed_worker_name`]; each finding names the
/// contract (`<context>.<service>`) and both hashes; the remedy repeats the
/// run's `--env` ([`deploy_command`]). Pure, so the wording is goldened.
pub(crate) fn callee_contract_skew_refusal(
    environment: &str,
    worker: &str,
    skews: &[CalleeSkew],
) -> String {
    let mut out = format!(
        "bynk: error: `{}` no longer provides the contract its live callers were compiled against ({CONTRACT_SKEW}):\n",
        deployed_worker_name(environment, worker)
    );
    for s in skews {
        out.push_str(&format!(
            "  `{}` calls {}.{} — compiled against {}, this build provides {}\n",
            deployed_worker_name(environment, &s.caller),
            s.dependency,
            s.service,
            s.expected,
            s.provided
        ));
        push_compat(&mut out, s.compat.as_ref());
    }
    push_compat_note(
        &mut out,
        skews.iter().map(|s| s.compat.as_ref()),
        SkewSide::Callee,
        &[],
    );
    out.push_str(
        "  Deploying this would ship a callee that rejects its live callers (409 ContractMismatch) on every call.\n",
    );
    out.push_str(&format!(
        "  Deploy the whole project (`{}`) so both sides move together.\n",
        deploy_command(environment)
    ));
    out
}

/// #2077 review: the refusal for a callee whose new build breaks a live caller
/// this project no longer builds — a Worker of a removed context, still
/// recorded in the ledger (the plan's `orphan worker`). A whole-project deploy
/// neither redeploys nor checks it, and `--prune` never deletes a Worker or its
/// record, so the remedy is to retire it (delete the Worker, then drop its
/// ledger entry, without which this gate keeps refusing) or to restore its
/// context and deploy the whole project ([`deploy_command`]). Pure, so the
/// wording is goldened.
pub(crate) fn orphaned_caller_skew_refusal(
    environment: &str,
    worker: &str,
    skews: &[CalleeSkew],
    (runner, script_of): (&str, &dyn Fn(&str) -> String),
) -> String {
    let mut out = format!(
        "bynk: error: `{}` no longer provides the contract a live Worker this project no longer builds was compiled against ({CONTRACT_SKEW}):\n",
        deployed_worker_name(environment, worker)
    );
    for s in skews {
        out.push_str(&format!(
            "  `{}` (orphaned) calls {}.{} — compiled against {}, this build provides {}\n",
            deployed_worker_name(environment, &s.caller),
            s.dependency,
            s.service,
            s.expected,
            s.provided
        ));
        push_compat(&mut out, s.compat.as_ref());
    }
    push_compat_note(
        &mut out,
        skews.iter().map(|s| s.compat.as_ref()),
        SkewSide::Callee,
        &[],
    );
    out.push_str(
        "  Deploying this would ship a callee that rejects it (409 ContractMismatch) on every call, and a whole-project deploy would not move it: that reports an orphaned Worker but never redeploys or deletes it, and `--prune` never deletes a Worker either.\n",
    );
    let mut callers: Vec<&str> = skews.iter().map(|s| s.caller.as_str()).collect();
    callers.dedup();
    out.push_str(&format!(
        "  If it is unused, delete it ({}) and remove its entry ({}) from {LOCK_FILE}. If it is still needed, restore its context and deploy the whole project (`{}`).\n",
        callers
            .iter()
            .map(|c| format!(
                "`{runner} wrangler delete --name {}`",
                deployed_worker_name(environment, &script_of(c))
            ))
            .collect::<Vec<_>>()
            .join(", "),
        callers
            .iter()
            .map(|c| format!("`[environments.{environment}.workers.{c}]`"))
            .collect::<Vec<_>>()
            .join(", "),
        deploy_command(environment)
    ));
    out
}

/// #2077: the warning for a live caller of the pushed `worker` whose ledger
/// record predates recorded `expects`, so the callee gate cannot check it.
/// Never a refusal: silence is not a match.
pub(crate) fn unrecorded_caller_warning(environment: &str, worker: &str, caller: &str) -> String {
    format!(
        "bynk: warning: could not check that `{}` still provides what the live `{}` was compiled against — its deploy record predates recorded expectations ({CONTRACT_SKEW}). A whole-project deploy (`{}`) records them.\n",
        deployed_worker_name(environment, worker),
        deployed_worker_name(environment, caller),
        deploy_command(environment)
    )
}

#[cfg(test)]
mod entry_tests {
    use super::*;

    fn skew() -> ContractSkew {
        ContractSkew {
            dependency: "shop.billing".into(),
            service: "charge".into(),
            expected: "aaaa".into(),
            live: "bbbb".into(),
            compat: None,
        }
    }

    fn callee() -> CalleeSkew {
        CalleeSkew {
            caller: "shop-orders".into(),
            dependency: "shop.billing".into(),
            service: "charge".into(),
            expected: "aaaa".into(),
            provided: "cccc".into(),
            compat: None,
        }
    }

    #[test]
    fn a_caller_skew_entry_names_the_side_the_worker_and_each_hash() {
        let e = contract_skew_entry("default", "shop-orders", &[skew()], "bynk: error: skew\n");
        assert_eq!(e.code, "bynk.deploy.contract_skew");
        assert_eq!(e.subject["side"], "caller");
        assert_eq!(
            e.subject["worker"],
            deployed_worker_name("default", "shop-orders")
        );
        assert_eq!(
            e.subject["skews"],
            serde_json::json!([{
                "dependency": "shop.billing",
                "service": "charge",
                "expected": "aaaa",
                "live": "bbbb",
            }])
        );
    }

    #[test]
    fn a_callee_skew_entry_names_its_side_and_the_live_caller() {
        for side in ["callee", "orphaned_caller"] {
            let e = callee_skew_entry(
                side,
                "bynk: error: skew\n",
                ("default", "shop-billing"),
                &[callee()],
            );
            assert_eq!(e.code, "bynk.deploy.contract_skew");
            assert_eq!(e.subject["side"], side);
            assert_eq!(
                e.subject["worker"],
                deployed_worker_name("default", "shop-billing")
            );
            assert_eq!(
                e.subject["skews"],
                serde_json::json!([{
                    "caller": deployed_worker_name("default", "shop-orders"),
                    "dependency": "shop.billing",
                    "service": "charge",
                    "expected": "aaaa",
                    "provided": "cccc",
                }])
            );
        }
    }
}
