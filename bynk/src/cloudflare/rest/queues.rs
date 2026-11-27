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

/// `GET /queues/{queue_id}/consumers`: the queue's consumers, for an id that [`is_queue_id`].
///
/// #4601: a queue a Worker consumes cannot be deleted (400 `11005`), and the Worker cannot be
/// deleted while it consumes the queue (403 `10064`), as the 1D.4 sandbox teardown found
/// (#4056). Removing the consumer first breaks the cycle, so a queue is deleted after its
/// consumers.
pub(crate) fn list_consumers(queue_id: &str) -> Request {
    Request {
        method: Method::Get,
        path: format!("{QUEUES}/{queue_id}/consumers"),
        body: None,
        header: None,
        idempotent: Idempotent::Yes,
    }
}

/// One of a queue's consumers: its id, the Worker it runs when it names one, and whether it is
/// an HTTP pull consumer (`"type": "http_pull"`), which runs no Worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Consumer {
    pub(crate) id: String,
    pub(crate) script: Option<String>,
    pub(crate) pull: bool,
}

impl Consumer {
    /// Whether this is a Worker consumer running one of `own` (the project's scripts).
    pub(crate) fn is_own(&self, own: &std::collections::BTreeSet<String>) -> bool {
        !self.pull && self.script.as_ref().is_some_and(|s| own.contains(s))
    }

    /// The consumer as a refusal names it, with the command that detaches it from `queue`.
    fn foreign(&self, queue: &str) -> (String, String) {
        match (&self.script, self.pull) {
            (Some(script), false) => (
                format!("the Worker `{script}` (not one of this project's Workers)"),
                format!("wrangler queues consumer remove {queue} {script}"),
            ),
            (_, true) => (
                format!("the HTTP pull consumer `{}`", self.id),
                format!("wrangler queues consumer http remove {queue}"),
            ),
            (None, false) => (
                format!("the consumer `{}`, which names no Worker", self.id),
                format!("DELETE /queues/{{queue_id}}/consumers/{}", self.id),
            ),
        }
    }
}

/// #4601: why the queue `queue` is not deleted while `foreign` consume it: each named, with the
/// command that detaches it. Bynk detaches only its own Workers; a foreign Worker or an HTTP pull
/// consumer is someone else's reader, so the queue is left for a person to decide.
pub(crate) fn foreign_consumers(queue: &str, foreign: &[&Consumer]) -> String {
    let (names, commands): (Vec<String>, Vec<String>) =
        foreign.iter().map(|c| c.foreign(queue)).unzip();
    format!(
        "the queue `{queue}` is consumed by {}, so nothing was deleted for it: detach {} by hand \
         (`{}`) or keep the queue",
        names.join(" and "),
        if foreign.len() == 1 { "it" } else { "them" },
        commands.join("`, `"),
    )
}

/// The consumers in a [`list_consumers`] answer; `None` when it has no `result` list or an entry
/// has no `consumer_id`.
pub(crate) fn consumers(body: &serde_json::Value) -> Option<Vec<Consumer>> {
    body["result"]
        .as_array()?
        .iter()
        .map(|c| {
            Some(Consumer {
                id: c["consumer_id"].as_str()?.to_string(),
                script: c["script"]
                    .as_str()
                    .or_else(|| c["script_name"].as_str())
                    .map(str::to_string),
                pull: c["type"].as_str() == Some("http_pull"),
            })
        })
        .collect()
}

/// `DELETE /queues/{queue_id}/consumers/{consumer_id}`, for ids that [`is_queue_id`].
pub(crate) fn delete_consumer(queue_id: &str, consumer_id: &str) -> Request {
    Request {
        method: Method::Delete,
        path: format!("{QUEUES}/{queue_id}/consumers/{consumer_id}"),
        body: None,
        header: None,
        idempotent: Idempotent::Yes,
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
        let listed = list_consumers("0a1b2c");
        assert_eq!(
            (listed.method, listed.path.as_str()),
            (Method::Get, "/queues/0a1b2c/consumers")
        );
        let removed = delete_consumer("0a1b2c", "9f8e7d");
        assert_eq!(
            (removed.method, removed.path.as_str()),
            (Method::Delete, "/queues/0a1b2c/consumers/9f8e7d")
        );
    }

    #[test]
    fn a_queues_consumers_are_read_with_the_worker_they_run() {
        let body = serde_json::json!({"result": [
            {"consumer_id": "9f8e7d", "script": "shop-jobs", "type": "worker"},
            {"consumer_id": "1a2b3c", "script_name": "older-shape"},
            {"consumer_id": "4d5e6f", "type": "http_pull"},
        ]});
        assert_eq!(
            consumers(&body).unwrap(),
            [
                Consumer {
                    id: "9f8e7d".into(),
                    script: Some("shop-jobs".into()),
                    pull: false,
                },
                Consumer {
                    id: "1a2b3c".into(),
                    script: Some("older-shape".into()),
                    pull: false,
                },
                Consumer {
                    id: "4d5e6f".into(),
                    script: None,
                    pull: true,
                },
            ]
        );
        assert_eq!(consumers(&serde_json::json!({"result": []})).unwrap(), []);
        assert_eq!(consumers(&serde_json::json!({})), None);
        assert_eq!(
            consumers(&serde_json::json!({"result": [{"script": "x"}]})),
            None
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
