//! Drift guard: every workflow that runs the suite with the workerd gate
//! required must pre-warm the npx cache for the exact spec the smokes run.
//!
//! Why this guard exists rather than a comment: the pre-warm step is the only
//! thing standing between the smokes and the concurrent-`npx`-install race
//! documented in `wrangler/mod.rs`, and its failure mode is silent. A spec bump
//! that updates the tests but not the workflows leaves a step that warms the
//! wrong package — the cache the tests want is still cold, they still race, and
//! the only symptom is an intermittent 180s timeout on a leg that used to be
//! green.

use std::path::{Path, PathBuf};

mod wrangler;

fn workflow(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../.github/workflows")
        .join(name)
}

/// The two workflows that run `--workspace` with `BYNK_REQUIRE_WORKERD` set on
/// Linux, i.e. the two where a failed provisioning is a red build rather than a
/// skip. `release-bootstrap.yml` also runs the suite but never sets the gate, so
/// wrangler there is genuinely optional. It pre-warms too (#1676: under nextest
/// its smokes run as concurrent processes), but a stale spec there costs a
/// skip, not a red build, so it is not pinned here.
const GATED_WORKFLOWS: [&str; 2] = ["ci.yml", "release.yml"];

fn assert_prewarms(path: &Path) {
    let yaml = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let expected = format!("npx -y {} --version", wrangler::SPEC);
    assert!(
        yaml.contains(&expected),
        "{} must pre-warm the npx cache with `{expected}` before running the \
         suite — without it the workerd smokes race on a cold `~/.npm/_npx` \
         install (see bynkc/tests/wrangler/mod.rs). If the spec was bumped, \
         bump it here and in the workflow together.",
        path.display()
    );
}

#[test]
fn gated_workflows_prewarm_the_spec_the_smokes_run() {
    for name in GATED_WORKFLOWS {
        assert_prewarms(&workflow(name));
    }
}

/// The guard above only bites while `BYNK_REQUIRE_WORKERD` is still what makes
/// wrangler mandatory in those workflows. If that gate is ever renamed, the
/// pre-warm assertion above would keep passing against workflows that no longer
/// require wrangler at all — so pin the coupling explicitly.
#[test]
fn gated_workflows_still_require_workerd() {
    for name in GATED_WORKFLOWS {
        let path = workflow(name);
        let yaml = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        assert!(
            yaml.contains("BYNK_REQUIRE_WORKERD"),
            "{} no longer sets BYNK_REQUIRE_WORKERD — either it stopped running \
             the workerd smokes (drop it from GATED_WORKFLOWS) or the gate was \
             renamed (update this guard and wrangler/mod.rs's docs)",
            path.display()
        );
    }
}
