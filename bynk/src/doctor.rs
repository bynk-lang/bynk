//! `bynk doctor` — the capability model, the checks, and the exit-code
//! contract.
//!
//! Probes are **grouped by the capability they unlock**, not listed flat, so a
//! compile-only user is never told they are "unhealthy" for lacking `wrangler`.
//! The exit-code contract turns on *what an invocation asks about* (ADR: the
//! doctor output / exit-code contract):
//!
//! - **Bare `bynk doctor`** is informational. It surveys everything but treats
//!   only the *compile floor* (`bynkc` resolvable and not majorly skewed) as
//!   required, so it exits `0` even with `test`/`dev` unavailable.
//! - **`--only <capability>`** promotes that capability's tools to required.
//! - **`--strict`** promotes *all* warnings (optional gaps, `npx`
//!   provisionability, minor skew) to failures, for an all-green CI gate.

use std::path::PathBuf;

use crate::compiler::{Compiler, Origin, Skew};
use crate::probe::{self, DetectOpts, Probe, Provenance, Toolbox};

/// A unit of work a user might want to do, and the tools it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// `bynkc` compile / check / fmt. Always satisfiable if `bynkc` resolved;
    /// also the home of the driver↔compiler skew check.
    Compile,
    /// `bynk test` — Node and one of `tsc`/`tsx` (the runner ladder).
    Test,
    /// `dev` / deploy to Cloudflare — Node and `wrangler`.
    Deploy,
    /// Editor support — `bynkc-lsp`. Optional; never a failure (except strict).
    Editor,
    /// Build Bynk from source — a Rust toolchain. Contributor-only; reported
    /// only inside the Bynk repo.
    BuildFromSource,
}

impl Capability {
    pub fn token(self) -> &'static str {
        match self {
            Capability::Compile => "compile",
            Capability::Test => "test",
            Capability::Deploy => "deploy",
            Capability::Editor => "editor",
            Capability::BuildFromSource => "build",
        }
    }

    /// Optional capabilities never fail a run on their own — they note, and
    /// `--strict` escalates.
    pub fn is_optional(self) -> bool {
        matches!(self, Capability::Editor | Capability::BuildFromSource)
    }
}

/// Health of a single row or a whole capability. Ordered: `Ok < Warn < Fail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

/// One rendered line under a capability: a tool (or an any-of group like
/// `tsc | tsx`), its health, a human detail, and a remedy when it is not `Ok`.
#[derive(Debug, Clone)]
pub struct Row {
    pub label: String,
    pub level: Level,
    pub detail: String,
    pub remedy: Option<String>,
}

/// A capability and its rows, with the aggregated health.
#[derive(Debug, Clone)]
pub struct CapabilityReport {
    pub capability: Capability,
    pub optional: bool,
    pub rows: Vec<Row>,
    pub level: Level,
}

/// The whole `doctor` result.
#[derive(Debug, Clone)]
pub struct Report {
    pub driver_version: String,
    pub compiler: Compiler,
    pub capabilities: Vec<CapabilityReport>,
}

/// User-facing knobs.
#[derive(Debug, Clone, Default)]
pub struct DoctorOptions {
    /// Scope the gate to one capability (promotes its tools to required).
    pub only: Option<Capability>,
    /// Escalate every warning to a failure.
    pub strict: bool,
}

/// Environment facts the caller supplies (real values in `main`, fixed values
/// in tests).
#[derive(Debug, Clone)]
pub struct Context {
    /// Discovered project root (`bynk.toml`), for project-local resolution.
    pub project_root: Option<PathBuf>,
    /// Whether to include the contributor `build` capability.
    pub in_repo: bool,
    /// Minimum supported Node major (single-sourced from `bynkc`).
    pub node_floor: u32,
}

impl Report {
    /// Should the process exit non-zero, given the options?
    ///
    /// Non-zero iff a *required* capability has a hard failure, or — under
    /// `--strict` — any capability is less than `Ok`. The compile floor is
    /// always required; `--only <cap>` adds that capability.
    pub fn exit_nonzero(&self, opts: &DoctorOptions) -> bool {
        for cap in &self.capabilities {
            let required =
                cap.capability == Capability::Compile || opts.only == Some(cap.capability);
            if required && cap.level == Level::Fail {
                return true;
            }
        }
        if opts.strict && self.capabilities.iter().any(|c| c.level != Level::Ok) {
            return true;
        }
        false
    }

    /// One-word overall summary for the human header.
    pub fn is_all_ok(&self) -> bool {
        self.capabilities.iter().all(|c| c.level == Level::Ok)
    }
}

/// Run the checks against a toolbox and a resolved compiler.
pub fn diagnose(
    tb: &dyn Toolbox,
    compiler: &Compiler,
    ctx: &Context,
    opts: &DoctorOptions,
) -> Report {
    let root = ctx.project_root.as_deref();
    let mut compile = compile_report(compiler);
    // #2240: a project with git dependencies needs `git` to fetch them.
    if root.is_some_and(has_git_dependencies) {
        compile.rows.push(detect_plain(
            tb,
            "git",
            "install git — this project has git dependencies, which `bynk fetch` clones",
        ));
        compile.level = compile
            .rows
            .iter()
            .map(|r| r.level)
            .max()
            .unwrap_or(Level::Ok);
    }
    let mut capabilities = vec![compile];

    // Only build the capability the user scoped to (plus the always-on compile
    // floor), so `--only test` doesn't probe Cloudflare. With no filter, build
    // them all.
    let want = |cap: Capability| opts.only.is_none() || opts.only == Some(cap);

    if want(Capability::Test) {
        let node = detect_node(tb, root, ctx.node_floor);
        let runner = detect_runner(tb, root);
        capabilities.push(capability(Capability::Test, vec![node, runner]));
    }
    if want(Capability::Deploy) {
        let node = detect_node(tb, root, ctx.node_floor);
        let wrangler = detect_wrangler(tb, root);
        capabilities.push(capability(Capability::Deploy, vec![node, wrangler]));
    }
    if want(Capability::Editor) {
        let lsp = detect_plain(
            tb,
            "bynkc-lsp",
            "install bynkc-lsp (or download from releases)",
        );
        capabilities.push(capability(Capability::Editor, vec![lsp]));
    }
    if ctx.in_repo && want(Capability::BuildFromSource) {
        let cargo = detect_plain(tb, "cargo", "install Rust via https://rustup.rs");
        capabilities.push(capability(Capability::BuildFromSource, vec![cargo]));
    }

    Report {
        driver_version: crate::DRIVER_VERSION.to_string(),
        compiler: compiler.clone(),
        capabilities,
    }
}

/// Compile/check/fmt. The compiler is **linked in-process** (slice 7 / ADR 0101),
/// so it is always available and cannot skew against itself — the always-ok row.
/// The external-`bynkc` resolution + skew check applies **only** under a
/// `BYNK_BYNKC` override (`Origin::Override`), the one path on which a second,
/// skewable compiler enters; with no override there is nothing external to check
/// (amends ADR 0084).
fn compile_report(compiler: &Compiler) -> CapabilityReport {
    let mut rows = vec![Row {
        label: "compiler".into(),
        level: Level::Ok,
        detail: "in-process".into(),
        remedy: None,
    }];

    // Only when the user explicitly pointed `bynk` at an external compiler does a
    // second binary — and thus skew — exist. Report it then, and only then.
    if matches!(compiler.origin, Some(Origin::Override)) {
        let ver = compiler
            .version
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".into());
        let row = match (&compiler.path, compiler.skew) {
            (None, _) => Row {
                label: "bynkc (override)".into(),
                level: Level::Fail,
                detail: "$BYNK_BYNKC set but not found".into(),
                remedy: Some("fix BYNK_BYNKC, or unset it to use the in-process compiler".into()),
            },
            (Some(_), Some(Skew::Major)) => Row {
                label: "bynkc (override)".into(),
                level: Level::Fail,
                detail: format!("{ver} — major skew vs driver"),
                remedy: Some("align the override bynkc with bynk, or unset BYNK_BYNKC".into()),
            },
            (Some(_), Some(Skew::Minor)) => Row {
                label: "bynkc (override)".into(),
                level: Level::Warn,
                detail: format!("{ver} — minor skew vs driver"),
                remedy: Some("align the override bynkc with bynk, or unset BYNK_BYNKC".into()),
            },
            // Resolved but its version could not be read — the binary may not
            // be a bynkc at all; don't render it as a clean Ok.
            (Some(_), None) => Row {
                label: "bynkc (override)".into(),
                level: Level::Warn,
                detail: "resolved, but its version could not be read".into(),
                remedy: Some("check that BYNK_BYNKC points at a bynkc binary".into()),
            },
            (Some(_), _) => Row {
                label: "bynkc (override)".into(),
                level: Level::Ok,
                detail: format!("{ver} (override)"),
                remedy: None,
            },
        };
        rows.push(row);
    }

    let level = rows.iter().map(|r| r.level).max().unwrap_or(Level::Ok);
    CapabilityReport {
        capability: Capability::Compile,
        optional: false,
        rows,
        level,
    }
}

/// Aggregate a capability from its rows (worst row wins).
fn capability(cap: Capability, rows: Vec<Row>) -> CapabilityReport {
    let level = rows.iter().map(|r| r.level).max().unwrap_or(Level::Ok);
    CapabilityReport {
        capability: cap,
        optional: cap.is_optional(),
        rows,
        level,
    }
}

fn detect_node(tb: &dyn Toolbox, root: Option<&std::path::Path>, floor: u32) -> Row {
    // A runtime is never npx-provisionable.
    let probe = probe::detect(
        tb,
        "node",
        DetectOpts {
            project_root: root,
            allow_npx: false,
        },
    );
    let remedy = format!("install Node.js ≥ {floor} from https://nodejs.org");
    if probe.is_missing() {
        return Row {
            label: "node".into(),
            level: Level::Fail,
            detail: "missing".into(),
            remedy: Some(remedy),
        };
    }
    let below = probe.version.map(|v| v.major < floor).unwrap_or(false);
    if below {
        let v = probe.version.unwrap();
        return Row {
            label: "node".into(),
            level: Level::Warn,
            detail: format!("v{v} below floor (≥ {floor})"),
            remedy: Some(remedy),
        };
    }
    Row {
        label: "node".into(),
        level: Level::Ok,
        detail: present_detail(&probe),
        remedy: None,
    }
}

/// The `tsc | tsx` runner requirement — satisfied by the *better* of the two.
fn detect_runner(tb: &dyn Toolbox, root: Option<&std::path::Path>) -> Row {
    let tsc = probe::detect(
        tb,
        "tsc",
        DetectOpts {
            project_root: root,
            allow_npx: true,
        },
    );
    let tsx = probe::detect(
        tb,
        "tsx",
        DetectOpts {
            project_root: root,
            allow_npx: true,
        },
    );
    let best = pick_better(&tsc, &tsx);
    let floor = bynk_emit::TYPESCRIPT_MAJOR_FLOOR;
    let tested = bynk_emit::TYPESCRIPT_MAJOR_TESTED;
    // #1672: lead with the TypeScript that is verified, and the one that
    // type-checks; `tsx` only runs the emitted code.
    let remedy = format!(
        "npm install -g typescript@{tested} (or `npm install -g tsx`, which runs tests without type-checking)"
    );
    match best {
        // #1672: a `tsc` below the verified floor is unsupported, a real defect
        // in this environment, so it warns. One above the tested major is
        // *reported* as untested but stays `ok`: it is a statement about this
        // repo's verification coverage, not a fault the user has, and a
        // warning would turn `doctor --strict` red for everyone the day a new
        // TypeScript major ships, with advice to downgrade a likely-working
        // toolchain.
        Some(p) if p.is_present() && p.tool == "tsc" => {
            let detail = format!("{} {}", p.tool, present_detail(p));
            match p.version.map(|v| v.major) {
                Some(major) if major < floor => Row {
                    label: "tsc | tsx".into(),
                    level: Level::Warn,
                    detail: format!("{detail}, below floor (≥ {floor})"),
                    remedy: Some(remedy),
                },
                Some(major) if major > tested => Row {
                    label: "tsc | tsx".into(),
                    level: Level::Ok,
                    detail: format!("{detail}, untested (verified up to {tested})"),
                    remedy: None,
                },
                _ => Row {
                    label: "tsc | tsx".into(),
                    level: Level::Ok,
                    detail,
                    remedy: None,
                },
            }
        }
        Some(p) if p.is_present() => Row {
            label: "tsc | tsx".into(),
            level: Level::Ok,
            detail: format!("{} {}", p.tool, present_detail(p)),
            remedy: None,
        },
        Some(p) => Row {
            // provisionable via npx
            label: "tsc | tsx".into(),
            level: Level::Warn,
            detail: format!("{} provisionable via npx (not installed)", p.tool),
            remedy: Some(remedy),
        },
        None => Row {
            label: "tsc | tsx".into(),
            level: Level::Fail,
            detail: "missing".into(),
            remedy: Some(remedy),
        },
    }
}

/// #1732: an installed wrangler older than [`bynk_emit::WRANGLER_MIN`], whose
/// `workerd` therefore refuses the pinned [`bynk_emit::COMPATIBILITY_DATE`].
/// `false` for an npx-provisioned or unversioned wrangler, which can't be
/// judged without running it. Shared by `doctor`'s row and `bynk dev`'s notice.
pub fn wrangler_below_min(probe: &Probe) -> bool {
    let min = probe::Version::parse(bynk_emit::WRANGLER_MIN).expect("WRANGLER_MIN is a version");
    probe.is_present()
        && probe
            .version
            .is_some_and(|v| (v.major, v.minor, v.patch) < (min.major, min.minor, min.patch))
}

/// #1732: how to upgrade a too-old wrangler, for where it came from. A
/// project's own wrangler (which the driver prefers over `PATH`) is upgraded in
/// the project, since a global install wouldn't change what runs. Shared by
/// `doctor`'s row and `bynk dev`'s notice so the two can't disagree. No version
/// in it: it shows in every `--format short` line and the goldens.
pub fn wrangler_upgrade_remedy(probe: &Probe) -> &'static str {
    match probe.provenance {
        Provenance::ProjectLocal(_) => "npm install --save-dev wrangler@4 (in the project)",
        _ => "npm install -g wrangler@4",
    }
}

/// #1732: wrangler, checked against [`bynk_emit::WRANGLER_MIN`], the oldest
/// whose `workerd` serves the [`bynk_emit::COMPATIBILITY_DATE`] every generated
/// `wrangler.toml` pins. An older one's `workerd` refuses the date, so
/// `bynk dev` fails. It also doesn't read the Durable Object `exports` map
/// (#1796), so a project with an agent can't deploy on it, and `bynk deploy`
/// refuses one that would push a class (`deploy::plan::wrangler_floor_refusal`).
/// A project with no agent still deploys (Cloudflare accepts any past date).
/// Doctor doesn't know which kind of project it is, so this stays a warning,
/// not a failure, and `doctor --only deploy` doesn't go red on a toolchain
/// that deploys an agent-free project. An npx-provisioned wrangler's version isn't known
/// without running npx (which may download), so its row keeps the usual
/// "provisionable" warning, and the remedy says to clear a stale npx cache: one
/// older than the minimum fails the same way, because npx keys the cache on the
/// spec `wrangler@4`, not the version.
fn detect_wrangler(tb: &dyn Toolbox, root: Option<&std::path::Path>) -> Row {
    let min = bynk_emit::WRANGLER_MIN;
    let date = bynk_emit::COMPATIBILITY_DATE;
    // No version or date in a remedy: it shows in every `--format short` line
    // (and the goldens), which a compatibility-date review shouldn't churn. The
    // below-minimum detail names both. Each remedy fits where wrangler came
    // from: a project's own wrangler is upgraded in the project, and only an npx
    // one can be a stale cache.
    let install = "npm install -g wrangler@4";
    let probe = probe::detect(
        tb,
        "wrangler",
        DetectOpts {
            project_root: root,
            allow_npx: true,
        },
    );
    if wrangler_below_min(&probe) {
        let remedy = wrangler_upgrade_remedy(&probe);
        return Row {
            label: "wrangler".into(),
            level: Level::Warn,
            detail: format!(
                "{}, below {min}: `bynk dev` can't serve compatibility date {date}, and agents can't deploy",
                present_detail(&probe)
            ),
            remedy: Some(remedy.into()),
        };
    }
    let mut row = npm_row("wrangler", &probe, install);
    if probe.is_provisionable() {
        row.remedy = Some(format!(
            "{install}, or clear a stale npx cache (~/.npm/_npx)"
        ));
    }
    row
}

/// #2240: whether the project at `root` (or its workspace) declares a git
/// dependency.
fn has_git_dependencies(root: &std::path::Path) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join("bynk.toml")) else {
        return false;
    };
    let Ok(doc) = text.parse::<toml::Table>() else {
        return false;
    };
    ["dependencies", "dev-dependencies"].iter().any(|table| {
        doc.get(*table).and_then(|v| v.as_table()).is_some_and(|t| {
            t.values()
                .any(|e| e.as_table().is_some_and(|e| e.contains_key("git")))
        })
    })
}

fn detect_plain(tb: &dyn Toolbox, tool: &str, remedy: &str) -> Row {
    let probe = probe::detect(
        tb,
        tool,
        DetectOpts {
            project_root: None,
            allow_npx: false,
        },
    );
    if probe.is_present() {
        Row {
            label: tool.into(),
            level: Level::Ok,
            detail: present_detail(&probe),
            remedy: None,
        }
    } else {
        Row {
            label: tool.into(),
            level: Level::Fail,
            detail: "missing".into(),
            remedy: Some(remedy.into()),
        }
    }
}

fn npm_row(tool: &str, probe: &Probe, remedy: &str) -> Row {
    if probe.is_present() {
        Row {
            label: tool.into(),
            level: Level::Ok,
            detail: present_detail(probe),
            remedy: None,
        }
    } else if probe.is_provisionable() {
        Row {
            label: tool.into(),
            level: Level::Warn,
            detail: "provisionable via npx (not installed)".into(),
            remedy: Some(remedy.into()),
        }
    } else {
        Row {
            label: tool.into(),
            level: Level::Fail,
            detail: "missing".into(),
            remedy: Some(remedy.into()),
        }
    }
}

/// `path`/`project-local` beats `npx` beats `missing`; among installed, prefer
/// the first argument (caller order).
fn pick_better<'a>(a: &'a Probe, b: &'a Probe) -> Option<&'a Probe> {
    fn rank(p: &Probe) -> u8 {
        if p.is_present() {
            2
        } else if p.is_provisionable() {
            1
        } else {
            0
        }
    }
    let (ra, rb) = (rank(a), rank(b));
    if ra == 0 && rb == 0 {
        None
    } else if ra >= rb {
        Some(a)
    } else {
        Some(b)
    }
}

fn present_detail(probe: &Probe) -> String {
    let ver = probe
        .version
        .map(|v| format!("v{v}"))
        .unwrap_or_else(|| "installed".into());
    format!("{ver} ({})", probe.provenance.token())
}
