//! KV namespaces over REST (1D.4a, #4056 Decision C): each operation as a [`Request`] and a
//! decoder for its answer, so the calls are data a later first-party binding could describe the
//! same way (§12). [`super::super::Account`] runs them.
//!
//! The endpoints are Cloudflare's (`/accounts/{account_id}/storage/kv/namespaces`), the ones
//! `wrangler kv namespace create/list/delete` called underneath, so the ids are the ones wrangler
//! printed.

use std::collections::BTreeSet;

use super::{Method, Request};
use crate::cloudflare::Idempotent;

/// The account's namespaces.
pub(crate) const NAMESPACES: &str = "/storage/kv/namespaces";

/// Cloudflare's code for a namespace that is not there (`namespace not found [code: 10013]`,
/// confirmed against a real account in #839): a delete answered with it is already done.
pub(crate) const NOT_FOUND: u64 = 10013;

/// How many namespaces a listing page asks for: the endpoint's largest page.
const PER_PAGE: u32 = 100;

/// `POST /storage/kv/namespaces {"title": …}`. Not idempotent: a second create with the same
/// title fails (code 10014), so a retry could not be told from a clash.
pub(crate) fn create(title: &str) -> Request {
    Request {
        method: Method::Post,
        path: NAMESPACES.to_string(),
        body: Some(serde_json::json!({ "title": title })),
        header: None,
        idempotent: Idempotent::No,
    }
}

/// The new namespace's id: `result.id`.
pub(crate) fn created(body: &serde_json::Value) -> Option<String> {
    body["result"]["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// `GET /storage/kv/namespaces?per_page=100&page=N`: one page of the listing.
pub(crate) fn list_page(page: u64) -> Request {
    Request {
        method: Method::Get,
        path: format!("{NAMESPACES}?per_page={PER_PAGE}&page={page}"),
        body: None,
        header: None,
        idempotent: Idempotent::Yes,
    }
}

/// The ids on one listing page, and how many entries it held; `None` when the answer has no
/// `result` list.
pub(crate) fn listed(body: &serde_json::Value) -> Option<(BTreeSet<String>, usize)> {
    let entries = body["result"].as_array()?;
    let ids = entries
        .iter()
        .filter_map(|entry| entry["id"].as_str().map(str::to_string))
        .collect();
    Some((ids, entries.len()))
}

/// Whether `id` can be a namespace id: Cloudflare's are 32 hex characters, so anything that is
/// not plain ASCII alphanumeric (a `/`, `..`, `?` or `#` from an edited `bynk.deploy.lock`) is
/// refused before it can reach a request path.
pub(crate) fn is_namespace_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `DELETE /storage/kv/namespaces/{id}`, for an id that [`is_namespace_id`].
pub(crate) fn delete(id: &str) -> Request {
    Request {
        method: Method::Delete,
        path: format!("{NAMESPACES}/{id}"),
        body: None,
        header: None,
        idempotent: Idempotent::Yes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_operation_is_the_documented_request() {
        let create = create("shop-api");
        assert_eq!(
            (create.method, create.path.as_str(), create.idempotent),
            (Method::Post, "/storage/kv/namespaces", Idempotent::No)
        );
        assert_eq!(create.body.unwrap().to_string(), r#"{"title":"shop-api"}"#);
        assert_eq!(
            list_page(2).path,
            "/storage/kv/namespaces?per_page=100&page=2"
        );
        let delete = delete("0123abcd");
        assert_eq!(
            (delete.method, delete.path.as_str(), delete.body),
            (Method::Delete, "/storage/kv/namespaces/0123abcd", None)
        );
    }

    #[test]
    fn only_a_plain_alphanumeric_id_is_a_namespace_id() {
        assert!(is_namespace_id("0f2a9c1d2e3f40516273849506a7b8c9"));
        assert!(is_namespace_id("kvabc"));
        for bad in [
            "",
            "../workers/scripts/x",
            "a/b",
            "a?b",
            "a#b",
            "a b",
            "a%2Fb",
            "é",
        ] {
            assert!(!is_namespace_id(bad), "{bad:?}");
        }
        assert!(!is_namespace_id(&"a".repeat(65)));
    }

    #[test]
    fn the_answers_are_read_from_their_documented_fields() {
        let body = serde_json::json!({"success": true, "result": {"id": "0f2a", "title": "t"}});
        assert_eq!(created(&body).as_deref(), Some("0f2a"));
        assert_eq!(created(&serde_json::json!({"result": {"id": ""}})), None);
        assert_eq!(created(&serde_json::json!({"result": null})), None);
        let page = serde_json::json!({"result": [{"id": "b2", "title": "x"}, {"id": "a1"}, {}]});
        let (ids, entries) = listed(&page).unwrap();
        assert_eq!(ids, BTreeSet::from(["a1".to_string(), "b2".to_string()]));
        assert_eq!(entries, 3);
        assert_eq!(listed(&serde_json::json!({"result": {"id": "x"}})), None);
    }
}
