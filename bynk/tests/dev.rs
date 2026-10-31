//! `bynk dev` — the deterministic output surface, pinned by goldens (proposal
//! §5). The `wrangler dev` stream itself (ports, timestamps, reload chatter) is
//! non-deterministic and is *not* goldened; what's pinned here is the part the
//! driver owns and renders itself: the **pre-flight failure report** and the
//! **context-selection** messages. The serve hand-off is covered by the unit
//! tests in `dev.rs` (selection rule, exit-code mapping) and the live
//! validation run, not by asserting wrangler's stdout.
//!
//! Goldens are blessed with `BYNK_BLESS=1 cargo test -p bynk`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk::compiler::{Compiler, Origin, Skew};
use bynk::dev::{self, SelectError};
use bynk::doctor::{self, Capability, Context, DoctorOptions};
use bynk::probe::{Toolbox, Version};

// ---------------------------------------------------------------------------
// Minimal fake toolbox (deterministic, host-independent)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Fake {
    on_path: HashMap<String, PathBuf>,
    project_local: HashMap<String, PathBuf>,
    versions: HashMap<PathBuf, Version>,
}

impl Fake {
    fn path_tool(mut self, tool: &str, path: &str, ver: Version) -> Self {
        let p = PathBuf::from(path);
        self.on_path.insert(tool.into(), p.clone());
        self.versions.insert(p, ver);
        self
    }
    fn local_tool(mut self, tool: &str, path: &str, ver: Version) -> Self {
        let p = PathBuf::from(path);
        self.project_local.insert(tool.into(), p.clone());
        self.versions.insert(p, ver);
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
        false
    }
}

fn v(major: u32, minor: u32, patch: u32) -> Version {
    Version {
        major,
        minor,
        patch,
    }
}

/// A `bynkc` resolved on PATH at a fixed path/version, so the rendered report is
/// stable across version bumps.
fn bynkc_pinned() -> Compiler {
    Compiler {
        path: Some(PathBuf::from("/opt/bynk/bin/bynkc")),
        origin: Some(Origin::Path),
        version: Some(v(9, 9, 9)),
        skew: Some(Skew::Match),
    }
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
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

// ---------------------------------------------------------------------------
// Pre-flight failure output
// ---------------------------------------------------------------------------

#[test]
fn golden_preflight_deploy_missing() {
    // Node present, wrangler absent and not provisionable → the deploy
    // capability fails, so `bynk dev` bails before compiling and prints this.
    let fake = Fake::default().path_tool(
        "node",
        "/usr/bin/node",
        v(bynk_emit::NODE_MAJOR_FLOOR, bynk_emit::NODE_MINOR_FLOOR, 0),
    );
    let ctx = Context {
        project_root: None,
        in_repo: false,
        node_floor: bynk_emit::NODE_FLOOR,
    };
    let opts = DoctorOptions {
        only: Some(Capability::Deploy),
        strict: false,
    };
    let mut report = doctor::diagnose(&fake, &bynkc_pinned(), &ctx, &opts);
    report.driver_version = "9.9.9".to_string(); // pin, survives version bumps

    // Sanity: this environment really does gate `dev`.
    assert!(
        report.exit_nonzero(&opts),
        "deploy must fail with no wrangler"
    );

    bless_or_assert(
        "dev-preflight.txt",
        &dev::preflight_failure_message(&report),
    );
}

// ---------------------------------------------------------------------------
// Context-selection messages (the CLI prints `bynk: {error}`)
// ---------------------------------------------------------------------------

#[test]
fn golden_select_errors() {
    // Only two selection failures remain. "Several contexts" used to be the
    // third: `dev` served one (ADR 0096 D3) and `deploy` shipped one, so more
    // than one was an ambiguity to resolve. Since #552 `dev` serves them all
    // and #601 `deploy` ships them all in order — nothing is ambiguous about a
    // project having several contexts, so the error is gone rather than
    // reworded.
    let errs: Vec<SelectError> = vec![
        // A named context that wasn't built.
        dev::select_contexts(&names(&["api"]), &names(&["nope"]))
            .expect_err("an unknown context must fail"),
        // Nothing was built at all.
        dev::select_contexts(&[], &[]).expect_err("an empty build must fail"),
    ];
    let mut out = String::new();
    for err in errs {
        out.push_str(&format!("bynk: {err}\n"));
    }
    bless_or_assert("dev-select-errors.txt", &out);
}

/// #552: the start-up report is the driver's own deterministic surface — which
/// context answers on which URL — so it is pinned exactly as the selection
/// messages are. The `wrangler dev` streams it precedes remain un-goldened.
#[test]
fn golden_serving_report() {
    let opts = dev::DevOptions {
        inspect: true,
        inspect_port: 9229,
        ..Default::default()
    };
    let mut out = String::new();
    out.push_str("# several contexts, wired\n");
    out.push_str(&dev::serving_report(&dev::allocate(
        &names(&["commerce-orders", "commerce-payment"]),
        None,
        &dev::DevOptions::default(),
    )));
    out.push_str("\n# a --context subset of one: no wiring is claimed\n");
    out.push_str(&dev::serving_report(&dev::allocate(
        &names(&["commerce-payment"]),
        Some(8890),
        &dev::DevOptions::default(),
    )));
    out.push_str("\n# --inspect allocates an inspector port per context\n");
    out.push_str(&dev::serving_report(&dev::allocate(
        &names(&["api", "worker"]),
        Some(8787),
        &opts,
    )));
    bless_or_assert("dev-serving-report.txt", &out);
}

/// #1732: `bynk dev` warns, before serving, about a wrangler older than
/// `bynk_emit::WRANGLER_MIN`, naming the version, the minimum, the pinned date
/// and the fix. A wrangler at the minimum gets no such warning, and neither
/// does one with no installed version to judge (missing here; npx-provisioned
/// is the same case, since `wrangler_below_min` requires an installed wrangler).
#[test]
fn dev_warns_about_a_wrangler_too_old_for_the_compatibility_date() {
    use bynk::probe::{self, DetectOpts};
    let min = Version::parse(bynk_emit::WRANGLER_MIN).expect("WRANGLER_MIN is a version");
    // The newest version strictly below `min`, for any `min` (`X.0.0` included).
    let old = if min.patch > 0 {
        v(min.major, min.minor, min.patch - 1)
    } else if min.minor > 0 {
        v(min.major, min.minor - 1, 999)
    } else {
        v(min.major - 1, 999, 999)
    };
    // As `bynk dev` probes: in the project, so a project-local wrangler wins.
    let detect = |fake: &Fake| {
        probe::detect(
            fake,
            "wrangler",
            DetectOpts {
                project_root: Some(Path::new("/proj")),
                allow_npx: true,
            },
        )
    };

    let notice = dev::wrangler_age_notice(&detect(&Fake::default().path_tool(
        "wrangler",
        "/usr/bin/wrangler",
        old,
    )))
    .expect("an old wrangler is warned about");
    for part in [
        format!("wrangler {old}"),
        bynk_emit::WRANGLER_MIN.to_string(),
        bynk_emit::COMPATIBILITY_DATE.to_string(),
        "npm install -g wrangler@4".to_string(),
    ] {
        assert!(notice.contains(&part), "missing {part:?} in: {notice}");
    }

    assert_eq!(
        dev::wrangler_age_notice(&detect(&Fake::default().path_tool(
            "wrangler",
            "/usr/bin/wrangler",
            min
        ))),
        None
    );
    assert_eq!(dev::wrangler_age_notice(&detect(&Fake::default())), None);

    // The project's own wrangler is the one `bynk dev` runs, so the fix upgrades
    // it in the project; a global install wouldn't change what runs.
    let notice = dev::wrangler_age_notice(&detect(
        &Fake::default()
            .path_tool("wrangler", "/usr/bin/wrangler", min)
            .local_tool("wrangler", "/proj/node_modules/.bin/wrangler", old),
    ))
    .expect("an old project-local wrangler is warned about");
    assert!(
        notice.contains("npm install --save-dev wrangler@4 (in the project)"),
        "{notice}"
    );
}
