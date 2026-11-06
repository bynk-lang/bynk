//! The decision-record index drift guard.
//!
//! `design/decisions/README.md` carries a table indexing every ADR (number,
//! one-line summary, status). The summaries and statuses are human-curated, but
//! *completeness* is mechanical and has drifted before (the table once stopped
//! 17 entries behind the files on disk — an index that silently lied). This
//! test makes completeness a CI contract: every `NNNN-*.md` ADR file MUST be
//! linked from the table, and every table link MUST resolve to a file. Either
//! one-sided change fails.
//!
//! It reads `../design/decisions/**`, which lives OUTSIDE the `rust`/`docs` CI
//! path filters, so a decisions-only PR would skip the main `test` job — the
//! `drift` job in `.github/workflows/ci.yml` runs this guard in exactly that
//! case (the same arrangement as `grammar_reference`).

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn decisions_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../design/decisions")
}

/// `NNNN-something.md` — four leading digits, a hyphen, a `.md` suffix. The
/// `README.md` index itself does not match (no leading digits), so it is
/// excluded for free.
fn is_adr_filename(name: &str) -> bool {
    let bytes = name.as_bytes();
    name.ends_with(".md")
        && bytes.len() > 5
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
}

/// Every ADR file on disk.
fn adr_files() -> BTreeSet<String> {
    fs::read_dir(decisions_dir())
        .expect("read design/decisions")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| is_adr_filename(n))
        .collect()
}

/// Every ADR filename the README links to, via markdown `](target)` spans.
/// Non-ADR links (the spec, sibling READMEs) don't match `is_adr_filename`,
/// so prose links are ignored.
fn indexed_files(readme: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = readme;
    while let Some(open) = rest.find("](") {
        let after = &rest[open + 2..];
        match after.find(')') {
            Some(close) => {
                let target = &after[..close];
                if is_adr_filename(target) {
                    out.insert(target.to_string());
                }
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    out
}

#[test]
fn readme_indexes_every_adr_and_every_link_resolves() {
    let files = adr_files();
    let readme = fs::read_to_string(decisions_dir().join("README.md"))
        .expect("read design/decisions/README.md");
    let indexed = indexed_files(&readme);

    let missing: Vec<_> = files.difference(&indexed).cloned().collect();
    let dangling: Vec<_> = indexed.difference(&files).cloned().collect();

    assert!(
        missing.is_empty() && dangling.is_empty(),
        "design/decisions/README.md index has drifted from the ADR files.\n  \
         ADR files with no table row (add one): {missing:?}\n  \
         table links to no such file (fix or remove): {dangling:?}"
    );
}
