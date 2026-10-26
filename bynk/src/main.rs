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
use bynk::registry::account;
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
            log_events,
            rewrite_frames,
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
                    log_events,
                    rewrite_frames,
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
            if prepare_offering_approval(&path, lock.options()) {
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
            if prepare_offering_approval(&path, opts) {
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
                    if prepare_offering_approval(&root, bynk::fetch::FetchOptions::default()) {
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
        Command::Publish { path, dry_run } => {
            let Some(root) = find_project_root(&path) else {
                eprintln!(
                    "bynk: not inside a Bynk project (no bynk.toml found from `{}`)",
                    path.display()
                );
                return ExitCode::FAILURE;
            };
            // Publishing never ships code that doesn't check.
            if !prepare(&root, bynk::fetch::FetchOptions::default()) {
                return ExitCode::FAILURE;
            }
            if run_check(root.clone(), CheckFormatArg::Short) != ExitCode::SUCCESS {
                eprintln!("bynk: not published: the package doesn't check");
                return ExitCode::FAILURE;
            }
            match bynk::registry::publish::publish(&root, dry_run) {
                Ok(p) if p.dry_run => {
                    eprintln!(
                        "bynk: would publish {} {} ({}) to `{}`",
                        p.name, p.version, p.checksum, p.registry
                    );
                    ExitCode::SUCCESS
                }
                Ok(p) => {
                    eprintln!(
                        "bynk: published {} {} ({}) to `{}`",
                        p.name, p.version, p.checksum, p.registry
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("bynk: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Yank {
            package,
            version,
            undo,
            path,
        } => {
            let root = find_project_root(&path).unwrap_or(path);
            match bynk::registry::publish::yank(&root, &package, &version, undo) {
                Ok(registry) => {
                    eprintln!(
                        "bynk: {} {package} {version} on `{registry}`",
                        if undo { "un-yanked" } else { "yanked" }
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("bynk: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Login { target } => account_result(account::login(&account_target(target))),
        Command::Logout { target } => account_result(account::logout(&account_target(target))),
        Command::Org { command } => run_org(command),
        Command::Approve {
            package,
            all,
            list,
            format,
            path,
        } => {
            let root = find_project_root(&path).unwrap_or(path);
            let mode = match (package, all, list) {
                (_, _, true) if format == bynk::cli::DeployFormatArg::Json => {
                    bynk::approve::Mode::ListJson
                }
                (_, _, true) => bynk::approve::Mode::List,
                (_, true, _) => bynk::approve::Mode::All,
                (Some(p), _, _) => bynk::approve::Mode::Package(p),
                (None, false, false) => bynk::approve::Mode::Ask,
            };
            match bynk::approve::run(&root, &mode) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("bynk: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Explain { code } => bynk::explain::run(&code),
    }
}

/// #2296: [`prepare`] for the commands that exist to change the lock (`add`,
/// `fetch`, `update`): afterwards, offer to approve what now awaits approval.
fn prepare_offering_approval(input: &Path, opts: bynk::fetch::FetchOptions) -> bool {
    let Some(project_root) = find_project_root(input) else {
        return true;
    };
    match bynk::fetch::prepare(&project_root, &opts) {
        Ok(done) => {
            report_prepared(&done);
            bynk::approve::after_write(&project_root, &done.pending);
            true
        }
        Err(e) => {
            eprintln!("bynk: {e}");
            false
        }
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
            report_prepared(&done);
            true
        }
        Err(e) => {
            eprintln!("bynk: {e}");
            false
        }
    }
}

/// #2378: `bynk org`.
fn run_org(command: bynk::cli::OrgCommand) -> ExitCode {
    use bynk::cli::{MemberCommand, OrgCommand, OrgTokenCommand};
    account_result(match command {
        OrgCommand::Claim {
            org,
            block,
            owner,
            target,
        } => account::claim(&account_target(target), &org, block, owner.as_deref()),
        OrgCommand::Show {
            org,
            format,
            target,
        } => account::show(
            &account_target(target),
            &org,
            matches!(format, bynk::cli::DeployFormatArg::Json),
        ),
        OrgCommand::Block { org, target } => {
            account::set_blocked(&account_target(target), &org, true)
        }
        OrgCommand::Unblock { org, target } => {
            account::set_blocked(&account_target(target), &org, false)
        }
        OrgCommand::Member { command } => match command {
            MemberCommand::Add { org, login, target } => {
                account::member_add(&account_target(target), &org, &login)
            }
            MemberCommand::Remove {
                org,
                member,
                target,
            } => account::member_remove(&account_target(target), &org, &member),
        },
        OrgCommand::Token { command } => match command {
            OrgTokenCommand::Mint {
                org,
                can,
                days,
                note,
                save,
                force,
                target,
            } => account::token_mint(
                &account_target(target),
                &org,
                &account::Mint {
                    can,
                    days,
                    note,
                    save,
                    force,
                },
            ),
            OrgTokenCommand::Revoke { org, id, target } => {
                account::token_revoke(&account_target(target), &org, &id)
            }
        },
    })
}

/// #2378: an account command's registry, from `--registry` and `--path`.
fn account_target(target: bynk::cli::AccountTarget) -> account::Target {
    account::Target {
        project: find_project_root(&target.path).unwrap_or(target.path),
        registry: target.registry,
    }
}

fn account_result(result: Result<(), bynk::fetch::FetchError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bynk: {e}");
            ExitCode::FAILURE
        }
    }
}

/// #2274: `bynk registry init|add|yank`.
fn run_registry(command: bynk::cli::RegistryCommand) -> ExitCode {
    use bynk::cli::RegistryCommand;
    use bynk::registry::commands;
    let result = match command {
        RegistryCommand::Conformance {
            index,
            org,
            name,
            key,
            read_only,
            package,
            narrow_token_env,
            max_upload,
            format,
        } => {
            return run_conformance(
                bynk::registry::conformance::Options {
                    index,
                    org: org.unwrap_or_default(),
                    name,
                    key,
                    read_only,
                    package,
                    narrow_token: narrow_token_env.map(|v| std::env::var(&v).map_err(|_| v)),
                    max_upload,
                },
                matches!(format, bynk::cli::DeployFormatArg::Json),
            );
        }
        RegistryCommand::Rotate { dir, key, key_out } => commands::rotate(&dir, &key, &key_out)
            .map(|k| {
                eprintln!(
                    "bynk: appended {k} to the key chain; its private key is `{}` — publish `config.json`",
                    key_out.display()
                )
            }),
        RegistryCommand::Delegate {
            dir,
            key,
            delegate_out,
        } => commands::delegate(&dir, &key, &delegate_out).map(|k| {
            eprintln!(
                "bynk: delegated signing to {k}; its private key is `{}` — publish `config.json`, then sign with it",
                delegate_out.display()
            )
        }),
        RegistryCommand::Revoke {
            dir,
            target,
            key,
            force,
        } => commands::revoke(&dir, &key, &target, force).map(|n| {
            eprintln!(
                "bynk: revoked {target} ({n} line(s) still signed by it) — publish `config.json`"
            )
        }),
        RegistryCommand::Resign { dir, key } => commands::resign(&dir, &key)
            .map(|n| eprintln!("bynk: re-signed {n} index line(s) — publish the index files")),
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
        RegistryCommand::Serve {
            dir,
            key,
            tokens,
            listen,
        } => bynk::registry::serve::serve(&dir, &key, &tokens, &listen, |addr| {
            eprintln!(
                "bynk: serving registry `{}` on {addr} (publish API at {addr}/api)",
                dir.display()
            );
        }),
        RegistryCommand::Token {
            tokens,
            organisations,
            can,
        } => bynk::registry::serve::mint(&tokens, &organisations, &can).map(|token| {
            eprintln!(
                "bynk: minted a token for {} — shown once; only its hash is in `{}`",
                organisations.join(", "),
                tokens.display()
            );
            println!("{token}");
        }),
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
        RegistryCommand::Withdraw {
            dir,
            package,
            version,
            reason,
        } => commands::withdraw(&dir, &package, &version, &reason).map(|archive| {
            if archive != commands::ArchiveOutcome::Finished {
                eprintln!("bynk: withdrew {package} {version}");
            }
            match archive {
                commands::ArchiveOutcome::Deleted => {
                    eprintln!("bynk: deleted its archive");
                }
                commands::ArchiveOutcome::Shared(other) => eprintln!(
                    "bynk: warning: kept its archive, which {other} names too (the same tree)"
                ),
                commands::ArchiveOutcome::Missing => {
                    eprintln!("bynk: its archive was already gone");
                }
                commands::ArchiveOutcome::Finished => {
                    eprintln!("bynk: it was already withdrawn; deleted the archive left behind");
                }
            }
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

/// #2311: `bynk registry conformance` — the report, and a failure when any
/// check failed (`bynk.registry.nonconformant`).
fn run_conformance(opts: bynk::registry::conformance::Options, json: bool) -> ExitCode {
    use bynk::registry::conformance::Outcome;
    eprintln!("bynk: registry conformance against {}", opts.index);
    let report = bynk::registry::conformance::run(&opts);
    if json {
        print!("{}", report.render_json());
    } else {
        for c in &report.checks {
            println!("  {:<5} {:<26} {}", c.outcome.as_str(), c.id, c.detail);
        }
    }
    if let Some(p) = &report.published {
        eprintln!("bynk: published (and left yanked) the throwaway package `{p}`");
    }
    let summary = format!(
        "{} passed, {} failed, {} skipped",
        report.count(Outcome::Pass),
        report.count(Outcome::Fail),
        report.count(Outcome::Skip)
    );
    if report.failed() > 0 {
        for c in report.checks.iter().filter(|c| c.outcome == Outcome::Fail) {
            eprintln!(
                "bynk: error[{}]: `{}` failed — see {}",
                bynk::registry::conformance::NONCONFORMANT,
                c.id,
                c.spec
            );
        }
        eprintln!("bynk: {summary}");
        ExitCode::FAILURE
    } else {
        eprintln!("bynk: {summary}");
        ExitCode::SUCCESS
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

fn report_prepared(done: &bynk::fetch::Prepared) {
    for f in &done.fetched {
        eprintln!("bynk: fetched {f}");
    }
    for w in &done.warnings {
        eprintln!("bynk: warning: {w}");
    }
    if done.wrote_lock {
        eprintln!("bynk: wrote bynk.lock");
    }
}
