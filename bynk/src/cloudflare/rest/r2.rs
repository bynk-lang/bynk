//! R2 buckets over REST (1D.4b, #4056 Decision C): each operation as a [`Request`] and a decoder
//! for its answer. [`super::super::Account`] runs them.
//!
//! Cloudflare's endpoints (`/accounts/{account_id}/r2/buckets`). A bucket's jurisdiction travels
//! in the `cf-r2-jurisdiction` header, not the body, as the plan's own R2 listing already sends it.

use super::{Method, Request};
use crate::cloudflare::Idempotent;

/// The account's buckets.
pub(crate) const BUCKETS: &str = "/r2/buckets";

/// The header that names a bucket's jurisdiction.
pub(crate) const JURISDICTION: &str = "cf-r2-jurisdiction";

/// Whether `name` can be a bucket name: 3 to 64 lowercase ASCII letters, digits and hyphens, as
/// Cloudflare's create accepts. Anything else is refused before it can reach a request path.
pub(crate) fn is_bucket_name(name: &str) -> bool {
    (3..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn header(jurisdiction: Option<&str>) -> Option<(&'static str, String)> {
    jurisdiction.map(|j| (JURISDICTION, j.to_string()))
}

/// `POST /r2/buckets {"name": …}`, in `jurisdiction` when given. Not idempotent: a second create
/// of the same name fails.
pub(crate) fn create(name: &str, jurisdiction: Option<&str>) -> Request {
    Request {
        method: Method::Post,
        path: BUCKETS.to_string(),
        body: Some(serde_json::json!({ "name": name })),
        header: header(jurisdiction),
        idempotent: Idempotent::No,
    }
}

/// `GET /r2/buckets/{name}`, in `jurisdiction` when given: answered only for a bucket this
/// account holds there.
pub(crate) fn bucket(name: &str, jurisdiction: Option<&str>) -> Request {
    Request {
        method: Method::Get,
        path: format!("{BUCKETS}/{name}"),
        body: None,
        header: header(jurisdiction),
        idempotent: Idempotent::Yes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_operation_is_the_documented_request() {
        let create = create("shop-files", Some("eu"));
        assert_eq!(
            (create.method, create.path.as_str(), create.idempotent),
            (Method::Post, "/r2/buckets", Idempotent::No)
        );
        assert_eq!(create.body.unwrap().to_string(), r#"{"name":"shop-files"}"#);
        assert_eq!(create.header, Some((JURISDICTION, "eu".to_string())));
        assert_eq!(super::create("b", None).header, None);
        let get = bucket("shop-files", None);
        assert_eq!(
            (get.method, get.path.as_str(), get.header),
            (Method::Get, "/r2/buckets/shop-files", None)
        );
    }

    #[test]
    fn only_a_valid_bucket_name_is_one() {
        assert!(is_bucket_name("shop-files"));
        assert!(is_bucket_name("abc"));
        for bad in ["ab", "Shop", "a/b", "../x", "a?b", "a_b", "a b", ""] {
            assert!(!is_bucket_name(bad), "{bad:?}");
        }
        assert!(!is_bucket_name(&"a".repeat(65)));
    }
}
