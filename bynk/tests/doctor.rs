//! `bynk doctor` — the capability/exit-code matrix, the detection probe, and
//! the pinned `--format short`/`json` goldens.
//!
//! Everything runs against a deterministic [`Fake`] toolbox and hand-built
//! [`Compiler`] states, so the contract is exercised without depending on the
//! host's tools. The goldens are blessed with `BYNK_BLESS=1`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk::compiler::{Compiler, Origin, Skew};
use bynk::doctor::{self, Capability, Context, DoctorOptions, Level};
use bynk::probe::{self, DetectOpts, Provenance, Toolbox, Version};
use bynk::report::{self, Format};

// ---------------------------------------------------------------------------
// Fake toolbox
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Fake {
    on_path: HashMap<String, PathBuf>,
    project_local: HashMap<String, PathBuf>,
    versions: HashMap<PathBuf, Version>,
    npx: bool,
}

impl Fake {
    fn path_tool(mut self, tool: &str, path: &str, ver: Option<Version>) -> Self {
        let p = PathBuf::from(path);
        self.on_path.insert(tool.into(), p.clone());
        if let Some(v) = ver {
            self.versions.insert(p, v);
        }
        self
    }
    fn local_tool(mut self, tool: &str, path: &str, ver: Option<Version>) -> Self {
        let p = PathBuf::from(path);
        self.project_local.insert(tool.into(), p.clone());
        if let Some(v) = ver {
            self.versions.insert(p, v);
        }
        self
    }
    fn with_npx(mut self) -> Self {
        self.npx = true;
        self
    }
}

impl Toolbox for Fake {
    fn on_path(&self, tool: &str) -> Option<PathBuf> {
        self.on_path.get(tool).cloned()
    }
    fn in_dir(&self, dir: &Path, tool: &str) -> Option<PathBuf> {
        // Only resolve project-local tools when asked inside a node_modules/.bin.
        if dir.ends_with(Path::new("node_modules/.bin")) {
            self.project_local.get(tool).cloned()
        } else {
            None
        }
    }
    fn version(&self, path: &Path) -> Option<Version> {
        self.versions.get(path).copied()
    }
    fn npx_available(&self) -> bool {
        self.npx
    }
}

/// #1674: the floor the shipped driver uses, not a copy of it, so these tests
/// model the real floor and a bump can't leave them asserting a stale one.
const FLOOR: u32 = bynk_emit::NODE_MAJOR_FLOOR;
/// P4a (#2602): the minor within it, 22.13.
const FLOOR_MINOR: u32 = bynk_emit::NODE_MINOR_FLOOR;

/// #1732: a `wrangler` exactly at `bynk_emit::WRANGLER_MIN`, the oldest whose
/// `workerd` serves the pinned compatibility date, read from the constant so a
/// review that moves it can't leave these tests on a stale version.
fn wrangler_at_min() -> Version {
    Version::parse(bynk_emit::WRANGLER_MIN).expect("WRANGLER_MIN is a version")
}

/// The newest version strictly below `min`, for any `min` above `0.0.0`
/// (`X.0.0` included), so a review that moves the minimum can't break it.
fn just_below(min: Version) -> Version {
    if min.patch > 0 {
        v(min.major, min.minor, min.patch - 1)
    } else if min.minor > 0 {
        v(min.major, min.minor - 1, 999)
    } else {
        v(min.major - 1, 999, 999)
    }
}

/// A `node` exactly at the floor: the oldest one `doctor` reports `ok`.
fn node_at_floor() -> Version {
    v(FLOOR, FLOOR_MINOR, 0)
}

fn v(major: u32, minor: u32, patch: u32) -> Version {
    Version {
        major,
        minor,
        patch,
    }
}

/// A `bynkc` resolved on PATH, with the given skew vs the driver.
fn bynkc_ok(skew: Skew) -> Compiler {
    Compiler {
        path: Some(PathBuf::from("/opt/bynk/bin/bynkc")),
        origin: Some(Origin::Path),
        version: Some(v(9, 9, 9)),
        skew: Some(skew),
    }
}

/// A `BYNK_BYNKC` override at the given skew. Slice 7: the in-process compiler
/// is always ok, so skew is only meaningful — and only reported — when the user
/// points `bynk` at an external compiler via the override (ADR 0084, amended).
fn bynkc_override(skew: Skew) -> Compiler {
    Compiler {
        path: Some(PathBuf::from("/opt/bynk/bin/bynkc")),
        origin: Some(Origin::Override),
        version: Some(v(9, 9, 9)),
        skew: Some(skew),
    }
}

/// A `BYNK_BYNKC` override that doesn't resolve — the only "broken compiler"
/// state under the amended contract (a missing in-process compiler is impossible).
fn bynkc_override_missing() -> Compiler {
    Compiler {
        path: None,
        origin: Some(Origin::Override),
        version: None,
        skew: None,
    }
}

fn bynkc_missing() -> Compiler {
    Compiler {
        path: None,
        origin: None,
        version: None,
        skew: None,
    }
}

fn ctx(in_repo: bool) -> Context {
    Context {
        project_root: None,
        in_repo,
        node_floor: bynk_emit::NODE_FLOOR,
    }
}

fn bare() -> DoctorOptions {
    DoctorOptions::default()
}

fn cap(report: &doctor::Report, c: Capability) -> &doctor::CapabilityReport {
    report
        .capabilities
        .iter()
        .find(|r| r.capability == c)
        .expect("capability present")
}

// ---------------------------------------------------------------------------
// Detection probe
// ---------------------------------------------------------------------------

#[test]
fn project_local_is_preferred_over_global() {
    let fake = Fake::default()
        .path_tool("tsc", "/usr/bin/tsc", Some(v(5, 0, 0)))
        .local_tool("tsc", "/proj/node_modules/.bin/tsc", Some(v(5, 4, 2)));
    let root = PathBuf::from("/proj");
    let probe = probe::detect(
        &fake,
        "tsc",
        DetectOpts {
            project_root: Some(&root),
            allow_npx: true,
        },
    );
    assert_eq!(
        probe.provenance,
        Provenance::ProjectLocal(PathBuf::from("/proj/node_modules/.bin/tsc"))
    );
    assert_eq!(probe.version, Some(v(5, 4, 2)));
}

#[test]
fn npx_is_provisionable_not_present() {
    let fake = Fake::default().with_npx();
    let probe = probe::detect(
        &fake,
        "wrangler",
        DetectOpts {
            project_root: None,
            allow_npx: true,
        },
    );
    assert!(probe.is_provisionable());
    assert!(!probe.is_present(), "npx must never read as installed");
    assert_eq!(probe.provenance, Provenance::Npx);
}

#[test]
fn absent_with_no_npx_is_missing() {
    let fake = Fake::default();
    let probe = probe::detect(
        &fake,
        "wrangler",
        DetectOpts {
            project_root: None,
            allow_npx: true,
        },
    );
    assert!(probe.is_missing());
}

// ---------------------------------------------------------------------------
// Capability / exit-code matrix
// ---------------------------------------------------------------------------

#[test]
fn bare_compile_only_env_exits_zero_with_test_dev_flagged() {
    // Only bynkc; no node/tsc/tsx/wrangler.
    let fake = Fake::default();
    let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
    assert_eq!(cap(&report, Capability::Compile).level, Level::Ok);
    assert_eq!(cap(&report, Capability::Test).level, Level::Fail);
    assert_eq!(cap(&report, Capability::Deploy).level, Level::Fail);
    // Bare run: only the compile floor is required, so it exits 0.
    assert!(!report.exit_nonzero(&bare()));
}

#[test]
fn everything_present_is_all_green_exit_zero() {
    let fake = Fake::default()
        .path_tool("node", "/usr/bin/node", Some(node_at_floor()))
        .path_tool("tsc", "/usr/bin/tsc", Some(v(5, 4, 2)))
        .path_tool("wrangler", "/usr/bin/wrangler", Some(wrangler_at_min()))
        .path_tool("bynkc-lsp", "/usr/bin/bynkc-lsp", Some(v(9, 9, 9)));
    let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
    assert!(report.is_all_ok());
    assert!(!report.exit_nonzero(&bare()));
}

/// #1732: wrangler is checked against `bynk_emit::WRANGLER_MIN`. Below it, its
/// `workerd` refuses the pinned compatibility date, so `bynk dev` fails: the row
/// warns, says why, and gives a remedy. It still deploys, so `--only deploy`
/// stays exit 0 (only `--strict` turns the warning red). At the minimum, or
/// newer, it's `ok`. An npx-provisioned wrangler can't be versioned without
/// running npx, so it keeps its usual "provisionable" warning, and its remedy
/// mentions the npx cache.
#[test]
fn wrangler_is_checked_against_the_compatibility_date_minimum() {
    let min = wrangler_at_min();
    let date = bynk_emit::COMPATIBILITY_DATE;
    let deploy = |fake: Fake| {
        let fake = fake.path_tool("node", "/usr/bin/node", Some(node_at_floor()));
        let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
        let r = cap(&report, Capability::Deploy).clone();
        let row = r
            .rows
            .iter()
            .find(|row| row.label == "wrangler")
            .expect("a wrangler row")
            .clone();
        (report, r.level, row)
    };

    let below = just_below(min);
    let (report, level, row) =
        deploy(Fake::default().path_tool("wrangler", "/usr/bin/wrangler", Some(below)));
    assert_eq!(level, Level::Warn, "{}", row.detail);
    assert!(
        row.detail.contains(&format!(
            "below {}: `bynk dev` can't serve compatibility date {date}",
            bynk_emit::WRANGLER_MIN
        )),
        "{}",
        row.detail
    );
    assert!(
        row.remedy
            .as_deref()
            .is_some_and(|r| r.starts_with("npm install -g wrangler@4")),
        "{:?}",
        row.remedy
    );
    let only_deploy = DoctorOptions {
        only: Some(Capability::Deploy),
        strict: false,
    };
    assert!(
        !report.exit_nonzero(&only_deploy),
        "an old wrangler still deploys"
    );
    // --strict fails on the warning: in an otherwise all-green environment,
    // the old wrangler alone turns it red, and one at the minimum doesn't.
    let strict = DoctorOptions {
        only: None,
        strict: true,
    };
    let all_green_but = |wrangler: Version| {
        let fake = Fake::default()
            .path_tool("node", "/usr/bin/node", Some(node_at_floor()))
            .path_tool("tsc", "/usr/bin/tsc", Some(v(5, 4, 2)))
            .path_tool("bynkc-lsp", "/usr/bin/bynkc-lsp", Some(v(9, 9, 9)))
            .path_tool("wrangler", "/usr/bin/wrangler", Some(wrangler));
        doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare())
    };
    assert!(
        !all_green_but(min).exit_nonzero(&strict),
        "control: all green"
    );
    assert!(
        all_green_but(below).exit_nonzero(&strict),
        "--strict fails on an old wrangler"
    );

    for ok in [min, v(min.major, min.minor + 1, 0), v(min.major + 1, 0, 0)] {
        let (_, level, row) =
            deploy(Fake::default().path_tool("wrangler", "/usr/bin/wrangler", Some(ok)));
        assert_eq!(level, Level::Ok, "{}", row.detail);
        assert_eq!(row.remedy, None);
    }

    // In a project, the project-local wrangler is the one the driver runs, so it
    // is the one judged, even with a newer one on PATH.
    let fake = Fake::default()
        .path_tool("node", "/usr/bin/node", Some(node_at_floor()))
        .path_tool("wrangler", "/usr/bin/wrangler", Some(min))
        .local_tool("wrangler", "/proj/node_modules/.bin/wrangler", Some(below));
    let in_project = Context {
        project_root: Some(PathBuf::from("/proj")),
        ..ctx(false)
    };
    let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &in_project, &bare());
    assert_eq!(cap(&report, Capability::Deploy).level, Level::Warn);
    let row = cap(&report, Capability::Deploy)
        .rows
        .iter()
        .find(|row| row.label == "wrangler")
        .expect("a wrangler row");
    assert_eq!(
        row.remedy.as_deref(),
        Some("npm install --save-dev wrangler@4 (in the project)"),
        "a project's own wrangler is upgraded in the project"
    );

    let (_, level, row) = deploy(Fake::default().with_npx());
    assert_eq!(level, Level::Warn);
    assert!(
        row.remedy
            .as_deref()
            .is_some_and(|r| r.contains("~/.npm/_npx")),
        "{:?}",
        row.remedy
    );
}

#[test]
fn only_deploy_without_wrangler_exits_nonzero() {
    // No wrangler, no npx → truly cannot deploy.
    let fake = Fake::default().path_tool("node", "/usr/bin/node", Some(node_at_floor()));
    let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
    let opts = DoctorOptions {
        only: Some(Capability::Deploy),
        strict: false,
    };
    assert_eq!(cap(&report, Capability::Deploy).level, Level::Fail);
    assert!(report.exit_nonzero(&opts));
    // …but the same environment, bare, exits 0 (deploy isn't asked about).
    assert!(!report.exit_nonzero(&bare()));
}

#[test]
fn strict_escalates_optional_only_gap() {
    // Everything an end user needs is present; only the optional editor LSP is
    // missing. Bare → 0; --strict → non-zero.
    let fake = Fake::default()
        .path_tool("node", "/usr/bin/node", Some(node_at_floor()))
        .path_tool("tsc", "/usr/bin/tsc", Some(v(5, 4, 2)))
        .path_tool("wrangler", "/usr/bin/wrangler", Some(wrangler_at_min()));
    let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
    assert_eq!(cap(&report, Capability::Editor).level, Level::Fail); // displayed as "note"
    assert!(!report.exit_nonzero(&bare()));
    let strict = DoctorOptions {
        only: None,
        strict: true,
    };
    assert!(report.exit_nonzero(&strict));
}

#[test]
fn in_process_compile_floor_is_always_ok() {
    // Slice 7 (ADR 0084 amended): the compiler is linked in-process, so the
    // compile floor is always satisfiable — a missing *external* bynkc is no
    // longer a failure (the driver doesn't shell it). Compile is ok, bare.
    let fake = Fake::default()
        .path_tool("node", "/usr/bin/node", Some(node_at_floor()))
        .path_tool("tsc", "/usr/bin/tsc", Some(v(5, 4, 2)));
    let report = doctor::diagnose(&fake, &bynkc_missing(), &ctx(false), &bare());
    assert_eq!(cap(&report, Capability::Compile).level, Level::Ok);
}

#[test]
fn broken_override_fails_even_bare() {
    // A `BYNK_BYNKC` override that doesn't resolve is the one broken-compiler
    // state left: the user explicitly asked for an external compiler and it
    // isn't there. Fails even bare.
    let fake = Fake::default()
        .path_tool("node", "/usr/bin/node", Some(node_at_floor()))
        .path_tool("tsc", "/usr/bin/tsc", Some(v(5, 4, 2)));
    let report = doctor::diagnose(&fake, &bynkc_override_missing(), &ctx(false), &bare());
    assert_eq!(cap(&report, Capability::Compile).level, Level::Fail);
    assert!(report.exit_nonzero(&bare()));
}

#[test]
fn skew_is_reported_only_under_override() {
    let fake = Fake::default();

    // Slice 7: skew at a non-override origin is ignored — the in-process
    // compiler can't drift against itself.
    let path_major = doctor::diagnose(&fake, &bynkc_ok(Skew::Major), &ctx(false), &bare());
    assert_eq!(cap(&path_major, Capability::Compile).level, Level::Ok);

    // Under a `BYNK_BYNKC` override, skew is real and reported.
    // Minor: bare exits 0 (warn), --strict fails.
    let minor = doctor::diagnose(&fake, &bynkc_override(Skew::Minor), &ctx(false), &bare());
    assert_eq!(cap(&minor, Capability::Compile).level, Level::Warn);
    assert!(!minor.exit_nonzero(&bare()));
    assert!(minor.exit_nonzero(&DoctorOptions {
        only: None,
        strict: true
    }));

    // Major: a contract mismatch — fails even bare.
    let major = doctor::diagnose(&fake, &bynkc_override(Skew::Major), &ctx(false), &bare());
    assert_eq!(cap(&major, Capability::Compile).level, Level::Fail);
    assert!(major.exit_nonzero(&bare()));
}

#[test]
fn npx_provisionable_runner_is_warn_not_ok() {
    // tsc/tsx only via npx → test capability is available but flagged (never a
    // green "ok"), and --strict escalates it.
    let fake = Fake::default()
        .path_tool("node", "/usr/bin/node", Some(node_at_floor()))
        .with_npx();
    let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
    assert_eq!(cap(&report, Capability::Test).level, Level::Warn);
    assert!(!report.exit_nonzero(&bare()));
    assert!(report.exit_nonzero(&DoctorOptions {
        only: Some(Capability::Test),
        strict: true,
    }));
}

#[test]
fn only_filter_scopes_probes() {
    // `--only test` must not probe Cloudflare/editor at all.
    let fake = Fake::default().path_tool("node", "/usr/bin/node", Some(node_at_floor()));
    let opts = DoctorOptions {
        only: Some(Capability::Test),
        strict: false,
    };
    let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &opts);
    let kinds: Vec<Capability> = report.capabilities.iter().map(|c| c.capability).collect();
    assert_eq!(kinds, vec![Capability::Compile, Capability::Test]);
}

// ---------------------------------------------------------------------------
// Pinned output goldens (--format short / json)
// ---------------------------------------------------------------------------

/// #1674: a `node` below the floor is reported, as a warning with the floor in
/// its detail, on every capability that needs Node — not passed as `ok`.
#[test]
fn node_below_the_floor_warns() {
    let fake = Fake::default()
        .path_tool("node", "/usr/bin/node", Some(v(FLOOR - 2, 11, 0)))
        .path_tool("tsc", "/usr/bin/tsc", Some(v(5, 4, 2)))
        .path_tool("wrangler", "/usr/bin/wrangler", Some(wrangler_at_min()));
    let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
    for c in [Capability::Test, Capability::Deploy] {
        let r = cap(&report, c);
        assert_eq!(r.level, Level::Warn, "{c:?} with a below-floor node");
        let detail = format!("v{}.11.0 below floor (≥ {FLOOR}.{FLOOR_MINOR})", FLOOR - 2);
        assert!(
            r.rows.iter().any(|row| row.detail == detail),
            "{c:?} names the floor: {:?}",
            r.rows
        );
    }
    // P4a (#2602): the floor has a minor, so a 22.x before 22.13 warns too.
    let fake = Fake::default()
        .path_tool("node", "/usr/bin/node", Some(v(FLOOR, FLOOR_MINOR - 1, 9)))
        .path_tool("tsc", "/usr/bin/tsc", Some(v(5, 4, 2)))
        .path_tool("wrangler", "/usr/bin/wrangler", Some(wrangler_at_min()));
    let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
    assert_eq!(cap(&report, Capability::Test).level, Level::Warn);
    // At the floor, the same environment is all green.
    let fake = Fake::default()
        .path_tool("node", "/usr/bin/node", Some(node_at_floor()))
        .path_tool("tsc", "/usr/bin/tsc", Some(v(5, 4, 2)))
        .path_tool("wrangler", "/usr/bin/wrangler", Some(wrangler_at_min()));
    let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
    assert_eq!(cap(&report, Capability::Test).level, Level::Ok);
    assert_eq!(cap(&report, Capability::Deploy).level, Level::Ok);
}

/// #1672: `tsc` is checked against the verified TypeScript majors. Below the
/// floor it warns. Above the tested major it is reported as untested but stays
/// `ok`, so `doctor --strict` does not go red the day a new major ships. Both
/// verified majors (and everything between) are plain `ok`. `tsx` type-checks
/// nothing, so it carries no TypeScript version to check.
#[test]
fn tsc_is_checked_against_the_verified_majors() {
    let floor = bynk_emit::TYPESCRIPT_MAJOR_FLOOR;
    let tested = bynk_emit::TYPESCRIPT_MAJOR_TESTED;
    let test_row = |tsc: Version| {
        let fake = Fake::default()
            .path_tool("node", "/usr/bin/node", Some(node_at_floor()))
            .path_tool("tsc", "/usr/bin/tsc", Some(tsc));
        let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
        let r = cap(&report, Capability::Test);
        let row = r
            .rows
            .iter()
            .find(|row| row.label == "tsc | tsx")
            .expect("a runner row")
            .clone();
        (r.level, row)
    };

    let (level, row) = test_row(v(floor - 1, 9, 0));
    assert_eq!(level, Level::Warn);
    assert!(
        row.detail.ends_with(&format!("below floor (≥ {floor})")),
        "{}",
        row.detail
    );
    assert!(
        row.remedy
            .as_deref()
            .is_some_and(|r| r.starts_with(&format!("npm install -g typescript@{tested}"))),
        "{:?}",
        row.remedy
    );

    let (level, row) = test_row(v(tested + 1, 0, 0));
    assert_eq!(level, Level::Ok, "untested is a report, not a fault");
    assert!(
        row.detail
            .ends_with(&format!("untested (verified up to {tested})")),
        "{}",
        row.detail
    );
    assert_eq!(row.remedy, None, "no advice to downgrade a working tsc");

    for ok in [v(floor, 0, 0), v(tested, 0, 2)] {
        let (level, row) = test_row(ok);
        assert_eq!(level, Level::Ok, "{}", row.detail);
        assert!(!row.detail.contains("untested"), "{}", row.detail);
    }

    // `tsc` is the runner whenever it is present (`bynkc test` prefers it), so a
    // below-floor `tsc` still warns even when a healthy `tsx` is installed too.
    let fake = Fake::default()
        .path_tool("node", "/usr/bin/node", Some(node_at_floor()))
        .path_tool("tsc", "/usr/bin/tsc", Some(v(floor - 1, 9, 0)))
        .path_tool("tsx", "/usr/bin/tsx", Some(v(4, 19, 0)));
    let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
    assert_eq!(cap(&report, Capability::Test).level, Level::Warn);

    // `tsx` alone: present, no TypeScript version to judge.
    let fake = Fake::default()
        .path_tool("node", "/usr/bin/node", Some(node_at_floor()))
        .path_tool("tsx", "/usr/bin/tsx", Some(v(4, 19, 0)));
    let report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
    assert_eq!(cap(&report, Capability::Test).level, Level::Ok);
}

/// A fixed, mixed environment: compile ok, test ok, deploy provisionable-only
/// (warn), editor missing (note). Driver/compiler versions are pinned to a
/// sentinel so the goldens survive version bumps.
fn golden_report() -> doctor::Report {
    let fake = Fake::default()
        .path_tool("node", "/usr/bin/node", Some(node_at_floor()))
        .path_tool("tsc", "/usr/bin/tsc", Some(v(5, 4, 2)))
        .with_npx(); // wrangler provisionable, not installed
    let mut report = doctor::diagnose(&fake, &bynkc_ok(Skew::Match), &ctx(false), &bare());
    report.driver_version = "9.9.9".to_string();
    report
}

fn bless_or_assert(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("BYNK_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden {}; regenerate with BYNK_BLESS=1 cargo test -p bynk",
            path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "golden {name} drifted; re-bless with BYNK_BLESS=1 cargo test -p bynk"
    );
}

#[test]
fn golden_short() {
    bless_or_assert(
        "doctor.short",
        &report::render(&golden_report(), Format::Short),
    );
}

#[test]
fn golden_json() {
    bless_or_assert(
        "doctor.json",
        &report::render(&golden_report(), Format::Json),
    );
}

#[test]
fn human_smoke() {
    // The human table is smoke-tested only (not pinned): it must mention each
    // capability and not leak an absolute path into a capability row.
    let out = report::render(&golden_report(), Format::Human);
    for token in ["compile", "test", "deploy", "editor"] {
        assert!(out.contains(token), "human output missing {token}");
    }
    assert!(out.contains("bynk doctor"));
}
