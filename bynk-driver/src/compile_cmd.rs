//! The `compile` command body, run by `bynk compile`
//! (D2/E1, #2998): before this, only `bynk` had it, so `bynk compile` had to
//! start a `bynk` process. Parameterised by the program name that prefixes
//! messages, like [`crate::run_check`] and [`crate::run_fmt`].

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::ValueEnum;

use crate::project::{BuildTarget, CompileOptions, SchemaLock, compile_project};
use bynk_check::firstparty::Platform;

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
    } = args;
    if !input.is_dir() {
        eprintln!(
            "{prog}: `build` takes a project directory; use `{prog} compile` for a single file"
        );
        return ExitCode::FAILURE;
    }
    let overlay = match std::fs::read_to_string(input.join("bynk.toml")) {
        Ok(text) => std::collections::HashMap::from([(input.join("bynk.toml"), text)]),
        Err(_) => std::collections::HashMap::new(),
    };
    let config = match bynk_project::try_read_build_config_with(&input, &overlay) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{prog}: {e}");
            return ExitCode::FAILURE;
        }
    };
    // `build_config_from` has already checked each value against the set.
    fn from_config(value: &Option<String>) -> Option<&str> {
        value.as_deref()
    }
    let target = target
        .or_else(|| from_config(&config.target).and_then(|v| CliTarget::from_str(v, true).ok()))
        .unwrap_or(CliTarget::Bundle);
    let platform = platform
        .or_else(|| from_config(&config.platform).and_then(|v| CliPlatform::from_str(v, true).ok()))
        .unwrap_or_default();
    let emit = emit
        .or_else(|| from_config(&config.emit).and_then(|v| EmitFormat::from_str(v, true).ok()))
        .unwrap_or_default();
    let output = out.unwrap_or_else(|| input.join(config.out.as_deref().unwrap_or("out")));
    let code = run_compile(
        prog,
        CompileArgs {
            input,
            output: output.clone(),
            target,
            platform,
            emit,
            sources_stdin: false,
        },
    );
    if code == ExitCode::SUCCESS {
        let target = match target {
            CliTarget::Bundle => "bundle",
            CliTarget::Workers => "workers",
        };
        eprintln!("{prog}: built {target} -> {}", output.display());
    }
    code
}

/// Run `compile`. `prog` prefixes the messages (`"bynk"`).
pub fn run_compile(prog: &str, args: CompileArgs) -> ExitCode {
    let CompileArgs {
        input,
        output,
        target,
        platform,
        emit,
        sources_stdin,
    } = args;
    if sources_stdin && !input.is_dir() {
        eprintln!("{prog}: --sources-stdin needs a project directory, not a single file");
        return ExitCode::FAILURE;
    }
    if input.is_dir() {
        compile_dir(prog, &input, &output, target, platform, emit, sources_stdin)
    } else {
        compile_file(prog, &input, &output, emit)
    }
}

fn compile_dir(
    prog: &str,
    input: &Path,
    output: &Path,
    target: CliTarget,
    platform: CliPlatform,
    emit: EmitFormat,
    sources_stdin: bool,
) -> ExitCode {
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
