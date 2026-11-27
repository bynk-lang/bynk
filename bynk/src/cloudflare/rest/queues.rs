//! Queues over REST (1D.4d, #4056 Decision C): each operation as a [`Request`] and a decoder for
//! its answer. [`super::super::Account`] runs them.
//!
//! Cloudflare's endpoints (`/accounts/{account_id}/queues`), as the API reference documents them:
//! the listing is paged (`page`, `per_page` up to 500, `result_info.total_pages`) and each entry
//! has a `queue_id` and a `queue_name`. A queue is created by name but deleted by id, and the
//! reference has no lookup by name, so a name is found by listing and matching it exactly. The
//! reference documents `10104 QueueNotFound` for a queue that does not exist, and no code for a
//! name already taken: a refused create is read from the account instead (`deploy::provisioning`).

use super::{Method, Request};
use crate::cloudflare::Idempotent;

/// The account's queues.
pub(crate) const QUEUES: &str = "/queues";

/// `QueueNotFound`: "Queue does not exist" (the Queues error-code reference).
pub(crate) const NOT_FOUND: u64 = 10104;

/// How many queues a listing page asks for.
const PER_PAGE: u32 = 100;

/// The most listing pages a lookup reads. The listing is documented as paged to
/// `result_info.total_pages`; this bound, and the stop on a page that brings no queue not already
/// seen, keep an answer without `total_pages` that ignored `page=N` from being asked for forever
/// (the 1D.4c review's finding for Vectorize). A hundred pages of 100 is far beyond any account
/// the deploy has met.
pub(crate) const MAX_PAGES: u64 = 100;

/// The `queue_id` of each entry on one listing page.
pub(crate) fn ids(body: &serde_json::Value) -> Vec<String> {
    body["result"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|q| q["queue_id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `id` can be a queue id: plain ASCII alphanumeric, at most 32 (the reference's
/// "maxLength 32"). Anything else is refused before it can reach a request path.
pub(crate) fn is_queue_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `GET /queues?per_page=100&page=N`: one page of the listing.
pub(crate) fn list_page(page: u64) -> Request {
    Request {
        method: Method::Get,
        path: format!("{QUEUES}?per_page={PER_PAGE}&page={page}"),
        body: None,
        header: None,
        idempotent: Idempotent::Yes,
    }
}

/// The id of the queue named exactly `name` on one listing page, and how many entries the page
/// held; `None` when the answer has no `result` list.
pub(crate) fn found(body: &serde_json::Value, name: &str) -> Option<(Option<String>, usize)> {
    let entries = body["result"].as_array()?;
    let id = entries
        .iter()
        .find(|q| q["queue_name"].as_str() == Some(name))
        .and_then(|q| q["queue_id"].as_str())
        .map(str::to_string);
    Some((id, entries.len()))
}

/// `POST /queues {"queue_name": …}`. Not idempotent: a second create of the same name fails.
pub(crate) fn create(name: &str) -> Request {
    Request {
        method: Method::Post,
        path: QUEUES.to_string(),
        body: Some(serde_json::json!({ "queue_name": name })),
        header: None,
        idempotent: Idempotent::No,
    }
}

/// `DELETE /queues/{queue_id}`, for an id that [`is_queue_id`].
pub(crate) fn delete(id: &str) -> Request {
    Request {
        method: Method::Delete,
        path: format!("{QUEUES}/{id}"),
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
        assert_eq!(list_page(3).path, "/queues?per_page=100&page=3");
        let create = create("jobs");
        assert_eq!(
            (create.method, create.idempotent),
            (Method::Post, Idempotent::No)
        );
        assert_eq!(create.body.unwrap().to_string(), r#"{"queue_name":"jobs"}"#);
        let delete = delete("0a1b2c");
        assert_eq!(
            (delete.method, delete.path.as_str()),
            (Method::Delete, "/queues/0a1b2c")
        );
    }

    #[test]
    fn a_queue_is_found_by_its_exact_name() {
        let page = serde_json::json!({"result": [
            {"queue_id": "q1", "queue_name": "jobs-dlq"},
            {"queue_id": "q2", "queue_name": "jobs"}
        ]});
        assert_eq!(found(&page, "jobs"), Some((Some("q2".to_string()), 2)));
        assert_eq!(found(&page, "job"), Some((None, 2)));
        assert_eq!(found(&serde_json::json!({"result": null}), "jobs"), None);
    }

    #[test]
    fn only_a_plain_id_is_a_queue_id() {
        assert!(is_queue_id("0123456789abcdef0123456789abcdef"));
        for bad in ["", "a/b", "../x", "a?b"] {
            assert!(!is_queue_id(bad), "{bad:?}");
        }
        assert!(!is_queue_id(&"a".repeat(33)));
    }
}
