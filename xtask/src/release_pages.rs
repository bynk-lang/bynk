//! The Book's release pages (web presence track, slice 3, #3868): one page per
//! release tag under [`RELEASES_DIR`], an Unreleased page the stamp writes each
//! increment's row into ([`UNRELEASED_PAGE`]), and an index listing every
//! release ([`INDEX_PAGE`]).
//!
//! `cargo xtask release-page` cuts a release: it turns Unreleased into the page
//! for the workspace's current version, starts an empty Unreleased, and adds
//! the release to the index. The maintainer runs it before tagging; the release
//! workflow refuses a tag whose page doesn't exist.

use crate::stamp::{Version, insert_after_table_separator};
use std::fs;
use std::io;
use std::path::Path;

/// The release pages' directory, from the repository root.
pub const RELEASES_DIR: &str = "site/src/content/docs/reference/releases";
/// The page the stamp writes each increment's changelog row into.
pub const UNRELEASED_PAGE: &str = "site/src/content/docs/reference/releases/unreleased.md";
/// The index of every release.
pub const INDEX_PAGE: &str = "site/src/content/docs/reference/releases/index.md";

/// The anchor of the changelog table on Unreleased and each release page.
pub const ROWS_ANCHOR: &str = "| Version |";
/// The anchor of the release table on the index.
pub const INDEX_ANCHOR: &str = "| Release |";

const TABLE: &str = "| Version | Highlights |\n|---|---|";

/// A release page's file stem, `v0-639-1` for v0.639.1. Astro slugifies a
/// file name into its URL and drops dots, so `v0.639.1.md` would be served at
/// `/v06391/`, which another version could also claim; dashes keep the URL
/// readable and unique. The page's title keeps the dots.
pub fn release_stem(version: Version) -> String {
    format!("v{}-{}-{}", version.major, version.minor, version.patch)
}

/// The release page for `version`, from the repository root.
pub fn release_page(version: Version) -> String {
    format!("{RELEASES_DIR}/{}.md", release_stem(version))
}

/// Every changelog row on every release page, Unreleased included, as one
/// text. What a check that "this version has a changelog row" reads.
pub fn all_rows(root: &Path) -> io::Result<String> {
    let mut text = String::new();
    let mut entries: Vec<_> = fs::read_dir(root.join(RELEASES_DIR))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    entries.sort();
    for path in entries {
        for line in fs::read_to_string(&path)?.lines() {
            if line.starts_with("| **v") {
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    Ok(text)
}

/// The version a changelog row names: `| **v0.639.1** | …` → `0.639.1`.
fn row_version(row: &str) -> Option<&str> {
    let rest = row.strip_prefix("| **v")?;
    Some(&rest[..rest.find("**")?])
}

/// A date as the Book writes it: `2026-11-17` → `17 November 2026`.
pub fn long_date(iso: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let mut parts = iso.splitn(3, '-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    let month = MONTHS.get(m.parse::<usize>().ok()?.checked_sub(1)?)?;
    Some(format!("{} {month} {y}", d.parse::<u32>().ok()?))
}

/// Today's UTC date as `YYYY-MM-DD`, from the system clock.
pub fn today_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    civil_from_days((secs / 86_400) as i64)
}

/// Days since 1970-01-01 → `YYYY-MM-DD` (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The three files a cut writes.
#[derive(Debug, PartialEq)]
pub struct Cut {
    /// The new release page's text.
    pub release: String,
    /// Unreleased, emptied.
    pub unreleased: String,
    /// The index, with the release added at the top of its table.
    pub index: String,
}

/// Cut Unreleased into the release page for `version`, released on `date`
/// (`YYYY-MM-DD`). Refuses an Unreleased with no rows: there is nothing to
/// release.
pub fn cut(unreleased: &str, index: &str, version: Version, date: &str) -> Result<Cut, String> {
    let rows: Vec<&str> = unreleased
        .lines()
        .filter(|l| l.starts_with("| **v"))
        .collect();
    let (Some(newest), Some(oldest)) = (
        rows.first().and_then(|r| row_version(r)),
        rows.last().and_then(|r| row_version(r)),
    ) else {
        return Err(format!(
            "{UNRELEASED_PAGE} has no increments, so there is nothing to release"
        ));
    };
    // The page is named for the workspace version; its rows must end there, or
    // the page would describe a release it isn't. The newest increment's
    // version *is* the workspace version, unless a bump happened without a row.
    if newest != version.to_string() {
        return Err(format!(
            "{UNRELEASED_PAGE}'s newest increment is v{newest}, but the workspace version is \
             v{version}. Cut the release from a commit where they agree."
        ));
    }
    let when = long_date(date).ok_or_else(|| format!("{date:?} is not a YYYY-MM-DD date"))?;
    let span = if oldest == newest {
        format!("one increment, v{newest}.")
    } else {
        format!("the increments from v{oldest} to v{newest}.")
    };
    let release = format!(
        "---\ntitle: v{version}\ndescription: The increments released in Bynk v{version}.\n---\n\
         Released on {when} as **v{version}**, with {span}\n\
         See [Upgrading](/reference/upgrading/) for what to change when you move to it.\n\n\
         {TABLE}\n{}\n",
        rows.join("\n")
    );
    let unreleased = format!(
        "---\ntitle: Unreleased\ndescription: The increments merged since the latest Bynk release.\n---\n\
         The increments merged since **v{version}**, the latest release. They ship in\n\
         the next one. Each increment's row is added here when it merges.\n\n{TABLE}\n"
    );
    let range = if oldest == newest {
        format!("v{newest}")
    } else {
        format!("v{oldest} to v{newest}")
    };
    let index_row = format!(
        "| [v{version}](/reference/releases/{}/) | {when} | {range} | {} |",
        release_stem(version),
        rows.len()
    );
    let index = insert_after_table_separator(index, INDEX_ANCHOR, &[index_row])?;
    Ok(Cut {
        release,
        unreleased,
        index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    const INDEX: &str = "---\ntitle: Releases\n---\nIntro.\n\n\
        | Release | Released | Versions | Increments |\n|---|---|---|---|\n\
        | [v0.1.0](/reference/releases/v0-1-0/) | 1 January 2026 | v0.1.0 | 1 |\n";

    fn unreleased(rows: &[&str]) -> String {
        format!(
            "---\ntitle: Unreleased\n---\nSince v0.1.0.\n\n{TABLE}\n{}\n",
            rows.join("\n")
        )
    }

    #[test]
    fn a_cut_moves_every_row_and_empties_unreleased() {
        let page = unreleased(&["| **v0.2.1** | Second |", "| **v0.2.0** | First |"]);
        let cut = cut(&page, INDEX, v("0.2.1"), "2026-11-22").unwrap();
        assert!(cut.release.contains("title: v0.2.1"));
        assert!(
            cut.release
                .contains("Released on 22 November 2026 as **v0.2.1**")
        );
        assert!(cut.release.contains("from v0.2.0 to v0.2.1"));
        assert!(
            cut.release
                .contains("| **v0.2.1** | Second |\n| **v0.2.0** | First |")
        );
        assert!(!cut.unreleased.contains("| **v"), "{}", cut.unreleased);
        assert!(cut.unreleased.contains("since **v0.2.1**"));
        assert!(cut.unreleased.contains(TABLE));
    }

    #[test]
    fn a_cut_adds_the_release_to_the_top_of_the_index() {
        let page = unreleased(&["| **v0.2.0** | Only |"]);
        let cut = cut(&page, INDEX, v("0.2.0"), "2026-11-22").unwrap();
        let new = cut.index.find("[v0.2.0]").unwrap();
        let old = cut.index.find("[v0.1.0]").unwrap();
        assert!(new < old, "{}", cut.index);
        assert!(
            cut.index.contains(
                "| [v0.2.0](/reference/releases/v0-2-0/) | 22 November 2026 | v0.2.0 | 1 |"
            )
        );
        assert_eq!(
            release_page(v("0.2.0")),
            format!("{RELEASES_DIR}/v0-2-0.md")
        );
        assert!(cut.release.contains("one increment, v0.2.0."));
    }

    #[test]
    fn rows_that_end_at_another_version_are_refused() {
        let page = unreleased(&["| **v0.2.0** | Only |"]);
        let err = cut(&page, INDEX, v("0.2.1"), "2026-11-22").unwrap_err();
        assert!(err.contains("newest increment is v0.2.0"), "{err}");
    }

    #[test]
    fn an_empty_unreleased_is_refused() {
        let err = cut(&unreleased(&[]), INDEX, v("0.2.0"), "2026-11-22").unwrap_err();
        assert!(err.contains("nothing to release"), "{err}");
    }

    #[test]
    fn an_index_without_its_table_is_refused() {
        let page = unreleased(&["| **v0.2.0** | Only |"]);
        assert!(cut(&page, "no table here", v("0.2.0"), "2026-11-22").is_err());
    }

    #[test]
    fn dates_read_the_british_way() {
        assert_eq!(long_date("2026-11-07").as_deref(), Some("7 November 2026"));
        assert_eq!(long_date("2026-13-01"), None);
        assert_eq!(civil_from_days(0), "1970-01-01");
        assert_eq!(civil_from_days(20_779), "2026-11-22");
    }
}
