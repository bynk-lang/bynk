//! Drift guard: #1732 — the docs that quote the pinned Workers compatibility
//! date, and the oldest wrangler that serves it, name the values the emitter
//! and `bynk doctor` actually use.
//!
//! A compatibility-date review moves `COMPATIBILITY_DATE` and `WRANGLER_MIN`
//! together (`design/bynk-release-discipline.md` Part 3). The docs restate both
//! for readers, and nothing else ties the prose to the constants. So a review
//! that moves the pair and misses a doc fails here, naming the doc.

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

/// Each doc, and which of the two values it must quote.
const DOCS: [(&str, bool, bool); 3] = [
    // (path, quotes the date, quotes the wrangler minimum)
    ("design/bynk-release-discipline.md", true, true),
    (
        "site/src/content/docs/reference/tooling/editor-and-tooling/doctor.md",
        true,
        true,
    ),
    (
        "site/src/content/docs/reference/tooling/emission.md",
        true,
        false,
    ),
];

#[test]
fn the_docs_quote_the_current_compatibility_date_and_wrangler_minimum() {
    let date = bynk_emit::COMPATIBILITY_DATE;
    let min = bynk_emit::WRANGLER_MIN;
    for (path, wants_date, wants_min) in DOCS {
        let text = read(path);
        if wants_date {
            assert!(
                text.contains(date),
                "{path} doesn't mention the compatibility date {date}. A \
                 compatibility-date review moved it: update the doc too."
            );
        }
        if wants_min {
            assert!(
                text.contains(min),
                "{path} doesn't mention the wrangler minimum {min}. A \
                 compatibility-date review moved it: update the doc too."
            );
        }
    }
}

/// The minimum is a plain `X.Y.Z`, so `bynk doctor` can compare against it.
#[test]
fn the_wrangler_minimum_is_a_plain_version() {
    let parts: Vec<&str> = bynk_emit::WRANGLER_MIN.split('.').collect();
    assert!(
        parts.len() == 3 && parts.iter().all(|p| p.parse::<u32>().is_ok()),
        "WRANGLER_MIN must be X.Y.Z, got {}",
        bynk_emit::WRANGLER_MIN
    );
}
