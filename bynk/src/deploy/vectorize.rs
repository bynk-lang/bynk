//! #3479: Vectorize indexes, which the deploy creates and never recreates.
//!
//! An index's dimensions and metric are fixed when it is created ("the
//! configuration of an index cannot be changed after creation"), and recreating
//! one deletes every vector in it. So the deploy creates a missing index with
//! the configuration the source declares, reuses one that matches, and fails on
//! one that does not, naming both configurations and the way out. Metadata
//! indexes are created straight after the index and before the Worker that
//! writes vectors is pushed, because a vector written before its property's
//! index existed is not covered by it until it is written again. Cloudflare
//! builds a metadata index asynchronously (`create-metadata-index` only
//! enqueues it), so the deploy waits for every one it just created to be listed
//! before it pushes the Worker; one that does not appear in time fails the deploy.
//!
//! Since 1D.4c (#4056) every call goes through Cloudflare's REST API, as KV, R2
//! and D1 do, so a Vectorize project needs `CLOUDFLARE_API_TOKEN` and
//! `CLOUDFLARE_ACCOUNT_ID` on every deploy (the index is checked each time). An
//! answer that cannot be understood fails the deploy and says so, and never
//! reads as "matches".

use super::*;
use bynk_driver::emsg;

use std::time::{Duration, Instant};

pub(crate) use crate::cloudflare::rest::vectorize::{IndexConfig, normalise_metric};

/// A mismatch between an existing index and the declaration, as the failure
/// the deploy reports. Pure, so the wording is tested without an account.
pub(crate) fn drift_message(
    index: &str,
    worker: &str,
    binding: &str,
    found: &IndexConfig,
    declared: &VectorizeDecl,
) -> String {
    format!(
        "the Vectorize index `{index}` for `{worker}`'s `{binding}` has {} dimensions and the `{}` metric, but the source declares {} dimensions and `{}`. Cloudflare cannot change an index's configuration after it is created, and recreating it would delete every vector in it, so nothing was changed. Rename the instance to get a new index, change the declaration back, or delete the index by hand and reindex",
        found.dimensions, found.metric, declared.dimensions, declared.metric
    )
}

/// A failed Vectorize call as the deploy says it, with the permission hint when the token lacks
/// it.
fn said(e: CloudflareError) -> String {
    with_permission_hint(&e)
}

/// #3495: a token without the Vectorize permission is answered with a bare authentication
/// error. Name the permission the call needs (it covered create, list, get, delete and both
/// metadata-index calls when checked against a real account); any other failure is said as is.
fn with_permission_hint(e: &CloudflareError) -> String {
    if e.is_denied() {
        format!("{e}\n  hint: the API token needs the Account > Vectorize > Edit permission")
    } else {
        e.to_string()
    }
}

/// Whether the account has an index called `name`.
pub(crate) fn index_exists(api: &dyn CloudflareApi, name: &str) -> Result<bool, String> {
    Ok(api
        .vectorize_names()
        .map_err(said)?
        .iter()
        .any(|n| n == name))
}

pub(crate) fn read_index_config(
    api: &dyn CloudflareApi,
    name: &str,
) -> Result<IndexConfig, String> {
    api.vectorize_config(name).map_err(|e| {
        format!(
            "{}; so `{name}` was not checked against the source",
            said(e)
        )
    })
}

pub(crate) fn create_index(
    api: &dyn CloudflareApi,
    name: &str,
    declared: &VectorizeDecl,
) -> Result<(), String> {
    api.vectorize_create(name, declared.dimensions, &declared.metric)
        .map_err(said)
}

pub(crate) fn existing_metadata_indexes(
    api: &dyn CloudflareApi,
    name: &str,
) -> Result<Vec<String>, String> {
    api.vectorize_metadata_indexes(name).map_err(said)
}

/// Enqueue the metadata index `entry` (`property:type`) on `name`. A create that Cloudflare
/// refuses may have found one already there: the reference documents no code for that, and one
/// listed between the listing and this create is exactly what Cloudflare's asynchronous builds
/// produce. So a refusal is `Ok(Some(what Cloudflare said))`, and the listing wait that follows
/// decides: the index listed is what was wanted, and one never listed fails the deploy with
/// that refusal in its message. Any other failure is the error.
pub(crate) fn create_metadata_index(
    api: &dyn CloudflareApi,
    name: &str,
    entry: &str,
) -> Result<Option<String>, String> {
    let (property, ty) = entry
        .split_once(':')
        .ok_or_else(|| format!("`{entry}` is not `name:type`"))?;
    match api.vectorize_create_metadata_index(name, property, ty) {
        Ok(()) => Ok(None),
        Err(e) if e.is_refusal() => Ok(Some(e.to_string())),
        Err(e) => Err(said(e)),
    }
}

/// How the deploy waits for new metadata indexes to be listed: the first delay,
/// the longest delay (each is double the last, up to this), and when to give up.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pacing {
    pub(crate) first: Duration,
    pub(crate) longest: Duration,
    pub(crate) timeout: Duration,
}

/// Five minutes: a metadata index was seen to take from 30 seconds to 144, and a
/// false timeout costs more than waiting.
pub(crate) const METADATA_INDEX_PACING: Pacing = Pacing {
    first: Duration::from_secs(3),
    longest: Duration::from_secs(15),
    timeout: Duration::from_secs(300),
};

/// Poll `list-metadata-index` until every entry of `wanted` (`name:type`) is
/// listed on `name`, backing off between polls. `Err` is what could not be read,
/// or the entries still unlisted at the timeout.
fn wait_for_metadata_indexes(
    api: &dyn CloudflareApi,
    name: &str,
    wanted: &[String],
    refused: &[String],
    pacing: Pacing,
) -> Result<(), String> {
    let start = Instant::now();
    let mut delay = pacing.first;
    let mut announced = false;
    loop {
        let listed = existing_metadata_indexes(api, name)?;
        // Listed means the property *and* its type: one of another type is not what was wanted,
        // and waiting will not change it.
        if let Some(message) = retyped_message(name, wanted, &listed) {
            return Err(message);
        }
        let pending: Vec<&String> = wanted.iter().filter(|w| !listed.contains(*w)).collect();
        if pending.is_empty() {
            if announced {
                emsg!(
                    "bynk: the metadata indexes on `{name}` are listed ({}s)",
                    start.elapsed().as_secs()
                );
            }
            return Ok(());
        }
        if start.elapsed() >= pacing.timeout {
            let refusals = if refused.is_empty() {
                String::new()
            } else {
                format!(
                    " (Cloudflare refused creating {}: {})",
                    if refused.len() == 1 { "one" } else { "some" },
                    refused.join("; ")
                )
            };
            return Err(format!(
                "the metadata index{} {} {} created but {} not listed after {} seconds{refusals}; Cloudflare builds them asynchronously. The Worker was not pushed: run the deploy again once they are listed",
                if pending.len() == 1 { "" } else { "es" },
                pending
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                if pending.len() == 1 { "was" } else { "were" },
                if pending.len() == 1 { "is" } else { "are" },
                pacing.timeout.as_secs()
            ));
        }
        if !announced {
            eprintln!(
                "bynk: waiting for Cloudflare to build the metadata indexes on `{name}` ({}), up to {} seconds",
                pending
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                pacing.timeout.as_secs()
            );
            announced = true;
        }
        std::thread::sleep(delay);
        delay = (delay * 2).min(pacing.longest);
    }
}

/// The failure for a wanted metadata index (`property:type`) that the listing shows under the
/// same property with another type: Cloudflare cannot retype one, so the deploy fails at once,
/// naming both types and the manual delete that clears it. `None` when no listed property has
/// another type than wanted. Pure.
fn retyped_message(name: &str, wanted: &[String], listed: &[String]) -> Option<String> {
    let (want, have) = wanted.iter().find_map(|want| {
        let (property, _) = want.split_once(':')?;
        let have = listed.iter().find(|have| {
            have.split_once(':').is_some_and(|(p, _)| p == property) && *have != want
        })?;
        Some((want, have))
    })?;
    let property = want.split_once(':').map_or(want.as_str(), |(p, _)| p);
    Some(format!(
        "the metadata index for `{property}` on `{name}` is listed as `{have}`, but the source declares `{want}`; Cloudflare cannot change a metadata index's type, so the Worker was not pushed. Delete it by hand (`wrangler vectorize delete-metadata-index {name} --propertyName {property}`) and deploy again, or declare `{have}`"
    ))
}

/// Delete the index `name` (`--prune`, after its confirmation). One already
/// gone is success, as for a queue: the ledger entry goes either way.
pub(crate) fn delete_vectorize_index(api: &dyn CloudflareApi, name: &str) -> Result<(), String> {
    match index_exists(api, name) {
        Ok(false) => return Ok(()),
        Ok(true) => {}
        Err(e) => return Err(e),
    }
    api.vectorize_delete(name).map_err(said)
}

/// The metadata indexes the declaration wants that the index lacks, and those
/// it has that the declaration no longer lists. Pure.
pub(crate) fn metadata_index_changes(
    declared: &[String],
    existing: &[String],
) -> (Vec<String>, Vec<String>) {
    let missing = declared
        .iter()
        .filter(|d| !existing.contains(d))
        .cloned()
        .collect();
    let removed = existing
        .iter()
        .filter(|e| !declared.contains(e))
        .cloned()
        .collect();
    (missing, removed)
}

/// The failure for a metadata index whose type the source changed
/// (`category:string` to `category:number`): the platform cannot retype one, so
/// the deploy says so before it creates anything. `None` when none changed.
pub(crate) fn type_change_message(
    index: &str,
    missing: &[String],
    removed: &[String],
) -> Option<String> {
    let changed: Vec<String> = missing
        .iter()
        .filter_map(|new| {
            let (property, _) = new.split_once(':')?;
            let old = removed
                .iter()
                .find(|r| r.split_once(':').is_some_and(|(p, _)| p == property))?;
            Some(format!("`{old}` is now `{new}`"))
        })
        .collect();
    (!changed.is_empty()).then(|| {
        format!(
            "the metadata index type of a property changed on `{index}` ({}); an index cannot be retyped. Keep the type, or declare the property under a new name",
            changed.join(", ")
        )
    })
}

/// Name the indexes `bynk.toml` adopts for `environment` on the declarations
/// (`[deploy.vectorize.<Capability>] index`). A capability with no entry for
/// the environment is created by the deploy, as before. Prints why and fails
/// when `bynk.toml` cannot be read.
pub(crate) fn apply_vectorize_adoptions(
    project_root: &Path,
    resources: &mut BTreeMap<String, Resources>,
    environment: &str,
) -> Result<(), ()> {
    if resources.values().all(|r| r.vectorize_indexes.is_empty()) {
        return Ok(());
    }
    let settings =
        bynk_project::vectorize::try_read_vectorize_deploy(project_root).map_err(|e| {
            emsg!("bynk: error: {e}");
        })?;
    // An entry that names no declared capability would be silently ignored,
    // and the deploy would create a fresh, empty index beside the one the team
    // meant to adopt: refuse it, with the nearest binding when there is one.
    let bindings: Vec<&str> = resources
        .values()
        .flat_map(|r| r.vectorize_indexes.iter().map(|d| d.binding.as_str()))
        .collect();
    for capability in settings.keys() {
        if resources
            .values()
            .flat_map(|r| &r.vectorize_indexes)
            .any(|d| d.matches_capability(capability))
        {
            continue;
        }
        let wanted = bynk_check::worker_bindings::instance_binding_name(
            bynk_check::firstparty::VECTORIZE_BINDING_PREFIX,
            capability,
        );
        let hint = bynk_project::paths::did_you_mean(&wanted, bindings.iter().copied())
            .map(|b| format!("; did you mean the capability whose binding is `{b}`?"))
            .unwrap_or_default();
        emsg!(
            "bynk: error: `[deploy.vectorize.{capability}]` in bynk.toml names no `Vectorize` capability in this project{hint}; an entry that matches nothing would have the deploy create a new, empty index instead of adopting yours"
        );
        return Err(());
    }
    for (capability, entry) in &settings {
        if !entry.index.contains_key(environment) {
            let named: Vec<&str> = entry.index.keys().map(String::as_str).collect();
            emsg!(
                "bynk: note: `[deploy.vectorize.{capability}]` adopts an index only for {}; under `{environment}` the deploy creates one",
                named.join(", ")
            );
        }
    }
    for declared in resources.values_mut() {
        for decl in &mut declared.vectorize_indexes {
            decl.adopt = settings
                .iter()
                .find(|(capability, _)| decl.matches_capability(capability))
                .and_then(|(_, entry)| entry.index.get(environment))
                .cloned();
        }
    }
    Ok(())
}

/// An adopted index: it has to exist (the deploy never creates it) and match
/// the declaration (it never changes it). A metadata index it lacks is said,
/// with the command that makes it, and not created: the index is not ours to
/// change.
fn adopt_index(
    api: &dyn CloudflareApi,
    worker: &str,
    decl: &VectorizeDecl,
    name: &str,
) -> Result<(), DeployFailure> {
    let fail = |e: String| {
        DeployFailure::driver(format!(
            "could not check the adopted Vectorize index `{name}` for `{worker}`'s `{}`: {e}",
            decl.binding
        ))
    };
    if !index_exists(api, name).map_err(&fail)? {
        return Err(DeployFailure::driver(format!(
            "the Vectorize index `{name}`, which bynk.toml adopts for `{worker}`'s `{}`, does not exist on the account; an adopted index is never created, so create it with `wrangler vectorize create {name} --dimensions {} --metric {}`, or remove the `index` entry to have the deploy create one",
            decl.binding, decl.dimensions, decl.metric
        )));
    }
    let found = read_index_config(api, name).map_err(&fail)?;
    if found.dimensions != decl.dimensions || found.metric != normalise_metric(&decl.metric) {
        return Err(DeployFailure::driver(drift_message(
            name,
            worker,
            &decl.binding,
            &found,
            decl,
        )));
    }
    let existing = existing_metadata_indexes(api, name).map_err(&fail)?;
    let (missing, _) = metadata_index_changes(&decl.indexed, &existing);
    for entry in &missing {
        if let Some((property, ty)) = entry.split_once(':') {
            eprintln!(
                "{}",
                unlisted_metadata_index_warning(name, entry, property, ty)
            );
        }
    }
    Ok(())
}

/// #3511: the warning for a metadata index an adopted index does not list. It
/// says "not listed", not "has none": Cloudflare builds a metadata index
/// asynchronously, so one another deploy has only just enqueued is not listed
/// yet, and a claim that it is missing would be a false alarm.
fn unlisted_metadata_index_warning(name: &str, entry: &str, property: &str, ty: &str) -> String {
    format!(
        "bynk: warning: the adopted index `{name}` does not list a metadata index for `{entry}` yet. If you or another deploy has just created it, Cloudflare may still be building it (it can take a few minutes): check `wrangler vectorize list-metadata-index {name}` before acting. Until it is listed, a query that filters on `{property}` matches nothing; if it never appears, create it (`wrangler vectorize create-metadata-index {name} --propertyName {property} --type {ty}`) and write the vectors again"
    )
}

/// An adopted index keeps the name it has on the account, not the derived one
/// the generated config carries: written into `config` for `environment`.
pub(crate) fn materialise_adopted_vectorize(
    config: &Path,
    worker: &str,
    declared: &Resources,
    environment: &str,
) -> Result<(), DeployFailure> {
    let adopted: BTreeMap<String, String> = declared
        .vectorize_indexes
        .iter()
        .filter(|v| v.adopt.is_some())
        .map(|v| (v.binding.clone(), v.physical_name(environment)))
        .collect();
    if adopted.is_empty() {
        return Ok(());
    }
    let text = std::fs::read_to_string(config).map_err(|e| {
        DeployFailure::driver(format!(
            "could not read `{worker}`'s generated configuration: {e}"
        ))
    })?;
    let patched = bynk_emit::emitter::wrangler::materialise_vectorize_index_names(
        &text,
        environment,
        &adopted,
    )
    .map_err(|e| {
        DeployFailure::driver(format!(
            "could not name the adopted Vectorize indexes in `{worker}`'s configuration: {e}"
        ))
    })?;
    std::fs::write(config, patched).map_err(|e| {
        DeployFailure::driver(format!(
            "could not write `{worker}`'s adopted Vectorize index names: {e}"
        ))
    })
}

/// One `Vectorize` index's `Index` step (1D.1c: [`Vectorize`]'s provisioning; 1D.2b its step):
/// created when the account has none of that name, checked against the declaration when it has (a
/// mismatch fails the deploy and changes nothing), and recorded in the ledger before the push. Its
/// metadata indexes are the next step ([`apply_metadata_indexes`]), so the record is written
/// first. These account checks are the kind's guards (#4001 Decision B).
pub(crate) fn apply_vectorize_index(
    cx: &StepCx<'_, '_, Vectorize>,
    step: &VectorizeIndexStep<'_>,
) -> Result<Applied<Vectorize>, DeployFailure> {
    let (worker, decl) = (cx.worker, step.decl);
    let api = cx.tools.api;
    let physical = step.physical.clone();
    if decl.adopt.is_some() {
        adopt_index(api, worker, decl, &physical)?;
        let already = step
            .recorded
            .as_ref()
            .is_some_and(|(name, adopted)| *name == physical && *adopted);
        let mut adopted = Applied::nothing();
        if !already {
            adopted = adopted.with(record(
                cx,
                decl,
                VectorizeRecord::of(&physical, true),
                &format!("adopted the Vectorize index `{physical}`"),
            ));
        }
        return Ok(adopted);
    }
    let fail = |what: &str, e: String| {
        DeployFailure::driver(format!(
            "could not {what} the Vectorize index `{physical}` for `{worker}`'s `{}`: {e}",
            decl.binding
        ))
    };
    let existed = index_exists(api, &physical).map_err(|e| fail("look up", e))?;
    if existed {
        let found = read_index_config(api, &physical).map_err(|e| fail("check", e))?;
        let matches =
            found.dimensions == decl.dimensions && found.metric == normalise_metric(&decl.metric);
        if !matches {
            return Err(DeployFailure::driver(drift_message(
                &physical,
                worker,
                &decl.binding,
                &found,
                decl,
            )));
        }
    } else {
        create_index(api, &physical, decl).map_err(|e| fail("create", e))?;
    }
    // Recorded before the push, as every created resource is: an
    // interrupted run never forgets an index it made.
    let already = step
        .recorded
        .as_ref()
        .is_some_and(|(name, _)| *name == physical);
    let mut applied = Applied::nothing();
    if !already {
        // An index already on the account was not made by this deploy, so
        // it is recorded as adopted: never pruned.
        applied = applied.with(record(
            cx,
            decl,
            VectorizeRecord::of(&physical, existed),
            &format!("provisioned the Vectorize index `{physical}`"),
        ));
    }
    // For the `MetadataIndexes` step, which follows once this one's record is written.
    step.existed.set(Some(existed));
    Ok(applied)
}

/// One `Vectorize` index's `MetadataIndexes` step: the metadata indexes it lacks, made and waited
/// for, now that the index is recorded. Nothing for an adopted index, whose `Index` step leaves
/// no fact here, or one whose `Index` step did not run.
pub(crate) fn apply_metadata_indexes(
    cx: &StepCx<'_, '_, Vectorize>,
    step: &VectorizeMetadataStep<'_>,
) -> Result<Applied<Vectorize>, DeployFailure> {
    if let Some(existed) = step.existed.get() {
        provision_metadata_indexes(cx, step.decl, &step.physical, existed, step.pacing)?;
    }
    Ok(Applied::nothing())
}

/// `decl`'s record of index `physical`, for the apply to write.
fn record(
    cx: &StepCx<'_, '_, Vectorize>,
    decl: &VectorizeDecl,
    record: VectorizeRecord,
    done: &str,
) -> Commit<VectorizeChange> {
    Commit::recorded(
        VectorizeChange::Record(TableChange::insert(cx.worker, &decl.binding, record)),
        done,
    )
}

/// The metadata indexes of index `physical` that `decl` lists and it lacks, made and waited for
/// once the index is recorded. `existed`: whether the account had the index before this run.
fn provision_metadata_indexes(
    cx: &StepCx<'_, '_, Vectorize>,
    decl: &VectorizeDecl,
    physical: &str,
    existed: bool,
    pacing: Pacing,
) -> Result<(), DeployFailure> {
    let (worker, environment) = (cx.worker, cx.environment);
    let api = cx.tools.api;
    let fail = |what: &str, e: String| {
        DeployFailure::driver(format!(
            "could not {what} the Vectorize index `{physical}` for `{worker}`'s `{}`: {e}",
            decl.binding
        ))
    };
    let existing = existing_metadata_indexes(api, physical)
        .map_err(|e| fail("read the metadata indexes of", e))?;
    let (missing, removed) = metadata_index_changes(&decl.indexed, &existing);
    if let Some(message) = type_change_message(physical, &missing, &removed) {
        return Err(DeployFailure::driver(message));
    }
    let mut refused = Vec::new();
    for entry in &missing {
        refused.extend(
            create_metadata_index(api, physical, entry)
                .map_err(|e| fail(&format!("create the metadata index `{entry}` on"), e))?,
        );
    }
    if !missing.is_empty() {
        // Only now is the index safe to write to: a vector upserted before a
        // metadata index is listed is silently left out of it.
        wait_for_metadata_indexes(api, physical, &missing, &refused, pacing).map_err(|e| {
            DeployFailure::driver(format!(
                "the Vectorize index `{physical}` for `{worker}`'s `{}`: {e}",
                decl.binding
            ))
        })?;
    }
    // Vectors can predate a metadata index only if something already wrote
    // to the index: an adopted one, or one this project's Worker was
    // deployed against. An index this very project created and never
    // deployed has none, which is also the state a deploy that timed out
    // above is re-run from.
    let may_hold_vectors = cx
        .ledger()
        .recorded::<Vectorize>(worker, &decl.binding)
        .is_some_and(|r| r.adopted)
        || cx.lock.is_deployed(environment, worker);
    if existed && may_hold_vectors && !missing.is_empty() {
        emsg!(
            "bynk: warning: `{physical}` already existed, so vectors written before {} will not be filterable on {} until they are written again (upsert them)",
            if missing.len() == 1 {
                "its metadata index"
            } else {
                "those metadata indexes"
            },
            missing.join(", ")
        );
    }
    if !removed.is_empty() {
        emsg!(
            "bynk: note: `{physical}` has metadata indexes the source no longer lists ({}); they are left in place, and a query that filters on one still works",
            removed.join(", ")
        );
    }
    Ok(())
}

/// Each of `worker`'s `Vectorize` indexes, as the deploy provisions them, waiting `pacing` for
/// their metadata indexes: what the recording-fake tests drive.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn provision_vectorize_indexes_paced(
    api: &dyn CloudflareApi,
    project_root: &Path,
    worker: &str,
    declared: &Resources,
    lock: &mut DeployLock,
    lock_path: &Path,
    environment: &str,
    pacing: Pacing,
) -> Result<(), DeployFailure> {
    let project = BTreeMap::new();
    let site = Site {
        tools: Tools { api, project_root },
        environment,
        worker,
        project: &project,
    };
    let mut writer = LedgerWriter::new(lock, lock_path);
    let steps = vectorize_steps(
        &PlanCx {
            ledger: LedgerView::of(writer.lock(), environment),
            worker,
        },
        declared,
        pacing,
    );
    apply_kind::<Vectorize>(site, &steps, &mut writer, Lives::default())?;
    Ok(())
}

#[cfg(test)]
mod tests;
