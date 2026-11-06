//! `bynkc`, as a name the `bynk` executable answers to (#3013, track E3).
//!
//! `bynkc` was a second binary; it is now `bynk` run under another name, for one
//! release. When the executable's file stem is `bynkc` (a copy in the release
//! archive, the loose `bynkc-<target>` asset, or the final `bynkc` crate's shim),
//! `bynk` parses `bynkc`'s four subcommands with their current flags, runs the
//! same driver command bodies with `prog = "bynkc"` (so every message reads as it
//! did), and prints one retirement notice on stderr. The alias, the shim crate
//! and the asset go in the release after (E3-4).
//!
//! The clap types live here (rather than in `main.rs`) so they are the single
//! source of truth for both the alias and the generated CLI reference page
//! `site/src/content/docs/docs/cli.md`. [`render_markdown`] walks the clap
//! command tree; the test `xtask/tests/docs_consistency/cli_reference.rs` checks
//! the page is up to date.

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(name = "bynkc", version, about = "The Bynk compiler", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// v0.38 (ADR 0071): `bynkc check --format` selector.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, ValueEnum)]
pub enum DiagFormat {
    /// Ariadne rendering with full source context (the default).
    #[default]
    Rich,
    /// One terse `path:line:col: severity[category]: message` line per
    /// diagnostic — for the VS Code problem-matcher, CI, and scripts.
    Short,
}

/// v0.59: `bynkc test --format` selector, and the `test` subcommand's flags —
/// [`bynk_driver::test_runner::TestFormat`]/[`TestArgs`], re-exported so
/// `TestFormat` resolves here unchanged (Wave 5 §5.4:
/// the `test` subcommand's contract, findings #40/#72, is now shared with
/// `bynk test` rather than a per-command near-duplicate).
pub use bynk_driver::test_runner::{TestArgs, TestFormat};

/// The `fmt` subcommand's flags, shared with `bynk fmt` the same way (#968):
/// one `#[derive(Args)]` struct both CLIs flatten, rather than two copies to
/// keep in step as formatting options are added.
pub use bynk_driver::{FmtArgs, IndentKind};

/// The `compile` subcommand's enums, shared with `bynk compile` (#2998): they live
/// in `bynk-driver` next to the command body, and are re-exported here.
pub use bynk_driver::{CliPlatform, CliTarget, EmitFormat};

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Compile a `.bynk` file (single-file commons) to a TypeScript file,
    /// or a directory project to a tree of TypeScript files mirroring the
    /// source layout.
    Compile {
        /// Input `.bynk` file, or directory project root. A workspace root is
        /// refused: compile each member.
        input: PathBuf,
        /// Output `.ts` file (for single-file input) or output root
        /// directory (for project input).
        #[arg(short, long)]
        output: PathBuf,
        /// Build target. `bundle` (default) produces a single deployment
        /// unit; `workers` produces one Cloudflare Worker per context with
        /// Service Binding plumbing (v0.8).
        #[arg(long, value_enum, default_value = "bundle")]
        target: CliTarget,
        /// Deploy platform selecting the `bynk` surface binding (v0.17). A new
        /// axis, distinct from `--target`: `cloudflare` (default), `node`, or
        /// `browser` (the in-browser playground binding; `Bundle` topology only).
        #[arg(long, value_enum, default_value = "cloudflare")]
        platform: CliPlatform,
        /// Artefact language (v0.108). `ts` (default) writes typed TypeScript;
        /// `js` writes the same modules with types stripped — a JavaScript
        /// artefact that runs with no `tsc` in the loop (ADR 0137).
        #[arg(long, value_enum, default_value = "ts")]
        emit: EmitFormat,
        /// Compile unsaved editor buffers (#2471): read a JSON object
        /// `{ "<absolute path>": "<text>" }` from stdin and layer it over the
        /// project's sources, so the overlay wins over disk. A path outside the
        /// project, or one that is not a `.bynk` file, is an error. Directory
        /// projects only. This is a preview build: `bynk.schema.lock` is
        /// neither read nor written, so nothing changes outside `--output`.
        #[arg(long)]
        sources_stdin: bool,
    },
    /// Type-check a `.bynk` file or project without writing output.
    Check {
        /// Input `.bynk` file or project root. A workspace root checks every
        /// member in turn.
        input: PathBuf,
        /// Diagnostic output format. `rich` (default) is the ariadne
        /// source-context rendering; `short` emits one terse
        /// `path:line:col: severity[category]: message` line per diagnostic,
        /// for tooling (the VS Code problem-matcher, CI, scripts). In both,
        /// `path` is the file as you'd type it from the working directory
        /// (the input you passed, joined with the file's place in it), the
        /// same path `fmt` reports.
        #[arg(long, value_enum, default_value = "rich")]
        format: DiagFormat,
        /// Apply the machine-applicable fix suggestions to the sources, in
        /// place, before checking (#2642): the same fixes the editor's
        /// "fix all" applies, repeated until none remain. Writes only files
        /// that change, does not format, and writes nothing if applying the
        /// fixes would leave a problem that needs a human. What remains is
        /// reported as usual.
        #[arg(long)]
        fix: bool,
    },
    /// Format `.bynk` source files in place. Passing `-` reads from stdin
    /// and writes to stdout.
    ///
    /// `--indent`, `--indent-width`, `--max-line-width` and
    /// `--no-trailing-comma` override the canonical style for this run; with
    /// none of them the output is the canonical formatting.
    Fmt {
        #[command(flatten)]
        args: bynk_driver::FmtArgs,
    },
    /// Discover and run test declarations in a project. Compiles the project
    /// (including all generated `tests/*.test.ts` modules), then invokes
    /// Node.js on the aggregated runner script. Requires `tsc` and `node`
    /// to be on PATH.
    Test {
        #[command(flatten)]
        args: bynk_driver::test_runner::TestArgs,
    },
}

/// The clap [`clap::Command`] tree for the `bynkc` CLI.
pub fn command() -> clap::Command {
    Cli::command()
}

fn styled_to_string(s: Option<&clap::builder::StyledStr>) -> String {
    s.map(|s| s.to_string()).unwrap_or_default()
}

/// One usage token for an argument, e.g. `<INPUT>`, `[--check]`, `--output <OUTPUT>`.
fn usage_token(arg: &clap::Arg) -> String {
    let required = arg.is_required_set();
    let is_flag = matches!(
        arg.get_action(),
        clap::ArgAction::SetTrue | clap::ArgAction::SetFalse
    );
    let value_name = arg
        .get_value_names()
        .and_then(|names| names.first().map(|n| n.to_string()))
        .unwrap_or_else(|| arg.get_id().to_string().to_uppercase());

    if arg.is_positional() {
        if required {
            format!("<{value_name}>")
        } else {
            format!("[{value_name}]")
        }
    } else {
        let long = arg
            .get_long()
            .map(|l| format!("--{l}"))
            .or_else(|| arg.get_short().map(|c| format!("-{c}")))
            .unwrap_or_default();
        if is_flag {
            format!("[{long}]")
        } else if required {
            format!("{long} <{value_name}>")
        } else {
            format!("[{long} <{value_name}>]")
        }
    }
}

/// Render the CLI reference as a Markdown page, walking the clap command tree.
pub fn render_markdown() -> String {
    let root = command();
    let mut out = String::new();

    out.push_str("# CLI (`bynkc`)\n\n");
    out.push_str(
        "<!-- GENERATED FILE — do not edit by hand.\n     \
         Source: bynk/src/bynkc_compat.rs (`render_markdown`).\n     \
         Regenerate with: BYNK_BLESS=1 cargo test -p xtask --test docs_consistency cli_reference:: -->\n\n",
    );
    let about = styled_to_string(root.get_about());
    if !about.is_empty() {
        out.push_str(&format!("{about}\n\n"));
    }
    out.push_str("Run `bynkc <command> --help` for the authoritative help text.\n");

    out.push_str(
        "\n## Exit codes and diagnostics\n\n\
         A diagnostic's **severity** decides whether it fails a build (v0.89). \
         An **`Error`** rejects the program: `bynkc compile`/`check` exit \
         non-zero and produce no output. A **`Warning`** is surfaced but does \
         **not** fail the build: these commands still **succeed (exit 0)** and \
         emit their output, with warnings reported alongside. The build-failure \
         gate counts error-severity diagnostics only. See the normative rule in \
         the [specification](../spec/diagnostics.md) and the \
         [diagnostic index](diagnostics.md) (warning-severity codes are marked \
         *(warning)*).\n",
    );

    let mut subs: Vec<&clap::Command> = root
        .get_subcommands()
        .filter(|c| c.get_name() != "help")
        .collect();
    subs.sort_by_key(|c| c.get_name().to_string());

    for sub in subs {
        let name = sub.get_name();
        out.push_str(&format!("\n## `bynkc {name}`\n\n"));
        let about = styled_to_string(sub.get_about());
        if !about.is_empty() {
            out.push_str(&format!("{about}\n\n"));
        }

        // Usage line: positionals in declaration order, then options.
        let mut usage = format!("bynkc {name}");
        for arg in sub.get_arguments().filter(|a| a.is_positional()) {
            usage.push(' ');
            usage.push_str(&usage_token(arg));
        }
        for arg in sub.get_arguments().filter(|a| !a.is_positional()) {
            usage.push(' ');
            usage.push_str(&usage_token(arg));
        }
        out.push_str(&format!("```text\n{usage}\n```\n\n"));

        let args: Vec<&clap::Arg> = sub.get_arguments().collect();
        if !args.is_empty() {
            out.push_str("| Argument | Required | Default | Description |\n");
            out.push_str("|---|---|---|---|\n");
            for arg in args {
                let label = if arg.is_positional() {
                    format!("`{}`", arg.get_id().to_string().to_uppercase())
                } else {
                    let long = arg
                        .get_long()
                        .map(|l| format!("`--{l}`"))
                        .unwrap_or_default();
                    match arg.get_short() {
                        Some(c) => format!("{long} (`-{c}`)"),
                        None => long,
                    }
                };
                let required = if arg.is_required_set() { "yes" } else { "no" };
                let default = {
                    let defs: Vec<String> = arg
                        .get_default_values()
                        .iter()
                        .map(|v| v.to_string_lossy().to_string())
                        .collect();
                    if defs.is_empty() {
                        "—".to_string()
                    } else {
                        format!("`{}`", defs.join(", "))
                    }
                };
                let mut desc = styled_to_string(arg.get_help())
                    .replace('\n', " ")
                    .replace('|', "\\|");
                // Boolean flags report `true`/`false` as possible values; that
                // is noise, so only list choices for value-taking options.
                let is_flag = matches!(
                    arg.get_action(),
                    clap::ArgAction::SetTrue | clap::ArgAction::SetFalse
                );
                let choices: Vec<String> = if is_flag {
                    Vec::new()
                } else {
                    arg.get_possible_values()
                        .iter()
                        .map(|pv| pv.get_name().to_string())
                        .collect()
                };
                if !choices.is_empty() {
                    desc.push_str(&format!(" (one of: {})", choices.join(", ")));
                }
                out.push_str(&format!("| {label} | {required} | {default} | {desc} |\n"));
            }
        }
    }

    out
}

/// Whether this process was started under the name `bynkc`: its executable's
/// file stem (so `bynkc` and `bynkc.exe`, any directory) is `bynkc`.
pub fn invoked_as_bynkc() -> bool {
    std::env::args_os()
        .next()
        .is_some_and(|arg0| is_bynkc_name(std::path::Path::new(&arg0)))
}

fn is_bynkc_name(arg0: &std::path::Path) -> bool {
    arg0.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("bynkc"))
}

/// The one-line stderr notice printed when `bynkc` runs a subcommand (Decision G).
pub const RETIREMENT_NOTICE: &str = "bynkc: notice: `bynkc` is retired and will be removed in the next release; use `bynk` (the same commands, flags and output).";

/// Run `bynkc`'s command line: parse it, print the notice, run the command.
/// Called by `bynk`'s `main` when invoked as `bynkc`, and by the final `bynkc`
/// crate's shim.
pub fn run() -> std::process::ExitCode {
    use clap::Parser as _;
    let cli = Cli::parse();
    eprintln!("{RETIREMENT_NOTICE}");
    match cli.command {
        Command::Compile {
            input,
            output,
            target,
            platform,
            emit,
            sources_stdin,
        } => bynk_driver::run_compile(
            "bynkc",
            bynk_driver::CompileArgs {
                input,
                output,
                target,
                platform,
                emit,
                sources_stdin,
            },
        ),
        Command::Check { input, format, fix } => {
            bynk_driver::run_check("bynkc", &input, format == DiagFormat::Short, fix)
        }
        Command::Fmt { args } => bynk_driver::run_fmt("bynkc", &args),
        Command::Test { args } => bynk_driver::test_runner::run_test("bynkc", args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_name_bynkc_is_recognised_with_or_without_a_directory_or_extension() {
        for name in ["bynkc", "/usr/local/bin/bynkc", "bynkc.exe", "BYNKC.EXE"] {
            assert!(is_bynkc_name(Path::new(name)), "{name}");
        }
        for name in ["bynk", "bynk-under-test", "bynkc-lsp", "/opt/bynkc/bynk"] {
            assert!(!is_bynkc_name(Path::new(name)), "{name}");
        }
    }
}
