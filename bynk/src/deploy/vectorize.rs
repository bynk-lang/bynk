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
//! Everything goes through wrangler, as D1 and R2 do, so the token the deploy
//! needs does not grow. Wrangler's `--json` output is not documented field by
//! field, so what is read from it is read defensively: an output that cannot
//! be understood fails the deploy and says so, and never reads as "matches".

use super::*;

use serde_json::Value;
use std::time::{Duration, Instant};

/// An index's fixed configuration, as the account reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexConfig {
    pub(crate) dimensions: i64,
    pub(crate) metric: String,
}

/// A metric as the declaration spells it (`dot-product`), whatever case or
/// separator the account's output uses.
fn normalise_metric(metric: &str) -> String {
    metric.trim().to_ascii_lowercase().replace('_', "-")
}

/// An index's configuration from `wrangler vectorize get --json`: at the top
/// level or under `config`. `None` when it is neither, so the caller can say it
/// could not read it rather than guess.
pub(crate) fn parse_index_config(json: &str) -> Option<IndexConfig> {
    let value: Value = serde_json::from_str(json).ok()?;
    let config = value.get("config").unwrap_or(&value);
    Some(IndexConfig {
        dimensions: config.get("dimensions")?.as_i64()?,
        metric: normalise_metric(config.get("metric")?.as_str()?),
    })
}

/// The names in `wrangler vectorize list --json`: an array of objects with a
/// `name`. `None` when it is not that.
pub(crate) fn parse_index_names(json: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(json).ok()?;
    value
        .as_array()?
        .iter()
        .map(|i| i.get("name")?.as_str().map(str::to_string))
        .collect()
}

/// The metadata indexes in `wrangler vectorize list-metadata-index --json`, as
/// `name:type`: an array of objects naming the property and its type. `None`
/// when it is not that.
pub(crate) fn parse_metadata_indexes(json: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(json).ok()?;
    // Wrangler has wrapped lists in an object (`{ "metadataIndexes": [...] }`).
    let list = value
        .as_array()
        .or_else(|| value.as_object()?.values().find_map(Value::as_array))?;
    list.iter()
        .map(|i| {
            let name = ["propertyName", "property_name", "name"]
                .iter()
                .find_map(|k| i.get(*k)?.as_str())?;
            let ty = ["indexType", "index_type", "type"]
                .iter()
                .find_map(|k| i.get(*k)?.as_str())?;
            Some(format!("{name}:{}", ty.to_ascii_lowercase()))
        })
        .collect()
}

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

/// Run `wrangler vectorize <args…>` from the project root; its stdout on
/// success, and what wrangler said on failure.
fn vectorize_command(
    provenance: &Provenance,
    args: &[&str],
    project_root: &Path,
) -> Result<String, String> {
    let Some(mut command) = workers::wrangler_command(provenance, "vectorize") else {
        return Err("wrangler not found".into());
    };
    // As for the other resources: run from the project root, where wrangler
    // cannot load and reject a generated config that is still incomplete.
    let output = command
        .args(args)
        .current_dir(project_root)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(with_permission_hint(wrangler_said(&output)))
    }
}

/// #3495: Cloudflare answers a token without the Vectorize permission with a
/// bare `Authentication error [code: 10000]`. Name the permission that call
/// needs (it covered create, list, get, delete and both metadata-index calls
/// when checked against a real account); any other message is returned as is.
fn with_permission_hint(said: String) -> String {
    if said.contains("code: 10000") {
        format!("{said}\n  hint: the API token needs the Account > Vectorize > Edit permission")
    } else {
        said
    }
}

/// Whether the account has an index called `name`.
fn index_exists(provenance: &Provenance, name: &str, project_root: &Path) -> Result<bool, String> {
    let out = vectorize_command(provenance, &["list", "--json"], project_root)?;
    let names = parse_index_names(&out)
        .ok_or("could not read the list of Vectorize indexes from wrangler's output")?;
    Ok(names.iter().any(|n| n == name))
}

fn read_index_config(
    provenance: &Provenance,
    name: &str,
    project_root: &Path,
) -> Result<IndexConfig, String> {
    let out = vectorize_command(provenance, &["get", name, "--json"], project_root)?;
    parse_index_config(&out).ok_or_else(|| {
        format!("could not read the dimensions and metric of `{name}` from wrangler's output, so it was not checked against the source")
    })
}

fn create_index(
    provenance: &Provenance,
    name: &str,
    declared: &VectorizeDecl,
    project_root: &Path,
) -> Result<(), String> {
    let dimensions = declared.dimensions.to_string();
    vectorize_command(
        provenance,
        &[
            "create",
            name,
            "--dimensions",
            &dimensions,
            "--metric",
            &declared.metric,
        ],
        project_root,
    )
    .map(|_| ())
}

fn existing_metadata_indexes(
    provenance: &Provenance,
    name: &str,
    project_root: &Path,
) -> Result<Vec<String>, String> {
    let out = vectorize_command(
        provenance,
        &["list-metadata-index", name, "--json"],
        project_root,
    )?;
    parse_metadata_indexes(&out)
        .ok_or_else(|| "could not read the metadata indexes from wrangler's output".to_string())
}

fn create_metadata_index(
    provenance: &Provenance,
    name: &str,
    entry: &str,
    project_root: &Path,
) -> Result<(), String> {
    let (property, ty) = entry
        .split_once(':')
        .ok_or_else(|| format!("`{entry}` is not `name:type`"))?;
    match vectorize_command(
        provenance,
        &[
            "create-metadata-index",
            name,
            "--propertyName",
            property,
            "--type",
            ty,
        ],
        project_root,
    ) {
        Ok(_) => Ok(()),
        // Listed between the listing and this request (Cloudflare builds them
        // asynchronously): the property is there, which is what was wanted.
        Err(e) if e.contains("metadata index already exists") => Ok(()),
        Err(e) => Err(e),
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
    provenance: &Provenance,
    name: &str,
    wanted: &[String],
    project_root: &Path,
    pacing: Pacing,
) -> Result<(), String> {
    let start = Instant::now();
    let mut delay = pacing.first;
    let mut announced = false;
    loop {
        let listed = existing_metadata_indexes(provenance, name, project_root)?;
        let pending: Vec<&String> = wanted.iter().filter(|w| !listed.contains(*w)).collect();
        if pending.is_empty() {
            if announced {
                eprintln!(
                    "bynk: the metadata indexes on `{name}` are listed ({}s)",
                    start.elapsed().as_secs()
                );
            }
            return Ok(());
        }
        if start.elapsed() >= pacing.timeout {
            return Err(format!(
                "the metadata index{} {} {} created but {} not listed after {} seconds; Cloudflare builds them asynchronously. The Worker was not pushed: run the deploy again once they are listed",
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

/// Delete the index `name` (`--prune`, after its confirmation). One already
/// gone is success, as for a queue: the ledger entry goes either way.
pub(crate) fn delete_vectorize_index(
    provenance: &Provenance,
    name: &str,
    project_root: &Path,
) -> Result<(), String> {
    match index_exists(provenance, name, project_root) {
        Ok(false) => return Ok(()),
        Ok(true) => {}
        Err(e) => return Err(e),
    }
    vectorize_command(provenance, &["delete", name, "--force"], project_root).map(|_| ())
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
            eprintln!("bynk: {e}");
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
        eprintln!(
            "bynk: `[deploy.vectorize.{capability}]` in bynk.toml names no `Vectorize` capability in this project{hint}; an entry that matches nothing would have the deploy create a new, empty index instead of adopting yours"
        );
        return Err(());
    }
    for (capability, entry) in &settings {
        if !entry.index.contains_key(environment) {
            let named: Vec<&str> = entry.index.keys().map(String::as_str).collect();
            eprintln!(
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
    provenance: &Provenance,
    project_root: &Path,
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
    if !index_exists(provenance, name, project_root).map_err(&fail)? {
        return Err(DeployFailure::driver(format!(
            "the Vectorize index `{name}`, which bynk.toml adopts for `{worker}`'s `{}`, does not exist on the account; an adopted index is never created, so create it with `wrangler vectorize create {name} --dimensions {} --metric {}`, or remove the `index` entry to have the deploy create one",
            decl.binding, decl.dimensions, decl.metric
        )));
    }
    let found = read_index_config(provenance, name, project_root).map_err(&fail)?;
    if found.dimensions != decl.dimensions || found.metric != normalise_metric(&decl.metric) {
        return Err(DeployFailure::driver(drift_message(
            name,
            worker,
            &decl.binding,
            &found,
            decl,
        )));
    }
    let existing = existing_metadata_indexes(provenance, name, project_root).map_err(&fail)?;
    let (missing, _) = metadata_index_changes(&decl.indexed, &existing);
    for entry in &missing {
        if let Some((property, ty)) = entry.split_once(':') {
            eprintln!(
                "bynk: warning: the adopted index `{name}` has no metadata index for `{entry}`, so a query that filters on `{property}` matches nothing until you create one (`wrangler vectorize create-metadata-index {name} --propertyName {property} --type {ty}`) and write the vectors again"
            );
        }
    }
    Ok(())
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

/// Each `Vectorize` index of `worker`: created when the account has none of
/// that name, checked against the declaration when it has (a mismatch fails the
/// deploy and changes nothing), recorded in the ledger before the push, then its
/// metadata indexes made before any vector can be written.
pub(crate) fn provision_vectorize_indexes(
    provenance: &Provenance,
    project_root: &Path,
    worker: &str,
    declared: &Resources,
    lock: &mut DeployLock,
    lock_path: &Path,
    environment: &str,
) -> Result<(), DeployFailure> {
    provision_vectorize_indexes_paced(
        provenance,
        project_root,
        worker,
        declared,
        lock,
        lock_path,
        environment,
        METADATA_INDEX_PACING,
    )
}

#[allow(clippy::too_many_arguments)]
fn provision_vectorize_indexes_paced(
    provenance: &Provenance,
    project_root: &Path,
    worker: &str,
    declared: &Resources,
    lock: &mut DeployLock,
    lock_path: &Path,
    environment: &str,
    pacing: Pacing,
) -> Result<(), DeployFailure> {
    for decl in &declared.vectorize_indexes {
        let physical = decl.physical_name(environment);
        if decl.adopt.is_some() {
            adopt_index(provenance, project_root, worker, decl, &physical)?;
            let already = recorded_vectorize(lock, worker, &decl.binding, environment)
                .is_some_and(|r| r.name == physical && r.adopted);
            if !already {
                lock.record_vectorize(environment, worker, &decl.binding, &physical, true);
                write_lock(lock_path, lock).map_err(|e| {
                    DeployFailure::driver(format!(
                        "adopted the Vectorize index `{physical}` but could not record it in {}: {e}",
                        lock_path.display()
                    ))
                })?;
            }
            continue;
        }
        let fail = |what: &str, e: String| {
            DeployFailure::driver(format!(
                "could not {what} the Vectorize index `{physical}` for `{worker}`'s `{}`: {e}",
                decl.binding
            ))
        };
        let existed =
            index_exists(provenance, &physical, project_root).map_err(|e| fail("look up", e))?;
        if existed {
            let found = read_index_config(provenance, &physical, project_root)
                .map_err(|e| fail("check", e))?;
            let matches = found.dimensions == decl.dimensions
                && found.metric == normalise_metric(&decl.metric);
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
            create_index(provenance, &physical, decl, project_root)
                .map_err(|e| fail("create", e))?;
        }
        // Recorded before the push, as every created resource is: an
        // interrupted run never forgets an index it made.
        let already = recorded_vectorize(lock, worker, &decl.binding, environment)
            .is_some_and(|r| r.name == physical);
        if !already {
            // An index already on the account was not made by this deploy, so
            // it is recorded as adopted: never pruned.
            lock.record_vectorize(environment, worker, &decl.binding, &physical, existed);
            write_lock(lock_path, lock).map_err(|e| {
                DeployFailure::driver(format!(
                    "provisioned the Vectorize index `{physical}` but could not record it in {}: {e}",
                    lock_path.display()
                ))
            })?;
        }
        let existing = existing_metadata_indexes(provenance, &physical, project_root)
            .map_err(|e| fail("read the metadata indexes of", e))?;
        let (missing, removed) = metadata_index_changes(&decl.indexed, &existing);
        if let Some(message) = type_change_message(&physical, &missing, &removed) {
            return Err(DeployFailure::driver(message));
        }
        for entry in &missing {
            create_metadata_index(provenance, &physical, entry, project_root)
                .map_err(|e| fail(&format!("create the metadata index `{entry}` on"), e))?;
        }
        if !missing.is_empty() {
            // Only now is the index safe to write to: a vector upserted before a
            // metadata index is listed is silently left out of it.
            wait_for_metadata_indexes(provenance, &physical, &missing, project_root, pacing)
                .map_err(|e| {
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
        let may_hold_vectors = recorded_vectorize(lock, worker, &decl.binding, environment)
            .is_some_and(|r| r.adopted)
            || lock.is_deployed(environment, worker);
        if existed && may_hold_vectors && !missing.is_empty() {
            eprintln!(
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
            eprintln!(
                "bynk: note: `{physical}` has metadata indexes the source no longer lists ({}); they are left in place, and a query that filters on one still works",
                removed.join(", ")
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
