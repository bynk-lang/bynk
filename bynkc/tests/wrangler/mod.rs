//! The one wrangler package spec the workerd smokes provision, shared so CI's
//! pre-warm step and the tests cannot name different ones.
//!
//! Each workerd smoke runs `npx -y <spec> dev`, and they do it concurrently:
//! `events_boundary_workerd.rs`'s two tests run on two threads under
//! `cargo test`, nextest runs every test as its own process in one pool, and
//! `events_ordering_workerd.rs` starts two workers at once inside a single
//! test. Concurrent `npx -y <spec>` invocations all resolve to the SAME
//! `~/.npm/_npx/<hash>` directory — the hash is of the package spec, not the
//! argv — and npm does not lock it. On a cold cache they race: the v0.245.0
//! release run had npm tear down its own half-extracted tree (`ENOTEMPTY` on
//! rmdir, then a fan of `TAR_ENTRY_ERROR ENOENT`), so no wrangler ever served
//! and both boundary tests burned the full 180s boot window before failing
//! under `BYNK_REQUIRE_WORKERD`.
//!
//! The fix is a CI step that provisions the spec once, serially, before the
//! suite runs — after which every test finds a complete cache and installs
//! nothing. That only works while the step names the spec the tests actually
//! run, which is what `wrangler_prewarm.rs` guards: a bump to `wrangler@5` here
//! that forgets the workflows would silently restore the race, visible only as
//! an intermittent red.

/// The `npx` package spec every workerd smoke provisions.
pub const SPEC: &str = "wrangler@4";

/// I2 (#1971): the `--var`s a workerd smoke passes `wrangler dev` so its
/// Worker holds the local key `bynk dev` gives it for each `(caller, callee)`
/// edge in `edges`, on both ends. A Worker holding a key for an edge it is not
/// on is harmless, so one list can serve every Worker of a test.
#[allow(dead_code)]
pub fn local_edge_vars(edges: &[(&str, &str)]) -> Vec<String> {
    let mut vars = Vec::new();
    for (caller, callee) in edges {
        for key in [
            bynk_check::wire_format::edge_key_from(caller),
            bynk_check::wire_format::edge_key_to(callee),
        ] {
            let key = key.expect("a test context's name is an edge name");
            vars.push("--var".to_string());
            vars.push(format!("{key}:{}", bynk_emit::LOCAL_CALL_KEY));
        }
    }
    vars
}

/// I2 (#1971): the headers `caller` signs a `POST` to `path` on `callee` with,
/// under the local key, computed by Node's `crypto` from the published signing
/// input (the smokes already need Node to run wrangler).
#[allow(dead_code)]
pub fn local_signed_headers(
    caller: &str,
    callee: &str,
    path: &str,
    body: &str,
) -> Vec<(String, String)> {
    use bynk_check::wire_format as wf;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs()
        .to_string();
    let script = r#"const c = require("node:crypto");
const [key, input, body] = process.argv.slice(1);
const digest = c.createHash("sha256").update(body, "utf8").digest("hex");
process.stdout.write(c.createHmac("sha256", key).update(input + digest, "utf8").digest("hex"));"#;
    // Everything but the body digest, which the script appends.
    let input = wf::signing_input(caller, callee, "POST", path, "", "1", &timestamp, "");
    let out = std::process::Command::new("node")
        .args(["-e", script, bynk_emit::LOCAL_CALL_KEY, &input, body])
        .output()
        .expect("node runs");
    assert!(out.status.success(), "node could not sign: {out:?}");
    vec![
        (wf::CALLER_HEADER.to_string(), caller.to_string()),
        (wf::TIMESTAMP_HEADER.to_string(), timestamp),
        (
            wf::SIGNATURE_HEADER.to_string(),
            String::from_utf8(out.stdout).expect("hex"),
        ),
    ]
}

/// The dev registry a test gave its wranglers ([`Wrangler::spawn`]), each
/// entry with the address it publishes, for a failure message: it names which
/// process a Service Binding was reaching.
#[allow(dead_code)]
pub fn registry_listing(registry: &std::path::Path) -> String {
    let Ok(entries) = std::fs::read_dir(registry) else {
        return format!("<no dev registry at {registry:?}>");
    };
    let mut listing: Vec<String> = entries
        .flatten()
        .map(|e| {
            let text = std::fs::read_to_string(e.path()).unwrap_or_default();
            format!("{}: {}", e.file_name().to_string_lossy(), text.trim())
        })
        .collect();
    listing.sort();
    listing.join("\n")
}

/// A spawned `npx -y wrangler@4 dev`, stopped together with **every process
/// under it** when it drops (#1686).
///
/// Signalling the child alone is not enough, whichever signal is used. The
/// child is `npx`, and the server is four processes further down: `npx` runs
/// `sh -c wrangler`, which runs wrangler's `node` launcher, which runs
/// wrangler's CLI, which runs two `workerd serve`s. Neither SIGKILL
/// (`Child::kill`) nor SIGTERM sent to `npx` reaches them. Both were tried, one
/// per smoke, and both left the whole chain re-parented to init with its ports
/// still bound. Over a day of local runs that was 172 processes, and the next
/// boot failed with `bind(): Address already in use`, which reads as a
/// regression in whatever was under test.
///
/// So the child gets a process group of its own, and the descendants inherit
/// it (wrangler never moves them out). Teardown signals the group. SIGTERM goes
/// first, because wrangler traps it and stops its `workerd`s cleanly. SIGKILL
/// follows only for what is still running after the grace period.
///
/// One cost: the group is not the terminal's foreground group, so a Ctrl-C on
/// a local `cargo test` no longer reaches the chain. The test process dies
/// without running `Drop`, and the wrangler it started is left running. Before
/// #1686 the SIGINT reached the whole chain directly. CI pays the same cost
/// when nextest terminates a hung test (`slow-timeout` in
/// `.config/nextest.toml`'s `ci` profile, #1929): the signal reaches the
/// test's process group, not this one, so the chain outlives the job's
/// remaining tests. The retry still boots, on its own fresh port. To clean up
/// after an interrupted run, kill the `workerd`s whose cwd is under
/// `/tmp/bynk-*`.
///
/// Off unix there are no process groups. On Windows the child is `cmd /C
/// npx`, so `Child::kill` alone stopped only `cmd` and left the rest of the
/// chain serving for the rest of the job (a dozen `LK` tests per run, and the
/// runner's orphan cleanup listing two dozen `node`s). Teardown there kills
/// the child's whole tree with `taskkill /T /F` instead.
///
/// Every wrangler is also given a dev registry of its own test's (see
/// [`Wrangler::spawn`]), so a chain that does outlive its test, whichever OS,
/// cannot be the Worker a later test's Service Binding reaches.
#[allow(dead_code)] // `wrangler_prewarm.rs` takes only `SPEC` from this module.
pub struct Wrangler {
    child: std::process::Child,
    /// `Some` once SIGTERM has been sent: whether `kill(1)` delivered it.
    terminated: Option<bool>,
    stopped: bool,
}

#[allow(dead_code)]
impl Wrangler {
    /// How long wrangler gets to stop its own `workerd`s after SIGTERM.
    pub const GRACE: std::time::Duration = std::time::Duration::from_secs(10);

    /// Spawn `cmd` as the leader of a new process group, with `registry` as
    /// its dev registry.
    ///
    /// The dev registry is where `wrangler dev` publishes each Worker's
    /// address by name, and where a Service Binding looks its target up. It
    /// defaults to one directory per machine, and miniflare will not register
    /// a name that a live process already holds: it skips, and retries once
    /// the entry is 90s stale. So a `fault-callee` leaked by a first nextest
    /// try kept the name, the retry's own callee never registered, and the
    /// retry's caller called the leaked one, which logged the fault into the
    /// first try's file (#2294). Every wrangler of one test gets the same
    /// `registry`, in that test's own temp directory, so the Workers it
    /// starts see each other and nothing else.
    ///
    /// Stdin is null. A process outside the terminal's foreground group that
    /// reads from the terminal, or changes its mode, is stopped by SIGTTIN or
    /// SIGTTOU. Wrangler's hotkeys would do both on a TTY, which would freeze
    /// it before it serves.
    pub fn spawn(
        cmd: &mut std::process::Command,
        registry: &std::path::Path,
    ) -> std::io::Result<Self> {
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(cmd, 0);
        Ok(Wrangler {
            child: cmd
                .env("WRANGLER_REGISTRY_PATH", registry)
                .stdin(std::process::Stdio::null())
                .spawn()?,
            terminated: None,
            stopped: false,
        })
    }

    /// The spawned child, for reading its piped output. Read a pipe only
    /// after [`Wrangler::stop`]: every process in the group holds the write
    /// end, so a read before then blocks for as long as they run.
    pub fn child(&mut self) -> &mut std::process::Child {
        &mut self.child
    }

    /// Send SIGTERM to the group without waiting. [`stop_all`] does this for
    /// every wrangler before waiting on any, so their shutdowns overlap.
    /// Idempotent.
    pub fn request_stop(&mut self) {
        if self.terminated.is_some() {
            return;
        }
        #[cfg(unix)]
        let sent = signal_group("TERM", &self.group());
        #[cfg(not(unix))]
        let sent = false;
        self.terminated = Some(sent);
    }

    /// Stop the whole group, giving wrangler [`Wrangler::GRACE`] to do it
    /// cleanly. Idempotent; `Drop` calls it.
    pub fn stop(&mut self) {
        self.stop_within(Self::GRACE);
    }

    /// [`Wrangler::stop`] with an explicit grace period, so a test can reach
    /// the SIGKILL escalation without waiting the full ten seconds.
    ///
    /// When it returns, the group is gone, so a caller can check right away
    /// that nothing survived. The exception is a failed `kill(1)`. That leaves
    /// only `Child::kill`, which reaches the leader alone.
    pub fn stop_within(&mut self, grace: std::time::Duration) {
        if std::mem::replace(&mut self.stopped, true) {
            return;
        }
        self.request_stop();
        #[cfg(unix)]
        if self.terminated == Some(true) {
            let group = self.group();
            let deadline = std::time::Instant::now() + grace;
            // Reap the leader first: a zombie leader still counts as a member,
            // so the group would never look empty until it is reaped.
            while self.child.try_wait().is_ok_and(|s| s.is_none())
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let empty = self.child.try_wait().is_ok_and(|s| s.is_some())
                && wait_until_empty(&group, deadline);
            // Only a group that still has members gets SIGKILL. Once the group
            // is empty and the leader reaped, its id can be reused by an
            // unrelated group.
            if !empty {
                signal_group("KILL", &group);
                let _ = self.child.wait();
                // SIGKILL is delivered asynchronously. Wait until it has landed.
                let soon = std::time::Instant::now() + std::time::Duration::from_secs(1);
                wait_until_empty(&group, soon);
            }
        }
        // `/T` walks the tree from the child, so it must run while `cmd` is
        // still alive to be its root.
        #[cfg(windows)]
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &self.child.id().to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// The group's id in the form `kill(1)` takes. It is the leader's pid,
    /// because `process_group(0)` made it so.
    #[cfg(unix)]
    fn group(&self) -> String {
        format!("-{}", self.child.id())
    }
}

impl Drop for Wrangler {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Stop several wranglers. All of them are signalled before any is waited on,
/// so the worst case is one grace period rather than one per wrangler, and
/// none keeps serving while another shuts down.
#[allow(dead_code)]
pub fn stop_all(wranglers: &mut [Wrangler]) {
    for w in wranglers.iter_mut() {
        w.request_stop();
    }
    for w in wranglers.iter_mut() {
        w.stop();
    }
}

/// Poll until `group` has no members, or until `deadline`. `true` if it
/// emptied.
#[cfg(unix)]
fn wait_until_empty(group: &str, deadline: std::time::Instant) -> bool {
    loop {
        if !signal_group("0", group) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Send `signal` to the process group `group` (written `-<pgid>`) with
/// `kill(1)`, since std has no way to signal a group. `true` when some member
/// received it. With signal `0` nothing is sent, so the result says whether
/// the group still has members.
#[cfg(unix)]
fn signal_group(signal: &str, group: &str) -> bool {
    std::process::Command::new("kill")
        // `--` stops `kill` from reading the group's leading `-` as an option.
        .args([&format!("-{signal}"), "--", group])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The pids of the live processes whose working directory is `dir` or is
/// under it. Run this before removing `dir`: once it is gone, the kernel
/// reports each such cwd with a ` (deleted)` suffix and they no longer match.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub fn processes_under(dir: &std::path::Path) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            std::fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|cwd| cwd.starts_with(dir))
        })
        .collect()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{Wrangler, processes_under};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    /// Run `script` under a `Wrangler` guard in a fresh directory, wait until
    /// it has started its background `sleep`s, then stop it within `grace` and
    /// return what survived. The `sleep`s stand in for wrangler's `workerd`s:
    /// grandchildren the child never signals itself.
    fn survivors(tag: &str, script: &str, grace: Duration) -> Vec<u32> {
        let dir =
            std::env::temp_dir().join(format!("bynk-wrangler-guard-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Null stdio: a stranded `sleep` holding the harness's output pipe
        // would hang the run instead of failing the assertion below.
        let mut guard = Wrangler::spawn(
            Command::new("sh")
                .args(["-c", script])
                .current_dir(&dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
            &dir.join("registry"),
        )
        .expect("sh spawns");
        // The `sh` and its two `sleep`s.
        let started = std::time::Instant::now();
        while processes_under(&dir).len() < 3 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the script never started its background processes"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        guard.stop_within(grace);
        let left = processes_under(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        left
    }

    /// The #1686 regression: before, signalling the child stranded its
    /// grandchildren. They must go with it.
    #[test]
    fn stopping_takes_the_grandchildren_with_it() {
        let left = survivors("term", "sleep 300 & sleep 300 & wait", Wrangler::GRACE);
        assert!(left.is_empty(), "processes survived the guard: {left:?}");
    }

    /// A group that ignores SIGTERM is still stopped, by SIGKILL once the
    /// grace period is over. The `sleep`s inherit the ignored disposition.
    #[test]
    fn a_group_that_ignores_sigterm_is_killed_after_the_grace_period() {
        let left = survivors(
            "kill",
            "trap '' TERM; sleep 300 & sleep 300 & wait",
            Duration::from_millis(300),
        );
        assert!(left.is_empty(), "processes survived SIGKILL: {left:?}");
    }
}
