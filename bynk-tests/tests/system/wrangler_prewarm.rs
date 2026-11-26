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

#[path = "../wrangler/mod.rs"]
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
         install (see bynk-tests/tests/wrangler/mod.rs). If the spec was bumped, \
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

// ---- #4456: the suite never sends Wrangler's usage telemetry ----

/// The value of `WRANGLER_SEND_METRICS` a command is configured with, `None` when it is not
/// set on the command.
fn telemetry_setting(cmd: &std::process::Command) -> Option<String> {
    cmd.get_envs()
        .find(|(key, _)| *key == "WRANGLER_SEND_METRICS")
        .and_then(|(_, value)| value.map(|v| v.to_string_lossy().into_owned()))
}

/// Every way the harness starts a Wrangler carries `WRANGLER_SEND_METRICS=false`: the `npx`
/// command the smokes run, the install that warms the cache, and the spawn of a Wrangler (or of
/// the `bynk dev` that starts them), which a child inherits.
#[test]
fn every_wrangler_the_harness_starts_has_telemetry_off() {
    assert_eq!(
        telemetry_setting(&wrangler::npx_command()).as_deref(),
        Some("false"),
        "the `npx` wrangler command"
    );
    assert_eq!(
        telemetry_setting(&wrangler::warm::provision_command(wrangler::SPEC)).as_deref(),
        Some("false"),
        "the cache warm-up"
    );
    let mut dev = std::process::Command::new("true");
    wrangler::Wrangler::prepare(&mut dev, Path::new("registry"));
    assert_eq!(
        telemetry_setting(&dev).as_deref(),
        Some("false"),
        "a spawned wrangler, or the `bynk dev` that starts them"
    );
}

/// CI sets it for every job, the backstop for a Wrangler the harness did not start itself: it
/// is an entry of the workflow's top-level `env:` block, however that block is ordered or its
/// value quoted.
#[test]
fn ci_turns_wrangler_telemetry_off_for_every_job() {
    let path = workflow("ci.yml");
    let yaml = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let top = yaml
        .split("\njobs:\n")
        .next()
        .expect("a workflow has a header before its jobs");
    let (_, after) = top
        .split_once("\nenv:\n")
        .expect("ci.yml has a top-level `env:` block before its jobs (#4456)");
    // The block is the indented lines that follow `env:`.
    let off = after
        .lines()
        .take_while(|l| l.is_empty() || l.starts_with(' '))
        .map(str::trim)
        .any(|l| l == "WRANGLER_SEND_METRICS: \"false\"" || l == "WRANGLER_SEND_METRICS: false");
    assert!(
        off,
        "ci.yml's top-level `env:` no longer sets `WRANGLER_SEND_METRICS` to false (#4456)"
    );
}

// ---- the cross-process warm-up (1D.3d) ----

use wrangler::warm::{self, Warmed};

/// Set in a child process started by
/// [`parallel_processes_on_a_cold_cache_provision_it_once`]: the directory to
/// warm in.
const CHILD_DIR: &str = "BYNK_TEST_WRANGLER_WARM_DIR";

/// The stand-in install the children run: it records that it ran, then takes
/// long enough that the other children are already waiting on the lock.
fn stand_in_install(dir: &Path) -> bool {
    use std::io::Write;
    let mut runs = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("installs"))
        .expect("open the install log");
    writeln!(runs, "{}", std::process::id()).expect("log the install");
    std::thread::sleep(std::time::Duration::from_millis(400));
    true
}

/// Not a test of its own: the body each child process runs. Ignored, so a
/// normal run skips it; the parent starts it with `--include-ignored`.
#[test]
#[ignore = "run only as a child of parallel_processes_on_a_cold_cache_provision_it_once"]
fn warm_up_child() {
    let Some(dir) = std::env::var_os(CHILD_DIR) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let outcome = warm::warm_in(&dir, warm::SPEC, || stand_in_install(&dir));
    assert!(
        matches!(outcome, Warmed::Now | Warmed::Already),
        "{outcome:?}"
    );
}

/// The race 1D.3d closes: several test processes start at once on a cold
/// cache, and exactly one of them installs. The others wait on the lock, then
/// find the marker.
#[test]
fn parallel_processes_on_a_cold_cache_provision_it_once() {
    const CHILDREN: usize = 6;
    let dir = std::env::temp_dir().join(format!("bynk-wrangler-warm-race-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let exe = std::env::current_exe().expect("the test binary");
    let name = format!(
        "{}::warm_up_child",
        module_path!()
            .split_once("::")
            .map_or(module_path!(), |(_, rest)| rest)
    );
    let children: Vec<_> = (0..CHILDREN)
        .map(|_| {
            std::process::Command::new(&exe)
                .args([&name, "--exact", "--include-ignored", "--test-threads=1"])
                .env(CHILD_DIR, &dir)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("start a child test process")
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().expect("a child finishes");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "a child failed or ran no test:\n{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let installs = std::fs::read_to_string(dir.join("installs")).expect("someone installed");
    assert_eq!(
        installs.lines().count(),
        1,
        "{CHILDREN} processes on a cold cache must install once: {installs}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A marker older than the TTL vouches for nothing: the cache may have been
/// cleared since, so the next process installs again.
#[test]
fn a_stale_marker_is_warmed_again() {
    let dir = std::env::temp_dir().join(format!("bynk-wrangler-warm-stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (_, marker, _) = warm::paths(&dir, warm::SPEC);

    assert_eq!(warm::warm_in(&dir, warm::SPEC, || true), Warmed::Now);
    assert_eq!(
        warm::warm_in(&dir, warm::SPEC, || panic!(
            "a fresh marker installs nothing"
        )),
        Warmed::Already
    );

    let old = std::time::SystemTime::now() - warm::MARKER_TTL - std::time::Duration::from_secs(60);
    std::fs::File::options()
        .write(true)
        .open(&marker)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert_eq!(warm::warm_in(&dir, warm::SPEC, || true), Warmed::Now);

    // A failed install leaves no marker, so a later run tries again.
    std::fs::remove_file(&marker).unwrap();
    assert_eq!(warm::warm_in(&dir, warm::SPEC, || false), Warmed::Failed);
    assert!(!marker.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A process that waited on a failing install gives up at once instead of
/// repeating it: N waiting processes must not serialise N failures. Once the
/// failure is stale, the install is tried again, and a success clears it.
#[test]
fn a_waiting_process_gives_up_at_once_after_a_failed_install() {
    let dir =
        std::env::temp_dir().join(format!("bynk-wrangler-warm-failed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (_, marker, failed) = warm::paths(&dir, warm::SPEC);

    assert_eq!(warm::warm_in(&dir, warm::SPEC, || false), Warmed::Failed);
    assert!(failed.exists() && !marker.exists());
    assert_eq!(
        warm::warm_in(&dir, warm::SPEC, || panic!(
            "a fresh failure is not retried"
        )),
        Warmed::Failed
    );

    let old = std::time::SystemTime::now() - warm::FAILURE_TTL - std::time::Duration::from_secs(60);
    std::fs::File::options()
        .write(true)
        .open(&failed)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert_eq!(warm::warm_in(&dir, warm::SPEC, || true), Warmed::Now);
    assert!(marker.exists() && !failed.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two npm caches on one machine (two users, or two jobs with different
/// homes) get their own lock and markers, so neither trusts the other's.
#[test]
fn each_npm_cache_has_its_own_lock_and_markers() {
    let dir = std::path::Path::new("/tmp");
    let a = warm::paths_for(dir, warm::SPEC, "/home/a");
    let b = warm::paths_for(dir, warm::SPEC, "/home/b");
    assert_ne!(a.0, b.0);
    assert_ne!(a.1, b.1);
    assert_ne!(a.2, b.2);
    assert_eq!(a, warm::paths_for(dir, warm::SPEC, "/home/a"));
}
