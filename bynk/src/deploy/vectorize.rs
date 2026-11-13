//! #3479: Vectorize indexes, which the deploy creates and never recreates.
//!
//! An index's dimensions and metric are fixed when it is created ("the
//! configuration of an index cannot be changed after creation"), and recreating
//! one deletes every vector in it. So the deploy creates a missing index with
//! the configuration the source declares, reuses one that matches, and fails on
//! one that does not, naming both configurations and the way out. Metadata
//! indexes are created straight after the index and before the Worker that
//! writes vectors is pushed, because a vector written before its property's
//! index existed is not covered by it until it is written again.
//!
//! Everything goes through wrangler, as D1 and R2 do, so the token the deploy
//! needs does not grow. Wrangler's `--json` output is not documented field by
//! field, so what is read from it is read defensively: an output that cannot
//! be understood fails the deploy and says so, and never reads as "matches".

use super::*;

use serde_json::Value;

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
        Err(wrangler_said(&output))
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
    vectorize_command(
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
    )
    .map(|_| ())
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
    if found.dimensions != decl.dimensions || found.metric != decl.metric {
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
            let matches = found.dimensions == decl.dimensions && found.metric == decl.metric;
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
            lock.record_vectorize(environment, worker, &decl.binding, &physical, false);
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
        for entry in &missing {
            create_metadata_index(provenance, &physical, entry, project_root)
                .map_err(|e| fail(&format!("create the metadata index `{entry}` on"), e))?;
        }
        if existed && !missing.is_empty() {
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
