use super::bindings::DEV_CONFIG;
use super::bindings::DevConfigs;
use super::bindings::dev_vars_for;
use super::bindings::hyperdrive_local_env;
use super::bindings::materialise_remote_state;
use super::bindings::warn_missing_local_secrets;
use super::events::DevEvent;
use super::events::DevFormat;
use super::events::emit;
use super::events::serving_event;
use super::events::serving_report;
use super::events::session_of;
use super::serving::LocalKeys;
use super::serving::allocate;
use super::serving::assign_free_inspector;
use super::serving::free_local_port;
use super::serving::inspector_port_in_use;
use super::serving::port_clash;
use super::serving::serve_args;
use super::stand_ins::plan_stand_ins;
use super::stand_ins::stand_in_report;
use super::stand_ins::write_stand_ins;
use super::supervise::DRAIN_GRACE;
use super::supervise::Served;
use super::supervise::join_pumps;
use super::supervise::pipes_output;
use super::supervise::pump_output;
use super::supervise::supervise;
use super::supervise::terminate;
use super::watch::MigrationsSeen;
use super::watch::apply_local_migrations;
use super::watch::watch_fingerprint;
use crate::doctor;
use crate::doctor::Capability;
use crate::doctor::Context;
use crate::doctor::DoctorOptions;
use crate::doctor::Report;
use crate::probe;
use crate::probe::Provenance;
use crate::probe::Toolbox;
use crate::report;
use crate::report::Format;
use crate::workers::discover_workers;
use crate::workers::prepare_build_dir;
use crate::workers::select_contexts;
use bynk_driver::colour;
use bynk_driver::emsg;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

/// Parsed `bynk dev` flags (the project `PATH` is resolved into `project_root`
/// before we get here).
#[derive(Debug, Clone, Default)]
pub struct DevOptions {
    /// `--no-install` (or `BYNK_NO_INSTALL`): stop on a missing npm package
    /// instead of installing it (#3350).
    pub no_install: bool,
    /// `--context NAME`, repeatable — which contexts' workers to serve. Empty
    /// serves **every** context in the project, wired (ADR 0096 D3 superseded).
    pub contexts: Vec<String>,
    /// `--base-port N` — the first port of the per-context allocation. `None`
    /// leaves a lone worker on wrangler's own default, so `-- --port N` keeps
    /// working exactly as it did when `dev` served one context.
    pub base_port: Option<u16>,
    /// `--inspect` (slice 3): start `wrangler dev` with the V8 inspector so a
    /// JavaScript debugger can attach; breakpoints in `.bynk` resolve through the
    /// emitted source maps composed into the worker bundle.
    pub inspect: bool,
    /// `--inspect-port N` — the base inspector port, allocated per context
    /// exactly as `base_port` is. `None` means it was not given: the base is
    /// 9229 under `--inspect` or with several contexts, and a lone worker
    /// without `--inspect` is given a free port of its own
    /// ([`assign_free_inspector`]). wrangler's inspector is always on, so left
    /// on its default every session would race for 9229 (#1954, #3237).
    pub inspect_port: Option<u16>,
    /// `--env NAME` (default `"default"`) — the environment whose
    /// `[env.<name>.vars]` values the project's `env(…)` constants take for
    /// every build `dev` runs (#3941), and, with `--remote`, which
    /// `bynk.deploy.lock` section the KV id is read from. `dev` never
    /// provisions and never writes the ledger. Never forwarded to
    /// `wrangler dev` itself, since `dev` curates no Wrangler-side
    /// environment config (#837: a project deployed only under a non-default
    /// `--env` otherwise read as "never provisioned" here).
    pub environment: String,
    /// `--local-worker NAME`, repeatable (interop I5, #1972): a foreign
    /// Worker a `Service` instance names that `wrangler dev` should reach in
    /// the user's own locally running `wrangler dev` of it, through the dev
    /// registry, rather than in the stand-in `dev` binds by default.
    pub local_workers: Vec<String>,
    /// Everything after `--`, forwarded to `wrangler dev` verbatim (D5).
    pub wrangler_args: Vec<String>,
    /// `--format` (#2094): `human` (the default) prints only the readable
    /// stderr stream; `json` also writes one [`DevEvent`] per line to stdout.
    pub format: DevFormat,
    /// `--ready-json FILE` (#2138): once every served context answers a
    /// probe, write `{ "<worker>": "<url>" }` to FILE ([`crate::ready`]).
    pub ready_json: Option<PathBuf>,
    /// `--log-events` (#2474): under `--format json`, also write each line a
    /// worker prints as a [`DevEvent::Log`], its stack frames rewritten to
    /// `.bynk` locations. Off by default, so a script reading the stream sees
    /// only the lifecycle events.
    pub log_events: bool,
    /// `--trace-events` (#3580): build the workers with per-request trace
    /// recording, and write each request as a [`DevEvent::Request`]. Implies
    /// `--format json`.
    pub trace_events: bool,
    /// `--rewrite-frames` (#2474): rewrite stack frames to `.bynk` even on a
    /// terminal, where the driver otherwise leaves wrangler's output alone.
    pub rewrite_frames: bool,
}

/// Whether the forwarded wrangler arguments ask for remote mode: `--remote`,
/// its `-r` alias, or `--remote=true`.
pub(crate) fn is_remote(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "--remote" | "-r" | "--remote=true"))
}

/// #837 review: once `--remote` reads a ledger section by `--env`, a
/// `-- --env`/`-- --environment` passthrough would silently diverge — `bynk`
/// materialises one environment's KV id while Wrangler actually connects to a
/// different one. Checked only when `--remote` is present, since `--env` is
/// otherwise inert (nothing reads the ledger without it). Prints the conflict
/// and returns `true` when there is one.
fn remote_env_conflict(opts: &DevOptions) -> bool {
    if !is_remote(&opts.wrangler_args) {
        return false;
    }
    let Some(conflict) = crate::deploy::conflicting_env_passthrough(&opts.wrangler_args) else {
        return false;
    };
    emsg!(
        "bynk: error: `--env {}` conflicts with `{conflict}` after `--` — pass one or the other, not both",
        opts.environment
    );
    true
}

/// #3562 (G6): a group is one Worker, so `--context` naming a member selects the
/// Worker that holds it, and says so: the rest of the group cannot be left out of a
/// Worker. A name that is no member passes through (it is a Worker's own, or unknown).
fn groups_serving(project_root: &Path, requested: &[String]) -> Result<Vec<String>, String> {
    let groups = bynk_project::worker_groups::try_read_worker_groups(project_root)
        .map_err(|e| format!("invalid `[deploy.workers]` in `bynk.toml`: {e}"))?;
    Ok(requested
        .iter()
        .map(|name| {
            let dotted = name.replace('-', ".");
            let Some((key, _)) = groups
                .iter()
                .find(|(_, g)| g.contexts.contains(&dotted))
            else {
                return name.clone();
            };
            emsg!(
                "bynk: `{dotted}` is in the Worker group `{key}`, which runs as one Worker: serving the whole group"
            );
            key.clone()
        })
        .collect())
}

/// The Workers to serve: every one, or those `--context` names (a group member selecting
/// its group, [`groups_serving`]).
fn selected_workers(
    project_root: &Path,
    available: &[String],
    requested: &[String],
) -> Option<Vec<String>> {
    let chosen = groups_serving(project_root, requested)
        .and_then(|r| select_contexts(available, &r).map_err(|e| e.to_string()));
    chosen.map_err(|e| emsg!("bynk: error: {e}")).ok()
}

/// Orchestrate a local dev session: pre-flight, compile, select the worker, and
/// hand off to `wrangler dev`. Returns wrangler's own exit code on a clean
/// hand-off, or a pre-flight/build failure code before serving.
pub fn run(
    tb: &dyn Toolbox,
    project_root: &Path,
    node_floor: (u32, u32),
    opts: &DevOptions,
) -> ExitCode {
    if remote_env_conflict(opts) {
        return ExitCode::FAILURE;
    }

    if opts.log_events && opts.format != DevFormat::Json {
        emsg!("bynk: error: `--log-events` writes events, so it needs `--format json`");
        return ExitCode::FAILURE;
    }

    // 1. Pre-flight — reuse doctor's Deploy gate (Node + wrangler) plus the
    //    always-on compile floor. Failing here, with doctor's remedy text, beats
    //    a confusing error out of a half-built tree (proposal §2.2).
    let ctx = Context {
        project_root: Some(project_root.to_path_buf()),
        in_repo: false,
        node_floor,
    };
    let preflight_opts = DoctorOptions {
        only: Some(Capability::Deploy),
        strict: false,
    };
    let report = doctor::diagnose(tb, &ctx, &preflight_opts);
    if report.exit_nonzero(&preflight_opts) {
        colour::eprint(&preflight_failure_message(&report));
        return ExitCode::FAILURE;
    }
    // 2. Compile — in-process (slice 7: the driver links the pipeline instead of
    //    shelling a separate compiler). Into the managed `.bynk/dev/` build dir (D1).
    //    Compilation is additive (never prunes), so clear `workers/` first;
    //    otherwise a renamed/deleted context would linger and spuriously trip the
    //    §2.4 ambiguity check.
    let build_dir = project_root.join(".bynk").join("dev");
    if let Err(e) = prepare_build_dir(project_root, &build_dir) {
        emsg!("bynk: error: could not prepare build directory: {e}");
        return ExitCode::FAILURE;
    }
    // #524: compile the SAME project shape as `bynk compile <project_root>`
    // — the shared rooting rule over the full `[paths]` layout. `dev`
    // previously re-rooted on the first `include` entry only, silently
    // dropping further includes and the whole `exclude` list.
    // #3100: the first build reports each context; a watch rebuild stays quiet.
    let Some(model) = crate::workers::compile_built(
        project_root,
        &build_dir,
        dev_build(opts),
        bynk_driver::progress::terminal_sink(),
    ) else {
        return ExitCode::FAILURE;
    };

    // 3. Select the workers — every context by default, or the `--context`
    //    subset (#552, superseding ADR 0096 D3's select-or-default). Serving
    //    them *together* is the whole point: a cross-context call only resolves
    //    when its callee is up too, so an ambiguity error here was the feature
    //    being withheld, not a project being wrong.
    let workers_dir = build_dir.join("workers");
    let available = discover_workers(&workers_dir);
    let Some(workers) = selected_workers(project_root, &available, &opts.contexts) else {
        return ExitCode::FAILURE;
    };
    let mut serving = allocate(&workers, opts.base_port, opts);
    // #3237: a lone worker would otherwise take wrangler's default inspector,
    // 9229, which two sessions starting together both claim. Not when the
    // caller put `--inspector-port` in the `--` passthrough: that is theirs.
    if !passthrough_has(&opts.wrangler_args, "--inspector-port") {
        assign_free_inspector(&mut serving, free_local_port());
    }
    if let Some(port) = inspector_port_in_use(&serving) {
        emsg!(
            "bynk: error: inspector port {port} is already in use — another `bynk dev` or debugger \
             holds it. Pass a different `--inspect-port`, or stop the process using it."
        );
        return ExitCode::FAILURE;
    }
    // I2 (#1971): over every built Worker, not only the served ones, so a
    // context served here alone still admits a caller served by another
    // `bynk dev --context` session.
    let local = match LocalKeys::read(&workers_dir, &available, &model) {
        Ok(keys) => keys,
        Err(e) => {
            emsg!("bynk: error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Where the driver injects a port it owns the allocation, so the same flag
    // arriving through `--` is a conflict — and wrangler rejects a repeated
    // `--port` with a usage dump rather than taking the last one. Catch it here
    // and name the driver flag that owns it.
    for (flag, owner, injected) in [
        (
            "--port",
            "--base-port",
            serving.iter().any(|s| s.port.is_some()),
        ),
        (
            "--inspector-port",
            "--inspect-port",
            serving.iter().any(|s| s.inspector_port.is_some()),
        ),
    ] {
        if injected && passthrough_has(&opts.wrangler_args, flag) {
            emsg!(
                "bynk: error: `{flag}` is allocated per context — pass `{owner}` to `bynk dev` instead of `-- {flag}`."
            );
            return ExitCode::FAILURE;
        }
    }

    // Now that inspectors are allocated without `--inspect` too (#1954), an
    // overlap with the HTTP range is reachable from plain flags (`--base-port
    // 9229` over two contexts). Name it here rather than leave wrangler to die
    // with `Address already in use`.
    if let Some(port) = port_clash(&serving) {
        emsg!(
            "bynk: error: port {port} is allocated both to a context's server and to an inspector — \
             move `--base-port` or `--inspect-port` so the two ranges do not overlap."
        );
        return ExitCode::FAILURE;
    }

    // #3477: a container Worker runs under Docker here, never `--remote`.
    let Some(containers) =
        crate::dev_containers::for_session(&model, &serving, &opts.wrangler_args)
    else {
        return ExitCode::FAILURE;
    };

    // Remote dev reads the real Cloudflare ids, unlike Miniflare's local mode,
    // from the deploy ledger immediately before Wrangler runs.
    if is_remote(&opts.wrangler_args)
        && !materialise_remote_state(project_root, &workers_dir, &serving, &opts.environment)
    {
        return ExitCode::FAILURE;
    }

    // Interop I5 (#1972, track §11 Q8): each context's Service Bindings to
    // foreign Workers get a stand-in in its own `wrangler dev`, unless
    // `--local-worker` names the Worker (the dev registry then reaches the
    // user's own `wrangler dev` of it). Under `--remote` the bindings are the
    // account's, so there is nothing to stand in for.
    let remote = is_remote(&opts.wrangler_args);
    let stand_in_plans = plan_stand_ins(&workers_dir, &serving, &available, opts, remote);
    let user_config = passthrough_has(&opts.wrangler_args, "-c")
        || passthrough_has(&opts.wrangler_args, "--config");
    if !stand_in_plans.is_empty() && user_config {
        emsg!(
            "bynk: error: `-c`/`--config` is the driver's when it runs stand-ins for foreign Workers — pass `--local-worker NAME` instead of a config after `--`."
        );
        return ExitCode::FAILURE;
    }
    let stand_in_configs = match write_stand_ins(&build_dir, &stand_in_plans) {
        Ok(configs) => configs,
        Err(e) => {
            emsg!("bynk: error: could not write the stand-ins for foreign Workers: {e}");
            return ExitCode::FAILURE;
        }
    };
    let dev_configs = DevConfigs {
        project_root,
        workers_dir: &workers_dir,
        available: &available,
        serving: &serving,
        stand_ins: &stand_in_plans,
        remote,
        environment: &opts.environment,
    };
    if !dev_configs.write(&model, true) {
        return ExitCode::FAILURE;
    }

    // 4. Serve — one `wrangler dev` per context, each from inside its own worker
    //    dir (the emitted `index.ts` imports `../../runtime.js`, so cwd must be
    //    the worker dir, exactly the manual recipe's `cd`). The processes find
    //    each other through wrangler's **dev registry** and wire the generated
    //    `[[services]]` bindings between themselves — verified: a binding starts
    //    `[not connected]` and converges to `[connected]` once its callee is up,
    //    so start order does not matter and we need not stage the spawns.
    //    Resolve wrangler once with doctor's provenance ordering; an npx
    //    resolution downloads on first use, so it is a notice, never a silent
    //    green path.
    let (manager, probe) = probe::detect_in_project(tb, "wrangler", project_root);
    if let Some(runner) = probe.provenance.runner_label() {
        emsg!("bynk: wrangler resolved via {runner} — it will download on first run.");
    }
    if let Some(notice) = wrangler_age_notice(&probe, manager) {
        colour::eprintln(&notice);
    }
    if matches!(probe.provenance, Provenance::Missing) {
        // The pre-flight gate should have caught this; defensive only.
        emsg!("bynk: error: wrangler not found (run `bynk doctor --only deploy`)");
        return ExitCode::FAILURE;
    }

    // #3477: Docker: containers off when it is absent.
    let containers_off = crate::dev_containers::session(tb, &containers);

    if !warm_up_wrangler(&probe.provenance, serving.len()) {
        return ExitCode::FAILURE;
    }

    // P4b (#2608): each served Worker's local D1 databases get their
    // migrations before it serves. Never with `--remote`: that database is a
    // real one, and only `bynk deploy` changes it. Nothing is spawned yet, so
    // a signal during a slow apply needs no reaping.
    let mut applied = MigrationsSeen::default();
    if !remote {
        apply_local_migrations(
            &probe.provenance,
            &workers_dir,
            &serving,
            &opts.wrangler_args,
            &mut applied,
        );
        // #3301: and a stored secret the local store does not hold is said
        // so now, not found as a fault on the first request.
        warn_missing_local_secrets(
            &probe.provenance,
            &workers_dir,
            &serving,
            &opts.wrangler_args,
        );
    }

    // Inherited stdio (the default) keeps every session interactive. The driver
    // and the wranglers share the terminal's foreground process group, so a
    // Ctrl-C SIGINT reaches them all — we must not bail before reaping; we reap
    // in the watch loop and propagate the first exit code (ADR 0096 §Exit).
    //
    // #1955: a SIGINT or SIGTERM sent to `bynk dev` *alone* (a CI step, a test
    // harness) used to kill it outright and strand every wrangler and its
    // `workerd`s. Record it instead, and let the watch loop stop the contexts.
    // Installed before the spawns, so there is no window in which a child is
    // up and the driver still dies of the default action; the children
    // themselves get the default back at `exec`.
    crate::stop::install();
    // #3319: each `Postgres` instance's local connection string, from the
    // variable `[dev.hyperdrive]` names, into that Worker's environment only.
    let Some(local_strings) = hyperdrive_local_env(project_root, &workers_dir, &serving, remote)
    else {
        return ExitCode::FAILURE;
    };
    // #3478: `BYNK_DEV` and the acknowledged remote-only kinds, as `--var`s.
    let Some(dev_vars) = dev_vars_for(project_root) else {
        return ExitCode::FAILURE;
    };
    let rewriter = std::sync::Arc::new(crate::logs::Rewriter::new(&build_dir));
    let mut children: Vec<(String, Served)> = Vec::new();
    let mut pumps: Vec<std::thread::JoinHandle<()>> = Vec::new();
    for s in &serving {
        let Some(mut cmd) = crate::cloudflare::spawn_command(&probe.provenance, "dev") else {
            emsg!("bynk: error: wrangler not found (run `bynk doctor --only deploy`)");
            terminate(&mut children, &workers_dir);
            return ExitCode::FAILURE;
        };
        cmd.current_dir(workers_dir.join(&s.worker));
        // Interop I5 (#1972): the context's own config first (wrangler's
        // primary Worker, the one served on the port), then its stand-ins.
        // #3505: a `-- -c`/`--config` of the developer's own (refused above when
        // stand-ins run) is served as given.
        if !user_config {
            cmd.args(["-c", DEV_CONFIG]);
        }
        for config in stand_in_configs.get(&s.worker).into_iter().flatten() {
            cmd.arg("-c").arg(config);
        }
        let keys = local
            .edge
            .get(&s.worker)
            .map(Vec::as_slice)
            .unwrap_or_default();
        cmd.args(serve_args(s, keys));
        cmd.args(&dev_vars);
        cmd.args(crate::dev_containers::off_args_for(
            containers_off,
            &containers,
            &s.worker,
        ));
        cmd.args(local.session_args(&s.worker, &opts.wrangler_args));
        for arg in &opts.wrangler_args {
            cmd.arg(arg);
        }
        for local in local_strings.get(&s.worker).into_iter().flatten() {
            cmd.env(&local.name, &local.value);
        }
        // #2474: off a terminal (the editor's task, CI) or on request,
        // wrangler's output passes through the driver, line by line, so a
        // stack frame into generated code is rewritten to the `.bynk` line it
        // came from. On a terminal it is left alone: piping would turn off
        // wrangler's colour and interactive hotkeys. Piped output is not a
        // terminal, so colour is asked back for when ours is one. `--color
        // always`/`never` is passed on either way.
        let piped = pipes_output(opts);
        bynk_driver::colour::apply_to_child(&mut cmd, piped);
        if piped {
            cmd.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
        } else if opts.format == DevFormat::Json {
            // #2094: stdout is the event stream, so wrangler's banner and
            // request log go to stderr with the rest of the readable output.
            cmd.stdout(std::io::stderr());
        }
        match cmd.spawn() {
            Ok(mut child) => {
                if piped {
                    pumps.extend(pump_output(&mut child, &s.worker, &rewriter, opts));
                }
                children.push((s.worker.clone(), Served::new(child)));
            }
            Err(e) => {
                let w = &s.worker;
                emsg!("bynk: error: could not run wrangler for `{w}`: {e}");
                terminate(&mut children, &workers_dir);
                return ExitCode::FAILURE;
            }
        }
    }
    colour::eprint(&serving_report(&serving));
    colour::eprint(&stand_in_report(&stand_in_plans));
    let serving_now = serving_event(&serving, &opts.wrangler_args);
    emit(opts.format, &serving_now);
    let _session = crate::dev_inspect::SessionFile::write(
        project_root,
        &session_of(&serving_now, &workers_dir),
    );
    // #2138: `--ready-json` — written from the watch loop below once every
    // served context answers a probe. The probe is plain HTTP on localhost,
    // so a server it can't reach never reads as ready; say so up front.
    if opts.ready_json.is_some()
        && let Some(why) = crate::ready::unprobeable(&opts.wrangler_args)
    {
        emsg!(
            "bynk dev: warning: {why}, which the `--ready-json` probe (plain HTTP on localhost) can't reach, so the file may never be written."
        );
    }
    let mut readiness = opts.ready_json.as_ref().map(|file| {
        let DevEvent::Serving { contexts } = &serving_now else {
            unreachable!("serving_event builds a Serving event")
        };
        crate::ready::Readiness::new(
            file.clone(),
            contexts
                .iter()
                .map(|c| (c.name.clone(), c.port, c.url.clone())),
        )
    });

    // 5. Watch — #524: `bynk dev` is the edit loop, so watch the project's
    // `.bynk` sources (the full `[paths]` layout plus `bynk.toml`) and rebuild
    // into the same build dir on change. Each `wrangler dev` watches its own
    // built worker files, so one rebuild hot-reloads every context that changed
    // without a restart; a failing rebuild renders diagnostics and keeps both
    // the watch and the last good build serving. std-only mtime polling
    // (500ms): no native watcher dependency, and an edit-loop latency well
    // under a keystroke-to-glance.
    emsg!("bynk dev: watching for source changes (edit `.bynk` files to rebuild)");
    let mut fingerprint = watch_fingerprint(project_root);
    // The model of the last good build, replaced only by a build that
    // succeeds, as the build it describes is.
    let mut model = model;
    let code = supervise(&mut children, &workers_dir, || {
        if let Some(r) = readiness.as_mut() {
            if let Some(written) = r.poll(crate::ready::probe) {
                match written {
                    Ok(()) => emsg!("bynk dev: every context answers — ready"),
                    // Not retried: the session keeps serving, unsignalled.
                    Err(e) => emsg!("bynk: warning: could not write the `--ready-json` file: {e}"),
                }
            }
            if let Some(note) = r.slow_note(std::time::Instant::now()) {
                colour::eprintln(&note);
            }
        }
        let now = watch_fingerprint(project_root);
        if now != fingerprint {
            fingerprint = now;
            emsg!("bynk dev: change detected — rebuilding…");
            emit(opts.format, &DevEvent::Rebuilding);
            let rebuild = dev_build(opts);
            if let Some(rebuilt) =
                crate::workers::compile_built(project_root, &build_dir, rebuild, None)
            {
                model = rebuilt;
                // Interop I5 (#1972): a binding added since start has no
                // stand-in until `bynk dev` restarts.
                dev_configs.write(&model, false);
                emsg!("bynk dev: rebuilt");
                emit(opts.format, &DevEvent::RebuildOk);
            } else {
                // The diagnostics are already rendered; keep serving the last
                // good build and keep watching.
                emit(opts.format, &DevEvent::RebuildFailed);
            }
        }
    });
    // The workers are stopped: let their last lines out before `stopped`.
    join_pumps(pumps, DRAIN_GRACE);
    if let Some(r) = &readiness {
        r.stop();
    }
    emit(opts.format, &DevEvent::Stopped { code });
    ExitCode::from(code)
}

/// How `bynk dev` builds, first and on every rebuild: it reconciles the schema
/// lock, never dry-runs, and (#3580, `--trace-events`) records request traces.
fn dev_build(opts: &DevOptions) -> crate::workers::Build {
    crate::workers::Build::new(true, false)
        .no_install(opts.no_install)
        .trace_requests(opts.trace_events)
        .inspect_agents(true)
        // #3941: `--env` selects whose `env(…)` values the build resolves.
        .environment(&opts.environment)
}

/// #1957: N parallel first-run `npx wrangler dev`s race to install into the
/// same `_npx` cache entry, and the loser dies at boot (ENOTEMPTY). Install it
/// once, up front, so every spawn finds it already there. Stdout is dropped
/// (the version line is noise); stderr stays, because npm's progress is the
/// only sign a slow first download is not a hang. `false` when it could not be
/// installed: spawning anyway would re-run the very race this avoids.
fn warm_up_wrangler(provenance: &Provenance, serving: usize) -> bool {
    if !needs_wrangler_warmup(provenance, serving) {
        return true;
    }
    let runner = provenance.runner_label().unwrap_or("npx");
    emsg!("bynk dev: installing wrangler via {runner} once before starting the contexts…");
    let warmed = crate::cloudflare::spawn_command(provenance, "--version").map(|mut cmd| {
        cmd.stdout(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    });
    if warmed != Some(true) {
        emsg!(
            "bynk: error: `{}` failed — wrangler could not be installed via {runner}.",
            wrangler_warm_up_command_line(provenance)
        );
        return false;
    }
    true
}

/// The warm-up command as a user would type it, to reproduce a failure.
fn wrangler_warm_up_command_line(provenance: &Provenance) -> String {
    let Provenance::Runner(manager) = provenance else {
        return "wrangler --version".to_string();
    };
    let program = manager.runner_program().unwrap_or("npx");
    let args = manager.runner_args(bynk_driver::tools::pins::WRANGLER_SPEC, "wrangler");
    format!("{program} {} --version", args.join(" "))
}

/// #1732: the warning `bynk dev` prints before serving with a wrangler older
/// than [`bynk_emit::WRANGLER_MIN`]. An older `workerd` can refuse the pinned
/// compatibility date outright, and wrangler's own error says nothing about
/// Bynk, so this names the cause and the fix first. A warning, not a refusal:
/// the version is `doctor`'s judgement, and the wrangler itself has the last
/// word. `None` when the wrangler is new enough, or can't be versioned (npx).
pub fn wrangler_age_notice(
    probe: &probe::Probe,
    manager: bynk_driver::npm_install::PackageManager,
) -> Option<String> {
    let v = probe
        .version
        .filter(|_| doctor::wrangler_below_min(probe))?;
    Some(format!(
        "bynk: warning: wrangler {v} is older than {}, the oldest this Bynk release supports; \
         an older runtime may refuse compatibility date {}, and `wrangler dev` then exits. \
         Upgrade with `{}`.",
        bynk_emit::WRANGLER_MIN,
        bynk_emit::COMPATIBILITY_DATE,
        doctor::wrangler_upgrade_remedy(probe, manager)
    ))
}

/// The text `bynk dev` prints when the deploy pre-flight fails: a lead line plus
/// doctor's own human report, so the remedy lines are identical to `bynk
/// doctor`. Pure (no I/O) so this deterministic surface is pinned by a golden
/// (§5), unlike the non-deterministic `wrangler dev` stream.
pub fn preflight_failure_message(report: &Report) -> String {
    format!(
        "bynk: error: environment not ready for `dev` — see below.\n\n{}",
        report::render(report, Format::Human)
    )
}

/// #1957: whether to install wrangler once before spawning the contexts. Only
/// an npx resolution installs on first use, and only several spawns at once
/// race each other in npm's `_npx` cache; a lone context installs alone.
pub fn needs_wrangler_warmup(provenance: &Provenance, contexts: usize) -> bool {
    matches!(provenance, Provenance::Runner(_)) && contexts > 1
}

/// Whether the `--` passthrough carries `flag`, which the driver also injects.
/// Wrangler rejects a repeated `--port`/`--inspector-port` outright ("expects a
/// single value, but received multiple"), so we catch the clash ourselves and
/// say which driver flag owns it instead of letting wrangler's usage dump land.
pub(super) fn passthrough_has(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|a| a == flag || a.starts_with(&format!("{flag}=")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev::test_support::group_project;
    use crate::dev::test_support::names;
    use crate::probe::Provenance;
    use crate::workers::SelectError;
    use crate::workers::select_contexts;
    use bynk_driver::npm_install::PackageManager;

    use std::path::PathBuf;

    /// #3562 (G6): `--context` naming a member of a group serves the Worker that holds it
    /// (dotted or dashed); a Worker's own name, or an unknown one, passes through.
    #[test]
    fn a_member_context_serves_the_group_that_holds_it() {
        let root = group_project(
            "serve",
            "[project]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deploy.workers.core]\ncontexts = [\"shop.a\", \"shop.b\"]\n",
        );
        let served = groups_serving(
            &root,
            &names(&["shop.a", "shop-b", "core", "shop.pay", "nope"]),
        )
        .unwrap();
        assert_eq!(served, names(&["core", "core", "core", "shop.pay", "nope"]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_project_with_no_groups_passes_its_contexts_through() {
        let root = group_project("none", "[project]\nname = \"p\"\nversion = \"0.1.0\"\n");
        assert_eq!(
            groups_serving(&root, &names(&["shop.a"])).unwrap(),
            names(&["shop.a"])
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_sole_context_is_selected_without_a_flag() {
        assert_eq!(
            select_contexts(&names(&["links"]), &[]),
            Ok(names(&["links"]))
        );
    }

    #[test]
    fn no_workers_is_its_own_error() {
        assert_eq!(select_contexts(&[], &[]), Err(SelectError::NoneBuilt));
    }

    // ---- #552: multi-context selection ------------------------------------

    #[test]
    fn no_context_flag_serves_every_context() {
        // The defining change: several contexts is the expected shape, so the
        // whole project is served rather than refused as ambiguous.
        assert_eq!(
            select_contexts(&names(&["api", "worker"]), &[]),
            Ok(names(&["api", "worker"]))
        );
    }

    #[test]
    fn context_flags_narrow_to_a_subset() {
        let avail = names(&["api", "commerce-payment", "worker"]);
        assert_eq!(
            select_contexts(&avail, &names(&["worker", "api"])),
            Ok(names(&["api", "worker"])),
            "the subset is served in the deterministic order, not the typed one"
        );
        // Dotted and dasherised forms both resolve, and repeats collapse.
        assert_eq!(
            select_contexts(&avail, &names(&["commerce.payment", "commerce-payment"])),
            Ok(names(&["commerce-payment"]))
        );
    }

    #[test]
    fn selecting_many_reports_an_unknown_context() {
        assert_eq!(
            select_contexts(&names(&["api"]), &names(&["api", "nope"])),
            Err(SelectError::NotFound {
                requested: "nope".to_string(),
                available: names(&["api"]),
            })
        );
    }

    #[test]
    fn selecting_many_from_an_empty_build_is_still_none_built() {
        assert_eq!(select_contexts(&[], &[]), Err(SelectError::NoneBuilt));
    }

    #[test]
    fn the_warm_up_failure_quotes_the_command_to_reproduce() {
        let line = |m| wrangler_warm_up_command_line(&Provenance::Runner(m));
        assert_eq!(
            line(PackageManager::Npm),
            "npx --yes -p wrangler@4 wrangler --version"
        );
        assert_eq!(
            line(PackageManager::Pnpm),
            "pnpm dlx --package wrangler@4 wrangler --version"
        );
        assert_eq!(
            line(PackageManager::Bun),
            "bunx --package wrangler@4 wrangler --version"
        );
    }

    #[test]
    fn npx_is_warmed_up_only_for_several_contexts() {
        // #1957: parallel first-run `npx` installs race in npm's cache.
        assert!(needs_wrangler_warmup(
            &Provenance::Runner(PackageManager::Npm),
            2
        ));
        assert!(!needs_wrangler_warmup(
            &Provenance::Runner(PackageManager::Npm),
            1
        ));
        let installed = Provenance::Path(PathBuf::from("/usr/bin/wrangler"));
        assert!(!needs_wrangler_warmup(&installed, 2));
    }
}
