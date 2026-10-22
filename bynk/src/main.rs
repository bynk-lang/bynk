//! `bynk` — the Bynk driver binary. See the crate docs in `lib.rs`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bynk::check;
use bynk::cli::{CheckFormatArg, Cli, Command};
use bynk::compiler::{self, Compiler};
use bynk::deploy::{self, DeployFormat, DeployOptions};
use bynk::dev::{self, DevOptions};
use bynk::doctor::{self, Context, DoctorOptions};
use bynk::fmt;
use bynk::new::{self, NewOptions};
use bynk::probe::{SystemToolbox, Toolbox, Version};
use bynk::report::{self, Format};
use bynk::test;
use clap::Parser;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor {
            input,
            only,
            strict,
            format,
        } => run_doctor(
            input,
            bynk::cli::doctor_options(only, strict),
            format.into(),
        ),
        Command::Dev {
            path,
            lock,
            contexts,
            base_port,
            inspect,
            inspect_port,
            env,
            local_workers,
            format,
            ready_json,
            wrangler_args,
        } => {
            if !prepare(&path, lock.options()) {
                return ExitCode::FAILURE;
            }
            run_dev(
                path,
                DevOptions {
                    contexts,
                    base_port,
                    inspect,
                    inspect_port,
                    environment: env,
                    local_workers,
                    wrangler_args,
                    format: format.into(),
                    ready_json,
                },
            )
        }
        Command::Deploy {
            path,
            lock,
            context,
            env,
            dry_run,
            format,
            yes,
            secrets_file,
            secrets,
            force,
            prune,
            confirm_delete,
            wrangler_args,
        } => {
            if !prepare(&path, lock.options()) {
                return ExitCode::FAILURE;
            }
            run_deploy(
                path,
                DeployOptions {
                    context,
                    environment: env,
                    dry_run,
                    format: match format {
                        bynk::cli::DeployFormatArg::Short => DeployFormat::Short,
                        bynk::cli::DeployFormatArg::Json => DeployFormat::Json,
                    },
                    yes,
                    secrets_file,
                    secrets,
                    force,
                    prune,
                    confirm_delete,
                    wrangler_args,
                },
            )
        }
        Command::New { path, name } => new::run(&NewOptions { path, name }),
        Command::Check {
            input,
            format,
            lock,
        } => {
            if !prepare(&input, lock.options()) {
                return ExitCode::FAILURE;
            }
            run_check(input, format)
        }
        Command::Fmt { args } => run_fmt(args),
        Command::Test {
            args,
            allow_skew,
            lock,
        } => {
            if !prepare(&args.input, lock.options()) {
                return ExitCode::FAILURE;
            }
            run_test(args, allow_skew)
        }
        Command::Fetch { path, lock } => {
            if prepare(&path, lock.options()) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Command::Update { names, path } => {
            let opts = bynk::fetch::FetchOptions {
                locked: false,
                offline: false,
                update: Some(names),
            };
            if prepare(&path, opts) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Command::Migrate { command } => match command {
            bynk::cli::MigrateCommand::Status { path, lock, format } => {
                if !prepare(&path, lock.options()) {
                    return ExitCode::FAILURE;
                }
                bynk::migrate::status(
                    &path,
                    match format {
                        bynk::cli::DeployFormatArg::Short => bynk::migrate::StatusFormat::Short,
                        bynk::cli::DeployFormatArg::Json => bynk::migrate::StatusFormat::Json,
                    },
                )
            }
            bynk::cli::MigrateCommand::Sweep {
                path,
                context,
                environments,
                batch,
                urls,
                yes,
            } => bynk::migrate::sweep(
                &path,
                &bynk::migrate::SweepOptions {
                    context,
                    environments,
                    batch: usize::from(batch),
                    yes,
                    urls,
                },
            ),
        },
        Command::Add {
            package,
            alias,
            path,
        } => {
            let Some(root) = find_project_root(&path) else {
                eprintln!(
                    "bynk: not inside a Bynk project (no bynk.toml found from `{}`)",
                    path.display()
                );
                return ExitCode::FAILURE;
            };
            match bynk::registry::commands::add_dependency(&root, &package, alias.as_deref()) {
                Ok(line) => {
                    eprintln!("bynk: added {line}");
                    if prepare(&root, bynk::fetch::FetchOptions::default()) {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    }
                }
                Err(e) => {
                    eprintln!("bynk: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Registry { command } => run_registry(command),
        Command::Explain { code } => bynk::explain::run(&code),
    }
}

/// #2240: resolve, fetch and lock the dependencies of the project holding
/// `input` before a command compiles it. Prints the failure and returns
/// `false` when it can't; a single file outside any project needs nothing.
fn prepare(input: &Path, opts: bynk::fetch::FetchOptions) -> bool {
    let Some(project_root) = find_project_root(input) else {
        return true;
    };
    match bynk::fetch::prepare(&project_root, &opts) {
        Ok(done) => {
            for f in &done.fetched {
                eprintln!("bynk: fetched {f}");
            }
            for w in &done.warnings {
                eprintln!("bynk: warning: {w}");
            }
            if done.wrote_lock {
                eprintln!("bynk: wrote bynk.lock");
            }
            true
        }
        Err(e) => {
            eprintln!("bynk: {e}");
            false
        }
    }
}

/// #2274: `bynk registry init|add|yank`.
fn run_registry(command: bynk::cli::RegistryCommand) -> ExitCode {
    use bynk::cli::RegistryCommand;
    use bynk::registry::commands;
    let result = match command {
        RegistryCommand::Init { dir, id, key_out } => {
            let key_out = key_out.unwrap_or_else(|| commands::default_key_path(&id));
            commands::init(&dir, &id, &key_out).map(|pin| {
                eprintln!(
                    "bynk: created registry `{id}` at `{}`; its signing key is `{}` — keep it secret, and out of the registry directory",
                    dir.display(),
                    key_out.display()
                );
                println!("{pin}");
            })
        }
        RegistryCommand::Add { dir, package, key } => {
            commands::add(&dir, &package, &key).map(|what| eprintln!("bynk: added {what}"))
        }
        RegistryCommand::Yank {
            dir,
            package,
            version,
            undo,
        } => commands::yank(&dir, &package, &version, undo).map(|()| {
            eprintln!(
                "bynk: {} {package} {version}",
                if undo { "un-yanked" } else { "yanked" }
            )
        }),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bynk: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Locate the compiler the driver shells: `$BYNK_BYNKC` override, else `PATH`,
/// else a sibling of this `bynk` binary. Shared by every subcommand.
fn resolve_compiler(tb: &dyn Toolbox) -> Compiler {
    let override_path = std::env::var_os("BYNK_BYNKC").map(PathBuf::from);
    let bynk_bin_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let driver = Version::parse(bynk::DRIVER_VERSION).unwrap_or(Version {
        major: 0,
        minor: 0,
        patch: 0,
    });
    compiler::resolve(
        tb,
        override_path.as_deref(),
        bynk_bin_dir.as_deref(),
        driver,
    )
}

fn run_doctor(input: PathBuf, opts: DoctorOptions, format: Format) -> ExitCode {
    let tb = SystemToolbox;
    let compiler = resolve_compiler(&tb);

    let project_root = find_project_root(&input);
    let ctx = Context {
        in_repo: in_bynk_repo(&input),
        project_root,
        node_floor: bynk_emit::NODE_MAJOR_FLOOR,
    };

    let report = doctor::diagnose(&tb, &compiler, &ctx, &opts);
    print!("{}", report::render(&report, format));

    if report.exit_nonzero(&opts) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn run_dev(path: PathBuf, opts: DevOptions) -> ExitCode {
    let tb = SystemToolbox;
    let compiler = resolve_compiler(&tb);

    let Some(project_root) = find_project_root(&path) else {
        eprintln!(
            "bynk: not inside a Bynk project (no bynk.toml found from `{}`)",
            path.display()
        );
        return ExitCode::FAILURE;
    };
    if refuse_workspace_root("dev", &project_root) {
        return ExitCode::FAILURE;
    }
    // #524: `dev` compiles and watches the whole `[paths]` layout from the
    // project root — the same shape as `bynkc compile` — so no per-include
    // root is selected here any more.
    dev::run(
        &tb,
        &compiler,
        &project_root,
        bynk_emit::NODE_MAJOR_FLOOR,
        &opts,
    )
}

fn run_deploy(path: PathBuf, opts: DeployOptions) -> ExitCode {
    let tb = SystemToolbox;
    let compiler = resolve_compiler(&tb);
    let Some(project_root) = find_project_root(&path) else {
        eprintln!(
            "bynk: not inside a Bynk project (no bynk.toml found from `{}`)",
            path.display()
        );
        return ExitCode::FAILURE;
    };
    if refuse_workspace_root("deploy", &project_root) {
        return ExitCode::FAILURE;
    }
    deploy::run(
        &tb,
        &compiler,
        &project_root,
        bynk_emit::NODE_MAJOR_FLOOR,
        &opts,
    )
}

/// `bynk check` (#487): resolve `bynkc` (for the override escape hatch) and
/// type-check in-process via the linked pipeline.
fn run_check(input: PathBuf, format: CheckFormatArg) -> ExitCode {
    let tb = SystemToolbox;
    let compiler = resolve_compiler(&tb);
    check::run(&compiler, input, format)
}

/// `bynk fmt` (#487): format in-process via `bynk-fmt`, or shell a pinned
/// `bynkc` under a `BYNK_BYNKC` override.
fn run_fmt(args: bynk_driver::FmtArgs) -> ExitCode {
    let tb = SystemToolbox;
    let compiler = resolve_compiler(&tb);
    fmt::run(&compiler, args)
}

/// `bynk test` (#487): delegate to the driver-resolved `bynkc`, forwarding every
/// flag verbatim.
fn run_test(args: bynk_driver::test_runner::TestArgs, allow_skew: bool) -> ExitCode {
    let tb = SystemToolbox;
    let compiler = resolve_compiler(&tb);
    test::run(&compiler, args, allow_skew)
}

/// #2207 (decision C): `dev` and `deploy` run one project, and a workspace
/// root isn't one. Says so and returns `true` when `root` is a workspace root.
fn refuse_workspace_root(command: &str, root: &Path) -> bool {
    if !bynk_emit::project::is_workspace_root(root) {
        return false;
    }
    eprintln!(
        "bynk {command}: `{}` is a workspace root, not a project — run `bynk {command}` inside a member",
        root.display()
    );
    true
}

/// Walk up from `start` for the nearest `bynk.toml` (the project root).
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.canonicalize().ok()?;
    loop {
        if dir.join("bynk.toml").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Are we inside the Bynk source repo? Gates the contributor `build`
/// capability. Identified by the workspace markers `bynkc/Cargo.toml` and
/// `design/decisions` in some ancestor.
fn in_bynk_repo(start: &Path) -> bool {
    let Ok(mut dir) = start.canonicalize() else {
        return false;
    };
    loop {
        if dir.join("bynkc").join("Cargo.toml").is_file()
            && dir.join("design").join("decisions").is_dir()
        {
            return true;
        }
        if !dir.pop() {
            return false;
        }
    }
}
