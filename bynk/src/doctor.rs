//! `bynk doctor` — the capability model, the checks, and the exit-code
//! contract.
//!
//! Probes are **grouped by the capability they unlock**, not listed flat, so a
//! compile-only user is never told they are "unhealthy" for lacking `wrangler`.
//! The exit-code contract turns on *what an invocation asks about* (ADR: the
//! doctor output / exit-code contract):
//!
//! - **Bare `bynk doctor`** is informational. It surveys everything but treats
//!   only the *compile floor* (the linked-in compiler) as
//!   required, so it exits `0` even with `test`/`dev` unavailable.
//! - **`--only <capability>`** promotes that capability's tools to required.
//! - **`--strict`** promotes *all* warnings (optional gaps, `npx`
//!   provisionability, minor skew) to failures, for an all-green CI gate.

use std::path::PathBuf;

use bynk_driver::npm_install::PackageManager;

use crate::probe::{self, DetectOpts, Probe, Provenance, Toolbox};

/// A unit of work a user might want to do, and the tools it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Compile / check / fmt. Always satisfiable: the compiler is linked in.
    Compile,
    /// `bynk test` — Node and one of `tsc`/`tsx` (the runner ladder).
    Test,
    /// `dev` / deploy to Cloudflare — Node and `wrangler`.
    Deploy,
    /// Editor support — `bynk-lsp`. Optional; never a failure (except strict).
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
    /// Minimum supported Node `(major, minor)` (single-sourced from
    /// `bynk_emit::NODE_FLOOR`).
    pub node_floor: (u32, u32),
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

/// Run the checks against a toolbox.
pub fn diagnose(tb: &dyn Toolbox, ctx: &Context, opts: &DoctorOptions) -> Report {
    let root = ctx.project_root.as_deref();
    let mut compile = compile_report();
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
        let packages = detect_package_manager(tb, root);
        capabilities.push(capability(Capability::Test, vec![node, runner, packages]));
    }
    if want(Capability::Deploy) {
        let node = detect_node(tb, root, ctx.node_floor);
        let wrangler = detect_wrangler(tb, root);
        let packages = detect_package_manager(tb, root);
        let mut rows = vec![node, wrangler, packages];
        // #3477: only a project that declares a container has a use for Docker,
        // so only its report carries the row.
        if root.is_some_and(declares_container) {
            rows.push(detect_docker(tb));
        }
        capabilities.push(capability(Capability::Deploy, rows));
    }
    if want(Capability::Editor) {
        let lsp = detect_plain(
            tb,
            "bynk-lsp",
            "install bynk-lsp (or download from releases)",
        );
        capabilities.push(capability(Capability::Editor, vec![lsp]));
    }
    if ctx.in_repo && want(Capability::BuildFromSource) {
        let cargo = detect_plain(tb, "cargo", "install Rust via https://rustup.rs");
        capabilities.push(capability(Capability::BuildFromSource, vec![cargo]));
    }

    Report {
        driver_version: crate::DRIVER_VERSION.to_string(),
        capabilities,
    }
}

/// Compile/check/fmt/test. The compiler is **linked in-process** (ADR 0101), so it
/// is always available and cannot skew against itself — the always-ok row. There
/// is no external compiler to resolve or report on (#2998, ADR 0736).
fn compile_report() -> CapabilityReport {
    CapabilityReport {
        capability: Capability::Compile,
        optional: false,
        rows: vec![Row {
            label: "compiler".into(),
            level: Level::Ok,
            detail: "in-process".into(),
            remedy: None,
        }],
        level: Level::Ok,
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

fn detect_node(tb: &dyn Toolbox, root: Option<&std::path::Path>, floor: (u32, u32)) -> Row {
    // A runtime is never npx-provisionable.
    let probe = probe::detect(
        tb,
        "node",
        DetectOpts {
            project_root: root,
            allow_npx: false,
            manager: None,
        },
    );
    let shown = format!("{}.{}", floor.0, floor.1);
    let remedy = format!("install Node.js ≥ {shown} from https://nodejs.org");
    if probe.is_missing() {
        return Row {
            label: "node".into(),
            level: Level::Fail,
            detail: "missing".into(),
            remedy: Some(remedy),
        };
    }
    let below = probe
        .version
        .map(|v| (v.major, v.minor) < floor)
        .unwrap_or(false);
    if below {
        let v = probe.version.unwrap();
        return Row {
            label: "node".into(),
            level: Level::Warn,
            detail: format!("v{v} below floor (≥ {shown})"),
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
    let manager = tool_manager(tb, root);
    let opts = DetectOpts {
        project_root: root,
        allow_npx: true,
        manager: Some(manager),
    };
    let tsc = probe::detect(tb, "tsc", opts);
    let tsx = probe::detect(tb, "tsx", opts);
    let best = pick_better(&tsc, &tsx);
    let tested = bynk_emit::TYPESCRIPT_MAJOR_TESTED;
    // #1672: lead with the TypeScript that is verified, and the one that
    // type-checks; `tsx` only runs the emitted code.
    let remedy = format!(
        "{} (or `{}`, which runs tests without type-checking)",
        manager.global_install(&format!("typescript@{tested}")),
        manager.global_install("tsx")
    );
    match best {
        // #1672, #3420: a `tsc` below the supported major is unsupported, a real defect
        // in this environment, so it warns. One above the tested major is
        // *reported* as untested but stays `ok`: it is a statement about this
        // repo's verification coverage, not a fault the user has, and a
        // warning would turn `doctor --strict` red for everyone the day a new
        // TypeScript major ships, with advice to downgrade a likely-working
        // toolchain.
        Some(p) if p.is_present() && p.tool == "tsc" => {
            let detail = format!("{} {}", p.tool, present_detail(p));
            match p.version.map(|v| v.major) {
                Some(major) if major < tested => Row {
                    label: "tsc | tsx".into(),
                    level: Level::Warn,
                    detail: format!("{detail}, below supported (≥ {tested})"),
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
            // provisionable through the package manager's runner
            label: "tsc | tsx".into(),
            level: Level::Warn,
            detail: format!(
                "{} provisionable via {} (not installed)",
                p.tool,
                p.provenance.runner_label().unwrap_or("npx")
            ),
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
pub fn wrangler_upgrade_remedy(probe: &Probe, manager: PackageManager) -> String {
    match probe.provenance {
        Provenance::ProjectLocal(_) => {
            format!("{} (in the project)", manager.dev_install("wrangler@4"))
        }
        _ => manager.global_install("wrangler@4"),
    }
}

/// The package manager the toolchain's tool fallbacks and hints follow for the
/// project at `root`: the detected one, or npm with no project.
fn tool_manager(tb: &dyn Toolbox, root: Option<&std::path::Path>) -> PackageManager {
    root.map_or(PackageManager::Npm, |r| {
        bynk_driver::npm_install::manager_in(r, tb)
    })
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
    let manager = tool_manager(tb, root);
    let install = manager.global_install("wrangler@4");
    let probe = probe::detect(
        tb,
        "wrangler",
        DetectOpts {
            project_root: root,
            allow_npx: true,
            manager: Some(manager),
        },
    );
    if wrangler_below_min(&probe) {
        let remedy = wrangler_upgrade_remedy(&probe, manager);
        return Row {
            label: "wrangler".into(),
            level: Level::Warn,
            detail: format!(
                "{}, below {min}: `bynk dev` can't serve compatibility date {date}, and agents can't deploy",
                present_detail(&probe)
            ),
            remedy: Some(remedy),
        };
    }
    let mut row = npm_row("wrangler", &probe, &install);
    // Only npx keys a cache on the spec (`wrangler@4`), so only it can be stale.
    if probe.provenance == Provenance::Runner(PackageManager::Npm) {
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

/// #3350: the package manager `bynk test`, `bynk dev` and `bynk deploy` install
/// the generated project's npm packages with, and the rule that chose it.
fn detect_package_manager(tb: &dyn Toolbox, root: Option<&std::path::Path>) -> Row {
    use bynk_driver::npm_install::{self, Project};

    /// The project root's files (none without a root) and the real environment.
    struct View<'a>(Option<&'a std::path::Path>);
    impl Project for View<'_> {
        fn read(&self, name: &str) -> Option<String> {
            self.0.and_then(|r| npm_install::RealProject(r).read(name))
        }
        fn exists(&self, name: &str) -> bool {
            self.0
                .is_some_and(|r| npm_install::RealProject(r).exists(name))
        }
        fn env(&self, key: &str) -> Option<String> {
            std::env::var(key).ok().filter(|v| !v.trim().is_empty())
        }
    }

    let setting = root.and_then(npm_install::configured_manager);
    match npm_install::detect(&View(root), setting.as_deref(), tb) {
        Ok(found) => Row {
            label: "package manager".into(),
            level: Level::Ok,
            detail: format!("{} ({})", found.manager.name(), found.rule.describe()),
            remedy: None,
        },
        // A package manager that cannot be chosen is a hard error for a run that
        // needs to install, so an unknown explicit setting warns.
        Err(e @ npm_install::DetectError::UnknownManager { .. }) => Row {
            label: "package manager".into(),
            level: Level::Warn,
            detail: "unknown".into(),
            remedy: Some(e.to_string()),
        },
        // Informational: only a project with npm dependencies needs one, so
        // its absence is not a fault (it would turn `--strict` red for others).
        Err(e) => Row {
            label: "package manager".into(),
            level: Level::Ok,
            detail: format!("none found ({e})"),
            remedy: None,
        },
    }
}

/// Whether the project declares a `Container` (`capability X = Container …`):
/// a text scan of its `.bynk` files, which skips dependency, build and VCS
/// directories. A comment that spells the declaration counts, which only makes
/// the Docker row louder than it needs to be.
pub(crate) fn declares_container(root: &std::path::Path) -> bool {
    fn walk(dir: &std::path::Path, depth: usize) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                depth < 8
                    && !matches!(
                        name.as_ref(),
                        "node_modules" | "target" | "out" | "dist" | ".git"
                    )
                    && walk(&path, depth + 1)
            } else {
                name.ends_with(".bynk")
                    && std::fs::read_to_string(&path).is_ok_and(|text| {
                        text.lines().any(|l| {
                            l.trim_start().starts_with("capability ")
                                && l.split_once("= Container").is_some_and(|(_, rest)| {
                                    !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_')
                                })
                        })
                    })
            }
        })
    }
    walk(root, 0)
}

/// #3477: Docker, which `bynk deploy` needs only to build a `Container`'s image
/// from its Dockerfile (an adopted image needs none). Listed only for a project
/// that declares a container ([`declares_container`]); a missing or stopped
/// Docker is a warning, not a failure, since the deploy may adopt an image
/// instead. Worth knowing before a deploy that needs it.
fn detect_docker(tb: &dyn Toolbox) -> Row {
    const REMEDY: &str = "needed only to build a Container's image at deploy: start Docker, or adopt a prebuilt image in bynk.toml ([deploy.containers.<Capability>])";
    let probe = probe::detect(
        tb,
        "docker",
        DetectOpts {
            project_root: None,
            allow_npx: false,
            manager: None,
        },
    );
    let path = match &probe.provenance {
        Provenance::Path(path) | Provenance::ProjectLocal(path) => Some(path.clone()),
        _ => None,
    };
    let warn = |detail: String| Row {
        label: "docker".into(),
        level: Level::Warn,
        detail,
        remedy: Some(REMEDY.into()),
    };
    let Some(path) = path.filter(|_| probe.is_present()) else {
        return warn("not found".into());
    };
    if tb.docker_daemon_running(&path) {
        Row {
            label: "docker".into(),
            level: Level::Ok,
            detail: format!("{} (daemon running)", present_detail(&probe)),
            remedy: None,
        }
    } else {
        warn(format!("{} (no daemon running)", present_detail(&probe)))
    }
}

fn detect_plain(tb: &dyn Toolbox, tool: &str, remedy: &str) -> Row {
    let probe = probe::detect(
        tb,
        tool,
        DetectOpts {
            project_root: None,
            allow_npx: false,
            manager: None,
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
            detail: format!(
                "provisionable via {} (not installed)",
                probe.provenance.runner_label().unwrap_or("npx")
            ),
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
