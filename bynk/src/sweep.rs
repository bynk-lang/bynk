//! Stopping what `bynk dev`'s wranglers leave behind (#1742).
//!
//! `bynk dev` stops each `wrangler dev` by signalling the child it spawned.
//! That child is rarely the server. Resolved via npx, it is `npx`, and the
//! server is four processes further down (`sh -c wrangler` → wrangler's `node`
//! launcher → its CLI → two `workerd serve`s); a signal to `npx` reaches none of
//! them. Resolved on PATH, the child is the launcher, which does pass SIGTERM
//! on. But when a context's wrangler *crashes*, its `workerd`s are re-parented
//! to init before `bynk dev` ever acts, and nothing walks from a child to them
//! any more. Either way the survivors keep their ports, and the next `bynk dev`
//! fails at boot with `bind(): Address already in use`.
//!
//! Two properties of those survivors outlast the re-parenting:
//!
//! - **Process group.** Nothing in the chain leaves the group it was spawned
//!   into, which is `bynk dev`'s own: they share it so a Ctrl-C reaches them
//!   all.
//! - **Working directory.** `bynk dev` starts each wrangler inside its worker
//!   dir under the managed build dir, and every descendant inherits it.
//!
//! [`sweep`] stops the processes that have both, so it reaches orphans without
//! guessing by name. The working directory is what keeps it from touching an
//! unrelated process: when `bynk dev` is not a group leader (a script without
//! job control runs it), its group can hold the script's other jobs. Neither
//! `bynk dev` itself nor its ancestors are ever selected.
//!
//! Unix only. Windows has neither process groups nor a working directory to
//! read. There, `bynk dev` puts each wrangler in a job object instead, which
//! holds the whole tree, orphans included, and stops it as one (#1762).

use bynk_driver::emsg;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Stop every process in this process group whose working directory is under
/// `root`: SIGTERM first, then SIGKILL for what is still running after
/// `grace`. Returns once none is left, or after SIGKILL has been given a moment
/// to land.
///
/// Best effort, but never silently. If there is something to stop, it says
/// so, because the wait can be long and would otherwise read as a hang. If
/// anything is still running at the end, or `kill(1)` cannot be run, it names
/// the survivors: they hold ports, and the next `bynk dev` would otherwise fail
/// with a bind error and nothing pointing back here. If `ps` cannot be run
/// there is nothing to select from, and this does nothing, as `bynk dev` did
/// before #1742.
pub fn sweep(root: &Path, grace: Duration) {
    #[cfg(unix)]
    {
        let deadline = std::time::Instant::now() + grace;
        let mut termed = false;
        loop {
            let left = stragglers(root);
            if left.is_empty() {
                return;
            }
            if !termed {
                eprintln!(
                    "bynk dev: stopping {} leftover process(es) under {}…",
                    left.len(),
                    root.display()
                );
                // Without `kill(1)` nothing can be signalled (std signals only
                // its own children), so waiting out the grace would be wasted.
                if !signal("TERM", &left) {
                    report_survivors("could not run `kill`", &left);
                    return;
                }
                termed = true;
            } else if std::time::Instant::now() >= deadline {
                signal("KILL", &left);
                // SIGKILL is delivered asynchronously. Wait briefly for it to
                // land, so a caller checking straight afterwards sees it.
                let soon = std::time::Instant::now() + Duration::from_secs(1);
                let mut left = stragglers(root);
                while !left.is_empty() && std::time::Instant::now() < soon {
                    std::thread::sleep(Duration::from_millis(20));
                    left = stragglers(root);
                }
                if !left.is_empty() {
                    report_survivors("they survived SIGKILL", &left);
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    // Windows: each wrangler's job object has already stopped its tree (#1762).
    #[cfg(not(unix))]
    let _ = (root, grace);
}

/// Name the processes [`sweep`] could not stop, and why.
#[cfg(unix)]
fn report_survivors(why: &str, pids: &[u32]) {
    let pids = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    emsg!(
        "bynk dev: could not stop leftover processes ({why}): {pids}. They may hold \
         ports the next `bynk dev` needs; stop them by hand."
    );
}

/// The processes [`sweep`] would stop now.
#[cfg(unix)]
pub fn stragglers(root: &Path) -> Vec<u32> {
    let Some(table) = process_table() else {
        return Vec::new();
    };
    // Compare canonical paths: a process's cwd is reported resolved, and the
    // build dir may sit behind a symlink (macOS's `/tmp` is `/private/tmp`).
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let candidates = in_own_group(&table, std::process::id());
    let cwds = cwds(&candidates);
    candidates
        .into_iter()
        .filter(|pid| {
            cwds.iter()
                .any(|(p, cwd)| p == pid && cwd.starts_with(&root))
        })
        .collect()
}

/// One row of `ps`: a process, its parent and its process group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(unix), allow(dead_code))]
struct Row {
    pid: u32,
    ppid: u32,
    pgid: u32,
}

/// Every process on the system, from `ps` (POSIX `-A -o`, so Linux and macOS
/// alike). `None` when `ps` cannot be run.
#[cfg(unix)]
fn process_table() -> Option<Vec<Row>> {
    let out = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,pgid="])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| parse_ps(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg_attr(not(unix), allow(dead_code))]
fn parse_ps(text: &str) -> Vec<Row> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace().map(str::parse::<u32>);
            let row = Row {
                pid: fields.next()?.ok()?,
                ppid: fields.next()?.ok()?,
                pgid: fields.next()?.ok()?,
            };
            Some(row)
        })
        .collect()
}

/// The other members of `me`'s process group, leaving out `me` and its
/// ancestors. An ancestor shares the group when nothing between it and `me`
/// started a new one, and stopping the shell or script that ran `bynk dev`
/// would be the worst possible outcome.
#[cfg_attr(not(unix), allow(dead_code))]
fn in_own_group(table: &[Row], me: u32) -> Vec<u32> {
    let Some(own) = table.iter().find(|r| r.pid == me) else {
        return Vec::new();
    };
    let mut ancestors = vec![me];
    let mut at = own.ppid;
    // Bounded, so a malformed table with a cycle cannot loop forever.
    for _ in 0..table.len() {
        if at == 0 || ancestors.contains(&at) {
            break;
        }
        ancestors.push(at);
        match table.iter().find(|r| r.pid == at) {
            Some(r) => at = r.ppid,
            None => break,
        }
    }
    table
        .iter()
        .filter(|r| r.pgid == own.pgid && !ancestors.contains(&r.pid))
        .map(|r| r.pid)
        .collect()
}

/// The working directory of each of `pids` that still exists and can be read.
#[cfg(target_os = "linux")]
fn cwds(pids: &[u32]) -> Vec<(u32, PathBuf)> {
    pids.iter()
        .filter_map(|&pid| Some((pid, std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?)))
        .collect()
}

/// The working directory of each of `pids`, from one `lsof` call: there is no
/// `/proc` here (macOS, the BSDs). Empty when `lsof` cannot be run.
#[cfg(all(unix, not(target_os = "linux")))]
fn cwds(pids: &[u32]) -> Vec<(u32, PathBuf)> {
    if pids.is_empty() {
        return Vec::new();
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    // `-a` ANDs the selections: only these pids, and only their cwd. `lsof`
    // exits 1 when some pid has gone, so its status says nothing; parse what
    // it printed.
    match std::process::Command::new("lsof")
        .args(["-a", "-p", &list, "-d", "cwd", "-F", "pn"])
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(out) => parse_lsof(&String::from_utf8_lossy(&out.stdout)),
        Err(_) => Vec::new(),
    }
}

/// Parse `lsof -F pn` output: a `p<pid>` line opens each process, and each
/// `n<path>` line after it names one of its files (here, only its cwd). Other
/// field lines (`lsof` adds `f<fd>`) are skipped.
#[cfg_attr(not(all(unix, not(target_os = "linux"))), allow(dead_code))]
fn parse_lsof(text: &str) -> Vec<(u32, PathBuf)> {
    let mut pid = None;
    let mut found = Vec::new();
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse::<u32>().ok();
        } else if let (Some(path), Some(pid)) = (line.strip_prefix('n'), pid) {
            found.push((pid, PathBuf::from(path)));
        }
    }
    found
}

/// Send `signal` to `pids` with `kill(1)`; std can send only SIGKILL, and only
/// to its own children.
///
/// `false` when `kill` could not be run at all. Its exit status is no test: a
/// pid that has exited in the meantime makes it fail as well.
#[cfg(unix)]
fn signal(signal: &str, pids: &[u32]) -> bool {
    std::process::Command::new("kill")
        .arg(format!("-{signal}"))
        .args(pids.iter().map(u32::to_string))
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, ppid: u32, pgid: u32) -> Row {
        Row { pid, ppid, pgid }
    }

    #[test]
    fn ps_rows_parse_and_junk_lines_are_skipped() {
        let text = "    1     0     1\n  200     1   200\nnot a row\n  201   200   200\n";
        assert_eq!(
            parse_ps(text),
            vec![row(1, 0, 1), row(200, 1, 200), row(201, 200, 200)]
        );
    }

    /// `bynk dev` (300) run by a script (200) that has no job control, so all
    /// three share the script's group with a sibling job (301). The orphan
    /// (400, re-parented to init) is still in the group.
    #[test]
    fn the_group_leaves_out_self_and_ancestors_but_keeps_orphans() {
        let table = [
            row(1, 0, 1),
            row(100, 1, 100),   // the login shell, its own group
            row(200, 100, 200), // the script
            row(300, 200, 200), // bynk dev
            row(301, 200, 200), // another job of the script
            row(310, 300, 200), // a wrangler bynk dev spawned
            row(400, 1, 200),   // an orphaned workerd
            row(500, 1, 500),   // unrelated
        ];
        let mut got = in_own_group(&table, 300);
        got.sort_unstable();
        // 301 is in the group too: the cwd filter, not the group, is what
        // leaves it alone.
        assert_eq!(got, vec![301, 310, 400]);
    }

    #[test]
    fn an_unlisted_self_selects_nothing() {
        assert!(in_own_group(&[row(1, 0, 1)], 999).is_empty());
    }

    #[test]
    fn lsof_output_parses_pid_and_cwd() {
        let text = "p310\nfcwd\nn/private/tmp/p/.bynk/dev/workers/a\np400\nfcwd\nn/Users/me\n";
        assert_eq!(
            parse_lsof(text),
            vec![
                (310, PathBuf::from("/private/tmp/p/.bynk/dev/workers/a")),
                (400, PathBuf::from("/Users/me")),
            ]
        );
    }

    /// The #1742 regression, with `sh` in place of wrangler. Killing the `sh`
    /// outright orphans its `sleep`s, the way a crashed wrangler orphans its
    /// `workerd`s. Nothing reaches them from the child any more, and the sweep
    /// must still stop them.
    #[cfg(unix)]
    fn orphans_are_swept(tag: &str, script: &str, grace: Duration) {
        use std::process::{Command, Stdio};
        let root = std::env::temp_dir().join(format!("bynk-sweep-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let worker = root.join("workers/ctx");
        std::fs::create_dir_all(&worker).unwrap();
        let mut sh = Command::new("sh")
            .args(["-c", script])
            .current_dir(&worker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sh spawns");
        let started = std::time::Instant::now();
        while stragglers(&root).len() < 3 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the script never started its background processes"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // A bystander: in the same process group (the test's), but working
        // elsewhere, like a script's other job. The sweep must not touch it.
        let mut bystander = Command::new("sleep")
            .arg("300")
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep spawns");
        let _ = sh.kill();
        let _ = sh.wait();
        let orphans = stragglers(&root);
        assert_eq!(
            orphans.len(),
            2,
            "the sleeps outlive their parent: {orphans:?}"
        );
        assert!(
            !orphans.contains(&bystander.id()),
            "a process outside the root was selected"
        );
        sweep(&root, grace);
        let left = stragglers(&root);
        let spared = bystander.try_wait().is_ok_and(|s| s.is_none());
        let _ = bystander.kill();
        let _ = bystander.wait();
        assert!(spared, "the sweep stopped a process outside the root");
        let _ = std::fs::remove_dir_all(&root);
        assert!(left.is_empty(), "processes survived the sweep: {left:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_crashed_childs_orphans_are_swept() {
        orphans_are_swept(
            "term",
            "sleep 300 & sleep 300 & wait",
            Duration::from_secs(10),
        );
    }

    /// Orphans that ignore SIGTERM are killed once the grace period is over.
    #[cfg(unix)]
    #[test]
    fn orphans_that_ignore_sigterm_are_killed_after_the_grace_period() {
        orphans_are_swept(
            "kill",
            "trap '' TERM; sleep 300 & sleep 300 & wait",
            Duration::from_millis(300),
        );
    }
}
