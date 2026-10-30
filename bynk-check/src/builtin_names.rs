//! Centralised string literals for the language's built-in vocabulary (refactor
//! track item 8, v0.29.11). Built-in type/method names were compared as bare
//! string literals scattered across the checker, emitter, and project modules;
//! a typo was a silent never-match. One edit point per name now.

/// Built-in type names (as they appear in source / qualified positions).
pub mod types {
    pub const JSON: &str = "Json";
    pub const LIST: &str = "List";
    pub const MAP: &str = "Map";
    /// K6a (#2596): the built-in immutable set.
    pub const SET: &str = "Set";
    pub const INT: &str = "Int";
    pub const FLOAT: &str = "Float";
    pub const DURATION: &str = "Duration";
    pub const INSTANT: &str = "Instant";
    pub const BYTES: &str = "Bytes";
    /// #2341 (L4): an HTTP handler's read-only view of the request's headers.
    pub const REQUEST_HEADERS: &str = "RequestHeaders";
    /// #2380 (L5): an `Oidc` actor's verified token claims.
    pub const TOKEN_CLAIMS: &str = "TokenClaims";
    pub const HTTP_RESULT: &str = "HttpResult";
    pub const QUEUE_RESULT: &str = "QueueResult";
    pub const STREAM: &str = "Stream";
    /// The compiler-known entry record a map's `.entries` query yields
    /// (v0.158, ADR 0184). A nominal generic record `{ key: K, value: V }`
    /// — bynk stays nominal (ADR 0120), so a map entry is a named record, not
    /// an anonymous pair. Non-boundary like every generic-record instantiation
    /// (ADR 0183): project it through `.map` into a named type before a
    /// terminal that leaves the pipeline.
    pub const MAP_ENTRY: &str = "MapEntry";
}

/// The `.entries` / `.keys` / `.values` accessors a `store Map[K, V]` field
/// exposes as lazy queries (v0.158, ADR 0184), and the two fields the
/// `MapEntry` record carries.
pub mod map_query {
    /// `map.entries : Query[MapEntry[K, V]]`.
    pub const ENTRIES: &str = "entries";
    /// `map.keys : Query[K]`.
    pub const KEYS: &str = "keys";
    /// `map.values : Query[V]`.
    pub const VALUES: &str = "values";
    /// `MapEntry.key : K`.
    pub const KEY: &str = "key";
    /// `MapEntry.value : V`.
    pub const VALUE: &str = "value";
}

/// Privileged built-in member names — constructors (`of`/`unsafe`), the refined
/// raw accessor (`raw`), and the effect terminals (`foldEff`/`forEach`).
pub mod methods {
    pub const OF: &str = "of";
    pub const UNSAFE: &str = "unsafe";
    pub const RAW: &str = "raw";
    pub const FOLD_EFF: &str = "foldEff";
    pub const FOR_EACH: &str = "forEach";
    pub const PAR_TRAVERSE: &str = "parTraverse";
    pub const TRAVERSE_ALL: &str = "traverseAll";
    pub const PAR_TRAVERSE_ALL: &str = "parTraverseAll";
    pub const TRAVERSE_TRY: &str = "traverseTry";
    pub const PAR_TRAVERSE_TRY: &str = "parTraverseTry";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_hold_expected_values() {
        assert_eq!(types::JSON, "Json");
        assert_eq!(types::HTTP_RESULT, "HttpResult");
        assert_eq!(methods::OF, "of");
        assert_eq!(methods::FOLD_EFF, "foldEff");
        assert_eq!(methods::FOR_EACH, "forEach");
        assert_eq!(methods::PAR_TRAVERSE, "parTraverse");
        assert_eq!(methods::TRAVERSE_ALL, "traverseAll");
        assert_eq!(methods::PAR_TRAVERSE_ALL, "parTraverseAll");
        assert_eq!(methods::TRAVERSE_TRY, "traverseTry");
        assert_eq!(methods::PAR_TRAVERSE_TRY, "parTraverseTry");
    }
}
