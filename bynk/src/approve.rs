//! `bynk approve` (packaging slice 5, #2296, decision D): review and approve
//! the npm packages that fetched (git or registry) packages' adapters bring
//! into the build, recording each approval in `bynk.lock`.
//!
//! An approval is per `[[npm]]` row — one npm package at one range (decision
//! A). The compile refuses a reached adapter's unapproved requirement
//! (`bynk.requires.unapproved`, decision B); this command is how a project
//! clears it. It rewrites only `approved` fields and never resolves.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use bynk_project::lock::{LOCK_FILE, Lock, LockedNpm, lock_root, parse_lock};

/// What `approve` should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Ask about each pending row (`bynk approve`); refused off a terminal.
    Ask,
    /// Approve the pending rows of this npm package.
    Package(String),
    /// Approve every pending row.
    All,
    /// List the rows; change nothing.
    List,
}

/// `bynk approve` for the project holding `project`.
pub fn run(project: &Path, mode: &Mode) -> Result<(), String> {
    let (path, mut lock) = read(project)?;
    let pending = pending_rows(&lock);
    if *mode == Mode::List {
        list(&lock, &pending);
        return Ok(());
    }
    let chosen: Vec<usize> = match mode {
        Mode::All => pending.clone(),
        Mode::Package(name) => {
            let rows: Vec<usize> = pending
                .iter()
                .copied()
                .filter(|&i| lock.npm[i].package == *name)
                .collect();
            if rows.is_empty() {
                return Err(if lock.npm.iter().any(|n| n.package == *name) {
                    format!("the npm package `{name}` has nothing awaiting approval")
                } else {
                    format!("no dependency's adapter requires the npm package `{name}`")
                });
            }
            for &i in &rows {
                describe(&lock, &lock.npm[i]);
            }
            rows
        }
        Mode::Ask => {
            if pending.is_empty() {
                eprintln!("bynk: no npm packages await approval");
                return Ok(());
            }
            if !std::io::stdin().is_terminal() {
                return Err(
                    "`bynk approve` asks on a terminal: name a package (`bynk approve <package>`), or use `--all`"
                        .to_string(),
                );
            }
            ask(&lock, &pending)
        }
        Mode::List => unreachable!(),
    };
    approve(&path, &mut lock, &chosen)
}

/// #2296: after `bynk add`, `fetch` or `update` wrote the lock with `pending`
/// rows, ask about each on a terminal; otherwise say what awaits approval and
/// how to approve it. Never fails the command (decision D).
pub fn after_write(project: &Path, pending: &[LockedNpm]) {
    if pending.is_empty() {
        return;
    }
    let Ok((path, mut lock)) = read(project) else {
        return;
    };
    let rows: Vec<usize> = pending_rows(&lock)
        .into_iter()
        .filter(|&i| {
            pending
                .iter()
                .any(|p| p.package == lock.npm[i].package && p.range == lock.npm[i].range)
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    if std::io::stdin().is_terminal() {
        eprintln!(
            "bynk: {} npm package{} from fetched packages await{} approval",
            rows.len(),
            if rows.len() == 1 { "" } else { "s" },
            if rows.len() == 1 { "s" } else { "" }
        );
        let chosen = ask(&lock, &rows);
        if let Err(e) = approve(&path, &mut lock, &chosen) {
            eprintln!("bynk: {e}");
        }
        return;
    }
    eprintln!("bynk: npm packages awaiting approval — a build reaching them fails until approved:");
    for &i in &rows {
        describe(&lock, &lock.npm[i]);
    }
    eprintln!(
        "bynk: review them, then run `bynk approve <package>` (or `bynk approve` on a terminal)"
    );
}

fn read(project: &Path) -> Result<(PathBuf, Lock), String> {
    let root = lock_root(project);
    let path = root.join(LOCK_FILE);
    let text = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "there is no `{}` here — run `bynk fetch` first",
            path.display()
        )
    })?;
    let lock = parse_lock(&text).map_err(|e| format!("`{}`: {}", path.display(), e.0))?;
    Ok((path, lock))
}

/// The rows awaiting approval: unapproved, and required by some fetched
/// (not path) package's adapter.
fn pending_rows(lock: &Lock) -> Vec<usize> {
    let fetched = |by: &str| {
        let name = by.split(':').next().unwrap_or_default();
        lock.package(name)
            .is_none_or(|p| !p.source.starts_with("path+"))
    };
    lock.npm
        .iter()
        .enumerate()
        .filter(|(_, n)| !n.approved && n.required_by.iter().any(|by| fetched(by)))
        .map(|(i, _)| i)
        .collect()
}

fn list(lock: &Lock, pending: &[usize]) {
    if lock.npm.is_empty() {
        eprintln!("bynk: no dependency's adapter requires an npm package");
        return;
    }
    for (i, row) in lock.npm.iter().enumerate() {
        let state = if row.approved {
            "approved"
        } else if pending.contains(&i) {
            "pending"
        } else {
            "own"
        };
        println!(
            "{state:<8} {} {}  (required by {})",
            row.package,
            row.range,
            row.required_by.join(", ")
        );
    }
}

/// Print one row: the package, range, who requires it and through what.
fn describe(lock: &Lock, row: &LockedNpm) {
    eprintln!("  {} {}", row.package, row.range);
    for by in &row.required_by {
        let package = by.split(':').next().unwrap_or_default();
        let mut chain = vec!["this project".to_string()];
        chain.extend(chain_to(lock, package));
        chain.push(by.clone());
        eprintln!("    required through {}", chain.join(" → "));
    }
    eprintln!("    https://www.npmjs.com/package/{}", row.package);
}

/// Ask `[y/N]` about each row; the rows answered yes.
fn ask(lock: &Lock, rows: &[usize]) -> Vec<usize> {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut chosen = Vec::new();
    for &i in rows {
        describe(lock, &lock.npm[i]);
        eprint!(
            "Approve `{}` {}? [y/N] ",
            lock.npm[i].package, lock.npm[i].range
        );
        let _ = std::io::stderr().flush();
        let answer = lines.next().and_then(Result::ok).unwrap_or_default();
        if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            chosen.push(i);
        }
    }
    chosen
}

fn approve(path: &Path, lock: &mut Lock, rows: &[usize]) -> Result<(), String> {
    if rows.is_empty() {
        eprintln!("bynk: nothing approved");
        return Ok(());
    }
    for &i in rows {
        lock.npm[i].approved = true;
    }
    crate::fetch::write_atomically(path, &lock.render())
        .map_err(|e| format!("could not write `{}`: {e}", path.display()))?;
    for &i in rows {
        eprintln!(
            "bynk: approved npm package `{}` {}",
            lock.npm[i].package, lock.npm[i].range
        );
    }
    Ok(())
}

/// A chain of locked packages from the project to `target`. The lock doesn't
/// record the project's own aliases, so its direct dependencies are taken to
/// be the locked packages no other one depends on: a direct dependency that is
/// also reached transitively shows through its longer path. The compile's
/// diagnostic (`PackageGraph::chain_to`) knows the aliases and is exact.
fn chain_to(lock: &Lock, target: &str) -> Vec<String> {
    let depended: BTreeSet<&str> = lock
        .packages
        .iter()
        .flat_map(|p| p.dependencies.iter().map(String::as_str))
        .collect();
    let mut parent: BTreeMap<&str, Option<&str>> = BTreeMap::new();
    let mut queue = VecDeque::new();
    for p in &lock.packages {
        if !depended.contains(p.name.as_str()) {
            parent.insert(&p.name, None);
            queue.push_back(p.name.as_str());
        }
    }
    while let Some(name) = queue.pop_front() {
        let Some(p) = lock.package(name) else {
            continue;
        };
        for d in &p.dependencies {
            if !parent.contains_key(d.as_str()) {
                parent.insert(d, Some(name));
                queue.push_back(d);
            }
        }
    }
    let mut chain = Vec::new();
    let mut at = parent.contains_key(target).then_some(target);
    while let Some(name) = at {
        chain.push(name.to_string());
        at = parent.get(name).copied().flatten();
        if chain.len() > lock.packages.len() {
            break;
        }
    }
    if chain.is_empty() {
        chain.push(target.to_string());
    }
    chain.reverse();
    chain
}
