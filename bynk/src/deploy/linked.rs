//! #3301: resources a deploy *links* to and never creates.
//!
//! A `StoredSecret` instance reads one secret of a Cloudflare Secrets Store.
//! The store and the secret belong to the account (shared across Workers,
//! rotated centrally), so unlike a KV namespace or a D1 database `bynk deploy`
//! does not make them. It finds them: the store by the name `@store` gives,
//! then the secret by the name `@secret` gives, and fails, before anything is
//! pushed, when either is missing, since a Worker bound to a secret that is
//! not there is broken from its first request. What it found, the store's id,
//! is recorded in the ledger and written into the Worker's configuration.
//!
//! This is the registry's `linked` flag ([`bynk_std::Instantiable::linked`])
//! made concrete for the one kind that sets it. A foreign `Service` Worker is
//! linked in spirit and has its own warn-only code (`foreign_worker_warnings`);
//! it moves onto this path in a follow-up, not here.
//!
//! An `AiSearch` instance (#3480) is linked the same way: by the name
//! `@instance` gives, looked up before anything is pushed, recorded in the
//! ledger, and a missing one stops the run. Nothing is created: the
//! instance's data source, models and indexing belong to whoever configured
//! it.
//!
//! Only names and ids cross the wire. A secret's *value* is never asked for,
//! so it cannot reach the plan, the ledger, the generated config or a log.

use super::*;
use crate::migrate::live::{self, Credentials, Rest};

/// The account's Secrets Stores, as far as linking needs them. A trait so the
/// resolution is tested against a fake and the REST client against a stub.
pub(crate) trait SecretsStores {
    /// The ids of the stores named `name` (one, when the account is
    /// well-formed; none when it has no such store).
    fn store_ids(&self, name: &str) -> Result<Vec<String>, String>;
    /// Whether the store `store_id` holds a secret named `secret`.
    fn has_secret(&self, store_id: &str, secret: &str) -> Result<bool, String>;
}

/// Cloudflare's REST API, for the Secrets Store listings. Wrangler's own
/// `secrets-store` commands print tables, not JSON, so they are not read.
pub(crate) struct RestStores {
    pub(crate) rest: Rest,
}

/// The permission a token needs to list stores and their secrets.
pub(crate) const TOKEN_SCOPE: &str = "Secrets Store: Read";

impl RestStores {
    /// A client for the account in `CLOUDFLARE_ACCOUNT_ID`, or a message
    /// naming the missing variable and the permission the token needs.
    pub(crate) fn from_env() -> Result<Self, String> {
        let credentials = Credentials::from_env_for(
            "link a Secrets Store secret",
            &format!(
                "an API token with \"{TOKEN_SCOPE}\" (a `wrangler login` session is not enough: the lookup calls Cloudflare's API directly)"
            ),
        )?;
        Ok(Self {
            rest: Rest {
                agent: live::agent()?,
                credentials,
                base: live::API.to_string(),
            },
        })
    }

    /// Every entry of the paginated listing at `path`, as `(id, name)`.
    fn listing(&self, path: &str) -> Result<Vec<(String, String)>, String> {
        let mut out = Vec::new();
        for page in 1.. {
            let body = self.rest.get(&format!("{path}?per_page=100&page={page}"))?;
            let entries = body["result"].as_array().cloned().unwrap_or_default();
            let n = entries.len();
            out.extend(entries.iter().filter_map(|e| {
                Some((
                    e["id"].as_str()?.to_string(),
                    e["name"].as_str()?.to_string(),
                ))
            }));
            let pages = body["result_info"]["total_pages"].as_u64().unwrap_or(1);
            if n == 0 || page >= pages {
                break;
            }
        }
        Ok(out)
    }
}

impl SecretsStores for RestStores {
    fn store_ids(&self, name: &str) -> Result<Vec<String>, String> {
        Ok(self
            .listing("/secrets_store/stores")?
            .into_iter()
            .filter(|(_, n)| n == name)
            .map(|(id, _)| id)
            .collect())
    }

    fn has_secret(&self, store_id: &str, secret: &str) -> Result<bool, String> {
        Ok(self
            .listing(&format!("/secrets_store/stores/{store_id}/secrets"))?
            .iter()
            .any(|(_, n)| n == secret))
    }
}

/// What the account answered about a store, for a message: Cloudflare says
/// 401 or 403 when the token lacks the permission, which is the one thing
/// worth naming rather than echoing.
fn explain(said: String) -> String {
    if said.contains("answered 401") || said.contains("answered 403") {
        format!(
            "{said} (the token needs the \"{TOKEN_SCOPE}\" permission, on this account, to find a store and its secrets)"
        )
    } else {
        said
    }
}

/// The store each `(worker, binding)` of `order` links to, as its id, after
/// asking `stores` once per store and once per secret. Every problem is
/// collected, not just the first: a missing secret is fixed by one command,
/// and three are better found together than one deploy at a time.
pub(crate) fn link_stored_secrets(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
    stores: &dyn SecretsStores,
) -> Result<BTreeMap<(String, String), String>, Vec<String>> {
    let mut store_ids: BTreeMap<String, Result<String, String>> = BTreeMap::new();
    let mut secrets: BTreeMap<(String, String), Result<bool, String>> = BTreeMap::new();
    let mut linked = BTreeMap::new();
    let mut problems = Vec::new();
    for worker in order {
        for decl in &resources[worker].stored_secrets {
            let who = format!("`{}` of `{worker}`", decl.binding);
            let store = store_ids
                .entry(decl.store.clone())
                .or_insert_with(|| match stores.store_ids(&decl.store) {
                    Ok(ids) => match ids.as_slice() {
                        [id] => Ok(id.clone()),
                        [] => Err(format!(
                            "Secrets Store `{}` does not exist on this account; create it with `wrangler secrets-store store create {} --remote`, or name the store the account has",
                            decl.store, decl.store
                        )),
                        _ => Err(format!(
                            "Secrets Store `{}` names more than one store on this account, so it cannot be linked by name",
                            decl.store
                        )),
                    },
                    Err(said) => Err(format!(
                        "could not look up Secrets Store `{}`: {}",
                        decl.store,
                        explain(said)
                    )),
                })
                .clone();
            let id = match store {
                Ok(id) => id,
                Err(why) => {
                    problems.push(format!("{who} reads secret `{}`: {why}", decl.secret));
                    continue;
                }
            };
            let present = secrets
                .entry((id.clone(), decl.secret.clone()))
                .or_insert_with(|| stores.has_secret(&id, &decl.secret).map_err(explain))
                .clone();
            match present {
                Ok(true) => {
                    linked.insert((worker.clone(), decl.binding.clone()), id);
                }
                Ok(false) => problems.push(format!(
                    "{who} reads secret `{secret}` of Secrets Store `{store}`, which has no such secret; create it with `wrangler secrets-store secret create {id} --name {secret} --scopes workers --remote`",
                    secret = decl.secret,
                    store = decl.store,
                )),
                Err(said) => problems.push(format!(
                    "{who} reads secret `{}` of Secrets Store `{}`; could not check it exists: {said}",
                    decl.secret, decl.store
                )),
            }
        }
    }
    if problems.is_empty() {
        Ok(linked)
    } else {
        Err(problems)
    }
}

/// What an AI Search instance lookup found, as far as the deploy cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstanceState {
    /// No instance of that name in the default namespace.
    Missing,
    /// It exists. `warning` says why it may not answer as expected, when the
    /// lookup shows it (paused or disabled), never a reason to stop.
    Found { warning: Option<String> },
}

/// The account's AI Search instances, as far as linking needs them. A trait so
/// the resolution is tested against a fake and the REST client against a stub.
pub(crate) trait AiSearchInstances {
    fn instance(&self, name: &str) -> Result<InstanceState, String>;
}

/// The permissions a token needs for the AI Search REST API. The docs name
/// these two for every request and list no read-only permission, so the
/// message names both.
pub(crate) const AI_SEARCH_TOKEN_SCOPE: &str = "AI Search: Edit and AI Search: Run";

/// Cloudflare's REST API, for the AI Search instance lookup.
pub(crate) struct RestAiSearch {
    pub(crate) rest: Rest,
}

impl RestAiSearch {
    /// A client for the account in `CLOUDFLARE_ACCOUNT_ID`, or a message
    /// naming the missing variable and the permissions the token needs.
    pub(crate) fn from_env() -> Result<Self, String> {
        let credentials = Credentials::from_env_for(
            "link an AI Search instance",
            &format!(
                "an API token with \"{AI_SEARCH_TOKEN_SCOPE}\" (a `wrangler login` session is not enough: the lookup calls Cloudflare's API directly)"
            ),
        )?;
        Ok(Self {
            rest: Rest {
                agent: live::agent()?,
                credentials,
                base: live::API.to_string(),
            },
        })
    }
}

/// The instance's lookup answer, read from the REST `result`: `paused` and
/// `enable` are the documented flags. Cloudflare does not document the values
/// of `status`; a real-account check (#3480) saw `waiting`, `index_source` and
/// `paused_user`. Only `paused_user` is worded from that observation: indexing
/// was not running, and searches and answers still worked but did not resume
/// it. Any other status of a paused instance is named but not interpreted.
fn instance_state(body: &serde_json::Value) -> InstanceState {
    let result = &body["result"];
    let warning = if result["paused"].as_bool() == Some(true) {
        Some(paused_warning(result["status"].as_str()))
    } else if result["enable"].as_bool() == Some(false) {
        Some("it is disabled".to_string())
    } else {
        None
    };
    InstanceState::Found { warning }
}

/// What a paused instance's warning says, by its reported `status`.
fn paused_warning(status: Option<&str>) -> String {
    match status {
        Some("paused_user") => "it is paused by a user (status `paused_user`), so its indexing is not running; \
             a search does not resume it, so resume it in the Cloudflare dashboard (searches still answered when checked)"
            .to_string(),
        Some(other) => format!(
            "it is paused (status `{other}`), so its indexing may not be running; \
             check its state in the Cloudflare dashboard"
        ),
        None => "it is paused, so its indexing may not be running; \
             check its state in the Cloudflare dashboard"
            .to_string(),
    }
}

impl AiSearchInstances for RestAiSearch {
    fn instance(&self, name: &str) -> Result<InstanceState, String> {
        match self
            .rest
            .get(&format!("/ai-search/namespaces/default/instances/{name}"))
        {
            Ok(body) => Ok(instance_state(&body)),
            // Cloudflare's documented answer for an instance that is not there
            // is code 7002 (`ai_search_not_found`). Any other 404 (a wrong
            // account id, a moved route) is not "missing", so it falls through
            // with what Cloudflare said.
            Err(said) if said.contains("\"code\":7002") => Ok(InstanceState::Missing),
            Err(said) if super::destinations::is_denied(&said) => Err(format!(
                "{said} (the token needs the \"{AI_SEARCH_TOKEN_SCOPE}\" permissions, on this account, to find an instance)"
            )),
            Err(said) => Err(said),
        }
    }
}

/// The warnings and problems of linking every `(worker, binding)` of `order`
/// to its AI Search instance, asking `search` once per instance name. Every
/// problem is collected, not just the first. A missing instance is a problem;
/// a paused or disabled one is a warning, since it still answers.
pub(crate) fn link_ai_search(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
    search: &dyn AiSearchInstances,
) -> (Vec<String>, Vec<String>) {
    let mut asked: BTreeMap<String, Result<InstanceState, String>> = BTreeMap::new();
    let (mut warnings, mut problems) = (Vec::new(), Vec::new());
    for worker in order {
        for decl in &resources[worker].ai_search {
            let who = format!("`{}` of `{worker}`", decl.binding);
            let found = asked
                .entry(decl.instance.clone())
                .or_insert_with(|| search.instance(&decl.instance))
                .clone();
            match found {
                Ok(InstanceState::Found { warning: None }) => {}
                Ok(InstanceState::Found {
                    warning: Some(why),
                }) => warnings.push(format!(
                    "bynk: warning: {who} queries AI Search instance `{}`, but {why}",
                    decl.instance
                )),
                Ok(InstanceState::Missing) => problems.push(format!(
                    "{who} queries AI Search instance `{name}`, which does not exist on this account. `bynk deploy` links to an instance and never creates one: create `{name}` in the Cloudflare dashboard, with `wrangler` or with the REST API, or name the instance the account has",
                    name = decl.instance
                )),
                Err(said) => problems.push(format!(
                    "{who} queries AI Search instance `{}`; could not look it up: {said}",
                    decl.instance
                )),
            }
        }
    }
    (warnings, problems)
}

/// The AI Search lookups a run does before it confirms: each instance of
/// `order` found on the account, warnings printed, `false` (after printing
/// every problem) when any is missing, so the run stops with nothing pushed.
pub(crate) fn link_ai_search_for_run(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
) -> bool {
    if !order.iter().any(|w| !resources[w].ai_search.is_empty()) {
        return true;
    }
    let search = match RestAiSearch::from_env() {
        Ok(search) => search,
        Err(e) => {
            eprintln!("bynk: {e}");
            return false;
        }
    };
    let (warnings, problems) = link_ai_search(order, resources, &search);
    for warning in &warnings {
        eprintln!("{warning}");
    }
    for problem in &problems {
        eprintln!("bynk: {problem}");
    }
    problems.is_empty()
}

/// Record the instances `worker`'s `AiSearch` bindings were linked to, before
/// the push. Looked up in [`link_ai_search_for_run`], which stops the run when
/// one is missing, so reaching here means each was found.
pub(crate) fn record_ai_search_links(
    worker: &str,
    declared: &Resources,
    lock: &mut DeployLock,
    lock_path: &Path,
    environment: &str,
) -> Result<(), DeployFailure> {
    let mut changed = false;
    for decl in &declared.ai_search {
        changed |= lock.record_ai_search(
            environment,
            worker,
            &decl.binding,
            AiSearchLink {
                instance: decl.instance.clone(),
            },
        );
    }
    if changed {
        write_lock(lock_path, lock).map_err(|e| {
            DeployFailure::driver(format!(
                "linked `{worker}`'s AI Search instances but could not record them in {}: {e}",
                lock_path.display()
            ))
        })?;
    }
    Ok(())
}

/// The account lookups a run does before it confirms: every stored secret of
/// `order` resolved against the account, and (#3302) every `@allowedSenders`
/// domain checked, each problem printed, `None` when any was found (the run
/// stops there, with nothing pushed).
pub(crate) fn link_for_run(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
) -> Option<BTreeMap<(String, String), String>> {
    if !check_senders_for_run(order, resources) {
        return None;
    }
    if !any_stored_secrets(order, resources) {
        return Some(BTreeMap::new());
    }
    let stores = match RestStores::from_env() {
        Ok(stores) => stores,
        Err(e) => {
            eprintln!("bynk: {e}");
            return None;
        }
    };
    match link_stored_secrets(order, resources, &stores) {
        Ok(linked) => Some(linked),
        Err(problems) => {
            for problem in problems {
                eprintln!("bynk: {problem}");
            }
            None
        }
    }
}

/// Record what `worker`'s stored secrets were linked to and write the stores'
/// ids into its generated config, before the push. Linked in
/// [`link_stored_secrets`], so a binding with no id here is a bug, not a user
/// error.
pub(crate) fn record_links(
    worker: &str,
    declared: &Resources,
    linked: &BTreeMap<(String, String), String>,
    lock: &mut DeployLock,
    lock_path: &Path,
    config: &Path,
    environment: &str,
) -> Result<(), DeployFailure> {
    let mut store_ids = BTreeMap::new();
    let mut changed = false;
    for decl in &declared.stored_secrets {
        let Some(store_id) = linked.get(&(worker.to_string(), decl.binding.clone())) else {
            return Err(DeployFailure::driver(format!(
                "`{}` of `{worker}` was not linked to a Secrets Store before the push",
                decl.binding
            )));
        };
        changed |= lock.record_stored_secret(
            environment,
            worker,
            &decl.binding,
            StoredSecretLink {
                store: decl.store.clone(),
                store_id: store_id.clone(),
                secret: decl.secret.clone(),
            },
        );
        store_ids.insert(decl.binding.clone(), store_id.clone());
    }
    if changed {
        write_lock(lock_path, lock).map_err(|e| {
            DeployFailure::driver(format!(
                "linked `{worker}`'s Secrets Store secrets but could not record them in {}: {e}",
                lock_path.display()
            ))
        })?;
    }
    if !store_ids.is_empty() && !materialise_stored_secret_stores(config, &store_ids) {
        return Err(DeployFailure::driver(format!(
            "could not materialise the Secrets Store ids into `{worker}`'s generated configuration"
        )));
    }
    Ok(())
}

/// Whether any Worker of `order` has a linked secret to look up.
pub(crate) fn any_stored_secrets(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
) -> bool {
    order
        .iter()
        .any(|w| !resources[w].stored_secrets.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::live::tests::{rest, stub};
    use std::cell::RefCell;

    /// An account with the stores `stores` (name, id, secrets), counting what
    /// it was asked.
    struct Fake {
        stores: Vec<(&'static str, &'static str, Vec<&'static str>)>,
        asked: RefCell<Vec<String>>,
        refuse: Option<&'static str>,
    }

    impl Fake {
        fn new(stores: Vec<(&'static str, &'static str, Vec<&'static str>)>) -> Self {
            Self {
                stores,
                asked: RefCell::new(Vec::new()),
                refuse: None,
            }
        }
    }

    impl SecretsStores for Fake {
        fn store_ids(&self, name: &str) -> Result<Vec<String>, String> {
            self.asked.borrow_mut().push(format!("store {name}"));
            if let Some(said) = self.refuse {
                return Err(said.to_string());
            }
            Ok(self
                .stores
                .iter()
                .filter(|(n, ..)| *n == name)
                .map(|(_, id, _)| id.to_string())
                .collect())
        }
        fn has_secret(&self, store_id: &str, secret: &str) -> Result<bool, String> {
            self.asked
                .borrow_mut()
                .push(format!("secret {store_id}/{secret}"));
            Ok(self
                .stores
                .iter()
                .any(|(_, id, secrets)| *id == store_id && secrets.contains(&secret)))
        }
    }

    /// A Worker's declared `(binding, store, secret)` triples.
    type Declared<'a> = &'a [(&'a str, &'a str, &'a str)];

    fn project(secrets: &[(&str, Declared)]) -> (Vec<String>, BTreeMap<String, Resources>) {
        let order = secrets.iter().map(|(w, _)| w.to_string()).collect();
        let resources = secrets
            .iter()
            .map(|(w, s)| (w.to_string(), Resources::default().stored_secrets(s)))
            .collect();
        (order, resources)
    }

    #[test]
    fn a_found_store_and_secret_link_by_the_stores_id() {
        let (order, resources) =
            project(&[("shop", &[("SECRET_STRIPE_KEY", "payments", "stripe-live")])]);
        let account = Fake::new(vec![("payments", "store-1", vec!["stripe-live", "other"])]);
        let linked = link_stored_secrets(&order, &resources, &account).unwrap();
        assert_eq!(
            linked[&("shop".to_string(), "SECRET_STRIPE_KEY".to_string())],
            "store-1"
        );
    }

    #[test]
    fn each_store_and_secret_is_asked_once_however_many_bind_it() {
        let (order, resources) = project(&[
            ("a", &[("SECRET_KEY", "payments", "stripe-live")]),
            (
                "b",
                &[
                    ("SECRET_KEY", "payments", "stripe-live"),
                    ("SECRET_HOOK", "payments", "stripe-hook"),
                ],
            ),
        ]);
        let account = Fake::new(vec![(
            "payments",
            "store-1",
            vec!["stripe-live", "stripe-hook"],
        )]);
        let linked = link_stored_secrets(&order, &resources, &account).unwrap();
        assert_eq!(linked.len(), 3);
        assert_eq!(
            *account.asked.borrow(),
            [
                "store payments",
                "secret store-1/stripe-live",
                "secret store-1/stripe-hook"
            ]
        );
    }

    #[test]
    fn a_missing_store_fails_naming_the_store_and_the_command_that_makes_it() {
        let (order, resources) =
            project(&[("shop", &[("SECRET_STRIPE_KEY", "payments", "stripe-live")])]);
        let problems = link_stored_secrets(&order, &resources, &Fake::new(Vec::new())).unwrap_err();
        assert_eq!(problems.len(), 1);
        let p = &problems[0];
        assert!(p.contains("`SECRET_STRIPE_KEY` of `shop`"), "{p}");
        assert!(p.contains("secret `stripe-live`"), "{p}");
        assert!(p.contains("Secrets Store `payments` does not exist"), "{p}");
        assert!(
            p.contains("wrangler secrets-store store create payments --remote"),
            "{p}"
        );
    }

    #[test]
    fn a_missing_secret_fails_naming_both_and_the_command_that_makes_it() {
        let (order, resources) =
            project(&[("shop", &[("SECRET_STRIPE_KEY", "payments", "stripe-live")])]);
        let account = Fake::new(vec![("payments", "store-1", vec!["something-else"])]);
        let problems = link_stored_secrets(&order, &resources, &account).unwrap_err();
        let p = &problems[0];
        assert!(
            p.contains("secret `stripe-live` of Secrets Store `payments`"),
            "{p}"
        );
        assert!(
            p.contains("wrangler secrets-store secret create store-1 --name stripe-live --scopes workers --remote"),
            "{p}"
        );
    }

    #[test]
    fn every_problem_is_reported_together() {
        let (order, resources) = project(&[(
            "shop",
            &[
                ("SECRET_A", "payments", "missing-secret"),
                ("SECRET_B", "no-store", "x"),
                ("SECRET_C", "payments", "present"),
            ],
        )]);
        let account = Fake::new(vec![("payments", "store-1", vec!["present"])]);
        let problems = link_stored_secrets(&order, &resources, &account).unwrap_err();
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn a_refused_lookup_names_the_permission_the_token_needs() {
        let (order, resources) = project(&[("shop", &[("SECRET_K", "payments", "s")])]);
        let mut account = Fake::new(Vec::new());
        account.refuse =
            Some("GET /secrets_store/stores: Cloudflare answered 403 [{\"code\":10000}]");
        let problems = link_stored_secrets(&order, &resources, &account).unwrap_err();
        assert!(problems[0].contains(TOKEN_SCOPE), "{}", problems[0]);
    }

    #[test]
    fn the_rest_client_follows_every_page_of_stores_and_secrets() {
        let (base, seen) = stub(vec![
            (
                "/secrets_store/stores?per_page=100&page=1",
                r#"{"success":true,"result":[{"id":"s1","name":"other"}],"result_info":{"total_pages":2}}"#.into(),
            ),
            (
                "/secrets_store/stores?per_page=100&page=2",
                r#"{"success":true,"result":[{"id":"s2","name":"payments"}],"result_info":{"total_pages":2}}"#.into(),
            ),
            (
                "/secrets_store/stores/s2/secrets?per_page=100&page=1",
                r#"{"success":true,"result":[{"id":"k1","name":"stripe-live","status":"active"}],"result_info":{"total_pages":1}}"#.into(),
            ),
        ]);
        let api = RestStores { rest: rest(base) };
        assert_eq!(api.store_ids("payments").unwrap(), ["s2"]);
        assert!(api.store_ids("absent").unwrap().is_empty());
        assert!(api.has_secret("s2", "stripe-live").unwrap());
        assert!(!api.has_secret("s2", "other").unwrap());
        let seen = seen.lock().unwrap();
        assert!(
            seen[0].starts_with("/accounts/acct/secrets_store/stores?"),
            "{seen:?}"
        );
        assert!(seen.iter().all(|s| s.ends_with("Bearer tok")), "{seen:?}");
    }

    #[test]
    fn the_rest_client_reads_an_instance_and_a_not_found_answer() {
        let (base, seen) = stub(vec![
            (
                "/ai-search/namespaces/default/instances/support-docs",
                r#"{"success":true,"result":{"id":"support-docs","paused":true}}"#.into(),
            ),
            (
                "/ai-search/namespaces/default/instances/denied",
                r#"{"success":false,"errors":[{"code":10000,"message":"Authentication error"}],"result":{}}"#.into(),
            ),
            (
                "/ai-search/namespaces/default/instances/absent",
                r#"{"success":false,"errors":[{"code":7002,"message":"ai_search_not_found"}],"result":{}}"#.into(),
            ),
        ]);
        let api = RestAiSearch { rest: rest(base) };
        assert!(matches!(
            api.instance("support-docs").unwrap(),
            InstanceState::Found { warning: Some(_) }
        ));
        // A refusal of the token names the permissions; a 404 that is not 7002
        // is not "missing", and nothing else is quietly swallowed.
        let denied = api.instance("denied").unwrap_err();
        assert!(
            denied.contains("AI Search: Edit and AI Search: Run"),
            "{denied}"
        );
        let other = api.instance("elsewhere").unwrap_err();
        assert!(
            other.contains("could not") || other.contains("7003"),
            "{other}"
        );
        assert_eq!(api.instance("absent").unwrap(), InstanceState::Missing);
        let seen = seen.lock().unwrap();
        assert!(
            seen[0].starts_with("/accounts/acct/ai-search/namespaces/default/instances/"),
            "{seen:?}"
        );
        assert!(seen.iter().all(|s| s.ends_with("Bearer tok")), "{seen:?}");
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bynk-linked-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const CONFIG: &str = "name = \"shop\"\n\n[[secrets_store_secrets]]\nbinding = \"SECRET_STRIPE_KEY\"\nstore_id = \"payments\" # the store's name; its id is set at deploy time\nsecret_name = \"stripe-live\"\n";

    #[test]
    fn a_link_is_recorded_and_its_store_id_written_into_the_config() {
        let dir = scratch("record");
        let config = dir.join("wrangler.toml");
        std::fs::write(&config, CONFIG).unwrap();
        let declared = Resources::default().stored_secrets(&[(
            "SECRET_STRIPE_KEY",
            "payments",
            "stripe-live",
        )]);
        let linked = BTreeMap::from([(
            ("shop".to_string(), "SECRET_STRIPE_KEY".to_string()),
            "store-1".to_string(),
        )]);
        let mut lock = DeployLock::default();
        let lock_path = dir.join(LOCK_FILE);
        record_links(
            "shop", &declared, &linked, &mut lock, &lock_path, &config, "staging",
        )
        .unwrap();
        let link = recorded_stored_secret(&lock, "shop", "SECRET_STRIPE_KEY", "staging").unwrap();
        assert_eq!(
            (
                link.store.as_str(),
                link.store_id.as_str(),
                link.secret.as_str()
            ),
            ("payments", "store-1", "stripe-live")
        );
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains("store_id = \"store-1\""), "{text}");
        assert!(text.contains("secret_name = \"stripe-live\""), "{text}");
        // The ledger holds names and an id, and is a committed file.
        let ledger = std::fs::read_to_string(&lock_path).unwrap();
        assert!(
            ledger.contains("store-1") && ledger.contains("stripe-live"),
            "{ledger}"
        );
        // A second run with the same link changes nothing.
        let before = std::fs::read_to_string(&lock_path).unwrap();
        record_links(
            "shop", &declared, &linked, &mut lock, &lock_path, &config, "staging",
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&lock_path).unwrap(), before);
        // A store recreated under another id is relinked.
        let relinked = BTreeMap::from([(
            ("shop".to_string(), "SECRET_STRIPE_KEY".to_string()),
            "store-2".to_string(),
        )]);
        record_links(
            "shop", &declared, &relinked, &mut lock, &lock_path, &config, "staging",
        )
        .unwrap();
        assert_eq!(
            recorded_stored_secret(&lock, "shop", "SECRET_STRIPE_KEY", "staging")
                .unwrap()
                .store_id,
            "store-2"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_binding_that_was_never_linked_is_refused_not_pushed() {
        let dir = scratch("unlinked");
        let config = dir.join("wrangler.toml");
        std::fs::write(&config, CONFIG).unwrap();
        let declared = Resources::default().stored_secrets(&[(
            "SECRET_STRIPE_KEY",
            "payments",
            "stripe-live",
        )]);
        let err = record_links(
            "shop",
            &declared,
            &BTreeMap::new(),
            &mut DeployLock::default(),
            &dir.join(LOCK_FILE),
            &config,
            "default",
        )
        .unwrap_err();
        assert!(err.message.contains("SECRET_STRIPE_KEY"), "{}", err.message);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_without_stored_secrets_asks_the_account_nothing() {
        let (order, resources) = project(&[("shop", &[])]);
        assert!(!any_stored_secrets(&order, &resources));
        assert_eq!(link_for_run(&order, &resources), Some(BTreeMap::new()));
    }

    // ---- #3480: AI Search instances are linked, never created ----

    struct FakeSearch(Vec<(&'static str, InstanceState)>, RefCell<Vec<String>>);

    impl AiSearchInstances for FakeSearch {
        fn instance(&self, name: &str) -> Result<InstanceState, String> {
            self.1.borrow_mut().push(name.to_string());
            Ok(self
                .0
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, s)| s.clone())
                .unwrap_or(InstanceState::Missing))
        }
    }

    fn search_project(
        workers: &[(&str, &[(&str, &str)])],
    ) -> (Vec<String>, BTreeMap<String, Resources>) {
        let order = workers.iter().map(|(w, _)| w.to_string()).collect();
        let resources = workers
            .iter()
            .map(|(w, instances)| (w.to_string(), Resources::default().ai_search(instances)))
            .collect();
        (order, resources)
    }

    #[test]
    fn a_found_instance_links_and_is_asked_once_per_name() {
        let (order, resources) = search_project(&[
            ("shop-ask", &[("AI_SEARCH_KB", "support-docs")]),
            ("shop-faq", &[("AI_SEARCH_DOCS", "support-docs")]),
        ]);
        let fake = FakeSearch(
            vec![("support-docs", InstanceState::Found { warning: None })],
            RefCell::new(Vec::new()),
        );
        let (warnings, problems) = link_ai_search(&order, &resources, &fake);
        assert!(warnings.is_empty() && problems.is_empty());
        assert_eq!(*fake.1.borrow(), ["support-docs"]);
    }

    #[test]
    fn every_missing_instance_is_named_at_once_and_none_is_created() {
        let (order, resources) = search_project(&[(
            "shop-ask",
            &[
                ("AI_SEARCH_KB", "support-docs"),
                ("AI_SEARCH_OLD", "old-docs"),
            ],
        )]);
        let fake = FakeSearch(vec![], RefCell::new(Vec::new()));
        let (_, problems) = link_ai_search(&order, &resources, &fake);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(
            problems[0].contains("`AI_SEARCH_KB` of `shop-ask`")
                && problems[0].contains("`support-docs`")
        );
        assert!(problems[0].contains("never creates one"));
        assert!(problems[1].contains("`old-docs`"));
    }

    #[test]
    fn a_paused_instance_is_a_warning_not_a_failure() {
        let (order, resources) =
            search_project(&[("shop-ask", &[("AI_SEARCH_KB", "support-docs")])]);
        let fake = FakeSearch(
            vec![(
                "support-docs",
                InstanceState::Found {
                    warning: Some(paused_warning(Some("paused_user"))),
                },
            )],
            RefCell::new(Vec::new()),
        );
        let (warnings, problems) = link_ai_search(&order, &resources, &fake);
        assert!(problems.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("support-docs") && warnings[0].contains("paused"));
    }

    #[test]
    fn the_lookup_answer_is_read_from_the_documented_flags() {
        let state =
            |result: serde_json::Value| instance_state(&serde_json::json!({ "result": result }));
        assert_eq!(
            state(serde_json::json!({ "id": "x", "status": "anything" })),
            InstanceState::Found { warning: None }
        );
        let InstanceState::Found { warning: Some(w) } =
            state(serde_json::json!({ "paused": true }))
        else {
            panic!("paused warns");
        };
        assert!(w.contains("paused") && !w.contains("resumes"));
        // `paused_user` is worded from what a real account showed (#3480).
        let InstanceState::Found { warning: Some(w) } =
            state(serde_json::json!({ "paused": true, "status": "paused_user" }))
        else {
            panic!("paused_user warns");
        };
        assert!(w.contains("`paused_user`") && w.contains("does not resume"));
        // Any other status is named, and nothing is claimed about it.
        let InstanceState::Found { warning: Some(w) } =
            state(serde_json::json!({ "paused": true, "status": "index_source" }))
        else {
            panic!("paused warns");
        };
        assert!(w.contains("`index_source`") && !w.contains("resume"));
        // Paused is said before disabled when both hold.
        let InstanceState::Found { warning: Some(w) } =
            state(serde_json::json!({ "paused": true, "enable": false }))
        else {
            panic!("paused and disabled warns");
        };
        assert!(w.contains("paused") && !w.contains("disabled"));
        let InstanceState::Found { warning: Some(w) } =
            state(serde_json::json!({ "enable": false }))
        else {
            panic!("disabled warns");
        };
        assert!(w.contains("disabled"));
    }

    #[test]
    fn a_failed_lookup_is_a_problem_naming_the_instance_not_a_missing_one() {
        struct Refusing;
        impl AiSearchInstances for Refusing {
            fn instance(&self, _: &str) -> Result<InstanceState, String> {
                Err("GET /ai-search/namespaces/default/instances/support-docs: Cloudflare answered 500 []".into())
            }
        }
        let (order, resources) =
            search_project(&[("shop-ask", &[("AI_SEARCH_KB", "support-docs")])]);
        let (warnings, problems) = link_ai_search(&order, &resources, &Refusing);
        assert!(warnings.is_empty());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains("could not look it up") && problems[0].contains("answered 500")
        );
        assert!(!problems[0].contains("does not exist"));
    }

    #[test]
    fn a_run_with_no_ai_search_instance_asks_nothing() {
        let (order, resources) = search_project(&[("shop-ask", &[])]);
        assert!(link_ai_search_for_run(&order, &resources));
    }

    #[test]
    fn linking_records_the_instance_and_a_repeat_is_not_a_change() {
        let declared = Resources::default().ai_search(&[("AI_SEARCH_KB", "support-docs")]);
        let dir = std::env::temp_dir().join(format!("bynk-ai-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bynk.deploy.lock");
        let mut lock = DeployLock::default();
        record_ai_search_links("shop-ask", &declared, &mut lock, &path, "production").unwrap();
        assert_eq!(
            recorded_ai_search(&lock, "shop-ask", "AI_SEARCH_KB", "production"),
            Some(&AiSearchLink {
                instance: "support-docs".into()
            })
        );
        assert!(!lock.record_ai_search(
            "production",
            "shop-ask",
            "AI_SEARCH_KB",
            AiSearchLink {
                instance: "support-docs".into()
            }
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
