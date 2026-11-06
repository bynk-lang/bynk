// bynkc — the Bynk v0.3 compiler CLI.

use std::path::PathBuf;
use std::process::ExitCode;

use bynkc::cli::{Cli, CliPlatform, CliTarget, Command, DiagFormat, EmitFormat};
use clap::Parser;

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
        } => run_compile(input, output, target, platform, emit, sources_stdin),
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
    target: CliTarget,
    platform: CliPlatform,
    emit: EmitFormat,
    sources_stdin: bool,
) -> ExitCode {
    // #2998: the command body is shared with the `bynk` driver.
    bynk_driver::run_compile(
        "bynkc",
        bynk_driver::CompileArgs {
            input,
            output,
            target,
            platform,
            emit,
            sources_stdin,
        },
    )
}

fn run_check(input: PathBuf, format: DiagFormat, fix: bool) -> ExitCode {
    // #521: the command body is shared with the `bynk` driver.
    bynk_driver::run_check("bynkc", &input, format == DiagFormat::Short, fix)
}
