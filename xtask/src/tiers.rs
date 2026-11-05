//! The test-tier probe (crate-hygiene track, C4b, #2940).
//!
//! Every test in the workspace belongs to exactly one tier (unit, integration,
//! system), selected by the `default-filter` of the `ci-unit`, `ci-integration`
//! and `ci-system` profiles in `.config/nextest.toml`. [`check_partition`]
//! takes the full test list and each tier's list and fails when a test is in
//! none or in more than one. A tier that selects nothing, or an empty full
//! list, also fails: a probe that parsed nothing must not read as a pass (the
//! lesson of A4b's review).
//!
//! [`probe`] gets the lists from `cargo nextest list`, so the filtersets are
//! evaluated by nextest itself, not re-implemented here. It needs `cargo-nextest`
//! and built test binaries, so CI runs it in the `test` job, where both exist.
//! [`check_profiles`] is the static half (no build, no nextest): the three tier
//! profiles exist, inherit `ci`, and carry a filter.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

/// The tiers, cheapest first: the order C4c runs them in.
pub const TIERS: [&str; 3] = ["unit", "integration", "system"];

/// The tests a profile selects in `cargo nextest list --message-format json`
/// output, one `<binary-id> <test name>` string each. Anything that is not that document
/// is an error, never an empty list.
pub fn parse_list(json: &str) -> Result<BTreeSet<String>, String> {
    let doc: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
    let suites = doc
        .get("rust-suites")
        .and_then(|s| s.as_object())
        .ok_or("no `rust-suites` object: not nextest list output")?;
    let mut out = BTreeSet::new();
    for (binary, suite) in suites {
        let cases = suite
            .get("testcases")
            .and_then(|c| c.as_object())
            .ok_or_else(|| format!("suite `{binary}` has no `testcases` object"))?;
        // `list` reports every test, filtered out or not, and marks the ones the
        // profile's `default-filter` selects. A test with no mark counts as
        // selected, so a format change makes tiers overlap (a loud failure)
        // rather than empty (a quiet pass).
        out.extend(
            cases
                .iter()
                .filter(|(_, case)| {
                    case.pointer("/filter-match/status")
                        .and_then(|v| v.as_str())
                        != Some("mismatch")
                })
                .map(|(name, _)| format!("{binary} {name}")),
        );
    }
    Ok(out)
}

/// Checks that `tiers` partition `all`. Returns each tier's size, or every
/// problem found.
pub fn check_partition(
    all: &BTreeSet<String>,
    tiers: &[(&str, BTreeSet<String>)],
) -> Result<Vec<(String, usize)>, Vec<String>> {
    let mut problems = Vec::new();
    if all.is_empty() {
        problems.push("the full test list is empty: nothing was listed".to_string());
    }
    for (name, set) in tiers {
        if set.is_empty() {
            problems.push(format!("tier `{name}` selects no test"));
        }
    }
    for test in all {
        let in_tiers: Vec<&str> = tiers
            .iter()
            .filter(|(_, set)| set.contains(test))
            .map(|(name, _)| *name)
            .collect();
        match in_tiers.len() {
            1 => {}
            0 => problems.push(format!("in no tier: {test}")),
            _ => problems.push(format!(
                "in {} tiers ({}): {test}",
                in_tiers.len(),
                in_tiers.join(", ")
            )),
        }
    }
    for (name, set) in tiers {
        for test in set.difference(all) {
            problems.push(format!(
                "tier `{name}` selects a test the full list lacks: {test}"
            ));
        }
    }
    if problems.is_empty() {
        Ok(tiers
            .iter()
            .map(|(n, s)| (n.to_string(), s.len()))
            .collect())
    } else {
        Err(problems)
    }
}

/// The static half: each tier profile exists in `nextest_toml`, inherits `ci`
/// and has a non-empty `default-filter`.
pub fn check_profiles(nextest_toml: &str) -> Result<(), Vec<String>> {
    let doc: toml::Table = nextest_toml
        .parse()
        .map_err(|e| vec![format!(".config/nextest.toml does not parse: {e}")])?;
    let profiles = doc
        .get("profile")
        .and_then(|p| p.as_table())
        .ok_or_else(|| vec![".config/nextest.toml has no [profile.*] tables".to_string()])?;
    let mut problems = Vec::new();
    for tier in TIERS {
        let name = format!("ci-{tier}");
        let Some(profile) = profiles.get(&name).and_then(|p| p.as_table()) else {
            problems.push(format!("[profile.{name}] is missing"));
            continue;
        };
        if profile.get("inherits").and_then(|v| v.as_str()) != Some("ci") {
            problems.push(format!("[profile.{name}] must `inherits = \"ci\"`"));
        }
        let filter = profile
            .get("default-filter")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if filter.trim().is_empty() {
            problems.push(format!("[profile.{name}] has no `default-filter`"));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

fn list(root: &Path, profile: &str) -> Result<BTreeSet<String>, String> {
    let out = Command::new("cargo")
        .args([
            "nextest",
            "list",
            "--workspace",
            "--locked",
            "--profile",
            profile,
        ])
        .args(["--message-format", "json"])
        .current_dir(root)
        .output()
        .map_err(|e| {
            format!("cannot run `cargo nextest list` (is cargo-nextest installed?): {e}")
        })?;
    if !out.status.success() {
        return Err(format!(
            "`cargo nextest list --profile {profile}` failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    parse_list(&String::from_utf8_lossy(&out.stdout)).map_err(|e| format!("profile {profile}: {e}"))
}

/// Lists the whole suite (`ci`) and each tier, and checks they partition it.
pub fn probe(root: &Path) -> Result<String, Vec<String>> {
    let all = list(root, "ci").map_err(|e| vec![e])?;
    let mut sets = Vec::new();
    for tier in TIERS {
        sets.push((
            tier,
            list(root, &format!("ci-{tier}")).map_err(|e| vec![e])?,
        ));
    }
    let sizes = check_partition(&all, &sets)?;
    let total: usize = sizes.iter().map(|(_, n)| n).sum();
    let parts: Vec<String> = sizes.iter().map(|(t, n)| format!("{t} {n}")).collect();
    Ok(format!(
        "{total} tests, each in exactly one tier ({})",
        parts.join(", ")
    ))
}
