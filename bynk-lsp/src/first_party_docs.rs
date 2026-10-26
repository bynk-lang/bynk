//! #2455: doc links to first-party names (`[Clock]`, `[Clock.now]`).
//!
//! First-party (`bynk`, `bynk.cloudflare`, `bynk.crypto`) units are synthetic:
//! their symbols never reach `index.symbols`, so `resolve_doc_link` cannot
//! find them and there is no file to open. They link to the Book instead, via
//! this curated table — deriving anchors from headings is brittle while some
//! capabilities are table rows rather than headings. Granularity is the
//! capability: an operation links to its capability's entry. The target is the
//! live Book, as the diagnostic `codeDescription` links are.

use std::collections::HashMap;

/// The Book page every entry below lives on.
const CAPABILITIES_PAGE: &str = "https://bynk-lang.org/book/reference/bynk-capabilities/";

/// `(first-party unit, capability, anchor on `CAPABILITIES_PAGE`)`. A capability
/// documented only as a table row links to its unit's surface section.
pub const FIRST_PARTY_DOCS: &[(&str, &str, &str)] = &[
    ("bynk", "Clock", "the-portable-surface--bynk"),
    ("bynk", "Random", "the-portable-surface--bynk"),
    ("bynk", "Logger", "the-portable-surface--bynk"),
    ("bynk", "Fetch", "the-portable-surface--bynk"),
    ("bynk", "Secrets", "the-portable-surface--bynk"),
    ("bynk", "Locale", "the-portable-surface--bynk"),
    ("bynk", "Idempotency", "the-idempotency-capability"),
    ("bynk", "Events", "the-events-capability"),
    (
        "bynk.cloudflare",
        "Kv",
        "the-cloudflare-surface--bynkcloudflare",
    ),
    (
        "bynk.cloudflare",
        "Service",
        "calling-a-worker-outside-the-project-service",
    ),
    ("bynk.cloudflare", "R2", "object-storage-r2"),
    ("bynk.cloudflare", "RateLimit", "rate-limiting-ratelimit"),
    ("bynk.cloudflare", "EdgeCache", "edge-caching-edgecache"),
];

/// The Book URL for the first-party capability a doc-link `name` (`Clock` or
/// `Clock.now`) names, provided its unit is in `owner_unit`'s doc-link scope.
/// Run only after the project scope found nothing.
pub fn resolve_first_party_doc_link(
    doc_scope: &HashMap<String, Vec<String>>,
    owner_unit: &str,
    name: &str,
) -> Option<String> {
    let capability = name.split('.').next()?;
    let scope = doc_scope.get(owner_unit)?;
    FIRST_PARTY_DOCS
        .iter()
        .find(|(unit, cap, _)| *cap == capability && scope.iter().any(|u| u == unit))
        .map(|(_, _, anchor)| format!("{CAPABILITIES_PAGE}#{anchor}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// github-slugger, as Starlight applies it to a heading.
    fn slug(heading: &str) -> String {
        heading
            .trim()
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
            .map(|c| if c == ' ' { '-' } else { c })
            .collect()
    }

    #[test]
    fn every_mapped_anchor_exists_in_the_book_source() {
        let page = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("site/src/content/docs/book/reference/bynk-capabilities.md");
        let Ok(text) = std::fs::read_to_string(&page) else {
            return; // published crate: the site is not packaged
        };
        let anchors: Vec<String> = text
            .lines()
            .filter(|l| l.starts_with('#'))
            .map(|l| slug(l.trim_start_matches('#')))
            .collect();
        for (_, cap, anchor) in FIRST_PARTY_DOCS {
            assert!(anchors.iter().any(|a| a == anchor), "{cap}: no `#{anchor}`");
        }
    }

    fn scope(units: &[&str]) -> HashMap<String, Vec<String>> {
        HashMap::from([(
            "ratelimit".to_string(),
            units.iter().map(|u| u.to_string()).collect(),
        )])
    }

    #[test]
    fn op_links_to_its_capability_when_the_unit_is_in_scope() {
        let url =
            resolve_first_party_doc_link(&scope(&["ratelimit", "bynk"]), "ratelimit", "Clock.now");
        assert_eq!(
            url.as_deref(),
            Some(
                "https://bynk-lang.org/book/reference/bynk-capabilities/#the-portable-surface--bynk"
            )
        );
    }

    #[test]
    fn out_of_scope_or_unknown_names_do_not_link() {
        let s = scope(&["ratelimit"]);
        assert!(resolve_first_party_doc_link(&s, "ratelimit", "Clock").is_none());
        let s = scope(&["ratelimit", "bynk"]);
        assert!(resolve_first_party_doc_link(&s, "ratelimit", "Kv").is_none());
        assert!(resolve_first_party_doc_link(&s, "ratelimit", "Nope").is_none());
    }
}
