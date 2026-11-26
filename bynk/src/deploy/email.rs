//! #3883: which inbound-email addresses a deployment environment claims.
//!
//! Wrangler's `addresses` is top-level only and applies to every environment of a Worker, and
//! naming an address a Worker's Email Routing rule already holds takes that rule over. So
//! `wrangler deploy --env staging` against the source's top-level list would take production's
//! rules over (or, run non-interactively, fail after the Worker is live). A non-default
//! environment therefore claims only what `[deploy.email]` maps it to, and when it maps nothing
//! the key is left out altogether: Wrangler documents an omitted `addresses` as changing no
//! Email Routing rule (an empty list would delete every rule the Worker manages).
//!
//! That behaviour is derived from Wrangler's documentation, not observed against a live
//! account; the live check is a separate follow-up (#4427).

use std::collections::{BTreeMap, BTreeSet};

use bynk_project::email_deploy::{DEFAULT_ENVIRONMENT, EmailDeployConfig, claimed_addresses};
use bynk_project::worker_layout::WorkerLayout;
use bynk_project::wrangler_keys::WranglerKey;
use toml_edit::{Array, DocumentMut, Item, Value};

use super::config::Resources;

/// Narrow every Worker's `email_addresses` to what `environment` claims, after checking the
/// `[deploy.email]` mapping against the addresses the source's services name. The source's own
/// list is kept in `email_source`, which a plan reads to say a Worker claims nothing.
pub(crate) fn claim(
    resources: &mut BTreeMap<String, Resources>,
    layout: &WorkerLayout,
    environment: &str,
) -> Result<(), String> {
    let config = layout.email_deploy();
    let source: BTreeSet<String> = resources
        .values()
        .flat_map(|r| r.email_addresses.iter().map(|a| a.to_ascii_lowercase()))
        .collect();
    check_mapping(config, &source)?;
    for r in resources.values_mut() {
        r.email_source = r.email_addresses.clone();
        r.email_addresses = claimed_addresses(config, &r.email_source, environment);
    }
    Ok(())
}

/// The mapping's own rules: a key names an address a `from email` service names, a mapped
/// address is well formed, and no address is claimed by two environments (or by the source).
fn check_mapping(config: &EmailDeployConfig, source: &BTreeSet<String>) -> Result<(), String> {
    // Who claims each address: the source for `default`, then each mapped environment.
    let mut claimed_by: BTreeMap<&str, String> = source
        .iter()
        .map(|a| (a.as_str(), DEFAULT_ENVIRONMENT.to_string()))
        .collect();
    for (key, entry) in config {
        if !source.contains(key) {
            let hint = bynk_project::paths::did_you_mean(key, source.iter().map(String::as_str))
                .map(|a| format!("; did you mean `{a}`?"))
                .unwrap_or_default();
            return Err(format!(
                "`[deploy.email.\"{key}\"]` in bynk.toml names no `from email` address in this project{hint}"
            ));
        }
        for (environment, mapped) in &entry.address {
            if let Some(problem) = bynk_check::email::address_problem(mapped) {
                return Err(format!(
                    "`[deploy.email.\"{key}\"]` maps `{environment}` to `{mapped}`, which is not an address Email Routing can match: {problem}"
                ));
            }
            if let Some(other) = claimed_by.insert(mapped, environment.clone()) {
                return Err(format!(
                    "`[deploy.email.\"{key}\"]` maps `{environment}` to `{mapped}`, which `{other}` already claims; two environments cannot hold one Email Routing rule"
                ));
            }
        }
    }
    Ok(())
}

/// `config_text` with its top-level `addresses` set to `claimed`, or removed when `claimed` is
/// empty. A text with no `addresses` is returned as it is. The rest of the file is untouched.
pub(crate) fn set_top_level_addresses(
    config_text: &str,
    claimed: &[String],
) -> Result<String, String> {
    let mut doc: DocumentMut = config_text
        .parse()
        .map_err(|e| format!("generated configuration is not valid TOML: {e}"))?;
    let key = WranglerKey::Addresses.header();
    if !doc.contains_key(key) {
        return Ok(config_text.to_string());
    }
    if claimed.is_empty() {
        doc.remove(key);
    } else {
        let list: Array = claimed.iter().map(String::as_str).collect();
        doc[key] = Item::Value(Value::Array(list));
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_project::email_deploy::deploy_email_from;

    fn config(text: &str) -> EmailDeployConfig {
        let doc: toml::Table = text.parse().unwrap();
        deploy_email_from(doc["deploy"].as_table().unwrap()).unwrap()
    }

    fn source(addresses: &[&str]) -> BTreeSet<String> {
        addresses.iter().map(|a| a.to_string()).collect()
    }

    const MAPPED: &str = "[deploy.email.\"support@shop.example\"]\naddress = { staging = \"support@staging.shop.example\" }\n";

    #[test]
    fn a_mapping_that_names_a_source_address_is_accepted() {
        assert!(check_mapping(&config(MAPPED), &source(&["support@shop.example"])).is_ok());
        assert!(check_mapping(&EmailDeployConfig::new(), &source(&[])).is_ok());
    }

    #[test]
    fn a_key_that_names_no_source_address_is_refused() {
        let error = check_mapping(&config(MAPPED), &source(&["sales@shop.example"])).unwrap_err();
        assert!(error.contains("names no `from email` address"), "{error}");
        let near = check_mapping(&config(MAPPED), &source(&["suport@shop.example"])).unwrap_err();
        assert!(
            near.contains("did you mean `suport@shop.example`?"),
            "{near}"
        );
    }

    #[test]
    fn a_mapped_address_must_be_well_formed() {
        let bad = config(
            "[deploy.email.\"support@shop.example\"]\naddress = { staging = \"no-at-sign\" }\n",
        );
        let error = check_mapping(&bad, &source(&["support@shop.example"])).unwrap_err();
        assert!(
            error.contains("not an address Email Routing can match"),
            "{error}"
        );
    }

    #[test]
    fn two_environments_cannot_claim_one_address() {
        let clash = config(
            "[deploy.email.\"support@shop.example\"]\naddress = { staging = \"s@x.example\", qa = \"s@x.example\" }\n",
        );
        let error = check_mapping(&clash, &source(&["support@shop.example"])).unwrap_err();
        assert!(error.contains("already claims"), "{error}");
        // Another entry's mapping clashes too.
        let across = config(
            "[deploy.email.\"a@shop.example\"]\naddress = { staging = \"s@x.example\" }\n[deploy.email.\"b@shop.example\"]\naddress = { qa = \"s@x.example\" }\n",
        );
        let error =
            check_mapping(&across, &source(&["a@shop.example", "b@shop.example"])).unwrap_err();
        assert!(error.contains("already claims"), "{error}");
    }

    #[test]
    fn a_mapping_to_the_sources_own_address_is_a_clash() {
        let own = config(
            "[deploy.email.\"a@shop.example\"]\naddress = { staging = \"b@shop.example\" }\n",
        );
        let error =
            check_mapping(&own, &source(&["a@shop.example", "b@shop.example"])).unwrap_err();
        assert!(error.contains("`default` already claims"), "{error}");
    }

    const WRANGLER: &str = "name = \"shop-support\"\nmain = \"index.ts\"\naddresses = [\"support@shop.example\"]\nworkers_dev = true\n";

    #[test]
    fn a_named_environment_with_no_mapping_drops_the_key() {
        let text = set_top_level_addresses(WRANGLER, &[]).unwrap();
        assert!(!text.contains("addresses"), "{text}");
        assert!(text.contains("workers_dev = true"), "{text}");
    }

    #[test]
    fn a_mapped_environment_writes_only_its_addresses() {
        let text = set_top_level_addresses(WRANGLER, &["support@staging.shop.example".to_string()])
            .unwrap();
        assert!(text.contains("support@staging.shop.example"), "{text}");
        assert!(!text.contains("support@shop.example"), "{text}");
    }

    #[test]
    fn a_config_with_no_addresses_is_unchanged() {
        let plain = "name = \"shop\"\nworkers_dev = true\n";
        assert_eq!(set_top_level_addresses(plain, &[]).unwrap(), plain);
        assert_eq!(
            set_top_level_addresses(plain, &["a@b.example".to_string()]).unwrap(),
            plain,
        );
    }
}
