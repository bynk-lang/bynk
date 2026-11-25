//! `cargo xtask <command>` — Bynk repo automation (increment-allocation track).
//!
//! - `check-pending` (Slice 0) validates the files under `design/pending/`.
//! - `stamp` (Slice 1) assigns the version(s) + ADR number(s) for the pending
//!   files and materialises them; dry-run by default, `--apply` to write.
//! - `bench` (#3168) measures compile time and editor latency on a synthetic
//!   project; `--record` writes `design/benchmarks/v<version>.json`.
//! - `release-notes <tag>` (#3178) prints the release body's "Upgrading from
//!   vPREV" section, assembled from the Book's `reference/upgrading.md`.
//! - `greenfield-status` (compiler-architecture track, T0.0, #999) runs all nineteen
//!   probes against the tree and prints the report (the fourteen gated ones are what
//!   the committed table is diffed on); `--apply` writes the committed table.
//! - `module-lines` (#4085) lists the back-half source files at or over 2,000 production
//!   lines against their committed ceilings (the ratchet `xtask/tests/module_lines_ratchet.rs`
//!   enforces); `--apply` rewrites `xtask/module-line-ceilings.txt`;
//! - `fn-lengths` (#3167) lists the non-test functions over 300 lines against their
//!   committed ceilings (the ratchet `xtask/tests/fn_length_ratchet.rs` enforces);
//!   `--apply` rewrites `xtask/fn-length-ceilings.txt` from the tree.
//! - `manifest-schema` (#2096) writes `bynk.toml`'s JSON Schema, generated from
//!   the manifest's Rust types; `--check` fails when the committed one is stale.
//! - `editor-contract` writes the compiler-owned facts the VS Code extension
//!   checks itself against (reserved keywords, semantic-token legend);
//!   `--check` fails when the committed snapshot is stale, `--tag T --out F`
//!   writes the release asset.
//! - `bench-runtime` (#3177) measures the run-time cost of compiled Bynk against
//!   hand-written TypeScript twins (bundle size, start-up, request latency),
//!   locally under workerd; `--record` writes `design/benchmarks/runtime/v<version>.json`.
//! - `tier-probe` (C4b, #2940) fails when a test is in no test tier, or in two.
//! - `ci` runs the gates CI runs, locally, in CI's own cheapest-first order —
//!   `--fast` stops before the test suite (fmt, clippy, rustdoc).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check-pending") => check_pending(),
        Some("sorted-lists") => sorted_lists(),
        Some("greenfield-status") => {
            let mut rest = args[1..].iter();
            let mut unknown = None;
            while let Some(a) = rest.next() {
                match a.as_str() {
                    "--apply" | "--list-unasserted" | "--trend-only" | "--check-growth" => {}
                    "--base" => {
                        rest.next();
                    }
                    _ => {
                        unknown = Some(a);
                        break;
                    }
                }
            }
            if let Some(bad) = unknown {
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
            if args.iter().any(|a| a == "--check-growth") {
                return greenfield_check_growth(&args[1..]);
            }
            let apply = args.iter().any(|a| a == "--apply");
            if args.iter().any(|a| a == "--trend-only") {
                return greenfield_trend_only(apply);
            }
            greenfield_status(apply)
        }
        Some("fn-lengths") => {
            if let Some(bad) = args[1..].iter().find(|a| *a != "--apply") {
                eprintln!("xtask fn-lengths: unknown argument {bad:?}");
                usage();
                return ExitCode::from(2);
            }
            let root = repo_root();
            let measured = xtask::fn_length::measure(&root);
            if args.iter().any(|a| a == "--apply") {
                let path = root.join(xtask::fn_length::CEILINGS_FILE);
                if let Err(e) = std::fs::write(&path, xtask::fn_length::render_ceilings(&measured))
                {
                    eprintln!("xtask fn-lengths: cannot write {}: {e}", path.display());
                    return ExitCode::FAILURE;
                }
                println!("wrote {} ({} functions)", path.display(), measured.len());
                return ExitCode::SUCCESS;
            }
            let ceilings = xtask::fn_length::read_ceilings(&root);
            for f in &measured {
                let ceiling = ceilings
                    .iter()
                    .find(|(k, _)| *k == f.key())
                    .map(|(_, c)| *c);
                match ceiling {
                    Some(c) => println!("{:>6}  (ceiling {c})  {}", f.lines, f.key()),
                    None => println!("{:>6}  (no ceiling)  {}", f.lines, f.key()),
                }
            }
            ExitCode::SUCCESS
        }
        Some("module-lines") => {
            if let Some(bad) = args[1..].iter().find(|a| *a != "--apply") {
                eprintln!("xtask module-lines: unknown argument {bad:?}");
                usage();
                return ExitCode::from(2);
            }
            let root = repo_root();
            let measured = xtask::module_lines::measure(&root);
            if args.iter().any(|a| a == "--apply") {
                let path = root.join(xtask::module_lines::CEILINGS_FILE);
                if let Err(e) =
                    std::fs::write(&path, xtask::module_lines::render_ceilings(&measured))
                {
                    eprintln!("xtask module-lines: cannot write {}: {e}", path.display());
                    return ExitCode::FAILURE;
                }
                println!("wrote {} ({} files)", path.display(), measured.len());
                return ExitCode::SUCCESS;
            }
            let ceilings = xtask::module_lines::read_ceilings(&root);
            for m in &measured {
                match ceilings.iter().find(|(k, _)| *k == m.file) {
                    Some((_, c)) => println!("{:>6}  (ceiling {c})  {}", m.lines, m.file),
                    None => println!("{:>6}  (no ceiling)  {}", m.lines, m.file),
                }
            }
            ExitCode::SUCCESS
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
        Some("bench") => bench(&args[1..]),
        Some("release-notes") => release_notes(&args[1..]),
        Some("release-page") => {
            // Dry-run by default, like `stamp`: a typo'd flag must not read as
            // "no --apply given" when the maintainer meant to write.
            if let Some(bad) = args[1..].iter().find(|a| *a != "--apply") {
                eprintln!("xtask release-page: unknown argument {bad:?}");
                usage();
                return ExitCode::from(2);
            }
            release_page(args.iter().any(|a| a == "--apply"))
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
        Some("tier-probe") => {
            if args.len() > 1 {
                eprintln!("xtask tier-probe: takes no arguments");
                usage();
                return ExitCode::from(2);
            }
            match xtask::tiers::probe(&repo_root()) {
                Ok(summary) => {
                    println!("tier-probe: {summary}");
                    ExitCode::SUCCESS
                }
                Err(problems) => {
                    eprintln!("tier-probe failed:\n  {}", problems.join("\n  "));
                    ExitCode::FAILURE
                }
            }
        }
        Some("bench-runtime") => bench_runtime(&args[1..]),
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

/// #3177: run-time benchmarks of compiled Bynk against hand-written TypeScript
/// twins (`benchmarks/runtime/`). Builds a release `bynk`, installs the
/// harness's pinned npm dependencies (no lockfile, nothing committed), and runs
/// `bench.mjs`. `--sizes-only` skips workerd and is deterministic; `--record`
/// writes `design/benchmarks/runtime/v<version>.json`; `--runs`, `--requests`
/// and `--startups` set the sample sizes.
fn bench_runtime(args: &[String]) -> ExitCode {
    let mut pass = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sizes-only" => pass.push("--sizes-only".to_string()),
            "--record" => {
                pass.push("--record".to_string());
                pass.push("design/benchmarks/runtime".to_string());
            }
            "--runs" | "--requests" | "--startups" => {
                let Some(value) = it.next().filter(|v| v.parse::<u32>().is_ok_and(|n| n > 0))
                else {
                    eprintln!("xtask bench-runtime: {a} needs a positive whole number");
                    usage();
                    return ExitCode::from(2);
                };
                pass.push(a.clone());
                pass.push(value.clone());
            }
            other => {
                eprintln!("xtask bench-runtime: unknown argument {other:?}");
                usage();
                return ExitCode::from(2);
            }
        }
    }
    if pass.iter().any(|a| a == "--sizes-only") && pass.iter().any(|a| a == "--record") {
        eprintln!(
            "xtask bench-runtime: --sizes-only cannot be recorded (it would replace the full record with a partial one)"
        );
        usage();
        return ExitCode::from(2);
    }
    let root = repo_root();
    let dir = root.join("benchmarks/runtime");
    let steps: [(&str, Command); 2] = [
        ("cargo build --release -p bynk", {
            let mut c = Command::new("cargo");
            c.args(["build", "--release", "-p", "bynk"])
                .current_dir(&root);
            c
        }),
        ("npm install", {
            let mut c = Command::new("npm");
            c.args(["install", "--no-package-lock", "--no-audit", "--no-fund"])
                .current_dir(&dir);
            c
        }),
    ];
    for (label, mut cmd) in steps {
        match cmd.status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("xtask bench-runtime: `{label}` failed ({status})");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("xtask bench-runtime: could not run `{label}`: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    let status = Command::new("node")
        .arg("bench.mjs")
        .args(&pass)
        .current_dir(&dir)
        .status();
    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => {
            eprintln!("xtask bench-runtime: bench.mjs failed ({status})");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("xtask bench-runtime: could not run node: {e}");
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
    for warning in xtask::check_warnings_in(&xtask::repo_root()) {
        eprintln!("{warning}");
    }
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

/// `sorted-lists`: every list the Book marks as alphabetical (`<!-- sorted: … -->`) is in order,
/// or this prints each entry that is not, and the one it should follow (#4206). Report only.
fn sorted_lists() -> ExitCode {
    let problems = xtask::sorted_lists::check_site(&repo_root());
    if problems.is_empty() {
        println!("sorted-lists: every marked list is in alphabetical order");
        return ExitCode::SUCCESS;
    }
    for p in &problems {
        eprintln!("{p}");
    }
    eprintln!(
        "sorted-lists: {} problem(s). For an entry out of order, move the whole entry (a table row, \
         or a heading with its section) to the place named, and do not rewrite its text; for a \
         marker problem, fix the `<!-- sorted -->` marker the message names.",
        problems.len()
    );
    ExitCode::FAILURE
}

fn stamp(apply: bool) -> ExitCode {
    let root = repo_root();
    let mut plan = match xtask::stamp::plan(&root) {
        Ok(p) => p,
        Err(errors) => {
            for e in &errors {
                eprintln!("{e}");
            }
            eprintln!("stamp: {} error(s) in design/pending/", errors.len());
            return ExitCode::FAILURE;
        }
    };

    if plan.has_nothing() {
        println!("stamp: no pending increments — nothing to do");
        return ExitCode::SUCCESS;
    }

    for stale in &plan.already_stamped {
        println!(
            "stamp: {} was already stamped as v{} — it gets no new version",
            stale.file, stale.entry.version
        );
    }
    if plan.is_empty() {
        // Only stale copies: with `--apply` they are deleted, and nothing else changes.
        if !apply {
            println!("stamp: dry run — pass --apply to delete the stale copies");
            return ExitCode::SUCCESS;
        }
        return match xtask::stamp::apply(&root, &plan, None, |_| Ok(())) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("stamp: apply failed: {e}");
                ExitCode::FAILURE
            }
        };
    }

    println!("stamp: from v{}", plan.base_version);
    let mut adr_number = plan.first_adr_number;
    for inc in &plan.increments {
        match inc.pr {
            Some(pr) => println!("  {} (#{pr})  →  v{}", inc.file, inc.version),
            None => println!("  {}  →  v{}", inc.file, inc.version),
        }
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
    // The raise ledger's from/to: the committed row before this stamp, and the
    // live reading it commits (see `stamp::raise_ledger_rows`).
    let live = xtask::greenfield_status::run_gated(&root);
    let refreshed_table = match std::fs::read_to_string(&table_path)
        .map_err(|e| format!("cannot read {}: {e}", table_path.display()))
        .and_then(|committed| {
            // Only when an increment declares `raises:`, so an unrelated stamp
            // cannot be wedged by a growth-row problem.
            if plan.increments.iter().any(|inc| !inc.raises.is_empty()) {
                plan.growth_rises = xtask::greenfield_status::growth_rises(&live, &committed)
                    .map_err(|errs| errs.join("\n"))?;
            }
            xtask::greenfield_status::refresh_for_stamp(&committed, &live, &acknowledged)
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

/// `greenfield-status --check-growth [--base <rev>]`: the growth-direction gate
/// against the committed table at the base revision (default: the merge base of
/// `HEAD` and `origin/main`). See `greenfield_status::check_growth`. Errors,
/// rather than passes, when no base can be determined.
fn greenfield_check_growth(args: &[String]) -> ExitCode {
    let root = repo_root();
    let base = match args.iter().position(|a| a == "--base") {
        Some(i) => match args.get(i + 1) {
            Some(rev) => rev.clone(),
            None => {
                eprintln!("greenfield-status --check-growth: --base needs a revision");
                return ExitCode::from(2);
            }
        },
        None => match git_stdout(&root, &["merge-base", "HEAD", "origin/main"]) {
            Ok(sha) => sha.trim().to_string(),
            Err(e) => {
                eprintln!(
                    "greenfield-status --check-growth: no base revision (pass --base <rev>, or \
                     fetch origin/main so `git merge-base HEAD origin/main` works): {e}"
                );
                return ExitCode::FAILURE;
            }
        },
    };
    let spec = format!("{base}:design/greenfield-status.md");
    let base_table = match git_stdout(&root, &["show", &spec]) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("greenfield-status --check-growth: cannot read {spec}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let live = xtask::greenfield_status::run_gated(&root);
    match xtask::greenfield_status::check_growth(&live, &base_table, &root) {
        Ok(()) => {
            println!("greenfield-status: no growth probe rose against {base}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("greenfield-status --check-growth (against {base}): {e}");
            ExitCode::FAILURE
        }
    }
}

/// `git <args>` at `root`, stdout on success, or why it failed.
fn git_stdout(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `greenfield-status --trend-only`: run only the five trend probes (including the
/// workspace-wide clippy pass), print them, and with `--apply` rewrite just the
/// trend rows of the committed table ([`xtask::greenfield_status::refresh_trend_rows`]).
/// Exits non-zero, writing nothing, if a probe failed to measure or a row is missing.
fn greenfield_trend_only(apply: bool) -> ExitCode {
    let root = repo_root();
    let probes = xtask::greenfield_status::run_trend(&root);
    for probe in &probes {
        println!("  {:<20} trend      {}", probe.name, probe.reads);
    }
    let path = xtask::greenfield_status::table_path(&root);
    let committed = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("greenfield-status: cannot read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let refreshed = match xtask::greenfield_status::refresh_trend_rows(&committed, &probes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("greenfield-status: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !apply {
        println!("greenfield-status: dry run (pass --apply to write the trend rows)");
        return ExitCode::SUCCESS;
    }
    if refreshed == committed {
        println!("greenfield-status: trend rows already current");
        return ExitCode::SUCCESS;
    }
    if let Err(e) = std::fs::write(&path, refreshed) {
        eprintln!("greenfield-status: failed to write committed table: {e}");
        return ExitCode::FAILURE;
    }
    println!("greenfield-status: refreshed trend rows of design/greenfield-status.md");
    ExitCode::SUCCESS
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
/// wait, and a second push. The `--fast` gates below (fmt, clippy, docs) run no
/// tests and are what the `pre-push` hook in `.githooks/` runs.
///
/// The commands mirror `.github/workflows/ci.yml`'s `fmt`, `clippy`, `api-docs` and
/// `test` jobs. They are deliberately NOT asserted equal to the workflow by a drift
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

    for (name, args, env) in &steps {
        println!("xtask ci: {name} — cargo {}", args.join(" "));
        let status = Command::new(cargo())
            .args(args)
            .envs(env.iter().copied())
            .current_dir(&root)
            .status();
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

/// One `ci` gate: `(name, cargo argv, extra env for that child only)`.
type Step = (
    &'static str,
    Vec<&'static str>,
    Vec<(&'static str, &'static str)>,
);

/// The gates `ci` runs, cheapest first, as `(name, cargo argv, env)`.
///
/// Split out from [`ci`] so the *selection* is assertable without spawning a
/// single cargo. That matters most for the `--fast` list: it is what the
/// `pre-push` hook execs, so a change that silently dropped clippy from it
/// would be caught only by the CI run the hook exists to pre-empt.
///
/// `nextest` says whether `cargo nextest` is available; the caller probes for
/// it (and reports the fallback) rather than this function, to keep it pure.
/// It is ignored when `fast` is set, since no test step is emitted then.
fn ci_steps(fast: bool, nextest: bool) -> Vec<Step> {
    let mut steps: Vec<Step> = vec![
        ("formatting", vec!["fmt", "--all", "--check"], vec![]),
        // #4206: the Reference's marked lists stay alphabetical. Cheap, so before the builds.
        (
            "sorted lists",
            vec!["run", "--locked", "-q", "-p", "xtask", "--", "sorted-lists"],
            vec![],
        ),
        // #3174: the examples are the corpus newcomers read, so they must be what
        // `bynk fmt` produces. (CI enforces the same via xtask's
        // `examples_fmt_clean` test.)
        (
            "examples formatting",
            vec![
                "run", "--locked", "-q", "-p", "bynk", "--", "fmt", "--check", "examples",
            ],
            vec![],
        ),
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
            vec![],
        ),
        // CI's `API docs (cargo doc)` job: `-D warnings` makes a broken
        // intra-doc link fail. Set on this child only, never process-wide, and
        // it deliberately REPLACES any RUSTDOCFLAGS the caller exported, to match CI.
        (
            "docs",
            vec!["doc", "--no-deps", "--workspace"],
            vec![("RUSTDOCFLAGS", "-D warnings")],
        ),
    ];

    if !fast {
        // Prefer nextest (what CI runs, and far faster across the suite's 141
        // test binaries), but fall back rather than fail: a contributor who has
        // not installed it should still get a working `cargo xtask ci`.
        steps.push(if nextest {
            (
                "tests",
                vec!["nextest", "run", "--workspace", "--locked"],
                vec![],
            )
        } else {
            ("tests", vec!["test", "--workspace", "--locked"], vec![])
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

/// `bench`: see [`xtask::bench`]. Local only; results are recorded per release,
/// never gated in CI (shared runners are too noisy for a threshold).
fn bench(args: &[String]) -> ExitCode {
    let mut opts = xtask::bench::Options::default();
    let mut record = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |what: &str| -> Option<String> {
            let v = it.next().cloned();
            if v.is_none() {
                eprintln!("xtask bench: {what} needs a value");
            }
            v
        };
        match a.as_str() {
            "--record" => record = true,
            "--sizes" => {
                let Some(v) = value("--sizes") else {
                    return ExitCode::from(2);
                };
                let sizes: Option<Vec<usize>> = v.split(',').map(|n| n.parse().ok()).collect();
                match sizes {
                    Some(s) if !s.is_empty() && s.iter().all(|n| *n > 0) => opts.sizes = s,
                    _ => {
                        eprintln!("xtask bench: --sizes takes positive integers, e.g. 10,50,200");
                        return ExitCode::from(2);
                    }
                }
            }
            "--shape" => {
                let Some(v) = value("--shape") else {
                    return ExitCode::from(2);
                };
                opts.shapes = match v.as_str() {
                    "independent" => vec![xtask::bench::Shape::Independent],
                    "linked" => vec![xtask::bench::Shape::Linked],
                    "both" => vec![
                        xtask::bench::Shape::Independent,
                        xtask::bench::Shape::Linked,
                    ],
                    _ => {
                        eprintln!("xtask bench: --shape is independent, linked or both");
                        return ExitCode::from(2);
                    }
                };
            }
            "--runs" | "--edits" => {
                let Some(v) = value(a) else {
                    return ExitCode::from(2);
                };
                let Some(n) = v.parse::<usize>().ok().filter(|n| *n > 0) else {
                    eprintln!("xtask bench: {a} takes a positive integer");
                    return ExitCode::from(2);
                };
                if a == "--runs" {
                    opts.runs = n;
                } else {
                    opts.edits = n;
                }
            }
            bad => {
                eprintln!("xtask bench: unknown argument {bad:?}");
                usage();
                return ExitCode::from(2);
            }
        }
    }
    // A record is read back by the `keystroke_latency` probe, which needs the
    // independent shape's `sizes`; a record of one shape would shadow earlier
    // complete ones. Measure one shape freely, but record only both.
    if record && opts.shapes.len() != 2 {
        eprintln!(
            "xtask bench: --record needs both shapes (--shape both), so the record is complete"
        );
        return ExitCode::from(2);
    }
    let root = repo_root();
    let result = match xtask::bench::run(&root, &opts, &|m| eprintln!("bench: {m}")) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("bench: {e}");
            return ExitCode::FAILURE;
        }
    };
    let text = serde_json::to_string_pretty(&result).expect("serialises") + "\n";
    // The accepted budget is a per-release check, not a CI gate: say so loudly,
    // but still print and (with --record) write the result.
    if let Some(warning) = xtask::bench::budget_warning(&result) {
        eprintln!("bench: WARNING: {warning}");
    }
    if let Some(warning) = xtask::bench::trigger_warning(&result) {
        eprintln!("bench: WARNING: {warning}");
    }
    if record {
        let version = result["version"].as_str().unwrap_or("unknown");
        let path = xtask::bench::record_path(&root, version);
        if let Err(e) = std::fs::create_dir_all(path.parent().expect("has parent"))
            .and_then(|()| std::fs::write(&path, &text))
        {
            eprintln!("bench: cannot write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        eprintln!("bench: recorded {}", path.display());
    }
    print!("{text}");
    ExitCode::SUCCESS
}

/// `release-notes <tag>`: the "Upgrading from vPREV" section for `tag`, on
/// stdout. The previous release is the greatest plain `v*` tag below `tag` in
/// this clone (the release workflow checks out full history and tags).
fn release_notes(args: &[String]) -> ExitCode {
    let [tag] = args else {
        eprintln!("xtask release-notes: expected exactly one argument, the release tag");
        usage();
        return ExitCode::from(2);
    };
    let Some(version) = xtask::release_notes::parse_release_tag(tag) else {
        eprintln!("xtask release-notes: {tag:?} is not a `vX.Y.Z[-suffix]` tag");
        return ExitCode::from(2);
    };
    let root = repo_root();
    let page = match std::fs::read_to_string(root.join(xtask::release_notes::UPGRADING_PAGE)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("release-notes: cannot read the upgrading page: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tags = match Command::new("git")
        .args(["tag", "--list", "v[0-9]*"])
        .current_dir(&root)
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        _ => {
            eprintln!("release-notes: `git tag --list` failed");
            return ExitCode::FAILURE;
        }
    };
    let prev = xtask::release_notes::previous_tag(&tags, version);
    print!(
        "{}",
        xtask::release_notes::release_section(&page, prev, version)
    );
    ExitCode::SUCCESS
}

/// `release-page [--apply]`: cut the Book's Unreleased page into the page for
/// the workspace's current version, before that version is tagged (#3868).
/// Without `--apply`, says what it would write.
fn release_page(apply: bool) -> ExitCode {
    use xtask::release_pages::{
        INDEX_PAGE, SHIPPED_CODES, UNRELEASED_PAGE, cut, release_page, today_utc,
        with_shipped_codes,
    };
    let root = repo_root();
    let read = |rel: &str| {
        std::fs::read_to_string(root.join(rel)).map_err(|e| format!("cannot read {rel}: {e}"))
    };
    let result = (|| -> Result<(), String> {
        let version = xtask::stamp::parse_workspace_version(&read("Cargo.toml")?)
            .ok_or("Cargo.toml has no [workspace.package] version")?;
        let page = release_page(version);
        if root.join(&page).exists() {
            return Err(format!(
                "{page} already exists: v{version} has been cut. Merge a change that bumps \
                 the version before cutting the next release."
            ));
        }
        let cut = cut(
            &read(UNRELEASED_PAGE)?,
            &read(INDEX_PAGE)?,
            version,
            &today_utc(),
        )?;
        let rows = cut
            .release
            .lines()
            .filter(|l| l.starts_with("| **v"))
            .count();
        if !apply {
            println!(
                "Would cut v{version} ({rows} increments): write {page}, empty {UNRELEASED_PAGE} \
                 and add the release to {INDEX_PAGE}, and add the registry's codes to {SHIPPED_CODES}.\nRun again with --apply to write them."
            );
            return Ok(());
        }
        let write = |rel: &str, text: &str| {
            std::fs::write(root.join(rel), text).map_err(|e| format!("cannot write {rel}: {e}"))
        };
        write(&page, &cut.release)?;
        write(UNRELEASED_PAGE, &cut.unreleased)?;
        write(INDEX_PAGE, &cut.index)?;
        let live: Vec<&str> = bynk_syntax::diagnostics::REGISTRY
            .iter()
            .map(|d| d.code)
            .collect();
        write(
            SHIPPED_CODES,
            &with_shipped_codes(&read(SHIPPED_CODES)?, &live),
        )?;
        println!(
            "Cut v{version} ({rows} increments) into {page}.\n\
             Commit it, merge it, then tag that merge commit v{version}."
        );
        Ok(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("release-page: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "usage: cargo xtask <check-pending | sorted-lists | ci [--fast] | fn-lengths [--apply] | module-lines [--apply] | greenfield-status [--apply | --list-unasserted | --check-growth [--base REV] | --trend-only [--apply]] | manifest-schema [--check] | editor-contract [--check | --tag T --out F] | stamp [--apply] | release-notes <vX.Y.Z> | release-page [--apply] | bench [--sizes N,N,…] [--shape independent|linked|both] [--runs N] [--edits N] [--record] | bench-runtime [--sizes-only] [--record] [--runs N] [--requests N] [--startups N] | tier-probe>"
    );
}

#[cfg(test)]
mod tests {
    use super::{Step, ci_steps};

    fn names(steps: &[Step]) -> Vec<String> {
        steps.iter().map(|(n, _, _)| (*n).to_string()).collect()
    }

    /// The list the `pre-push` hook execs. If clippy ever falls out of it the
    /// hook still exits 0 and still looks like it ran, and the lint failure
    /// resurfaces only on CI — the round trip the hook exists to prevent.
    #[test]
    fn fast_is_exactly_the_non_test_gates() {
        let steps = ci_steps(true, false);
        assert_eq!(
            names(&steps),
            [
                "formatting",
                "sorted lists",
                "examples formatting",
                "clippy",
                "docs"
            ]
        );
        assert_eq!(steps[0].1[0], "fmt");
        assert_eq!(steps[3].1[0], "clippy");
        assert_eq!(steps[4].1[0], "doc");
    }

    /// Matches CI's `API docs (cargo doc)` job, in both `--fast` and the full
    /// run, with `-D warnings` scoped to that one child and no other step.
    #[test]
    fn docs_step_matches_ci_and_scopes_its_env() {
        for (fast, nextest) in [(true, false), (false, true), (false, false)] {
            let steps = ci_steps(fast, nextest);
            let docs: Vec<_> = steps.iter().filter(|s| s.0 == "docs").collect();
            assert_eq!(docs.len(), 1, "fast={fast} nextest={nextest}");
            assert_eq!(docs[0].1, ["doc", "--no-deps", "--workspace"]);
            assert_eq!(docs[0].2, [("RUSTDOCFLAGS", "-D warnings")]);
            for s in steps.iter().filter(|s| s.0 != "docs") {
                assert!(s.2.is_empty(), "{} must not set env", s.0);
            }
        }
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
        assert_eq!(
            names(&steps),
            [
                "formatting",
                "sorted lists",
                "examples formatting",
                "clippy",
                "docs",
                "tests"
            ]
        );
    }

    /// Both arms run the whole workspace and both pass `--locked`, so a local
    /// run fails on lockfile drift exactly as CI's `test` job does.
    #[test]
    fn both_test_arms_are_workspace_wide_and_locked() {
        for nextest in [true, false] {
            let steps = ci_steps(false, nextest);
            let (_, args, _) = steps.last().expect("a test step");
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
        let (_, args, _) = steps.last().unwrap();
        assert!(
            !args.contains(&"--profile"),
            "local run must use nextest's default profile, got {args:?}"
        );
    }
}
