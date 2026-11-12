//! The cumulative migration note (#3178).
//!
//! `design/bynk-release-discipline.md` promises each release an upgrade note:
//! what breaks, and how to move. It is assembled, not remembered. An increment
//! declares `breaking:` in its pending file; the stamp ([`crate::stamp`]) writes
//! it under the increment's version into the Book page [`UPGRADING_PAGE`]; and
//! [`release_section`] — behind `cargo xtask release-notes` — reads that page to
//! build the "Upgrading from vPREV" section of a release body, covering every
//! version after the previous release up to and including this one.
//!
//! The page is the canonical record; the release body is derived from it, so
//! the two cannot drift.

use crate::stamp::Version;

/// The Book's upgrading page, relative to the repo root.
pub const UPGRADING_PAGE: &str = "site/src/content/docs/book/reference/upgrading.md";

/// The marker the stamp inserts new entries after (newest first). It is an HTML
/// comment so it is invisible in the rendered page.
pub const ENTRIES_MARKER: &str = "<!-- stamp:breaking-entries -->";

/// One page entry: a `## vX.Y.Z` heading and the note.
pub fn upgrading_entry(version: Version, note: &str) -> String {
    format!("## v{version}\n\n{note}\n\n")
}

/// Insert `entries` (already newest-first) straight after [`ENTRIES_MARKER`].
pub fn insert_upgrading_entries(page: &str, entries: &[String]) -> Result<String, String> {
    let Some(at) = page.find(ENTRIES_MARKER) else {
        return Err(format!(
            "{UPGRADING_PAGE} has no `{ENTRIES_MARKER}` line to insert breaking-change \
             entries after"
        ));
    };
    let split = at + ENTRIES_MARKER.len();
    let (head, tail) = page.split_at(split);
    let tail = tail.trim_start_matches('\n');
    Ok(format!("{head}\n\n{}{tail}", entries.concat()))
}

/// Every `(version, note)` entry on the page, in page order (newest first).
pub fn parse_entries(page: &str) -> Vec<(Version, String)> {
    let Some(at) = page.find(ENTRIES_MARKER) else {
        return Vec::new();
    };
    let mut out: Vec<(Version, String)> = Vec::new();
    for line in page[at + ENTRIES_MARKER.len()..].lines() {
        if let Some(v) = line
            .strip_prefix("## v")
            .and_then(|rest| Version::parse(rest.trim()))
        {
            out.push((v, String::new()));
        } else if let Some((_, note)) = out.last_mut() {
            note.push_str(line);
            note.push('\n');
        }
    }
    for (_, note) in &mut out {
        *note = note.trim().to_string();
    }
    out
}

/// The version a release tag names: `vX.Y.Z`, with any pre-release suffix
/// (`v0.16.0-rc1`) dropped, as `release.yml`'s verify job does, so a pre-release
/// tag still gets a release body.
pub fn parse_release_tag(tag: &str) -> Option<Version> {
    let rest = tag.strip_prefix('v')?;
    Version::parse(rest.split('-').next()?)
}

fn ordering_key(v: Version) -> (u64, u64, u64) {
    (v.major, v.minor, v.patch)
}

/// The release before `current` among `tags`: the greatest plain `vX.Y.Z` tag
/// strictly below it. Pre-release tags (`v0.16.0-rc1`) and anything unparseable
/// are ignored.
pub fn previous_tag(tags: &[String], current: Version) -> Option<Version> {
    tags.iter()
        .filter_map(|t| Version::parse(t.trim().strip_prefix('v')?))
        .filter(|v| ordering_key(*v) < ordering_key(current))
        .max_by_key(|v| ordering_key(*v))
}

/// The release-body section: breaking changes in `(prev, tag]`, oldest first.
/// Always a section, even when empty, so a reader can tell "nothing breaks" from
/// "nobody wrote the note".
pub fn release_section(page: &str, prev: Option<Version>, tag: Version) -> String {
    let mut entries: Vec<(Version, String)> = parse_entries(page)
        .into_iter()
        .filter(|(v, _)| {
            ordering_key(*v) <= ordering_key(tag)
                && prev.is_none_or(|p| ordering_key(*v) > ordering_key(p))
        })
        .collect();
    entries.sort_by_key(|(v, _)| ordering_key(*v));

    let heading = match prev {
        Some(p) => format!("## Upgrading from v{p}"),
        None => "## Upgrading".to_string(),
    };
    if entries.is_empty() {
        return format!("{heading}\n\nNo breaking changes.\n");
    }
    let mut out = format!("{heading}\n\n");
    for (v, note) in entries {
        out.push_str(&format!("- **v{v}** — {note}\n"));
    }
    out.push_str(
        "\nThe full history is in the \
         [upgrading page](https://bynk-lang.org/book/reference/upgrading/).\n",
    );
    out
}

/// Consistency problems on the upgrading page (empty means consistent).
///
/// The stamp writes the page, so these only fire on a hand edit or a stamp bug:
/// the marker is missing, a `## v…` heading does not parse, versions are not
/// strictly newest-first, an entry has no note, or a version has no row in the
/// changelog (`changelog`, the changelog page's text) — an entry for an
/// increment that never shipped.
pub fn check_page(page: &str, changelog: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(at) = page.find(ENTRIES_MARKER) else {
        return vec![format!("{UPGRADING_PAGE}: missing `{ENTRIES_MARKER}`")];
    };
    let headings = page[at..]
        .lines()
        .filter(|l| l.starts_with("## "))
        .collect::<Vec<_>>();
    let entries = parse_entries(page);
    if headings.len() != entries.len() {
        problems.push(format!(
            "{UPGRADING_PAGE}: {} heading(s) but {} parse as `## vX.Y.Z`",
            headings.len(),
            entries.len()
        ));
    }
    for pair in entries.windows(2) {
        if ordering_key(pair[0].0) <= ordering_key(pair[1].0) {
            problems.push(format!(
                "{UPGRADING_PAGE}: v{} must come before v{} (newest first, no duplicates)",
                pair[1].0, pair[0].0
            ));
        }
    }
    for (version, note) in &entries {
        if note.is_empty() {
            problems.push(format!("{UPGRADING_PAGE}: v{version} has no note"));
        }
        if !changelog.contains(&format!("**v{version}**")) {
            problems.push(format!(
                "{UPGRADING_PAGE}: v{version} has no row in the changelog"
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    const PAGE: &str = "# Upgrading\n\nIntro.\n\n<!-- stamp:breaking-entries -->\n\n## v0.3.0\n\nThree breaks `a`.\n\n## v0.2.1\n\nTwo-one breaks `b`.\n\n## v0.1.0\n\nOne breaks `c`.\n\n";

    #[test]
    fn entries_round_trip_newest_first() {
        let es = parse_entries(PAGE);
        let got: Vec<String> = es.iter().map(|(v, _)| v.to_string()).collect();
        assert_eq!(got, ["0.3.0", "0.2.1", "0.1.0"]);
        assert_eq!(es[1].1, "Two-one breaks `b`.");
    }

    #[test]
    fn insertion_puts_new_entries_above_old_ones() {
        let new = vec![upgrading_entry(v("0.4.0"), "Four breaks `d`.")];
        let out = insert_upgrading_entries(PAGE, &new).unwrap();
        let es = parse_entries(&out);
        assert_eq!(es.len(), 4);
        assert_eq!(es[0].0, v("0.4.0"));
        assert!(out.starts_with("# Upgrading\n\nIntro."));
    }

    #[test]
    fn insertion_needs_the_marker() {
        assert!(insert_upgrading_entries("# no marker\n", &[]).is_err());
    }

    #[test]
    fn a_release_tag_may_carry_a_prerelease_suffix() {
        assert_eq!(parse_release_tag("v0.16.0"), Some(v("0.16.0")));
        assert_eq!(parse_release_tag("v0.16.0-rc1"), Some(v("0.16.0")));
        assert_eq!(parse_release_tag("0.16.0"), None);
        assert_eq!(parse_release_tag("vjunk"), None);
    }

    #[test]
    fn previous_tag_skips_prereleases_and_later_tags() {
        let tags: Vec<String> = ["v0.1.0", "v0.2.1", "v0.3.0-rc1", "v0.3.0", "v0.4.0", "junk"]
            .map(String::from)
            .to_vec();
        assert_eq!(previous_tag(&tags, v("0.3.0")), Some(v("0.2.1")));
        assert_eq!(previous_tag(&tags, v("0.1.0")), None);
        // Numeric, not lexical, ordering.
        let tags = vec!["v0.9.0".to_string(), "v0.10.0".to_string()];
        assert_eq!(previous_tag(&tags, v("0.11.0")), Some(v("0.10.0")));
    }

    #[test]
    fn section_covers_exactly_the_versions_after_prev_up_to_tag() {
        let s = release_section(PAGE, Some(v("0.1.0")), v("0.3.0"));
        assert!(s.starts_with("## Upgrading from v0.1.0\n"));
        assert!(s.contains("**v0.2.1** — Two-one breaks `b`."));
        assert!(s.contains("**v0.3.0** — Three breaks `a`."));
        assert!(!s.contains("One breaks"));
        // Oldest first.
        assert!(s.find("v0.2.1").unwrap() < s.find("v0.3.0").unwrap());
        // A version beyond the tag is not in this release.
        let s = release_section(PAGE, Some(v("0.1.0")), v("0.2.1"));
        assert!(!s.contains("Three breaks"));
    }

    #[test]
    fn section_says_so_when_nothing_breaks() {
        let s = release_section(PAGE, Some(v("0.3.0")), v("0.3.0"));
        assert!(s.contains("No breaking changes."));
        let s = release_section("# no entries\n", None, v("0.3.0"));
        assert!(s.starts_with("## Upgrading\n"));
    }

    const LOG: &str = "| **v0.3.0** | x |\n| **v0.2.1** | x |\n| **v0.1.0** | x |\n";

    #[test]
    fn the_sample_page_is_consistent() {
        assert_eq!(check_page(PAGE, LOG), Vec::<String>::new());
    }

    #[test]
    fn the_gate_flags_each_kind_of_drift() {
        assert!(check_page("# none\n", LOG)[0].contains("missing"));
        let swapped = PAGE.replace("## v0.3.0", "## v0.0.9");
        assert!(
            check_page(&swapped, LOG)
                .iter()
                .any(|p| p.contains("newest first"))
        );
        let dup = PAGE.replace("## v0.2.1", "## v0.3.0");
        assert!(
            check_page(&dup, LOG)
                .iter()
                .any(|p| p.contains("newest first"))
        );
        let junk = PAGE.replace("## v0.2.1", "## v0.2");
        assert!(check_page(&junk, LOG).iter().any(|p| p.contains("parse")));
        let empty = format!("{PAGE}## v0.0.1\n");
        assert!(
            check_page(&empty, LOG)
                .iter()
                .any(|p| p.contains("no note"))
        );
        assert!(check_page(PAGE, "").iter().any(|p| p.contains("no row")));
    }
}
