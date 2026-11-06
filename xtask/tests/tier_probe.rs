//! C4b (#2940): the tier partition's checks, and the committed tier profiles.
//!
//! The live probe (`cargo xtask tier-probe`) needs nextest and built test
//! binaries, so CI runs it in the `test` job. What runs here, with no build, is
//! the checking logic on small lists (each way it can fail, including the
//! vacuous ones) and the static shape of `.config/nextest.toml`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use xtask::tiers::{check_lower, check_partition, check_profiles, parse_list};

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_clean_partition_passes_and_reports_sizes() {
    let all = set(&["a t1", "a t2", "b t3"]);
    let tiers = [
        ("unit", set(&["a t1"])),
        ("integration", set(&["a t2"])),
        ("system", set(&["b t3"])),
    ];
    let sizes = check_partition(&all, &tiers).unwrap();
    assert_eq!(sizes.iter().map(|(_, n)| n).sum::<usize>(), 3);
}

#[test]
fn a_test_in_no_tier_fails() {
    let all = set(&["a t1", "a t2"]);
    let tiers = [("unit", set(&["a t1"])), ("system", set(&["a t1"]))];
    let err = check_partition(&all, &tiers).unwrap_err().join("\n");
    assert!(err.contains("in no tier: a t2"), "{err}");
    assert!(err.contains("in 2 tiers (unit, system): a t1"), "{err}");
}

#[test]
fn an_empty_tier_or_an_empty_list_fails_instead_of_passing() {
    let all = set(&["a t1"]);
    let err = check_partition(&all, &[("unit", set(&["a t1"])), ("system", set(&[]))])
        .unwrap_err()
        .join("\n");
    assert!(err.contains("tier `system` selects no test"), "{err}");
    let err = check_partition(&set(&[]), &[("unit", set(&[]))])
        .unwrap_err()
        .join("\n");
    assert!(err.contains("full test list is empty"), "{err}");
}

#[test]
fn a_tier_selecting_an_unlisted_test_fails() {
    let err = check_partition(&set(&["a t1"]), &[("unit", set(&["a t1", "ghost t9"]))])
        .unwrap_err()
        .join("\n");
    assert!(err.contains("lacks: ghost t9"), "{err}");
}

#[test]
fn parse_list_reads_nextest_json_and_refuses_anything_else() {
    let json = r#"{"rust-suites":{"bynk-fmt::golden":{"testcases":{"x":{},"y":{}}},"bynk":{"testcases":{"z":{}}}}}"#;
    assert_eq!(
        parse_list(json).unwrap(),
        set(&["bynk-fmt::golden x", "bynk-fmt::golden y", "bynk z"])
    );
    for bad in [
        "",
        "not json",
        "{}",
        r#"{"rust-suites":[]}"#,
        r#"{"rust-suites":{"b":{}}}"#,
    ] {
        assert!(parse_list(bad).is_err(), "must refuse {bad:?}");
    }
}

#[test]
fn the_committed_nextest_config_defines_every_tier_profile() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.config/nextest.toml");
    let text = std::fs::read_to_string(path).expect("read .config/nextest.toml");
    check_profiles(&text).unwrap_or_else(|p| panic!("{}", p.join("\n")));
}

#[test]
fn profile_check_flags_a_missing_inherits_or_filter() {
    let err = check_profiles(
        "[profile.ci-unit]\ndefault-filter = 'all()'\n[profile.ci-integration]\ninherits = 'ci'\n",
    )
    .unwrap_err()
    .join("\n");
    assert!(err.contains("ci-unit] must `inherits"), "{err}");
    assert!(
        err.contains("ci-integration] has no `default-filter`"),
        "{err}"
    );
    assert!(err.contains("ci-system] is missing"), "{err}");
    assert!(err.contains("ci-lower] is missing"), "{err}");
    assert!(check_profiles("garbage = = =").is_err());
}

#[test]
fn the_lower_profile_must_be_exactly_unit_plus_integration() {
    let unit = set(&["a t1"]);
    let integration = set(&["a t2"]);
    assert!(check_lower(&set(&["a t1", "a t2"]), &unit, &integration).is_ok());
    // Drops a test: it would run in no job.
    let err = check_lower(&set(&["a t1"]), &unit, &integration).unwrap_err();
    assert!(
        err[0].contains("omits") && err[0].contains("a t2"),
        "{err:?}"
    );
    // Picks up a system test: it would run in both jobs.
    let err = check_lower(&set(&["a t1", "a t2", "b t3"]), &unit, &integration).unwrap_err();
    assert!(
        err[0].contains("selects") && err[0].contains("b t3"),
        "{err:?}"
    );
}
