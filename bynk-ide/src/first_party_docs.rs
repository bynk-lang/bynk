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

/// `(first-party unit, capability, anchor on `CAPABILITIES_PAGE`, operations)`.
/// A capability documented only as a table row links to its unit's surface
/// section. The operations are listed so a misspelt `[Clock.nwo]` stays plain
/// text, as a project-scope miss does, and they are the did-you-mean candidates
/// for the unresolved-link hint, so do not trim them.
type Entry = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
);
pub const FIRST_PARTY_DOCS: &[Entry] = &[
    ("bynk", "Clock", "the-portable-surface--bynk", &["now"]),
    (
        "bynk",
        "Random",
        "the-portable-surface--bynk",
        &["uuid", "int", "bytes"],
    ),
    (
        "bynk",
        "Logger",
        "the-portable-surface--bynk",
        &["info", "error", "warn", "debug", "log"],
    ),
    (
        "bynk",
        "Fetch",
        "the-portable-surface--bynk",
        &["send", "sendWith", "receiveBytes", "sendBytes"],
    ),
    ("bynk", "Secrets", "the-portable-surface--bynk", &["get"]),
    ("bynk", "Locale", "the-portable-surface--bynk", &["current"]),
    (
        "bynk",
        "Idempotency",
        "the-idempotency-capability",
        &["dedup", "remember"],
    ),
    ("bynk", "Events", "the-events-capability", &["emit"]),
    (
        "bynk.cloudflare",
        "Kv",
        "the-cloudflare-surface--bynkcloudflare",
        &[
            "get", "put", "putTtl", "delete", "list", "getWith", "putWith", "getBytes", "putBytes",
            "listPage",
        ],
    ),
    (
        "bynk.cloudflare",
        "Service",
        "calling-a-worker-outside-the-project-service",
        &["send"],
    ),
    (
        "bynk.cloudflare",
        "R2",
        "object-storage-r2",
        &["get", "head", "put", "putWith", "delete", "list"],
    ),
    (
        "bynk.cloudflare",
        "RateLimit",
        "rate-limiting-ratelimit",
        &["allow"],
    ),
    (
        "bynk.cloudflare",
        "Queue",
        "sending-to-a-queue-queue",
        &["send", "sendWith", "sendBatch"],
    ),
    (
        "bynk.cloudflare",
        "Analytics",
        "writing-events-analytics",
        &["write"],
    ),
    (
        "bynk.cloudflare",
        "D1",
        "sql-databases-d1",
        &["all", "first", "run", "batch"],
    ),
    (
        "bynk.cloudflare",
        "EdgeCache",
        "edge-caching-edgecache",
        &["purge"],
    ),
];

/// The Book URL for the first-party capability a doc-link `name` (`Clock` or
/// `Clock.now`) names, provided its unit is in `owner_unit`'s doc-link scope.
/// Run only after the project scope found nothing.
pub fn resolve_first_party_doc_link(
    doc_scope: &HashMap<String, Vec<String>>,
    owner_unit: &str,
    name: &str,
) -> Option<String> {
    let mut parts = name.split('.');
    let capability = parts.next()?;
    let op = parts.next();
    if parts.next().is_some() {
        return None;
    }
    let scope = doc_scope.get(owner_unit)?;
    FIRST_PARTY_DOCS
        .iter()
        .find(|(unit, cap, _, ops)| {
            *cap == capability
                && op.is_none_or(|o| ops.contains(&o))
                && scope.iter().any(|u| u == unit)
        })
        .map(|(_, _, anchor, _)| format!("{CAPABILITIES_PAGE}#{anchor}"))
}

/// Every first-party capability (`Clock`) and operation (`Clock.now`) name
/// whose unit is in `owner_unit`'s doc-link scope: the did-you-mean candidates
/// for a link that [`resolve_first_party_doc_link`] rejected.
pub fn in_scope_names(doc_scope: &HashMap<String, Vec<String>>, owner_unit: &str) -> Vec<String> {
    let Some(scope) = doc_scope.get(owner_unit) else {
        return Vec::new();
    };
    FIRST_PARTY_DOCS
        .iter()
        .filter(|(unit, ..)| scope.iter().any(|u| u == unit))
        .flat_map(|(_, cap, _, ops)| {
            std::iter::once((*cap).to_string()).chain(ops.iter().map(move |o| format!("{cap}.{o}")))
        })
        .collect()
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
    fn every_mapped_anchor_and_operation_exists_in_the_book_source() {
        let site = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("site");
        if !site.exists() {
            return; // published crate: the site is not packaged
        }
        let text = std::fs::read_to_string(
            site.join("src/content/docs/book/reference/bynk-capabilities.md"),
        )
        .expect("the first-party capabilities page moved: update CAPABILITIES_PAGE and this test");
        let mut in_fence = false;
        let mut anchors = Vec::new();
        for l in text.lines() {
            if l.trim_start().starts_with("```") {
                in_fence = !in_fence;
            } else if !in_fence && l.starts_with('#') {
                anchors.push(slug(l.trim_start_matches('#')));
            }
        }
        for (_, cap, anchor, ops) in FIRST_PARTY_DOCS {
            assert!(anchors.iter().any(|a| a == anchor), "{cap}: no `#{anchor}`");
            for op in *ops {
                assert!(
                    text.contains(&format!("{op}(")) || text.contains(&format!("{op}[")),
                    "{cap}.{op}: not in the Book"
                );
            }
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
        // An op the capability lacks, or a longer path, stays plain text.
        assert!(resolve_first_party_doc_link(&s, "ratelimit", "Clock.nwo").is_none());
        assert!(resolve_first_party_doc_link(&s, "ratelimit", "Kv.a.b").is_none());
    }
}
