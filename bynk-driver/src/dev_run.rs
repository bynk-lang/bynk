//! #2138 (end-to-end testing track §3.5): `bynk test --against dev`, the
//! `system` suites run against a local `bynk dev` the runner starts itself.
//!
//! For each distinct `env` the `system` cases run with (one group), this:
//!
//! 1. starts `bynk dev --ready-json` (local `wrangler dev`/workerd only, never
//!    `-- --remote`) with the group's `env` and a fixed test value for every
//!    other secret the Workers declare, passed as `-- --var NAME:VALUE`;
//! 2. waits for the readiness file, whose URLs must all be loopback;
//! 3. runs the group's cases against those URLs (this binary again, with
//!    [`crate::remote::DEV_URLS_ENV`] and [`crate::remote::DEV_ENV_ENV`]),
//!    signing with the same secret values;
//! 4. stops `bynk dev` by its own pid (it stops its wranglers and their
//!    `workerd`s), then its process group if anything is left.
//!
//! Groups run in turn; their results are reported together.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use bynk_emit::project::{BuildTarget, RemoteTests};

use crate::remote::{DEV, DEV_ENV_ENV, DEV_SECRETS_ENV, DEV_URLS_ENV, is_loopback_http};
use crate::test_json::TestRun;

/// The value every declared secret the case doesn't set takes, both in the
/// Workers (`-- --var`) and in the runner that signs for them.
pub const TEST_SECRET: &str = "__bynk_test_secret";

/// How long to wait for a `bynk dev` to answer before giving up.
const READY_TIMEOUT: Duration = Duration::from_secs(180);

/// How long a stopped `bynk dev` gets to stop its Workers before its process
/// group is killed.
const STOP_GRACE: Duration = Duration::from_secs(30);

/// What the orchestration needs from the `test` invocation.
#[derive(Debug, Clone)]
pub struct DevRunArgs {
    pub input: PathBuf,
    pub output: PathBuf,
    pub json: bool,
    pub seed: Option<String>,
    pub case: Option<String>,
    pub skip_unsupported: bool,
    /// `--settle`, forwarded to each group's run.
    pub settle: Option<String>,
    /// The `run.id` every group's run shares.
    pub run_id: String,
    /// The `bynk` binary that starts `bynk dev`.
    pub bynk: PathBuf,
}

/// Run every `system` suite against a local `bynk dev`, one per distinct
/// `env`.
pub fn run(program: &str, args: &DevRunArgs) -> ExitCode {
    let fail = |message: String| {
        if args.json {
            print!("{}", TestRun::runtime_error(message, None).render());
        } else {
            eprintln!("{program} test: {message}");
        }
        ExitCode::FAILURE
    };
    let plan = match plan(args) {
        Ok(plan) => plan,
        Err(Planned::Failed(code)) => return code,
        Err(Planned::Message(m)) => return fail(m),
    };
    if plan.groups.is_empty() {
        if args.json {
            print!(
                "{}",
                TestRun::empty()
                    .with_remote(format!("remote:{DEV}"), &[])
                    .render()
            );
        } else {
            eprintln!("{program} test: no `system` suites to run against `bynk dev`");
        }
        return ExitCode::SUCCESS;
    }
    let mut docs: Vec<serde_json::Value> = Vec::new();
    let mut all_ok = true;
    for (i, group) in plan.groups.iter().enumerate() {
        let vars = dev_vars(group, &plan.secrets);
        if !args.json {
            eprintln!(
                "{program} test: starting `bynk dev`{}",
                if group.is_empty() {
                    String::new()
                } else {
                    format!(
                        " with env {}",
                        group
                            .iter()
                            .map(|(k, v)| format!("{k}={v:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            );
        }
        let ready = args.output.join(format!(".bynk-dev-ready-{i}.json"));
        let log = args.output.join(format!(".bynk-dev-{i}.log"));
        let mut dev = match Dev::start(&args.bynk, &args.input, &ready, &log, &vars) {
            Ok(dev) => dev,
            Err(e) => return fail(format!("could not start `bynk dev`: {e}")),
        };
        let urls = match dev.wait_ready(&ready) {
            Ok(urls) => urls,
            Err(e) => {
                drop(dev);
                return fail(format!("{e} (its output is in {})", log.display()));
            }
        };
        let outcome = run_group(args, group, &urls, &vars);
        dev.stop();
        match outcome {
            Ok((ok, doc)) => {
                all_ok &= ok;
                docs.extend(doc);
            }
            Err(e) => return fail(e),
        }
    }
    if args.json {
        print!("{}", merge(&docs).render());
    }
    if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// The groups to run, and the secrets the Workers declare.
struct Plan {
    groups: Vec<BTreeMap<String, String>>,
    secrets: BTreeSet<String>,
}

enum Planned {
    /// A compile failure, already reported.
    Failed(ExitCode),
    Message(String),
}

/// Compile the project as a `--against dev` build once: refuse what a real
/// runtime can't honour (as the group runs would), and read the distinct
/// `env` maps of its `system` cases and the secrets its Workers declare.
fn plan(args: &DevRunArgs) -> Result<Plan, Planned> {
    let options = crate::try_project_options(&args.input)
        .map_err(|e| Planned::Message(e.to_string()))?
        .contracts(true)
        .remote_tests(Some(RemoteTests {
            skip_unsupported: args.skip_unsupported,
            dev: true,
            ..RemoteTests::default()
        }))
        .target(BuildTarget::Workers);
    let out = match bynk_emit::project::compile_project(&options) {
        Ok(out) => out,
        Err(failure) => {
            if args.json {
                print!(
                    "{}",
                    TestRun::compile_error(crate::project_failure_short_lines(&failure)).render()
                );
            } else {
                crate::print_project_failure(&failure);
            }
            return Err(Planned::Failed(ExitCode::FAILURE));
        }
    };
    let mut groups: Vec<BTreeMap<String, String>> = Vec::new();
    for env in out.discovered.iter().flat_map(|s| s.envs.iter()) {
        if !groups.contains(env) {
            groups.push(env.clone());
        }
    }
    let mut secrets = BTreeSet::new();
    for (path, doc) in &out.artefacts.docs {
        if path.file_name().is_some_and(|n| n == "bynk-secrets.json") {
            let manifest: serde_json::Value = serde_json::from_str(&doc.text()).unwrap_or_default();
            for key in ["declared", "read"] {
                for name in manifest[key].as_array().into_iter().flatten() {
                    if let Some(name) = name.as_str() {
                        secrets.insert(name.to_string());
                    }
                }
            }
        }
    }
    Ok(Plan { groups, secrets })
}

/// The variables one `bynk dev` serves: the group's `env`, then each declared
/// secret the group doesn't set at the fixed test value.
pub fn dev_vars(
    group: &BTreeMap<String, String>,
    secrets: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut vars = group.clone();
    for s in secrets {
        vars.entry(s.clone())
            .or_insert_with(|| TEST_SECRET.to_string());
    }
    vars
}

/// Run one group's cases against `urls`: this binary again, as a run
/// against `dev` with the URLs and the group's `env`, and the group's
/// variables in its environment so the drivers sign with them. Returns
/// whether it passed and, under `--format json`, its document.
fn run_group(
    args: &DevRunArgs,
    group: &BTreeMap<String, String>,
    urls: &BTreeMap<String, String>,
    vars: &BTreeMap<String, String>,
) -> Result<(bool, Option<serde_json::Value>), String> {
    let me = std::env::current_exe().map_err(|e| format!("could not find this binary: {e}"))?;
    let mut cmd = Command::new(me);
    cmd.arg("test")
        .arg(&args.input)
        .arg("--output")
        .arg(&args.output);
    cmd.arg("--format")
        .arg(if args.json { "json" } else { "rich" });
    cmd.args(["--against", DEV]);
    if let Some(seed) = &args.seed {
        cmd.args(["--seed", seed]);
    }
    if let Some(case) = &args.case {
        cmd.args(["--case", case]);
    }
    if args.skip_unsupported {
        cmd.arg("--skip-unsupported");
    }
    if let Some(settle) = &args.settle {
        cmd.args(["--settle", settle]);
    }
    cmd.env_remove(crate::remote::DEV_COMMAND_ENV);
    // One `run.id` for every group of this run, through the orchestration's
    // own variable: the group's runner always sets the harness's.
    cmd.env(crate::remote::DEV_RUN_ID_ENV, &args.run_id);
    cmd.env(
        DEV_URLS_ENV,
        serde_json::to_string(urls).expect("a string map serialises"),
    );
    cmd.env(
        DEV_ENV_ENV,
        serde_json::to_string(group).expect("a string map serialises"),
    );
    // One JSON variable, never a variable per name: an `env` entry or secret
    // named `PATH` or `NODE_OPTIONS` must not change the runner itself.
    cmd.env(
        DEV_SECRETS_ENV,
        serde_json::to_string(vars).expect("a string map serialises"),
    );
    if args.json {
        let out = cmd
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| format!("could not run the suites: {e}"))?;
        let doc = serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("the run printed no JSON document: {e}"))?;
        Ok((out.status.success(), Some(doc)))
    } else {
        let status = cmd
            .status()
            .map_err(|e| format!("could not run the suites: {e}"))?;
        Ok((status.success(), None))
    }
}

/// Fold the groups' documents into one: counts summed, suites of the same
/// name merged, the first error kept, `transport: "remote:dev"`.
fn merge(docs: &[serde_json::Value]) -> TestRun {
    use crate::test_json::{Case, Location, Suite, TestError};
    let count = |d: &serde_json::Value, k: &str| d[k].as_u64().unwrap_or(0) as u32;
    let mut run = TestRun::empty();
    run.transport = Some(format!("remote:{DEV}"));
    let suites = run.suites.get_or_insert_with(Vec::new);
    for doc in docs {
        run.passed += count(doc, "passed");
        run.failed += count(doc, "failed");
        run.skipped += count(doc, "skipped");
        for s in doc["suites"].as_array().into_iter().flatten() {
            let name = s["name"].as_str().unwrap_or_default().to_string();
            let cases = s["cases"].as_array().into_iter().flatten().map(|c| Case {
                name: c["name"].as_str().unwrap_or_default().to_string(),
                outcome: c["outcome"].as_str().unwrap_or_default().to_string(),
                message: c["message"].as_str().map(str::to_string),
                location: c["location"].as_object().map(|l| Location {
                    path: l["path"].as_str().unwrap_or_default().to_string(),
                    line: l["line"].as_u64().unwrap_or(0) as u32,
                    col: l["col"].as_u64().unwrap_or(0) as u32,
                }),
                retries: c["retries"].as_u64().map(|n| n as u32),
                tier: c["tier"].as_str().map(str::to_string),
            });
            match suites.iter_mut().find(|x| x.name == name) {
                Some(existing) => {
                    // Every group's build reports the same skipped cases;
                    // list each once.
                    for case in cases {
                        let repeat = case.outcome == "skipped"
                            && existing
                                .cases
                                .iter()
                                .any(|c| c.outcome == "skipped" && c.name == case.name);
                        if repeat {
                            run.skipped = run.skipped.saturating_sub(1);
                        } else {
                            existing.cases.push(case);
                        }
                    }
                }
                None => suites.push(Suite {
                    name,
                    kind: s["kind"].as_str().unwrap_or("integration").to_string(),
                    cases: cases.collect(),
                }),
            }
        }
        if run.error.is_none()
            && let Some(e) = doc["error"].as_object()
        {
            run.error = Some(TestError {
                kind: e["kind"].as_str().unwrap_or("runtime").to_string(),
                message: e
                    .get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string),
                diagnostics: e
                    .get("diagnostics")
                    .and_then(|d| d.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|d| d.as_str().map(str::to_string))
                    .collect(),
                stderr: e.get("stderr").and_then(|m| m.as_str()).map(str::to_string),
            });
        }
    }
    run
}

/// Remove a readiness file left by an earlier run, so it is never read as
/// this session being ready. A missing file is fine.
pub fn remove_stale(ready: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(ready) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Two distinct localhost ports, free when asked: the base of the HTTP and
/// the inspector allocations `bynk dev` makes.
fn free_ports() -> std::io::Result<(u16, u16)> {
    let a = std::net::TcpListener::bind("127.0.0.1:0")?;
    let b = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok((a.local_addr()?.port(), b.local_addr()?.port()))
}

/// A running `bynk dev`, stopped on drop.
struct Dev {
    child: Child,
    stopped: bool,
}

impl Dev {
    fn start(
        bynk: &Path,
        input: &Path,
        ready: &Path,
        log: &Path,
        vars: &BTreeMap<String, String>,
    ) -> std::io::Result<Dev> {
        if let Some(dir) = ready.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // A file left by an earlier run that was killed must never read as
        // this session being ready.
        crate::dev_run::remove_stale(ready)?;
        let (port, inspector) = free_ports()?;
        let log_file = std::fs::File::create(log)?;
        let mut cmd = Command::new(bynk);
        cmd.arg("dev")
            .arg(input)
            .arg("--base-port")
            .arg(port.to_string())
            .arg("--inspect-port")
            .arg(inspector.to_string())
            .arg("--ready-json")
            .arg(ready)
            .arg("--");
        // Only variables are passed through: never `--remote`.
        for (k, v) in vars {
            cmd.arg("--var").arg(format!("{k}:{v}"));
        }
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let child = cmd
            .stdin(Stdio::null())
            .stdout(log_file.try_clone()?)
            .stderr(log_file)
            .spawn()?;
        Ok(Dev {
            child,
            stopped: false,
        })
    }

    /// Wait for the readiness file and read its URLs, which must all be
    /// loopback.
    fn wait_ready(&mut self, ready: &Path) -> Result<BTreeMap<String, String>, String> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(text) = std::fs::read_to_string(ready)
                && let Ok(urls) = serde_json::from_str::<BTreeMap<String, String>>(&text)
            {
                if let Some(bad) = urls.values().find(|u| !is_loopback_http(u)) {
                    return Err(format!("`bynk dev` reported `{bad}`, which isn't loopback"));
                }
                return Ok(urls);
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(format!("`bynk dev` exited before it was ready ({status})"));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "`bynk dev` wasn't ready within {}s",
                    READY_TIMEOUT.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Stop `bynk dev`: SIGTERM to its own pid, so it stops its Workers; then,
    /// always, its whole process group, so a wrangler or `workerd` it leaves
    /// behind (because it exited first, or didn't stop them in time) never
    /// leaks. Runs once; a second call (the drop after an explicit stop) does
    /// nothing.
    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        #[cfg(unix)]
        {
            let pid = self.child.id().to_string();
            if !matches!(self.child.try_wait(), Ok(Some(_))) {
                let _ = Command::new("kill").args(["-s", "TERM", &pid]).status();
                let deadline = Instant::now() + STOP_GRACE;
                while Instant::now() < deadline {
                    if matches!(self.child.try_wait(), Ok(Some(_))) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            // `bynk dev` leads its own process group, so `-<pid>` names
            // exactly what it started and nothing else on the machine.
            let _ = Command::new("kill")
                .args(["-s", "KILL", "--", &format!("-{pid}")])
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Dev {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A merged `--against dev` document keeps each case's `tier`, which the
    /// groups' own documents carry.
    #[test]
    fn merging_the_groups_keeps_each_cases_tier() {
        let doc = serde_json::json!({
            "passed": 1, "failed": 0,
            "suites": [{"name": "shop", "kind": "integration", "cases": [
                {"name": "wired", "outcome": "pass", "tier": "system"},
                {"name": "plain", "outcome": "pass"}
            ]}]
        });
        let merged = merge(&[doc]);
        let cases = &merged.suites.as_ref().unwrap()[0].cases;
        assert_eq!(cases[0].tier.as_deref(), Some("system"));
        assert_eq!(cases[1].tier, None);
    }

    #[test]
    fn a_group_s_env_wins_and_other_secrets_get_the_test_value() {
        let group = BTreeMap::from([
            ("PAYMENTS".to_string(), "fake".to_string()),
            ("AUTH_SECRET".to_string(), "s3".to_string()),
        ]);
        let secrets = BTreeSet::from(["AUTH_SECRET".to_string(), "HOOK_SECRET".to_string()]);
        let vars = dev_vars(&group, &secrets);
        assert_eq!(vars["PAYMENTS"], "fake");
        assert_eq!(vars["AUTH_SECRET"], "s3");
        assert_eq!(vars["HOOK_SECRET"], TEST_SECRET);
    }

    #[test]
    fn a_stale_ready_file_is_removed_before_a_session_starts() {
        let dir = std::env::temp_dir().join(format!("bynk-dev-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ready = dir.join(".bynk-dev-ready-0.json");
        // An earlier, killed run's file, pointing somewhere bogus.
        std::fs::write(&ready, r#"{"svc":"http://localhost:1"}"#).unwrap();
        // `Dev::start` removes it before spawning, even when the spawn then
        // fails (here: no such binary), so it can never read as ready.
        let started = Dev::start(
            &dir.join("no-such-bynk"),
            &dir,
            &ready,
            &dir.join("dev.log"),
            &BTreeMap::new(),
        );
        assert!(started.is_err());
        assert!(!ready.exists(), "the stale file is gone");
        remove_stale(&ready).expect("removing an absent file is fine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn group_documents_merge_into_one() {
        let a = serde_json::json!({"passed": 1, "failed": 0, "suites": [{"name": "s", "kind": "integration", "cases": [{"name": "a", "outcome": "pass"}]}], "transport": "remote:dev"});
        let b = serde_json::json!({"passed": 0, "failed": 1, "suites": [{"name": "s", "kind": "integration", "cases": [{"name": "b", "outcome": "fail", "message": "m"}]}], "transport": "remote:dev"});
        let run = merge(&[a, b]);
        assert_eq!((run.passed, run.failed), (1, 1));
        let suites = run.suites.expect("suites");
        assert_eq!(suites.len(), 1);
        assert_eq!(suites[0].cases.len(), 2);
        assert_eq!(run.transport.as_deref(), Some("remote:dev"));
    }
}
