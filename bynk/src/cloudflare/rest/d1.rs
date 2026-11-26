//! D1 databases over REST (1D.4b, #4056 Decision C): each operation as a [`Request`] and a
//! decoder for its answer. [`super::super::Account`] runs them. D1 *migrations* stay on wrangler
//! (Q4: it keeps its own migrations table).
//!
//! Cloudflare's endpoints (`/accounts/{account_id}/d1/database`): the create takes the
//! jurisdiction in its body, and the new database's id is `result.uuid`, the `database_id`
//! wrangler printed.

use super::{Method, Request};
use crate::cloudflare::Idempotent;

/// The account's databases.
pub(crate) const DATABASES: &str = "/d1/database";

/// How many databases a lookup page asks for.
const PER_PAGE: u32 = 100;

/// `POST /d1/database {"name": …, "jurisdiction": …}`. Not idempotent: a second create of the
/// same name fails.
pub(crate) fn create(name: &str, jurisdiction: Option<&str>) -> Request {
    let mut body = serde_json::json!({ "name": name });
    if let Some(jurisdiction) = jurisdiction {
        body["jurisdiction"] = jurisdiction.into();
    }
    Request {
        method: Method::Post,
        path: DATABASES.to_string(),
        body: Some(body),
        header: None,
        idempotent: Idempotent::No,
    }
}

/// The new database's id: `result.uuid`.
pub(crate) fn created(body: &serde_json::Value) -> Option<String> {
    body["result"]["uuid"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// `GET /d1/database?name=…&per_page=100&page=N`: one page of the databases Cloudflare finds for
/// `name`. The reference calls `name` "a database name to search for" and does not say the
/// match is exact, so [`found`] matches the name exactly.
pub(crate) fn find_page(name: &str, page: u64) -> Request {
    Request {
        method: Method::Get,
        path: format!(
            "{DATABASES}?name={}&per_page={PER_PAGE}&page={page}",
            crate::dev_inspect::percent_encode(name)
        ),
        body: None,
        header: None,
        idempotent: Idempotent::Yes,
    }
}

/// The id of the database named exactly `name` on one lookup page, and how many entries the
/// page held; `None` when the answer has no `result` list.
pub(crate) fn found(body: &serde_json::Value, name: &str) -> Option<(Option<String>, usize)> {
    let entries = body["result"].as_array()?;
    let id = entries
        .iter()
        .find(|db| db["name"].as_str() == Some(name))
        .and_then(|db| db["uuid"].as_str())
        .map(str::to_string);
    Some((id, entries.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_operation_is_the_documented_request() {
        let create = create("shop-orders", Some("eu"));
        assert_eq!(
            (create.method, create.path.as_str(), create.idempotent),
            (Method::Post, "/d1/database", Idempotent::No)
        );
        assert_eq!(
            create.body.unwrap().to_string(),
            r#"{"jurisdiction":"eu","name":"shop-orders"}"#
        );
        assert_eq!(
            super::create("d", None).body.unwrap().to_string(),
            r#"{"name":"d"}"#
        );
        assert_eq!(
            find_page("shop orders", 2).path,
            "/d1/database?name=shop%20orders&per_page=100&page=2"
        );
    }

    #[test]
    fn the_answers_are_read_from_their_documented_fields() {
        let body = serde_json::json!({"result": {"uuid": "9b3c", "name": "d"}});
        assert_eq!(created(&body).as_deref(), Some("9b3c"));
        assert_eq!(created(&serde_json::json!({"result": {}})), None);
        // A search that also finds a longer name: only the exact one is the database.
        let page = serde_json::json!({"result": [
            {"name": "shop-orders-old", "uuid": "u-old"},
            {"name": "shop-orders", "uuid": "u-1"}
        ]});
        assert_eq!(
            found(&page, "shop-orders"),
            Some((Some("u-1".to_string()), 2))
        );
        assert_eq!(found(&page, "shop"), Some((None, 2)));
        assert_eq!(found(&serde_json::json!({"result": null}), "x"), None);
    }
}
