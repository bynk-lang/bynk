//! The `compile` command body, run by `bynk compile`
//! (D2/E1, #2998): before this, only `bynk` had it, so `bynk compile` had to
//! start a `bynk` process. Parameterised by the program name that prefixes
//! messages, like [`crate::run_check`] and [`crate::run_fmt`].

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::ValueEnum;

use crate::project::{BuildTarget, CompileOptions, SchemaLock, compile_project};
use bynk_check::firstparty::Platform;

/// `--profile`: a named build profile (#3054). It chooses the defaults a
/// `[profile.<name>]` table can override; today that is `contracts` only.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CliProfile {
    /// The development build: the function-contract call-site guard is on.
    Dev,
    /// The deployable build (the default): no contract guard, the same bytes
    /// `bynk compile` writes.
    Release,
}

impl CliProfile {
    fn name(self) -> &'static str {
        match self {
            CliProfile::Dev => "dev",
            CliProfile::Release => "release",
        }
    }

    /// Whether the profile carries the contract guard when the manifest says
    /// nothing.
    fn contracts_by_default(self) -> bool {
        self == CliProfile::Dev
    }
}

/// #3107: whether a build under `profile` carries the contract guard, the one
/// rule every verb that builds shares: `[profile.<name>] contracts`, else the
/// profile's default (`dev` on, `release` off). `bynk build` also takes a
/// profile from its flag; `test` is always `dev`, `deploy` always `release`.
pub fn profile_contracts(
    profiles: &std::collections::BTreeMap<String, bynk_project::ProfileConfig>,
    profile: CliProfile,
) -> bool {
    profiles
        .get(profile.name())
        .and_then(|p| p.contracts)
        .unwrap_or_else(|| profile.contracts_by_default())
}

/// [`profile_contracts`] for the project at `root`, reading its `bynk.toml`.
pub fn project_contracts(root: &Path, profile: CliProfile) -> Result<bool, String> {
    // Only `[profile]`: `test` and `deploy` do not read `[build]`.
    let profiles = bynk_project::try_read_profiles(root).map_err(|e| e.to_string())?;
    Ok(profile_contracts(&profiles, profile))
}

/// #3107: `deploy` always builds `release`, and a deployment never carries
/// contract checks (DECISION J of v0.115), so `[profile.release] contracts =
/// true` is refused for it, naming the table.
pub fn refuse_release_contracts(root: &Path) -> Result<(), String> {
    if project_contracts(root, CliProfile::Release)? {
        return Err("`[profile.release] contracts = true` in `bynk.toml` would put contract checks in what is deployed, and a deployment never carries them — set it to `false` or remove it (it still applies to `bynk build`)".to_string());
    }
    Ok(())
}

/// `--target`: the build topology.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CliTarget {
    /// Single-bundle output (the default). Cross-context calls compile to
    /// direct function invocation.
    Bundle,
    /// One Cloudflare Worker per context. Cross-context calls go over
    /// Service Bindings using a JSON wire format.
    Workers,
}

impl From<CliTarget> for BuildTarget {
    fn from(t: CliTarget) -> Self {
        match t {
            CliTarget::Bundle => BuildTarget::Bundle,
            CliTarget::Workers => BuildTarget::Workers,
        }
    }
}

/// v0.108 (in-browser track, slice 1): the emitted artefact language. `ts` (the
/// default and primary output) writes the typed TypeScript modules; `js` writes
/// the same modules with their types stripped — an *emit-then-strip* JavaScript
/// artefact (ADR 0137) runnable with no `tsc` in the loop. Orthogonal to
/// `--target` (topology) and `--platform` (binding).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, ValueEnum)]
pub enum EmitFormat {
    /// TypeScript modules (the default, primary artefact).
    #[default]
    Ts,
    /// JavaScript modules, types stripped (no `tsc` dependency).
    Js,
}

/// v0.17: the deploy platform that selects the `bynk` surface binding. Distinct
/// from [`CliTarget`] (the emit topology). v0.18 adds `node`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, ValueEnum)]
pub enum CliPlatform {
    /// Cloudflare Workers runtime (the default).
    #[default]
    Cloudflare,
    /// Node.js (≥ 22) runtime (v0.18).
    Node,
    /// The browser — the `bynk` surface over Web APIs, for the in-browser
    /// REPL/playground (v0.108). `Bundle` topology only; `Fetch`/`Secrets` are
    /// withheld (see ADR 0138).
    Browser,
}

impl From<CliPlatform> for Platform {
    fn from(p: CliPlatform) -> Self {
        match p {
            CliPlatform::Cloudflare => Platform::Cloudflare,
            CliPlatform::Node => Platform::Node,
            CliPlatform::Browser => Platform::Browser,
        }
    }
}

/// What a `compile` run was asked to do.
#[derive(Clone, Debug)]
pub struct CompileArgs {
    /// A `.bynk` file or a project directory.
    pub input: PathBuf,
    /// Output file (single-file input) or output root (project input).
    pub output: PathBuf,
    pub target: CliTarget,
    pub platform: CliPlatform,
    pub emit: EmitFormat,
    /// #2471: layer the JSON document on stdin over the project's sources.
    pub sources_stdin: bool,
    /// #3054: build with the function-contract call-site guard. `bynk compile`
    /// passes `false`; only `bynk build --profile dev` (or a `[profile]` table)
    /// turns it on.
    pub contracts: bool,
}

/// What a `build` run was asked to do: the flags given, over `[build]`.
#[derive(Clone, Debug, Default)]
pub struct BuildArgs {
    /// A project directory.
    pub input: PathBuf,
    pub out: Option<PathBuf>,
    pub target: Option<CliTarget>,
    pub platform: Option<CliPlatform>,
    pub emit: Option<EmitFormat>,
    /// The profile; `release` when unset.
    pub profile: Option<CliProfile>,
}

/// Run `build` (#3054): resolve each setting as flag, else `[build]`, else the
/// built-in default, then run [`run_compile`]. `compile` itself never reads
/// `[build]`.
pub fn run_build(prog: &str, args: BuildArgs) -> ExitCode {
    let BuildArgs {
        input,
        out,
        target,
        platform,
        emit,
        profile,
    } = args;
    if !input.is_dir() {
        eprintln!(
            "{prog}: `build` takes a project directory; use `{prog} compile` for a single file"
        );
        return ExitCode::FAILURE;
    }
    let config = match bynk_project::try_read_build_config(&input) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{prog}: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The manifest's strings were checked against the same sets in
    // `bynk-project`; a name that no longer parses is a bug there, never a
    // silent fall back to the default.
    fn parse<T: ValueEnum>(key: &str, value: Option<&str>) -> Result<Option<T>, String> {
        match value {
            None => Ok(None),
            Some(v) => T::from_str(v, true).map(Some).map_err(|_| {
                format!("`[build]` `{key}` = \"{v}\" is not a value this `bynk` knows")
            }),
        }
    }
    let resolved = (|| -> Result<_, String> {
        Ok((
            target
                .or(parse("target", config.target.as_deref())?)
                .unwrap_or(CliTarget::Bundle),
            platform
                .or(parse("platform", config.platform.as_deref())?)
                .unwrap_or_default(),
            emit.or(parse("emit", config.emit.as_deref())?)
                .unwrap_or_default(),
        ))
    })();
    let (target, platform, emit) = match resolved {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{prog}: {e}");
            return ExitCode::FAILURE;
        }
    };
    // CLI flag > `[profile.<name>]` > `[build]` > built-in default. `contracts`
    // has no flag and no `[build]` key, so it is the profile's table, else the
    // profile's default.
    let profile = profile.unwrap_or(CliProfile::Release);
    let contracts = profile_contracts(&config.profiles, profile);
    let output = out.unwrap_or_else(|| input.join(config.out.as_deref().unwrap_or("out")));
    if let Err(e) = check_output_dir(&input, &output) {
        eprintln!("{prog}: {e}");
        return ExitCode::FAILURE;
    }
    let code = run_compile_reporting(
        prog,
        CompileArgs {
            input,
            output: output.clone(),
            target,
            platform,
            emit,
            sources_stdin: false,
            contracts,
        },
        crate::progress::terminal_sink(),
    );
    if code == ExitCode::SUCCESS {
        let target = match target {
            CliTarget::Bundle => "bundle",
            CliTarget::Workers => "workers",
        };
        let profile = match (profile, contracts) {
            (CliProfile::Release, false) => String::new(),
            (p, c) => format!(
                " ({} profile, contracts {})",
                p.name(),
                if c { "on" } else { "off" }
            ),
        };
        eprintln!("{prog}: built {target}{profile} -> {}", output.display());
    }
    code
}

/// `path` with symlinks resolved as far as it exists, the rest appended as
/// written, so a not-yet-created output directory can still be placed.
fn resolve(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut cur = path.to_path_buf();
    loop {
        if let Ok(real) = cur.canonicalize() {
            return rest.iter().rev().fold(real, |acc, c| acc.join(c));
        }
        match (cur.file_name().map(ToOwned::to_owned), cur.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                cur = if parent.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    parent.to_path_buf()
                };
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Whether any `.bynk` file sits under `dir` (build-output caches skipped).
fn holds_bynk_source(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let path = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        match e.file_type() {
            Ok(t) if t.is_dir() => {
                name != "node_modules" && !name.starts_with('.') && holds_bynk_source(&path)
            }
            Ok(t) if t.is_file() => path.extension().is_some_and(|x| x == "bynk"),
            _ => false,
        }
    })
}

/// #3054 review: the writer prunes `.ts`/`.js`/`.json`/… files in the output
/// directory that the build did not produce, so an output directory that is
/// the project root, a source root, holds or sits inside one, escapes the
/// project (also through a symlink), or already holds `.bynk` source would
/// delete hand-written files on every build. Refuse those.
fn check_output_dir(project: &Path, output: &Path) -> Result<(), String> {
    let root = resolve(project);
    // `output` is relative to the cwd (a flag) or already joined onto
    // `project` (the manifest key); `canonicalize` resolves both from the cwd.
    let out = resolve(output);
    let shown = output.display();
    if !out.starts_with(&root) || out == root {
        return Err(format!(
            "the output directory `{shown}` must be a directory inside the project, not the project root or a path outside it"
        ));
    }
    let paths = bynk_project::try_read_project_paths(project)
        .unwrap_or_else(|_| bynk_project::ProjectPaths::conventional(project));
    for include in &paths.include {
        let source = resolve(&project.join(include));
        // A flat project's root is the source root; the output lives inside
        // it by design (`out/` is skipped by discovery).
        if source == root {
            continue;
        }
        if out.starts_with(&source) || source.starts_with(&out) {
            return Err(format!(
                "the output directory `{shown}` overlaps the source root `{}`, and a build deletes files it did not write from its output directory; choose another `[build] out`",
                include.display()
            ));
        }
    }
    if holds_bynk_source(&out) {
        return Err(format!(
            "the output directory `{shown}` holds `.bynk` sources, and a build deletes files it did not write from its output directory; choose another `[build] out`"
        ));
    }
    Ok(())
}

/// Run `compile`. `prog` prefixes the messages (`"bynk"`).
pub fn run_compile(prog: &str, args: CompileArgs) -> ExitCode {
    run_compile_reporting(prog, args, None)
}

/// [`run_compile`], reporting each context to `progress` as it compiles (#3100).
fn run_compile_reporting(
    prog: &str,
    args: CompileArgs,
    progress: Option<crate::project::ProgressSink>,
) -> ExitCode {
    let CompileArgs {
        input,
        output,
        target,
        platform,
        emit,
        sources_stdin,
        contracts,
    } = args;
    if sources_stdin && !input.is_dir() {
        eprintln!("{prog}: --sources-stdin needs a project directory, not a single file");
        return ExitCode::FAILURE;
    }
    if input.is_dir() {
        compile_dir(
            prog,
            &input,
            &output,
            DirSettings {
                target,
                platform,
                emit,
                sources_stdin,
                contracts,
                progress,
            },
        )
    } else {
        compile_file(prog, &input, &output, emit)
    }
}

/// What a project (directory) compile is configured with.
struct DirSettings {
    target: CliTarget,
    platform: CliPlatform,
    emit: EmitFormat,
    sources_stdin: bool,
    contracts: bool,
    progress: Option<crate::project::ProgressSink>,
}

fn compile_dir(prog: &str, input: &Path, output: &Path, settings: DirSettings) -> ExitCode {
    let DirSettings {
        target,
        platform,
        emit,
        sources_stdin,
        contracts,
        progress,
    } = settings;
    // #1078: `bynk-emit` touches no disk for `bynk.schema.lock` — read
    // its current content here (verified-absent `None` for a fresh
    // project) and hand it in; write the reconciled content back after
    // a clean compile, below.
    let schema_lock = match crate::schema_lock::read(input) {
        // #2471: a preview build reconciles nothing and writes nothing.
        Ok(_) if sources_stdin => SchemaLock::Off,
        Ok(existing) => SchemaLock::On { existing },
        Err(e) => {
            eprintln!(
                "{prog}: could not read {}: {e}",
                crate::schema_lock::lock_path(input).display()
            );
            return ExitCode::FAILURE;
        }
    };
    let options: CompileOptions = match crate::try_project_options(input) {
        // Events track, slice 3c (#980): a real directory build reconciles
        // and writes `bynk.schema.lock` — the Cargo.lock model. Every
        // other `compile_project` caller (in-memory builds, `bynk/
        // tests/e2e.rs`'s in-place fixtures, the LSP) leaves this off.
        // #1821 (ADR 0147 D3): a build strips every `suite`; only
        // `bynk test` compiles them.
        Ok(o) => o
            .target(target.into())
            .platform(platform.into())
            .schema_registry(schema_lock)
            .contracts(contracts)
            .progress(progress)
            .tests(false),
        Err(e) => {
            eprintln!("{prog}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let options = if sources_stdin {
        match overlay_from_stdin(options, input) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("{prog}: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        options
    };
    // Multi-file project compile.
    match compile_project(&options) {
        Ok(out) => {
            // A write failure here is reported but does not fail the
            // build — the same non-fatal handling this used to get
            // (`schema_registry::write`'s own eprintln) when the write
            // lived inside `compile_project` itself.
            if let Some(content) = &out.schema_lock
                && let Err(e) = crate::schema_lock::write(input, content)
            {
                eprintln!(
                    "{prog}: could not write {}: {e}",
                    crate::schema_lock::lock_path(input).display()
                );
            }
            // `--emit js` (the in-browser track, slice 1, ADR 0137): the
            // emitter always produces TypeScript; a JS artefact is that same
            // output with types stripped (the emitter is strip-only — ADR
            // 0136 — so the strip is total). Rewrite the file set before the
            // shared writer touches disk.
            let out = match emit {
                EmitFormat::Ts => out,
                EmitFormat::Js => match crate::strip::strip_project_to_js(out) {
                    Ok(out) => out,
                    Err(e) => {
                        eprintln!("{prog}: could not produce JavaScript output: {e}");
                        return ExitCode::FAILURE;
                    }
                },
            };
            match crate::write_output(&out, output) {
                Ok(()) => {
                    // ADR 0117: surface non-failing warnings; the build succeeds.
                    crate::print_project_warnings(&out.warnings, &out.snapshots, &out.display_root);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!(
                        "{prog}: could not write output under `{}`: {e}",
                        output.display()
                    );
                    ExitCode::FAILURE
                }
            }
        }
        Err(failure) => {
            crate::print_project_failure(&failure);
            ExitCode::FAILURE
        }
    }
}

fn compile_file(prog: &str, input: &Path, output: &Path, emit: EmitFormat) -> ExitCode {
    let source = match std::fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{prog}: could not read `{}`: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let filename = input.display().to_string();
    match bynk_emit::compile_with_warnings(&source, &filename) {
        Ok(compiled) => {
            // For `--emit js` the single emitted module is stripped to JS; the
            // caller names the output path (e.g. `-o out.js`).
            let body = match emit {
                EmitFormat::Ts => compiled.ts,
                EmitFormat::Js => match bynk_strip::strip_types(&compiled.ts, &filename) {
                    Ok(js) => js,
                    Err(e) => {
                        eprintln!("{prog}: could not produce JavaScript output: {e}");
                        return ExitCode::FAILURE;
                    }
                },
            };
            if let Some(parent) = output.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(output, body) {
                eprintln!("{prog}: could not write `{}`: {e}", output.display());
                return ExitCode::FAILURE;
            }
            // ADR 0117: render non-failing warnings with source context.
            // #2838: a hint is for the editor, not the terminal.
            let warnings = crate::render::without_hints(&compiled.warnings);
            if !warnings.is_empty() {
                crate::render::print_errors(&warnings, &source, &filename);
            }
            ExitCode::SUCCESS
        }
        Err(errors) => {
            crate::render::print_errors(&errors, &source, &filename);
            ExitCode::FAILURE
        }
    }
}

/// #2471: layer the `--sources-stdin` document over `options`' sources.
fn overlay_from_stdin(
    options: CompileOptions,
    root: &Path,
) -> Result<CompileOptions, Box<dyn std::error::Error>> {
    use std::io::Read as _;
    let mut json = String::new();
    std::io::stdin().read_to_string(&mut json)?;
    let overlay = crate::source_overlay::parse_overlay(&json)?;
    Ok(crate::source_overlay::apply_overlay(
        options, root, overlay,
    )?)
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    use bynk_project::ProfileConfig;

    type Profiles = std::collections::BTreeMap<String, ProfileConfig>;

    fn with(dev: Option<bool>, release: Option<bool>) -> Profiles {
        let mut c = Profiles::new();
        for (name, v) in [("dev", dev), ("release", release)] {
            if v.is_some() {
                c.insert(name.to_string(), ProfileConfig { contracts: v });
            }
        }
        c
    }

    #[test]
    fn a_profile_defaults_then_its_table_overrides() {
        let none = with(None, None);
        assert!(profile_contracts(&none, CliProfile::Dev));
        assert!(!profile_contracts(&none, CliProfile::Release));
        let set = with(Some(false), Some(true));
        assert!(!profile_contracts(&set, CliProfile::Dev));
        assert!(profile_contracts(&set, CliProfile::Release));
    }

    #[test]
    fn one_profiles_table_does_not_touch_the_other() {
        let only_dev = with(Some(false), None);
        assert!(!profile_contracts(&only_dev, CliProfile::Dev));
        assert!(!profile_contracts(&only_dev, CliProfile::Release));
    }
}
