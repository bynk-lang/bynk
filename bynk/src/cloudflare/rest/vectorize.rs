//! Vectorize indexes over REST (1D.4c, #4056 Decision C): each operation as a [`Request`] and a
//! decoder for its answer. [`super::super::Account`] runs them.
//!
//! Cloudflare's v2 endpoints (`/accounts/{account_id}/vectorize/v2/indexes`), as the API
//! reference documents them: an index's `config` holds its `dimensions` and `metric`
//! (`cosine`, `euclidean` or `dot-product`), and a metadata-index listing is
//! `{ "metadataIndexes": [{ "propertyName", "indexType" }] }`. The reference documents no
//! paging for the index listing and no error code for a metadata index that already exists, so
//! the listing is followed while pages are full, within [`MAX_PAGES`], and an existing metadata
//! index is read from the account (see `deploy::vectorize`).

use super::{Method, Request};
use crate::cloudflare::Idempotent;

/// The account's indexes.
pub(crate) const INDEXES: &str = "/vectorize/v2/indexes";

/// The most pages of the index listing read (#4568 review). The reference documents no paging
/// for it, so the listing is read while pages are full; this bound, and the stop on a page that
/// brings no new name, keep an endpoint that ignores `?page=N` from being asked forever. A
/// hundred full pages is far beyond any account the deploy has met: one listed past it reads as
/// absent, and its create is then refused and reported.
pub(crate) const MAX_PAGES: u64 = 100;

/// An index's fixed configuration, as the account reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexConfig {
    pub(crate) dimensions: i64,
    pub(crate) metric: String,
}

/// A metric as a declaration spells it (`dot-product`), whatever case or separator the answer
/// uses.
pub(crate) fn normalise_metric(metric: &str) -> String {
    metric.trim().to_ascii_lowercase().replace('_', "-")
}

/// Whether `name` can be an index name: ASCII letters, digits, hyphens and underscores, at most
/// 64. Anything else is refused before it can reach a request path.
pub(crate) fn is_index_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn request(method: Method, path: String, body: Option<serde_json::Value>) -> Request {
    let idempotent = if method == Method::Post {
        Idempotent::No
    } else {
        Idempotent::Yes
    };
    Request {
        method,
        path,
        body,
        header: None,
        idempotent,
    }
}

/// `GET /vectorize/v2/indexes?page=N`: one page of the listing.
pub(crate) fn list_page(page: u64) -> Request {
    request(Method::Get, format!("{INDEXES}?page={page}"), None)
}

/// The index names on one listing page, and how many entries it held; `None` when the answer
/// has no `result` list or an entry has no name.
pub(crate) fn listed(body: &serde_json::Value) -> Option<(Vec<String>, usize)> {
    let entries = body["result"].as_array()?;
    let names = entries
        .iter()
        .map(|i| i["name"].as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    Some((names, entries.len()))
}

/// `GET /vectorize/v2/indexes/{name}`.
pub(crate) fn get(name: &str) -> Request {
    request(Method::Get, format!("{INDEXES}/{name}"), None)
}

/// The index's configuration: `result.config`, or `result` itself when it carries the fields.
pub(crate) fn config(body: &serde_json::Value) -> Option<IndexConfig> {
    let result = &body["result"];
    let config = result.get("config").unwrap_or(result);
    Some(IndexConfig {
        dimensions: config.get("dimensions")?.as_i64()?,
        metric: normalise_metric(config.get("metric")?.as_str()?),
    })
}

/// `POST /vectorize/v2/indexes {"name", "config": {"dimensions", "metric"}}`.
pub(crate) fn create(name: &str, dimensions: i64, metric: &str) -> Request {
    request(
        Method::Post,
        INDEXES.to_string(),
        Some(serde_json::json!({
            "name": name,
            "config": { "dimensions": dimensions, "metric": metric },
        })),
    )
}

/// `DELETE /vectorize/v2/indexes/{name}`.
pub(crate) fn delete(name: &str) -> Request {
    request(Method::Delete, format!("{INDEXES}/{name}"), None)
}

/// `GET /vectorize/v2/indexes/{name}/metadata_index/list`.
pub(crate) fn metadata_list(name: &str) -> Request {
    request(
        Method::Get,
        format!("{INDEXES}/{name}/metadata_index/list"),
        None,
    )
}

/// The metadata indexes in a listing, as `name:type` (the type lowercased): `None` when the
/// answer has no `metadataIndexes` list or an entry lacks a property or a type.
pub(crate) fn metadata(body: &serde_json::Value) -> Option<Vec<String>> {
    body["result"]["metadataIndexes"]
        .as_array()?
        .iter()
        .map(|i| {
            let name = i["propertyName"].as_str()?;
            let ty = i["indexType"].as_str()?;
            Some(format!("{name}:{}", ty.to_ascii_lowercase()))
        })
        .collect()
}

/// `POST /vectorize/v2/indexes/{name}/metadata_index/create {"propertyName", "indexType"}`.
/// Cloudflare only enqueues the build (it answers a `mutationId`).
pub(crate) fn metadata_create(name: &str, property: &str, ty: &str) -> Request {
    request(
        Method::Post,
        format!("{INDEXES}/{name}/metadata_index/create"),
        Some(serde_json::json!({ "propertyName": property, "indexType": ty })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_operation_is_the_documented_request() {
        assert_eq!(list_page(2).path, "/vectorize/v2/indexes?page=2");
        assert_eq!(get("docs").path, "/vectorize/v2/indexes/docs");
        let create = create("docs", 768, "cosine");
        assert_eq!(
            (create.method, create.idempotent),
            (Method::Post, Idempotent::No)
        );
        assert_eq!(
            create.body.unwrap().to_string(),
            r#"{"config":{"dimensions":768,"metric":"cosine"},"name":"docs"}"#
        );
        assert_eq!(delete("docs").method, Method::Delete);
        assert_eq!(
            metadata_list("docs").path,
            "/vectorize/v2/indexes/docs/metadata_index/list"
        );
        let meta = metadata_create("docs", "category", "string");
        assert_eq!(
            meta.path,
            "/vectorize/v2/indexes/docs/metadata_index/create"
        );
        assert_eq!(
            meta.body.unwrap().to_string(),
            r#"{"indexType":"string","propertyName":"category"}"#
        );
    }

    #[test]
    fn the_answers_are_read_from_their_documented_fields() {
        let index = serde_json::json!({"result": {"name": "docs", "config": {"dimensions": 768, "metric": "dot-product"}}});
        assert_eq!(
            config(&index),
            Some(IndexConfig {
                dimensions: 768,
                metric: "dot-product".into()
            })
        );
        let shouting = serde_json::json!({"result": {"dimensions": 3, "metric": "DOT_PRODUCT"}});
        assert_eq!(config(&shouting).unwrap().metric, "dot-product");
        assert_eq!(config(&serde_json::json!({"result": {"name": "x"}})), None);
        let list = serde_json::json!({"result": [{"name": "a", "config": {}}, {"name": "b"}]});
        assert_eq!(listed(&list), Some((vec!["a".into(), "b".into()], 2)));
        assert_eq!(
            listed(&serde_json::json!({"result": [{"config": {}}]})),
            None
        );
        let meta = serde_json::json!({"result": {"metadataIndexes": [
            {"propertyName": "category", "indexType": "String"},
            {"propertyName": "year", "indexType": "number"}
        ]}});
        assert_eq!(
            metadata(&meta),
            Some(vec!["category:string".into(), "year:number".into()])
        );
        assert_eq!(
            metadata(&serde_json::json!({"result": {"metadataIndexes": [{"x": 1}]}})),
            None
        );
        assert_eq!(metadata(&serde_json::json!({"result": []})), None);
    }

    #[test]
    fn only_a_plain_index_name_is_one() {
        assert!(is_index_name("shop-docs_2"));
        for bad in ["", "a/b", "../x", "a?b", "a b"] {
            assert!(!is_index_name(bad), "{bad:?}");
        }
    }
}
