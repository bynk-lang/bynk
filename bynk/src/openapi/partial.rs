//! #4389: the `206 Partial Content` response of `bynk openapi`.

use serde_json::{Value, json};

/// The part's range header, `bytes first-last/total` (`*` for an unknown total); every `206` carries it.
pub(super) fn content_range_header() -> Value {
    json!({ "required": true, "schema": { "type": "string" } })
}
