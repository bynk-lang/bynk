//! End-to-end `stamp` on a fixture tree — the whole apply flow (changelog rows,
//! ADR files + index rows, the rule ledger, pending-file deletion, the injected
//! version bump) without running the real, side-effect-heavy `bump-version.sh`.

use std::fs;
use std::path::{Path, PathBuf};
use xtask::stamp::{self, Version};

/// A throwaway repo fixture. Named per the calling test so parallel runs don't
/// collide; removed and recreated on construction.
fn fixture(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("xtask-stamp-{tag}"));
    let _ = fs::remove_dir_all(&root);
    let write = |rel: &str, body: &str| {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    };

    write("Cargo.toml", "[workspace.package]\nversion = \"0.185.0\"\n");
    write(
        "site/src/content/docs/book/reference/changelog.md",
        "# Changelog\n\n## Recent increments\n\n| Version | Highlights |\n|---|---|\n| **v0.185.0** | Old thing |\n",
    );
    write(
        "design/decisions/README.md",
        "# Decision records\n\n| # | Decision | Status |\n|---|---|---|\n| [0205](0205-old.md) | **Old** — old | Accepted (v0.185) |\n",
    );
    write("design/decisions/0205-old.md", "# 0205 — Old\n");
    fs::create_dir_all(root.join("design/pending")).unwrap();
    root
}

fn bumper(root: &Path) -> impl Fn(Version) -> std::io::Result<()> + '_ {
    move |v: Version| {
        let cargo = root.join("Cargo.toml");
        let src = fs::read_to_string(&cargo)?;
        let out = src
            .lines()
            .map(|l| {
                if l.starts_with("version = \"") {
                    format!("version = \"{v}\"")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&cargo, out + "\n")
    }
}

#[test]
fn stamp_one_increment_with_an_adr() {
    let root = fixture("one");
    fs::write(
        root.join("design/pending/feat-thing.md"),
        "---\nlevel: minor\nchangelog: Add the thing\n---\n\n## ADR: the-thing\ntitle: The thing is added\nsummary: How the thing works\n\n**Decision.** Add it.\n",
    )
    .unwrap();

    let plan = stamp::plan(&root).expect("plan");
    assert_eq!(
        plan.final_version(),
        Version {
            major: 0,
            minor: 186,
            patch: 0
        }
    );
    assert_eq!(plan.first_adr_number, 206);

    stamp::apply(&root, &plan, None, bumper(&root)).expect("apply");

    // Changelog: new row on top.
    let changelog =
        fs::read_to_string(root.join("site/src/content/docs/book/reference/changelog.md")).unwrap();
    let sep = changelog.find("|---|---|").unwrap();
    let after = &changelog[sep..];
    assert!(
        after.contains("| **v0.186.0** | Add the thing |"),
        "changelog row missing:\n{changelog}"
    );
    assert!(
        after.find("v0.186.0").unwrap() < after.find("v0.185.0").unwrap(),
        "new row must sit above the old one"
    );

    // ADR file 0206 written with title/status/body (MINOR-only version string).
    let adr = fs::read_to_string(root.join("design/decisions/0206-the-thing.md")).unwrap();
    assert!(adr.starts_with("# 0206 — The thing is added\n"));
    assert!(adr.contains("- **Status:** Accepted (v0.186)"));
    assert!(adr.contains("**Decision.** Add it."));

    // Index row for 0206, using the explicit summary.
    let readme = fs::read_to_string(root.join("design/decisions/README.md")).unwrap();
    assert!(readme.contains(
        "| [0206](0206-the-thing.md) | **The thing is added** (v0.186) — How the thing works | Accepted (v0.186) |"
    ));

    // Pending file consumed; manifests bumped.
    assert!(!root.join("design/pending/feat-thing.md").exists());
    assert!(
        fs::read_to_string(root.join("Cargo.toml"))
            .unwrap()
            .contains("version = \"0.186.0\"")
    );

    // Idempotent: a re-run finds nothing to do.
    let replan = stamp::plan(&root).expect("replan");
    assert!(replan.is_empty());
}

#[test]
fn stamp_two_increments_get_sequential_versions() {
    let root = fixture("two");
    fs::write(
        root.join("design/pending/a-first.md"),
        "---\nlevel: minor\nchangelog: First\n---\n",
    )
    .unwrap();
    fs::write(
        root.join("design/pending/b-second.md"),
        "---\nlevel: patch\nchangelog: Second\n---\n",
    )
    .unwrap();

    let plan = stamp::plan(&root).expect("plan");
    // Filename order: a-first (minor -> 0.186.0), then b-second (patch -> 0.186.1).
    assert_eq!(
        plan.increments
            .iter()
            .map(|s| s.version.to_string())
            .collect::<Vec<_>>(),
        vec!["0.186.0", "0.186.1"]
    );
    assert_eq!(
        plan.final_version(),
        Version {
            major: 0,
            minor: 186,
            patch: 1
        }
    );

    stamp::apply(&root, &plan, None, bumper(&root)).expect("apply");

    let changelog =
        fs::read_to_string(root.join("site/src/content/docs/book/reference/changelog.md")).unwrap();
    // Newest (0.186.1) on top, then 0.186.0, then the pre-existing 0.185.0.
    let pos = |v: &str| changelog.find(v).unwrap();
    assert!(pos("v0.186.1") < pos("v0.186.0"));
    assert!(pos("v0.186.0") < pos("v0.185.0"));
    assert!(
        fs::read_to_string(root.join("Cargo.toml"))
            .unwrap()
            .contains("version = \"0.186.1\"")
    );
}

#[test]
fn two_adrs_across_two_increments_number_sequentially() {
    let root = fixture("twoadr");
    fs::write(
        root.join("design/pending/a-first.md"),
        "---\nlevel: minor\nchangelog: First\n---\n\n## ADR: alpha\ntitle: Alpha\n\nBody a.\n",
    )
    .unwrap();
    fs::write(
        root.join("design/pending/b-second.md"),
        "---\nlevel: patch\nchangelog: Second\n---\n\n## ADR: beta\ntitle: Beta\n\nBody b.\n",
    )
    .unwrap();

    let plan = stamp::plan(&root).expect("plan");
    assert_eq!(plan.first_adr_number, 206);
    stamp::apply(&root, &plan, None, bumper(&root)).expect("apply");

    // First increment's ADR is 0206 at its version (0.186); second is 0207 at 0.186.1.
    let alpha = fs::read_to_string(root.join("design/decisions/0206-alpha.md")).unwrap();
    assert!(alpha.starts_with("# 0206 — Alpha\n"));
    assert!(alpha.contains("(v0.186)"));
    let beta = fs::read_to_string(root.join("design/decisions/0207-beta.md")).unwrap();
    assert!(beta.starts_with("# 0207 — Beta\n"));
    assert!(beta.contains("(v0.186.1)"));

    // Both index rows present; newest (0207) above 0206, both above the old 0205.
    let readme = fs::read_to_string(root.join("design/decisions/README.md")).unwrap();
    let pos = |s: &str| readme.find(s).unwrap();
    assert!(pos("0207-beta.md") < pos("0206-alpha.md"));
    assert!(pos("0206-alpha.md") < pos("0205-old.md"));
}

#[test]
fn a_failed_bump_rolls_back_and_leaves_a_clean_retry() {
    let root = fixture("rollback");
    fs::write(
        root.join("design/pending/feat-thing.md"),
        "---\nlevel: minor\nchangelog: Add the thing\n---\n\n## ADR: the-thing\ntitle: The thing is added\n\n**Decision.** Add it.\n",
    )
    .unwrap();

    let changelog_path = root.join("site/src/content/docs/book/reference/changelog.md");
    let readme_path = root.join("design/decisions/README.md");
    let changelog_before = fs::read_to_string(&changelog_path).unwrap();
    let readme_before = fs::read_to_string(&readme_path).unwrap();

    let plan = stamp::plan(&root).expect("plan");
    let err = stamp::apply(&root, &plan, None, |_| {
        Err(std::io::Error::other("bump exploded"))
    })
    .expect_err("apply must surface the bump failure");
    assert!(err.to_string().contains("bump exploded"));

    // Nothing survived the failure: the docs are byte-identical, the ADR was not
    // left on disk, and the pending file is intact.
    assert_eq!(
        fs::read_to_string(&changelog_path).unwrap(),
        changelog_before
    );
    assert_eq!(fs::read_to_string(&readme_path).unwrap(), readme_before);
    assert!(!root.join("design/decisions/0206-the-thing.md").exists());
    assert!(root.join("design/pending/feat-thing.md").exists());

    // The retry recomputes the identical plan and applies cleanly — no
    // duplicated rows, no re-numbered ADR.
    let replan = stamp::plan(&root).expect("replan");
    assert_eq!(replan.first_adr_number, 206);
    stamp::apply(&root, &replan, None, bumper(&root)).expect("retry apply");

    let changelog = fs::read_to_string(&changelog_path).unwrap();
    assert_eq!(
        changelog
            .matches("| **v0.186.0** | Add the thing |")
            .count(),
        1,
        "the changelog row must appear exactly once after the retry"
    );
    assert!(root.join("design/decisions/0206-the-thing.md").exists());
    assert!(!root.join("design/pending/feat-thing.md").exists());
}

#[test]
fn apply_refuses_to_clobber_an_existing_adr() {
    let root = fixture("clobber");
    fs::write(
        root.join("design/pending/feat-thing.md"),
        "---\nlevel: minor\nchangelog: Add the thing\n---\n\n## ADR: the-thing\ntitle: The thing is added\n\n**Decision.** Add it.\n",
    )
    .unwrap();

    let plan = stamp::plan(&root).expect("plan");
    assert_eq!(plan.first_adr_number, 206);

    // Pre-place the ADR file the plan would write, simulating stale debris. The
    // guard must refuse rather than overwrite it and restart the series.
    let target = root.join("design/decisions/0206-the-thing.md");
    fs::write(&target, "# 0206 — pre-existing, do not clobber\n").unwrap();
    let sentinel = fs::read_to_string(&target).unwrap();

    let changelog_path = root.join("site/src/content/docs/book/reference/changelog.md");
    let changelog_before = fs::read_to_string(&changelog_path).unwrap();

    let err = stamp::apply(&root, &plan, None, |_| panic!("bump must not run"))
        .expect_err("apply must refuse the collision");
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);

    // The collision is detected in the staging pass, before any write: the
    // existing ADR is untouched, the changelog unchanged, the pending intact.
    assert_eq!(fs::read_to_string(&target).unwrap(), sentinel);
    assert_eq!(
        fs::read_to_string(&changelog_path).unwrap(),
        changelog_before
    );
    assert!(root.join("design/pending/feat-thing.md").exists());
}

#[test]
fn plan_errors_when_the_decisions_dir_is_missing() {
    let root = fixture("nodecisions");
    fs::write(
        root.join("design/pending/feat-thing.md"),
        "---\nlevel: minor\nchangelog: Add the thing\n---\n",
    )
    .unwrap();
    // A missing decisions dir must be an error, not a silent restart from 0001.
    fs::remove_dir_all(root.join("design/decisions")).unwrap();
    let err = stamp::plan(&root).expect_err("plan must fail when decisions is unreadable");
    assert!(
        err.iter().any(|e| e.contains("design/decisions")),
        "error should name the unreadable directory: {err:?}"
    );
}

#[test]
fn empty_pending_is_a_noop() {
    let root = fixture("empty");
    fs::create_dir_all(root.join("design/pending")).unwrap();
    let plan = stamp::plan(&root).expect("plan");
    assert!(plan.is_empty());
    // apply must not touch anything or fail.
    stamp::apply(&root, &plan, None, |_| {
        panic!("bump must not run for an empty plan")
    })
    .expect("apply");
    assert!(
        fs::read_to_string(root.join("Cargo.toml"))
            .unwrap()
            .contains("version = \"0.185.0\"")
    );
}

// --- The rule ledger (#1001) -----------------------------------------------

#[test]
fn stamp_with_closes_rule_writes_the_rule_ledger() {
    let root = fixture("ledger-new");
    fs::write(
        root.join("design/bynk-greenfield-compiler.md"),
        "**R2.3 — A rule.**\n",
    )
    .unwrap();
    fs::write(
        root.join("design/pending/feat-thing.md"),
        "---\nlevel: patch\nchangelog: Close a rule\ncloses_rule: R2.3\n---\n",
    )
    .unwrap();

    let plan = stamp::plan(&root).expect("plan");
    stamp::apply(&root, &plan, Some(1234), bumper(&root)).expect("apply");

    let ledger = fs::read_to_string(root.join("design/greenfield-status-rules.md")).unwrap();
    assert!(
        ledger.contains("| R2.3 | v0.185.1 | #1234 | Close a rule |"),
        "{ledger}"
    );
}

#[test]
fn stamp_without_a_pr_number_leaves_the_pr_cell_empty() {
    let root = fixture("ledger-no-pr");
    fs::write(
        root.join("design/bynk-greenfield-compiler.md"),
        "**R2.3 — A rule.**\n",
    )
    .unwrap();
    fs::write(
        root.join("design/pending/feat-thing.md"),
        "---\nlevel: patch\nchangelog: Close a rule\ncloses_rule: R2.3\n---\n",
    )
    .unwrap();

    let plan = stamp::plan(&root).expect("plan");
    stamp::apply(&root, &plan, None, bumper(&root)).expect("apply");

    let ledger = fs::read_to_string(root.join("design/greenfield-status-rules.md")).unwrap();
    assert!(
        ledger.contains("| R2.3 | v0.185.1 |  | Close a rule |"),
        "PR cell should be empty, not a placeholder: {ledger}"
    );
}

#[test]
fn an_increment_that_closes_no_rule_never_touches_the_ledger() {
    let root = fixture("ledger-untouched");
    fs::write(
        root.join("design/pending/feat-thing.md"),
        "---\nlevel: patch\nchangelog: An ordinary increment\n---\n",
    )
    .unwrap();
    let plan = stamp::plan(&root).expect("plan");
    stamp::apply(&root, &plan, Some(1), bumper(&root)).expect("apply");
    assert!(
        !root.join("design/greenfield-status-rules.md").exists(),
        "no closes_rule anywhere means the ledger file shouldn't be created at all"
    );
}

/// Regression test for the finding in #1001's review: a single `pr_number`
/// threaded through a *multi*-increment plan would attribute every increment's
/// ledger row to the same PR — wrong for whichever increment didn't actually
/// come from that PR (the scenario: a prior stamp run exhausted its push
/// retries and left its pending file for this run to pick up alongside a new
/// one). The recovered number must be dropped entirely rather than risk a
/// confidently wrong provenance record.
#[test]
fn a_multi_increment_plan_never_attributes_a_single_pr_to_every_row() {
    let root = fixture("ledger-multi-increment");
    fs::write(
        root.join("design/bynk-greenfield-compiler.md"),
        "**R2.3 — A rule.**\n**R2.4 — Another.**\n",
    )
    .unwrap();
    fs::write(
        root.join("design/pending/a-first.md"),
        "---\nlevel: minor\nchangelog: First\ncloses_rule: R2.3\n---\n",
    )
    .unwrap();
    fs::write(
        root.join("design/pending/b-second.md"),
        "---\nlevel: patch\nchangelog: Second\ncloses_rule: R2.4\n---\n",
    )
    .unwrap();

    let plan = stamp::plan(&root).expect("plan");
    assert_eq!(
        plan.increments.len(),
        2,
        "both pending files must be in one plan"
    );
    stamp::apply(&root, &plan, Some(999), bumper(&root)).expect("apply");

    let ledger = fs::read_to_string(root.join("design/greenfield-status-rules.md")).unwrap();
    assert!(
        !ledger.contains("#999"),
        "a multi-increment plan must not attribute the run's PR to any row: {ledger}"
    );
    assert!(
        ledger.contains("| R2.3 | v0.186.0 |  | First |"),
        "{ledger}"
    );
    assert!(
        ledger.contains("| R2.4 | v0.186.1 |  | Second |"),
        "{ledger}"
    );
}

#[test]
fn a_failed_bump_rolls_back_a_newly_created_rule_ledger() {
    let root = fixture("ledger-rollback-new");
    fs::write(
        root.join("design/bynk-greenfield-compiler.md"),
        "**R2.3 — A rule.**\n",
    )
    .unwrap();
    fs::write(
        root.join("design/pending/feat-thing.md"),
        "---\nlevel: patch\nchangelog: Close a rule\ncloses_rule: R2.3\n---\n",
    )
    .unwrap();
    let ledger_path = root.join("design/greenfield-status-rules.md");
    assert!(!ledger_path.exists());

    let plan = stamp::plan(&root).expect("plan");
    let err = stamp::apply(&root, &plan, Some(1), |_| {
        Err(std::io::Error::other("bump exploded"))
    })
    .expect_err("apply must surface the bump failure");
    assert!(err.to_string().contains("bump exploded"));

    // The ledger did not exist before this run; a rolled-back run must not
    // leave a header-only file behind — that would be a different kind of
    // corruption from the changelog/README case (a spurious new file, not a
    // reverted edit), and it's exactly the failure mode the `ledger_existed`
    // tracking exists to prevent.
    assert!(
        !ledger_path.exists(),
        "a newly-created ledger must be removed on rollback, not left with just its header"
    );
}

#[test]
fn a_failed_bump_rolls_back_an_existing_rule_ledger_to_its_prior_content() {
    let root = fixture("ledger-rollback-existing");
    fs::write(
        root.join("design/bynk-greenfield-compiler.md"),
        "**R2.3 — A rule.**\n**R2.4 — Another.**\n",
    )
    .unwrap();
    let ledger_path = root.join("design/greenfield-status-rules.md");
    let prior = "# Rules closed\n\n| Rule | Version | PR | Changelog |\n|---|---|---|---|\n\
                 | R2.4 | v0.180.0 | #1 | Earlier |\n";
    fs::write(&ledger_path, prior).unwrap();
    fs::write(
        root.join("design/pending/feat-thing.md"),
        "---\nlevel: patch\nchangelog: Close a rule\ncloses_rule: R2.3\n---\n",
    )
    .unwrap();

    let plan = stamp::plan(&root).expect("plan");
    let err = stamp::apply(&root, &plan, Some(2), |_| {
        Err(std::io::Error::other("bump exploded"))
    })
    .expect_err("apply must surface the bump failure");
    assert!(err.to_string().contains("bump exploded"));

    assert_eq!(
        fs::read_to_string(&ledger_path).unwrap(),
        prior,
        "rollback must restore exactly the prior content, not just leave it stale-but-close"
    );
}

const UPGRADING_FIXTURE: &str =
    "# Upgrading\n\nIntro.\n\n<!-- stamp:breaking-entries -->\n\n## v0.185.0\n\nOld break.\n\n";

fn upgrading_path(root: &Path) -> PathBuf {
    root.join("site/src/content/docs/book/reference/upgrading.md")
}

#[test]
fn stamp_collects_breaking_notes_into_the_upgrading_page() {
    let root = fixture("breaking");
    fs::write(upgrading_path(&root), UPGRADING_FIXTURE).unwrap();
    fs::write(
        root.join("design/pending/a-break.md"),
        "---\nlevel: minor\nchangelog: Break a\nbreaking: `a` is gone; use `b`\n---\n",
    )
    .unwrap();
    // No `breaking:` — must leave no entry.
    fs::write(
        root.join("design/pending/b-quiet.md"),
        "---\nlevel: patch\nchangelog: Quiet\n---\n",
    )
    .unwrap();
    fs::write(
        root.join("design/pending/c-break.md"),
        "---\nlevel: patch\nchangelog: Break c\nbreaking: `c` renamed\n---\n",
    )
    .unwrap();

    let plan = stamp::plan(&root).expect("plan");
    stamp::apply(&root, &plan, None, bumper(&root)).expect("apply");

    let page = fs::read_to_string(upgrading_path(&root)).unwrap();
    let entries = xtask::release_notes::parse_entries(&page);
    let got: Vec<(String, String)> = entries
        .into_iter()
        .map(|(v, n)| (v.to_string(), n))
        .collect();
    // b-quiet (0.186.1) declared nothing, so it has no entry; newest first.
    let want: Vec<(String, String)> = [
        ("0.186.2", "`c` renamed"),
        ("0.186.0", "`a` is gone; use `b`"),
        ("0.185.0", "Old break."),
    ]
    .iter()
    .map(|(v, n)| (v.to_string(), n.to_string()))
    .collect();
    assert_eq!(got, want);
}

#[test]
fn increments_without_breaking_never_need_the_upgrading_page() {
    // Backward compatibility: a tree with no page, and pending files with no
    // `breaking:`, stamps exactly as before and does not create the page.
    let root = fixture("no-breaking");
    fs::write(
        root.join("design/pending/quiet.md"),
        "---\nlevel: patch\nchangelog: Quiet\n---\n",
    )
    .unwrap();
    let plan = stamp::plan(&root).expect("plan");
    stamp::apply(&root, &plan, None, bumper(&root)).expect("apply");
    assert!(!upgrading_path(&root).exists());
}

#[test]
fn a_failed_bump_rolls_back_the_upgrading_page() {
    let root = fixture("breaking-rollback");
    fs::write(upgrading_path(&root), UPGRADING_FIXTURE).unwrap();
    fs::write(
        root.join("design/pending/a-break.md"),
        "---\nlevel: patch\nchangelog: Break a\nbreaking: `a` is gone\n---\n",
    )
    .unwrap();
    let plan = stamp::plan(&root).expect("plan");
    stamp::apply(&root, &plan, None, |_| Err(std::io::Error::other("boom")))
        .expect_err("bump failure surfaces");
    assert_eq!(
        fs::read_to_string(upgrading_path(&root)).unwrap(),
        UPGRADING_FIXTURE
    );
    assert!(root.join("design/pending/a-break.md").exists());
}

#[test]
fn breaking_without_the_page_fails_before_writing_anything() {
    let root = fixture("breaking-no-page");
    fs::write(
        root.join("design/pending/a-break.md"),
        "---\nlevel: patch\nchangelog: Break a\nbreaking: `a` is gone\n---\n",
    )
    .unwrap();
    let plan = stamp::plan(&root).expect("plan");
    stamp::apply(&root, &plan, None, bumper(&root)).expect_err("page is missing");
    let changelog =
        fs::read_to_string(root.join("site/src/content/docs/book/reference/changelog.md")).unwrap();
    assert!(!changelog.contains("Break a"), "nothing may be written");
    assert!(root.join("design/pending/a-break.md").exists());
}
