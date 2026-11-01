//! bynkc — the Bynk v0.3 compiler CLI.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bynkc::BuildTarget;
use bynkc::cli::{Cli, Command, DiagFormat, EmitFormat};
use clap::Parser;

/// Root a directory project the way every project command should (#46): a
/// `bynk.toml` or a `src/` subdir selects **project** mode, whose flat
/// `[paths] include`/`exclude` layout (v0.113, DECISION S) defaults to the
/// conventional roots that exist (`src`, `tests`) or the project root itself;
/// otherwise the legacy **single-tree** where `<dir>` is itself the root.
/// `check`, `compile`, and `test` all route through this so the conventional
/// layout works the same from any of them. Test-ness is structural — a `suite`
/// is discovered wherever it sits and stripped from the production build — so
/// tests need no dedicated directory.
///
/// A malformed `bynk.toml` fails loudly here rather than silently falling
/// back to the conventional layout.
fn try_project_options(input: &Path) -> Result<bynkc::CompileOptions, bynkc::ProjectOptionsError> {
    bynk_driver::try_project_options(input)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Compile {
            input,
            output,
            target,
            platform,
            emit,
            sources_stdin,
        } => run_compile(
            input,
            output,
            target.into(),
            platform.into(),
            emit,
            sources_stdin,
        ),
        Command::Check { input, format, fix } => run_check(input, format, fix),
        Command::Fmt { args } => run_fmt(args),
        Command::Test { args } => run_test(args),
    }
}

fn run_test(args: bynkc::cli::TestArgs) -> ExitCode {
    // #521/Wave 5 §5.4: the command body is shared with the `bynk` driver.
    bynk_driver::test_runner::run_test("bynkc", args)
}

fn run_fmt(args: bynkc::cli::FmtArgs) -> ExitCode {
    // #521: the command body is shared with the `bynk` driver.
    bynk_driver::run_fmt("bynkc", &args)
}

fn run_compile(
    input: PathBuf,
    output: PathBuf,
    target: BuildTarget,
    platform: bynkc::Platform,
    emit: EmitFormat,
    sources_stdin: bool,
) -> ExitCode {
    if sources_stdin && !input.is_dir() {
        eprintln!("bynkc: --sources-stdin needs a project directory, not a single file");
        return ExitCode::FAILURE;
    }
    if input.is_dir() {
        // #1078: `bynk-emit` touches no disk for `bynk.schema.lock` — read
        // its current content here (verified-absent `None` for a fresh
        // project) and hand it in; write the reconciled content back after
        // a clean compile, below.
        let schema_lock = match bynk_driver::schema_lock::read(&input) {
            // #2471: a preview build reconciles nothing and writes nothing.
            Ok(_) if sources_stdin => bynkc::SchemaLock::Off,
            Ok(existing) => bynkc::SchemaLock::On { existing },
            Err(e) => {
                eprintln!(
                    "bynkc: could not read {}: {e}",
                    bynk_driver::schema_lock::lock_path(&input).display()
                );
                return ExitCode::FAILURE;
            }
        };
        let options = match try_project_options(&input) {
            // Events track, slice 3c (#980): a real directory build reconciles
            // and writes `bynk.schema.lock` — the Cargo.lock model. Every
            // other `compile_project` caller (in-memory builds, `bynkc/
            // tests/e2e.rs`'s in-place fixtures, the LSP) leaves this off.
            // #1821 (ADR 0147 D3): a build strips every `suite`; only
            // `bynkc test` compiles them.
            Ok(o) => o
                .target(target)
                .platform(platform)
                .schema_registry(schema_lock)
                .tests(false),
            Err(e) => {
                eprintln!("bynkc: {e}");
                return ExitCode::FAILURE;
            }
        };
        let options = if sources_stdin {
            match overlay_from_stdin(options, &input) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("bynkc: {e}");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            options
        };
        // Multi-file project compile.
        match bynkc::compile_project(&options) {
            Ok(out) => {
                // A write failure here is reported but does not fail the
                // build — the same non-fatal handling this used to get
                // (`schema_registry::write`'s own eprintln) when the write
                // lived inside `compile_project` itself.
                if let Some(content) = &out.schema_lock
                    && let Err(e) = bynk_driver::schema_lock::write(&input, content)
                {
                    eprintln!(
                        "bynkc: could not write {}: {e}",
                        bynk_driver::schema_lock::lock_path(&input).display()
                    );
                }
                // `--emit js` (the in-browser track, slice 1, ADR 0137): the
                // emitter always produces TypeScript; a JS artefact is that same
                // output with types stripped (the emitter is strip-only — ADR
                // 0136 — so the strip is total). Rewrite the file set before the
                // shared writer touches disk.
                let out = match emit {
                    EmitFormat::Ts => out,
                    EmitFormat::Js => match bynkc::strip_project_to_js(out) {
                        Ok(out) => out,
                        Err(e) => {
                            eprintln!("bynkc: could not produce JavaScript output: {e}");
                            return ExitCode::FAILURE;
                        }
                    },
                };
                match bynkc::write_output(&out, &output) {
                    Ok(()) => {
                        // ADR 0117: surface non-failing warnings; the build succeeds.
                        bynkc::print_project_warnings(
                            &out.warnings,
                            &out.snapshots,
                            &out.display_root,
                        );
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!(
                            "bynkc: could not write output under `{}`: {e}",
                            output.display()
                        );
                        ExitCode::FAILURE
                    }
                }
            }
            Err(failure) => {
                bynkc::print_project_failure(&failure);
                ExitCode::FAILURE
            }
        }
    } else {
        let source = match std::fs::read_to_string(&input) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("bynkc: could not read `{}`: {e}", input.display());
                return ExitCode::FAILURE;
            }
        };
        let filename = input.display().to_string();
        match bynkc::compile_with_warnings(&source, &filename) {
            Ok(compiled) => {
                // For `--emit js` the single emitted module is stripped to JS; the
                // caller names the output path (e.g. `-o out.js`).
                let body = match emit {
                    EmitFormat::Ts => compiled.ts,
                    EmitFormat::Js => match bynkc::strip_types(&compiled.ts, &filename) {
                        Ok(js) => js,
                        Err(e) => {
                            eprintln!("bynkc: could not produce JavaScript output: {e}");
                            return ExitCode::FAILURE;
                        }
                    },
                };
                if let Some(parent) = output.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&output, body) {
                    eprintln!("bynkc: could not write `{}`: {e}", output.display());
                    return ExitCode::FAILURE;
                }
                // ADR 0117: render non-failing warnings with source context.
                if !compiled.warnings.is_empty() {
                    bynkc::print_errors(&compiled.warnings, &source, &filename);
                }
                ExitCode::SUCCESS
            }
            Err(errors) => {
                bynkc::print_errors(&errors, &source, &filename);
                ExitCode::FAILURE
            }
        }
    }
}

/// #2471: layer the `--sources-stdin` document over `options`' sources.
fn overlay_from_stdin(
    options: bynkc::CompileOptions,
    root: &Path,
) -> Result<bynkc::CompileOptions, Box<dyn std::error::Error>> {
    use std::io::Read as _;
    let mut json = String::new();
    std::io::stdin().read_to_string(&mut json)?;
    let overlay = bynk_driver::source_overlay::parse_overlay(&json)?;
    Ok(bynk_driver::source_overlay::apply_overlay(
        options, root, overlay,
    )?)
}

fn run_check(input: PathBuf, format: DiagFormat, fix: bool) -> ExitCode {
    // #521: the command body is shared with the `bynk` driver.
    bynk_driver::run_check("bynkc", &input, format == DiagFormat::Short, fix)
}
