//! `cargo xtask greenfield-status` — the probe harness (track doc §8, proposal #999).
//!
//! Twenty-one probes measuring the tree against `design/bynk-greenfield-compiler.md`:
//! the twelve in track doc §8, `emit_abi_shapes` (ADR 0310's probe, #999 Decision E —
//! this slice measures the emit-ABI enumeration guard but does not wire it; wiring is
//! packaging-track work), phase 7's own four — `ts_writes`, `ts_any`,
//! `verbatim_origins`, `verbatim_sites` (P7.0/#1296, P7.5/#1307 — see phase 7's own
//! closing summary in `design/archive/retired-tracks.md`) — and phase 8's own two,
//! `incremental_query_types` and `keystroke_latency` (P8.0/#1510, settled by #1509's
//! Q5/ADR 0414 — see `design/tracks/incrementality.md` §5), plus the IR cutover
//! track's own adoption probe, `unconsumed_ir_items` (Slice D3 of #1542 — the gate the
//! 30 August 2026 post-restructuring review's Part 5 §8 asked for), and the
//! runtime-semantics track's `diagnostic_coverage` (#1662, G2 of #1648).
//!
//! **Sixteen are gated**, committed and diffed: `workspace_lints`, `fs_below_driver`,
//! `options_sources`, `hoist_sinks`, `span_keyed_maps`, `emit_diagnostics`,
//! `ide_emit_edge`, `ast_importers`, `emit_abi_shapes`, `ts_writes`, `ts_any`,
//! `verbatim_origins`, `verbatim_sites`, `incremental_query_types`,
//! `unconsumed_ir_items`, `diagnostic_coverage`. Ten of these are
//! zero/closure-shaped — a boolean, or a count pinned at a small, argued floor
//! (`ast_importers` = 5, `emit_abi_shapes` = 1). Phase 7's own four are the same shape:
//! each converged toward an argued floor over dozens of slices, the same trajectory
//! `ast_importers` had throughout phase 6's 59 — retired at `ts_writes` = 809,
//! `ts_any` = 26, `verbatim_origins` = 1, `verbatim_sites` = 2 (phase 7's own closing
//! summary, `design/archive/retired-tracks.md`), none the literal 0 first proposed —
//! gated throughout despite the churn that implies, a deliberate call argued in ADR
//! 0389/ADR 0390 (review of #1297), not an oversight of #999 Decision D's
//! churn-avoidance principle. `incremental_query_types` is a different shape again —
//! phase 8's own probe reads a one-time existence proof, not a count trending toward a
//! floor (settled §5/Q5, ADR 0414), re-settled by #1537 to certify that the two
//! levels that never found a consumer stay deleted; see its own doc comment. A
//! disagreement between a
//! fresh run and the committed table fails `greenfield_status_table_is_current`
//! (`xtask/tests/greenfield_status.rs`), which rides both the `test` job (`cargo test
//! --workspace`, any Rust-touching PR) and the `drift` job's existing `cargo test -p
//! xtask` (pending/decisions-only PRs) — no new CI wiring (#999 Decision D, which also
//! explains why a `drift`-job *step* would have been silently skipped on the PRs that
//! move these probes most).
//!
//! A disagreement a pending file acknowledges (its `probes` key) does not fail the
//! gate: the increment leaves the shared table alone and `cargo xtask stamp --apply`
//! rewrites the gated rows on `main` in merge order ([`refresh_gated_rows`]). The
//! acknowledgement still has to be written, and reviewed, in the PR — a gated probe
//! still cannot move silently — but parallel PRs no longer conflict on these rows.
//!
//! **Five are count/ratio trend probes**, recomputed and printed but never diffed:
//! `wildcard_arms`, `keep_in_sync`, `test_density`, `fixture_kinds`,
//! `keystroke_latency`. The first four move on nearly any ordinary Rust PR with no
//! slice actively driving them toward a floor (§8 calls two of them "trends, not
//! gates"); hard-gating them would make the committed table churn, and conflict, on
//! routine work for no corresponding benefit. `keystroke_latency` moves on nothing
//! yet — settled (Q3/ADR 0414) as staying "not measured" for phase 8's own whole
//! lifetime, since no scheduler ships this phase to produce a real number; reported
//! anyway so the trajectory's own §3.0 baseline has a live, CI-computed row.
//!
//! `Closes-Rule:` rule-id provenance (#999 Decision B) is deferred to a follow-on
//! slice — the committed table below carries no rule-citation column yet.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One probe's result. `gated` probes are diffed against the committed table by
/// [`crate::greenfield_status::gated_disagreements`]; the rest are reported only.
pub struct Probe {
    pub name: &'static str,
    pub gated: bool,
    pub reads: String,
}

pub struct Report {
    pub probes: Vec<Probe>,
}

impl Report {
    pub fn get(&self, name: &str) -> &str {
        self.probes
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.reads.as_str())
            .unwrap_or_else(|| panic!("no probe named {name:?}"))
    }
}

/// Run every probe against the tree rooted at `root` (the repo root). Used by the CLI's
/// full report; the gating test uses the sixteen gated probes alone
/// ([`gated_disagreements`]) so it never pays for a workspace-wide clippy pass
/// (`wildcard_arms`) just to check the probes that are actually diffed.
pub fn run(root: &Path) -> Report {
    let mut probes = run_gated(root);
    probes.extend(run_trend(root));
    Report { probes }
}

/// The gated probes' names, in [`run_gated`]'s order — what a pending file's
/// `probes` key may cite. `greenfield_status_table_is_current` asserts this
/// matches the probes [`run_gated`] actually returns, so the two cannot drift.
pub const GATED_PROBES: [&str; 16] = [
    "workspace_lints",
    "fs_below_driver",
    "options_sources",
    "hoist_sinks",
    "span_keyed_maps",
    "emit_diagnostics",
    "ide_emit_edge",
    "ast_importers",
    "emit_abi_shapes",
    "ts_writes",
    "ts_any",
    "verbatim_origins",
    "verbatim_sites",
    "incremental_query_types",
    "unconsumed_ir_items",
    "diagnostic_coverage",
];

/// The sixteen gated (zero/closure) probes only — what [`gated_disagreements`] diffs.
pub fn run_gated(root: &Path) -> Vec<Probe> {
    vec![
        workspace_lints(root),
        fs_below_driver(root),
        options_sources(root),
        hoist_sinks(root),
        span_keyed_maps(root),
        emit_diagnostics(root),
        ide_emit_edge(root),
        ast_importers(root),
        emit_abi_shapes(root),
        ts_writes(root),
        ts_any(root),
        verbatim_origins(root),
        verbatim_sites(root),
        incremental_query_types(root),
        unconsumed_ir_items(root),
        diagnostic_coverage(root),
    ]
}

/// The five reported-only trend probes — never diffed, and notably including the one
/// (`wildcard_arms`) that shells out to a full `cargo clippy --workspace` pass, which
/// the gating test must not pay for on every run.
fn run_trend(root: &Path) -> Vec<Probe> {
    vec![
        wildcard_arms(root),
        keep_in_sync(root),
        test_density(root),
        fixture_kinds(root),
        keystroke_latency(root),
    ]
}

/// `design/greenfield-status.md` — the committed table this probe set regenerates.
pub fn table_path(root: &Path) -> PathBuf {
    root.join("design/greenfield-status.md")
}

// --- Filesystem helpers --------------------------------------------------

/// Every `.rs` file under `dir`, recursively, as `(path, contents)`. Unreadable files
/// (permissions, non-UTF-8) are skipped rather than failing the whole walk — this is a
/// measurement tool, not a build step.
fn rust_files(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && let Ok(contents) = std::fs::read_to_string(&path)
        {
            out.push((path, contents));
        }
    }
}

/// The inner text of every **standalone** `"bynk.<ident>"` string literal (the
/// `bynk.*` convention used for diagnostic codes and commons/namespace paths alike).
///
/// Standalone, not merely prefix-matching: the identifier run must be immediately
/// followed by the closing quote, matching the naive `rg -o '"bynk\.[a-zA-Z0-9_.]*"'`
/// this probe is deliberately more careful than (#999 Decision A). Without that
/// requirement this would also match the *start* of an unrelated, longer message that
/// merely happens to begin with "bynk." — e.g. a panic string
/// `"bynk.map itself uses bynk.list, so list must be injected too: {paths:?}"` is prose
/// beginning with a namespace-shaped word, not a `"bynk.map"` code literal, and a
/// dev-only compile-time error message split across lines with a `\`-continuation
/// (`"bynk.emit.unresolved_cross_context_signature: no signature for \` ...) is one
/// string, not a diagnostic-code literal, even though its first segment matches the
/// identifier charset. Both were found — and wrongly counted — by an earlier,
/// less careful version of this scan; the fix is requiring the closing quote.
fn bynk_dotted_literals(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while let Some(rel) = src[i..].find("\"bynk.") {
        let start = i + rel + 1; // skip the opening quote
        let mut end = start;
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_' || bytes[end] == b'.')
        {
            end += 1;
        }
        if end < bytes.len() && bytes[end] == b'"' {
            out.push(&src[start..end]);
        }
        i = end.max(start + 1);
    }
    out
}

/// True if `line`, trimmed, is a `//` or `///` or `//!` line comment. Doesn't attempt
/// block comments (`/* */`) — none of this codebase's `bynk.*`/dead-identifier
/// mentions live in one.
fn is_line_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

// --- Gated probe 1: workspace_lints --------------------------------------

/// R2.12. `[workspace.lints]` presence and `clippy::wildcard_enum_match_arm`'s level in
/// the root `Cargo.toml`. A boolean-shaped probe (not a count) — gated because it only
/// ever changes once, when T0.3 lands it.
fn workspace_lints(root: &Path) -> Probe {
    let cargo_toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
    let has_section = cargo_toml
        .lines()
        .any(|l| l.trim() == "[workspace.lints.clippy]" || l.trim() == "[workspace.lints]");
    let level = cargo_toml
        .lines()
        .find(|l| l.contains("wildcard_enum_match_arm"))
        .map(|l| l.trim().to_string());
    let reads = match (has_section, level) {
        (true, Some(l)) => format!("present — {l}"),
        (true, None) => "present, wildcard_enum_match_arm not set".to_string(),
        (false, _) => "absent".to_string(),
    };
    Probe {
        name: "workspace_lints",
        gated: true,
        reads,
    }
}

// --- Gated probe 2: fs_below_driver --------------------------------------

/// R2.3. Files under `bynk-emit/src`, `bynk-ide/src`, `bynk-fmt/src` (the crates below
/// the `bynk` driver, which owns disk I/O) that touch `std::fs` in **production** code.
///
/// Excludes usage inside a trailing `#[cfg(test)] mod tests { ... }` block — the
/// convention every file in this codebase uses, always the last item in the file. A
/// line is production-scope unless it falls at or after the line following a
/// `#[cfg(test)]` attribute whose very next non-empty line opens a `mod ... {` block
/// (as opposed to a `mod name;` external-file declaration, which is not a scope at
/// all). This mirrors the comment-exclusion discipline elsewhere in this probe set:
/// tests writing fixtures to a tempdir are not "the driver's job" bypassed, and
/// counting them would report a rule open that the production code has already closed.
///
/// A file counts if its own text names `std::fs` ([`has_production_std_fs`]), **or** if
/// a bare `fs::`-style call site in it resolves to `std::fs` through its imports
/// ([`production_std_fs_files`]) — a module-level `use std::fs;` in a parent module is
/// visible to a child through `use super::*;` (module privacy is ancestor-scoped), so
/// `bynk-emit/src/project/discovery.rs` reads and walks the filesystem while never
/// spelling `std::fs` itself. The literal text scan alone missed exactly that file,
/// so a probe reading `bynk-emit=0` would have asserted R2.3 closed on a false
/// premise (#1013).
///
/// #1104 (a content-ownership (#1086) probe-precision follow-on): a flagged *count*
/// alone can't tell a residual R2.3 violation from a documented, permanent exception —
/// `bynk-emit` read 3 that way from the track's retirement (`design/archive/
/// retired-tracks.md`'s closing summary) until P4.0 moved all three named files out of
/// `bynk-emit` entirely, leaving [`NAMED_FS_EXCEPTIONS`] empty (#1561) — the mechanism
/// stays live for whatever named exception is decided next. So each flagged file is
/// additionally classified as a **named floor** file — every
/// production-scope touch it has is either inside one of those named functions, or is
/// a bare import declaration (no fn encloses it — [`enclosing_fn`] returns `None`) that
/// performs no I/O of its own, existing only so a *descendant* module's bare `fs::`
/// call can resolve (the motivating case, #1013: `project.rs`'s own `use std::fs;`,
/// which `discovery.rs` and `paths.rs` glob-imported via `use super::*;` before P4.0
/// moved both files out) — or a **residual** file: any other file
/// touching `std::fs` in production scope, which still reads as a real R2.3 violation
/// ([`file_is_named_fs_floor`]).
fn fs_below_driver(root: &Path) -> Probe {
    let crates = ["bynk-emit", "bynk-ide", "bynk-fmt"];
    let mut per_crate = Vec::new();
    let mut total = 0usize;
    let mut total_floor = 0usize;
    for krate in crates {
        let dir = root.join(krate).join("src");
        let files: Vec<(PathBuf, String)> = rust_files(&dir)
            .into_iter()
            .map(|(path, contents)| {
                let rel = path.strip_prefix(&dir).unwrap_or(&path).to_path_buf();
                (rel, contents)
            })
            .collect();
        let flagged = production_std_fs_files(&files);
        let count = flagged.len();
        total += count;
        let facts: Vec<FsImportFacts> = files.iter().map(|(_, s)| fs_import_facts(s)).collect();
        let parents: Vec<Option<usize>> = files
            .iter()
            .map(|(p, _)| module_parent(p, &files))
            .collect();
        let floor = flagged
            .iter()
            .filter(|&&i| {
                file_is_named_fs_floor(krate, &files, &facts, &parents, i, NAMED_FS_EXCEPTIONS)
            })
            .count();
        total_floor += floor;
        let residual = count - floor;
        per_crate.push(if floor > 0 {
            format!("{krate}={count} ({floor} named floor, {residual} residual)")
        } else {
            format!("{krate}={count}")
        });
    }
    Probe {
        name: "fs_below_driver",
        gated: true,
        reads: format!(
            "{total} files ({}) — {total_floor} named floor, {} residual total",
            per_crate.join(", "),
            total - total_floor
        ),
    }
}

/// #1104: the specific, permanently-carved-out production functions whose
/// `std::fs` touch is a *named* exception, not evidence of unfinished R2.3
/// migration — settled in `design/tracks/content-ownership.md` §3.2 (retired) and
/// its closing summary in `design/archive/retired-tracks.md`. `(crate, file path
/// relative to that crate's `src/`, enclosing production fn name)`. A future
/// carve-out decided the same deliberate way joins this list; anything touching
/// `std::fs` in production scope that isn't listed here reads as a residual R2.3
/// violation, per [`file_is_named_fs_floor`].
///
/// Empty since #1561: these three entries were real when #1104 (`769a60a3`,
/// 6 Aug 2026) added them, but P4.0 (`69af8f2d`, the very next day) moved
/// `discover_bynk_files`/`read_adapter_binding` (`project/discovery.rs`) and
/// `try_read_project_paths` (`project/paths.rs`) out of `bynk-emit` into
/// `bynk-project` entirely — dead from that point on, unnoticed for weeks.
/// `fs_below_driver`'s `bynk-emit` reading was already 0 with them present
/// (nothing in the tree matched the dead tuples), confirmed unchanged with
/// them gone.
const NAMED_FS_EXCEPTIONS: &[(&str, &str, &str)] = &[];

/// Is flagged file `files[i]` (already known, by [`production_std_fs_files`], to touch
/// `std::fs` in production scope) a **named floor** file — every production-scope touch
/// it has is either inside a [`NAMED_FS_EXCEPTIONS`] function for this exact
/// `(krate, file)`, or a bare `use` import declaration (which reads but performs no
/// filesystem operation by itself, unlike a module-scope `static`/`const` initialiser or
/// macro invocation that might)? `facts`/`parents` are the caller's already-computed
/// [`fs_import_facts`]/[`module_parent`] vectors for `files`, threaded through rather
/// than recomputed per flagged file.
///
/// A single disallowed touch — inside an unlisted fn, inside a listed fn's *file* but
/// wrong *name*, or outside every fn and not a plain import — makes the whole file
/// residual: partial credit isn't meaningful here, since the point is "can a reader stop
/// cross-referencing track docs for this file," not a ratio. Likewise, a file this
/// function attributes *no* touch line to at all (despite the caller already knowing it's
/// flagged — [`line_touches_std_fs`]'s re-implementation of the file-level detection
/// disagreeing with it) reads as residual, not floor: an unattributable touch means this
/// classifier doesn't understand the file, which must fail loud, not quiet.
///
/// `exceptions` is [`NAMED_FS_EXCEPTIONS`] at the one production call site
/// ([`fs_below_driver`]) — parameterised (#1561) so a test can exercise the
/// matching logic against a synthetic tuple instead of real, currently-empty
/// production data, the same way `facts`/`parents` are caller-supplied rather
/// than recomputed.
fn file_is_named_fs_floor(
    krate: &str,
    files: &[(PathBuf, String)],
    facts: &[FsImportFacts],
    parents: &[Option<usize>],
    i: usize,
    exceptions: &[(&str, &str, &str)],
) -> bool {
    let (path, _) = &files[i];
    let rel = path.to_string_lossy().replace('\\', "/");
    let lines: Vec<&str> = files[i].1.lines().collect();
    let ranges = test_mod_ranges(&lines);
    let fn_ranges = production_fn_ranges(&lines, &ranges);

    let mut saw_touch = false;
    for (li, line) in lines.iter().enumerate() {
        if in_test_range(li, &ranges) {
            continue;
        }
        if !line_touches_std_fs(i, line, facts, parents, files) {
            continue;
        }
        saw_touch = true;
        let Some(fn_name) = enclosing_fn(li, &fn_ranges) else {
            // No enclosing fn is harmless only when the line is literally an import
            // declaration. A module-scope `static`/`const` initialiser, a macro
            // invocation, or a fn shape `fn_name_on_line` can't parse (`extern "C" fn`)
            // does real I/O outside every known range and must read as residual.
            if use_declaration(line).is_some() {
                continue;
            }
            return false;
        };
        let named = exceptions
            .iter()
            .any(|&(c, f, func)| c == krate && f == rel && func == fn_name);
        if !named {
            return false;
        }
    }
    saw_touch
}

/// Does `line` (already known to be production-scope) itself touch `std::fs` — by the
/// same two means [`production_std_fs_files`] checks at file granularity, applied here
/// to one line: a literal `std::fs` substring, or a bare/qualified path this line spells
/// that resolves to `std::fs` through file `i`'s visible import bindings.
fn line_touches_std_fs(
    i: usize,
    line: &str,
    facts: &[FsImportFacts],
    parents: &[Option<usize>],
    files: &[(PathBuf, String)],
) -> bool {
    if line.contains("std::fs") {
        return true;
    }
    let mut roots = BTreeSet::new();
    collect_bare_path_roots(line, &mut roots);
    if roots.iter().any(|name| {
        matches!(
            resolve_name_in_module(i, name, facts, parents),
            NameResolution::StdFs
        )
    }) {
        return true;
    }
    let mut chains = BTreeSet::new();
    collect_qualified_paths(line, &mut chains);
    chains
        .iter()
        .any(|chain| qualified_chain_reaches_std_fs(chain, i, facts, parents, files))
}

/// The name and inclusive body line-range of every production-scope `fn` in `lines`
/// (`test_ranges` excluded, same as everywhere else in this probe) — used by
/// [`file_is_named_fs_floor`] to attribute a flagged touch line to its enclosing
/// function. A wrapped signature (the `{` arriving lines after the `fn` line, past a
/// multi-line parameter list) is handled the same way [`test_mod_ranges`] handles a
/// `mod` line: brace depth is tracked starting at the `fn` line itself, but a parameter
/// list has no `{`/`}` in it, so `started` only flips true once the real body-opening
/// brace arrives, however many lines later.
fn production_fn_ranges(
    lines: &[&str],
    test_ranges: &[(usize, usize)],
) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if in_test_range(i, test_ranges) {
            continue;
        }
        let Some(name) = fn_name_on_line(line) else {
            continue;
        };
        let mut state = BraceScanState::Normal;
        let mut depth = 0i32;
        let mut started = false;
        let mut end = lines.len() - 1;
        for (j, l) in lines[i..].iter().enumerate() {
            let (delta, new_state) = brace_delta(l, state);
            state = new_state;
            depth += delta;
            if depth != 0 {
                started = true;
            }
            if started && depth == 0 {
                end = i + j;
                break;
            }
        }
        out.push((name, i, end));
    }
    out
}

/// The leading `fn NAME` on `line`, past an optional `pub`/`pub(...)`, `async`,
/// `unsafe`, `const` modifier run (in any order/repetition, mirroring
/// [`collect_declared_type_name`]'s `pub`-stripping) — `None` if `line` doesn't open a
/// function at all (a call site, a doc comment mentioning "fn", a closure). Doesn't
/// require a trailing `{` or even `(` on this same line — a wrapped signature's `fn`
/// line can end right at the name.
fn fn_name_on_line(line: &str) -> Option<String> {
    let mut t = line.trim();
    loop {
        if let Some(rest) = t.strip_prefix("pub") {
            let rest = rest.trim_start();
            t = if let Some(after_paren) = rest.strip_prefix('(') {
                after_paren.split_once(')')?.1.trim_start()
            } else {
                rest
            };
            continue;
        }
        let mut advanced = false;
        for kw in ["async ", "unsafe ", "const "] {
            if let Some(rest) = t.strip_prefix(kw) {
                t = rest.trim_start();
                advanced = true;
                break;
            }
        }
        if !advanced {
            break;
        }
    }
    let rest = t.strip_prefix("fn ")?;
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_string())
}

/// The innermost [`production_fn_ranges`] entry containing `line_idx`, by name — `None`
/// if `line_idx` sits outside every production fn (module scope: a `use` declaration,
/// a `const`/`static`, or a `struct`/`enum` body).
fn enclosing_fn(line_idx: usize, fn_ranges: &[(String, usize, usize)]) -> Option<String> {
    fn_ranges
        .iter()
        .filter(|(_, start, end)| line_idx >= *start && line_idx <= *end)
        .min_by_key(|(_, start, end)| end - start)
        .map(|(name, _, _)| name.clone())
}

/// The literal text component of [`fs_below_driver`]: some production-scope line names
/// `std::fs`. Necessary but not sufficient (#1013) — a file can touch `std::fs`
/// through a glob-imported parent binding without ever spelling it; that resolution
/// lives in [`production_std_fs_files`], which layers on top of this scan.
fn has_production_std_fs(src: &str) -> bool {
    let lines: Vec<&str> = src.lines().collect();
    let ranges = test_mod_ranges(&lines);
    for (i, line) in lines.iter().enumerate() {
        if in_test_range(i, &ranges) {
            continue;
        }
        if line.contains("std::fs") {
            return true;
        }
    }
    false
}

/// Indices (into `files`, whose paths are relative to the crate's `src/` root) of the
/// files that touch `std::fs` in production code — the union of the literal text scan
/// ([`has_production_std_fs`]) and import resolution: a path whose leading module
/// segment a production `use` declaration binds to `std::fs` (or an item under it),
/// either a bare `NAME::` root resolved in the file itself (`use std::{fs, io};` — a
/// form the substring scan can't see) or in an ancestor module reached through
/// `use super::*;`, transitively (#1013), or a `super::`/`self::`/`crate::`-qualified
/// path walked through the module tree to the same bindings (#1016 review — a
/// descendant may spell `super::fs::read_to_string(p)` with no glob import at all,
/// one disambiguating edit away from a currently-flagged bare call).
///
/// Resolution is Rust-shaped, not hand-tracked (#1013 rejects a special-case list):
/// a private `use std::fs;` in a parent is visible to descendants because module
/// privacy is ancestor-scoped, a chain of `use super::*;` globs re-reaches it from
/// any depth, and a nearer binding of the same name shadows a farther one — whether
/// that binding is another `use` or a locally-declared type-namespace item (`mod fs;`,
/// `struct File`, …; value-namespace items like `fn` can't head a `NAME::` path, so
/// they don't shadow one) — so a child that binds `fs` to something else keeps its
/// bare `fs::` calls unflagged. Visibility is *not* modelled: a path that names a
/// too-private binding wouldn't compile anyway, so over-approximating is safe.
///
/// Known remaining gaps, accepted as out of reach for a text-level scanner: an
/// ancestor's `use std::fs::read_to_string;` item import called bare (`read_to_string(p)`)
/// presents no `::` path segment to resolve — the same import used as a path root
/// (`File::open`) **is** caught, since item bindings under `std::fs` participate in
/// the same resolution — and a `use` declaration rustfmt has split across lines is
/// not parsed. #1013 grepped the three scanned crates for the item-import form, and
/// the #1016 review for the qualified-path and split-declaration forms — zero hits.
fn production_std_fs_files(files: &[(PathBuf, String)]) -> Vec<usize> {
    let facts: Vec<FsImportFacts> = files.iter().map(|(_, src)| fs_import_facts(src)).collect();
    let parents: Vec<Option<usize>> = files
        .iter()
        .map(|(path, _)| module_parent(path, files))
        .collect();
    (0..files.len())
        .filter(|&i| {
            has_production_std_fs(&files[i].1)
                || resolves_bare_std_fs(i, &facts, &parents)
                || resolves_qualified_std_fs(i, &facts, &parents, files)
        })
        .collect()
}

/// Per-file production-scope import facts for [`production_std_fs_files`]'s
/// resolution. All fields exclude `#[cfg(test)] mod` ranges — a test module's
/// `use super::*;` or tempdir `fs::write` must not make the file, or its children,
/// read as production `std::fs` (the `bynk-ide` files' shape).
#[derive(Default)]
struct FsImportFacts {
    /// A production `use super::*;` (optionally `pub`-qualified) — the edge that lets
    /// this file see its parent module's `use` bindings, and (chained) its ancestors'.
    glob_imports_super: bool,
    /// Names production `use` declarations bind to `std::fs` or an item under it:
    /// `use std::fs;` → `fs`, `use std::fs as x;` → `x`, `use std::{fs, io};` → `fs`,
    /// `use std::fs::File;` → `File`.
    std_fs_bindings: BTreeSet<String>,
    /// Every name a production `use` declaration binds, whatever the target — the
    /// shadow set: a nearer non-`std::fs` binding of a candidate name stops resolution.
    use_bound_names: BTreeSet<String>,
    /// Type-namespace items the file declares (`mod fs;`, `struct File`, `enum`,
    /// `trait`, `type`, `union`) — these beat a glob-imported name in real Rust, so
    /// they join [`Self::use_bound_names`] on the shadow side of resolution (#1016
    /// review). Value-namespace items (`fn`, `const`, `static`) can't head a `NAME::`
    /// module path and are deliberately not collected.
    declared_type_names: BTreeSet<String>,
    /// Identifiers appearing as a bare path root `NAME::` (not preceded by another
    /// path segment) on a production line — the call-site side of the resolution.
    bare_path_roots: BTreeSet<String>,
    /// Segment chains of `super::`/`self::`/`crate::`-qualified paths on production
    /// lines — `super::fs::read_to_string` records `["super", "fs", "read_to_string"]`.
    /// These need no glob import to reach an ancestor's binding (#1016 review).
    qualified_paths: BTreeSet<Vec<String>>,
}

fn fs_import_facts(src: &str) -> FsImportFacts {
    let lines: Vec<&str> = src.lines().collect();
    let ranges = test_mod_ranges(&lines);
    let mut facts = FsImportFacts::default();
    for (i, line) in lines.iter().enumerate() {
        if in_test_range(i, &ranges) {
            continue;
        }
        if let Some(decl) = use_declaration(line) {
            if decl == "super::*" {
                facts.glob_imports_super = true;
            }
            collect_use_bindings("", decl, &mut facts);
        }
        collect_declared_type_name(line, &mut facts.declared_type_names);
        collect_bare_path_roots(line, &mut facts.bare_path_roots);
        collect_qualified_paths(line, &mut facts.qualified_paths);
    }
    facts
}

/// The path text of a single-line `use` declaration — `use std::fs;` → `std::fs`,
/// with an optional `pub`/`pub(crate)`/`pub(in …)` prefix stripped and a trailing
/// `//` comment tolerated (`use super::*; // parent's fs` must not silently sever
/// the glob edge for a whole subtree — #1016 review; safe to split on `//` because
/// a `use` path can contain neither a comment marker nor a string). A declaration
/// rustfmt has split across lines has no trailing `;` here and is not recognised —
/// none of the `std::fs` forms in the scanned crates are long enough to split.
fn use_declaration(line: &str) -> Option<&str> {
    let mut t = line.trim();
    if let Some(rest) = t.strip_prefix("pub") {
        let rest = rest.trim_start();
        t = if let Some(after_paren) = rest.strip_prefix('(') {
            after_paren.split_once(')')?.1.trim_start()
        } else {
            rest
        };
    }
    let body = t.strip_prefix("use ")?;
    let body = body.split("//").next().unwrap_or(body);
    body.trim().strip_suffix(';').map(str::trim)
}

/// If `line` declares a type-namespace item — `mod`/`struct`/`enum`/`trait`/`type`/
/// `union`, optionally `pub`-qualified, optionally `unsafe` (traits) — record its
/// name. Field/variable positions can't start a trimmed line with these keywords, so
/// a leading-keyword scan is enough for rustfmt-shaped code.
fn collect_declared_type_name(line: &str, out: &mut BTreeSet<String>) {
    let mut t = line.trim();
    if let Some(rest) = t.strip_prefix("pub") {
        let rest = rest.trim_start();
        t = if let Some(after_paren) = rest.strip_prefix('(') {
            match after_paren.split_once(')') {
                Some((_, after)) => after.trim_start(),
                None => return,
            }
        } else {
            rest
        };
    }
    if let Some(rest) = t.strip_prefix("unsafe ") {
        t = rest.trim_start();
    }
    for kw in ["mod ", "struct ", "enum ", "trait ", "type ", "union "] {
        if let Some(rest) = t.strip_prefix(kw) {
            let rest = rest.trim_start();
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            if end > 0 {
                out.insert(rest[..end].to_string());
            }
            return;
        }
    }
}

/// Record the name(s) a `use` path binds into `facts` — `path::to::name`,
/// `path as alias`, and brace groups (`std::{fs, path::PathBuf}`, nested one level
/// per recursion). `prefix` is the already-consumed leading path (empty at the top).
fn collect_use_bindings(prefix: &str, entry: &str, facts: &mut FsImportFacts) {
    let entry = entry.trim();
    if entry.is_empty() {
        return;
    }
    if let Some((path_part, group)) = entry.split_once('{') {
        let inner_prefix = join_use_path(prefix, path_part.trim().trim_end_matches("::"));
        let group = group.strip_suffix('}').unwrap_or(group);
        for part in split_group_entries(group) {
            collect_use_bindings(&inner_prefix, part, facts);
        }
        return;
    }
    let (path_part, alias) = match entry.split_once(" as ") {
        Some((p, a)) => (p.trim(), Some(a.trim())),
        None => (entry, None),
    };
    let full = join_use_path(prefix, path_part);
    // `use std::fs::{self};` binds `fs` — normalise the `self` leaf away.
    let full = full.strip_suffix("::self").unwrap_or(&full);
    let last = full.rsplit("::").next().unwrap_or(full);
    let name = alias.unwrap_or(last);
    if name.is_empty() || name == "*" {
        return; // globs bind no single name; `super::*` is tracked separately
    }
    facts.use_bound_names.insert(name.to_string());
    if full == "std::fs" || full.starts_with("std::fs::") {
        facts.std_fs_bindings.insert(name.to_string());
    }
}

fn join_use_path(prefix: &str, part: &str) -> String {
    if prefix.is_empty() {
        part.to_string()
    } else {
        format!("{prefix}::{part}")
    }
}

/// Split a brace group's contents on top-level commas only — `fs::{self, File}, io`
/// is two entries, not three.
fn split_group_entries(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Every identifier `NAME` occurring as `NAME::` where the character before `NAME` is
/// not `:` — i.e. a path *root*, so `std::fs::read` contributes `std`, never `fs`.
/// Same line discipline as the text scan: comments included, production scope only
/// (the caller has already excluded test ranges).
fn collect_bare_path_roots(line: &str, out: &mut BTreeSet<String>) {
    let bytes = line.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = line[search_from..].find("::") {
        let pos = search_from + rel;
        let mut start = pos;
        while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
            start -= 1;
        }
        if start < pos && (start == 0 || bytes[start - 1] != b':') {
            out.insert(line[start..pos].to_string());
        }
        search_from = pos + 2;
    }
}

/// Every `super::`/`self::`/`crate::`-rooted path on `line`, as its segment chain —
/// `super::fs::read_to_string(p)` yields `["super", "fs", "read_to_string"]`. The
/// root must sit at a bare word boundary (not `a_super::` or `a::super::`), so only
/// genuine path roots are collected; `Self::` (capital) never matches, and `self.x`
/// has no `::` to match.
fn collect_qualified_paths(line: &str, out: &mut BTreeSet<Vec<String>>) {
    let bytes = line.as_bytes();
    for root in ["super", "self", "crate"] {
        let mut from = 0;
        while let Some(rel) = line[from..].find(root) {
            let start = from + rel;
            let root_end = start + root.len();
            from = root_end;
            let boundary_ok = start == 0 || {
                let c = bytes[start - 1];
                !(c.is_ascii_alphanumeric() || c == b'_' || c == b':')
            };
            if !boundary_ok || !line[root_end..].starts_with("::") {
                continue;
            }
            let mut segments = vec![root.to_string()];
            let mut pos = root_end;
            while line[pos..].starts_with("::") {
                let seg_start = pos + 2;
                let mut seg_end = seg_start;
                while seg_end < bytes.len()
                    && (bytes[seg_end].is_ascii_alphanumeric() || bytes[seg_end] == b'_')
                {
                    seg_end += 1;
                }
                if seg_end == seg_start {
                    break; // `super::*` and friends — no further identifier
                }
                segments.push(line[seg_start..seg_end].to_string());
                pos = seg_end;
            }
            if segments.len() >= 2 {
                out.insert(segments);
            }
        }
    }
}

/// The file defining `path`'s parent module, by the standard layout: `a/b.rs`'s parent
/// is `a.rs` (or `a/mod.rs`), `a/mod.rs`'s parent is the crate root, and the roots
/// (`lib.rs`/`main.rs`) have none. `#[path]`-remapped modules are not handled — none
/// exist below the driver, and a text-level probe can't chase them anyway.
fn module_parent(path: &Path, files: &[(PathBuf, String)]) -> Option<usize> {
    let stem = path.file_stem()?.to_str()?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    let parent_module: PathBuf = if stem == "mod" {
        dir?.parent().map(Path::to_path_buf).unwrap_or_default()
    } else if let Some(dir) = dir {
        dir.to_path_buf()
    } else {
        if stem == "lib" || stem == "main" {
            return None;
        }
        PathBuf::new()
    };
    let candidates = if parent_module.as_os_str().is_empty() {
        vec![PathBuf::from("lib.rs"), PathBuf::from("main.rs")]
    } else {
        vec![
            parent_module.with_extension("rs"),
            parent_module.join("mod.rs"),
        ]
    };
    candidates
        .iter()
        .find_map(|c| files.iter().position(|(p, _)| p == c))
}

/// The scopes whose bindings a name used in module `i` can see: the module itself,
/// then each ancestor reachable while every module below it glob-imports `super::*`.
fn visible_scopes(i: usize, facts: &[FsImportFacts], parents: &[Option<usize>]) -> Vec<usize> {
    let mut scopes = vec![i];
    let mut cur = i;
    loop {
        if !facts[cur].glob_imports_super {
            break;
        }
        let Some(parent) = parents[cur] else { break };
        scopes.push(parent);
        cur = parent;
    }
    scopes
}

/// How `name` resolves in module `m`'s namespace, walking [`visible_scopes`] with
/// nearest binding winning — a closer non-`std::fs` `use` binding *or* locally
/// declared type-namespace item shadows a farther `std::fs` binding, as in Rust.
enum NameResolution {
    StdFs,
    Other,
    Unbound,
}

fn resolve_name_in_module(
    m: usize,
    name: &str,
    facts: &[FsImportFacts],
    parents: &[Option<usize>],
) -> NameResolution {
    for s in visible_scopes(m, facts, parents) {
        if facts[s].std_fs_bindings.contains(name) {
            return NameResolution::StdFs;
        }
        if facts[s].use_bound_names.contains(name) || facts[s].declared_type_names.contains(name) {
            return NameResolution::Other;
        }
    }
    NameResolution::Unbound
}

/// Does a bare path root in file `i` resolve to `std::fs` through the bindings it
/// can see? Candidates are the names any visible scope binds to `std::fs`; each is
/// then resolved from `i` with nearest-binding-wins shadowing.
fn resolves_bare_std_fs(i: usize, facts: &[FsImportFacts], parents: &[Option<usize>]) -> bool {
    let scopes = visible_scopes(i, facts, parents);
    let mut candidates: BTreeSet<&str> = BTreeSet::new();
    for &s in &scopes {
        candidates.extend(facts[s].std_fs_bindings.iter().map(String::as_str));
    }
    candidates.into_iter().any(|name| {
        facts[i].bare_path_roots.contains(name)
            && matches!(
                resolve_name_in_module(i, name, facts, parents),
                NameResolution::StdFs
            )
    })
}

/// Does a `super::`/`self::`/`crate::`-qualified path in file `i` reach a `std::fs`
/// binding (#1016 review)? Unlike the bare-root case these need no glob import: the
/// root picks the starting module directly (`super`-hops up the parent chain, `self`
/// the file itself, `crate` the crate root), then each further segment either
/// resolves in that module's namespace — `std::fs` flags, anything else stops — or
/// descends into a child module file and continues. Inline `mod name { … }` blocks
/// are not modelled (their `use` bindings live in the same file, which the text scan
/// and bare-root resolution already cover).
fn resolves_qualified_std_fs(
    i: usize,
    facts: &[FsImportFacts],
    parents: &[Option<usize>],
    files: &[(PathBuf, String)],
) -> bool {
    facts[i]
        .qualified_paths
        .iter()
        .any(|chain| qualified_chain_reaches_std_fs(chain, i, facts, parents, files))
}

fn qualified_chain_reaches_std_fs(
    chain: &[String],
    i: usize,
    facts: &[FsImportFacts],
    parents: &[Option<usize>],
    files: &[(PathBuf, String)],
) -> bool {
    let mut idx = 1;
    let mut m = match chain[0].as_str() {
        "self" => i,
        "crate" => {
            let root = files
                .iter()
                .position(|(p, _)| p == Path::new("lib.rs") || p == Path::new("main.rs"));
            match root {
                Some(root) => root,
                None => return false,
            }
        }
        "super" => {
            let mut m = i;
            idx = 0;
            while idx < chain.len() && chain[idx] == "super" {
                let Some(parent) = parents[m] else {
                    return false;
                };
                m = parent;
                idx += 1;
            }
            m
        }
        _ => return false,
    };
    while idx < chain.len() {
        let seg = chain[idx].as_str();
        // Resolve `seg` in `m`, nearest scope first. Within a scope, a child module
        // file for `seg` is checked *before* the shadow set: a declared `mod seg;`
        // lands `seg` in `declared_type_names`, but that declaration IS the child
        // module — it's the path's next hop, not a shadow over it. (In valid Rust a
        // module and another same-name type-namespace item can't coexist in one
        // scope, so the ordering costs nothing.)
        let mut next = None;
        for s in visible_scopes(m, facts, parents) {
            if facts[s].std_fs_bindings.contains(seg) {
                return true;
            }
            if let Some(child) = child_module_file(s, seg, files) {
                next = Some(child);
                break;
            }
            if facts[s].use_bound_names.contains(seg) || facts[s].declared_type_names.contains(seg)
            {
                return false; // bound to something that is neither std::fs nor a module
            }
        }
        let Some(child) = next else {
            return false;
        };
        m = child;
        idx += 1;
    }
    false
}

/// The file defining module `m`'s child module `seg`, if it exists as a file:
/// `lib.rs` + `a` → `a.rs`/`a/mod.rs`, `a.rs` + `b` → `a/b.rs`/`a/b/mod.rs`,
/// `a/mod.rs` + `b` → `a/b.rs`/`a/b/mod.rs`.
fn child_module_file(m: usize, seg: &str, files: &[(PathBuf, String)]) -> Option<usize> {
    let m_path = &files[m].0;
    let module_dir: PathBuf = match m_path.file_stem().and_then(|s| s.to_str()) {
        Some("mod") => m_path.parent().unwrap_or(Path::new("")).to_path_buf(),
        Some("lib") | Some("main") if m_path.parent().is_none_or(|p| p.as_os_str().is_empty()) => {
            PathBuf::new()
        }
        _ => m_path.with_extension(""),
    };
    let candidates = [
        module_dir.join(format!("{seg}.rs")),
        module_dir.join(seg).join("mod.rs"),
    ];
    candidates
        .iter()
        .find_map(|c| files.iter().position(|(p, _)| p == c))
}

/// Every `#[cfg(test)] mod <ident> { ... }` block in `lines`, as inclusive
/// `(start_line, end_line)` line-index ranges — every occurrence, not just a single
/// trailing block. A file in this codebase can carry several test modules scattered
/// through it with production code between them — `bynk-emit/src/emitter/lower.rs` has
/// two, 1031 lines apart, and treating "everything after the first (or last)
/// `#[cfg(test)]`" as one cutoff silently misclassifies that intervening production
/// code as test-scope (caught in review: it made `fs_below_driver`, a *gated* probe,
/// blind over that span, and inflated `test_density`'s ratio by up to 39%).
///
/// A block's end is found by real brace-depth counting via [`brace_delta`], not a
/// "first column-0 `}`" shortcut: an earlier version of this fix tried exactly that
/// shortcut (reasoning that rustfmt always dedents a closing brace back to column 0)
/// and it broke on files like `bynk-ide/src/sequence.rs`, whose test module embeds
/// multi-line `.bynk`/TypeScript fixture source as string literals — source that
/// itself contains a column-0 `}` closing a top-level construct *inside the string*,
/// which the shortcut mistook for the end of the Rust `mod` block, truncating it by
/// hundreds of lines. `brace_delta` skips characters inside Rust string/char literals
/// and comments, so embedded fixture text can't be mistaken for real Rust braces.
///
/// Only matches a brace-opening `mod` line — `#[cfg(test)] mod foo;` (an external-file
/// declaration, not an inline scope) does not open a range.
fn test_mod_ranges(lines: &[&str]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "#[cfg(test)]"
            && let Some(off) = lines[i + 1..].iter().position(|l| !l.trim().is_empty())
        {
            let mod_line = i + 1 + off;
            let t = lines[mod_line].trim();
            if t.starts_with("mod ") && t.ends_with('{') {
                let mut depth = 0i32;
                let mut state = BraceScanState::Normal;
                let mut started = false;
                let mut end = lines.len() - 1;
                for (j, line) in lines[mod_line..].iter().enumerate() {
                    let (delta, new_state) = brace_delta(line, state);
                    state = new_state;
                    depth += delta;
                    if depth != 0 {
                        started = true;
                    }
                    if started && depth == 0 {
                        end = mod_line + j;
                        break;
                    }
                }
                ranges.push((mod_line, end));
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    ranges
}

fn in_test_range(line_idx: usize, ranges: &[(usize, usize)]) -> bool {
    ranges
        .iter()
        .any(|(start, end)| line_idx >= *start && line_idx <= *end)
}

/// Scanner state carried across lines for [`brace_delta`]: whether the cursor is
/// inside a string literal, a raw string (with its `#`-count), or a block comment
/// (with nesting depth — Rust block comments nest).
#[derive(Clone, Copy, PartialEq)]
enum BraceScanState {
    Normal,
    InString,
    InRawString(u8),
    InBlockComment(u32),
}

/// The net `{`/`}` depth change in `line`, skipping characters inside Rust string/char
/// literals, raw strings, and line/block comments — a naive per-character brace count
/// breaks the moment a line contains a fixture string like `"fn f() { \"{\" }"` or a
/// doc comment mentioning a brace. Returns the depth delta and the state to carry into
/// the next line (a string or block comment can span line boundaries).
fn brace_delta(line: &str, mut state: BraceScanState) -> (i32, BraceScanState) {
    let mut delta = 0i32;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match state {
            BraceScanState::Normal => {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'/') {
                    break; // rest of the line is a line comment
                }
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    state = BraceScanState::InBlockComment(1);
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    state = BraceScanState::InString;
                    i += 1;
                    continue;
                }
                if chars[i] == 'r' && matches!(chars.get(i + 1), Some('"') | Some('#')) {
                    let mut j = i + 1;
                    let mut hashes = 0u8;
                    while chars.get(j) == Some(&'#') {
                        hashes += 1;
                        j += 1;
                    }
                    if chars.get(j) == Some(&'"') {
                        state = BraceScanState::InRawString(hashes);
                        i = j + 1;
                        continue;
                    }
                }
                if chars[i] == '\'' {
                    // A `'\x'`/`'\\'`-style escaped char literal, or a plain `'x'` —
                    // skip past it so its contents can't be mistaken for braces.
                    // Anything else (no closing `'` within a couple of chars) is a
                    // lifetime, which owns no closing quote to skip.
                    if chars.get(i + 1) == Some(&'\\') {
                        let mut j = i + 2;
                        while j < chars.len() && chars[j] != '\'' {
                            j += 1;
                        }
                        i = (j + 1).min(chars.len());
                        continue;
                    } else if chars.get(i + 2) == Some(&'\'') {
                        i += 3;
                        continue;
                    }
                }
                match chars[i] {
                    '{' => delta += 1,
                    '}' => delta -= 1,
                    _ => {}
                }
                i += 1;
            }
            BraceScanState::InString => {
                if chars[i] == '\\' {
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    state = BraceScanState::Normal;
                }
                i += 1;
            }
            BraceScanState::InRawString(hashes) => {
                if chars[i] == '"' {
                    let mut j = i + 1;
                    let mut h = 0u8;
                    while chars.get(j) == Some(&'#') && h < hashes {
                        h += 1;
                        j += 1;
                    }
                    if h == hashes {
                        state = BraceScanState::Normal;
                        i = j;
                        continue;
                    }
                }
                i += 1;
            }
            BraceScanState::InBlockComment(depth) => {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    state = BraceScanState::InBlockComment(depth + 1);
                    i += 2;
                    continue;
                }
                if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    state = if depth <= 1 {
                        BraceScanState::Normal
                    } else {
                        BraceScanState::InBlockComment(depth - 1)
                    };
                    i += 2;
                    continue;
                }
                i += 1;
            }
        }
    }
    (delta, state)
}

// --- Gated probe 3: options_sources --------------------------------------

/// R2.3. `CompileOptions` (in `bynk-emit/src/project.rs`) has a `sources` field.
fn options_sources(root: &Path) -> Probe {
    let src = std::fs::read_to_string(root.join("bynk-emit/src/project.rs")).unwrap_or_default();
    let present = struct_body(&src, "CompileOptions").is_some_and(|body| body.contains("sources"));
    Probe {
        name: "options_sources",
        gated: true,
        reads: if present {
            "present".to_string()
        } else {
            "absent".to_string()
        },
    }
}

/// The `{ ... }` body text of `struct <name>` in `src`, brace-matched from the struct's
/// own opening brace to its close.
fn struct_body<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("struct {name}");
    let start = src.find(&needle)?;
    let open = start + src[start..].find('{')?;
    let mut depth = 0i32;
    for (offset, ch) in src[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&src[open..open + offset + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

// --- Gated probe 4: hoist_sinks -------------------------------------------

/// R6.2. Live (non-comment) occurrences of the sink-passing signature
/// `stmts: &mut Vec<String>` in `bynk-emit`. Tier B (T2.1) deletes it entirely.
fn hoist_sinks(root: &Path) -> Probe {
    let dir = root.join("bynk-emit/src");
    let needle = "stmts: &mut Vec<String>";
    let mut count = 0usize;
    for (_, contents) in rust_files(&dir) {
        for line in contents.lines() {
            if !is_line_comment(line) && line.contains(needle) {
                count += 1;
            }
        }
    }
    Probe {
        name: "hoist_sinks",
        gated: true,
        reads: count.to_string(),
    }
}

// --- Gated probe 5: span_keyed_maps ---------------------------------------

/// R2.4. Whole-repo occurrences of `HashMap<Span` (comments included — the phase-3
/// migration target is every mention, not just live call sites), **excluding
/// `xtask` itself**: this probe's own doc comment and source both name the search
/// string, which would otherwise self-count every time this file is touched — the
/// same self-reference hazard flagged for the dead-identifier probes below, caught
/// here by running the probe against itself before committing the first table.
fn span_keyed_maps(root: &Path) -> Probe {
    let count = count_repo_wide(root, "HashMap<Span", &["xtask"]);
    Probe {
        name: "span_keyed_maps",
        gated: true,
        reads: count.to_string(),
    }
}

fn count_repo_wide(root: &Path, needle: &str, exclude_crates: &[&str]) -> usize {
    let mut total = 0usize;
    for entry in top_level_crate_dirs(root) {
        if exclude_crates
            .iter()
            .any(|c| entry.file_name().is_some_and(|n| n == *c))
        {
            continue;
        }
        for (_, contents) in rust_files(&entry.join("src")) {
            total += contents.matches(needle).count();
        }
    }
    total
}

/// Every workspace member crate directory (anything at the repo root with a
/// `Cargo.toml` and a `src/` dir), excluding `target` and non-crate directories.
fn top_level_crate_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("Cargo.toml").is_file() && path.join("src").is_dir() {
            out.push(path);
        }
    }
    out.sort();
    out
}

// --- Gated probe 6: emit_diagnostics --------------------------------------

/// R3.5. `bynk.*` string literals in `bynk-emit`/`bynk-check` source, cross-referenced
/// against `bynk_syntax::diagnostics::REGISTRY` — not pattern-matched. A literal not in
/// `REGISTRY` is a commons/namespace path (e.g. `bynk.locale`, the compiled first-party
/// source module name), not a diagnostic code, and must not inflate the count (#999
/// Decision A: this cross-reference is what makes the exclusion correct by
/// construction rather than a second hand-maintained list).
fn emit_diagnostics(root: &Path) -> Probe {
    let registry: BTreeSet<&str> = bynk_syntax::diagnostics::REGISTRY
        .iter()
        .map(|d| d.code)
        .collect();
    let mut parts = Vec::new();
    for (label, dir) in [
        ("bynk-emit", "bynk-emit/src"),
        ("bynk-check", "bynk-check/src"),
    ] {
        let mut naive: BTreeSet<String> = BTreeSet::new();
        for (_, contents) in rust_files(&root.join(dir)) {
            for lit in bynk_dotted_literals(&contents) {
                naive.insert(lit.to_string());
            }
        }
        let true_count = naive
            .iter()
            .filter(|l| registry.contains(l.as_str()))
            .count();
        parts.push(format!("{label}={true_count}/{}", naive.len()));
    }
    Probe {
        name: "emit_diagnostics",
        gated: true,
        reads: format!("{} (true/naive)", parts.join(", ")),
    }
}

// --- Gated probe 7: ide_emit_edge -----------------------------------------

/// R10.2. `bynk-ide` → `bynk-emit` in the manifest (`bynk-emit.workspace = true` or an
/// equivalent path/version dependency line).
fn ide_emit_edge(root: &Path) -> Probe {
    let manifest = std::fs::read_to_string(root.join("bynk-ide/Cargo.toml")).unwrap_or_default();
    let present = manifest
        .lines()
        .any(|l| l.trim_start().starts_with("bynk-emit"));
    Probe {
        name: "ide_emit_edge",
        gated: true,
        reads: if present {
            "present".to_string()
        } else {
            "absent".to_string()
        },
    }
}

// --- Gated probe 8: ast_importers -----------------------------------------

/// #1176: `bynk-emit::ir`'s own two files — named exactly, not by path prefix, the same
/// permanent-carve-out discipline [`NAMED_FS_EXCEPTIONS`] and [`emit_diagnostics`]'s
/// registry cross-reference already use. An `Ast → Ir` lowering pass importing
/// `bynk_syntax::ast` is that pass's entire job, not the AST-walking this track is
/// closing (phase 6's own P6.9 correction, #1167 — see the retired `the-ir.md`'s
/// closing summary, `design/archive/retired-tracks.md`) — but `project.rs` also
/// imports `bynk_syntax::ast` today (`EmitProjectCtx` holding `ActorDecl`/`AgentDecl`
/// fields directly), and that *is* exactly the still-open R6.13 defect this probe
/// tracks (P6.6: "closes the emitter reading AST declarations directly"). A
/// path-prefix rule scoped to `emitter/**` would exclude that file right along with
/// `ir/`'s legitimate ones, silently undercounting real remaining work — see
/// [`is_named_ast_importer`].
///
/// #1184 review: this exclusion is necessary but not sufficient for R6.13. `bynk-ir`
/// still embeds AST types directly in IR struct fields (`TypeShape::Refined`'s
/// `BaseType`/`Refinement`, ADR 0366) rather than IR-native equivalents — an emitter
/// reading such a field touches the AST without ever spelling `bynk_syntax::ast`
/// itself, so it is invisible to this probe by construction. `ast_importers` reading
/// its retired floor (5 — `design/archive/retired-tracks.md`'s own closing summary
/// has the per-file argument) proves no *remaining* file outside these two and the
/// five-file rendering subtree imports the AST module directly; it does not by
/// itself prove every `bynk-ir` field is AST-free.
///
/// #1187's own closing scoping pass adds one more, on different grounds than the
/// `ir.rs`/`ir/lower.rs` pair above: `project/tests_emit.rs` was deliberately *not*
/// added alongside `project.rs` when this list was first cut (the
/// `ast_importer_exclusion_is_named_not_prefixed` test below used to assert exactly
/// that) — #1187's own scoping pass found new evidence changing that: its test/suite
/// case bodies call `emitter::lower_block_to_async_body`/`lower_test_case_body`/
/// `lower_integration_case_body` directly (the Q7-settled body-rendering pass —
/// `emitter/lower.rs` keeps hand-writing TypeScript source text after phase 6's
/// cutover, the printer that would change that is phase 7's), and
/// its own `driver_param_ty`/`strip_effect_httpresult` read a handler's *declared*
/// param/return `TypeRef` with no corresponding `TyId` available at that call site
/// (the same caller-reads-callee's-raw-declared-shape pattern #661 established for
/// cross-context codec generation). Both are the Q7/printer kind of unreachable, not
/// the "still open, real work" kind the original exclusion list deliberately left this
/// file out of — the correction is new evidence, not a reversal of that reasoning.
///
/// Review of #1210: `emitter.rs`/`emitter/lower.rs` themselves were considered for
/// this same exclusion and **rejected** — Q7 settles that these files' *body-rendering*
/// surface stays AST-parameter-driven, but both files also hold live, currently
/// untouched AST-*declaration* reads with no such gate: `emitter.rs`'s own
/// `CommonsItem::Service`/`svc.protocol` walk (consumed-event-root collection) and
/// `emitter/lower.rs`'s own `cap_op_param_names` (`CommonsItem::Capability`/`c.ops`)
/// were exactly the P6.2/P6.6-class conversions phase 6's own slice decomposition
/// still listed as in scope at the time, not body-rendering. Excluding either file
/// would have hidden that real, fixable surface from this probe the same way a
/// path-prefix rule would — the harm the named-not-prefixed discipline above exists
/// to prevent, just at file granularity instead of directory granularity. (Both
/// converted their own reachable surface later, without joining this list — phase 6's
/// closing summary, `design/archive/retired-tracks.md`, has the account.)
///
/// P6.33 (phase 6's own §6a.D re-settling, 19 August 2026): `emitter/serialisation.rs`
/// joins the list, on grounds distinct from every entry above — not Q7 body-rendering,
/// not test-only reach, but a phase boundary. Unlike `emitter.rs`/`emitter/lower.rs`,
/// this file holds no `CommonsItem`-declaration-read surface at all (confirmed:
/// `grep -c bynk_syntax::ast` finds only its one `use` line and its `#[cfg(test)]`
/// module) — its entire AST surface, ~120 sites, *is* the `TypeRef`-driven JSON/wire
/// codec renderer (`emit_record_codec`/`emit_sum_codec`/`serialise_expr`/
/// `deserialise_expr`/`ts_inner_type` and siblings). Rendering a checker type as TS
/// codec source text is the same class of question Q7 already settled belongs to the
/// eventual printer (phase 7, `bynk-ts`) — this file has no `use crate::ir` at all, so
/// nothing here has been resisting an available IR-native alternative; none exists. The
/// re-settling found no clean way to shrink this file's AST surface further without
/// building printer infrastructure phase 6's own scope already excluded.
///
/// P6.49 (phase 6's own §6b, 19 August 2026): `project.rs` — R6.13's own still-open
/// declaration-read surface named at the top of this doc block, above — cleared
/// **without joining this list**. Nine slices (P6.42–P6.49) relocated its remaining
/// declaration reads to the `bynk-check`/`bynk-project` crates that already own the
/// data (`SourceUnit::name()`, `own_contract_hashes`, `discover_event_subscribers`,
/// `combined_types_for_unit_info`, two owner-side accessors,
/// `lower_event_subscriber_shapes_ir`, `walk_unit_table_bodies`) or re-exported a type
/// from a `bynk-check` module whose own public API was already parameterised by it
/// (`TypeDecl`/`FnDecl`/`Visibility` from `project_model`, `ActorDecl` from `actors` —
/// the P6.27 `ExprId` precedent, applied four more times). This is the evidence this
/// exclusion list's own entries above are real, earned exclusions and not a standing
/// habit: the harder file cleared its own way, on its own schedule, with zero new
/// entries here.
///
/// **P6.58/P6.59, 19 August 2026: phase 6 (`the-ir.md`, spine #1137) retired at this
/// probe reading 5, not 0.** The floor is exactly `bynk-emit/src/emitter{,/**}` —
/// `emitter.rs`, `emitter/emit.rs`, `emitter/lower.rs`, `emitter/workers.rs`,
/// `emitter/workers_entry.rs` — the TypeScript-rendering subtree phase 7's own printer
/// inherits; each file's own structural reason, and the full slice history behind
/// every correction this doc block narrates, live in `design/archive/retired-tracks.md`
/// now that `the-ir.md` itself is gone. This exclusion list does **not** grow to
/// reach that floor — the floor is a fact about `AST_IMPORTER_EXCEPTIONS`'s own
/// four entries staying exactly these four, not a fifth argument for adding to them.
/// The probe itself stays gated, unchanged, reading 5: a regression ratchet phase 7
/// inherits and drives down as it builds the printer this floor's own residue names.
///
/// **Arc D, P7.12 (crate carve): `ir.rs`/`ir/lower.rs` drop out of this list
/// entirely — not because they stopped importing the AST (unchanged, still
/// do), but because they left `bynk-emit/src` altogether, carved into the new
/// `bynk-ir`/`bynk-lower` crates ADR 0332 deferred and ADR 0385 triggered.**
/// This probe was never scoped to those crates (`ast_importer_files` walks
/// `bynk-emit/src` only), so the pair is simply outside its universe now,
/// the same way a file moving to `bynk-check`/`bynk-project` already leaves
/// silently rather than needing its own exclusion-list removal step. Two
/// named exclusions remain.
const AST_IMPORTER_EXCEPTIONS: &[&str] = &["project/tests_emit.rs", "emitter/serialisation.rs"];

/// Is `rel_path` (relative to `bynk-emit/src`) one of [`AST_IMPORTER_EXCEPTIONS`]?
fn is_named_ast_importer(rel_path: &Path) -> bool {
    let rel = rel_path.to_string_lossy().replace('\\', "/");
    AST_IMPORTER_EXCEPTIONS.contains(&rel.as_str())
}

/// Is `contents` a module (not a nested block) that glob-imports its parent —
/// i.e. does it carry a top-level (column-0) `use super::*;`? Rust's own privacy
/// rule makes a parent module's private `use` visible to descendant modules, so a
/// file matching this can expose `bynk_syntax::ast` names it never spells itself
/// (P6.26 review, #1259) — deliberately column-0 only, so a `use super::*;`
/// *inside* a nested `#[cfg(test)] mod tests { .. }` block (glob-importing its own
/// immediately-enclosing module, not the grandparent file on disk) doesn't
/// false-positive this check.
fn has_module_level_super_glob(contents: &str) -> bool {
    contents.lines().any(|line| line == "use super::*;")
}

/// For `rel_path` = `<dir>/<file>.rs`, does the sibling module file `<dir>.rs`
/// (the parent module a top-level `use super::*;` in `rel_path` would inherit
/// from) itself contain `bynk_syntax::ast`? `None` if `rel_path` has no such
/// parent (a file directly under `bynk-emit/src`, e.g. `emitter.rs` itself).
fn super_glob_parent_imports_ast(dir: &Path, rel_path: &Path) -> Option<bool> {
    let parent_dir = rel_path.parent()?;
    if parent_dir.as_os_str().is_empty() {
        return None;
    }
    let parent_file = dir.join(parent_dir).with_extension("rs");
    Some(
        std::fs::read_to_string(&parent_file)
            .is_ok_and(|contents| contents.contains("bynk_syntax::ast")),
    )
}

/// The files [`ast_importers`] counts: `bynk-emit/src` files whose contents match
/// `bynk_syntax::ast` **or** that inherit it from an AST-importing parent through a
/// top-level `use super::*;` (P6.26 review, #1259 — a file that stops spelling the
/// AST module directly by deleting its own explicit import, while a live `use
/// super::*;` still channels a still-AST-importing parent's names in, must stay
/// counted; otherwise a future partial conversion could silently drop this probe
/// without the underlying AST dependency actually being gone), excluding
/// [`AST_IMPORTER_EXCEPTIONS`]. Split out from [`ast_importers`] so a test can
/// assert on the actual survivor set, not just its length (#1184 review).
fn ast_importer_files(root: &Path) -> Vec<PathBuf> {
    let dir = root.join("bynk-emit/src");
    rust_files(&dir)
        .into_iter()
        .filter(|(path, contents)| {
            contents.contains("bynk_syntax::ast")
                || (has_module_level_super_glob(contents)
                    && super_glob_parent_imports_ast(&dir, path.strip_prefix(&dir).unwrap_or(path))
                        .unwrap_or(false))
        })
        .filter(|(path, _)| !is_named_ast_importer(path.strip_prefix(&dir).unwrap_or(path)))
        .map(|(path, _)| path)
        .collect()
}

/// R6.13. Files in `bynk-emit/src` that import `bynk_syntax::ast`, excluding
/// [`AST_IMPORTER_EXCEPTIONS`] — phase 6's own remaining AST import surface (retired,
/// spine #1137; `design/archive/retired-tracks.md` has the closing summary). #1176:
/// the unexcluded, crate-wide count could never reach 0 while `bynk-emit::ir`'s
/// lowering pass exists at all; this exclusion is what let the probe track phase 6's
/// real completion criterion instead of a floor its own IR module structurally could
/// not clear. Gated at 5, phase 6's own retired floor, for phase 7 to drive down.
fn ast_importers(root: &Path) -> Probe {
    Probe {
        name: "ast_importers",
        gated: true,
        reads: ast_importer_files(root).len().to_string(),
    }
}

// --- Gated probe 9: emit_abi_shapes ---------------------------------------

/// ADR 0310 D1's four emit-ABI shapes, as they surface as import names in the vendored
/// bindings — the `Result`/`Option` tag layout plus `JsonError`, `Uuid`, `FetchError`.
const EMIT_ABI: &[&str] = &[
    "Result",
    "Option",
    "Ok",
    "Err",
    "Some",
    "None",
    "JsonError",
    "Uuid",
    "FetchError",
];

/// The capability interfaces a vendored binding legitimately imports to implement what
/// it declares — governed by language-stability rules, not ADR 0310's codegen-freeze
/// concern. See [`emit_abi_shapes`] and #999 Decision E for the two-list rationale.
const CAPABILITY_SURFACE: &[&str] = &[
    "Clock",
    "Fetch",
    "Idempotency",
    "Locale",
    "Logger",
    "Random",
    "Secrets",
    "Request",
    "Response",
    "LocaleTag",
    "Kv",
    "KVNamespace",
    // Interop I5 (#1972): `bynk.cloudflare`'s `Service`, and the Cloudflare
    // `Fetch` provider its binding sends through, so the two share one set of
    // header and response rules. Binding-to-binding, not a codegen shape.
    "Service",
    "FetchProvider",
];

/// Is `ident` one of ADR 0310's enumerated emit-ABI shapes, or part of the capability
/// surface a binding is required to import? If neither, it's a leak `emit_abi_shapes`
/// flags — this is the single predicate both the probe and its tests use, so a test
/// asserting "no leak" can't silently pass against a list the test itself redefined.
fn is_enumerated_emit_abi_or_capability_surface(ident: &str) -> bool {
    EMIT_ABI.contains(&ident) || CAPABILITY_SURFACE.contains(&ident)
}

/// The directories holding the vendored first-party TypeScript bindings, relative to
/// the repo root (standard-library track, #1940): the `bynk` surface's per-platform
/// bindings in `bynk-std`'s portable layer, and the Cloudflare platform adapter's
/// binding in its own subtree. [`emit_abi_shapes`] reads every `.ts` file in each,
/// non-recursively, so a binding added to a new subtree must be listed here.
const EMIT_ABI_BINDING_DIRS: &[&str] = &["bynk-std/lib/core/bindings", "bynk-std/lib/cloudflare"];

/// ADR 0310's probe (#999 Decision E). The vendored first-party bindings under
/// [`EMIT_ABI_BINDING_DIRS`] must reference only [`EMIT_ABI`]'s nine names.
///
/// This does NOT count every non-enumerated import: a binding legitimately imports the
/// [`CAPABILITY_SURFACE`] interfaces it implements — that surface is governed by
/// language-stability rules, not ADR 0310's codegen-freeze concern, and a probe that
/// flagged it would read non-zero on every binding by construction. See #999 Decision
/// E for the two-list rationale and its falsifier.
///
/// A listed directory that is missing, or holds no `.ts` file, reads as a failure
/// rather than as zero leaks: a binding moved out of the probe's sight must not pass
/// it vacuously (#1940 Decision D).
fn emit_abi_shapes(root: &Path) -> Probe {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for rel in EMIT_ABI_BINDING_DIRS {
        let Ok(entries) = std::fs::read_dir(root.join(rel)) else {
            return Probe {
                name: "emit_abi_shapes",
                gated: true,
                reads: format!("bindings directory not found: {rel}"),
            };
        };
        let before = files.len();
        files.extend(
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|path| path.extension().is_some_and(|e| e == "ts")),
        );
        if files.len() == before {
            return Probe {
                name: "emit_abi_shapes",
                gated: true,
                reads: format!("no bindings found in {rel}"),
            };
        }
    }
    // By file name, so the reading does not depend on which directory a binding sits in.
    files.sort_by_key(|path| path.file_name().map(|n| n.to_os_string()));
    let mut leaks: Vec<String> = Vec::new();
    for path in files {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        for ident in ts_named_imports_from_runtime_modules(&contents) {
            if !is_enumerated_emit_abi_or_capability_surface(&ident) {
                leaks.push(format!("{name}:{ident}"));
            }
        }
    }
    Probe {
        name: "emit_abi_shapes",
        gated: true,
        reads: format!("{} ({})", leaks.len(), leaks.join(", ")),
    }
}

// --- Gated probe 10: ts_writes ---------------------------------------------

/// Files under `bynk-emit/src` that contain `write!`/`writeln!`/`format!` calls but
/// produce no TypeScript at all — excluded from both [`ts_writes`] and [`ts_any`], each
/// argued individually the same way [`AST_IMPORTER_EXCEPTIONS`] is, not assumed from a
/// path prefix: `emitter/wrangler.rs` writes `wrangler.toml`; `emitter/secrets.rs`
/// writes `bynk-secrets.json`; `emitter/contracts.rs` writes `bynk-contracts.json`;
/// `testkit.rs` builds a `.bynk` source fixture — a compiler *input* for tests, not
/// output. P7.3 (#1303): `emitter/toml_doc.rs` writes `wrangler.toml` text too —
/// `emitter/wrangler.rs`'s own writes moved here when it stopped building the TOML text
/// directly and started building a typed `TomlDocument` for this module to print — same
/// rationale, same exclusion.
///
/// (`ir/lower.rs` — Rust-internal `String` values stored on `Ir*` struct fields during
/// the checker→IR lowering pass, never emitted syntax — was excluded here for the same
/// reason until Arc D's P7.12 crate carve moved it to `bynk-lower` entirely, outside
/// this probe's own `bynk-emit/src` universe; no exclusion needed for a file this probe
/// no longer walks. `emitter/source_map.rs`, which wrote source-map JSON, is the same
/// shape one carve earlier — P7.5 (#1308) relocated it to `bynk-ts/src/source_map.rs`
/// in full; its own #1561 removal from this list.)
const TS_WRITES_EXCLUDED_FILES: &[&str] = &[
    "emitter/wrangler.rs",
    "emitter/toml_doc.rs",
    "emitter/secrets.rs",
    "emitter/contracts.rs",
    "testkit.rs",
];

/// Is `rel_path` (relative to `bynk-emit/src`) one of [`TS_WRITES_EXCLUDED_FILES`]?
fn is_ts_writes_excluded_file(rel_path: &Path) -> bool {
    let rel = rel_path.to_string_lossy().replace('\\', "/");
    TS_WRITES_EXCLUDED_FILES.contains(&rel.as_str())
}

/// True if `line` builds a filesystem path via `format!` rather than TypeScript text —
/// the `PathBuf::from(format!(...))`/`.join(format!(...))`/`.with_file_name(format!(...))`
/// idiom [`ts_writes`] excludes at line granularity, not by file, because the files it
/// appears in (`project.rs`, `project/tests_emit.rs`) are otherwise genuinely
/// TypeScript-producing.
///
/// **`.with_file_name(format!` found and added by Arc F's own item-4 investigation
/// (#1457):** `project.rs`'s `sibling_path` (`output_path.with_file_name(format!(
/// "{name}.{suffix}"))`) builds a sibling filesystem path the same way the two idioms
/// above do, but spelled with `.with_file_name(` — the prior substring match didn't
/// catch it, over-counting `ts_writes` by this one site.
fn is_path_construction_line(line: &str) -> bool {
    line.contains("PathBuf::from(format!")
        || line.contains(".join(format!")
        || line.contains(".with_file_name(format!")
}

/// Relativises every path in [`rust_files`]'s output against `dir`, so [`ts_writes`]
/// and [`ts_any`]'s counting logic ([`ts_writes_violations`], [`ts_any_violations`])
/// takes the same `&[(PathBuf, String)]` shape [`production_std_fs_files`] does — an
/// in-memory file list a test can construct directly, per review of #1297 (a first cut
/// of these two probes took `root: &Path` and did its own walk, so nothing but the
/// drift gate actually exercised the exclusion logic; deleting a `continue` left every
/// test green).
fn rust_files_relative(dir: &Path) -> Vec<(PathBuf, String)> {
    rust_files(dir)
        .into_iter()
        .map(|(path, contents)| {
            let rel = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
            (rel, contents)
        })
        .collect()
}

/// [`ts_writes`]'s counting logic, over an explicit `(relative path, contents)` list —
/// see [`rust_files_relative`] for why this isn't `root: &Path`.
///
/// **A real mistake this slice's own grounding found and fixed, not carried forward:**
/// an earlier survey (during phase 7's own track-opening research) characterised
/// `project/tests_emit.rs`'s 128 such sites as excludable "test-assertion strings" — the
/// same mischaracterisation `semantics-in-the-checker.md`'s own settling review caught
/// and corrected for a *different* probe (`emit_diagnostics`) on this same file: it is
/// `process_tests`/`process_integration_tests`, real production TypeScript-emission
/// code, not fixture noise, and none of its 128 sites fall inside its own single
/// `#[cfg(test)] mod tests { .. }` block. All 128 count here, less the one line that
/// genuinely builds a file path ([`is_path_construction_line`]).
///
/// **Known, accepted gap:** `project/tests_emit.rs`'s
/// `target_name: format!("system · {suite}")` builds a human-readable struct-field
/// label, not TypeScript text, and matches neither exclusion rule. A text-level scanner
/// has no cheap way to catch one field-name-specific site without a bespoke rule for it
/// alone — accepted as a one-site over-count, the same "known remaining gaps, out of
/// reach for a text-level scanner" discipline [`production_std_fs_files`] already
/// documents for a different probe.
fn ts_writes_violations(files: &[(PathBuf, String)]) -> usize {
    let mut count = 0usize;
    for (rel, contents) in files {
        if is_ts_writes_excluded_file(rel) {
            continue;
        }
        let lines: Vec<&str> = contents.lines().collect();
        let ranges = test_mod_ranges(&lines);
        for (i, line) in lines.iter().enumerate() {
            if in_test_range(i, &ranges) || is_line_comment(line) || is_path_construction_line(line)
            {
                continue;
            }
            if line.contains("write!") || line.contains("writeln!") || line.contains("format!") {
                count += 1;
            }
        }
    }
    count
}

/// The trajectory's own phase-7 probe (`design/bynk-compiler-trajectory.md` §3):
/// "TypeScript-producing `write!` outside a printer". Never measured before this slice
/// (P7.0, #1296; see phase 7's own closing summary, `design/archive/retired-tracks.md`,
/// for the full retirement argument) — `bynk-ts` did not exist yet at measurement time,
/// so "outside a printer" reduced then to "in `bynk-emit`, outside a `Verbatim`
/// construction"; the `Verbatim` half of that exclusion was vacuous until P7.5 built the
/// type.
///
/// **Not "zero/closure"-shaped like this module's other twelve gated probes, and gated
/// anyway — a deliberate choice, not an inherited one.** The reading started at 1641 and
/// converged, over dozens of slices, to phase 7's own argued retirement floor, **809**
/// (ADR 0409; the full bucket-by-bucket accounting is in phase 7's own closing summary,
/// `design/archive/retired-tracks.md`) — never the literal 0 first proposed, nor a small
/// fixed number the way `ast_importers`/`emit_abi_shapes` are. Stays gated after
/// retirement, not deleted — a regression ratchet like `ast_importers` (floor 5) already
/// is: it moves on any `bynk-emit` PR that adds or removes a single `write!`/`writeln!`/
/// `format!` line anywhere in the crate — the same volatility #999 Decision D cites for
/// *not* gating `wildcard_arms` (311, ungated for exactly this reason) — but a floor this
/// track spent dozens of slices earning is worth catching a silent regression against,
/// the same trade `ast_importers` already made successfully across phase 6's 59 slices.
/// The churn cost is real and accepted, not overlooked: see ADR 0389/ADR 0390 for the
/// argument in full (review of #1297).
///
/// Counts `bynk-emit/src/**/*.rs` lines — excluding comments, `#[cfg(test)]` test-module
/// ranges, [`TS_WRITES_EXCLUDED_FILES`], and [`is_path_construction_line`] matches —
/// containing `write!`, `writeln!` or `format!`. See [`ts_writes_violations`] for the
/// counting logic itself.
fn ts_writes(root: &Path) -> Probe {
    let dir = root.join("bynk-emit/src");
    Probe {
        name: "ts_writes",
        gated: true,
        reads: ts_writes_violations(&rust_files_relative(&dir)).to_string(),
    }
}

// --- Gated probe 11: ts_any -------------------------------------------------

/// True if `line` (not a comment) violates R7.1's `TsType::Any` prohibition: an
/// `as any` cast, a bare `: any` type annotation, or `any` in generic type-argument
/// position (`Array<any>`, `Record<string, any[]>`, `Promise<any>`).
///
/// Six patterns, not `as any` alone, following three rounds of the same finding.
/// Round one (Q3, phase 7's own settling pass) found `as any` alone
/// under-counts R7.1 and added bare `: any`. Round two (review of #1297) found *that*
/// still under-counts: `bynk-emit/src/emitter/lower.rs`'s `joinOn`/`leftJoin`/`groupBy`
/// emit `const __h: Record<string, any[]> = {}` — `, any[]` contains neither `as any`
/// nor `: any`, so three live, production, TypeScript-emitting sites read as clean
/// under the round-one predicate. Widened to also match `<any`, `any>` and `any[]` —
/// each checked against the live tree for false positives (no non-`any`-typed English
/// word starts with `any` immediately after `<` or ends in `any` immediately before
/// `>`/`[]` anywhere in `bynk-emit/src` today) rather than assumed safe. Round three
/// (review of #1322) found a fourth spelling: once a site builds a real `bynk_ts::
/// TsType` node instead of writing TypeScript text directly, an emitted `any` no
/// longer appears as Rust-source `as any`/`: any` at all — `workers.rs`'s own
/// `TsType::named("any")` calls (#1321) emit the identical `payload as any`/
/// `let __who: any` text as before, byte-for-byte, but the *Rust spelling* that
/// produces it no longer matches any of the five text patterns above, so the probe
/// silently uncounted three real, still-live R7.1 residuals. Every later Arc C slice
/// converting an `any`-emitting `writeln!`/`format!` site the same way would keep
/// deflating this count the same way, so the fix generalises rather than special-
/// cases these three lines: match the construction spelling itself
/// (`named("any"`), not just raw emitted text.
///
/// Split out from [`ts_any_violations`] so a test can exercise the predicate directly,
/// without file I/O.
fn line_violates_ts_any(line: &str) -> bool {
    !is_line_comment(line)
        && (line.contains("as any")
            || line.contains(": any")
            || line.contains("<any")
            || line.contains("any>")
            || line.contains("any[]")
            || line.contains("named(\"any\""))
}

/// [`ts_any`]'s counting logic, over an explicit `(relative path, contents)` list — see
/// [`rust_files_relative`] for why this isn't `root: &Path`.
fn ts_any_violations(files: &[(PathBuf, String)]) -> usize {
    let mut count = 0usize;
    for (rel, contents) in files {
        if is_ts_writes_excluded_file(rel) {
            continue;
        }
        let lines: Vec<&str> = contents.lines().collect();
        let ranges = test_mod_ranges(&lines);
        for (i, line) in lines.iter().enumerate() {
            if in_test_range(i, &ranges) {
                continue;
            }
            if line_violates_ts_any(line) {
                count += 1;
            }
        }
    }
    count
}

/// Reference rule R7.1 (`design/bynk-greenfield-compiler.md` Part 7) — "the tree
/// contains no ... `TsType::Any`". Gated for the same reason [`ts_writes`] is (see its
/// own doc comment): started at 55 (not the settling review's estimated ~24 — ADR
/// 0390), converged over several slices to phase 7's own argued retirement floor, **26**
/// (ADR 0404; six already-argued families, none newly tractable — full accounting in
/// phase 7's own closing summary, `design/archive/retired-tracks.md`), and stays gated
/// after retirement as a regression ratchet, the same "I removed an `Any`" CI-checkable
/// claim every slice needed.
///
/// Counts `bynk-emit/src/**/*.rs` lines — excluding `#[cfg(test)]` test-module ranges
/// and [`TS_WRITES_EXCLUDED_FILES`] (the same files [`ts_writes`] excludes for producing
/// no TypeScript at all; an `any`-typed value there isn't R7.1's business either) —
/// matching [`line_violates_ts_any`]. Hand-written runtime `.ts` files under
/// `bynk-emit/runtime/` are out of scope by construction: [`rust_files`] only walks
/// `.rs` files, and R7.1 governs the emitted *tree*, not the hand-written runtime R7.7
/// separately covers.
fn ts_any(root: &Path) -> Probe {
    let dir = root.join("bynk-emit/src");
    Probe {
        name: "ts_any",
        gated: true,
        reads: ts_any_violations(&rust_files_relative(&dir)).to_string(),
    }
}

// --- Gated probe 12: verbatim_origins ---------------------------------------

/// P7.5 (#1307): distinct `bynk_ts::VerbatimOrigin` variants named in
/// `bynk-emit/src` — how many *families* of residual, not-yet-converted
/// emission remain, not their size (`verbatim_sites`, below, is the size).
/// Retired at an **argued floor**, named file-by-file the way `ast_importers`'s
/// floor of 5 was: **1** (ADR 0410) — only `NotYetConverted` has a live
/// production reference, at the same two sites `verbatim_sites`'s own floor
/// names permanent; full accounting in phase 7's own closing summary,
/// `design/archive/retired-tracks.md`. Read **0** at this slice's own
/// landing (`bynk-emit` built no `Verbatim` content yet, #1307's Decision C
/// — Arc C's own first slice is what gave this probe something to count);
/// stays gated after retirement as a regression ratchet.
///
/// Line-scans for `VerbatimOrigin::<Variant>` and counts distinct variant
/// names referenced, the same needle-scan shape [`hoist_sinks`] uses. A
/// known, accepted gap (review of #1308, finding 6): a bare `use
/// bynk_ts::VerbatimOrigin::Contracts;` followed by unqualified `Contracts`
/// elsewhere would undercount, since the needle is the qualified path. Not
/// worth a real-parser fix for an *argued-floor* probe (unlike
/// `verbatim_sites`'s own floor of exactly 0) — `bynk-emit`'s own existing
/// call-site style always qualifies (`TsStmt::verbatim(VerbatimOrigin::X,
/// …)`), so this is a theoretical undercount, not an observed one.
fn verbatim_origins(root: &Path) -> Probe {
    let dir = root.join("bynk-emit/src");
    Probe {
        name: "verbatim_origins",
        gated: true,
        reads: verbatim_origins_violations(&rust_files_relative(&dir)).to_string(),
    }
}

/// [`verbatim_origins`]'s counting logic, over an explicit `(relative path,
/// contents)` list — see [`rust_files_relative`] for why this isn't `root:
/// &Path`. Excludes `#[cfg(test)]` ranges the same way [`ts_any_violations`]
/// does (review of #1308, finding 6): without this, one `bynk-emit` unit
/// test constructing a `VerbatimOrigin` for its own fixture pins this probe
/// above its argued floor permanently, for a reason that has nothing to do
/// with residual production emission.
fn verbatim_origins_violations(files: &[(PathBuf, String)]) -> usize {
    let needle = "VerbatimOrigin::";
    let mut variants: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (_, contents) in files {
        let lines: Vec<&str> = contents.lines().collect();
        let ranges = test_mod_ranges(&lines);
        for (i, line) in lines.iter().enumerate() {
            if in_test_range(i, &ranges) || is_line_comment(line) {
                continue;
            }
            let mut rest = *line;
            while let Some(idx) = rest.find(needle) {
                let after = &rest[idx + needle.len()..];
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                rest = &after[name.len()..];
                if !name.is_empty() {
                    variants.insert(name);
                }
            }
        }
    }
    variants.len()
}

// --- Gated probe 13: verbatim_sites -----------------------------------------

/// P7.5 (#1307): distinct `TsStmt::verbatim(...)`/`TsExpr::VerbatimExpr(...)`
/// construction call sites in `bynk-emit/src`, line-scanned the same way
/// [`hoist_sinks`] counts `stmts: &mut Vec<String>` occurrences. Every call
/// site converting to a real tree node is what Arc C's own per-file slices
/// were actually for — `verbatim_origins` alone can't distinguish "3
/// variants, 12 residual call sites" from "3 variants, 900 residual call
/// sites, two files never decomposed"; this is what closes that gap. Retired
/// at an **argued floor** for the `TsStmt` half, not the flat 0 first
/// proposed: **2** (ADR 0399/ADR 0407, confirmed unchanged by the #1486
/// capstone) — `project.rs`'s adapter-binding copy loop (a foreign,
/// user-authored TypeScript payload) and its `runtime.ts` staging (a
/// committed npm build artifact), neither ever generated by `bynk-emit`;
/// full accounting in phase 7's own closing summary, `design/archive/
/// retired-tracks.md`.
///
/// #1539 widens the scan to the `TsExpr` half of the same escape hatch
/// (`TsExpr::VerbatimExpr`, closing the untagged-`Ident` gap the review
/// found) and moves the floor to **11** (the 2 permanent `TsStmt` sites plus
/// 9 residual `TsExpr` construction sites in `emit.rs` — a generic-typed
/// callee `Call`/`New` has no `type_args` field, a nested `As`-under-`As`
/// chain the printer's own operand-parenthesisation rule would mis-wrap, a
/// block-bodied ICU IIFE, and a `pred_condition_and_message`-style message
/// that a second `TsLit::Str` escaping pass would corrupt — see
/// `TsExpr::VerbatimExpr`'s own doc for the full list). Unlike the `TsStmt`
/// pair, these 9 are not argued-permanent the same way: each converts to a
/// real node the day `bynk-ts` gains the matching type-algebra piece
/// (`type_args`, a parenthesisation fix, …), so this half of the floor is
/// expected to keep shrinking, tracked here rather than assumed fixed. Read
/// **0** at this slice's own landing, same reason `verbatim_origins` did;
/// stays gated after retirement as a regression ratchet.
fn verbatim_sites(root: &Path) -> Probe {
    let dir = root.join("bynk-emit/src");
    Probe {
        name: "verbatim_sites",
        gated: true,
        reads: verbatim_sites_violations(&rust_files_relative(&dir)).to_string(),
    }
}

/// [`verbatim_sites`]'s counting logic, over an explicit `(relative path,
/// contents)` list — see [`rust_files_relative`] for why this isn't `root:
/// &Path`. Excludes `#[cfg(test)]` ranges the same way [`ts_any_violations`]
/// does (review of #1308, finding 6): `verbatim_sites` is documented as
/// retiring at 0, so a residual construction site inside a test fixture
/// would pin it above zero permanently for a reason that has nothing to do
/// with production emission conversion.
fn verbatim_sites_violations(files: &[(PathBuf, String)]) -> usize {
    let needles = ["TsStmt::verbatim(", "TsExpr::VerbatimExpr("];
    let mut count = 0usize;
    for (_, contents) in files {
        let lines: Vec<&str> = contents.lines().collect();
        let ranges = test_mod_ranges(&lines);
        for (i, line) in lines.iter().enumerate() {
            if in_test_range(i, &ranges) || is_line_comment(line) {
                continue;
            }
            if needles.iter().any(|needle| line.contains(needle)) {
                count += 1;
            }
        }
    }
    count
}

/// Named identifiers imported from the compiler-generated firstparty/runtime relative
/// modules (`./bynk.js`, `./runtime.js`, `./bynk/locale/types.js`, `./cloudflare.js`,
/// or their `../` forms) — `import type { A, B }`/`import { A, B }` braces, stripping
/// `type ` markers and `X as Y` aliases (keeping the imported name, not the local one,
/// since the allowlists are about what's referenced, not what it's called locally).
fn ts_named_imports_from_runtime_modules(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        if !line.starts_with("import") {
            continue;
        }
        let is_runtime_module = ["\"./bynk.js\"", "\"./runtime.js\"", "\"../runtime.js\""]
            .iter()
            .any(|m| line.ends_with(&format!("from {m};")))
            || line.contains("__bynk_runtime/")
            || line.contains("bynk/locale/types.js")
            || line.contains("cloudflare.js");
        if !is_runtime_module {
            continue;
        }
        let Some(open) = line.find('{') else { continue };
        let Some(close) = line.find('}') else {
            continue;
        };
        for part in line[open + 1..close].split(',') {
            let part = part.trim().trim_start_matches("type ").trim();
            if part.is_empty() {
                continue;
            }
            let imported = part.split(" as ").next().unwrap_or(part).trim();
            out.push(imported.to_string());
        }
    }
    out
}

// --- Gated probe 14: incremental_query_types --------------------------------

/// Phase 8's own completion criterion (`design/bynk-compiler-trajectory.md` §3,
/// "keystroke-to-diagnostic latency by query level"), settled by #1509 (Q5, ADR 0414;
/// `design/tracks/incrementality.md` §5) as a one-time **existence** proof, not a count
/// trending toward a floor the way every other gated probe in this module is shaped —
/// R3.13/R3.14 describe a property to construct, not a defect to exhaust, so a
/// shrinking count would be the wrong shape regardless of how it was tuned.
///
/// **Re-settled by #1537 (2 September 2026), after the 30 August post-restructuring
/// review found the probe could not tell adoption from existence.** Phase 8 built all
/// four R3.13 levels; only the file level (P8.4's shared parse cache) and the unit
/// level's *proof* (P8.2's stability test over `UnitSignature`) had a consumer. The
/// definition level (`Body(DefId)`/`TypeOf(DefId)`, 816 lines) and the project level
/// (`ProjectGraph`, 174 lines) were reachable only from their own tests, with no
/// scheduler to call them and — per R3.15 and #1523 — no trigger yet for one. Both
/// were deleted rather than left "available but unwired" (P5), the same decision the
/// IR cutover (#1542) reached for phase 6's expression IR. This probe now certifies
/// the decision, in both directions:
///
/// 1. **Unit level** — `struct UnitSignature` exists as real code in `bynk-check`
///    (P8.1, ADR 0412). It is the R3.14 firewall's own specification, and the one
///    phase 8 artefact #1523's trigger presupposes; it stays as a *proof*, not a
///    production path (its only reader is clause 3's test), argued in #1537's ADR.
/// 2. **Shared cache** — the file-level parse cache has migrated off
///    `PROJECT_UNIT_CACHE` (`bynk-ide/src/completion.rs`) onto one shared,
///    `bynk-project`-owned cache (P8.4, ADR 0413). Checked two ways: the old static
///    gone from `bynk-ide/src`, *and* some cache-shaped `static`/`struct` present in
///    `bynk-project/src` — absence alone would read "migrated" for a bare deletion.
/// 3. **Stability test** — some `#[test]` under `bynk-check/tests/` proves
///    `UnitSignature`'s stability under a body edit (P8.2): any test name containing
///    both `unit_signature` and `stab`.
/// 4. **Definition and project levels absent** — no `struct ProjectGraph`, and no
///    `DefId`-keyed `fn body(`/`fn type_of(`, in **any** workspace crate's `src/`
///    ([`workspace_crate_src_files`], the same walk `unconsumed_ir_items` uses, minus
///    `xtask` itself, whose source spells the needles) — not just the two crates
///    phase 8 landed them in, since R3.13's own table
///    assigns `DefId` bindings to a `bynk-resolve` crate that does not exist yet and
///    a rebuild might put them there (review of #1582). `checker.rs`'s own
///    pre-existing per-expression `type_of`, which has no `DefId` parameter, does
///    not count — the false positive #1510's first run caught, now with the opposite
///    consequence. This clause is what makes the probe a gate on #1537 rather than a
///    memorial: re-adding either level changes the committed reading and fails the
///    currency test, so it needs a consumer and a re-settling — the trigger
///    R3.15/#1523 names. **What the gate does not see, stated rather than asserted
///    away:** a `DefId` parameter wrapped onto the line after `fn body(` (the
///    same-line rule [`defid_query_fn_present`] documents), or a query function under
///    any other name. Text-level, like every probe in this file.
///
/// Every clause is a static read of the tree (never a nested build or test run — see
/// [`unit_signature_present`]'s own doc comment for why a "does the stability test
/// *pass*" clause was deliberately rejected, #1510's own review-shaped framing).
fn incremental_query_types(root: &Path) -> Probe {
    let check_src = rust_files(&root.join("bynk-check/src"));
    let project_src = rust_files(&root.join("bynk-project/src"));
    let ide_src = rust_files(&root.join("bynk-ide/src"));
    let check_tests = rust_files(&root.join("bynk-check/tests"));

    let unit_present = unit_signature_present(&check_src);
    let cache_migrated = shared_cache_migrated(&ide_src, &project_src);
    let test_present = stability_test_present(&check_tests);
    // Every workspace crate but this one: the harness's own source spells the
    // needles it scans for (in these very functions and their tests), so it
    // would read as a re-add of both levels on every run.
    let workspace_src: Vec<(PathBuf, String)> = workspace_crate_src_files(root)
        .into_iter()
        .filter(|(krate, _, _)| krate != "xtask")
        .map(|(_, path, contents)| (path, contents))
        .collect();
    let readded = deleted_levels_present(&workspace_src);

    let reads = format!(
        "unit_signature {}; shared_cache {}; stability_test {}; definition/project levels {}",
        if unit_present { "present" } else { "absent" },
        if cache_migrated {
            "migrated"
        } else {
            "not migrated (PROJECT_UNIT_CACHE still bynk-ide-local)"
        },
        if test_present { "present" } else { "absent" },
        if readded.is_empty() {
            "absent (deleted by #1537)".to_string()
        } else {
            format!(
                "re-added ({}) — need a consumer and a re-settling of #1537",
                readded.join(", ")
            )
        },
    );
    Probe {
        name: "incremental_query_types",
        gated: true,
        reads,
    }
}

/// Clause 1 of [`incremental_query_types`]: does `struct UnitSignature` exist as real
/// code (not a comment or doc prose) in `bynk-check`? The same "grep for the real
/// identifier, not the doc claim" discipline `design/tracks/incrementality.md` §1 used
/// to measure this reading as zero at settling. Deliberately *not* a "does P8.2's
/// fixture pass" check: every gated probe here is a static read of the tree, computed
/// from inside `xtask/tests/greenfield_status.rs`'s own `#[test]`; shelling out to
/// `cargo test` from inside a running `cargo test` is the identical nested-invocation
/// cost [`wildcard_arms`] (the one probe that shells out, and stays trend-only for
/// exactly this reason) avoids.
fn unit_signature_present(check_src: &[(PathBuf, String)]) -> bool {
    any_real_code_line(check_src, "struct UnitSignature")
}

/// Clause 4 of [`incremental_query_types`]: which of the two levels #1537 deleted are
/// back anywhere in `src` — `ProjectGraph` as a real struct, or a `DefId`-keyed
/// `body`/`type_of` query function ([`defid_query_fn_present`]). Empty is the
/// committed reading; any entry changes the table and fails the currency gate, which
/// is the point.
fn deleted_levels_present(src: &[(PathBuf, String)]) -> Vec<&'static str> {
    let mut found = Vec::new();
    if any_real_code_line(src, "struct ProjectGraph") {
        found.push("ProjectGraph");
    }
    if defid_query_fn_present(src, "fn body(") {
        found.push("Body");
    }
    if defid_query_fn_present(src, "fn type_of(") {
        found.push("TypeOf");
    }
    found
}

/// A `fn_needle`-matching signature line that *also* names `DefId` on the same line —
/// not just `fn_needle` alone. **A real, empirically-confirmed false positive this
/// slice's own first run caught, not a hypothetical:** `bynk-check/src/checker.rs`
/// already has a `pub(crate) fn type_of(expr: &Expr, expected: Option<TyId>, ctx: &mut
/// Ctx) -> Option<TyId>` — real, pre-existing, ordinary per-expression type-inference
/// plumbing that predates this whole track and has nothing to do with R3.13's
/// `DefId`-keyed query — a naive `fn type_of(` scan reads this as `TypeOf` already
/// existing on the very first run, before P8.5 does any work at all. Requiring
/// `DefId` on the same signature line is a real, if narrow, precision fix: it correctly
/// reads false against `checker.rs`'s own `type_of` today, and correctly flips true
/// once P8.5 lands a real `DefId`-keyed function, whatever it ends up calling it, as
/// long as the parameter appears on the `fn` line itself (a wrapped multi-line
/// signature would need widening this scan window — not needed for any function in
/// the tree today).
fn defid_query_fn_present(files: &[(PathBuf, String)], fn_needle: &str) -> bool {
    files.iter().any(|(_, contents)| {
        contents.lines().any(|line| {
            !is_line_comment(line) && line.contains(fn_needle) && line.contains("DefId")
        })
    })
}

/// Whether the file-level parse cache has migrated off `bynk-ide`'s own
/// `PROJECT_UNIT_CACHE` onto some shared cache in `bynk-project` — see
/// [`incremental_query_types`]'s own doc comment (clause 2) for why *both* halves are
/// checked: absence from `bynk-ide` alone cannot distinguish a real migration from a
/// bare rename or deletion with nothing shared put in its place. The needle is
/// anchored on the `static` declaration line (`static PROJECT_UNIT_CACHE`), not a bare
/// substring, so `PROJECT_UNIT_CACHE_CAP` (a real, unrelated `const` in
/// `bynk-ide/src/completion.rs`) can't hold this false on its own.
fn shared_cache_migrated(ide_src: &[(PathBuf, String)], project_src: &[(PathBuf, String)]) -> bool {
    !any_real_code_line(ide_src, "static PROJECT_UNIT_CACHE")
        && cache_shaped_item_present(project_src)
}

/// Whether `bynk-project/src` has a `static`/`struct` item whose name mentions "cache"
/// (case-insensitive) — the only crate-boundary-checkable proxy for "some shared cache
/// now lives where P8.4 is meant to put it," until that slice pins the real identifier
/// down. Deliberately loose, the same "exact name not yet proposed" reasoning
/// [`stability_test_present`] already uses.
fn cache_shaped_item_present(project_src: &[(PathBuf, String)]) -> bool {
    project_src.iter().any(|(_, contents)| {
        contents.lines().any(|line| {
            if is_line_comment(line) {
                return false;
            }
            let trimmed = line.trim_start();
            let is_item = trimmed.starts_with("static ")
                || trimmed.starts_with("pub static ")
                || trimmed.starts_with("struct ")
                || trimmed.starts_with("pub struct ");
            is_item && line.to_lowercase().contains("cache")
        })
    })
}

/// Whether any `#[test]` fn under `bynk-check/tests/` looks like P8.2's own
/// body-edit-stability property test — see [`incremental_query_types`]'s own doc
/// comment (clause 3) for why the name match (`unit_signature` + `stab`) is
/// deliberately loose. The `#[test]` attribute itself is *not* loose: it must be on the
/// matching `fn` line or on a contiguous run of attribute lines directly above it, so a
/// same-named non-test helper (a fixture builder, say) can't satisfy this clause.
fn stability_test_present(check_tests: &[(PathBuf, String)]) -> bool {
    check_tests.iter().any(|(_, contents)| {
        let lines: Vec<&str> = contents.lines().collect();
        lines.iter().enumerate().any(|(i, line)| {
            if is_line_comment(line) {
                return false;
            }
            let lower = line.to_lowercase();
            let name_matches =
                lower.contains("fn ") && lower.contains("unit_signature") && lower.contains("stab");
            if !name_matches {
                return false;
            }
            line.contains("#[test]")
                || lines[..i]
                    .iter()
                    .rev()
                    .take_while(|l| l.trim_start().starts_with('#'))
                    .any(|l| l.trim() == "#[test]")
        })
    })
}

/// Whether any line in `files` (excluding comments) contains `needle` — the shared
/// existence-check primitive [`unit_signature_present`]/[`deleted_levels_present`]/
/// [`shared_cache_migrated`] all use.
fn any_real_code_line(files: &[(PathBuf, String)], needle: &str) -> bool {
    files.iter().any(|(_, contents)| {
        contents
            .lines()
            .any(|line| !is_line_comment(line) && line.contains(needle))
    })
}

// --- Gated probe 15: unconsumed_ir_items ------------------------------------

/// Slice D3 of #1542 (`design/archive/retired-tracks.md`, the IR cutover's own
/// closing summary): the adoption probe the 30 August 2026 post-restructuring
/// review (`design/reviews/2026-08-30-post-restructuring-review.md`, Part 5 §8)
/// asked for — for each `pub` item in `bynk-ir/src` and `bynk-lower/src`, does
/// a production call site exist outside the owning crate and outside a test
/// module? That review found phase 6 had shipped an expression IR nothing
/// consumed (fifteen `bynk-lower` entry points, twenty-one `bynk-ir` types),
/// invisible to every existing gate because each of them certifies that a name
/// *exists* in a directory, not that anything *reads* it. Scoped to the two IR
/// crates because their entire purpose is to be consumed elsewhere: a `pub`
/// item in either with no reader in another crate is, by construction, either
/// dead or a second path waiting to be wired in — the P5 failure
/// (`bynk-greenfield-compiler.md`) both phases 6 and 8 reproduced.
///
/// Reads **0** at its own landing, by construction: Slices D0–D2 deleted every
/// unconsumed item (D1 also demoted the two crate-internal helpers,
/// `lower_fn_sig_ir_from_types`/`lower_op_sig_ir_from_commons`, that would
/// otherwise have read as 2). Gated as a ratchet: a new `pub` item in either
/// crate with no consumer moves it off zero and fails
/// `greenfield_status_table_is_current`, so the "available but unwired" state
/// cannot land silently again. The reading names the offending items so the
/// failure is actionable, not just a count.
///
/// **What counts as a consumer.** A non-comment line, outside any `#[cfg(test)]
/// mod` range ([`test_mod_ranges`]), in a workspace crate's `src/` that is
/// **neither owner crate**, containing the item's name as a whole word. The
/// two owners do not vouch for each other, on purpose (review of this slice's
/// own PR, #1581): run against pre-D0 `main` with only the owning crate
/// excluded, every one of phase 6's twenty-one unconsumed `bynk-ir` types
/// would have read as consumed, because `bynk-lower`'s own unconsumed
/// constructors named them — the probe would have missed half the surface it
/// was built to see. With both excluded, its first honest run read 5
/// (`IndexIr` and the four `MUTATING_*_OPS` tables, read only from
/// `bynk-lower`), resolved by inlining the alias and moving the tables beside
/// their one reader rather than arguing a floor. Text-level, like every probe
/// in this file — a name that happens to be shared with an unrelated item in
/// another crate would read as consumed (a false negative for the ratchet,
/// never a false positive that blocks a PR), accepted the same way
/// [`ts_writes`]'s own known over-count is. Items are `pub` at column zero
/// only — `fn`, `struct`, `enum`, `type`, `const`, `static`, `trait`, `union`,
/// with any `async`/`unsafe`/`const`/`extern` qualifier on a `fn` — see
/// [`column_zero_pub_item_name`]: `pub(crate)` is by definition not offered
/// to another crate, a `pub` item nested inside an `impl` block is reachable
/// only through its owner (which is what gets counted), and `pub use`/`pub
/// mod` re-export rather than declare.
fn unconsumed_ir_items(root: &Path) -> Probe {
    let owners = ["bynk-ir", "bynk-lower"];
    let mut owner_files = Vec::new();
    for owner in owners {
        for (path, contents) in rust_files_relative(&root.join(owner).join("src")) {
            owner_files.push((owner.to_string(), path, contents));
        }
    }
    let consumer_files = workspace_crate_src_files(root);
    let unconsumed = unconsumed_pub_items(&owners, &owner_files, &consumer_files);
    let reads = if unconsumed.is_empty() {
        "0".to_string()
    } else {
        format!("{} ({})", unconsumed.len(), unconsumed.join(", "))
    };
    Probe {
        name: "unconsumed_ir_items",
        gated: true,
        reads,
    }
}

/// Every `.rs` file under `<crate>/src` for every workspace crate — each entry
/// tagged with its crate directory name so [`unconsumed_pub_items`] can exclude
/// an item's own crate. A workspace crate is any immediate child of `root` with
/// both a `Cargo.toml` and a `src/`, the same shape every `members` entry in the
/// root manifest has; reading the manifest itself would add a TOML parse for no
/// gain in precision.
fn workspace_crate_src_files(root: &Path) -> Vec<(String, PathBuf, String)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    let mut crates: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("Cargo.toml").is_file() && p.join("src").is_dir())
        .collect();
    crates.sort();
    for krate in crates {
        let name = krate
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for (path, contents) in rust_files_relative(&krate.join("src")) {
            out.push((name.clone(), path, contents));
        }
    }
    out
}

/// [`unconsumed_ir_items`]'s counting logic over explicit `(crate, relative
/// path, contents)` lists — see [`rust_files_relative`] for why this isn't
/// `root: &Path`. Returns `crate::item` for every column-zero `pub` item in
/// `owner_files` that no non-comment, non-test line in a file belonging to a
/// crate outside `owners` names as a whole word; sorted and deduplicated (a
/// name declared in two files of one crate is one item, not two), so the
/// reading is stable across runs and across a crate being split into modules.
///
/// Each consumer file is split and its `#[cfg(test)]` ranges computed once,
/// up front, not once per candidate item (review of #1581): this runs under
/// `cargo test --workspace` on every Rust-touching PR, and the failure path —
/// an item with no consumer — is exactly the one that scans every file.
fn unconsumed_pub_items(
    owners: &[&str],
    owner_files: &[(String, PathBuf, String)],
    consumer_files: &[(String, PathBuf, String)],
) -> Vec<String> {
    let mut items: Vec<(String, String)> = Vec::new();
    for (krate, _, contents) in owner_files {
        let lines: Vec<&str> = contents.lines().collect();
        let ranges = test_mod_ranges(&lines);
        for (i, line) in lines.iter().enumerate() {
            if in_test_range(i, &ranges) {
                continue;
            }
            if let Some(name) = column_zero_pub_item_name(line) {
                items.push((krate.clone(), name.to_string()));
            }
        }
    }
    items.sort();
    items.dedup();

    // Production, non-owner lines only — preprocessed once.
    let consumer_lines: Vec<&str> = consumer_files
        .iter()
        .filter(|(krate, _, _)| !owners.contains(&krate.as_str()))
        .flat_map(|(_, _, contents)| {
            let lines: Vec<&str> = contents.lines().collect();
            let ranges = test_mod_ranges(&lines);
            lines
                .iter()
                .enumerate()
                .filter(|(i, line)| !in_test_range(*i, &ranges) && !is_line_comment(line))
                .map(|(_, line)| *line)
                .collect::<Vec<_>>()
        })
        .collect();

    items
        .iter()
        .filter(|(_, name)| !consumer_lines.iter().any(|line| contains_word(line, name)))
        .map(|(owner, name)| format!("{owner}::{name}"))
        .collect()
}

/// The name of a column-zero `pub` item declaration, if `line` is one: `pub`
/// followed by `fn`, `struct`, `enum`, `type`, `const`, `static`, `trait` or
/// `union`, where a `fn` may carry `async`/`unsafe`/`const`/`extern "…"`
/// qualifiers in any order. `pub(crate)`/`pub(super)` do not qualify (they
/// are not offered to other crates), and neither does anything indented (a
/// method or associated item, reachable only through its owner), nor `pub
/// use`/`pub mod` (re-exports and module declarations, not items).
fn column_zero_pub_item_name(line: &str) -> Option<&str> {
    let mut rest = line.strip_prefix("pub ")?;
    // `pub const fn` / `pub async unsafe fn` / `pub unsafe extern "C" fn` …:
    // peel qualifiers until the item keyword is exposed. A bare `pub const X`
    // is a constant, not a qualifier, so `const` only peels when a `fn`
    // (possibly behind further qualifiers) follows it.
    loop {
        if let Some(r) = rest
            .strip_prefix("async ")
            .or_else(|| rest.strip_prefix("unsafe "))
        {
            rest = r;
            continue;
        }
        if let Some(r) = rest.strip_prefix("extern ") {
            // `extern "C" fn` — skip the ABI string if present.
            let r = r.trim_start();
            let r = if let Some(after_quote) = r.strip_prefix('"') {
                after_quote
                    .find('"')
                    .map(|q| after_quote[q + 1..].trim_start())
                    .unwrap_or(r)
            } else {
                r
            };
            rest = r;
            continue;
        }
        if let Some(r) = rest.strip_prefix("const ")
            && (r.starts_with("fn ")
                || r.starts_with("async ")
                || r.starts_with("unsafe ")
                || r.starts_with("extern "))
        {
            rest = r;
            continue;
        }
        break;
    }
    let rest = [
        "fn ", "struct ", "enum ", "type ", "const ", "static ", "trait ", "union ",
    ]
    .iter()
    .find_map(|kw| rest.strip_prefix(kw))?;
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// Does `line` contain `word` as a whole identifier — not as a prefix or
/// suffix of a longer one (`IrExpr` inside `IrExprKind` must not count)?
fn contains_word(line: &str, word: &str) -> bool {
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(pos) = line[from..].find(word) {
        let start = from + pos;
        let end = start + word.len();
        let before_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_ident_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

// --- Reported probe 1: wildcard_arms --------------------------------------

/// R2.12. `clippy::wildcard_enum_match_arm` diagnostics, forced on via `-W` so the
/// count is real from day one and doesn't wait on `workspace_lints`/T0.3 (#999 Decision
/// C — delegating to clippy's own type-aware pass, rather than a hand-rolled scan for
/// "compiler-owned enum", so the probe and the enforcement mechanism can never
/// disagree). A count, not a boolean — moves on nearly every match statement anyone
/// writes, so it is reported, not gated (#999 Decision D).
fn wildcard_arms(root: &Path) -> Probe {
    let reads = match run_clippy_wildcard_scan(root) {
        Ok(n) => n.to_string(),
        Err(e) => format!("error running clippy: {e}"),
    };
    Probe {
        name: "wildcard_arms",
        gated: false,
        reads,
    }
}

/// Runs clippy with the lint forced on and parses the NDJSON output properly —
/// **not** a substring count. A single `wildcard_enum_match_arm` diagnostic's JSON
/// repeats the lint name several times (the `code` field, the human-readable message,
/// the `#[warn(...)]` note, and the `rendered` field duplicating the whole thing as
/// text), so `stdout.matches("wildcard_enum_match_arm").count()` overcounts by roughly
/// 3x — caught by cross-checking this probe's own first run against a real JSON parse
/// (296 real diagnostics, not the naive scan's 888).
///
/// Checks the process exit status: a forced `-W` (not `-D`) never fails the build on
/// account of the lint itself, so a non-zero exit means clippy genuinely could not run
/// (a compile error elsewhere, a missing toolchain component, offline with no cached
/// index) — in which case stdout carries no `compiler-message` lines and a silent
/// success would report a false, and indistinguishable, `0`. This probe is reported,
/// not gated, precisely so an honest "couldn't measure" surfaces loudly here rather
/// than being read as "closed."
fn run_clippy_wildcard_scan(root: &Path) -> std::io::Result<usize> {
    let output = Command::new("cargo")
        .args([
            "clippy",
            "--workspace",
            "--message-format=json",
            "--",
            "-W",
            "clippy::wildcard_enum_match_arm",
        ])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "cargo clippy exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut count = 0usize;
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
            continue;
        }
        let code = value.pointer("/message/code/code").and_then(|c| c.as_str());
        if code == Some("clippy::wildcard_enum_match_arm") {
            count += 1;
        }
    }
    Ok(count)
}

// --- Reported probe 2: keep_in_sync ---------------------------------------

/// P2 (trend only). Comments across the workspace containing "in sync", "mirrors",
/// "parity", or "must match" — each one names a rule the compiler cannot teach itself
/// and must be taught in review, every time.
fn keep_in_sync(root: &Path) -> Probe {
    let phrases = ["in sync", "mirrors", "parity", "must match"];
    let mut count = 0usize;
    for dir in top_level_crate_dirs(root) {
        for (_, contents) in rust_files(&dir.join("src")) {
            for line in contents.lines() {
                if is_line_comment(line) {
                    let lower = line.to_lowercase();
                    if phrases.iter().any(|p| lower.contains(p)) {
                        count += 1;
                    }
                }
            }
        }
    }
    Probe {
        name: "keep_in_sync",
        gated: false,
        reads: count.to_string(),
    }
}

// --- Reported probe 3: test_density ---------------------------------------

/// R11.1, and §3.4's phase-3 trigger. Per crate: (lines inside `#[test]` fn bodies,
/// plus lines inside `#[cfg(test)] mod` blocks outside those fns) ÷ (non-blank,
/// non-comment lines under that crate's `src/`) — #999 Decision F's definition,
/// written down precisely because an undefined "ratio" is exactly the ambiguity that
/// produced the track doc §9's four-row ambiguity.
fn test_density(root: &Path) -> Probe {
    let mut parts = Vec::new();
    for dir in top_level_crate_dirs(root) {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let src_dir = dir.join("src");
        let mut test_lines = 0usize;
        let mut code_lines = 0usize;
        for (_, contents) in rust_files(&src_dir) {
            let lines: Vec<&str> = contents.lines().collect();
            let ranges = test_mod_ranges(&lines);
            for (i, line) in lines.iter().enumerate() {
                let is_blank_or_comment = line.trim().is_empty() || is_line_comment(line);
                if !is_blank_or_comment {
                    code_lines += 1;
                }
                if in_test_range(i, &ranges) && !is_blank_or_comment {
                    test_lines += 1;
                }
            }
        }
        if code_lines > 0 {
            let ratio = 100.0 * test_lines as f64 / code_lines as f64;
            parts.push(format!("{name}={ratio:.1}%"));
        }
    }
    Probe {
        name: "test_density",
        gated: false,
        reads: parts.join(", "),
    }
}

// --- diagnostic_coverage ---------------------------------------------------

/// #1662 (track #1648, G2). Registry codes that some test **asserts**, out of all
/// `bynk_syntax::diagnostics::REGISTRY` codes, and the count no test asserts.
///
/// Static, so it runs inside this harness: the review that set the baseline
/// (2026-10-01, #1647 Part 4) instrumented `CompileError::new` across a full
/// `cargo test --workspace`, which this harness cannot afford. Asserted is a
/// subset of produced (a passing test that names a code saw it), so driving the
/// unasserted count to the argued floor meets "every reachable code is produced
/// by a test" a fortiori.
///
/// A code counts as asserted when it appears in:
/// - line 1 of a negative fixture's `expected_error.txt` (line 2 is a message
///   substring and may quote other codes);
/// - any other non-`.bynk` file under a crate's `tests/` directory (test
///   sources, expected-diagnostics files, JSON goldens). `.bynk` sources are
///   skipped, because their comments often name the code they provoke, and Rust
///   sources are read with their `//` comments removed, for the same reason;
/// - a `#[cfg(test)] mod` block in a crate's `src/`, comments removed;
/// - a blessed diagnostic transcript, `site/src/diagnostics/*.txt`.
///
/// Gated at the argued floor of **4** (#1662 Decision B). Each of the four is
/// emitted where no compiler test can reach it:
/// - `bynk.deploy.contract_skew`: `bynk deploy`, against a live deployment's
///   lock;
/// - `bynk.project.read_failed`: only when a host's file overlay omits a
///   discovered file, which neither the CLI nor the LSP does;
/// - `bynk.target.vendor_conflict`: needs platform-native capabilities from two
///   platforms, and only Cloudflare ships any today (the decision function is
///   unit-tested);
/// - `bynk.wasm.strip_failed`: stripping the compiler's own emitted TypeScript,
///   which fails only on an emitter bug.
///
/// `cargo xtask greenfield-status --list-unasserted` prints the codes.
pub fn unasserted_codes(root: &Path) -> Vec<&'static str> {
    let registry: BTreeSet<&'static str> = bynk_syntax::diagnostics::REGISTRY
        .iter()
        .map(|d| d.code)
        .collect();
    let mut asserted: BTreeSet<String> = BTreeSet::new();
    let mut note = |text: &str| {
        for code in registry_tokens(text) {
            asserted.insert(code);
        }
    };
    for krate in top_level_crate_dirs(root) {
        for (path, contents) in text_files(&krate.join("tests")) {
            if path.extension().is_some_and(|e| e == "bynk") {
                continue;
            }
            if path.file_name().is_some_and(|n| n == "expected_error.txt") {
                note(
                    contents
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or(""),
                );
            } else if path.extension().is_some_and(|e| e == "rs") {
                note(&strip_line_comments(&contents));
            } else {
                note(&contents);
            }
        }
        for (_, contents) in rust_files(&krate.join("src")) {
            let lines: Vec<&str> = contents.lines().collect();
            for (start, end) in test_mod_ranges(&lines) {
                note(&strip_line_comments(&lines[start..=end].join("\n")));
            }
        }
    }
    for (path, contents) in text_files(&root.join("site/src/diagnostics")) {
        if path.extension().is_some_and(|e| e == "txt") {
            note(&contents);
        }
    }
    registry
        .into_iter()
        .filter(|c| !asserted.contains(*c))
        .collect()
}

/// `src` with every `//` comment (including `///` and `//!` doc comments)
/// removed, so a code named only in prose does not count as asserted. A `//`
/// inside a string literal is kept; block comments are rare enough in this
/// workspace to leave.
fn strip_line_comments(src: &str) -> String {
    src.lines()
        .map(|line| {
            let bytes = line.as_bytes();
            let (mut in_str, mut escaped) = (false, false);
            for (i, &b) in bytes.iter().enumerate() {
                if escaped {
                    escaped = false;
                } else if b == b'\\' && in_str {
                    escaped = true;
                } else if b == b'"' {
                    in_str = !in_str;
                } else if b == b'/' && !in_str && bytes.get(i + 1) == Some(&b'/') {
                    return &line[..i];
                }
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `bynk.<family>.<name>` token in `text` — the shape of a registry code,
/// whether quoted (Rust source) or bare (fixtures, transcripts).
fn registry_tokens(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'.';
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = text[i..].find("bynk.") {
        let start = i + rel;
        let mut end = start;
        while end < bytes.len() && ident(bytes[end]) {
            end += 1;
        }
        if start == 0 || !ident(bytes[start - 1]) {
            out.push(text[start..end].trim_end_matches('.').to_string());
        }
        i = end.max(start + 1);
    }
    out
}

/// Every readable UTF-8 file under `dir`, recursively, as `(path, contents)`.
fn text_files(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(contents) = std::fs::read_to_string(&path) {
                out.push((path, contents));
            }
        }
    }
    out
}

fn diagnostic_coverage(root: &Path) -> Probe {
    let total = bynk_syntax::diagnostics::REGISTRY.len();
    let unasserted = unasserted_codes(root).len();
    Probe {
        name: "diagnostic_coverage",
        gated: true,
        reads: format!(
            "unasserted={unasserted} (asserted {}/{total})",
            total - unasserted
        ),
    }
}

// --- Reported probe 4: fixture_kinds --------------------------------------

/// R11.2. Fixture directories under `bynkc/tests` using each assertion granularity —
/// `expected_contains.txt` / `expected_absent.txt` / `expected_diagnostics.txt` — set
/// against the older, coarser `expected_error.txt` (category-string) convention.
///
/// #1660 (runtime-semantics track §3.5) added two counts:
/// - `warnings` (`expected_warnings.txt`, a positive fixture's pinned warnings), which
///   this probe had never counted;
/// - `run` (`expected_run.txt`), the positive fixtures whose `suite`s
///   `bynkc/tests/behaviour_fixtures.rs` actually *runs*, the one granularity that
///   asserts runtime behaviour rather than emitted text or diagnostics.
fn fixture_kinds(root: &Path) -> Probe {
    let tests_dir = root.join("bynkc/tests");
    let contains = count_files_named(&tests_dir, "expected_contains.txt");
    let absent = count_files_named(&tests_dir, "expected_absent.txt");
    let diagnostics = count_files_named(&tests_dir, "expected_diagnostics.txt");
    let error = count_files_named(&tests_dir, "expected_error.txt");
    let warnings = count_files_named(&tests_dir, "expected_warnings.txt");
    let run = count_files_named(&tests_dir, "expected_run.txt");
    Probe {
        name: "fixture_kinds",
        gated: false,
        reads: format!(
            "contains={contains}, absent={absent}, diagnostics={diagnostics}, error={error}, warnings={warnings}, run={run}"
        ),
    }
}

fn count_files_named(dir: &Path, filename: &str) -> usize {
    let mut count = 0usize;
    count_files_named_walk(dir, filename, &mut count);
    count
}

fn count_files_named_walk(dir: &Path, filename: &str, count: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count_files_named_walk(&path, filename, count);
        } else if path.file_name().is_some_and(|n| n == filename) {
            *count += 1;
        }
    }
}

// --- Reported probe 5: keystroke_latency ------------------------------------

/// Phase 8's own trend-only probe (`design/bynk-compiler-trajectory.md` §3,
/// "keystroke-to-diagnostic latency by query level") — settled (Q3/Q5, ADR 0414;
/// `design/tracks/incrementality.md` §5) as staying **"not measured" for this whole
/// phase's lifetime**: R3.15's scheduler decision defers whole (no memo table, salsa or
/// otherwise, ships in phase 8), and the literal latency number presupposes query
/// levels attributing latency to — levels [`incremental_query_types`] itself proves
/// exist, but attributing real latency to them needs a scheduler this phase
/// deliberately does not build. Added now, not deferred to whenever a scheduler
/// exists, so the trajectory's own §3.0 baseline table carries a live, CI-computed row
/// instead of a static doc claim — the same "instrument even a number that won't move
/// yet" precedent `test_density`/`fixture_kinds` already set for this module.
fn keystroke_latency(_root: &Path) -> Probe {
    Probe {
        name: "keystroke_latency",
        gated: false,
        reads: "not measured — no scheduler exists yet (R3.15, deferred whole this phase)"
            .to_string(),
    }
}

// --- Rendering + diffing ---------------------------------------------------

/// The committed table: a plain Markdown table, probe name → gated?/reads, plus a
/// pointer to the rule ledger `stamp::apply` writes (#1001).
pub fn render_table(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("<!-- GENERATED FILE — do not edit by hand.\n");
    out.push_str("     Source: cargo xtask greenfield-status (xtask/src/greenfield_status.rs).\n");
    out.push_str("     Regenerate with: cargo xtask greenfield-status --apply -->\n\n");
    out.push_str("# Greenfield status\n\n");
    out.push_str(
        "Track slice T0.0 (#999); `ts_writes`/`ts_any` added by P7.0 (#1296); \
         `verbatim_origins`/`verbatim_sites` added by P7.5 (#1307); \
         `incremental_query_types`/`keystroke_latency` added by P8.0 (#1510); \
         `unconsumed_ir_items` added by Slice D3 of the IR cutover (#1542); \
         `diagnostic_coverage` added by #1662. Sixteen \
         probes are gated — a disagreement between this file and a fresh run fails \
         `greenfield_status_table_is_current` (`xtask/tests/greenfield_status.rs`). \
         Five are trend probes, reported only.\n\n",
    );
    out.push_str("| Probe | Gated | Reads |\n|---|---|---|\n");
    for probe in &report.probes {
        let _ = writeln!(
            out,
            "| `{}` | {} | {} |",
            probe.name,
            if probe.gated { "yes" } else { "no (trend)" },
            probe.reads
        );
    }

    out.push_str("\n## Rules closed\n\n");
    // A static, unconditional link — not a count, and not even an existence
    // check. A first draft read `design/greenfield-status-rules.md` here to
    // report a row count, but nothing regenerates *this* file when `stamp`
    // writes the ledger (`stamp.yml` never runs `greenfield-status --apply`,
    // and the gating test only diffs the nine probes) — so a count or an
    // exists/doesn't-exist message would silently go stale the moment the
    // first `closes_rule` landed, which is exactly the drift this section
    // exists to avoid, not invite (#1001 review). Static text can't go stale;
    // the ledger is one click away either way.
    out.push_str(
        "See [`design/greenfield-status-rules.md`](greenfield-status-rules.md) for rule ids \
         closed so far (written by `cargo xtask stamp --apply` at merge; may not exist yet if \
         no increment has cited `closes_rule`).\n",
    );
    out
}

/// Every gated probe whose live reading disagrees with the committed table's, as
/// `(probe name, committed, live)`. Trend probes are never compared, and never
/// computed here — this only runs the sixteen gated probes, so checking currency never
/// pays for `wildcard_arms`'s workspace-wide clippy pass. For a caller that has already
/// run the full report (e.g. to print it), use [`gated_disagreements_in`] instead so the
/// sixteen gated probes aren't computed a second time.
pub fn gated_disagreements(root: &Path) -> Vec<(String, String, String)> {
    gated_disagreements_in(&run_gated(root), root)
}

/// Like [`gated_disagreements`], but diffs `probes` (typically a [`Report`]'s
/// `.probes`, already computed) instead of re-running the gated probes.
pub fn gated_disagreements_in(probes: &[Probe], root: &Path) -> Vec<(String, String, String)> {
    let committed = std::fs::read_to_string(table_path(root)).unwrap_or_default();
    let mut out = Vec::new();
    for probe in probes.iter().filter(|p| p.gated) {
        let row_prefix = format!("| `{}` | yes | ", probe.name);
        let committed_reads = committed
            .lines()
            .find(|l| l.starts_with(&row_prefix))
            .and_then(|l| l.strip_prefix(&row_prefix))
            .and_then(|l| l.strip_suffix(" |"))
            .unwrap_or("<row missing>");
        if committed_reads != probe.reads {
            out.push((
                probe.name.to_string(),
                committed_reads.to_string(),
                probe.reads.clone(),
            ));
        }
    }
    out
}

/// The disagreements no pending file acknowledges — what actually fails the gate.
///
/// A gated probe may move in a PR in one of two ways: the PR regenerates
/// `design/greenfield-status.md` itself (the original mechanism, still valid for
/// a change that carries no increment), or the PR's `design/pending/<slug>.md`
/// names the probe in its `probes` key and leaves the table alone. The second
/// keeps the acknowledgement in review — a probe still cannot move silently —
/// without every probe-moving PR editing the same shared rows, which is what
/// made this table the repo's most-conflicted file. The stamp then rewrites the
/// gated rows on `main` in merge order ([`refresh_gated_rows`]).
pub fn unacknowledged(
    disagreements: Vec<(String, String, String)>,
    acknowledged: &BTreeSet<String>,
) -> Vec<(String, String, String)> {
    disagreements
        .into_iter()
        .filter(|(name, _, _)| !acknowledged.contains(name))
        .collect()
}

/// `committed` (the table's current text) with every gated row's reading
/// replaced by `probes`' live one; trend rows, prose and the rule-ledger pointer
/// are left exactly as committed, so the stamp never pays for the trend probes'
/// workspace-wide clippy pass. Errors naming any gated probe with no row to
/// rewrite — a table that has lost a row needs a full
/// `cargo xtask greenfield-status --apply`, not a silent append.
pub fn refresh_gated_rows(committed: &str, probes: &[Probe]) -> Result<String, String> {
    let mut out = String::with_capacity(committed.len());
    let mut seen = BTreeSet::new();
    for line in committed.split_inclusive('\n') {
        let replacement = probes.iter().filter(|p| p.gated).find_map(|p| {
            let prefix = format!("| `{}` | yes | ", p.name);
            line.starts_with(&prefix).then(|| {
                seen.insert(p.name);
                let eol = if line.ends_with('\n') { "\n" } else { "" };
                format!("{prefix}{} |{eol}", p.reads)
            })
        });
        out.push_str(replacement.as_deref().unwrap_or(line));
    }
    let missing: Vec<_> = probes
        .iter()
        .filter(|p| p.gated && !seen.contains(p.name))
        .map(|p| p.name)
        .collect();
    if missing.is_empty() {
        Ok(out)
    } else {
        Err(format!(
            "design/greenfield-status.md has no row for gated probe(s) {} — run \
             `cargo xtask greenfield-status --apply`",
            missing.join(", ")
        ))
    }
}

/// The stamp's refreshed table: [`refresh_gated_rows`], but only if every gated
/// probe whose live reading differs from `committed` is named by one of the
/// increments being stamped (`acknowledged` — the union of their `probes`).
///
/// Rewriting every gated row unconditionally would silently absorb a move that
/// reached `main` with no acknowledgement (a bypass push, say): after the stamp
/// the table would match by construction, and nothing downstream could notice.
/// Refusing instead keeps the invariant the gate enforces on PRs — a gated probe
/// never moves unacknowledged — true on `main` too. The stamp computes this
/// before applying, so a refusal leaves nothing written and the pending files
/// in place.
pub fn refresh_for_stamp(
    committed: &str,
    probes: &[Probe],
    acknowledged: &BTreeSet<String>,
) -> Result<String, String> {
    let unnamed: Vec<String> = probes
        .iter()
        .filter(|p| p.gated && !acknowledged.contains(p.name))
        .filter_map(|p| {
            let prefix = format!("| `{}` | yes | ", p.name);
            let committed_reads = committed
                .lines()
                .find(|l| l.starts_with(&prefix))?
                .strip_prefix(&prefix)?
                .strip_suffix(" |")?;
            (committed_reads != p.reads).then(|| {
                format!(
                    "  {}: committed {committed_reads:?}, live {:?}",
                    p.name, p.reads
                )
            })
        })
        .collect();
    if !unnamed.is_empty() {
        return Err(format!(
            "gated greenfield-status probe(s) moved without being named by any increment being \
             stamped — refusing to absorb them into the table. Acknowledge them in a pending \
             file or regenerate the table in a PR:\n{}",
            unnamed.join("\n")
        ));
    }
    refresh_gated_rows(committed, probes)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- probe acknowledgement + refresh ------------------------------------

    fn gated(name: &'static str, reads: &str) -> Probe {
        Probe {
            name,
            gated: true,
            reads: reads.to_string(),
        }
    }

    #[test]
    fn unacknowledged_drops_only_acknowledged_probes() {
        let d = vec![
            (
                "ts_writes".to_string(),
                "850".to_string(),
                "851".to_string(),
            ),
            ("ts_any".to_string(), "26".to_string(), "27".to_string()),
        ];
        let ack: BTreeSet<String> = ["ts_writes".to_string()].into();
        let left = unacknowledged(d, &ack);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].0, "ts_any");
    }

    #[test]
    fn refresh_rewrites_gated_rows_and_leaves_the_rest() {
        let committed = "# Greenfield status\n\n| Probe | Gated | Reads |\n|---|---|---|\n\
                         | `ts_writes` | yes | 850 |\n\
                         | `wildcard_arms` | no (trend) | 355 |\n\nfooter\n";
        let out = refresh_gated_rows(committed, &[gated("ts_writes", "851")]).unwrap();
        assert!(out.contains("| `ts_writes` | yes | 851 |\n"));
        assert!(out.contains("| `wildcard_arms` | no (trend) | 355 |\n"));
        assert!(out.ends_with("\nfooter\n"));
        assert_eq!(out.lines().count(), committed.lines().count());
    }

    const STAMP_TABLE: &str = "| Probe | Gated | Reads |\n|---|---|---|\n\
                               | `ts_writes` | yes | 850 |\n\
                               | `ts_any` | yes | 26 |\n";

    #[test]
    fn stamp_refresh_rewrites_named_moves() {
        let ack: BTreeSet<String> = ["ts_writes".to_string()].into();
        let out = refresh_for_stamp(
            STAMP_TABLE,
            &[gated("ts_writes", "851"), gated("ts_any", "26")],
            &ack,
        )
        .unwrap();
        assert!(out.contains("| `ts_writes` | yes | 851 |"));
        assert!(out.contains("| `ts_any` | yes | 26 |"));
    }

    #[test]
    fn stamp_refresh_refuses_an_unnamed_move() {
        let ack: BTreeSet<String> = ["ts_writes".to_string()].into();
        let err = refresh_for_stamp(
            STAMP_TABLE,
            &[gated("ts_writes", "851"), gated("ts_any", "27")],
            &ack,
        )
        .unwrap_err();
        assert!(err.contains("ts_any"), "{err}");
        assert!(!err.contains("ts_writes"), "{err}");
    }

    #[test]
    fn refresh_errors_on_a_missing_gated_row() {
        let err = refresh_gated_rows("| Probe | Gated | Reads |\n", &[gated("ts_any", "26")])
            .unwrap_err();
        assert!(err.contains("ts_any"));
    }

    // --- emit_diagnostics (#999 Decision A) ---------------------------------

    /// A standalone `"bynk.foo"` literal is found — the ordinary case.
    #[test]
    fn bynk_dotted_literals_finds_standalone_literal() {
        let src = r#"code("bynk.check.something", "a message")"#;
        assert_eq!(bynk_dotted_literals(src), vec!["bynk.check.something"]);
    }

    /// The bug this slice found in its own first draft: a longer message that merely
    /// *starts* with "bynk." must not be truncated into a fake code literal. Regression
    /// test for `bynk.map itself uses bynk.list, so list must be injected too: {paths:?}`
    /// (`bynk-emit/src/project.rs`), which an earlier, less careful version of this scan
    /// wrongly counted as the literal `"bynk.map"`.
    #[test]
    fn bynk_dotted_literals_ignores_prefix_of_a_longer_message() {
        let src = r#"assert!(cond, "bynk.map itself uses bynk.list, so list must be injected too: {paths:?}");"#;
        assert!(bynk_dotted_literals(src).is_empty());
    }

    /// Regression test for the other half of the same bug: a `\`-continued string
    /// literal (`"bynk.emit.unresolved_cross_context_signature: no signature for \`,
    /// continued on the next source line) is one string, not a diagnostic-code literal,
    /// even though its first segment matches the identifier charset — because the
    /// character after the run is `:`, never a closing quote, on either line.
    #[test]
    fn bynk_dotted_literals_ignores_a_line_continued_message() {
        let src =
            "\"bynk.emit.unresolved_cross_context_signature: no signature for \\\n     the rest\"";
        assert!(bynk_dotted_literals(src).is_empty());
    }

    /// The whole point of Decision A: cross-referencing the real registry, not a
    /// hand-maintained exclusion list, correctly separates a real diagnostic code from
    /// a commons/namespace path that merely looks like one.
    #[test]
    fn emit_diagnostics_cross_references_the_real_registry() {
        let registry: BTreeSet<&str> = bynk_syntax::diagnostics::REGISTRY
            .iter()
            .map(|d| d.code)
            .collect();
        // A code this registry is known to carry (bynk-syntax/src/diagnostics.rs).
        assert!(registry.contains("bynk.parse.expected_expression"));
        // A commons/namespace path, not a diagnostic code — #999's own verified survey.
        assert!(!registry.contains("bynk.locale"));
    }

    // --- ast_importers (#1176) ------------------------------------------------

    /// The exclusion is named, not prefixed: `project/tests_emit.rs` is the
    /// Q7-settled `Ir → String` half that keeps hand-writing TypeScript by calling
    /// straight into `emitter.rs`'s own body-rendering, and
    /// keeps reading a handler's declared param/return `TypeRef` with no `TyId`
    /// available at that call site — but `project.rs` (which also imports
    /// `bynk_syntax::ast`, via `EmitProjectCtx`) must stay counted, and so, per
    /// review of #1210, must `emitter.rs`/`emitter/lower.rs` themselves: both still
    /// hold live AST-*declaration* reads (`emitter.rs`'s `CommonsItem::Service`/
    /// `svc.protocol` walk, `emitter/lower.rs`'s `cap_op_param_names`) that are the
    /// still-open R6.13 defect this probe tracks, not the Q7 kind — excluding either
    /// file would hide that real work the same way a path-prefix rule would. A
    /// path-prefix rule (e.g. "only `emitter/**` counts") would have excluded
    /// `project.rs` right along with the legitimate ones, silently undercounting
    /// real work. (`ir.rs`/`ir/lower.rs`, the lowering pass's own former `Ast → Ir`
    /// exclusion, left this list at Arc D's P7.12 crate carve — they left
    /// `bynk-emit/src` entirely, not merely this list.)
    #[test]
    fn ast_importer_exclusion_is_named_not_prefixed() {
        assert!(is_named_ast_importer(Path::new("project/tests_emit.rs")));
        assert!(is_named_ast_importer(Path::new("emitter/serialisation.rs")));
        assert!(!is_named_ast_importer(Path::new("project.rs")));
        assert!(!is_named_ast_importer(Path::new("emitter.rs")));
        assert!(!is_named_ast_importer(Path::new("emitter/lower.rs")));
        assert!(!is_named_ast_importer(Path::new("emitter/workers.rs")));
        assert!(!is_named_ast_importer(Path::new("ir.rs")));
        assert!(!is_named_ast_importer(Path::new("ir/lower.rs")));
    }

    /// #1184 review: an `AST_IMPORTER_EXCEPTIONS` entry going stale (renamed or split,
    /// e.g. `ir/lower.rs` becoming `ir/lower/mod.rs`) must fail loud here, not surface
    /// as a silent `ast_importers` regression in `greenfield_status_table_is_current` —
    /// mirrors [`file_is_named_fs_floor`]'s own "fail loud, not quiet" discipline.
    #[test]
    fn ast_importer_exceptions_still_exist_and_still_import_the_ast() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("bynk-emit/src");
        for rel in AST_IMPORTER_EXCEPTIONS {
            let contents = std::fs::read_to_string(dir.join(rel)).unwrap_or_else(|e| {
                panic!("AST_IMPORTER_EXCEPTIONS entry {rel:?} does not exist: {e}")
            });
            assert!(
                contents.contains("bynk_syntax::ast"),
                "AST_IMPORTER_EXCEPTIONS entry {rel:?} no longer imports bynk_syntax::ast \
                 — it excludes nothing and should be removed"
            );
        }
    }

    /// #1184 review, extended by #1187's own closing scoping pass (and narrowed by
    /// review of #1210, which found `emitter.rs`/`emitter/lower.rs` still hold live,
    /// in-scope AST-declaration reads and must stay counted) and by P6.33's own
    /// re-settling (`emitter/serialisation.rs`, a phase boundary rather than a
    /// declaration-read exemption): exercises the real filter over the live tree, not
    /// just the pure predicate — the survivor set the PR's own named-vs-prefix
    /// argument depends on: the named exclusions drop out
    /// (`project/tests_emit.rs`'s Q7-settled `Ir → String` case and
    /// `emitter/serialisation.rs`'s phase-7 codec renderer), while `emitter.rs`/
    /// `emitter/lower.rs`/`emitter/workers.rs` do not. `ir.rs`/`ir/lower.rs` (the
    /// lowering pass's own former `Ast → Ir` pair, excluded here until Arc D's
    /// P7.12 crate carve) are asserted absent below for a different reason now:
    /// they left `bynk-emit/src` entirely, so `ast_importer_files` never walks
    /// them at all, named exclusion or not.
    ///
    /// P6.49 (phase 6's own §6b): `project.rs` and `project/diagnostics.rs`
    /// join the *excluded* side of this assertion — the opposite of what this test
    /// checked before. `project.rs` cleared without joining
    /// [`AST_IMPORTER_EXCEPTIONS`]: nine slices (P6.42–P6.49) either relocated its
    /// remaining declaration reads to the `bynk-check`/`bynk-project` crates that
    /// already own the data, or re-exported a type from a `bynk-check` module whose
    /// own public API was already parameterised by it (the P6.27 `ExprId` precedent,
    /// applied to `TypeDecl`/`FnDecl`/`Visibility`/`ActorDecl`) — real, verified
    /// movement, not a probe exemption. `project/diagnostics.rs` rides on it, per the
    /// same super-glob rule this file's own regression guard below pins.
    #[test]
    fn ast_importers_excludes_the_named_pairs_and_project_rs() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let dir = root.join("bynk-emit/src");
        let counted: BTreeSet<String> = ast_importer_files(&root)
            .into_iter()
            .map(|path| {
                path.strip_prefix(&dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert!(!counted.contains("ir.rs"), "moved to bynk-ir at P7.12");
        assert!(
            !counted.contains("ir/lower.rs"),
            "moved to bynk-lower at P7.12"
        );
        assert!(!counted.contains("project/tests_emit.rs"));
        assert!(!counted.contains("emitter/serialisation.rs"));
        assert!(!counted.contains("project.rs"));
        assert!(!counted.contains("project/diagnostics.rs"));
        assert!(counted.contains("emitter.rs"));
        assert!(counted.contains("emitter/lower.rs"));
        assert!(counted.contains("emitter/workers.rs"));
    }

    /// P6.26 review (#1259): a module-level `use super::*;` is a real inheritance
    /// channel (Rust's own privacy rule makes a parent's private `use` visible to
    /// descendants) — must be detected — but a `use super::*;` nested inside a
    /// `#[cfg(test)] mod tests { .. }` block glob-imports its own *immediately
    /// enclosing* module, not the grandparent file on disk, and must not
    /// false-positive.
    #[test]
    fn module_level_super_glob_detection_ignores_nested_test_mod() {
        assert!(has_module_level_super_glob(
            "use std::fmt;\nuse super::*;\n"
        ));
        assert!(!has_module_level_super_glob(
            "fn f() {}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n}\n"
        ));
    }

    /// P6.26 review (#1259): pins the real scenario the review found —
    /// `emitter/emit.rs` and `emitter/lower.rs` both carry a live, module-level
    /// `use super::*;` inheriting from `emitter.rs`, which itself still imports
    /// `bynk_syntax::ast` directly. Regression guard: if a future slice deletes
    /// either child's own explicit AST import while this inheritance channel and
    /// the parent's own AST dependency both remain, [`ast_importer_files`] must
    /// keep counting it rather than silently dropping the probe.
    #[test]
    fn super_glob_children_of_an_ast_importing_parent_are_detected() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let dir = root.join("bynk-emit/src");
        for rel in ["emitter/emit.rs", "emitter/lower.rs"] {
            let contents = std::fs::read_to_string(dir.join(rel))
                .unwrap_or_else(|e| panic!("{rel:?} does not exist: {e}"));
            assert!(
                has_module_level_super_glob(&contents),
                "{rel:?} no longer carries a module-level `use super::*;` — this \
                 regression guard (and the false-zero hazard it pins) no longer applies \
                 and may be deleted"
            );
            assert_eq!(
                super_glob_parent_imports_ast(&dir, Path::new(rel)),
                Some(true),
                "{rel:?}'s parent (`emitter.rs`) no longer imports bynk_syntax::ast — \
                 update this guard's expectation"
            );
        }
        // A file directly under `bynk-emit/src` (no directory component) has no
        // `use super::*;` parent to inherit from.
        assert_eq!(
            super_glob_parent_imports_ast(&dir, Path::new("emitter.rs")),
            None
        );
    }

    // --- fs_below_driver / test_density (trailing `#[cfg(test)] mod tests {}`) ---

    #[test]
    fn production_std_fs_usage_is_detected() {
        let src = "fn load(p: &Path) -> String {\n    std::fs::read_to_string(p).unwrap()\n}\n";
        assert!(has_production_std_fs(src));
    }

    #[test]
    fn std_fs_inside_a_trailing_test_mod_is_not_production() {
        let src = "fn load(p: &Path) -> String {\n    String::new()\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        std::fs::write(\"x\", \"y\").unwrap();\n    }\n}\n";
        assert!(!has_production_std_fs(src));
    }

    /// Regression test for the other real bug this slice found: `bynk-emit/src/lib.rs`
    /// has `#[cfg(test)] pub(crate) mod testkit;` — an external-file module
    /// *declaration* (ends in `;`), not an inline block. It must not be mistaken for a
    /// scope-opening `mod tests { ... }`, or the (genuinely production) code after it in
    /// the same file would be wrongly excluded.
    #[test]
    fn cfg_test_external_mod_declaration_does_not_open_a_test_region() {
        let src = "#[cfg(test)]\npub(crate) mod testkit;\n\nfn load(p: &Path) -> String {\n    std::fs::read_to_string(p).unwrap()\n}\n";
        assert!(has_production_std_fs(src));
    }

    // --- unconsumed_ir_items (Slice D3 of #1542) ----------------------------

    fn files(entries: &[(&str, &str, &str)]) -> Vec<(String, PathBuf, String)> {
        entries
            .iter()
            .map(|(k, p, c)| (k.to_string(), PathBuf::from(p), c.to_string()))
            .collect()
    }

    /// The ordinary case: a `pub` item read from another crate's production
    /// code is consumed; one read from nowhere is not.
    #[test]
    fn unconsumed_pub_items_reports_only_items_with_no_other_crate_reader() {
        let owners = files(&[(
            "bynk-ir",
            "lib.rs",
            "pub struct Used;\npub struct Unused;\npub fn helper() {}\n",
        )]);
        let consumers = files(&[
            (
                "bynk-ir",
                "lib.rs",
                "pub struct Used;\npub struct Unused;\npub fn helper() {}\n",
            ),
            (
                "bynk-emit",
                "a.rs",
                "fn f(x: bynk_ir::Used) { helper(); }\n",
            ),
        ]);
        assert_eq!(
            unconsumed_pub_items(&["bynk-ir"], &owners, &consumers),
            vec!["bynk-ir::Unused"]
        );
    }

    /// A mention in a comment, inside a `#[cfg(test)]` module, or in the
    /// owning crate itself is not a consumer.
    #[test]
    fn unconsumed_pub_items_ignores_comments_tests_and_the_owning_crate() {
        let owners = files(&[(
            "bynk-lower",
            "lib.rs",
            "pub fn lower_x() {}\npub fn lower_y() {}\npub fn lower_z() {}\n",
        )]);
        let consumers = files(&[
            (
                "bynk-lower",
                "lib.rs",
                "pub fn lower_x() {}\nfn own() { lower_x(); }\n",
            ),
            (
                "bynk-emit",
                "a.rs",
                "// lower_y() used to be called here\n/// and [`lower_y`] linked here\n",
            ),
            (
                "bynk-emit",
                "b.rs",
                "fn prod() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() { lower_z(); }\n}\n",
            ),
        ]);
        assert_eq!(
            unconsumed_pub_items(&["bynk-lower"], &owners, &consumers),
            vec![
                "bynk-lower::lower_x",
                "bynk-lower::lower_y",
                "bynk-lower::lower_z"
            ]
        );
    }

    /// Whole-word matching: `IrExpr` inside `IrExprKind` is not a read of
    /// `IrExpr`, and `pub(crate)`/indented items are not offered to other
    /// crates so are never counted either way.
    #[test]
    fn unconsumed_pub_items_matches_whole_words_and_skips_non_public_items() {
        let owners = files(&[(
            "bynk-ir",
            "lib.rs",
            "pub struct IrExpr;\npub enum IrExprKind {}\npub(crate) fn internal() {}\nimpl IrExpr {\n    pub fn method() {}\n}\n",
        )]);
        let consumers = files(&[("bynk-emit", "a.rs", "fn f(k: IrExprKind) {}\n")]);
        assert_eq!(
            unconsumed_pub_items(&["bynk-ir"], &owners, &consumers),
            vec!["bynk-ir::IrExpr"]
        );
    }

    /// The two IR crates do not vouch for each other (review of #1581): a
    /// `bynk-ir` type read only from `bynk-lower` is unconsumed, and so is a
    /// `bynk-lower` helper read only from `bynk-ir` — pre-D0 `main`'s exact
    /// shape, where `bynk-lower`'s own unconsumed constructors named every
    /// unconsumed `bynk-ir` type.
    #[test]
    fn unconsumed_pub_items_does_not_let_owner_crates_vouch_for_each_other() {
        let owners = files(&[
            (
                "bynk-ir",
                "lib.rs",
                "pub struct IrExpr;\npub struct Shape;\n",
            ),
            (
                "bynk-lower",
                "lib.rs",
                "pub fn lower_expr_ir() -> IrExpr { IrExpr }\npub fn shape() -> Shape { Shape }\n",
            ),
        ]);
        let consumers = files(&[
            (
                "bynk-ir",
                "lib.rs",
                "pub struct IrExpr;\npub struct Shape;\n",
            ),
            (
                "bynk-lower",
                "lib.rs",
                "pub fn lower_expr_ir() -> IrExpr { IrExpr }\npub fn shape() -> Shape { Shape }\n",
            ),
            (
                "bynk-emit",
                "a.rs",
                "fn f() -> Shape { bynk_lower::shape() }\n",
            ),
        ]);
        assert_eq!(
            unconsumed_pub_items(&["bynk-ir", "bynk-lower"], &owners, &consumers),
            vec!["bynk-ir::IrExpr", "bynk-lower::lower_expr_ir"]
        );
    }

    /// One item declared in two files of the same crate (a split module) is
    /// reported once, not twice.
    #[test]
    fn unconsumed_pub_items_deduplicates_a_name_declared_in_two_files() {
        let owners = files(&[
            ("bynk-lower", "a.rs", "pub fn twice() {}\n"),
            ("bynk-lower", "b.rs", "pub fn twice() {}\n"),
        ]);
        assert_eq!(
            unconsumed_pub_items(&["bynk-lower"], &owners, &[]),
            vec!["bynk-lower::twice"]
        );
    }

    #[test]
    fn column_zero_pub_item_name_accepts_every_item_kind_and_fn_qualifier() {
        assert_eq!(column_zero_pub_item_name("pub fn f(x: i32) {}"), Some("f"));
        assert_eq!(column_zero_pub_item_name("pub struct S<'a> {"), Some("S"));
        assert_eq!(column_zero_pub_item_name("pub enum E {"), Some("E"));
        assert_eq!(
            column_zero_pub_item_name("pub type T = (u8, u8);"),
            Some("T")
        );
        assert_eq!(
            column_zero_pub_item_name("pub const C: &[&str] = &[];"),
            Some("C")
        );
        assert_eq!(
            column_zero_pub_item_name("pub static S: &[&str] = &[];"),
            Some("S")
        );
        assert_eq!(column_zero_pub_item_name("pub trait Tr {}"), Some("Tr"));
        assert_eq!(column_zero_pub_item_name("pub union U {"), Some("U"));
        // `fn` qualifiers, alone and stacked (review of #1581).
        assert_eq!(column_zero_pub_item_name("pub async fn a() {}"), Some("a"));
        assert_eq!(column_zero_pub_item_name("pub unsafe fn u() {}"), Some("u"));
        assert_eq!(column_zero_pub_item_name("pub const fn c() {}"), Some("c"));
        assert_eq!(
            column_zero_pub_item_name("pub extern \"C\" fn x() {}"),
            Some("x")
        );
        assert_eq!(
            column_zero_pub_item_name("pub const unsafe fn cu() {}"),
            Some("cu")
        );
        assert_eq!(
            column_zero_pub_item_name("pub unsafe extern \"C\" fn ue() {}"),
            Some("ue")
        );
        assert_eq!(
            column_zero_pub_item_name("pub async unsafe fn au() {}"),
            Some("au")
        );
        // Not items offered to another crate.
        assert_eq!(column_zero_pub_item_name("pub(crate) fn g() {}"), None);
        assert_eq!(column_zero_pub_item_name("pub(super) struct P;"), None);
        assert_eq!(column_zero_pub_item_name("    pub fn method() {}"), None);
        assert_eq!(column_zero_pub_item_name("pub use foo::Bar;"), None);
        assert_eq!(column_zero_pub_item_name("pub mod m;"), None);
        assert_eq!(column_zero_pub_item_name("pub impl Foo {}"), None);
    }

    /// [`workspace_crate_src_files`] tags each file with its crate directory
    /// and only walks directories that are actually crates (a `Cargo.toml`
    /// *and* a `src/`), so a stray directory with one but not the other is
    /// neither an owner nor a consumer.
    #[test]
    fn workspace_crate_src_files_tags_files_by_crate_and_skips_non_crates() {
        let root = std::env::temp_dir().join(format!(
            "bynk-xtask-unconsumed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mk = |rel: &str, contents: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, contents).unwrap();
        };
        mk("alpha/Cargo.toml", "[package]\nname = \"alpha\"\n");
        mk("alpha/src/lib.rs", "pub fn a() {}\n");
        mk("alpha/src/inner/mod.rs", "pub fn b() {}\n");
        mk("beta/Cargo.toml", "[package]\nname = \"beta\"\n");
        mk("beta/src/lib.rs", "fn c() { alpha::a() }\n");
        mk("no-src/Cargo.toml", "[package]\nname = \"no-src\"\n");
        mk("no-manifest/src/lib.rs", "pub fn d() {}\n");
        let files = workspace_crate_src_files(&root);
        let _ = std::fs::remove_dir_all(&root);
        let mut tagged: Vec<(String, String)> = files
            .iter()
            .map(|(k, p, _)| (k.clone(), p.to_string_lossy().replace('\\', "/")))
            .collect();
        tagged.sort();
        assert_eq!(
            tagged,
            vec![
                ("alpha".to_string(), "inner/mod.rs".to_string()),
                ("alpha".to_string(), "lib.rs".to_string()),
                ("beta".to_string(), "lib.rs".to_string()),
            ]
        );
    }

    /// Regression test for the bug caught in review: a file with **two** scattered
    /// `#[cfg(test)] mod ... { ... }` blocks, with real production code between them —
    /// exactly `bynk-emit/src/emitter/lower.rs`'s shape (two test modules, 1031
    /// production lines apart). A single "everything from the first/last `#[cfg(test)]`
    /// onward" cutoff would misclassify `lower_lambda` here as test-scope; the fix must
    /// close each block at its own boundary and resume production scanning after it.
    #[test]
    fn production_code_between_two_scattered_test_mods_is_detected() {
        let src = "\
#[cfg(test)]
mod decode_map_key_tests {
    #[test]
    fn t() {
        assert_eq!(1, 1);
    }
}

fn lower_lambda(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

#[cfg(test)]
mod idempotency_scoping_tests {
    #[test]
    fn t2() {
        assert_eq!(2, 2);
    }
}
";
        assert!(has_production_std_fs(src));
    }

    /// The same fixture's `test_mod_ranges` shape, checked directly: two disjoint
    /// ranges, not one span from the first block to the last.
    #[test]
    fn test_mod_ranges_finds_each_block_separately() {
        let src = "\
#[cfg(test)]
mod a {
    fn x() {}
}

fn production() {}

#[cfg(test)]
mod b {
    fn y() {}
}
";
        let lines: Vec<&str> = src.lines().collect();
        let ranges = test_mod_ranges(&lines);
        assert_eq!(
            ranges.len(),
            2,
            "expected two disjoint test-mod ranges: {ranges:?}"
        );
        // Line 5 (0-indexed) is `fn production() {}`, between the two blocks.
        assert!(
            !in_test_range(5, &ranges),
            "production() must not read as test-scope"
        );
    }

    /// Regression test for the bug in the *fix* for the above: a column-0-`}`
    /// shortcut (tried and reverted during review) truncates a test module the moment
    /// its body embeds a multi-line fixture string containing a `}` flush against the
    /// left margin — exactly `bynk-ide/src/sequence.rs`'s shape, whose test mod embeds
    /// `.bynk` source fixtures. The real brace-depth scanner must see through the
    /// string and find the module's *actual* closing brace, hundreds of lines later.
    /// Uses a raw string for the outer fixture so the embedded `"..."` doesn't need
    /// escaping, and locates the real end by content rather than a hand-counted index
    /// — a hand-counted line number is exactly the kind of easy-to-miscount detail
    /// this codebase's own convention (verify, don't assume) warns against.
    #[test]
    fn test_mod_ranges_is_not_fooled_by_a_column_zero_brace_inside_a_string() {
        let src = r#"#[cfg(test)]
mod tests {
    const FIXTURE: &str = "
commons app.demo {
}
";

    fn real_end_of_module() {}
}
"#;
        let lines: Vec<&str> = src.lines().collect();
        let ranges = test_mod_ranges(&lines);
        assert_eq!(ranges.len(), 1, "expected exactly one range: {ranges:?}");
        let (_, end) = ranges[0];
        // `str::lines()` drops the trailing newline, so the module's real closing
        // brace — the fixture's last line — is at `lines.len() - 1`. The string's
        // embedded `}` (an earlier line) must not be mistaken for it.
        assert_eq!(
            end,
            lines.len() - 1,
            "closed too early — mistook the string's `}}` for the module's: {ranges:?}"
        );
    }

    // --- fs_below_driver: import resolution through `use super::*;` (#1013) ---

    /// Run [`production_std_fs_files`] over an in-memory crate layout and name the
    /// flagged files, so each case reads as "these files, and only these".
    fn flagged(files: &[(&str, &str)]) -> Vec<String> {
        let owned: Vec<(PathBuf, String)> = files
            .iter()
            .map(|(p, s)| (PathBuf::from(p), (*s).to_string()))
            .collect();
        production_std_fs_files(&owned)
            .into_iter()
            .map(|i| files[i].0.to_string())
            .collect()
    }

    /// The concrete #1013 instance, in miniature: `project.rs` has a module-level
    /// `use std::fs;` (ancestor-scoped, so visible to descendants), `discovery.rs`
    /// glob-imports it via `use super::*;` and calls bare `fs::read_to_string` —
    /// touching `std::fs` in production while never spelling it. The text scan alone
    /// reads only `project.rs`; the resolved probe must read both.
    #[test]
    fn bare_fs_reached_through_a_glob_imported_parent_is_flagged() {
        let files = [
            ("lib.rs", "mod project;\n"),
            ("project.rs", "use std::fs;\n\nmod discovery;\n"),
            (
                "project/discovery.rs",
                "use super::*;\n\nfn read_source(path: &std::path::Path) -> String {\n    fs::read_to_string(path).unwrap()\n}\n",
            ),
        ];
        assert!(
            !has_production_std_fs(files[2].1),
            "the text scan alone must miss it"
        );
        assert_eq!(flagged(&files), vec!["project.rs", "project/discovery.rs"]);
    }

    /// Without `use super::*;` there is no path from the bare `fs::` to the parent's
    /// binding — the probe must not guess one into existence.
    #[test]
    fn bare_fs_without_a_glob_super_import_is_not_flagged() {
        let files = [
            ("lib.rs", "mod project;\n"),
            ("project.rs", "use std::fs;\n\nmod discovery;\n"),
            (
                "project/discovery.rs",
                "fn read_source(path: &std::path::Path) -> String {\n    fs::read_to_string(path).unwrap()\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), vec!["project.rs"]);
    }

    /// Glob chains re-reach ancestors transitively — grandparent binds `fs`, both
    /// hops glob-import `super::*` — and the `mod.rs` layout maps to the same module
    /// tree as the `name.rs` one. The middle file sees `fs` but never uses it, so
    /// only the leaf joins the (text-flagged) root.
    #[test]
    fn glob_super_resolution_is_transitive_across_mod_rs_parents() {
        let files = [
            ("lib.rs", "use std::fs;\n\nmod a;\n"),
            ("a/mod.rs", "use super::*;\n\nmod b;\n"),
            (
                "a/b.rs",
                "use super::*;\n\nfn walk() {\n    let _ = fs::read_dir(\".\");\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), vec!["lib.rs", "a/b.rs"]);
    }

    /// A break anywhere in the chain stops resolution: the middle module does not
    /// glob-import `super::*`, so the leaf's `use super::*;` reaches a module with no
    /// `fs` binding to offer.
    #[test]
    fn a_break_in_the_glob_chain_stops_resolution() {
        let files = [
            ("lib.rs", "use std::fs;\n\nmod a;\n"),
            ("a/mod.rs", "mod b;\n"),
            (
                "a/b.rs",
                "use super::*;\n\nfn walk() {\n    let _ = fs::read_dir(\".\");\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), vec!["lib.rs"]);
    }

    /// Nearest binding wins, as in Rust: the child re-binds `fs` to something that is
    /// not `std::fs`, so its bare `fs::` calls are that something's, not std's.
    #[test]
    fn a_local_non_std_binding_shadows_the_ancestors_std_fs() {
        let files = [
            ("lib.rs", "mod project;\n"),
            ("project.rs", "use std::fs;\n\nmod overlay;\nmod d;\n"),
            ("project/overlay.rs", "pub fn read(_p: &str) {}\n"),
            (
                "project/d.rs",
                "use super::*;\nuse crate::project::overlay as fs;\n\nfn f() {\n    let _ = fs::read(\"x\");\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), vec!["project.rs"]);
    }

    /// An aliased module binding resolves under its alias — the call site never
    /// contains the substring `fs::` at all.
    #[test]
    fn an_aliased_std_fs_binding_resolves_through_the_glob() {
        let files = [
            ("lib.rs", "mod p;\n"),
            ("p.rs", "use std::fs as stdfs;\n\nmod c;\n"),
            (
                "p/c.rs",
                "use super::*;\n\nfn f() {\n    stdfs::write(\"a\", \"b\").unwrap();\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), vec!["p.rs", "p/c.rs"]);
    }

    /// `use std::{fs, io};` binds `fs` without ever containing the substring
    /// `std::fs` — the same blind spot as #1013's, one file deep. Resolution applies
    /// in the file's own scope, no glob import required.
    #[test]
    fn a_group_imported_fs_binding_is_resolved_in_its_own_file() {
        let src = "use std::{fs, io};\n\nfn f() -> io::Result<()> {\n    fs::metadata(\"x\").map(|_| ())\n}\n";
        assert!(
            !has_production_std_fs(src),
            "the text scan alone must miss it"
        );
        let files = [("thing.rs", src)];
        assert_eq!(flagged(&files), vec!["thing.rs"]);
    }

    /// The item-import shape #1013 scope-checked (zero current instances), at the
    /// granularity this probe can reach: an ancestor's `use std::fs::File;` used as a
    /// bare path root `File::open` in a glob-importing child resolves and flags. (A
    /// bare *call* of an imported fn — `read_to_string(p)`, no `::` — presents no
    /// path root and remains out of a text-level scanner's reach, per the doc.)
    #[test]
    fn an_item_import_under_std_fs_resolves_as_a_path_root() {
        let files = [
            ("lib.rs", "mod p;\n"),
            ("p.rs", "use std::fs::File;\n\nmod c;\n"),
            (
                "p/c.rs",
                "use super::*;\n\nfn f() {\n    let _ = File::open(\"x\");\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), vec!["p.rs", "p/c.rs"]);
    }

    /// A test module's `use super::*;` and tempdir `fs::` calls are test-scope — the
    /// `bynk-ide` files' shape (`architecture.rs`, `sequence.rs`), which must stay
    /// unflagged exactly as they were under the text-only scan.
    #[test]
    fn glob_and_bare_fs_inside_a_test_mod_stay_test_scope() {
        let files = [
            ("lib.rs", "use std::fs;\n\nmod w;\n"),
            (
                "w.rs",
                "fn production() {}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    use std::fs;\n\n    #[test]\n    fn t() {\n        let _ = fs::read_dir(\".\");\n    }\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), vec!["lib.rs"]);
    }

    /// The module-tree mapping behind the resolution, checked directly: `name.rs` and
    /// `mod.rs` layouts, a preferred `a.rs` over `a/mod.rs`, and rootless roots.
    #[test]
    fn module_parent_maps_both_file_layouts() {
        let files: Vec<(PathBuf, String)> = ["lib.rs", "a.rs", "a/b.rs", "c/mod.rs", "c/d.rs"]
            .iter()
            .map(|p| (PathBuf::from(p), String::new()))
            .collect();
        let idx = |name: &str| {
            files
                .iter()
                .position(|(p, _)| p == Path::new(name))
                .unwrap()
        };
        assert_eq!(module_parent(Path::new("lib.rs"), &files), None);
        assert_eq!(
            module_parent(Path::new("a.rs"), &files),
            Some(idx("lib.rs"))
        );
        assert_eq!(
            module_parent(Path::new("a/b.rs"), &files),
            Some(idx("a.rs"))
        );
        assert_eq!(
            module_parent(Path::new("c/mod.rs"), &files),
            Some(idx("lib.rs"))
        );
        assert_eq!(
            module_parent(Path::new("c/d.rs"), &files),
            Some(idx("c/mod.rs"))
        );
    }

    // --- fs_below_driver: #1016 review findings ------------------------------

    /// Finding 1: a `super::`-qualified path needs no glob import — module privacy is
    /// ancestor-scoped, so `super::fs` names the parent's private `use std::fs;` from
    /// any child. One disambiguating edit away from `discovery.rs:39`'s bare call,
    /// and it must not drop the file out of the count.
    #[test]
    fn a_super_qualified_path_resolves_without_a_glob_import() {
        let files = [
            ("lib.rs", "mod project;\n"),
            ("project.rs", "use std::fs;\n\nmod discovery;\n"),
            (
                "project/discovery.rs",
                "fn read_source(path: &std::path::Path) -> String {\n    super::fs::read_to_string(path).unwrap()\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), vec!["project.rs", "project/discovery.rs"]);
    }

    /// Finding 1, the `crate::`-rooted form: the walk descends the module tree from
    /// the crate root file by file, then resolves the leaf against that module's
    /// bindings — from anywhere in the crate, glob import or not.
    #[test]
    fn a_crate_qualified_path_resolves_through_the_module_tree() {
        let files = [
            ("lib.rs", "mod other;\nmod project;\n"),
            (
                "other.rs",
                "fn f() {\n    let _ = crate::project::fs::read_dir(\".\");\n}\n",
            ),
            ("project.rs", "use std::fs;\n"),
        ];
        assert_eq!(flagged(&files), vec!["other.rs", "project.rs"]);
    }

    /// Finding 1, stacked hops: `super::super::` climbs two parents (through a
    /// glob-free middle module — qualified paths don't need the glob chain).
    #[test]
    fn stacked_super_hops_climb_the_parent_chain() {
        let files = [
            ("lib.rs", "use std::fs;\n\nmod a;\n"),
            ("a/mod.rs", "mod b;\n"),
            (
                "a/b.rs",
                "fn f() {\n    let _ = super::super::fs::read_dir(\".\");\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), vec!["lib.rs", "a/b.rs"]);
    }

    /// Finding 1, `self::` composed with the glob chain: `self::fs` resolves in the
    /// file's own namespace, which includes what its `use super::*;` pulled in.
    #[test]
    fn a_self_qualified_path_resolves_through_the_files_own_glob_chain() {
        let files = [
            ("lib.rs", "mod p;\n"),
            ("p.rs", "use std::fs;\n\nmod c;\n"),
            (
                "p/c.rs",
                "use super::*;\n\nfn f() {\n    let _ = self::fs::read_dir(\".\");\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), vec!["p.rs", "p/c.rs"]);
    }

    /// Finding 1's negatives: a qualified path to a name the parent binds to
    /// something other than `std::fs` stops at that binding, and a path through a
    /// module that doesn't exist resolves nowhere.
    #[test]
    fn a_qualified_path_to_a_non_std_binding_or_missing_module_is_not_flagged() {
        let files = [
            ("lib.rs", "mod overlay;\nmod p;\n"),
            ("overlay.rs", "pub fn read_dir(_p: &str) {}\n"),
            ("p.rs", "use crate::overlay as fs;\n\nmod d;\n"),
            (
                "p/d.rs",
                "fn f() {\n    let _ = super::fs::read_dir(\".\");\n    let _ = crate::missing::fs::read_dir(\".\");\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), Vec::<String>::new());
    }

    /// Finding 2: a locally-declared type-namespace item beats a glob-imported name
    /// in real Rust — a child with its own `mod fs;` calling `fs::…` is calling its
    /// own submodule, not the ancestor's `std::fs`.
    #[test]
    fn a_locally_declared_module_shadows_the_ancestors_std_fs() {
        let files = [
            ("lib.rs", "mod p;\n"),
            ("p.rs", "use std::fs;\n\nmod c;\n"),
            (
                "p/c.rs",
                "use super::*;\n\nmod fs;\n\nfn f() {\n    let _ = fs::read_dir(\".\");\n}\n",
            ),
            ("p/c/fs.rs", "pub fn read_dir(_p: &str) {}\n"),
        ];
        assert_eq!(flagged(&files), vec!["p.rs"]);
    }

    /// Finding 3: a trailing `//` comment on a `use` line must not sever the edge —
    /// neither the glob (`use super::*; // …`) nor the binding (`use std::fs; // …`).
    #[test]
    fn a_trailing_comment_on_a_use_line_does_not_sever_resolution() {
        let files = [
            ("lib.rs", "mod p;\n"),
            (
                "p.rs",
                "use std::fs; // read_source's disk fallback\n\nmod c;\n",
            ),
            (
                "p/c.rs",
                "use super::*; // parent's fs, PathBuf\n\nfn f() {\n    let _ = fs::read_dir(\".\");\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), vec!["p.rs", "p/c.rs"]);
    }

    /// Finding 4: the nested-group + `::self` normalisation branches, pinned
    /// directly — `use std::{fs::{self, File}, io};` binds `fs` *and* `File` to
    /// `std::fs`, and `io` only to the shadow set. Getting `::self` wrong would
    /// silently under-count, which is exactly this probe's failure mode.
    #[test]
    fn a_nested_group_with_self_binds_the_module_and_its_items() {
        let facts = fs_import_facts("use std::{fs::{self, File}, io};\n");
        let bound: Vec<&str> = facts.std_fs_bindings.iter().map(String::as_str).collect();
        assert_eq!(bound, vec!["File", "fs"]);
        assert!(facts.use_bound_names.contains("io"));
        assert!(!facts.std_fs_bindings.contains("io"));
    }

    /// Finding 4, the children half of [`FsImportFacts`]' contract: a parent whose
    /// *only* `use std::fs;` lives in its `#[cfg(test)] mod` hands no binding to a
    /// glob-importing child — `bynk-ide/src/symbols.rs`' shape, latent until it
    /// grows a submodule.
    #[test]
    fn a_parents_test_mod_use_std_fs_does_not_reach_its_children() {
        let files = [
            ("lib.rs", "mod p;\n"),
            (
                "p.rs",
                "mod c;\n\nfn production() {}\n\n#[cfg(test)]\nmod tests {\n    use std::fs;\n\n    #[test]\n    fn t() {\n        let _ = fs::read_dir(\".\");\n    }\n}\n",
            ),
            (
                "p/c.rs",
                "use super::*;\n\nfn f() {\n    let _ = fs::read_dir(\".\");\n}\n",
            ),
        ];
        assert_eq!(flagged(&files), Vec::<String>::new());
    }

    // --- fs_below_driver: named-floor classification (#1104) -----------------

    #[test]
    fn fn_name_on_line_strips_modifiers() {
        assert_eq!(fn_name_on_line("fn foo() {"), Some("foo".to_string()));
        assert_eq!(
            fn_name_on_line("pub(crate) fn read_adapter_binding("),
            Some("read_adapter_binding".to_string())
        );
        assert_eq!(
            fn_name_on_line("pub async unsafe fn go() {"),
            Some("go".to_string())
        );
    }

    #[test]
    fn fn_name_on_line_ignores_non_fn_lines() {
        assert_eq!(fn_name_on_line("    let f = foo();"), None);
        assert_eq!(fn_name_on_line("/// calls fn bar somewhere"), None);
    }

    /// A signature whose `{` arrives lines after the `fn` line — `read_adapter_binding`'s
    /// own real shape — must still resolve to the correct body range: `started` can't
    /// flip true on the parameter list, which has no braces of its own.
    #[test]
    fn production_fn_ranges_handles_a_wrapped_signature() {
        let src = "pub(crate) fn read_adapter_binding(\n    path: &Path,\n) -> std::io::Result<String> {\n    fs::read_to_string(path)\n}\n";
        let lines: Vec<&str> = src.lines().collect();
        let ranges = production_fn_ranges(&lines, &[]);
        assert_eq!(ranges.len(), 1);
        let (name, start, end) = &ranges[0];
        assert_eq!(name, "read_adapter_binding");
        assert_eq!(*start, 0);
        assert_eq!(*end, lines.len() - 1);
        assert_eq!(
            enclosing_fn(3, &ranges),
            Some("read_adapter_binding".to_string())
        );
    }

    /// #1561: [`NAMED_FS_EXCEPTIONS`] itself is empty (P4.0 moved every file it used to
    /// name out of `bynk-emit` entirely — see the const's own doc comment), so the tests
    /// below that exercise a *match* use these synthetic tuples instead of real,
    /// currently-empty production data — same crate as the real #1104 shape, but a
    /// file/fn pair invented for this test and never a real production exception.
    const SYNTHETIC_EXCEPTIONS: &[(&str, &str, &str)] = &[
        (
            "bynk-emit",
            "project/synthetic_example.rs",
            "synthetic_named_fn_a",
        ),
        (
            "bynk-emit",
            "project/synthetic_example.rs",
            "synthetic_named_fn_b",
        ),
    ];

    /// Build the `facts`/`parents` vectors [`file_is_named_fs_floor`] now takes as
    /// caller-supplied arguments, the same way [`fs_below_driver`] does, so each test
    /// below reads as "classify this file" rather than repeating the setup.
    fn classify(
        krate: &str,
        files: &[(PathBuf, String)],
        i: usize,
        exceptions: &[(&str, &str, &str)],
    ) -> bool {
        let facts: Vec<FsImportFacts> = files.iter().map(|(_, s)| fs_import_facts(s)).collect();
        let parents: Vec<Option<usize>> =
            files.iter().map(|(p, _)| module_parent(p, files)).collect();
        file_is_named_fs_floor(krate, files, &facts, &parents, i, exceptions)
    }

    /// The concrete #1104 shape, in miniature (#1561: against [`SYNTHETIC_EXCEPTIONS`],
    /// `NAMED_FS_EXCEPTIONS` itself being empty): `project.rs`'s bare `use std::fs;` (no
    /// enclosing fn — never itself a violation) plus `synthetic_example.rs`'s two
    /// named-exception functions. The whole file must read as a named floor, not
    /// residual.
    #[test]
    fn file_is_named_fs_floor_true_for_a_file_of_only_named_exceptions() {
        let files = [
            (
                PathBuf::from("project.rs"),
                "use std::fs;\n\nmod synthetic_example;\n".to_string(),
            ),
            (
                PathBuf::from("project/synthetic_example.rs"),
                "use super::*;\n\npub(crate) fn synthetic_named_fn_a() {\n    let _ = fs::read_dir(\".\");\n}\n\npub(crate) fn synthetic_named_fn_b(path: &Path) -> std::io::Result<String> {\n    fs::read_to_string(path)\n}\n".to_string(),
            ),
        ];
        assert!(classify("bynk-emit", &files, 1, SYNTHETIC_EXCEPTIONS));
    }

    /// A new, unlisted fn touching `std::fs` in the *same file* as two named exceptions
    /// must flip the whole file to residual — no partial credit, since "named floor"
    /// must mean every touch is accounted for, not most of them.
    #[test]
    fn file_is_named_fs_floor_false_when_an_unnamed_fn_also_touches_fs() {
        let files = [
            (
                PathBuf::from("project.rs"),
                "use std::fs;\n\nmod synthetic_example;\n".to_string(),
            ),
            (
                PathBuf::from("project/synthetic_example.rs"),
                "use super::*;\n\npub(crate) fn synthetic_named_fn_a() {\n    let _ = fs::read_dir(\".\");\n}\n\nfn some_new_helper() {\n    let _ = fs::write(\"x\", \"y\");\n}\n".to_string(),
            ),
        ];
        assert!(!classify("bynk-emit", &files, 1, SYNTHETIC_EXCEPTIONS));
    }

    /// A file whose only production-scope touch is a bare `use std::fs;` import — no
    /// enclosing fn at all — is trivially a named floor: the import performs no I/O by
    /// itself, and the descendant it enables is checked (and named) separately. No
    /// named exceptions needed at all here — nothing to match against.
    #[test]
    fn file_is_named_fs_floor_true_for_an_import_only_file() {
        let files = [(
            PathBuf::from("project.rs"),
            "use std::fs;\n\nmod discovery;\n".to_string(),
        )];
        assert!(classify("bynk-emit", &files, 0, &[]));
    }

    /// The same synthetic-example shape under the wrong crate label must not read as a
    /// floor — [`NAMED_FS_EXCEPTIONS`] is keyed on `(crate, file, fn)`, not `(file, fn)`
    /// alone, so a same-named file/fn pair in a different crate isn't accidentally
    /// covered.
    #[test]
    fn file_is_named_fs_floor_false_under_the_wrong_crate() {
        let files = [
            (
                PathBuf::from("project.rs"),
                "use std::fs;\n\nmod synthetic_example;\n".to_string(),
            ),
            (
                PathBuf::from("project/synthetic_example.rs"),
                "use super::*;\n\npub(crate) fn synthetic_named_fn_a() {\n    let _ = fs::read_dir(\".\");\n}\n".to_string(),
            ),
        ];
        assert!(!classify("bynk-ide", &files, 1, SYNTHETIC_EXCEPTIONS));
    }

    /// Review finding (#1106): a module-scope `std::fs` touch that isn't an import
    /// declaration — a `static` initialiser doing real I/O — has no enclosing fn either,
    /// but is a genuine R2.3 violation and must not be waved through as a floor just
    /// because it sits outside every known fn range.
    #[test]
    fn file_is_named_fs_floor_false_for_a_module_scope_static_that_reads() {
        let files = [(
            PathBuf::from("project.rs"),
            "use std::fs;\n\nstatic ROOT: once_cell::sync::Lazy<String> = once_cell::sync::Lazy::new(|| fs::read_to_string(\"x\").unwrap());\n"
                .to_string(),
        )];
        assert!(!classify("bynk-emit", &files, 0, &[]));
    }

    /// Same review finding, the [`fn_name_on_line`] half: an `extern "C" fn` (a modifier
    /// combination the parser doesn't strip) produces no [`production_fn_ranges`] entry
    /// at all, so its whole body would fall into the "no enclosing fn" branch. It must
    /// still read as residual, not floor, once it touches `std::fs`.
    #[test]
    fn file_is_named_fs_floor_false_for_an_unparsed_extern_fn_body() {
        let files = [(
            PathBuf::from("project.rs"),
            "use std::fs;\n\nextern \"C\" fn callback() {\n    let _ = fs::read_dir(\".\");\n}\n"
                .to_string(),
        )];
        assert!(!classify("bynk-emit", &files, 0, &[]));
    }

    /// #1587: [`NAMED_FS_EXCEPTIONS`] is empty today (#1561 cleared the three entries
    /// that outlived their files' P4.0 move out of `bynk-emit` for weeks, unnoticed,
    /// while `fs_below_driver`'s `0 named floor` reading stayed vacuously "healthy"),
    /// so this loop is vacuous now — but guards whatever named exception is decided
    /// next the same way [`ts_writes_excluded_files_still_exist`] and
    /// [`ast_importer_exceptions_still_exist_and_still_import_the_ast`] guard their own
    /// lists: a future entry going stale must fail loud here, not silently surface as
    /// a falsely-healthy `fs_below_driver` count. The tuple's third field is checked
    /// too (review of #1591), not just discarded — [`file_is_named_fs_floor`] matches
    /// on the full `(crate, file, fn)` key, so a refactor that keeps the file but
    /// renames or deletes the named function would otherwise leave a silently dead
    /// entry, the same recurrence shape #1561's three entries took.
    #[test]
    fn named_fs_exceptions_still_exist() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        for (krate, rel, fn_name) in NAMED_FS_EXCEPTIONS {
            let path = root.join(krate).join("src").join(rel);
            let contents = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("NAMED_FS_EXCEPTIONS entry {krate:?}/{rel:?} does not exist: {e}")
            });
            assert!(
                contents.contains(fn_name),
                "NAMED_FS_EXCEPTIONS entry {krate:?}/{rel:?}/{fn_name:?} no longer names \
                 a function in that file — it excludes nothing and should be removed"
            );
        }
    }

    // --- emit_abi_shapes (#999 Decision E) ----------------------------------

    /// A binding's ordinary capability-interface imports, and the emit-ABI tag-layout
    /// names, must not be flagged — the exact failure mode Decision E rebuilt the probe
    /// to avoid (the original single-allowlist definition read 29-33 here, not 1).
    ///
    /// Exercises the real production allowlists via [`is_enumerated_emit_abi_or_capability_surface`]
    /// — not a local re-declaration. A test with its own copy of `EMIT_ABI` would still
    /// pass if the real one lost an entry (e.g. deleting `Uuid` from the production
    /// list), proving nothing about the probe it claims to cover.
    #[test]
    fn emit_abi_shapes_does_not_flag_capability_or_tag_layout_imports() {
        let src = "import type { Clock, Fetch, Locale } from \"./bynk.js\";\n\
                    import { FetchError, Uuid } from \"./bynk.js\";\n\
                    import { Err, None, Ok, Some, type Option, type Result } from \"./runtime.js\";\n";
        let imports = ts_named_imports_from_runtime_modules(src);
        let leaks: Vec<&String> = imports
            .iter()
            .filter(|i| !is_enumerated_emit_abi_or_capability_surface(i))
            .collect();
        assert!(leaks.is_empty(), "unexpected leaks: {leaks:?}");
    }

    /// The falsifier from #999 Decision E, checked directly: deleting an entry from the
    /// real production allowlist must be detectable by *some* test — this one flags
    /// `Uuid` as a leak the moment it's removed from [`EMIT_ABI`], which the test above
    /// (using the real const) would also start failing on.
    #[test]
    fn is_enumerated_checks_the_real_production_allowlist() {
        assert!(is_enumerated_emit_abi_or_capability_surface("Uuid"));
        assert!(is_enumerated_emit_abi_or_capability_surface("LocaleTag"));
        assert!(!is_enumerated_emit_abi_or_capability_surface(
            "negotiateLocale"
        ));
    }

    /// The real, current-tree finding this probe exists to surface: `negotiateLocale`,
    /// a plain value helper from `./runtime.js` alongside the tag-layout constructors,
    /// is neither an enumerated emit-ABI shape nor a capability-interface import.
    #[test]
    fn emit_abi_shapes_flags_a_non_enumerated_runtime_helper() {
        let src = "import { Err, None, Ok, Some, negotiateLocale, type Option, type Result } from \"./runtime.js\";\n";
        let imports = ts_named_imports_from_runtime_modules(src);
        assert!(imports.contains(&"negotiateLocale".to_string()));
    }

    /// `FetchError` is `import type` in one binding and a plain value import in
    /// another (`FetchError.Timeout`) — Decision E's rejected type-vs-value
    /// discriminator. Confirms the extractor treats both forms as the same identifier,
    /// so the allowlist check doesn't depend on which form a given file happens to use.
    #[test]
    fn ts_import_extraction_ignores_type_only_vs_value_distinction() {
        let type_only = "import type { FetchError } from \"./bynk.js\";\n";
        let value = "import { FetchError, Uuid } from \"./bynk.js\";\n";
        assert_eq!(
            ts_named_imports_from_runtime_modules(type_only),
            vec!["FetchError".to_string()]
        );
        assert!(ts_named_imports_from_runtime_modules(value).contains(&"FetchError".to_string()));
    }

    // --- options_sources -----------------------------------------------------

    #[test]
    fn struct_body_finds_a_field_by_name() {
        let src = "struct Foo {\n    pub sources: Option<HashMap<PathBuf, String>>,\n    pub other: bool,\n}\n";
        let body = struct_body(src, "Foo").expect("struct body found");
        assert!(body.contains("sources"));
    }

    #[test]
    fn struct_body_does_not_match_an_unrelated_struct() {
        let src =
            "struct Bar {\n    pub sources: bool,\n}\n\nstruct Foo {\n    pub other: bool,\n}\n";
        let body = struct_body(src, "Foo").expect("struct body found");
        assert!(!body.contains("sources"));
    }

    // --- render_table's "Rules closed" section (#1001) ------------------------

    fn empty_report() -> Report {
        Report { probes: Vec::new() }
    }

    /// The section is static text — no count, no existence check — precisely
    /// because nothing regenerates `design/greenfield-status.md` when `stamp`
    /// writes the ledger, so a computed count would silently go stale the
    /// moment the first `closes_rule` landed (the drift a first draft of this
    /// section introduced, caught in #1001's review). This test pins "static"
    /// as the actual behaviour, not just the intent in a comment.
    #[test]
    fn render_table_rules_closed_section_is_static_regardless_of_the_tree() {
        let out = render_table(&empty_report());
        assert!(out.contains("greenfield-status-rules.md"), "{out}");
        assert!(
            out.contains("may not exist yet"),
            "the wording must not claim to know whether the ledger exists: {out}"
        );
    }

    // --- ts_writes / ts_any (P7.0, #1296; testability + widening, review of #1297) --

    /// Run [`ts_writes_violations`] over an in-memory file list — mirrors
    /// [`flagged`]'s own role for `production_std_fs_files`.
    fn ts_writes_over(files: &[(&str, &str)]) -> usize {
        let owned: Vec<(PathBuf, String)> = files
            .iter()
            .map(|(p, s)| (PathBuf::from(p), (*s).to_string()))
            .collect();
        ts_writes_violations(&owned)
    }

    /// Run [`ts_any_violations`] over an in-memory file list.
    fn ts_any_over(files: &[(&str, &str)]) -> usize {
        let owned: Vec<(PathBuf, String)> = files
            .iter()
            .map(|(p, s)| (PathBuf::from(p), (*s).to_string()))
            .collect();
        ts_any_violations(&owned)
    }

    /// Run [`verbatim_origins_violations`] over an in-memory file list.
    fn verbatim_origins_over(files: &[(&str, &str)]) -> usize {
        let owned: Vec<(PathBuf, String)> = files
            .iter()
            .map(|(p, s)| (PathBuf::from(p), (*s).to_string()))
            .collect();
        verbatim_origins_violations(&owned)
    }

    /// Run [`verbatim_sites_violations`] over an in-memory file list.
    fn verbatim_sites_over(files: &[(&str, &str)]) -> usize {
        let owned: Vec<(PathBuf, String)> = files
            .iter()
            .map(|(p, s)| (PathBuf::from(p), (*s).to_string()))
            .collect();
        verbatim_sites_violations(&owned)
    }

    #[test]
    fn ts_writes_excluded_files_are_recognised() {
        assert!(is_ts_writes_excluded_file(Path::new("emitter/wrangler.rs")));
        assert!(is_ts_writes_excluded_file(Path::new("emitter/secrets.rs")));
        assert!(is_ts_writes_excluded_file(Path::new(
            "emitter/contracts.rs"
        )));
        assert!(is_ts_writes_excluded_file(Path::new("testkit.rs")));
        // Name proximity to a file that used to be excluded must not false-positive:
        // `emitter/lower.rs` (the emitter's own lowering pass) is genuinely
        // TS-producing and must stay counted — unlike `ir/lower.rs` (the checker→IR
        // pass), which isn't a name-proximity risk at all any more: it left
        // `bynk-emit/src` entirely at Arc D's P7.12 crate carve. `emitter/source_map.rs`
        // is the same shape (#1561): it left for `bynk-ts/src/source_map.rs` at P7.5
        // (#1308), so it's no longer excluded either — there's nothing left to exclude.
        assert!(!is_ts_writes_excluded_file(Path::new("emitter/lower.rs")));
        assert!(!is_ts_writes_excluded_file(Path::new("ir/lower.rs")));
        assert!(!is_ts_writes_excluded_file(Path::new(
            "emitter/source_map.rs"
        )));
        assert!(!is_ts_writes_excluded_file(Path::new("emitter.rs")));
        assert!(!is_ts_writes_excluded_file(Path::new("project.rs")));
        assert!(!is_ts_writes_excluded_file(Path::new(
            "project/tests_emit.rs"
        )));
    }

    /// #1587: the exact failure #1561 fixed for [`NAMED_FS_EXCEPTIONS`] (an entry
    /// outliving its file by weeks while the gated probe kept reading a vacuous,
    /// falsely-healthy number) applies just as well to [`TS_WRITES_EXCLUDED_FILES`] —
    /// nothing previously caught a stale entry here either. Must fail loud, not surface
    /// as a silent `ts_writes`/`ts_any` regression in `greenfield_status_table_is_current`
    /// — mirrors [`ast_importer_exceptions_still_exist_and_still_import_the_ast`]'s own
    /// discipline for the sibling list: existence alone isn't enough (review of #1591)
    /// — a file that survives but stops containing any `write!`/`writeln!`/`format!`
    /// site would excludes nothing, and removing it would change no probe count, so
    /// [`ts_writes_violations`] must actually see a nonzero count over each entry's real
    /// content. Scored under a dummy, non-excluded path so the exclusion filter itself
    /// doesn't short-circuit the check.
    #[test]
    fn ts_writes_excluded_files_still_exist() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("bynk-emit/src");
        for rel in TS_WRITES_EXCLUDED_FILES {
            let contents = std::fs::read_to_string(dir.join(rel)).unwrap_or_else(|e| {
                panic!("TS_WRITES_EXCLUDED_FILES entry {rel:?} does not exist: {e}")
            });
            let dummy = [(PathBuf::from("__not_excluded__.rs"), contents)];
            assert!(
                ts_writes_violations(&dummy) > 0,
                "TS_WRITES_EXCLUDED_FILES entry {rel:?} no longer contains a \
                 write!/writeln!/format! call outside a test module — it excludes \
                 nothing and should be removed"
            );
        }
    }

    /// Regression for a real mistake this slice's own grounding found: an earlier
    /// survey during phase 7's own track-opening research treated
    /// `project/tests_emit.rs` as excludable "test-assertion" noise. It is real
    /// production code (`process_tests`/`process_integration_tests`) per
    /// `semantics-in-the-checker.md`'s own settling finding for a different probe on
    /// the same file. Exercises the real probe, not just the predicate (review of
    /// #1297 — a first cut of this test called `is_ts_writes_excluded_file` directly,
    /// which can't catch a bug in [`ts_writes_violations`]'s own use of it).
    #[test]
    fn ts_writes_does_not_exclude_tests_emit_rs_wholesale() {
        let count = ts_writes_over(&[(
            "project/tests_emit.rs",
            "fn process_tests() {\n    let _ = format!(\"const x = 1;\");\n    let _ = writeln!(out, \"const y = 2;\");\n}\n",
        )]);
        assert_eq!(
            count, 2,
            "tests_emit.rs's own production emission code must be counted, not excluded wholesale"
        );
    }

    #[test]
    fn is_path_construction_line_catches_the_idiom_not_ordinary_format_calls() {
        assert!(is_path_construction_line(
            "let p = PathBuf::from(format!(\"workers/{dashes}/index.ts\"));"
        ));
        assert!(is_path_construction_line(
            "root.join(format!(\"tests/integration_{sanitized}.test.ts\"))"
        ));
        assert!(is_path_construction_line(
            "output_path.with_file_name(format!(\"{name}.{suffix}\"))"
        ));
        // An ordinary TS-producing `format!` call, no path construction, must not be
        // excluded by this idiom.
        assert!(!is_path_construction_line(
            "writeln!(out, \"{}\", format!(\"const {name} = 1;\"))"
        ));
    }

    #[test]
    fn line_violates_ts_any_catches_the_cast_and_the_bare_annotation() {
        assert!(line_violates_ts_any("let x = (value as any).field;"));
        assert!(line_violates_ts_any("format!(\"{}: any\", name)"));
        assert!(line_violates_ts_any(
            "\"(seq: any[]) => ({ns} as any).drive(seq)\""
        ));
        assert!(!line_violates_ts_any("let x: unknown = value;"));
    }

    /// Regression for review of #1297, finding 1: `any` in generic type-argument
    /// position (`Record<string, any[]>`, the live `emitter/lower.rs`
    /// `joinOn`/`leftJoin`/`groupBy` shape) contains neither `as any` nor `: any` and
    /// was silently uncounted by the round-one predicate.
    #[test]
    fn line_violates_ts_any_catches_generic_position_any() {
        assert!(line_violates_ts_any(
            "\"{{ const __h: Record<string, any[]> = {{}}; ...}}\""
        ));
        assert!(line_violates_ts_any("\"Array<any>\""));
        assert!(line_violates_ts_any("\"Promise<any>\""));
        // Must not regress the round-one patterns while widening.
        assert!(line_violates_ts_any("(value as any).field"));
        assert!(line_violates_ts_any("(e: any) => {}"));
    }

    /// Regression for review of #1322, finding 2: once a site builds a real
    /// `bynk_ts::TsType` node instead of writing TypeScript text directly, the
    /// emitted `any` no longer appears as Rust-source `as any`/`: any` — the round-
    /// one/round-two patterns above all match *emitted-text* spellings, none of
    /// which appear in `TsType::named("any")`. `workers.rs`'s own three real sites
    /// (#1321) were silently uncounted until this pattern was added.
    #[test]
    fn line_violates_ts_any_catches_the_named_any_construction_spelling() {
        assert!(line_violates_ts_any(
            "    let mut args = vec![as_expr(ident(\"payload\"), TsType::named(\"any\"))];"
        ));
        assert!(line_violates_ts_any(
            "        Some(TsType::named(\"any\")),"
        ));
        // Must not regress the round-one/round-two patterns while widening.
        assert!(line_violates_ts_any("(value as any).field"));
        assert!(line_violates_ts_any("\"Array<any>\""));
    }

    /// A comment mentioning either pattern in prose — the same self-reference-shaped
    /// hazard [`bynk_dotted_literals`]'s own regression tests guard against for a
    /// different probe — must not count.
    #[test]
    fn line_violates_ts_any_ignores_comments() {
        assert!(!line_violates_ts_any(
            "/// lowering machinery, same as any other subexpression."
        ));
        assert!(!line_violates_ts_any(
            "// TODO: stop emitting `: any` here once bynk-ts exists"
        ));
    }

    /// `#[cfg(test)]`-gated write!-family calls (a file's own unit tests constructing a
    /// fixture string) must not count toward either probe — mirrors
    /// [`has_production_std_fs`]'s own test-range exclusion for a different probe.
    /// Exercises the real probes end to end, not a re-implementation of their loop
    /// (review of #1297, finding 2): deleting either probe's `in_test_range` guard, its
    /// `is_ts_writes_excluded_file` `continue`, or (for `ts_writes`) its
    /// `is_path_construction_line` `continue` now fails one of these tests.
    #[test]
    fn ts_writes_and_ts_any_exclude_cfg_test_ranges() {
        let src = "fn production() {\n    let _ = format!(\"const x = 1;\");\n}\n\n\
                    #[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        \
                    let _ = format!(\"(v as any)\");\n    }\n}\n";
        assert_eq!(
            ts_writes_over(&[("emitter.rs", src)]),
            1,
            "only the production format! call counts"
        );
        assert_eq!(
            ts_any_over(&[("emitter.rs", src)]),
            0,
            "the test-only `as any` site must be excluded"
        );
    }

    /// Exercises the real probes' file-exclusion `continue`, not just the predicate:
    /// a whole file on [`TS_WRITES_EXCLUDED_FILES`] must contribute 0 to either count
    /// even when its content would otherwise match both.
    #[test]
    fn ts_writes_and_ts_any_exclude_named_non_ts_files_end_to_end() {
        let files = [(
            "emitter/wrangler.rs",
            "fn write_toml(out: &mut String) {\n    let _ = writeln!(out, \"name = {v}\");\n    let __x: any = 1;\n}\n",
        )];
        assert_eq!(ts_writes_over(&files), 0);
        assert_eq!(ts_any_over(&files), 0);
    }

    /// Exercises the real probes' [`is_path_construction_line`] `continue` end to end,
    /// not just the predicate in isolation.
    #[test]
    fn ts_writes_excludes_path_construction_end_to_end() {
        let files = [(
            "project.rs",
            "fn out_path(dashes: &str) -> PathBuf {\n    PathBuf::from(format!(\"workers/{dashes}/index.ts\"))\n}\n\nfn emit(out: &mut String) {\n    let _ = writeln!(out, \"export const x = 1;\");\n}\n",
        )];
        assert_eq!(
            ts_writes_over(&files),
            1,
            "the path-construction line must not count; the genuine emission line must"
        );
    }

    #[test]
    fn verbatim_origins_counts_distinct_variants_not_construction_sites() {
        let files = [(
            "emitter/contracts.rs",
            "fn a() { TsStmt::verbatim(VerbatimOrigin::Contracts, \"x\", None) }\nfn b() { TsStmt::verbatim(VerbatimOrigin::Contracts, \"y\", None) }\nfn c() { TsStmt::verbatim(VerbatimOrigin::Secrets, \"z\", None) }\n",
        )];
        // Three construction sites, but only two distinct origins.
        assert_eq!(verbatim_origins_over(&files), 2);
        assert_eq!(verbatim_sites_over(&files), 3);
    }

    #[test]
    fn verbatim_origins_and_sites_ignore_comments() {
        let files = [(
            "emitter/contracts.rs",
            "// TsStmt::verbatim(VerbatimOrigin::Contracts, \"x\", None)\n/// Mentions VerbatimOrigin::Secrets in prose.\n",
        )];
        assert_eq!(verbatim_origins_over(&files), 0);
        assert_eq!(verbatim_sites_over(&files), 0);
    }

    #[test]
    fn verbatim_origins_and_sites_read_zero_over_an_empty_tree() {
        let files: [(&str, &str); 0] = [];
        assert_eq!(verbatim_origins_over(&files), 0);
        assert_eq!(verbatim_sites_over(&files), 0);
    }

    /// Review of #1308, finding 6: without stripping `#[cfg(test)]` ranges,
    /// a single `bynk-emit` unit test fixture constructing a `TsStmt::
    /// verbatim(...)` for its own coverage would pin `verbatim_sites` above
    /// its documented 0 floor permanently, for a reason unrelated to
    /// residual production emission.
    #[test]
    fn verbatim_origins_and_sites_exclude_cfg_test_ranges() {
        let src = "fn production() {\n    TsStmt::verbatim(VerbatimOrigin::Contracts, \"x\", None);\n}\n\n\
                    #[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        \
                    TsStmt::verbatim(VerbatimOrigin::Secrets, \"y\", None);\n    }\n}\n";
        assert_eq!(
            verbatim_origins_over(&[("emitter/contracts.rs", src)]),
            1,
            "only the production-code origin counts"
        );
        assert_eq!(
            verbatim_sites_over(&[("emitter/contracts.rs", src)]),
            1,
            "the test-only construction site must be excluded"
        );
    }

    /// #1539: `verbatim_sites` widened from a `TsStmt::verbatim(`-only scan
    /// to also count `TsExpr::VerbatimExpr(` construction sites — the same
    /// escape hatch, now closed at the expression level too. Pins that both
    /// needles are counted (and both still respect the comment/`#[cfg(test)]`
    /// exclusions the sibling tests above already establish for the
    /// `TsStmt` half).
    #[test]
    fn verbatim_sites_counts_the_expr_level_needle_too() {
        let src = "fn f() {\n    let x = TsExpr::VerbatimExpr(\"a\".to_string(), VerbatimOrigin::Emit);\n    let y = TsStmt::verbatim(VerbatimOrigin::Emit, \"b\", None);\n}\n";
        assert_eq!(
            verbatim_sites_over(&[("emitter/emit.rs", src)]),
            2,
            "one TsExpr::VerbatimExpr( site plus one TsStmt::verbatim( site"
        );
        assert_eq!(
            verbatim_origins_over(&[("emitter/emit.rs", src)]),
            1,
            "both reference the same VerbatimOrigin::Emit variant"
        );
    }

    // --- incremental_query_types (P8.0, #1510; re-settled by #1537) ----------

    fn owned(files: &[(&str, &str)]) -> Vec<(PathBuf, String)> {
        files
            .iter()
            .map(|(p, s)| (PathBuf::from(p), (*s).to_string()))
            .collect()
    }

    #[test]
    fn unit_signature_present_recognises_the_real_struct_and_ignores_a_comment() {
        assert!(unit_signature_present(&owned(&[(
            "unit_signature.rs",
            "pub struct UnitSignature {\n    types: HashMap<String, Arc<TypeDecl>>,\n}\n",
        )])));
        assert!(!unit_signature_present(&owned(&[(
            "lib.rs",
            "// TODO: build a struct UnitSignature here eventually\n",
        )])));
        assert!(!unit_signature_present(&[]));
    }

    /// The committed reading after #1537: neither deleted level is back.
    #[test]
    fn deleted_levels_present_is_empty_when_neither_level_exists() {
        assert!(deleted_levels_present(&[]).is_empty());
    }

    /// Re-adding `ProjectGraph` anywhere flips the reading — the whole workspace is
    /// scanned (review of #1582) because P8.3 (ADR 0415) already landed it in a
    /// different crate from the one first assumed, and R3.13's table names a third
    /// (`bynk-resolve`) that does not exist yet.
    #[test]
    fn deleted_levels_present_sees_project_graph_wherever_it_lands() {
        let graph = "pub struct ProjectGraph {\n    units: HashMap<UnitId, Unit>,\n}\n";
        assert_eq!(
            deleted_levels_present(&owned(&[("project_graph.rs", graph)])),
            vec!["ProjectGraph"]
        );
        assert_eq!(
            deleted_levels_present(&owned(&[("resolve/graph.rs", graph)])),
            vec!["ProjectGraph"]
        );
        assert!(
            deleted_levels_present(&owned(&[(
                "lib.rs",
                "// a struct ProjectGraph used to live here\n"
            )]))
            .is_empty()
        );
    }

    /// **The empirically-confirmed false positive #1510's own first run caught** (see
    /// [`defid_query_fn_present`]): `bynk-check/src/checker.rs` has an ordinary,
    /// pre-existing `fn type_of(expr: &Expr, ..)` with no `DefId` anywhere in its
    /// signature. After #1537 the direction of the mistake reverses — it would now
    /// read as the deleted level having been *re-added* — so it is pinned against the
    /// real function's own signature text here too.
    #[test]
    fn deleted_levels_present_does_not_count_checkers_pre_existing_type_of() {
        let found = deleted_levels_present(&owned(&[(
            "checker.rs",
            "pub(crate) fn type_of(expr: &Expr, expected: Option<TyId>, ctx: &mut Ctx) -> Option<TyId> {\n",
        )]));
        assert!(
            found.is_empty(),
            "checker.rs's own type_of has no DefId parameter and must not count: {found:?}"
        );
    }

    #[test]
    fn deleted_levels_present_sees_a_real_defid_keyed_body_and_type_of() {
        let queries = "pub fn body(id: DefId) -> Body {\n    todo!()\n}\n\npub fn type_of(id: DefId) -> TypeOf {\n    todo!()\n}\n";
        assert_eq!(
            deleted_levels_present(&owned(&[("queries.rs", queries)])),
            vec!["Body", "TypeOf"]
        );
    }

    #[test]
    fn shared_cache_migrated_is_false_while_project_unit_cache_still_exists() {
        let ide_src: Vec<(PathBuf, String)> = vec![(
            PathBuf::from("completion.rs"),
            "static PROJECT_UNIT_CACHE: LazyLock<Mutex<HashMap<PathBuf, CachedUnit>>> = ..;"
                .to_string(),
        )];
        let project_src: Vec<(PathBuf, String)> = vec![(
            PathBuf::from("cache.rs"),
            "pub struct SharedUnitCache { .. }".to_string(),
        )];
        assert!(!shared_cache_migrated(&ide_src, &project_src));
    }

    #[test]
    fn shared_cache_migrated_is_true_once_project_unit_cache_is_gone_and_a_shared_cache_lands() {
        let ide_src: Vec<(PathBuf, String)> = vec![(
            PathBuf::from("completion.rs"),
            "fn cached_project_unit(path: &Path, content: &str) -> Option<Arc<SourceUnit>> { .. }"
                .to_string(),
        )];
        let project_src: Vec<(PathBuf, String)> = vec![(
            PathBuf::from("cache.rs"),
            "pub struct SharedUnitCache { units: HashMap<FileId, Arc<SourceUnit>> }".to_string(),
        )];
        assert!(shared_cache_migrated(&ide_src, &project_src));
    }

    /// **The real hole finding 1 caught**: absence of `PROJECT_UNIT_CACHE` from
    /// `bynk-ide` alone used to read "migrated" even when nothing shared replaced it —
    /// a rename or deletion with no cache anywhere in `bynk-project` satisfied the old
    /// clause. Now requires a cache-shaped item to actually land in
    /// `bynk-project/src` too.
    #[test]
    fn shared_cache_migrated_is_false_when_project_unit_cache_is_gone_but_nothing_shared_replaces_it()
     {
        let ide_src: Vec<(PathBuf, String)> = vec![(
            PathBuf::from("completion.rs"),
            "fn cached_project_unit(path: &Path, content: &str) -> Option<Arc<SourceUnit>> { .. }"
                .to_string(),
        )];
        assert!(!shared_cache_migrated(&ide_src, &[]));
    }

    /// The needle is anchored on `static PROJECT_UNIT_CACHE`, not a bare substring —
    /// `PROJECT_UNIT_CACHE_CAP` (a real, unrelated `const` in
    /// `bynk-ide/src/completion.rs`) must not hold this false on its own.
    #[test]
    fn shared_cache_migrated_is_not_confused_by_project_unit_cache_cap() {
        let ide_src: Vec<(PathBuf, String)> = vec![(
            PathBuf::from("completion.rs"),
            "const PROJECT_UNIT_CACHE_CAP: usize = 4096;".to_string(),
        )];
        let project_src: Vec<(PathBuf, String)> = vec![(
            PathBuf::from("cache.rs"),
            "pub struct SharedUnitCache { .. }".to_string(),
        )];
        assert!(shared_cache_migrated(&ide_src, &project_src));
    }

    #[test]
    fn stability_test_present_recognises_a_matching_test_name() {
        let check_tests: Vec<(PathBuf, String)> = vec![(
            PathBuf::from("unit_signature.rs"),
            "#[test]\nfn unit_signature_is_stable_under_a_body_edit() { .. }\n".to_string(),
        )];
        assert!(stability_test_present(&check_tests));
    }

    #[test]
    fn stability_test_present_recognises_test_attribute_separated_by_other_attributes() {
        let check_tests: Vec<(PathBuf, String)> = vec![(
            PathBuf::from("unit_signature.rs"),
            "#[test]\n#[should_panic]\nfn unit_signature_panics_when_stability_is_violated() { .. }\n"
                .to_string(),
        )];
        assert!(stability_test_present(&check_tests));
    }

    #[test]
    fn stability_test_present_is_false_for_an_unrelated_test() {
        let check_tests: Vec<(PathBuf, String)> = vec![(
            PathBuf::from("differential_analysis.rs"),
            "#[test]\nfn new_entry_point_matches_analyse_project_with() { .. }\n".to_string(),
        )];
        assert!(!stability_test_present(&check_tests));
    }

    #[test]
    fn stability_test_present_ignores_a_comment_mentioning_it() {
        let check_tests: Vec<(PathBuf, String)> = vec![(
            PathBuf::from("lib.rs"),
            "// TODO: add a unit_signature stability test (P8.2)\n".to_string(),
        )];
        assert!(!stability_test_present(&check_tests));
    }

    /// **The real hole finding 3 caught**: the old match required only
    /// `fn `+`unit_signature`+`stab` on one line, with no check for an actual
    /// `#[test]` attribute — a plain, non-test helper used to satisfy the clause with
    /// no passing test in existence.
    #[test]
    fn stability_test_present_is_false_for_a_non_test_helper_with_a_matching_name() {
        let check_tests: Vec<(PathBuf, String)> = vec![(
            PathBuf::from("unit_signature.rs"),
            "fn unit_signature_stability_fixture(edit: &Edit) -> UnitSignature { .. }\n"
                .to_string(),
        )];
        assert!(!stability_test_present(&check_tests));
    }
}
