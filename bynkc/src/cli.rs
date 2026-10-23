//! The `bynkc` command-line interface definition.
//!
//! The clap types live here (rather than in `main.rs`) so they are the single
//! source of truth for both the binary and the generated CLI reference page
//! `site/src/content/docs/docs/cli.md`. [`render_markdown`] walks the
//! clap command tree;
//! the test `tests/cli_reference.rs` checks the page is up to date.

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

use crate::BuildTarget;

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
/// existing `bynkc::cli::TestFormat` paths resolve unchanged (Wave 5 §5.4:
/// the `test` subcommand's contract, findings #40/#72, is now shared with
/// `bynk test` rather than a per-command near-duplicate).
pub use bynk_driver::test_runner::{TestArgs, TestFormat};

/// The `fmt` subcommand's flags, shared with `bynk fmt` the same way (#968):
/// one `#[derive(Args)]` struct both CLIs flatten, rather than two copies to
/// keep in step as formatting options are added.
pub use bynk_driver::{FmtArgs, IndentKind};

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
    /// Node.js (≥ [`NODE_MAJOR_FLOOR`](crate::NODE_MAJOR_FLOOR)) runtime (v0.18).
    Node,
    /// The browser — the `bynk` surface over Web APIs, for the in-browser
    /// REPL/playground (v0.108). `Bundle` topology only; `Fetch`/`Secrets` are
    /// withheld (see ADR 0138).
    Browser,
}

impl From<CliPlatform> for crate::Platform {
    fn from(p: CliPlatform) -> Self {
        match p {
            CliPlatform::Cloudflare => crate::Platform::Cloudflare,
            CliPlatform::Node => crate::Platform::Node,
            CliPlatform::Browser => crate::Platform::Browser,
        }
    }
}

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
         Source: bynkc/src/cli.rs (`render_markdown`).\n     \
         Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it cli_reference:: -->\n\n",
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
