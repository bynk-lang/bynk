//! `bynk dev` — build a project and serve it locally in one step.
//!
//! Collapses the manual recipe (compile → `cd` into the generated worker dir →
//! `wrangler dev`) into a single command (proposal v0.57). The orchestration is
//! **pre-flight → compile → select → serve**, and almost every piece is reused:
//! [`compiler::resolve`](crate::compiler) for `bynkc`, the doctor `Deploy`
//! capability for the Node + `wrangler` gate, and [`probe`] for locating
//! `wrangler` with the same provenance ordering doctor reports.
//!
//! The serve step runs `wrangler dev` in **local mode** (Miniflare), which
//! simulates KV / Durable Objects / queues keyed by *binding name* — so no
//! namespace provisioning is needed and the generated `wrangler.toml` is served
//! untouched (proposal §1, D4). Everything `wrangler`-specific is encapsulated
//! here so the serve step can later be swapped for a first-party `workerd`
//! server without touching the rest (proposal §4).
//!
//! Since #552 the step is **one `wrangler dev` per context**, not one per
//! project: the processes discover each other through wrangler's dev registry
//! and wire the emitted `[[services]]` bindings between themselves, so a
//! cross-context call resolves locally. That makes the driver a supervisor of N
//! children rather than a hand-off to one — hence the port allocation
//! ([`allocate`]), the joint teardown (`terminate`, private), and the plural
//! selection rule ([`select_contexts`]) that replaced ADR 0096 D3's ambiguity
//! error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use bynk_emit::project::{ProjectPaths, try_read_project_paths};

use crate::compiler::Compiler;
use crate::doctor::{self, Capability, Context, DoctorOptions, Report};
use crate::probe::{self, DetectOpts, Provenance, Toolbox};
use crate::report::{self, Format};
use crate::shell::exit_status_byte;

// The build/select/`wrangler` plumbing `dev` shares with `deploy` now lives in
// [`crate::workers`] (it is neither command's to own). Re-exported rather than
// merely imported, so `bynk::dev::compile_once` and `bynk::dev::select_contexts`
// — the paths this crate's integration tests already use — keep resolving.
pub use crate::workers::{
    SelectError, compile_once, discover_workers, prepare_build_dir, select_contexts,
    wrangler_command,
};

/// Parsed `bynk dev` flags (the project `PATH` is resolved into `project_root`
/// before we get here).
#[derive(Debug, Clone, Default)]
pub struct DevOptions {
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
    /// Base inspector port (default 9229); allocated per context exactly as
    /// `base_port` is. Used under `--inspect`, and whenever several contexts
    /// are served: wrangler's inspector is always on, so N wranglers left on
    /// its default would all race for 9229 (#1954).
    pub inspect_port: u16,
    /// `--env NAME` (default `"default"`) — which `bynk.deploy.lock` section
    /// `--remote` reads the KV id from. `dev` never provisions and never
    /// writes the ledger (unchanged); this only selects which of `deploy`'s
    /// environments `--remote` connects the placeholder to. Purely a `bynk`
    /// concept — never forwarded to `wrangler dev` itself, since `dev` curates
    /// no Wrangler-side environment config (slice 4, #837 review: a project
    /// deployed only under a non-default `--env` previously read as
    /// "never provisioned" here, because this always looked at `"default"`).
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
}

/// `bynk dev --format` (#2094).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DevFormat {
    /// The readable stream on stderr, unchanged.
    #[default]
    Human,
    /// The readable stream on stderr, plus one JSON [`DevEvent`] per line on
    /// stdout. `wrangler dev`'s own stdout moves to stderr, so stdout carries
    /// nothing but events.
    Json,
}

/// One lifecycle event of a `bynk dev --format json` session (#2094): a stable,
/// line-delimited interface for the editor and CI scripts, so neither has to
/// parse the human report, whose wording is UX and changes. Serialised with an
/// `event` tag in snake_case. A consumer should ignore fields and events it
/// does not know: new ones may be added, existing ones keep their meaning.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DevEvent {
    /// Every `wrangler dev` has been started. Ports are allocated, but a
    /// worker may still be booting; poll its URL for readiness.
    Serving { contexts: Vec<ServedContext> },
    /// A source change was seen and a rebuild has begun.
    Rebuilding,
    /// The rebuild succeeded; the workers hot-reload from it.
    RebuildOk,
    /// The rebuild failed. Its diagnostics are on stderr, and the last good
    /// build keeps serving.
    RebuildFailed,
    /// The session ended, with the exit code `bynk dev` is about to return.
    Stopped { code: u8 },
}

/// One served context in a [`DevEvent::Serving`] event.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ServedContext {
    /// The context's worker dir (dasherised), as `--context` accepts it.
    pub name: String,
    /// The HTTP port it answers on.
    pub port: u16,
    /// `http://localhost:<port>`.
    pub url: String,
    /// Its V8 inspector port, when one was allocated (`--inspect`, or several
    /// contexts served); `null` otherwise.
    pub inspector_port: Option<u16>,
}

/// The [`DevEvent::Serving`] event for an allocation. A lone worker left on
/// wrangler's default (`port: None`) reports the port it actually gets: a
/// `-- --port N` passthrough, else wrangler's 8787.
pub fn serving_event(serving: &[Serving], wrangler_args: &[String]) -> DevEvent {
    DevEvent::Serving {
        contexts: serving
            .iter()
            .map(|s| {
                let port = s
                    .port
                    .or_else(|| passthrough_port(wrangler_args))
                    .unwrap_or(DEFAULT_BASE_PORT);
                ServedContext {
                    name: s.worker.clone(),
                    port,
                    url: format!("http://localhost:{port}"),
                    inspector_port: s.inspector_port,
                }
            })
            .collect(),
    }
}

/// One event as its JSON line (no trailing newline).
pub fn event_line(event: &DevEvent) -> String {
    serde_json::to_string(event).expect("a dev event serialises")
}

/// Write `event` to stdout under `--format json`; nothing under `human`.
fn emit(format: DevFormat, event: &DevEvent) {
    if format == DevFormat::Json {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{}", event_line(event));
        let _ = out.flush();
    }
}

/// The `--port` value of a `-- --port N` / `-- --port=N` passthrough.
fn passthrough_port(args: &[String]) -> Option<u16> {
    args.iter().enumerate().find_map(|(i, a)| {
        if a == "--port" {
            args.get(i + 1).and_then(|v| v.parse().ok())
        } else {
            a.strip_prefix("--port=").and_then(|v| v.parse().ok())
        }
    })
}

/// Interop I5 (#1972): one stand-in `bynk dev` runs for a foreign Worker that
/// a served context binds: the Worker's name, and the context's bindings to
/// it. It runs inside that context's own `wrangler dev`, as a second `-c`
/// config, under a name of its own ([`stand_in_name`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandIn {
    pub worker: String,
    pub bindings: Vec<String>,
}

/// The Worker name `context`'s stand-in for `worker` runs under. Not the
/// foreign Worker's own: a `-c` config's Worker also registers in wrangler's
/// dev registry, which every local `wrangler dev` shares (verified on 4.149.0),
/// so under the real name it would answer other sessions' bindings to that
/// Worker and, once stopped, could leave a stale entry for `--local-worker` to
/// connect to. The context's binding is pointed at this name instead
/// (`point_at_stand_ins`).
///
/// Kept within Cloudflare's 63-character Worker name limit: a longer name is
/// cut and given a hash of the whole, so two stand-ins never share one.
pub fn stand_in_name(context: &str, worker: &str) -> String {
    let name = format!("bynk-stand-in-{context}-{worker}");
    if name.len() <= 63 {
        return name;
    }
    // FNV-1a: stable across runs and toolchains, unlike `DefaultHasher`.
    let hash = name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    let suffix = format!("-{:08x}", hash as u32);
    let mut cut = 63 - suffix.len();
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{suffix}", name[..cut].trim_end_matches('-'))
}

/// The config a context with stand-ins is served from: its generated
/// `wrangler.toml` with each binding to a foreign Worker pointed at the
/// stand-in, beside the original so `main` resolves the same.
pub const STAND_IN_CONFIG: &str = "wrangler.dev.toml";

/// Write each served context's [`STAND_IN_CONFIG`] from its generated
/// `wrangler.toml`, run after every build. A copy, not an edit: a rebuild
/// rewrites `wrangler.toml` with the Workers' real names, and `wrangler dev`
/// watching it would reload once with the real Worker bound (reaching a real
/// local session of it, which the stand-in's own name exists to prevent)
/// before an edit could retarget it (review of #2008). The copy is replaced
/// by a rename, so wrangler only ever reads it whole, and only when it
/// changes.
fn point_at_stand_ins(workers_dir: &Path, plans: &[(String, Vec<StandIn>)]) -> Result<(), String> {
    for (context, stand_ins) in plans {
        let targets: std::collections::BTreeMap<String, String> = stand_ins
            .iter()
            .flat_map(|s| {
                let name = stand_in_name(context, &s.worker);
                s.bindings.iter().map(move |b| (b.clone(), name.clone()))
            })
            .collect();
        let dir = workers_dir.join(context);
        let text = std::fs::read_to_string(dir.join("wrangler.toml")).map_err(|e| e.to_string())?;
        let out = bynk_emit::emitter::wrangler::retarget_services(&text, &targets)?;
        let path = dir.join(STAND_IN_CONFIG);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(out.as_str()) {
            let staged = dir.join(format!("{STAND_IN_CONFIG}.tmp"));
            std::fs::write(&staged, out).map_err(|e| e.to_string())?;
            std::fs::rename(&staged, &path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Interop I5 (#1972): the Service Bindings in a generated `wrangler.toml`
/// whose target is none of `project_workers`, as `(binding, worker)` in
/// binding order: the bindings to foreign Workers, by the rule `bynk deploy`
/// uses. An unreadable config has none; the serve step reports it.
pub fn foreign_services(config_text: &str, project_workers: &[String]) -> Vec<(String, String)> {
    let Ok(doc) = config_text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = doc
        .get("services")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let binding = s.get("binding")?.as_str()?;
            let worker = s.get("service")?.as_str()?;
            (!project_workers.iter().any(|w| w == worker))
                .then(|| (binding.to_string(), worker.to_string()))
        })
        .collect();
    out.sort();
    out
}

/// Interop I5 (#1972, track §11 Q8): the stand-ins one context's `wrangler
/// dev` runs, one per foreign Worker it binds, in Worker order, less those
/// `local` names. Verified on wrangler 4.149.0: a second `-c` config's Worker
/// answers its primary's binding to it, and without one a binding to a Worker
/// that is not running answers 503 ("Worker … not found").
pub fn stand_ins(foreign: &[(String, String)], local: &[String]) -> Vec<StandIn> {
    let mut by_worker: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
    for (binding, worker) in foreign {
        if !local.iter().any(|l| l == worker) {
            by_worker.entry(worker).or_default().push(binding.clone());
        }
    }
    by_worker
        .into_iter()
        .map(|(worker, bindings)| StandIn {
            worker: worker.to_string(),
            bindings,
        })
        .collect()
}

/// The stand-in's `wrangler.toml` and `index.js`. It answers every request
/// with [`bynk_emit::emitter::wrangler::FOREIGN_SERVICE_DOUBLE_STATUS`] and a
/// body saying what it is and how to reach the real Worker, as the
/// integration test harness's stand-in does.
pub fn stand_in_files(context: &str, stand_in: &StandIn) -> (String, String) {
    let config = format!(
        "# Generated by bynk dev — `{context}`'s local stand-in for Worker `{w}`.\nname = {name}\nmain = \"index.js\"\ncompatibility_date = \"{date}\"\n",
        w = stand_in.worker,
        name = toml::Value::String(stand_in_name(context, &stand_in.worker)),
        date = bynk_emit::emitter::wrangler::COMPATIBILITY_DATE,
    );
    let body = format!(
        "{} Run that Worker locally with `wrangler dev` and pass `--local-worker {}` to `bynk dev` to call it instead.",
        bynk_emit::emitter::wrangler::foreign_service_double_body(
            &stand_in.bindings.join("`, `"),
            &stand_in.worker
        ),
        stand_in.worker
    );
    let script = format!(
        "// Generated by bynk dev — a local stand-in for Worker `{w}`.\nexport default {{\n  async fetch() {{\n    return new Response({body}, {{\n      status: {status},\n      headers: {{ \"content-type\": \"text/plain; charset=utf-8\", \"x-bynk-stand-in\": {name} }},\n    }});\n  }},\n}};\n",
        w = stand_in.worker,
        body = serde_json::Value::String(body),
        status = bynk_emit::emitter::wrangler::FOREIGN_SERVICE_DOUBLE_STATUS,
        name = serde_json::Value::String(stand_in.worker.clone()),
    );
    (config, script)
}

/// Write each served context's stand-ins under `<build>/stand-ins/<context
/// worker>/<foreign worker>/`, returning their config paths by context. The
/// directory is rebuilt each session, like `workers/`.
fn write_stand_ins(
    build_dir: &Path,
    plans: &[(String, Vec<StandIn>)],
) -> std::io::Result<std::collections::BTreeMap<String, Vec<PathBuf>>> {
    let root = build_dir.join("stand-ins");
    match std::fs::remove_dir_all(&root) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut configs = std::collections::BTreeMap::new();
    for (context, stand_ins) in plans {
        for stand_in in stand_ins {
            let dir = root.join(context).join(&stand_in.worker);
            std::fs::create_dir_all(&dir)?;
            let (config, script) = stand_in_files(context, stand_in);
            std::fs::write(dir.join("wrangler.toml"), config)?;
            std::fs::write(dir.join("index.js"), script)?;
            configs
                .entry(context.clone())
                .or_insert_with(Vec::new)
                .push(dir.join("wrangler.toml"));
        }
    }
    Ok(configs)
}

/// The start-up lines naming each stand-in, so a 503 from one is never a
/// mystery. Empty when there are none.
pub fn stand_in_report(plans: &[(String, Vec<StandIn>)]) -> String {
    let mut out = String::new();
    for (context, stand_ins) in plans {
        for s in stand_ins {
            out.push_str(&format!(
                "bynk dev: `{context}` binds Worker `{w}` ({}) to a local stand-in that answers 503; pass `--local-worker {w}` to reach a `wrangler dev` of it instead.\n",
                s.bindings.join(", "),
                w = s.worker,
            ));
        }
    }
    out
}

/// Wrangler's own default dev port — the base of the per-context allocation
/// when `--base-port` is not given, so a multi-context project's first worker
/// lands where a single-context one always has.
const DEFAULT_BASE_PORT: u16 = 8787;

/// One worker to serve: its dasherised context dir, its HTTP port (`None` = let
/// wrangler choose, the lone-worker default), its inspector port, and whether
/// the inspector was asked for (`--inspect`) rather than only allocated so the
/// contexts' default inspectors do not collide (#1954).
#[derive(Debug, PartialEq, Eq)]
pub struct Serving {
    pub worker: String,
    pub port: Option<u16>,
    pub inspector_port: Option<u16>,
    pub inspect: bool,
}

/// Orchestrate a local dev session: pre-flight, compile, select the worker, and
/// hand off to `wrangler dev`. Returns wrangler's own exit code on a clean
/// hand-off, or a pre-flight/build failure code before serving.
pub fn run(
    tb: &dyn Toolbox,
    compiler: &Compiler,
    project_root: &Path,
    node_floor: u32,
    opts: &DevOptions,
) -> ExitCode {
    // #837 review: once `--remote` reads a ledger section by `--env`, a
    // `-- --env`/`-- --environment` passthrough would silently diverge —
    // `bynk` materialises one environment's KV id while Wrangler actually
    // connects to a different one. Checked only when `--remote` is present,
    // since `--env` is otherwise inert (nothing reads the ledger without it).
    if opts.wrangler_args.iter().any(|arg| arg == "--remote")
        && let Some(conflict) = crate::deploy::conflicting_env_passthrough(&opts.wrangler_args)
    {
        eprintln!(
            "bynk: `--env {}` conflicts with `{conflict}` after `--` — pass one or the other, not both",
            opts.environment
        );
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
    let report = doctor::diagnose(tb, compiler, &ctx, &preflight_opts);
    if report.exit_nonzero(&preflight_opts) {
        eprint!("{}", preflight_failure_message(&report));
        return ExitCode::FAILURE;
    }
    // 2. Compile — in-process (slice 7: the driver links the pipeline instead of
    //    shelling `bynkc`). Into the managed `.bynk/dev/` build dir (D1).
    //    Compilation is additive (never prunes), so clear `workers/` first;
    //    otherwise a renamed/deleted context would linger and spuriously trip the
    //    §2.4 ambiguity check.
    let build_dir = project_root.join(".bynk").join("dev");
    if let Err(e) = prepare_build_dir(project_root, &build_dir) {
        eprintln!("bynk: could not prepare build directory: {e}");
        return ExitCode::FAILURE;
    }
    // #524: compile the SAME project shape as `bynkc compile <project_root>`
    // — the shared rooting rule over the full `[paths]` layout. `dev`
    // previously re-rooted on the first `include` entry only, silently
    // dropping further includes and the whole `exclude` list.
    if !compile_once(compiler, project_root, &build_dir, true) {
        return ExitCode::FAILURE;
    }

    // 3. Select the workers — every context by default, or the `--context`
    //    subset (#552, superseding ADR 0096 D3's select-or-default). Serving
    //    them *together* is the whole point: a cross-context call only resolves
    //    when its callee is up too, so an ambiguity error here was the feature
    //    being withheld, not a project being wrong.
    let workers_dir = build_dir.join("workers");
    let available = discover_workers(&workers_dir);
    let workers = match select_contexts(&available, &opts.contexts) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("bynk: {e}");
            return ExitCode::FAILURE;
        }
    };
    let serving = allocate(&workers, opts.base_port, opts);
    // I2 (#1971): over every built Worker, not only the served ones, so a
    // context served here alone still admits a caller served by another
    // `bynk dev --context` session.
    let edge_keys = match crate::deploy::local_edge_keys(&workers_dir, &available) {
        Ok(keys) => keys,
        Err(e) => {
            eprintln!("bynk: {e}");
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
            eprintln!(
                "bynk: `{flag}` is allocated per context — pass `{owner}` to `bynk dev` instead of `-- {flag}`."
            );
            return ExitCode::FAILURE;
        }
    }

    // Now that inspectors are allocated without `--inspect` too (#1954), an
    // overlap with the HTTP range is reachable from plain flags (`--base-port
    // 9229` over two contexts). Name it here rather than leave wrangler to die
    // with `Address already in use`.
    if let Some(port) = port_clash(&serving) {
        eprintln!(
            "bynk: port {port} is allocated both to a context's server and to an inspector — \
             move `--base-port` or `--inspect-port` so the two ranges do not overlap."
        );
        return ExitCode::FAILURE;
    }

    // Remote dev reads the real Cloudflare KV id, unlike Miniflare's local
    // mode. Resolve it from the deploy ledger immediately before Wrangler
    // runs; a never-deployed project gets an actionable error instead of
    // sending the generated placeholder to Cloudflare.
    if opts.wrangler_args.iter().any(|arg| arg == "--remote") {
        for s in &serving {
            if let Err(e) = crate::deploy::materialise_deploy_state(
                project_root,
                &s.worker,
                &workers_dir.join(&s.worker).join("wrangler.toml"),
                &opts.environment,
            ) {
                eprintln!("bynk: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    // Interop I5 (#1972, track §11 Q8): each context's Service Bindings to
    // foreign Workers get a stand-in in its own `wrangler dev`, unless
    // `--local-worker` names the Worker (the dev registry then reaches the
    // user's own `wrangler dev` of it). Under `--remote` the bindings are the
    // account's, so there is nothing to stand in for.
    let remote = opts.wrangler_args.iter().any(|arg| arg == "--remote");
    let mut stand_in_plans: Vec<(String, Vec<StandIn>)> = Vec::new();
    let mut bound_foreign: Vec<String> = Vec::new();
    for s in &serving {
        let config = std::fs::read_to_string(workers_dir.join(&s.worker).join("wrangler.toml"))
            .unwrap_or_default();
        let foreign = foreign_services(&config, &available);
        bound_foreign.extend(foreign.iter().map(|(_, w)| w.clone()));
        let planned = stand_ins(&foreign, &opts.local_workers);
        if !remote && !planned.is_empty() {
            stand_in_plans.push((s.worker.clone(), planned));
        }
    }
    for local in &opts.local_workers {
        if !bound_foreign.contains(local) {
            eprintln!(
                "bynk: warning: `--local-worker {local}` names no Worker the served contexts bind with a `Service` instance."
            );
        }
    }
    if !stand_in_plans.is_empty()
        && (passthrough_has(&opts.wrangler_args, "-c")
            || passthrough_has(&opts.wrangler_args, "--config"))
    {
        eprintln!(
            "bynk: `-c`/`--config` is the driver's when it runs stand-ins for foreign Workers — pass `--local-worker NAME` instead of a config after `--`."
        );
        return ExitCode::FAILURE;
    }
    let stand_in_configs = match write_stand_ins(&build_dir, &stand_in_plans) {
        Ok(configs) => configs,
        Err(e) => {
            eprintln!("bynk: could not write the stand-ins for foreign Workers: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = point_at_stand_ins(&workers_dir, &stand_in_plans) {
        eprintln!("bynk: could not bind the stand-ins for foreign Workers: {e}");
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
    let probe = probe::detect(
        tb,
        "wrangler",
        DetectOpts {
            project_root: Some(project_root),
            allow_npx: true,
        },
    );
    if matches!(probe.provenance, Provenance::Npx) {
        eprintln!("bynk: wrangler resolved via npx — it will download on first run.");
    }
    if let Some(notice) = wrangler_age_notice(&probe) {
        eprintln!("{notice}");
    }
    if matches!(probe.provenance, Provenance::Missing) {
        // The pre-flight gate should have caught this; defensive only.
        eprintln!("bynk: wrangler not found (run `bynk doctor --only deploy`)");
        return ExitCode::FAILURE;
    }

    // #1957: N parallel first-run `npx wrangler dev`s race to install into the
    // same `_npx` cache entry, and the loser dies at boot (ENOTEMPTY). Install
    // it once, up front, so every spawn below finds it already there. Stdout
    // is dropped (the version line is noise); stderr stays, because npm's
    // progress is the only sign a slow first download is not a hang.
    if needs_wrangler_warmup(&probe.provenance, serving.len()) {
        eprintln!("bynk dev: installing wrangler via npx once before starting the contexts…");
        let warmed = wrangler_command(&probe.provenance, "--version").map(|mut cmd| {
            cmd.stdout(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        });
        if warmed != Some(true) {
            // Spawning anyway would re-run the very race this avoids.
            eprintln!(
                "bynk: `npx --yes wrangler@4 --version` failed — wrangler could not be installed via npx."
            );
            return ExitCode::FAILURE;
        }
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
    let mut children: Vec<(String, Served)> = Vec::new();
    for s in &serving {
        let Some(mut cmd) = wrangler_command(&probe.provenance, "dev") else {
            eprintln!("bynk: wrangler not found (run `bynk doctor --only deploy`)");
            terminate(&mut children, &workers_dir);
            return ExitCode::FAILURE;
        };
        cmd.current_dir(workers_dir.join(&s.worker));
        // Interop I5 (#1972): the context's own config first (wrangler's
        // primary Worker, the one served on the port), then its stand-ins.
        if let Some(configs) = stand_in_configs.get(&s.worker) {
            cmd.args(["-c", STAND_IN_CONFIG]);
            for config in configs {
                cmd.arg("-c").arg(config);
            }
        }
        let keys = edge_keys
            .get(&s.worker)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for arg in serve_args(s, keys) {
            cmd.arg(arg);
        }
        for arg in &opts.wrangler_args {
            cmd.arg(arg);
        }
        if opts.format == DevFormat::Json {
            // #2094: stdout is the event stream, so wrangler's banner and
            // request log go to stderr with the rest of the readable output.
            cmd.stdout(std::io::stderr());
        }
        match cmd.spawn() {
            Ok(child) => children.push((s.worker.clone(), Served::new(child))),
            Err(e) => {
                eprintln!("bynk: could not run wrangler for `{}`: {e}", s.worker);
                terminate(&mut children, &workers_dir);
                return ExitCode::FAILURE;
            }
        }
    }
    eprint!("{}", serving_report(&serving));
    eprint!("{}", stand_in_report(&stand_in_plans));
    emit(opts.format, &serving_event(&serving, &opts.wrangler_args));

    // 5. Watch — #524: `bynk dev` is the edit loop, so watch the project's
    // `.bynk` sources (the full `[paths]` layout plus `bynk.toml`) and rebuild
    // into the same build dir on change. Each `wrangler dev` watches its own
    // built worker files, so one rebuild hot-reloads every context that changed
    // without a restart; a failing rebuild renders diagnostics and keeps both
    // the watch and the last good build serving. std-only mtime polling
    // (500ms): no native watcher dependency, and an edit-loop latency well
    // under a keystroke-to-glance.
    eprintln!("bynk dev: watching for source changes (edit `.bynk` files to rebuild)");
    let mut fingerprint = watch_fingerprint(project_root);
    let code = supervise(&mut children, &workers_dir, || {
        let now = watch_fingerprint(project_root);
        if now != fingerprint {
            fingerprint = now;
            eprintln!("bynk dev: change detected — rebuilding…");
            emit(opts.format, &DevEvent::Rebuilding);
            if compile_once(compiler, project_root, &build_dir, true) {
                // Interop I5 (#1972): refresh the served copy from the
                // rebuilt config. A binding added since start has no
                // stand-in until `bynk dev` restarts.
                if let Err(e) = point_at_stand_ins(&workers_dir, &stand_in_plans) {
                    eprintln!("bynk: could not bind the stand-ins for foreign Workers: {e}");
                }
                eprintln!("bynk dev: rebuilt");
                emit(opts.format, &DevEvent::RebuildOk);
            } else {
                // The diagnostics are already rendered; keep serving the last
                // good build and keep watching.
                emit(opts.format, &DevEvent::RebuildFailed);
            }
        }
    });
    emit(opts.format, &DevEvent::Stopped { code });
    ExitCode::from(code)
}

/// The watch loop: every 500ms, end the session if it was asked to stop or a
/// worker exited, and otherwise run `tick` (the rebuild check). Returns the
/// session's exit byte once every child is stopped and reaped.
fn supervise(
    children: &mut Vec<(String, Served)>,
    workers_dir: &Path,
    mut tick: impl FnMut(),
) -> u8 {
    loop {
        // #1955: a SIGINT/SIGTERM recorded by `stop::install`. Checked before
        // the children, so a Ctrl-C — which reaches the wranglers too — reads
        // as the stop it is rather than as a worker exiting.
        if let Some(sig) = crate::stop::received() {
            eprintln!(
                "bynk dev: received {} — stopping the contexts.",
                crate::stop::name(sig)
            );
            terminate(children, workers_dir);
            return crate::stop::exit_byte(sig);
        }
        // Any worker exiting ends the session: the survivors' bindings now
        // point at a context that is gone, so a half-served project would fail
        // in a way that looks like a code bug. Stop them and propagate the
        // first exit code.
        for i in 0..children.len() {
            let status = match children[i].1.child.try_wait() {
                Ok(status) => status,
                Err(e) => {
                    eprintln!("bynk: could not poll wrangler: {e}");
                    terminate(children, workers_dir);
                    return 1;
                }
            };
            if let Some(status) = status {
                // Dropped here. On Windows that stops its job (#1762), and
                // with it the `workerd`s the exited wrangler left running.
                let (name, _) = children.remove(i);
                if !children.is_empty() {
                    eprintln!("bynk dev: `{name}` exited — stopping the other contexts.");
                }
                terminate(children, workers_dir);
                return exit_status_byte(&status);
            }
        }
        std::thread::sleep(Duration::from_millis(500));
        tick();
    }
}

/// One `wrangler dev` that `bynk dev` is serving: the child it spawned and, on
/// Windows, the job object that holds the child's whole process tree (#1762).
struct Served {
    child: std::process::Child,
    /// `None` when the child could not be put in a job. It is then stopped
    /// with `Child::kill` alone, as before #1762, which leaves its descendants
    /// running.
    #[cfg(windows)]
    job: Option<crate::job::Job>,
}

impl Served {
    fn new(child: std::process::Child) -> Served {
        #[cfg(windows)]
        {
            let job = match crate::job::Job::assign(&child) {
                Ok(job) => Some(job),
                Err(e) => {
                    eprintln!(
                        "bynk dev: could not put wrangler in a job object ({e}); \
                         stopping it may leave its workerd processes running."
                    );
                    None
                }
            };
            Served { child, job }
        }
        #[cfg(not(windows))]
        Served { child }
    }
}

/// Stop every remaining `wrangler dev` and reap it, so a session that ends on
/// one worker's exit does not strand the others — each holds a port and a
/// `workerd` child, and a stranded one makes the *next* `bynk dev` fail on a
/// port clash. Signal them all first, then reap, so the shutdowns overlap.
///
/// Then [`sweep`](crate::sweep::sweep) what is still running under
/// `workers_dir` (#1742). Signalling the children is not enough: via npx the
/// child is `npx`, which does not pass the signal on, and a worker that exited
/// on its own has already orphaned its `workerd`s. On Windows the sweep does
/// nothing, and each child's job object stops its tree instead (#1762).
fn terminate(children: &mut Vec<(String, Served)>, workers_dir: &Path) {
    for (_, served) in children.iter_mut() {
        request_stop(served);
    }
    for (_, served) in children.iter_mut() {
        reap(&mut served.child);
    }
    children.clear();
    crate::sweep::sweep(workers_dir, STOP_GRACE);
}

/// Ask one `wrangler dev` to stop **and take its own process tree with it**.
///
/// SIGTERM, not [`std::process::Child::kill`]'s SIGKILL: wrangler traps SIGTERM
/// and tears down the `node` and `workerd` processes it spawned, whereas SIGKILL
/// is untrappable — verified, a SIGKILLed wrangler strands an orphaned `workerd
/// serve` still holding the port. std exposes no SIGTERM, so we go through POSIX
/// `kill(1)`.
///
/// On Windows the child is a `cmd.exe` wrapper, and `Child::kill` stops only
/// that. Terminating the child's job object stops the whole tree instead
/// (#1762). It is a hard stop, but inside a job nothing is stranded by one.
fn request_stop(served: &mut Served) {
    #[cfg(windows)]
    if let Some(job) = &served.job {
        job.terminate();
        return;
    }
    let child = &mut served.child;
    #[cfg(unix)]
    {
        let sent = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status()
            .is_ok_and(|s| s.success());
        if sent {
            return;
        }
        // `kill` missing or the process already gone — fall through.
    }
    let _ = child.kill();
}

/// How long a stopping wrangler gets to run its own teardown before SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// Reap a signalled child, giving it a moment to run wrangler's own teardown
/// before escalating to SIGKILL. Without the escalation a wrangler wedged in
/// shutdown would hang `bynk dev` forever; without the grace period we would be
/// back to stranding `workerd`.
fn reap(child: &mut std::process::Child) {
    const TICK: Duration = Duration::from_millis(50);
    let mut waited = Duration::ZERO;
    while waited < STOP_GRACE {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(_) => break,
        }
        std::thread::sleep(TICK);
        waited += TICK;
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// #524: a change fingerprint over the project's watched inputs — every
/// `.bynk` file under the `[paths] include` roots (author `exclude` subtrees
/// and tool/VCS directories skipped), `bynk.toml` itself, and (#1883, slice
/// 1b) every file in the project's `assets/` folder, so editing a stylesheet
/// rebuilds and rehashes it. Hashes each
/// file's path, mtime, and length, so an edit, add, delete, or rename all
/// change the fingerprint. I/O errors skip the entry rather than aborting the
/// watch.
fn watch_fingerprint(project_root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    // The watch loop's own fingerprint has no error channel to surface a
    // malformed `bynk.toml` through — the CLI's real build/check paths
    // (`bynk-driver::project_options`) already do that, via
    // `try_read_project_paths_with`. This falls back to the conventional
    // layout on any error, same as `read_project_paths`'s deleted total form
    // (R3.8, #1113) always did for this one call site.
    let paths = try_read_project_paths(project_root)
        .unwrap_or_else(|_| ProjectPaths::conventional(project_root));
    let excludes: Vec<PathBuf> = paths.exclude.iter().map(|e| project_root.join(e)).collect();
    let mut entries: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();
    let record = |path: &Path, entries: &mut Vec<(PathBuf, std::time::SystemTime, u64)>| {
        if let Ok(meta) = std::fs::metadata(path)
            && let Ok(mtime) = meta.modified()
        {
            entries.push((path.to_path_buf(), mtime, meta.len()));
        }
    };
    record(&project_root.join("bynk.toml"), &mut entries);
    for root in &paths.include {
        collect_bynk_files(&project_root.join(root), &excludes, &mut |p| {
            record(p, &mut entries)
        });
    }
    if let Ok(assets) = bynk_emit::project::assets::list_asset_files(project_root) {
        for rel in assets {
            record(&project_root.join(rel), &mut entries);
        }
    }
    entries.sort();
    let mut hasher = std::hash::DefaultHasher::new();
    for (path, mtime, len) in &entries {
        path.hash(&mut hasher);
        mtime.hash(&mut hasher);
        len.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod watch_tests {
    use super::*;

    /// #1883, slice 1b: an edit to a file in `assets/` changes the
    /// fingerprint, so `bynk dev` rebuilds and the `.url` hash moves with it.
    #[test]
    fn an_asset_edit_changes_the_watch_fingerprint() {
        let root = std::env::temp_dir().join(format!("bynk-dev-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("src/a.bynk"), "commons a\n").unwrap();
        std::fs::write(root.join("assets/board.css"), "a{}").unwrap();
        let before = watch_fingerprint(&root);
        std::fs::write(root.join("assets/board.css"), "a{color:red}").unwrap();
        let after = watch_fingerprint(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_ne!(before, after, "an asset edit must trigger a rebuild");
    }
}

/// Walk `dir` recursively, calling `visit` for each `.bynk` file. Skips the
/// author `exclude` subtrees and the tool/VCS directories a source walk never
/// wants (`.bynk` build dir, `.git`, `node_modules`, `target`).
fn collect_bynk_files(dir: &Path, excludes: &[PathBuf], visit: &mut dyn FnMut(&Path)) {
    const SKIP_DIRS: [&str; 4] = [".bynk", ".git", "node_modules", "target"];
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            if SKIP_DIRS.iter().any(|s| name == *s) {
                continue;
            }
            if excludes.iter().any(|e| path.starts_with(e)) {
                continue;
            }
            collect_bynk_files(&path, excludes, visit);
        } else if path.extension().is_some_and(|e| e == "bynk") {
            visit(&path);
        }
    }
}

/// #1732: the warning `bynk dev` prints before serving with a wrangler older
/// than [`bynk_emit::WRANGLER_MIN`]. Its `workerd` refuses the pinned
/// compatibility date outright, and wrangler's own error says nothing about
/// Bynk, so this names the cause and the fix first. A warning, not a refusal:
/// the version is `doctor`'s judgement, and the wrangler itself has the last
/// word. `None` when the wrangler is new enough, or can't be versioned (npx).
pub fn wrangler_age_notice(probe: &probe::Probe) -> Option<String> {
    let v = probe
        .version
        .filter(|_| doctor::wrangler_below_min(probe))?;
    Some(format!(
        "bynk: warning: wrangler {v} is older than {}, the first whose runtime serves \
         compatibility date {}; `wrangler dev` will refuse it. Upgrade with \
         `{}`.",
        bynk_emit::WRANGLER_MIN,
        bynk_emit::COMPATIBILITY_DATE,
        doctor::wrangler_upgrade_remedy(probe)
    ))
}

/// The text `bynk dev` prints when the deploy pre-flight fails: a lead line plus
/// doctor's own human report, so the remedy lines are identical to `bynk
/// doctor`. Pure (no I/O) so this deterministic surface is pinned by a golden
/// (§5), unlike the non-deterministic `wrangler dev` stream.
pub fn preflight_failure_message(report: &Report) -> String {
    format!(
        "bynk: environment not ready for `dev` — see below.\n\n{}",
        report::render(report, Format::Human)
    )
}

/// Allocate a port per worker (#552): `wrangler dev` binds one port per process,
/// so serving N contexts means N distinct ports, assigned `base + i` over the
/// deterministic worker order.
///
/// The one exception preserves the pre-#552 contract: a **lone** worker with no
/// explicit `--base-port` gets `None` — no injected `--port` at all — so it
/// lands on wrangler's own default and `-- --port N` still works. Injecting
/// unconditionally would break that, because a repeated `--port` is a hard
/// wrangler error, not last-wins.
///
/// Inspector ports follow the same shape, from `--inspect-port`. They are
/// allocated under `--inspect`, and also whenever more than one worker is
/// served (#1954): `wrangler dev` always starts its inspector, on 9229 unless
/// told otherwise, so N wranglers started together all bind 9229 and every one
/// but the first dies with `Address already in use`. A lone worker without
/// `--inspect` gets none, keeping its invocation byte-for-byte as before.
pub fn allocate(workers: &[String], base_port: Option<u16>, opts: &DevOptions) -> Vec<Serving> {
    let lone = workers.len() == 1 && base_port.is_none();
    let base = base_port.unwrap_or(DEFAULT_BASE_PORT);
    let inspectors = opts.inspect || workers.len() > 1;
    workers
        .iter()
        .enumerate()
        .map(|(i, worker)| Serving {
            worker: worker.clone(),
            port: (!lone).then(|| base.saturating_add(i as u16)),
            inspector_port: inspectors.then(|| opts.inspect_port.saturating_add(i as u16)),
            inspect: opts.inspect,
        })
        .collect()
}

/// A port the allocation hands out twice: one context's HTTP port that is also
/// some context's inspector port.
fn port_clash(serving: &[Serving]) -> Option<u16> {
    serving
        .iter()
        .filter_map(|s| s.port)
        .find(|p| serving.iter().any(|s| s.inspector_port == Some(*p)))
}

/// #1957: whether to install wrangler once before spawning the contexts. Only
/// an npx resolution installs on first use, and only several spawns at once
/// race each other in npm's `_npx` cache; a lone context installs alone.
pub fn needs_wrangler_warmup(provenance: &Provenance, contexts: usize) -> bool {
    matches!(provenance, Provenance::Npx) && contexts > 1
}

/// Whether the `--` passthrough carries `flag`, which the driver also injects.
/// Wrangler rejects a repeated `--port`/`--inspector-port` outright ("expects a
/// single value, but received multiple"), so we catch the clash ourselves and
/// say which driver flag owns it instead of letting wrangler's usage dump land.
fn passthrough_has(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|a| a == flag || a.starts_with(&format!("{flag}=")))
}

/// The `wrangler dev` flags the driver injects for one worker: the ports it
/// allocated (#552) — `--port` when serving several contexts, `--inspector-port`
/// under `--inspect` (slice 3, ADR 0104), so a JavaScript debugger can attach and
/// `.bynk` breakpoints resolve through the emitted source maps.
///
/// A lone worker without `--base-port` or `--inspect` gets no port flag — the
/// pre-#552 invocation, so that path keeps its `-- --port N` passthrough.
///
/// I2 (#1971): and the local key for each edge the Worker is on (`edge_keys`,
/// its [`crate::deploy::local_edge_keys`]), as `--var`s rather than a file:
/// every internal request (`/_bynk/call/`, `/_bynk/event/`) is signed under
/// its edge's key, and a lone context still delivers its own events to
/// itself. The fixed [`bynk_emit::LOCAL_CALL_KEY`], so two `bynk dev
/// --context` sessions run side by side admit each other's calls.
fn serve_args(s: &Serving, edge_keys: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    for key in edge_keys {
        args.push("--var".to_string());
        args.push(format!("{key}:{}", bynk_emit::LOCAL_CALL_KEY));
    }
    if let Some(port) = s.port {
        args.push("--port".to_string());
        args.push(port.to_string());
    }
    if let Some(port) = s.inspector_port {
        args.push("--inspector-port".to_string());
        args.push(port.to_string());
    }
    args
}

/// The start-up report: which context answers on which URL, plus the inspector
/// notice under `--inspect`. Pure and deterministic, so it is golden-pinned in
/// the style of ADR 0096 §Exit — unlike the `wrangler dev` streams it precedes.
///
/// A lone worker on wrangler's own default port prints no table: there is no
/// allocation to disclose and wrangler announces its own `Ready on` line, so
/// that session reads exactly as it did before #552.
pub fn serving_report(serving: &[Serving]) -> String {
    let mut out = String::new();
    if serving.iter().any(|s| s.port.is_some()) {
        // Only claim the wiring when there is something to wire: a subset of
        // one has no sibling to bind to, and saying otherwise would explain a
        // cross-context call's failure as a bug rather than as the missing
        // context it is.
        out.push_str(&match serving.len() {
            1 => "bynk dev: serving 1 context.\n".to_string(),
            n => format!(
                "bynk dev: serving {n} contexts — service bindings between them are wired.\n"
            ),
        });
        let width = serving.iter().map(|s| s.worker.len()).max().unwrap_or(0);
        for s in serving {
            let Some(port) = s.port else { continue };
            out.push_str(&format!(
                "  {:width$}  http://localhost:{port}\n",
                s.worker,
                width = width
            ));
        }
    }
    // Only `--inspect` announces the inspector. Without it a multi-context
    // session still allocates inspector ports (#1954), but only so they do not
    // collide; that is wrangler's default inspector, moved, not a debug session.
    let inspected = serving
        .iter()
        .filter(|s| s.inspect && s.inspector_port.is_some())
        .count();
    if inspected > 0 {
        out.push_str(&match inspected {
            1 => "bynk dev --inspect: the worker runs with the V8 inspector enabled.\n".to_string(),
            _ => "bynk dev --inspect: each worker runs with the V8 inspector enabled, on its own port.\n"
                .to_string(),
        });
        for s in serving {
            let (true, Some(port)) = (s.inspect, s.inspector_port) else {
                continue;
            };
            out.push_str(&format!(
                "  {} — inspector on port {port} (CDP discovery: http://127.0.0.1:{port}/json)\n",
                s.worker
            ));
        }
        out.push_str(
            "  Breakpoints set in `.bynk` sources resolve through the emitted source maps.\n\
             \x20 A hand-rolled CDP client must send an `Origin` header — VS Code's\n\
             \x20 JavaScript debugger does this for you.\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
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

    #[test]
    fn exit_status_byte_maps_codes_and_signals() {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            use std::process::ExitStatus;
            // Wait statuses: exit codes sit in the high byte; the low byte is
            // the terminating signal.
            assert_eq!(exit_status_byte(&ExitStatus::from_raw(0)), 0);
            assert_eq!(exit_status_byte(&ExitStatus::from_raw(1 << 8)), 1);
            // A shared Ctrl-C (SIGINT = 2) is a clean stop…
            assert_eq!(exit_status_byte(&ExitStatus::from_raw(2)), 0);
            // …but a SIGSEGV (11) or SIGKILL (9, the OOM killer) is a real
            // failure — previously these read as passing in CI.
            assert_eq!(exit_status_byte(&ExitStatus::from_raw(11)), 128 + 11);
            assert_eq!(exit_status_byte(&ExitStatus::from_raw(9)), 128 + 9);
        }
    }

    /// I2 (#1971): a Worker's edge keys come first, each the local key.
    #[test]
    fn every_edge_key_is_passed_as_the_local_key() {
        let serving = allocate(&names(&["api"]), None, &DevOptions::default());
        assert_eq!(
            serve_args(
                &serving[0],
                &names(&["BYNK_CALL_FROM_WORKER", "BYNK_CALL_TO_WORKER"])
            ),
            names(&[
                "--var",
                "BYNK_CALL_FROM_WORKER:bynk-local-call-key",
                "--var",
                "BYNK_CALL_TO_WORKER:bynk-local-call-key",
            ])
        );
    }

    #[test]
    fn inspect_injects_the_inspector_port() {
        let off = DevOptions::default();
        let lone = allocate(&names(&["links"]), None, &off);
        assert_eq!(
            serve_args(&lone[0], &[]),
            Vec::<String>::new(),
            "a lone worker without --inspect keeps the pre-#552 invocation"
        );

        let on = DevOptions {
            inspect: true,
            inspect_port: 9229,
            ..Default::default()
        };
        let lone = allocate(&names(&["links"]), None, &on);
        assert_eq!(
            serve_args(&lone[0], &[]),
            names(&["--inspector-port", "9229"])
        );
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

    // ---- #552: port allocation --------------------------------------------

    #[test]
    fn ports_are_allocated_per_context_from_the_base() {
        let opts = DevOptions {
            inspect: true,
            inspect_port: 9229,
            ..Default::default()
        };
        let serving = allocate(&names(&["api", "worker"]), None, &opts);
        assert_eq!(
            serving.iter().map(|s| s.port).collect::<Vec<_>>(),
            vec![Some(8787), Some(8788)],
            "each wrangler dev binds its own port"
        );
        assert_eq!(
            serving.iter().map(|s| s.inspector_port).collect::<Vec<_>>(),
            vec![Some(9229), Some(9230)],
            "inspector ports must not collide either"
        );
        assert_eq!(
            serve_args(&serving[1], &[]),
            names(&["--port", "8788", "--inspector-port", "9230"])
        );
    }

    #[test]
    fn base_port_moves_the_whole_allocation() {
        let serving = allocate(&names(&["a", "b"]), Some(9000), &DevOptions::default());
        assert_eq!(
            serving.iter().map(|s| s.port).collect::<Vec<_>>(),
            vec![Some(9000), Some(9001)]
        );
    }

    #[test]
    fn a_lone_worker_keeps_wranglers_own_port() {
        // The back-compat contract: no injected --port, so `-- --port N` still
        // reaches wrangler (a repeated --port is a hard error, not last-wins).
        let serving = allocate(&names(&["links"]), None, &DevOptions::default());
        assert_eq!(serving[0].port, None);
        assert_eq!(serve_args(&serving[0], &[]), Vec::<String>::new());
        // …but an explicit --base-port is honoured even for one context.
        let pinned = allocate(&names(&["links"]), Some(8900), &DevOptions::default());
        assert_eq!(pinned[0].port, Some(8900));
    }

    #[test]
    fn several_contexts_get_distinct_inspector_ports_without_inspect() {
        // #1954: wrangler's inspector is always on, so two wranglers left on
        // its default both bind 9229 and the second dies at boot.
        let plain = DevOptions {
            inspect_port: 9229, // the CLI default
            ..Default::default()
        };
        let serving = allocate(&names(&["api", "worker"]), None, &plain);
        assert_eq!(
            serving.iter().map(|s| s.inspector_port).collect::<Vec<_>>(),
            vec![Some(9229), Some(9230)]
        );
        assert_eq!(
            serve_args(&serving[1], &[]),
            names(&["--port", "8788", "--inspector-port", "9230"])
        );
        // `--inspect-port` moves that allocation even without `--inspect`…
        let moved = DevOptions {
            inspect_port: 9300,
            ..Default::default()
        };
        let serving = allocate(&names(&["api", "worker"]), None, &moved);
        assert_eq!(
            serving.iter().map(|s| s.inspector_port).collect::<Vec<_>>(),
            vec![Some(9300), Some(9301)]
        );
        // …and a lone worker without `--inspect` still gets none.
        let lone = allocate(&names(&["links"]), Some(8900), &DevOptions::default());
        assert_eq!(lone[0].inspector_port, None);
    }

    #[test]
    fn an_allocated_inspector_is_announced_only_under_inspect() {
        let report = serving_report(&allocate(
            &names(&["api", "worker"]),
            None,
            &DevOptions::default(),
        ));
        assert!(!report.contains("inspector"), "{report}");
    }

    #[test]
    fn overlapping_http_and_inspector_ranges_are_caught() {
        let opts = DevOptions {
            inspect_port: 9229,
            ..Default::default()
        };
        let two = names(&["api", "worker"]);
        // HTTP 9228/9229 against inspectors 9229/9230.
        assert_eq!(port_clash(&allocate(&two, Some(9228), &opts)), Some(9229));
        assert_eq!(port_clash(&allocate(&two, None, &opts)), None);
        // A lone worker has neither, so nothing can clash.
        assert_eq!(port_clash(&allocate(&names(&["links"]), None, &opts)), None);
    }

    #[test]
    fn npx_is_warmed_up_only_for_several_contexts() {
        // #1957: parallel first-run `npx` installs race in npm's cache.
        assert!(needs_wrangler_warmup(&Provenance::Npx, 2));
        assert!(!needs_wrangler_warmup(&Provenance::Npx, 1));
        let installed = Provenance::Path(PathBuf::from("/usr/bin/wrangler"));
        assert!(!needs_wrangler_warmup(&installed, 2));
    }

    /// #1955: a SIGTERM sent to `bynk dev` alone stops its contexts and what
    /// they started. `sh` stands in for wrangler and its `sleep`s for the
    /// `workerd`s: the `sh` dies of the SIGTERM `terminate` sends and orphans
    /// them, as a wrangler resolved via npx does, and the sweep must stop them.
    #[cfg(unix)]
    #[test]
    fn a_sigterm_to_the_driver_stops_every_context_and_its_children() {
        use std::process::{Command, Stdio};
        // #2055: the deploy tests that change signal dispositions must not
        // restore the default while this one signals the process.
        let _lock = crate::stop::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("bynk-dev-sigterm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut children = Vec::new();
        for ctx in ["api", "worker"] {
            let dir = root.join(ctx);
            std::fs::create_dir_all(&dir).unwrap();
            let child = Command::new("sh")
                .args(["-c", "sleep 300 & sleep 300 & wait"])
                .current_dir(&dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("sh spawns");
            children.push((ctx.to_string(), Served::new(child)));
        }
        let started = std::time::Instant::now();
        while crate::sweep::stragglers(&root).len() < 6 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the stand-ins never started their sleeps"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        // What a CI step's `kill -TERM <bynk dev>` does. Without the handler
        // this would end the test binary.
        crate::stop::install();
        let sent = Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status()
            .is_ok_and(|s| s.success());
        assert!(sent, "kill(1) runs");
        let started = std::time::Instant::now();
        while crate::stop::received().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the SIGTERM was never recorded"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let code = supervise(&mut children, &root, || {
            panic!("a recorded stop must end the session before any rebuild")
        });
        crate::stop::reset();
        let left = crate::sweep::stragglers(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(code, 143, "a SIGTERM stop exits 128 + 15");
        assert!(children.is_empty());
        assert!(left.is_empty(), "processes survived the stop: {left:?}");
    }

    #[test]
    fn passthrough_port_is_detected_in_both_spellings() {
        assert!(passthrough_has(&names(&["--port", "8788"]), "--port"));
        assert!(passthrough_has(&names(&["--port=8788"]), "--port"));
        assert!(!passthrough_has(&names(&["--remote"]), "--port"));
        // `--inspector-port` must not be mistaken for `--port`.
        assert!(!passthrough_has(
            &names(&["--inspector-port", "9229"]),
            "--port"
        ));
    }

    #[test]
    fn the_serving_report_lists_context_urls() {
        let serving = allocate(
            &names(&["commerce-orders", "commerce-payment"]),
            None,
            &DevOptions::default(),
        );
        let report = serving_report(&serving);
        assert!(report.contains("serving 2 contexts"), "{report}");
        // Names are padded to the widest, so the URLs line up in a column.
        assert!(
            report.contains("commerce-orders   http://localhost:8787"),
            "{report}"
        );
        assert!(
            report.contains("commerce-payment  http://localhost:8788"),
            "{report}"
        );
        // A lone default-port worker discloses no allocation — wrangler's own
        // `Ready on` line is the announcement, exactly as before #552.
        assert_eq!(
            serving_report(&allocate(&names(&["links"]), None, &DevOptions::default())),
            ""
        );
    }

    #[test]
    fn the_serving_event_reports_every_port() {
        let serving = allocate(
            &names(&["commerce-orders", "commerce-payment"]),
            None,
            &DevOptions {
                inspect_port: 9229,
                ..Default::default()
            },
        );
        assert_eq!(
            event_line(&serving_event(&serving, &[])),
            r#"{"event":"serving","contexts":[{"name":"commerce-orders","port":8787,"url":"http://localhost:8787","inspector_port":9229},{"name":"commerce-payment","port":8788,"url":"http://localhost:8788","inspector_port":9230}]}"#
        );
        // A lone worker on wrangler's default still reports a port: 8787, or a
        // `-- --port N` passthrough in either spelling.
        let lone = allocate(&names(&["links"]), None, &DevOptions::default());
        let port = |args: &[&str]| match serving_event(&lone, &names(args)) {
            DevEvent::Serving { contexts } => (contexts[0].port, contexts[0].inspector_port),
            other => panic!("{other:?}"),
        };
        assert_eq!(port(&[]), (8787, None));
        assert_eq!(port(&["--port", "9000"]), (9000, None));
        assert_eq!(port(&["--remote", "--port=9001"]), (9001, None));
    }

    #[test]
    fn lifecycle_events_are_tagged_lines() {
        assert_eq!(
            event_line(&DevEvent::Rebuilding),
            r#"{"event":"rebuilding"}"#
        );
        assert_eq!(
            event_line(&DevEvent::RebuildOk),
            r#"{"event":"rebuild_ok"}"#
        );
        assert_eq!(
            event_line(&DevEvent::RebuildFailed),
            r#"{"event":"rebuild_failed"}"#
        );
        assert_eq!(
            event_line(&DevEvent::Stopped { code: 130 }),
            r#"{"event":"stopped","code":130}"#
        );
    }

    #[test]
    fn a_subset_of_one_claims_no_wiring() {
        // "bindings between them are wired" would be a lie for a single
        // context, and a misleading one: it invites reading a cross-context
        // call's failure as a bug rather than as the context left unserved.
        let report = serving_report(&allocate(
            &names(&["commerce-payment"]),
            Some(8890),
            &DevOptions::default(),
        ));
        assert!(report.contains("serving 1 context."), "{report}");
        assert!(!report.contains("bindings"), "{report}");
    }

    // ---- interop I5 (#1972): stand-ins for foreign Workers ----

    const BILLING_CONFIG: &str = "name = \"shop-billing\"\n\n[[services]]\nbinding = \"SHOP_FRONT\"\nservice = \"shop-front\"\n\n[[services]]\nbinding = \"SERVICE_LEDGER\"\nservice = \"ledger\"\n\n[[services]]\nbinding = \"SERVICE_BILLING\"\nservice = \"legacy-billing\"\n\n[[services]]\nbinding = \"SERVICE_INVOICES\"\nservice = \"legacy-billing\"\n";

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn foreign_services_are_the_bindings_to_workers_outside_the_build() {
        assert_eq!(
            foreign_services(BILLING_CONFIG, &names(&["shop-billing", "shop-front"])),
            pairs(&[
                ("SERVICE_BILLING", "legacy-billing"),
                ("SERVICE_INVOICES", "legacy-billing"),
                ("SERVICE_LEDGER", "ledger"),
            ])
        );
        assert!(foreign_services("not toml [", &[]).is_empty());
    }

    /// One stand-in per foreign Worker, carrying every binding to it, less
    /// the Workers `--local-worker` names.
    #[test]
    fn stand_ins_are_one_per_foreign_worker_less_the_local_ones() {
        let foreign = foreign_services(BILLING_CONFIG, &names(&["shop-front"]));
        assert_eq!(
            stand_ins(&foreign, &[]),
            [
                StandIn {
                    worker: "ledger".into(),
                    bindings: names(&["SERVICE_LEDGER"]),
                },
                StandIn {
                    worker: "legacy-billing".into(),
                    bindings: names(&["SERVICE_BILLING", "SERVICE_INVOICES"]),
                },
            ]
        );
        assert_eq!(
            stand_ins(&foreign, &names(&["legacy-billing"])),
            [StandIn {
                worker: "ledger".into(),
                bindings: names(&["SERVICE_LEDGER"]),
            }]
        );
    }

    /// The stand-in runs under a name of its own, never the foreign Worker's
    /// (which wrangler's shared dev registry would hand to other sessions), and
    /// says what it is.
    #[test]
    fn a_stand_in_is_named_for_its_worker_and_answers_503() {
        let (config, script) = stand_in_files(
            "shop-billing",
            &StandIn {
                worker: "legacy-billing".into(),
                bindings: names(&["SERVICE_BILLING"]),
            },
        );
        let doc: toml::Table = config.parse().expect("valid TOML");
        assert_eq!(
            doc["name"].as_str(),
            Some("bynk-stand-in-shop-billing-legacy-billing")
        );
        assert_eq!(doc["main"].as_str(), Some("index.js"));
        assert_eq!(
            doc["compatibility_date"].as_str(),
            Some(bynk_emit::emitter::wrangler::COMPATIBILITY_DATE)
        );
        assert!(script.contains("status: 503"), "{script}");
        assert!(
            script.contains("`--local-worker legacy-billing`"),
            "{script}"
        );
        assert!(script.contains("`SERVICE_BILLING`"), "{script}");
    }

    #[test]
    fn a_stand_in_name_fits_a_worker_name() {
        assert_eq!(
            stand_in_name("shop-billing", "legacy-billing"),
            "bynk-stand-in-shop-billing-legacy-billing"
        );
        let long_context = "commerce-fulfilment-warehouse-allocation";
        let a = stand_in_name(long_context, "legacy-billing-reconciliation");
        let b = stand_in_name(long_context, "legacy-billing-reconciliation-eu");
        assert!(a.len() <= 63 && b.len() <= 63, "{a} / {b}");
        assert_ne!(a, b);
        assert_eq!(
            a,
            stand_in_name(long_context, "legacy-billing-reconciliation")
        );
        assert!(!a.contains("--"), "{a}");
    }

    /// The served copy points the binding at the stand-in; the generated
    /// config keeps the real name (review of #2008).
    #[test]
    fn the_stand_in_config_is_a_retargeted_copy() {
        let dir = std::env::temp_dir().join(format!("bynk-stand-in-copy-{}", std::process::id()));
        let ctx = dir.join("shop-billing");
        std::fs::create_dir_all(&ctx).unwrap();
        let generated = "name = \"shop-billing\"\n\n[[services]]\nbinding = \"SERVICE_BILLING\"\nservice = \"legacy-billing\"\n";
        std::fs::write(ctx.join("wrangler.toml"), generated).unwrap();
        let plans = vec![(
            "shop-billing".to_string(),
            vec![StandIn {
                worker: "legacy-billing".into(),
                bindings: names(&["SERVICE_BILLING"]),
            }],
        )];
        point_at_stand_ins(&dir, &plans).unwrap();
        assert_eq!(
            std::fs::read_to_string(ctx.join("wrangler.toml")).unwrap(),
            generated
        );
        let served = std::fs::read_to_string(ctx.join(STAND_IN_CONFIG)).unwrap();
        assert!(
            served.contains("service = \"bynk-stand-in-shop-billing-legacy-billing\""),
            "{served}"
        );
        assert!(!ctx.join(format!("{STAND_IN_CONFIG}.tmp")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_stand_in_report_names_each_and_the_way_out() {
        assert_eq!(stand_in_report(&[]), "");
        let report = stand_in_report(&[(
            "shop-billing".into(),
            vec![StandIn {
                worker: "legacy-billing".into(),
                bindings: names(&["SERVICE_BILLING"]),
            }],
        )]);
        assert_eq!(
            report,
            "bynk dev: `shop-billing` binds Worker `legacy-billing` (SERVICE_BILLING) to a local stand-in that answers 503; pass `--local-worker legacy-billing` to reach a `wrangler dev` of it instead.\n"
        );
    }
}
