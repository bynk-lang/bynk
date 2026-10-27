//! `cargo xtask <command>` — Bynk repo automation (increment-allocation track).
//!
//! - `check-pending` (Slice 0) validates the files under `design/pending/`.
//! - `stamp` (Slice 1) assigns the version(s) + ADR number(s) for the pending
//!   files and materialises them; dry-run by default, `--apply` to write.
//! - `greenfield-status` (compiler-architecture track, T0.0, #999) runs all nineteen
//!   probes against the tree and prints the report (the fourteen gated ones are what
//!   the committed table is diffed on); `--apply` writes the committed table.
//! - `manifest-schema` (#2096) writes `bynk.toml`'s JSON Schema, generated from
//!   the manifest's Rust types; `--check` fails when the committed one is stale.
//! - `editor-contract` writes the compiler-owned facts the VS Code extension
//!   checks itself against (reserved keywords, semantic-token legend);
//!   `--check` fails when the committed snapshot is stale, `--tag T --out F`
//!   writes the release asset.
//! - `ci` runs the gates CI runs, locally, in CI's own cheapest-first order —
//!   `--fast` stops after the two that need no compile-and-link.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check-pending") => check_pending(),
        Some("greenfield-status") => {
            if let Some(bad) = args[1..]
                .iter()
                .find(|a| *a != "--apply" && *a != "--list-unasserted")
            {
                eprintln!("xtask greenfield-status: unknown argument {bad:?}");
                usage();
                return ExitCode::from(2);
            }
            // #1662: the codes behind `diagnostic_coverage`'s count, one per line.
            if args.iter().any(|a| a == "--list-unasserted") {
                for code in xtask::greenfield_status::unasserted_codes(&repo_root()) {
                    println!("{code}");
                }
                return ExitCode::SUCCESS;
            }
            greenfield_status(args.iter().any(|a| a == "--apply"))
        }
        Some("stamp") => {
            // Reject unknown flags rather than silently dry-running — on a
            // dry-run-by-default command, a typo'd `--aply` must not read as "no
            // --apply given" when the maintainer meant to write.
            if let Some(bad) = args[1..].iter().find(|a| *a != "--apply") {
                eprintln!("xtask stamp: unknown argument {bad:?}");
                usage();
                return ExitCode::from(2);
            }
            stamp(args.iter().any(|a| a == "--apply"))
        }
        Some("manifest-schema") => {
            if let Some(bad) = args[1..].iter().find(|a| *a != "--check") {
                eprintln!("xtask manifest-schema: unknown argument {bad:?}");
                usage();
                return ExitCode::from(2);
            }
            manifest_schema(args.iter().any(|a| a == "--check"))
        }
        Some("editor-contract") => editor_contract(&args[1..]),
        Some("ci") => {
            if let Some(bad) = args[1..].iter().find(|a| *a != "--fast") {
                eprintln!("xtask ci: unknown argument {bad:?}");
                usage();
                return ExitCode::from(2);
            }
            ci(args.iter().any(|a| a == "--fast"))
        }
        Some(other) => {
            eprintln!("xtask: unknown command {other:?}");
            usage();
            ExitCode::from(2)
        }
        None => {
            usage();
            ExitCode::from(2)
        }
    }
}

/// The repo root — this crate's manifest dir is `<root>/xtask`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// #2096: write `bynk.toml`'s JSON Schema, or with `--check` fail when the
/// committed one is stale.
fn manifest_schema(check: bool) -> ExitCode {
    use xtask::manifest_schema::{SCHEMA_PATH, render, write};
    let root = repo_root();
    if check {
        let committed = std::fs::read_to_string(root.join(SCHEMA_PATH)).unwrap_or_default();
        if committed == render() {
            println!("manifest-schema: {SCHEMA_PATH} is current");
            return ExitCode::SUCCESS;
        }
        eprintln!("manifest-schema: {SCHEMA_PATH} is stale; run `cargo xtask manifest-schema`");
        return ExitCode::FAILURE;
    }
    match write(&root) {
        Ok(()) => {
            println!("manifest-schema: wrote {SCHEMA_PATH}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("manifest-schema: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Write the editor contract: the committed snapshot by default, `--check` to
/// fail when it is stale, or `--tag T --out F` for the release asset (the
/// snapshot plus the release's version, which the snapshot omits so a version
/// bump never makes it stale).
fn editor_contract(args: &[String]) -> ExitCode {
    use xtask::editor_contract::{SNAPSHOT_PATH, render, render_release, write};
    let (mut check, mut tag, mut out) = (false, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--check" => check = true,
            "--tag" | "--out" => {
                let Some(value) = it.next().filter(|v| !v.starts_with("--")) else {
                    eprintln!("xtask editor-contract: {a} needs a value");
                    usage();
                    return ExitCode::from(2);
                };
                if a == "--tag" {
                    tag = Some(value.clone());
                } else {
                    out = Some(value.clone());
                }
            }
            other => {
                eprintln!("xtask editor-contract: unknown argument {other:?}");
                usage();
                return ExitCode::from(2);
            }
        }
    }
    let root = repo_root();
    if tag.is_some() != out.is_some() || (check && tag.is_some()) {
        eprintln!("xtask editor-contract: --tag and --out go together, and not with --check");
        usage();
        return ExitCode::from(2);
    }
    if let (Some(tag), Some(out)) = (tag, out) {
        return match std::fs::write(&out, render_release(&tag)) {
            Ok(()) => {
                println!("editor-contract: wrote {out} for {tag}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("editor-contract: {out}: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if check {
        let committed = std::fs::read_to_string(root.join(SNAPSHOT_PATH)).unwrap_or_default();
        if committed == render() {
            println!("editor-contract: {SNAPSHOT_PATH} is current");
            return ExitCode::SUCCESS;
        }
        eprintln!("editor-contract: {SNAPSHOT_PATH} is stale; run `cargo xtask editor-contract`");
        return ExitCode::FAILURE;
    }
    match write(&root) {
        Ok(()) => {
            println!("editor-contract: wrote {SNAPSHOT_PATH}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("editor-contract: {e}");
            ExitCode::FAILURE
        }
    }
}

fn check_pending() -> ExitCode {
    match xtask::check_all() {
        Ok(n) => {
            println!("check-pending: {n} pending file(s) valid");
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for e in &errors {
                eprintln!("{e}");
            }
            eprintln!("check-pending: {} error(s)", errors.len());
            ExitCode::FAILURE
        }
    }
}

fn stamp(apply: bool) -> ExitCode {
    let root = repo_root();
    let plan = match xtask::stamp::plan(&root) {
        Ok(p) => p,
        Err(errors) => {
            for e in &errors {
                eprintln!("{e}");
            }
            eprintln!("stamp: {} error(s) in design/pending/", errors.len());
            return ExitCode::FAILURE;
        }
    };

    if plan.is_empty() {
        println!("stamp: no pending increments — nothing to do");
        return ExitCode::SUCCESS;
    }

    println!("stamp: from v{}", plan.base_version);
    let mut adr_number = plan.first_adr_number;
    for inc in &plan.increments {
        println!("  {}  →  v{}", inc.file, inc.version);
        println!("      changelog: {}", inc.changelog);
        for adr in &inc.adrs {
            println!("      ADR {adr_number:04}-{}: {}", adr.slug, adr.title);
            adr_number += 1;
        }
    }

    if !apply {
        println!(
            "stamp: dry run — pass --apply to write (manifests would end on v{})",
            plan.final_version()
        );
        return ExitCode::SUCCESS;
    }

    // `--apply` edits tracked files in place. Refuse on a dirty worktree so the
    // debris of an earlier failed run (a half-written changelog, an orphaned ADR,
    // a partial `bump-version.sh`) is resolved first — otherwise a retry could
    // compound it. `apply` itself unwinds its own writes on failure, but a bump
    // script that dies mid-edit is outside its reach; this is the belt to that
    // rollback's suspenders.
    if let Err(e) = ensure_clean_worktree(&root) {
        eprintln!("stamp: {e}");
        return ExitCode::FAILURE;
    }

    // Compute the refreshed greenfield-status table *before* applying, so a
    // table that can't be refreshed fails the stamp with nothing written. The
    // probes read Rust sources and fixtures, none of which the stamp touches, so
    // the pre-apply reading is the post-apply one. Only moves named by the
    // increments being stamped are written; any other drift refuses the stamp
    // rather than being silently absorbed (see `refresh_for_stamp`).
    let acknowledged: std::collections::BTreeSet<String> = plan
        .increments
        .iter()
        .flat_map(|inc| inc.probes.iter().cloned())
        .collect();
    let table_path = xtask::greenfield_status::table_path(&root);
    let refreshed_table = match std::fs::read_to_string(&table_path)
        .map_err(|e| format!("cannot read {}: {e}", table_path.display()))
        .and_then(|committed| {
            xtask::greenfield_status::refresh_for_stamp(
                &committed,
                &xtask::greenfield_status::run_gated(&root),
                &acknowledged,
            )
        }) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("stamp: {e}");
            return ExitCode::FAILURE;
        }
    };

    let pr_number = pr_number_from_head(&root);
    match pr_number {
        Some(n) => println!("stamp: rule-ledger provenance: #{n} (from HEAD's commit message)"),
        None => println!(
            "stamp: rule-ledger provenance: none found (HEAD's commit message has no trailing \
             `(#NNNN)`) — any closes_rule row will have an empty PR cell"
        ),
    }

    match xtask::stamp::apply(&root, &plan, pr_number, |v| run_bump_version(&root, v)) {
        Ok(()) => {
            println!("stamp: applied — manifests on v{}", plan.final_version());
            if let Err(e) = std::fs::write(&table_path, refreshed_table) {
                eprintln!("stamp: failed to rewrite {}: {e}", table_path.display());
                return ExitCode::FAILURE;
            }
            println!("stamp: refreshed gated rows of design/greenfield-status.md");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("stamp: apply failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The PR number that closed the current `HEAD` commit, if it can be recovered
/// from the commit message alone — `git log -1`, no GitHub API call: no token
/// dependency, no rate limit, and (unlike a full-history `--grep` scan, which
/// #999 found needs `fetch-depth: 0`) a single commit's message is always
/// available regardless of checkout depth. GitHub's squash-merge appends
/// `(#NNNN)` to the PR title, which becomes the commit's subject line (`%s`,
/// the first line); this looks for exactly that suffix. Returns `None` on a
/// rebase-merge, a hand-written commit, or any message that doesn't end that
/// way — the ledger just records no PR rather than guessing.
fn pr_number_from_head(root: &Path) -> Option<u32> {
    let out = Command::new("git")
        .args(["log", "-1", "--format=%s"])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let subject = String::from_utf8_lossy(&out.stdout);
    xtask::pr_number_from_subject(subject.trim())
}

/// Run `scripts/bump-version.sh <version>` at the repo root (an absolute path so
/// child-cwd resolution is not a factor).
fn run_bump_version(root: &Path, v: xtask::stamp::Version) -> std::io::Result<()> {
    let script = root.join("scripts/bump-version.sh");
    let status = Command::new(&script)
        .arg(v.to_string())
        .current_dir(root)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{} exited with {status}",
            script.display()
        )))
    }
}

/// Refuse to `--apply` unless the worktree is clean. Runs `git status
/// --porcelain` at `root`; empty output means clean (no tracked modifications
/// and no untracked files — the orphaned ADR a partial run leaves is untracked,
/// so untracked files must count). A git that fails to run is treated as an
/// error rather than a pass: the guard exists precisely to be conservative.
fn ensure_clean_worktree(root: &Path) -> Result<(), String> {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git to check the worktree: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git status failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    if !out.stdout.is_empty() {
        return Err(format!(
            "refusing to --apply on a dirty worktree — commit or discard changes first:\n{}",
            String::from_utf8_lossy(&out.stdout).trim_end()
        ));
    }
    Ok(())
}

/// Run the thirteen greenfield-status probes and print the report. Without `--apply`,
/// this also diffs the nine gated probes against the committed table and exits
/// non-zero on disagreement — the same check `xtask/tests/greenfield_status.rs` runs
/// under CI. With `--apply`, it (re)writes the committed table instead.
fn greenfield_status(apply: bool) -> ExitCode {
    let root = repo_root();
    let report = xtask::greenfield_status::run(&root);

    if apply {
        let table = xtask::greenfield_status::render_table(&report);
        if let Err(e) = std::fs::write(xtask::greenfield_status::table_path(&root), table) {
            eprintln!("greenfield-status: failed to write committed table: {e}");
            return ExitCode::FAILURE;
        }
        println!("greenfield-status: wrote design/greenfield-status.md");
        return ExitCode::SUCCESS;
    }

    for probe in &report.probes {
        println!(
            "  {:<20} {:<10} {}",
            probe.name,
            if probe.gated { "gated" } else { "trend" },
            probe.reads
        );
    }

    let disagreements = xtask::greenfield_status::gated_disagreements_in(&report.probes, &root);
    let acknowledged = xtask::acknowledged_probes(&root);
    for (name, committed, live) in &disagreements {
        if acknowledged.contains(name) {
            println!(
                "greenfield-status: {name} moves {committed:?} -> {live:?} (acknowledged by a \
                 pending file — the stamp rewrites the row on merge)"
            );
        }
    }
    let unacknowledged = xtask::greenfield_status::unacknowledged(disagreements, &acknowledged);
    if unacknowledged.is_empty() {
        println!("greenfield-status: table current (or every move acknowledged)");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "greenfield-status: gated probe(s) moved without acknowledgement — name them in \
             your pending file's `probes` key, or run `cargo xtask greenfield-status --apply`:"
        );
        for (name, committed, live) in &unacknowledged {
            eprintln!("  {name}: committed {committed:?}, live {live:?}");
        }
        ExitCode::FAILURE
    }
}

/// Run the CI gates locally, cheapest first, stopping at the first failure.
///
/// This exists to move a whole class of round trip off CI. The PR gate's
/// critical path is the `Test suite (windows-latest)` leg at ~7 minutes, so a
/// `cargo fmt` slip — detectable in 17 seconds — costs a push, a seven-minute
/// wait, and a second push. The two `--fast` gates below need no
/// compile-and-link and are what the `pre-push` hook in `.githooks/` runs.
///
/// The commands mirror `.github/workflows/ci.yml`'s `fmt`, `clippy` and `test`
/// jobs. They are deliberately NOT asserted equal to the workflow by a drift
/// guard: this is a convenience that front-runs CI, and CI remains the
/// authority. If the two drift, CI still fails and the gate still holds — the
/// only cost is a round trip this command would have saved.
///
/// The test leg runs under nextest's `default` profile, not CI's `ci` profile:
/// that is the documented intent in `.config/nextest.toml` (local runs keep
/// `retries = 0` so a flake surfaces immediately instead of being absorbed).
fn ci(fast: bool) -> ExitCode {
    let root = repo_root();

    let nextest = !fast && have_nextest(&root);
    if !fast && !nextest {
        eprintln!(
            "xtask ci: cargo-nextest not found — falling back to `cargo test`. \
             CI runs nextest; install it with `cargo install cargo-nextest --locked` \
             for a faster and closer-to-CI run."
        );
    }
    let steps = ci_steps(fast, nextest);

    for (name, args) in &steps {
        println!("xtask ci: {name} — cargo {}", args.join(" "));
        let status = Command::new(cargo()).args(args).current_dir(&root).status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("xtask ci: {name} failed ({s})");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("xtask ci: cannot run cargo for {name}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    if fast {
        println!(
            "xtask ci: fast gates passed — the test suite was NOT run (drop --fast to run it)"
        );
    } else {
        println!("xtask ci: all gates passed");
    }
    ExitCode::SUCCESS
}

/// The gates `ci` runs, cheapest first, as `(name, cargo argv)`.
///
/// Split out from [`ci`] so the *selection* is assertable without spawning a
/// single cargo. That matters most for the `--fast` list: it is what the
/// `pre-push` hook execs, so a change that silently dropped clippy from it
/// would be caught only by the CI run the hook exists to pre-empt.
///
/// `nextest` says whether `cargo nextest` is available; the caller probes for
/// it (and reports the fallback) rather than this function, to keep it pure.
/// It is ignored when `fast` is set, since no test step is emitted then.
fn ci_steps(fast: bool, nextest: bool) -> Vec<(&'static str, Vec<&'static str>)> {
    let mut steps: Vec<(&str, Vec<&str>)> = vec![
        ("formatting", vec!["fmt", "--all", "--check"]),
        (
            "clippy",
            vec![
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
    ];

    if !fast {
        // Prefer nextest (what CI runs, and far faster across the suite's 141
        // test binaries), but fall back rather than fail: a contributor who has
        // not installed it should still get a working `cargo xtask ci`.
        steps.push(if nextest {
            ("tests", vec!["nextest", "run", "--workspace", "--locked"])
        } else {
            ("tests", vec!["test", "--workspace", "--locked"])
        });
    }

    steps
}

/// The cargo that invoked us, so a non-default toolchain stays consistent across
/// the child processes. Falls back to plain `cargo` when run outside cargo.
fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn have_nextest(root: &Path) -> bool {
    Command::new(cargo())
        .args(["nextest", "--version"])
        .current_dir(root)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn usage() {
    eprintln!(
        "usage: cargo xtask <check-pending | ci [--fast] | greenfield-status [--apply | --list-unasserted] | manifest-schema [--check] | editor-contract [--check | --tag T --out F] | stamp [--apply]>"
    );
}

#[cfg(test)]
mod tests {
    use super::ci_steps;

    fn names(steps: &[(&str, Vec<&str>)]) -> Vec<String> {
        steps.iter().map(|(n, _)| (*n).to_string()).collect()
    }

    /// The list the `pre-push` hook execs. If clippy ever falls out of it the
    /// hook still exits 0 and still looks like it ran, and the lint failure
    /// resurfaces only on CI — the round trip the hook exists to prevent.
    #[test]
    fn fast_is_exactly_the_two_compile_free_gates() {
        let steps = ci_steps(true, false);
        assert_eq!(names(&steps), ["formatting", "clippy"]);
        assert_eq!(steps[0].1[0], "fmt");
        assert_eq!(steps[1].1[0], "clippy");
    }

    /// `--fast` must not run the suite whether or not nextest is installed —
    /// the nextest probe is skipped entirely on that path.
    #[test]
    fn fast_never_emits_a_test_step() {
        for nextest in [true, false] {
            let steps = ci_steps(true, nextest);
            assert!(
                !names(&steps).contains(&"tests".to_string()),
                "--fast emitted a test step with nextest={nextest}"
            );
        }
    }

    #[test]
    fn full_run_appends_the_suite_after_the_lints() {
        let steps = ci_steps(false, true);
        assert_eq!(names(&steps), ["formatting", "clippy", "tests"]);
    }

    /// Both arms run the whole workspace and both pass `--locked`, so a local
    /// run fails on lockfile drift exactly as CI's `test` job does.
    #[test]
    fn both_test_arms_are_workspace_wide_and_locked() {
        for nextest in [true, false] {
            let steps = ci_steps(false, nextest);
            let (_, args) = steps.last().expect("a test step");
            assert!(args.contains(&"--workspace"), "{args:?} is not --workspace");
            assert!(args.contains(&"--locked"), "{args:?} is not --locked");
        }
    }

    /// The fallback picks a genuinely different subcommand rather than passing
    /// nextest's argv to `cargo test` (which would reject `run`).
    #[test]
    fn the_nextest_fallback_switches_to_cargo_test() {
        assert_eq!(
            ci_steps(false, true).last().unwrap().1,
            ["nextest", "run", "--workspace", "--locked"]
        );
        assert_eq!(
            ci_steps(false, false).last().unwrap().1,
            ["test", "--workspace", "--locked"]
        );
    }

    /// No `--profile ci`: `.config/nextest.toml` keeps `retries = 0` on the
    /// default profile deliberately, so a local flake surfaces immediately
    /// rather than being absorbed the way CI's `ci` profile absorbs it.
    #[test]
    fn the_local_run_does_not_borrow_cis_nextest_profile() {
        let steps = ci_steps(false, true);
        let (_, args) = steps.last().unwrap();
        assert!(
            !args.contains(&"--profile"),
            "local run must use nextest's default profile, got {args:?}"
        );
    }
}
