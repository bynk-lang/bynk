//! The `[env.<name>]` block of a named-environment deploy (back-half paydown 1C.2, Decision A of
//! #4039).
//!
//! **Why a named environment needs a block at all.** Wrangler does not inherit a Worker's
//! bindings into a named environment: a bare `--env staging` against the flat config
//! [`super::emit_wrangler_toml`] writes would deploy with *no* bindings. The block restates what
//! Wrangler does not inherit, and it cannot be written at build time, because no environment is
//! known then and its values (the ids the deploy has just resolved, the environment's names) do
//! not exist until the deploy.
//!
//! **What this renders it from.** The generated `wrangler.toml` as the deploy has materialised it
//! for the environment, which is the file Wrangler is about to upload, so the top level and the
//! block cannot disagree (ADR 0193 D3, ADR 0254 D2). What it does with each top-level key is the
//! key's [`WranglerKey::environment_rule`], one exhaustive `match` over every table the emitter
//! writes: a binding table cannot be left out of the block by omission (#3577, #3586). A key that
//! is not one of the emitter's is dropped.
//!
//! **The bytes.** The block is a [`toml::Table`], keys sorted, written by
//! `toml::to_string_pretty` and appended after a newline. The top level is never touched. This
//! layout is what `bynk deploy` has always handed Wrangler, and it stays (no format change).

use std::collections::{BTreeMap, BTreeSet};

use bynk_project::environment::{analytics_dataset_name, deployed_worker_name, env_qualify};
use bynk_project::wrangler_keys::{EnvironmentRule, Qualify, WranglerKey};

/// What the deploy has resolved for the environment that the generated file does not say.
pub struct Resolved<'a> {
    /// The resolved id of each KV namespace, by binding. Empty for a Worker with no KV.
    pub kv_ids: &'a BTreeMap<String, String>,
    /// Interop I5 (#1972): the bindings of Service Bindings to foreign Workers, whose targets
    /// are copied as declared.
    pub foreign_bindings: &'a BTreeSet<String>,
}

/// `config_text` with the `[env.<environment>]` block appended, or unchanged when the block
/// would be empty (a config that names nothing and binds nothing, which `bynk build` never
/// emits).
pub fn render(config_text: &str, environment: &str, resolved: &Resolved) -> Result<String, String> {
    let doc: toml::Table = config_text
        .parse()
        .map_err(|e| format!("generated configuration is not valid TOML: {e}"))?;
    let mut block = toml::Table::new();
    for (key, value) in &doc {
        let Some(rule) = rule_of(key) else {
            continue;
        };
        if let Some(value) = apply(rule, value, environment, resolved) {
            block.insert(key.clone(), value);
        }
    }
    if block.is_empty() {
        return Ok(config_text.to_string());
    }
    let mut env_table = toml::Table::new();
    env_table.insert(environment.to_string(), toml::Value::Table(block));
    let mut wrapper = toml::Table::new();
    wrapper.insert("env".to_string(), toml::Value::Table(env_table));
    let appended = toml::to_string_pretty(&wrapper)
        .map_err(|e| format!("could not serialise the `[env.{environment}]` block: {e}"))?;
    Ok(format!("{config_text}\n{appended}"))
}

/// The rule for the top-level key `key`: the emitter's own, or a one-per-Worker table the
/// registry names (copied). `None` for a key the emitter does not write.
fn rule_of(key: &str) -> Option<EnvironmentRule> {
    WranglerKey::from_top_key(key)
        .map(WranglerKey::environment_rule)
        .or_else(|| {
            singleton_tables()
                .any(|table| table == key)
                .then_some(EnvironmentRule::Copied)
        })
}

/// The one-per-Worker tables of `bynk.cloudflare` (`version_metadata`, `ai`, `images`,
/// `analytics`), as `bynk-std`'s registry spells them.
fn singleton_tables() -> impl Iterator<Item = &'static str> {
    bynk_check::firstparty::unit(bynk_check::firstparty::CLOUDFLARE_UNIT)
        .into_iter()
        .flat_map(|unit| unit.singletons.iter().map(|s| s.wrangler_table))
}

/// The block's value for a top-level `value` under `rule`, or `None` when the block has none.
fn apply(
    rule: EnvironmentRule,
    value: &toml::Value,
    environment: &str,
    resolved: &Resolved,
) -> Option<toml::Value> {
    match rule {
        EnvironmentRule::Inherited => None,
        EnvironmentRule::Copied => Some(value.clone()),
        // #2028: the deployed name, from the function the plan prints it with.
        EnvironmentRule::DeployedName => value
            .as_str()
            .map(|name| toml::Value::String(deployed_worker_name(environment, name))),
        // S4b (#1967): each namespace's resolved id, by binding. A Worker with no ids resolved
        // restates none of its namespaces.
        EnvironmentRule::ResolvedIds { field } => {
            if resolved.kv_ids.is_empty() {
                return None;
            }
            let mut entries = array_of(value)?;
            for entry in &mut entries {
                if let toml::Value::Table(t) = entry
                    && let Some(id) = t
                        .get("binding")
                        .and_then(toml::Value::as_str)
                        .and_then(|b| resolved.kv_ids.get(b))
                {
                    t.insert(field.to_string(), toml::Value::String(id.clone()));
                }
            }
            Some(toml::Value::Array(entries))
        }
        // P5 (#2323), #3479, P3b (#2582): each entry under its environment-qualified name, or
        // staging would write into the default environment's bucket, index or dataset.
        EnvironmentRule::Qualified { field, form } => {
            let mut entries = array_of(value)?;
            for entry in &mut entries {
                qualify_field(entry, field, |name| match form {
                    Qualify::Suffix => env_qualify(environment, name),
                    Qualify::Dataset => analytics_dataset_name(environment, name),
                });
            }
            Some(toml::Value::Array(entries))
        }
        // #3513 B: the `[vars]` the dataset names travel in (`Analytics.dataset()` reads them),
        // each under the qualified name its binding now has, or `dataset()` would name the
        // default environment's table from a staging Worker. `vars` is not inherited either.
        EnvironmentRule::QualifiedVars { prefix } => {
            let toml::Value::Table(vars) = value else {
                return None;
            };
            let carried = vars
                .iter()
                .map(|(key, value)| {
                    let value = match value {
                        toml::Value::String(name) if key.starts_with(prefix) => {
                            toml::Value::String(analytics_dataset_name(environment, name))
                        }
                        toml::Value::String(_)
                        | toml::Value::Integer(_)
                        | toml::Value::Float(_)
                        | toml::Value::Boolean(_)
                        | toml::Value::Datetime(_)
                        | toml::Value::Array(_)
                        | toml::Value::Table(_) => value.clone(),
                    };
                    (key.clone(), value)
                })
                .collect();
            Some(toml::Value::Table(carried))
        }
        // P2 (#2578): a producer's queue is qualified as a consumer's is, or staging would send
        // into the default environment's queue.
        EnvironmentRule::QualifiedQueues => {
            let mut queues = value.clone();
            if let toml::Value::Table(q) = &mut queues {
                for side in ["consumers", "producers"] {
                    let Some(toml::Value::Array(entries)) = q.get_mut(side) else {
                        continue;
                    };
                    for entry in entries.iter_mut() {
                        qualify_field(entry, "queue", |name| env_qualify(environment, name));
                        // #3877: the dead-letter queue is qualified as the consumer's is.
                        qualify_field(entry, "dead_letter_queue", |name| {
                            env_qualify(environment, name)
                        });
                    }
                }
            }
            Some(queues)
        }
        // A Service Binding to a project Worker names the Worker's deployed name; one to a
        // foreign Worker, which Bynk did not name, is copied as declared (interop I5, #1972).
        EnvironmentRule::QualifiedServices => {
            let mut services = array_of(value)?;
            for service in &mut services {
                let foreign = matches!(service, toml::Value::Table(t)
                    if t.get("binding").and_then(toml::Value::as_str).is_some_and(|b| resolved.foreign_bindings.contains(b)));
                if !foreign {
                    qualify_field(service, "service", |target| {
                        deployed_worker_name(environment, target)
                    });
                }
            }
            Some(toml::Value::Array(services))
        }
    }
}

/// `value`'s entries, when it is an array.
fn array_of(value: &toml::Value) -> Option<Vec<toml::Value>> {
    value.as_array().cloned()
}

/// Replace `entry`'s string field `field` with `qualify` of it, if the entry is a table with one.
fn qualify_field(entry: &mut toml::Value, field: &str, qualify: impl Fn(&str) -> String) {
    if let toml::Value::Table(t) = entry
        && let Some(name) = t
            .get(field)
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    {
        t.insert(field.to_string(), toml::Value::String(qualify(&name)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(
        config: &str,
        environment: &str,
        kv: &[(&str, &str)],
        foreign: &[&str],
    ) -> toml::Table {
        let kv_ids: BTreeMap<String, String> = kv
            .iter()
            .map(|(b, i)| (b.to_string(), i.to_string()))
            .collect();
        let foreign_bindings: BTreeSet<String> = foreign.iter().map(|b| b.to_string()).collect();
        let out = render(
            config,
            environment,
            &Resolved {
                kv_ids: &kv_ids,
                foreign_bindings: &foreign_bindings,
            },
        )
        .expect("renders");
        let parsed: toml::Table = out.parse().expect("valid TOML out");
        parsed["env"][environment]
            .as_table()
            .expect("a block")
            .clone()
    }

    #[test]
    fn the_name_is_the_deployed_name() {
        let b = block("name = \"shop\"\n", "staging", &[], &[]);
        assert_eq!(b["name"].as_str(), Some("shop-staging"));
        let b = block("name = \"shop\"\n", "default", &[], &[]);
        assert_eq!(b["name"].as_str(), Some("shop"));
    }

    #[test]
    fn a_namespace_takes_its_resolved_id_and_none_resolved_leaves_the_table_out() {
        let config = "name = \"s\"\n[[kv_namespaces]]\nbinding = \"KV\"\nid = \"<KV_ID>\"\n";
        let b = block(config, "staging", &[("KV", "abc")], &[]);
        assert_eq!(b["kv_namespaces"][0]["id"].as_str(), Some("abc"));
        let b = block(config, "staging", &[], &[]);
        assert!(!b.contains_key("kv_namespaces"), "{b:?}");
    }

    #[test]
    fn a_bucket_an_index_and_a_dataset_are_qualified_each_in_its_form() {
        let config = "name = \"s\"\n[[r2_buckets]]\nbinding = \"B\"\nbucket_name = \"media\"\n\
                      [[vectorize]]\nbinding = \"V\"\nindex_name = \"docs\"\n\
                      [[analytics_engine_datasets]]\nbinding = \"A\"\ndataset = \"signups\"\n";
        let b = block(config, "eu-staging", &[], &[]);
        assert_eq!(
            b["r2_buckets"][0]["bucket_name"].as_str(),
            Some("media-eu-staging")
        );
        assert_eq!(
            b["vectorize"][0]["index_name"].as_str(),
            Some("docs-eu-staging")
        );
        assert_eq!(
            b["analytics_engine_datasets"][0]["dataset"].as_str(),
            Some("signups_eu_staging")
        );
    }

    #[test]
    fn only_the_dataset_variables_are_qualified() {
        let config = "name = \"s\"\n[vars]\nBYNK_DATASET_A = \"signups\"\nOTHER = \"keep\"\n";
        let b = block(config, "staging", &[], &[]);
        assert_eq!(
            b["vars"]["BYNK_DATASET_A"].as_str(),
            Some("signups_staging")
        );
        assert_eq!(b["vars"]["OTHER"].as_str(), Some("keep"));
    }

    #[test]
    fn producers_and_consumers_are_both_qualified() {
        let config = "name = \"s\"\n[[queues.producers]]\nbinding = \"Q\"\nqueue = \"jobs\"\n\
                      [[queues.consumers]]\nqueue = \"jobs\"\nmax_batch_size = 10\n";
        let b = block(config, "staging", &[], &[]);
        assert_eq!(
            b["queues"]["producers"][0]["queue"].as_str(),
            Some("jobs-staging")
        );
        assert_eq!(
            b["queues"]["consumers"][0]["queue"].as_str(),
            Some("jobs-staging")
        );
    }

    #[test]
    fn a_service_to_a_project_worker_is_qualified_and_a_foreign_one_is_not() {
        let config = "name = \"s\"\n[[services]]\nbinding = \"OTHER\"\nservice = \"other\"\n\
                      [[services]]\nbinding = \"LEGACY\"\nservice = \"legacy\"\n";
        let b = block(config, "staging", &[], &["LEGACY"]);
        assert_eq!(b["services"][0]["service"].as_str(), Some("other-staging"));
        assert_eq!(b["services"][1]["service"].as_str(), Some("legacy"));
    }

    #[test]
    fn the_inherited_keys_and_an_unknown_one_are_not_restated_and_a_singleton_is_copied() {
        let config = "name = \"s\"\nmain = \"index.ts\"\ncompatibility_date = \"2026-01-01\"\n\
                      compatibility_flags = []\nworkers_dev = true\naddresses = [\"a@b\"]\n\
                      unknown = 1\n[assets]\ndirectory = \"a\"\n[cache]\nenabled = true\n\
                      [[rules]]\ntype = \"CompiledWasm\"\n[version_metadata]\nbinding = \"V\"\n\
                      [ai]\nbinding = \"AI\"\n";
        let b = block(config, "staging", &[], &[]);
        let keys: Vec<&str> = b.keys().map(String::as_str).collect();
        assert_eq!(keys, ["ai", "name", "version_metadata"]);
    }

    /// Every top-level key the rules copy is copied byte for byte from the top level, which the
    /// deploy has already made this environment's.
    #[test]
    fn a_copied_table_is_the_top_levels() {
        let config = "name = \"s\"\n[[d1_databases]]\nbinding = \"D\"\ndatabase_name = \"x\"\n\
                      database_id = \"1\"\n[triggers]\ncrons = [\"0 * * * *\"]\n";
        let b = block(config, "staging", &[], &[]);
        assert_eq!(b["d1_databases"][0]["database_id"].as_str(), Some("1"));
        assert_eq!(b["triggers"]["crons"][0].as_str(), Some("0 * * * *"));
    }

    #[test]
    fn nothing_to_state_is_left_exactly_as_emitted_and_bad_text_is_an_error() {
        let nameless = "main = \"index.ts\"\n";
        let out = render(
            nameless,
            "staging",
            &Resolved {
                kv_ids: &BTreeMap::new(),
                foreign_bindings: &BTreeSet::new(),
            },
        );
        assert_eq!(out.as_deref(), Ok(nameless));
        let bad = render(
            "not = valid = toml",
            "staging",
            &Resolved {
                kv_ids: &BTreeMap::new(),
                foreign_bindings: &BTreeSet::new(),
            },
        );
        assert!(bad.is_err());
    }
}
