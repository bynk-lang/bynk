//! Installing the npm packages a generated build directory lists, with the
//! package manager the user already has (#3350).
//!
//! `bynk test`, `bynk dev` and `bynk deploy` all build into a directory bynk
//! generates (`out/`, `.bynk/dev`, `.bynk/deploy`) whose `package.json` pins the
//! toolchain's own npm packages (`postal-mime` for inbound email, ADR 0799 C)
//! and any adapter's approved ones. This module is the one routine that makes
//! those packages present, and it runs a tool only when a listed package is
//! missing, so a warm re-run needs no network.
//!
//! Which tool downloads the packages is decided by [`detect`], a pure function
//! over an injected [`Project`] view (files and environment) and a
//! [`Toolbox`] (PATH), so every rule is unit-tested without a real tool. The
//! install runs through a [`Runner`] for the same reason.
//!
//! Lifecycle scripts are always off. The approval of a fetched adapter's npm
//! packages (ADR 0575) is enforced when the project compiles, before the build
//! directory exists, so nothing here can install an unapproved package: the
//! `package.json` this reads only lists approved ones and the toolchain's own.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::probe::{SystemToolbox, Toolbox};

/// The environment variable that overrides `bynk.toml`'s `[build] package_manager`.
pub const PACKAGE_MANAGER_ENV: &str = "BYNK_PACKAGE_MANAGER";

/// The environment form of `--no-install` (for `bynk dev` and `bynk deploy`).
pub const NO_INSTALL_ENV: &str = "BYNK_NO_INSTALL";

/// Whether `BYNK_NO_INSTALL` asks for `--no-install`: set, and not `0`,
/// `false`, `no` or `off`.
pub fn no_install_from_env() -> bool {
    std::env::var(NO_INSTALL_ENV).is_ok_and(|v| {
        let v = v.trim().to_ascii_lowercase();
        !matches!(v.as_str(), "" | "0" | "false" | "no" | "off")
    })
}

/// A package manager bynk can install with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Npm,
    Pnpm,
    Bun,
    /// Yarn classic (v1) only; Yarn Berry is refused (its Plug'n'Play layout leaves no `node_modules`).
    Yarn,
}

impl PackageManager {
    /// Every manager a setting may name.
    pub const ALL: [PackageManager; 4] = [Self::Npm, Self::Pnpm, Self::Bun, Self::Yarn];

    /// The executable's name, which is also the setting's value.
    pub fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Bun => "bun",
            Self::Yarn => "yarn",
        }
    }

    /// Parse a setting or a `packageManager` name (no version).
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name.trim())
    }

    /// The arguments of the install. Lifecycle scripts are always off.
    pub fn install_args(self) -> &'static [&'static str] {
        match self {
            Self::Npm => &["install", "--no-audit", "--no-fund", "--ignore-scripts"],
            // Without `--ignore-workspace`, a `pnpm-workspace.yaml` above the
            // generated directory would pull it into the user's workspace.
            Self::Pnpm => &["install", "--ignore-scripts", "--ignore-workspace"],
            Self::Bun => &["install", "--ignore-scripts"],
            Self::Yarn => &["install", "--ignore-scripts", "--non-interactive"],
        }
    }

    /// The command as a user would type it, for messages (`pnpm install`).
    pub fn command_line(self) -> String {
        format!("{} {}", self.name(), self.install_args().join(" "))
    }
}

/// The rule that chose the package manager, for `bynk doctor` and messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// `BYNK_PACKAGE_MANAGER`.
    Env,
    /// `bynk.toml`'s `[build] package_manager`.
    Setting,
    /// The project root `package.json`'s `packageManager` field.
    PackageJson,
    /// A lockfile at the project root, by file name.
    Lockfile(&'static str),
    /// `npm_config_user_agent`.
    UserAgent,
    /// The first of npm, pnpm and bun found on PATH.
    Path,
}

impl Rule {
    /// A short phrase for the reader: why this tool.
    pub fn describe(&self) -> String {
        match self {
            Rule::Env => format!("`{PACKAGE_MANAGER_ENV}` is set"),
            Rule::Setting => "`bynk.toml` sets `[build] package_manager`".to_string(),
            Rule::PackageJson => "`package.json` names it in `packageManager`".to_string(),
            Rule::Lockfile(file) => format!("`{file}` is at the project root"),
            Rule::UserAgent => "bynk was started by it (`npm_config_user_agent`)".to_string(),
            Rule::Path => "the first of npm, pnpm and bun on PATH".to_string(),
        }
    }
}

/// The detected package manager and the rule that chose it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub manager: PackageManager,
    pub rule: Rule,
}

/// Why detection chose nothing usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectError {
    /// An explicit setting names a manager bynk does not know.
    UnknownManager { source: &'static str, value: String },
    /// No explicit choice, and none of npm, pnpm or bun is on PATH.
    NoneAvailable,
}

impl std::fmt::Display for DetectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetectError::UnknownManager { source, value } => write!(
                f,
                "{source} names the package manager `{value}`, which bynk cannot install with (npm, pnpm, bun or yarn)"
            ),
            DetectError::NoneAvailable => f.write_str(
                "none of npm, pnpm or bun is on PATH to install the generated project's npm packages; install Node.js (which ships npm), or set `[build] package_manager`",
            ),
        }
    }
}

/// The files and environment of the project, as a view detection reads.
pub trait Project {
    /// The text of a file at the project root, if it exists.
    fn read(&self, name: &str) -> Option<String>;
    /// Whether a file exists at the project root.
    fn exists(&self, name: &str) -> bool;
    /// An environment variable.
    fn env(&self, key: &str) -> Option<String>;
}

/// [`Project`] over the real file system and process environment.
pub struct RealProject<'a>(pub &'a Path);

impl Project for RealProject<'_> {
    fn read(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(name)).ok()
    }

    fn exists(&self, name: &str) -> bool {
        self.0.join(name).is_file()
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.trim().is_empty())
    }
}

/// Lockfiles in the order they are tried.
const LOCKFILES: [(&str, PackageManager); 5] = [
    ("pnpm-lock.yaml", PackageManager::Pnpm),
    ("bun.lock", PackageManager::Bun),
    ("bun.lockb", PackageManager::Bun),
    ("yarn.lock", PackageManager::Yarn),
    ("package-lock.json", PackageManager::Npm),
];

/// The managers tried against PATH, in order. npm first: it ships with Node.
const PATH_ORDER: [PackageManager; 3] = [
    PackageManager::Npm,
    PackageManager::Pnpm,
    PackageManager::Bun,
];

/// Choose the package manager, first match wins:
///
/// 1. an explicit setting, `BYNK_PACKAGE_MANAGER` over `bynk.toml`'s
///    `[build] package_manager` (`setting`);
/// 2. the `packageManager` field of the project root's `package.json`;
/// 3. a lockfile at the project root;
/// 4. `npm_config_user_agent`;
/// 5. the first of npm, pnpm and bun on PATH.
///
/// A `packageManager` or user agent naming a tool bynk does not install with
/// is skipped, since neither is a setting bynk owns; an explicit setting
/// naming one is an error.
pub fn detect(
    project: &dyn Project,
    setting: Option<&str>,
    tb: &dyn Toolbox,
) -> Result<Detection, DetectError> {
    if let Some(found) = explicit(project, setting)? {
        return Ok(found);
    }
    if let Some(found) = from_package_json(project)
        .or_else(|| from_lockfile(project))
        .or_else(|| from_user_agent(project))
    {
        return Ok(found);
    }
    PATH_ORDER
        .into_iter()
        .find(|m| tb.on_path(m.name()).is_some())
        .map(|manager| Detection {
            manager,
            rule: Rule::Path,
        })
        .ok_or(DetectError::NoneAvailable)
}

fn explicit(
    project: &dyn Project,
    setting: Option<&str>,
) -> Result<Option<Detection>, DetectError> {
    let candidates = [
        (
            Rule::Env,
            "`BYNK_PACKAGE_MANAGER`",
            project.env(PACKAGE_MANAGER_ENV),
        ),
        (
            Rule::Setting,
            "`bynk.toml`'s `[build] package_manager`",
            setting.map(str::to_string),
        ),
    ];
    for (rule, source, value) in candidates {
        let Some(value) = value else { continue };
        let manager = PackageManager::parse(&value).ok_or(DetectError::UnknownManager {
            source,
            value: value.clone(),
        })?;
        return Ok(Some(Detection { manager, rule }));
    }
    Ok(None)
}

fn from_package_json(project: &dyn Project) -> Option<Detection> {
    let text = project.read("package.json")?;
    let doc: serde_json::Value = serde_json::from_str(&text).ok()?;
    let field = doc.get("packageManager")?.as_str()?;
    let manager = PackageManager::parse(field.split('@').next()?)?;
    Some(Detection {
        manager,
        rule: Rule::PackageJson,
    })
}

fn from_lockfile(project: &dyn Project) -> Option<Detection> {
    LOCKFILES
        .into_iter()
        .find(|(file, _)| project.exists(file))
        .map(|(file, manager)| Detection {
            manager,
            rule: Rule::Lockfile(file),
        })
}

fn from_user_agent(project: &dyn Project) -> Option<Detection> {
    let agent = project.env("npm_config_user_agent")?;
    // `pnpm/9.1.0 npm/? node/v22 linux x64`: the first token names the tool.
    let tool = agent.split(['/', ' ']).next()?;
    let manager = PackageManager::parse(tool)?;
    Some(Detection {
        manager,
        rule: Rule::UserAgent,
    })
}

/// The npm packages the emitted `package.json` lists (an adapter's, and the
/// toolchain's own, ADR 0799 C) that `node_modules` beside it does not hold,
/// sorted.
pub fn missing_npm_dependencies(build_dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(build_dir.join("package.json")) else {
        return Vec::new();
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let mut missing: Vec<String> = manifest["dependencies"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, _)| name.clone())
        .filter(|name| {
            !build_dir
                .join("node_modules")
                .join(name)
                .join("package.json")
                .is_file()
        })
        .collect();
    missing.sort();
    missing
}

/// What an install request is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Install what is missing.
    Install,
    /// `--no-install`: stop, naming the command, instead of installing.
    NoInstall,
    /// `--dry-run`: report what would be installed, and run nothing.
    DryRun,
}

impl Mode {
    /// The mode of a `bynk dev` / `bynk deploy` run. A dry run never installs,
    /// so it outranks `--no-install`.
    pub fn of(dry_run: bool, no_install: bool) -> Mode {
        if dry_run {
            Mode::DryRun
        } else if no_install {
            Mode::NoInstall
        } else {
            Mode::Install
        }
    }
}

/// What [`ensure_installed`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Every listed package was present (or none is listed): nothing ran.
    Present,
    /// The packages were installed.
    Installed(Detection),
    /// A dry run: these would be installed, with this tool.
    WouldInstall(Vec<String>, Detection),
}

/// Why the packages are not installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    Detect(DetectError),
    /// `--no-install`: today's preflight stop.
    Declined {
        missing: Vec<String>,
        command: String,
        dir: PathBuf,
    },
    /// The chosen tool is not on PATH.
    ToolMissing {
        detection: Detection,
    },
    /// Yarn Berry, which bynk does not install with.
    YarnBerry {
        version: String,
    },
    /// The tool ran and failed, or could not be started.
    Failed {
        command: String,
        dir: PathBuf,
        detail: String,
    },
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Detect(e) => write!(f, "{e}"),
            InstallError::Declined {
                missing,
                command,
                dir,
            } => write!(
                f,
                "the build needs npm packages that are not installed: {}. Run `{command}` in `{}`, then try again",
                missing.join(", "),
                dir.display()
            ),
            InstallError::ToolMissing { detection } => write!(
                f,
                "`{}` is not on PATH to install the generated project's npm packages ({}); install it, or choose another with `[build] package_manager` or `{PACKAGE_MANAGER_ENV}`",
                detection.manager.name(),
                detection.rule.describe()
            ),
            InstallError::YarnBerry { version } => write!(
                f,
                "yarn {version} is Yarn Berry, which bynk does not install with (its Plug'n'Play layout leaves no `node_modules`); use Yarn classic, or choose npm, pnpm or bun with `[build] package_manager` or `{PACKAGE_MANAGER_ENV}`"
            ),
            InstallError::Failed {
                command,
                dir,
                detail,
            } => write!(
                f,
                "`{command}` failed in `{}`:\n{}",
                dir.display(),
                detail.trim_end()
            ),
        }
    }
}

impl std::error::Error for InstallError {}

/// Runs a tool to completion. The seam for tests; [`SystemRunner`] is the real one.
pub trait Runner {
    /// Run `program args` in `dir`; `Ok((success, output))` with the combined
    /// stdout and stderr, `Err` when the tool could not be started.
    fn run(&self, program: &Path, args: &[&str], dir: &Path) -> Result<(bool, String), String>;
}

/// [`Runner`] over [`std::process::Command`].
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &Path, args: &[&str], dir: &Path) -> Result<(bool, String), String> {
        let out = Command::new(program)
            .args(args)
            .current_dir(dir)
            .output()
            .map_err(|e| format!("could not run `{}`: {e}", program.display()))?;
        let mut text = String::from_utf8_lossy(&out.stderr).into_owned();
        if text.trim().is_empty() {
            text = String::from_utf8_lossy(&out.stdout).into_owned();
        }
        Ok((out.status.success(), text))
    }
}

/// Everything [`ensure_installed`] reads: the project, its `bynk.toml`
/// setting, the toolbox and the runner.
pub struct Install<'a> {
    pub project: &'a dyn Project,
    /// `bynk.toml`'s `[build] package_manager`.
    pub setting: Option<&'a str>,
    pub toolbox: &'a dyn Toolbox,
    pub runner: &'a dyn Runner,
}

/// Make the packages `build_dir/package.json` lists present in its
/// `node_modules`. Nothing runs when none is missing.
pub fn ensure_installed(
    install: &Install<'_>,
    build_dir: &Path,
    mode: Mode,
) -> Result<Outcome, InstallError> {
    let missing = missing_npm_dependencies(build_dir);
    if missing.is_empty() {
        return Ok(Outcome::Present);
    }
    let detection =
        detect(install.project, install.setting, install.toolbox).map_err(InstallError::Detect)?;
    match mode {
        Mode::DryRun => Ok(Outcome::WouldInstall(missing, detection)),
        Mode::NoInstall => Err(InstallError::Declined {
            command: detection.manager.command_line(),
            missing,
            dir: build_dir.to_path_buf(),
        }),
        Mode::Install => run_install(install, build_dir, detection),
    }
}

fn run_install(
    install: &Install<'_>,
    build_dir: &Path,
    detection: Detection,
) -> Result<Outcome, InstallError> {
    let manager = detection.manager;
    let Some(program) = install.toolbox.on_path(manager.name()) else {
        return Err(InstallError::ToolMissing { detection });
    };
    if manager == PackageManager::Yarn {
        check_yarn_classic(install.runner, &program, build_dir)?;
    }
    let failed = |detail: String| InstallError::Failed {
        command: manager.command_line(),
        dir: build_dir.to_path_buf(),
        detail,
    };
    match install
        .runner
        .run(&program, manager.install_args(), build_dir)
    {
        Ok((true, _)) => Ok(Outcome::Installed(detection)),
        Ok((false, output)) => Err(failed(output)),
        Err(e) => Err(failed(e)),
    }
}

/// Yarn Berry defaults to Plug'n'Play, which leaves no `node_modules` for
/// wrangler's bundler or the test link, so only Yarn classic installs here.
fn check_yarn_classic(runner: &dyn Runner, yarn: &Path, dir: &Path) -> Result<(), InstallError> {
    let Ok((true, text)) = runner.run(yarn, &["--version"], dir) else {
        return Ok(());
    };
    let version = text.trim().to_string();
    match version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
    {
        Some(major) if major >= 2 => Err(InstallError::YarnBerry { version }),
        _ => Ok(()),
    }
}

/// The tool `bynk dev`/`bynk deploy`/`bynk test` print before a real install,
/// so a slow download does not read as a hang.
pub fn announce(detection: &Detection, build_dir: &Path) -> String {
    format!(
        "bynk: installing npm packages with {} in `{}`",
        detection.manager.name(),
        build_dir.display()
    )
}

/// The dry-run line: what would be installed, and with which tool.
pub fn dry_run_message(missing: &[String], detection: &Detection, build_dir: &Path) -> String {
    format!(
        "bynk: the build needs npm packages that are not installed: {}. A real run would install them with `{}` in `{}` ({})",
        missing.join(", "),
        detection.manager.command_line(),
        build_dir.display(),
        detection.rule.describe()
    )
}

/// The whole step for a caller with the real file system, tools and process
/// environment: detect, install, and print the announcements. `Err` is the
/// message to print (without the `bynk: ` prefix).
pub fn install_for_project(
    project_root: &Path,
    build_dir: &Path,
    setting: Option<&str>,
    mode: Mode,
) -> Result<(), String> {
    let project = RealProject(project_root);
    let toolbox = SystemToolbox;
    let install = Install {
        project: &project,
        setting,
        toolbox: &toolbox,
        runner: &SystemRunner,
    };
    // Announce before running: a download with no output reads as a hang.
    if mode == Mode::Install
        && !missing_npm_dependencies(build_dir).is_empty()
        && let Ok(found) = detect(&project, setting, &toolbox)
    {
        eprintln!("{}", announce(&found, build_dir));
    }
    match ensure_installed(&install, build_dir, mode).map_err(|e| e.to_string())? {
        Outcome::WouldInstall(missing, found) => {
            eprintln!("{}", dry_run_message(&missing, &found, build_dir));
            Ok(())
        }
        Outcome::Present | Outcome::Installed(_) => Ok(()),
    }
}

/// `bynk.toml`'s `[build] package_manager`, if it sets one and the manifest
/// reads. A malformed manifest is reported by the build, not here.
pub fn configured_manager(project_root: &Path) -> Option<String> {
    bynk_project::try_read_build_config(project_root)
        .ok()
        .and_then(|c| c.package_manager)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::Version;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Default)]
    struct Fake {
        files: BTreeMap<&'static str, String>,
        env: BTreeMap<&'static str, &'static str>,
        path: BTreeSet<&'static str>,
    }

    impl Project for Fake {
        fn read(&self, name: &str) -> Option<String> {
            self.files.get(name).cloned()
        }
        fn exists(&self, name: &str) -> bool {
            self.files.contains_key(name)
        }
        fn env(&self, key: &str) -> Option<String> {
            self.env.get(key).map(|v| v.to_string())
        }
    }

    impl Toolbox for Fake {
        fn on_path(&self, tool: &str) -> Option<PathBuf> {
            self.path
                .contains(tool)
                .then(|| PathBuf::from(format!("/bin/{tool}")))
        }
        fn in_dir(&self, _: &Path, _: &str) -> Option<PathBuf> {
            None
        }
        fn version(&self, _: &Path) -> Option<Version> {
            None
        }
        fn npx_available(&self) -> bool {
            false
        }
    }

    fn pick(fake: &Fake, setting: Option<&str>) -> Result<Detection, DetectError> {
        detect(fake, setting, fake)
    }

    fn chose(fake: &Fake, setting: Option<&str>) -> (PackageManager, Rule) {
        let d = pick(fake, setting).unwrap();
        (d.manager, d.rule)
    }

    #[test]
    fn the_environment_overrides_the_setting() {
        let mut f = Fake::default();
        f.env.insert(PACKAGE_MANAGER_ENV, "bun");
        assert_eq!(chose(&f, Some("pnpm")), (PackageManager::Bun, Rule::Env));
    }

    #[test]
    fn the_setting_beats_package_json_and_lockfiles() {
        let mut f = Fake::default();
        f.files
            .insert("package.json", r#"{"packageManager":"pnpm@9.0.0"}"#.into());
        f.files.insert("yarn.lock", String::new());
        assert_eq!(chose(&f, Some("bun")), (PackageManager::Bun, Rule::Setting));
    }

    #[test]
    fn an_unknown_explicit_setting_is_an_error_naming_where_it_came_from() {
        let f = Fake::default();
        let e = pick(&f, Some("deno")).unwrap_err();
        assert!(
            e.to_string().contains("`bynk.toml`") && e.to_string().contains("`deno`"),
            "{e}"
        );
        let mut f = Fake::default();
        f.env.insert(PACKAGE_MANAGER_ENV, "nope");
        assert!(
            pick(&f, None)
                .unwrap_err()
                .to_string()
                .contains("BYNK_PACKAGE_MANAGER")
        );
    }

    #[test]
    fn package_json_names_the_manager_before_any_version() {
        for (field, want) in [
            ("pnpm@9.1.0+sha512.abc", PackageManager::Pnpm),
            ("yarn@1.22.22", PackageManager::Yarn),
            ("bun@1.1.0", PackageManager::Bun),
            ("npm@10.0.0", PackageManager::Npm),
        ] {
            let mut f = Fake::default();
            f.files
                .insert("package.json", format!(r#"{{"packageManager":"{field}"}}"#));
            f.files.insert("package-lock.json", String::new());
            assert_eq!(chose(&f, None), (want, Rule::PackageJson), "{field}");
        }
    }

    #[test]
    fn an_unknown_or_absent_package_manager_field_falls_through() {
        let mut f = Fake::default();
        f.files
            .insert("package.json", r#"{"packageManager":"deno@2"}"#.into());
        f.files.insert("pnpm-lock.yaml", String::new());
        assert_eq!(
            chose(&f, None),
            (PackageManager::Pnpm, Rule::Lockfile("pnpm-lock.yaml"))
        );
        let mut f = Fake::default();
        f.files.insert("package.json", "{ not json".into());
        f.files.insert("yarn.lock", String::new());
        assert_eq!(
            chose(&f, None),
            (PackageManager::Yarn, Rule::Lockfile("yarn.lock"))
        );
    }

    #[test]
    fn each_lockfile_names_its_manager() {
        for (file, want) in [
            ("pnpm-lock.yaml", PackageManager::Pnpm),
            ("bun.lock", PackageManager::Bun),
            ("bun.lockb", PackageManager::Bun),
            ("yarn.lock", PackageManager::Yarn),
            ("package-lock.json", PackageManager::Npm),
        ] {
            let mut f = Fake::default();
            f.files.insert(file, String::new());
            assert_eq!(chose(&f, None), (want, Rule::Lockfile(file)), "{file}");
        }
    }

    #[test]
    fn a_lockfile_beats_the_user_agent_and_the_user_agent_beats_path() {
        let mut f = Fake::default();
        f.files.insert("yarn.lock", String::new());
        f.env
            .insert("npm_config_user_agent", "pnpm/9.0.0 npm/? node/v22");
        assert_eq!(chose(&f, None).0, PackageManager::Yarn);

        let mut f = Fake::default();
        f.env
            .insert("npm_config_user_agent", "pnpm/9.0.0 npm/? node/v22");
        f.path.insert("npm");
        assert_eq!(chose(&f, None), (PackageManager::Pnpm, Rule::UserAgent));
        for (agent, want) in [
            ("bun/1.1.0 npm/? node/v22", PackageManager::Bun),
            ("yarn/1.22.0 npm/? node/v22", PackageManager::Yarn),
            ("npm/10.0.0 node/v22 linux x64", PackageManager::Npm),
        ] {
            let mut f = Fake::default();
            f.env.insert("npm_config_user_agent", agent);
            assert_eq!(chose(&f, None), (want, Rule::UserAgent), "{agent}");
        }
    }

    #[test]
    fn an_unrecognised_user_agent_falls_through_to_path() {
        let mut f = Fake::default();
        f.env.insert("npm_config_user_agent", "deno/2.0");
        f.path.insert("bun");
        assert_eq!(chose(&f, None), (PackageManager::Bun, Rule::Path));
    }

    #[test]
    fn path_prefers_npm_then_pnpm_then_bun() {
        let mut f = Fake::default();
        f.path.extend(["bun", "pnpm", "npm"]);
        assert_eq!(chose(&f, None), (PackageManager::Npm, Rule::Path));
        f.path.remove("npm");
        assert_eq!(chose(&f, None).0, PackageManager::Pnpm);
        f.path.remove("pnpm");
        assert_eq!(chose(&f, None).0, PackageManager::Bun);
        f.path.remove("bun");
        assert_eq!(pick(&f, None), Err(DetectError::NoneAvailable));
    }

    #[test]
    fn each_tools_arguments_turn_lifecycle_scripts_off() {
        assert_eq!(
            PackageManager::Npm.install_args(),
            ["install", "--no-audit", "--no-fund", "--ignore-scripts"]
        );
        assert_eq!(
            PackageManager::Pnpm.install_args(),
            ["install", "--ignore-scripts", "--ignore-workspace"]
        );
        assert_eq!(
            PackageManager::Bun.install_args(),
            ["install", "--ignore-scripts"]
        );
        assert_eq!(
            PackageManager::Yarn.install_args(),
            ["install", "--ignore-scripts", "--non-interactive"]
        );
        for m in PackageManager::ALL {
            assert!(m.install_args().contains(&"--ignore-scripts"), "{m:?}");
        }
    }

    // -- ensure_installed ---------------------------------------------------

    #[derive(Default)]
    struct Spy {
        calls: RefCell<Vec<(PathBuf, Vec<String>, PathBuf)>>,
        yarn_version: &'static str,
        fail: bool,
    }

    impl Runner for Spy {
        fn run(&self, program: &Path, args: &[&str], dir: &Path) -> Result<(bool, String), String> {
            if args == ["--version"] {
                return Ok((true, self.yarn_version.to_string()));
            }
            self.calls.borrow_mut().push((
                program.to_path_buf(),
                args.iter().map(|a| a.to_string()).collect(),
                dir.to_path_buf(),
            ));
            if self.fail {
                Ok((false, "ERR network unreachable\n".to_string()))
            } else {
                Ok((true, String::new()))
            }
        }
    }

    fn build_dir(name: &str, listed: &[&str], installed: &[&str]) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("bynk-npm-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let deps: Vec<String> = listed
            .iter()
            .map(|n| format!("\"{n}\": \"1.0.0\""))
            .collect();
        std::fs::write(
            d.join("package.json"),
            format!("{{\"dependencies\": {{{}}}}}", deps.join(", ")),
        )
        .unwrap();
        for n in installed {
            let p = d.join("node_modules").join(n);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("package.json"), "{}").unwrap();
        }
        d
    }

    fn with_pnpm() -> Fake {
        let mut f = Fake::default();
        f.files.insert("pnpm-lock.yaml", String::new());
        f.path.insert("pnpm");
        f
    }

    fn run_mode(fake: &Fake, spy: &Spy, dir: &Path, mode: Mode) -> Result<Outcome, InstallError> {
        let install = Install {
            project: fake,
            setting: None,
            toolbox: fake,
            runner: spy,
        };
        ensure_installed(&install, dir, mode)
    }

    #[test]
    fn a_missing_package_runs_the_detected_tool_in_the_build_directory() {
        let d = build_dir("run", &["postal-mime"], &[]);
        let spy = Spy::default();
        let got = run_mode(&with_pnpm(), &spy, &d, Mode::Install).unwrap();
        assert!(matches!(got, Outcome::Installed(ref det) if det.manager == PackageManager::Pnpm));
        let calls = spy.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, PathBuf::from("/bin/pnpm"));
        assert_eq!(
            calls[0].1,
            ["install", "--ignore-scripts", "--ignore-workspace"]
        );
        assert_eq!(calls[0].2, d);
    }

    #[test]
    fn a_warm_directory_runs_nothing_and_needs_no_tool() {
        let d = build_dir("warm", &["postal-mime", "jose"], &["postal-mime", "jose"]);
        let spy = Spy::default();
        // Not even a detectable manager: nothing is looked up.
        let got = run_mode(&Fake::default(), &spy, &d, Mode::Install).unwrap();
        assert_eq!(got, Outcome::Present);
        assert!(spy.calls.borrow().is_empty());
        let none = build_dir("none", &[], &[]);
        assert_eq!(
            run_mode(&Fake::default(), &spy, &none, Mode::Install).unwrap(),
            Outcome::Present
        );
    }

    #[test]
    fn a_partly_installed_directory_installs_again() {
        let d = build_dir("partial", &["postal-mime", "jose"], &["jose"]);
        let spy = Spy::default();
        run_mode(&with_pnpm(), &spy, &d, Mode::Install).unwrap();
        assert_eq!(spy.calls.borrow().len(), 1);
    }

    #[test]
    fn no_install_stops_naming_the_detected_command_and_directory() {
        let d = build_dir("declined", &["postal-mime"], &[]);
        let spy = Spy::default();
        let e = run_mode(&with_pnpm(), &spy, &d, Mode::NoInstall).unwrap_err();
        let text = e.to_string();
        assert!(
            text.contains("postal-mime")
                && text.contains("Run `pnpm install --ignore-scripts --ignore-workspace`")
                && text.contains(&d.display().to_string())
                && !text.contains("`npm install"),
            "{text}"
        );
        assert!(spy.calls.borrow().is_empty());
    }

    #[test]
    fn a_dry_run_reports_what_it_would_install_and_runs_nothing() {
        let d = build_dir("dry", &["postal-mime"], &[]);
        let spy = Spy::default();
        let got = run_mode(&with_pnpm(), &spy, &d, Mode::DryRun).unwrap();
        let Outcome::WouldInstall(missing, det) = got else {
            panic!("{got:?}")
        };
        assert_eq!(missing, ["postal-mime"]);
        assert_eq!(det.manager, PackageManager::Pnpm);
        let msg = dry_run_message(&missing, &det, &d);
        assert!(
            msg.contains("pnpm install") && msg.contains("pnpm-lock.yaml"),
            "{msg}"
        );
        assert!(spy.calls.borrow().is_empty());
        // A dry run outranks `--no-install`.
        assert_eq!(Mode::of(true, true), Mode::DryRun);
        assert_eq!(Mode::of(false, true), Mode::NoInstall);
        assert_eq!(Mode::of(false, false), Mode::Install);
    }

    #[test]
    fn a_failed_install_names_the_command_the_directory_and_the_tools_error() {
        let d = build_dir("failed", &["postal-mime"], &[]);
        let spy = Spy {
            fail: true,
            ..Spy::default()
        };
        let text = run_mode(&with_pnpm(), &spy, &d, Mode::Install)
            .unwrap_err()
            .to_string();
        assert!(
            text.contains("pnpm install")
                && text.contains(&d.display().to_string())
                && text.contains("network unreachable"),
            "{text}"
        );
    }

    #[test]
    fn a_chosen_tool_that_is_not_installed_says_why_it_was_chosen() {
        let d = build_dir("toolless", &["postal-mime"], &[]);
        let mut f = Fake::default();
        f.files.insert("bun.lock", String::new());
        let text = run_mode(&f, &Spy::default(), &d, Mode::Install)
            .unwrap_err()
            .to_string();
        assert!(
            text.contains("`bun`") && text.contains("bun.lock"),
            "{text}"
        );
    }

    #[test]
    fn the_npm_fallback_installs_with_npm_when_nothing_else_points_anywhere() {
        let d = build_dir("npm", &["postal-mime"], &[]);
        let mut f = Fake::default();
        f.path.insert("npm");
        let spy = Spy::default();
        run_mode(&f, &spy, &d, Mode::Install).unwrap();
        assert_eq!(
            spy.calls.borrow()[0].1,
            ["install", "--no-audit", "--no-fund", "--ignore-scripts"]
        );
    }

    #[test]
    fn yarn_classic_installs_and_berry_is_refused() {
        let mut f = Fake::default();
        f.files.insert("yarn.lock", String::new());
        f.path.insert("yarn");
        let d = build_dir("yarn", &["postal-mime"], &[]);
        let classic = Spy {
            yarn_version: "1.22.22\n",
            ..Spy::default()
        };
        run_mode(&f, &classic, &d, Mode::Install).unwrap();
        assert_eq!(classic.calls.borrow().len(), 1);
        let berry = Spy {
            yarn_version: "4.5.0\n",
            ..Spy::default()
        };
        let e = run_mode(&f, &berry, &d, Mode::Install).unwrap_err();
        assert!(matches!(e, InstallError::YarnBerry { .. }), "{e}");
        assert!(berry.calls.borrow().is_empty());
        assert!(e.to_string().contains("Berry"));
    }

    // -- real tools ----------------------------------------------------------

    /// Install `postal-mime` with the real `tool` into a directory below a
    /// `pnpm-workspace.yaml`, then run again. Skips when the tool is absent.
    fn real_install(tool: &str) {
        let Ok(path) = which::which(tool) else {
            eprintln!("skipping: `{tool}` is not installed");
            return;
        };
        let up = std::env::temp_dir().join(format!("bynk-npm-real-{tool}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&up);
        // A workspace file above the generated directory: `pnpm install` would
        // adopt the directory as a member without `--ignore-workspace`.
        std::fs::create_dir_all(&up).unwrap();
        std::fs::write(
            up.join("pnpm-workspace.yaml"),
            "packages:\n  - 'elsewhere'\n",
        )
        .unwrap();
        let dir = up.join(".bynk").join("deploy");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"type":"module","dependencies":{"postal-mime":"4.1.4"}}"#,
        )
        .unwrap();
        let mut fake = Fake::default();
        fake.path.insert(match tool {
            "npm" => "npm",
            "pnpm" => "pnpm",
            "bun" => "bun",
            _ => "yarn",
        });
        // The fake toolbox resolves the name to `/bin/<tool>`; the real runner
        // needs the real path.
        struct At(PathBuf);
        impl Runner for At {
            fn run(&self, _: &Path, args: &[&str], dir: &Path) -> Result<(bool, String), String> {
                SystemRunner.run(&self.0, args, dir)
            }
        }
        let runner = At(path);
        let install = Install {
            project: &fake,
            setting: Some(tool),
            toolbox: &fake,
            runner: &runner,
        };
        let first = ensure_installed(&install, &dir, Mode::Install);
        assert!(
            matches!(first, Ok(Outcome::Installed(_))),
            "{tool}: {first:?}"
        );
        assert!(
            dir.join("node_modules/postal-mime/package.json").is_file(),
            "{tool}: installed into the generated directory"
        );
        assert!(
            !up.join("node_modules").exists(),
            "{tool}: nothing was installed at the workspace root"
        );
        // Lifecycle scripts stay off, and a warm re-run runs nothing.
        assert_eq!(
            ensure_installed(&install, &dir, Mode::Install),
            Ok(Outcome::Present),
            "{tool}"
        );
        let _ = std::fs::remove_dir_all(&up);
    }

    #[test]
    fn npm_installs_for_real() {
        real_install("npm");
    }

    #[test]
    fn pnpm_installs_for_real_beside_a_workspace_file() {
        real_install("pnpm");
    }

    #[test]
    fn bun_installs_for_real() {
        real_install("bun");
    }

    #[test]
    fn yarn_classic_installs_for_real() {
        let classic = which::which("yarn").ok().is_some_and(|y| {
            Command::new(y)
                .arg("--version")
                .output()
                .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).starts_with("1."))
        });
        if classic {
            real_install("yarn");
        } else {
            eprintln!("skipping: Yarn classic is not installed");
        }
    }
}
