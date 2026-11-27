//! [`Account`]: the one [`CloudflareApi`] a run uses (1D.4, #4056 Decision C). It routes each
//! operation to its transport: a moved kind's to Cloudflare's REST API, through its request
//! module (`rest::kv`), and every other one to the wrangler adapter ([`Wrangler`]).
//!
//! Q4's boundary as amended by #4056 Decision B: wrangler builds and runs code (the push, the
//! triggers, `rollback`, D1 migrations, `whoami`, `bynk dev`'s local state) and keeps the
//! Worker-scoped operations (its deployments listing and its secrets); REST manages the account's
//! resources. 1D.4a moves KV; R2 and D1, Vectorize and queues follow, one PR each, and the
//! `wrangler_resource_ops` probe counts what is left.

use std::collections::BTreeSet;
use std::path::Path;

use super::api::{CloudflareApi, PushCall, PushRun};
use super::error::{CloudflareError, WranglerError};
use super::paging::{MissingTotal, Numbered, Page};
use super::process::Wrangler;
use super::rest::vectorize::IndexConfig;
use super::rest::{self, Purpose, Request, Rest};

/// The account, over both transports.
pub(crate) struct Account<'a> {
    wrangler: Wrangler<'a>,
    /// The REST client, when the caller has one; otherwise each moved operation asks for the
    /// run's own client in its kind's words ([`rest::for_kind`]), so a missing credential is
    /// named by the kind that needed it.
    rest: Option<Rest>,
}

impl<'a> Account<'a> {
    /// The account over `wrangler`, with the run's REST client resolved when a moved operation
    /// first needs it.
    pub(crate) fn new(wrangler: Wrangler<'a>) -> Self {
        Account {
            wrangler,
            rest: None,
        }
    }

    /// The account over `wrangler` and `rest` (a test's stub).
    #[cfg(test)]
    pub(crate) fn with_rest(wrangler: Wrangler<'a>, rest: Rest) -> Self {
        Account {
            wrangler,
            rest: Some(rest),
        }
    }

    /// The REST client for `purpose`'s operations.
    fn rest(&self, purpose: Purpose) -> Result<Rest, CloudflareError> {
        match &self.rest {
            Some(rest) => Ok(rest.clone()),
            // A test never reaches a real account: without a stub, a moved operation fails
            // here instead of reading the machine's credentials.
            #[cfg(test)]
            None => {
                let _ = purpose;
                Err(CloudflareError::Other(
                    "no REST stub for this test's account".to_string(),
                ))
            }
            #[cfg(not(test))]
            None => rest::for_kind(purpose),
        }
    }

    /// Send `request` for `purpose`.
    fn send(
        &self,
        purpose: Purpose,
        request: &Request,
    ) -> Result<serde_json::Value, CloudflareError> {
        Ok(self.rest(purpose)?.request(request)?)
    }
}

/// `name`, refused unless it can be a bucket name, before it reaches a request path.
fn bucket_name(name: &str) -> Result<(), CloudflareError> {
    if rest::r2::is_bucket_name(name) {
        Ok(())
    } else {
        Err(CloudflareError::Other(format!(
            "`{name}` is not a valid R2 bucket name, so nothing was sent"
        )))
    }
}

/// `name`, refused unless it can be a Vectorize index name, before it reaches a request path.
fn index_name(name: &str) -> Result<(), CloudflareError> {
    if rest::vectorize::is_index_name(name) {
        Ok(())
    } else {
        Err(CloudflareError::Other(format!(
            "`{name}` is not a valid Vectorize index name, so nothing was sent"
        )))
    }
}

/// An answer that succeeded but did not hold what the operation reads.
fn unreadable(request: &Request, missing: &str) -> CloudflareError {
    let method = match request.method {
        rest::Method::Get => "GET",
        rest::Method::Post => "POST",
        rest::Method::Delete => "DELETE",
    };
    CloudflareError::Other(format!(
        "{method} {}: Cloudflare's answer has no {missing}",
        request.path
    ))
}

impl CloudflareApi for Account<'_> {
    fn queue_create(&self, name: &str) -> Result<(), CloudflareError> {
        self.send(Purpose::Queues, &rest::queues::create(name))
            .map(drop)
    }

    /// Every page until the exact name turns up, to `total_pages` (or while full), never past
    /// [`rest::queues::MAX_PAGES`] or a page that brings no queue not already seen.
    fn queue_find(&self, name: &str) -> Result<Option<String>, CloudflareError> {
        let mut found = None;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let rule = Numbered {
            cap: Some(rest::queues::MAX_PAGES),
            missing_total: MissingTotal::WhileFull,
            stop_on_empty: false,
        };
        rule.follow(|page| -> Result<Page, CloudflareError> {
            let request = rest::queues::list_page(page);
            let body = self.send(Purpose::Queues, &request)?;
            let (id, entries) = rest::queues::found(&body, name)
                .ok_or_else(|| unreadable(&request, "`result` list"))?;
            let before = seen.len();
            seen.extend(rest::queues::ids(&body));
            let fresh = seen.len() > before;
            let done = id.is_some() || !fresh;
            if found.is_none() {
                found = id;
            }
            Ok(Page::of(&body, entries).done_if(done))
        })?;
        Ok(found)
    }

    fn queue_delete(&self, name: &str) -> Result<bool, CloudflareError> {
        let Some(id) = self.queue_find(name)? else {
            return Ok(false);
        };
        if !rest::queues::is_queue_id(&id) {
            return Err(CloudflareError::Other(format!(
                "the queue `{name}` has the id `{id}`, which is not a queue id, so nothing was deleted"
            )));
        }
        self.send(Purpose::Queues, &rest::queues::delete(&id))
            .map(|_| true)
    }

    fn kv_create(&self, title: &str) -> Result<String, CloudflareError> {
        let request = rest::kv::create(title);
        let body = self.send(Purpose::Kv, &request)?;
        rest::kv::created(&body).ok_or_else(|| unreadable(&request, "namespace id"))
    }

    /// Every page, followed as the plan's own KV listing follows it (`on_account`): to
    /// `total_pages`, or while a page is full when Cloudflare leaves it out. A missed page would
    /// read a live namespace as gone and create it again, so the listing is read to its end.
    fn kv_list(&self) -> Result<BTreeSet<String>, CloudflareError> {
        let mut ids = BTreeSet::new();
        let rule = Numbered {
            cap: None,
            missing_total: MissingTotal::WhileFull,
            stop_on_empty: false,
        };
        rule.follow(|page| -> Result<Page, CloudflareError> {
            let request = rest::kv::list_page(page);
            let body = self.send(Purpose::Kv, &request)?;
            let (listed, entries) =
                rest::kv::listed(&body).ok_or_else(|| unreadable(&request, "`result` list"))?;
            ids.extend(listed);
            Ok(Page::of(&body, entries))
        })?;
        Ok(ids)
    }

    /// The id comes from `bynk.deploy.lock`, a committed file: one that could not be a namespace
    /// id is refused before it is put in a request path.
    fn kv_delete(&self, id: &str) -> Result<(), CloudflareError> {
        if !rest::kv::is_namespace_id(id) {
            return Err(CloudflareError::Other(format!(
                "`{id}` in bynk.deploy.lock is not a KV namespace id, so nothing was deleted"
            )));
        }
        self.send(Purpose::Kv, &rest::kv::delete(id)).map(drop)
    }

    fn d1_migrations_apply(
        &self,
        worker_dir: &Path,
        binding: &str,
        environment: &str,
    ) -> Result<(), WranglerError> {
        self.wrangler
            .d1_migrations_apply(worker_dir, binding, environment)
    }

    fn d1_create(&self, name: &str, jurisdiction: Option<&str>) -> Result<String, CloudflareError> {
        let request = rest::d1::create(name, jurisdiction);
        let body = self.send(Purpose::D1, &request)?;
        rest::d1::created(&body).ok_or_else(|| unreadable(&request, "database id"))
    }

    /// Every page of the name search until the exact name turns up, followed as the plan's own
    /// D1 listing is (while a page is full: the answer carries no `total_pages`). Only a database
    /// named exactly `name` is found, so a longer name the search also matches is never adopted.
    fn d1_find(&self, name: &str) -> Result<Option<String>, CloudflareError> {
        let mut found = None;
        let rule = Numbered {
            cap: None,
            missing_total: MissingTotal::WhileFull,
            stop_on_empty: false,
        };
        rule.follow(|page| -> Result<Page, CloudflareError> {
            let request = rest::d1::find_page(name, page);
            let body = self.send(Purpose::D1, &request)?;
            let (id, entries) = rest::d1::found(&body, name)
                .ok_or_else(|| unreadable(&request, "`result` list"))?;
            let done = id.is_some();
            if found.is_none() {
                found = id;
            }
            Ok(Page::of(&body, entries).done_if(done))
        })?;
        Ok(found)
    }

    fn r2_create(&self, name: &str, jurisdiction: Option<&str>) -> Result<(), CloudflareError> {
        bucket_name(name)?;
        self.send(Purpose::R2, &rest::r2::create(name, jurisdiction))
            .map(drop)
    }

    /// 404 is "not this account's"; any other failure is the error.
    fn r2_owned(&self, name: &str, jurisdiction: Option<&str>) -> Result<bool, CloudflareError> {
        bucket_name(name)?;
        match self.send(Purpose::R2, &rest::r2::bucket(name, jurisdiction)) {
            Ok(_) => Ok(true),
            Err(e) if e.status() == Some(404) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Followed while a page is full, which, with no paging documented, is one request; never
    /// past [`rest::vectorize::MAX_PAGES`], and not past a page that brings no name not already
    /// listed (an endpoint that ignored `?page=N` would repeat its first).
    fn vectorize_names(&self) -> Result<Vec<String>, CloudflareError> {
        let mut names: Vec<String> = Vec::new();
        let rule = Numbered {
            cap: Some(rest::vectorize::MAX_PAGES),
            missing_total: MissingTotal::WhileFull,
            stop_on_empty: false,
        };
        rule.follow(|page| -> Result<Page, CloudflareError> {
            let request = rest::vectorize::list_page(page);
            let body = self.send(Purpose::Vectorize, &request)?;
            let (listed, entries) = rest::vectorize::listed(&body)
                .ok_or_else(|| unreadable(&request, "list of index names"))?;
            let fresh: Vec<String> = listed.into_iter().filter(|n| !names.contains(n)).collect();
            let repeated = fresh.is_empty();
            names.extend(fresh);
            Ok(Page::of(&body, entries).done_if(repeated))
        })?;
        Ok(names)
    }

    fn vectorize_config(&self, name: &str) -> Result<IndexConfig, CloudflareError> {
        index_name(name)?;
        let request = rest::vectorize::get(name);
        let body = self.send(Purpose::Vectorize, &request)?;
        rest::vectorize::config(&body).ok_or_else(|| unreadable(&request, "dimensions and metric"))
    }

    fn vectorize_create(
        &self,
        name: &str,
        dimensions: i64,
        metric: &str,
    ) -> Result<(), CloudflareError> {
        index_name(name)?;
        self.send(
            Purpose::Vectorize,
            &rest::vectorize::create(name, dimensions, metric),
        )
        .map(drop)
    }

    fn vectorize_metadata_indexes(&self, name: &str) -> Result<Vec<String>, CloudflareError> {
        index_name(name)?;
        let request = rest::vectorize::metadata_list(name);
        let body = self.send(Purpose::Vectorize, &request)?;
        rest::vectorize::metadata(&body)
            .ok_or_else(|| unreadable(&request, "list of metadata indexes"))
    }

    fn vectorize_create_metadata_index(
        &self,
        name: &str,
        property: &str,
        ty: &str,
    ) -> Result<(), CloudflareError> {
        index_name(name)?;
        self.send(
            Purpose::Vectorize,
            &rest::vectorize::metadata_create(name, property, ty),
        )
        .map(drop)
    }

    fn vectorize_delete(&self, name: &str) -> Result<(), CloudflareError> {
        index_name(name)?;
        self.send(Purpose::Vectorize, &rest::vectorize::delete(name))
            .map(drop)
    }

    fn push(&self, call: PushCall<'_>) -> Result<PushRun, WranglerError> {
        self.wrangler.push(call)
    }

    fn triggers_deploy(&self, worker_dir: &Path, environment: &str) -> Result<(), WranglerError> {
        self.wrangler.triggers_deploy(worker_dir, environment)
    }

    fn whoami(&self) -> Result<(), WranglerError> {
        self.wrangler.whoami()
    }

    fn worker_deployments(&self, name: &str) -> Result<(), WranglerError> {
        self.wrangler.worker_deployments(name)
    }

    fn rollback(&self, script: &str, version: &str) -> Result<(), WranglerError> {
        self.wrangler.rollback(script, version)
    }

    fn secret_list(&self, worker_dir: &Path, environment: &str) -> Result<String, WranglerError> {
        self.wrangler.secret_list(worker_dir, environment)
    }

    fn secret_put(
        &self,
        worker_dir: &Path,
        name: &str,
        value: &str,
        environment: &str,
    ) -> Result<(), WranglerError> {
        self.wrangler
            .secret_put(worker_dir, name, value, environment)
    }

    fn named_secret(
        &self,
        dir: &Path,
        verb: &str,
        script: &str,
        name: &str,
        stdin: &str,
    ) -> Result<(), WranglerError> {
        self.wrangler.named_secret(dir, verb, script, name, stdin)
    }

    fn d1_migrations_apply_local(
        &self,
        worker_dir: &Path,
        binding: &str,
        persist: &[String],
    ) -> Result<(), WranglerError> {
        self.wrangler
            .d1_migrations_apply_local(worker_dir, binding, persist)
    }

    fn secrets_store_list_local(
        &self,
        worker_dir: &Path,
        store: &str,
        persist: &[String],
    ) -> Result<String, WranglerError> {
        self.wrangler
            .secrets_store_list_local(worker_dir, store, persist)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudflare::rest::tests::{answering_in_turn, rest};

    /// An account whose wrangler is a script that fails any call, over a REST stub at `base`:
    /// a KV operation that reached wrangler would say so.
    fn account(base: String, root: &Path) -> Account<'_> {
        Account::with_rest(
            Wrangler::scripted("echo \"wrangler ran: $*\" >&2; exit 1", root),
            rest(base),
        )
    }

    /// The stub pinning: a create is one POST, its id read from `result.id`.
    #[test]
    fn a_namespace_is_created_with_one_post_and_its_id_read() {
        let (base, seen) = answering_in_turn(&[(
            200,
            r#"{"success":true,"errors":[],"result":{"id":"0f2a9c","title":"shop-api"}}"#,
        )]);
        let root = std::env::temp_dir();
        assert_eq!(
            account(base, &root).kv_create("shop-api").unwrap(),
            "0f2a9c"
        );
        assert_eq!(
            seen.join().unwrap(),
            ["POST /accounts/acct/storage/kv/namespaces"]
        );
    }

    #[test]
    fn a_create_that_answers_no_id_is_a_failure_that_says_so() {
        let (base, _seen) = answering_in_turn(&[(200, r#"{"success":true,"result":{}}"#)]);
        let root = std::env::temp_dir();
        assert_eq!(
            account(base, &root).kv_create("t").unwrap_err().to_string(),
            "POST /storage/kv/namespaces: Cloudflare's answer has no namespace id"
        );
    }

    /// A title another namespace holds is Cloudflare's 10014, rendered as any REST failure: the
    /// kind's `could not create …:` prefix goes in front.
    #[test]
    fn a_taken_title_is_a_rest_failure() {
        let (base, _seen) = answering_in_turn(&[(
            400,
            r#"{"success":false,"errors":[{"code":10014,"message":"a namespace with this account ID and title already exists"}],"result":null}"#,
        )]);
        let root = std::env::temp_dir();
        let e = account(base, &root).kv_create("shop-api").unwrap_err();
        assert!(e.rest().is_some_and(|r| r.has_code(10014)), "{e}");
        assert_eq!(
            e.to_string(),
            r#"POST /storage/kv/namespaces: Cloudflare answered 400 [{"code":10014,"message":"a namespace with this account ID and title already exists"}]"#
        );
    }

    /// The listing is read to its end: here two pages, by `total_pages`.
    #[test]
    fn the_listing_reads_every_page() {
        let (base, seen) = answering_in_turn(&[
            (
                200,
                r#"{"success":true,"result":[{"id":"b2","title":"x"}],"result_info":{"page":1,"per_page":100,"count":1,"total_count":2,"total_pages":2}}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[{"id":"a1","title":"y"}],"result_info":{"page":2,"per_page":100,"count":1,"total_count":2,"total_pages":2}}"#,
            ),
        ]);
        let root = std::env::temp_dir();
        assert_eq!(
            account(base, &root).kv_list().unwrap(),
            BTreeSet::from(["a1".to_string(), "b2".to_string()])
        );
        assert_eq!(
            seen.join().unwrap(),
            [
                "GET /accounts/acct/storage/kv/namespaces?per_page=100&page=1",
                "GET /accounts/acct/storage/kv/namespaces?per_page=100&page=2",
            ]
        );
    }

    /// A delete is one DELETE by id. Cloudflare answers a namespace already gone with 404 and
    /// code 10013 (the code wrangler printed in #839's real-account check); the error carries it
    /// for the kind's "already gone" policy.
    #[test]
    fn a_delete_is_one_request_and_a_gone_namespace_carries_10013() {
        let (base, seen) = answering_in_turn(&[
            (200, r#"{"success":true,"errors":[],"result":null}"#),
            (
                404,
                r#"{"success":false,"errors":[{"code":10013,"message":"delete namespace: 'namespace not found'"}],"result":null}"#,
            ),
        ]);
        let root = std::env::temp_dir();
        let account = account(base, &root);
        account.kv_delete("0123abcd").unwrap();
        let gone = account.kv_delete("0123abcd").unwrap_err();
        assert!(
            gone.rest().is_some_and(|r| r.has_code(rest::kv::NOT_FOUND)),
            "{gone}"
        );
        assert_eq!(
            seen.join().unwrap(),
            [
                "DELETE /accounts/acct/storage/kv/namespaces/0123abcd",
                "DELETE /accounts/acct/storage/kv/namespaces/0123abcd",
            ]
        );
    }

    /// An id that could leave the namespaces path is refused before any request is sent.
    #[test]
    fn an_id_that_is_not_a_namespace_id_is_never_sent() {
        let root = std::env::temp_dir();
        // A port nothing listens on: a request that was sent would fail differently.
        let account = account("http://127.0.0.1:9".into(), &root);
        let e = account.kv_delete("../../workers/scripts/api").unwrap_err();
        assert_eq!(
            e.to_string(),
            "`../../workers/scripts/api` in bynk.deploy.lock is not a KV namespace id, so nothing was deleted"
        );
    }

    /// 1D.4b: the name search is followed while pages are full (the answer has no
    /// `total_pages`) until the exact name turns up; a longer name it also matches is not it.
    #[test]
    fn a_database_is_found_by_exact_name_across_full_pages() {
        let full: &'static str = Box::leak(
            format!(
                r#"{{"success":true,"result":[{}],"result_info":{{"page":1,"per_page":100,"count":100}}}}"#,
                (0..100)
                    .map(|i| format!(r#"{{"name":"shop-db-{i}","uuid":"u{i}"}}"#))
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .into_boxed_str(),
        );
        let answers: &'static [(u16, &'static str)] = Box::leak(Box::new([
            (200, full),
            (
                200,
                r#"{"success":true,"result":[{"name":"shop-db","uuid":"the-one"}],"result_info":{"page":2,"per_page":100,"count":1}}"#,
            ),
        ]));
        let (base, seen) = answering_in_turn(answers);
        let root = std::env::temp_dir();
        assert_eq!(
            account(base, &root).d1_find("shop-db").unwrap().as_deref(),
            Some("the-one")
        );
        assert_eq!(
            seen.join().unwrap(),
            [
                "GET /accounts/acct/d1/database?name=shop-db&per_page=100&page=1",
                "GET /accounts/acct/d1/database?name=shop-db&per_page=100&page=2",
            ]
        );
        let (base, _) = answering_in_turn(&[(
            200,
            r#"{"success":true,"result":[{"name":"shop-db-old","uuid":"u"}],"result_info":{"page":1,"per_page":100,"count":1}}"#,
        )]);
        assert_eq!(account(base, &root).d1_find("shop-db").unwrap(), None);
    }

    /// #4568 review: the index listing is followed while pages are full, and stops at a page
    /// that brings no new name, so an endpoint that ignored `?page=N` is asked twice, not forever.
    #[test]
    fn the_index_listing_follows_full_pages_and_stops_on_a_repeat() {
        let page = |from: usize, to: usize| -> &'static str {
            Box::leak(
                format!(
                    r#"{{"success":true,"result":[{}],"result_info":{{"per_page":2}}}}"#,
                    (from..to)
                        .map(|i| format!(r#"{{"name":"ix-{i}"}}"#))
                        .collect::<Vec<_>>()
                        .join(",")
                )
                .into_boxed_str(),
            )
        };
        let root = std::env::temp_dir();
        let answers: &'static [(u16, &'static str)] = Box::leak(Box::new([
            (200, page(0, 2)),
            (200, page(2, 4)),
            (200, page(4, 5)),
        ]));
        let (base, seen) = answering_in_turn(answers);
        assert_eq!(
            account(base, &root).vectorize_names().unwrap(),
            ["ix-0", "ix-1", "ix-2", "ix-3", "ix-4"]
        );
        assert_eq!(
            seen.join().unwrap(),
            [
                "GET /accounts/acct/vectorize/v2/indexes?page=1",
                "GET /accounts/acct/vectorize/v2/indexes?page=2",
                "GET /accounts/acct/vectorize/v2/indexes?page=3",
            ]
        );
        let answers: &'static [(u16, &'static str)] =
            Box::leak(Box::new([(200, page(0, 2)), (200, page(0, 2))]));
        let (base, seen) = answering_in_turn(answers);
        assert_eq!(
            account(base, &root).vectorize_names().unwrap(),
            ["ix-0", "ix-1"]
        );
        assert_eq!(seen.join().unwrap().len(), 2, "the repeated page ends it");
    }

    /// 1D.4d: the queues listing is followed while pages are full (when `total_pages` is left out)
    /// until the exact name turns up, and stops at a page that brings no new queue, so an answer
    /// that ignored `page=N` is asked twice, not forever.
    #[test]
    fn a_queue_is_found_across_full_pages_and_a_repeated_page_ends_the_lookup() {
        let full = |from: usize| -> &'static str {
            Box::leak(
                format!(
                    r#"{{"success":true,"result":[{}],"result_info":{{"per_page":2}}}}"#,
                    (from..from + 2)
                        .map(|i| format!(r#"{{"queue_id":"q{i}","queue_name":"other-{i}"}}"#))
                        .collect::<Vec<_>>()
                        .join(",")
                )
                .into_boxed_str(),
            )
        };
        let root = std::env::temp_dir();
        let answers: &'static [(u16, &'static str)] = Box::leak(Box::new([
            (200, full(0)),
            (200, full(2)),
            (
                200,
                r#"{"success":true,"result":[{"queue_id":"theone","queue_name":"jobs"}],"result_info":{"per_page":2}}"#,
            ),
        ]));
        let (base, seen) = answering_in_turn(answers);
        assert_eq!(
            account(base, &root).queue_find("jobs").unwrap().as_deref(),
            Some("theone")
        );
        assert_eq!(seen.join().unwrap().len(), 3);
        let answers: &'static [(u16, &'static str)] =
            Box::leak(Box::new([(200, full(0)), (200, full(0))]));
        let (base, seen) = answering_in_turn(answers);
        assert_eq!(account(base, &root).queue_find("jobs").unwrap(), None);
        assert_eq!(seen.join().unwrap().len(), 2, "the repeated page ends it");
    }

    /// 1D.4b: the account answers for its own bucket; a 404 is a bucket it does not hold, and
    /// any other failure is the error. A name that is not a bucket name is never sent.
    #[test]
    fn a_bucket_is_owned_only_when_the_account_answers_for_it() {
        let root = std::env::temp_dir();
        let (base, seen) = answering_in_turn(&[
            (200, r#"{"success":true,"result":{"name":"shop-files"}}"#),
            (
                404,
                r#"{"success":false,"errors":[{"code":10006,"message":"The specified bucket does not exist."}]}"#,
            ),
            (403, r#"{"success":false,"errors":[{"code":10000}]}"#),
        ]);
        let account = account(base, &root);
        assert!(account.r2_owned("shop-files", Some("eu")).unwrap());
        assert!(!account.r2_owned("shop-files", None).unwrap());
        assert!(account.r2_owned("shop-files", None).is_err());
        assert_eq!(seen.join().unwrap().len(), 3);
        assert_eq!(
            account.r2_create("../x", None).unwrap_err().to_string(),
            "`../x` is not a valid R2 bucket name, so nothing was sent"
        );
    }

    /// Without a stub a test's account never reads the machine's credentials.
    #[test]
    fn a_test_account_without_a_stub_reaches_no_account() {
        let root = std::env::temp_dir();
        let account = Account::new(Wrangler::scripted("exit 1", &root));
        assert_eq!(
            account.kv_list().unwrap_err().to_string(),
            "no REST stub for this test's account"
        );
    }
}
