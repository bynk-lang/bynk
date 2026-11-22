//! The generated names that both the case lowering and the test harness spell (back-half
//! paydown 1B.2c, #4028). The emitter's `bynk test` case lowering *calls* the `system`-tier
//! drivers and the harness (`testgen`) *defines* them; each name has one spelling here, so the
//! caller and the definer cannot drift. The harness's own names are in `testgen/names.rs`.

/// Which `system`-tier HTTP driver a name is for: the typed (JSON) or the raw (`Wire(..)`)
/// form, and a kind that names its principal (`noauth`, `forged`, `stale`, ...), empty for the
/// plain driver. The kind carries no trailing `_`.
#[derive(Clone, Copy)]
pub(crate) struct SysdriveKind<'a> {
    pub(crate) raw: bool,
    pub(crate) kind: &'a str,
}

impl<'a> SysdriveKind<'a> {
    /// A typed driver of `kind`.
    pub(crate) fn typed(kind: &'a str) -> Self {
        SysdriveKind { raw: false, kind }
    }

    /// A raw driver of `kind`.
    pub(crate) fn raw(kind: &'a str) -> Self {
        SysdriveKind { raw: true, kind }
    }
}

/// The name of a `system`-tier HTTP driver: `__sysdrive_[raw]<kind>[_]<service>_<key>`. The
/// separator after the kind is absent only for the plain typed driver, which has no kind.
pub(crate) fn sysdrive_name(kind: SysdriveKind<'_>, service: &str, key: &str) -> String {
    let raw = if kind.raw { "raw" } else { "" };
    let sep = if kind.raw || !kind.kind.is_empty() {
        "_"
    } else {
        ""
    };
    format!("__sysdrive_{raw}{}{sep}{service}_{key}", kind.kind)
}

/// The generic `405` driver of a service (#707).
pub(crate) fn wrong_method_driver_name(service: &str) -> String {
    format!("__sysdrive_wrongmethod_{service}")
}

/// A service route's key with a suffix (`?query`, `?cookies`, `?form`, `?extra-headers`), the
/// key under which the harness records the route's extra slots.
pub(crate) fn route_with(path: &str, suffix: &str) -> String {
    format!("{path}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every spelling a driver has today, so the helper cannot drift from the literals it
    /// replaced (the plain driver has no `_` after an empty kind, the raw one keeps it).
    #[test]
    fn sysdrive_names_keep_their_six_forms() {
        let name = |kind| sysdrive_name(kind, "api", "http_GET_open");
        assert_eq!(
            name(SysdriveKind::typed("")),
            "__sysdrive_api_http_GET_open"
        );
        assert_eq!(
            name(SysdriveKind::raw("")),
            "__sysdrive_raw_api_http_GET_open"
        );
        assert_eq!(
            name(SysdriveKind::typed("noauth")),
            "__sysdrive_noauth_api_http_GET_open"
        );
        assert_eq!(
            name(SysdriveKind::raw("noauth")),
            "__sysdrive_rawnoauth_api_http_GET_open"
        );
        assert_eq!(
            name(SysdriveKind::typed("forged")),
            "__sysdrive_forged_api_http_GET_open"
        );
        assert_eq!(
            name(SysdriveKind::raw("stale")),
            "__sysdrive_rawstale_api_http_GET_open"
        );
    }

    #[test]
    fn the_wrong_method_driver_and_route_keys_have_one_spelling() {
        assert_eq!(
            wrong_method_driver_name("api"),
            "__sysdrive_wrongmethod_api"
        );
        assert_eq!(route_with("/a", "?query"), "/a?query");
    }
}
