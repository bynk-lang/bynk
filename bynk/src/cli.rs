//! The `bynk` driver command-line interface.
//!
//! The developer front-end: `doctor` / `new` / `dev`, plus the everyday
//! `check` / `fmt` / `test` (v0.138, #487). `check` and `fmt` run the linked
//! pipeline in-process; `test` delegates to the driver-resolved `bynkc`. The
//! flag surfaces mirror `bynkc`'s so the two are drop-in equivalent.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::doctor::{Capability, DoctorOptions};
use crate::report::Format;

#[derive(Parser, Debug)]
#[command(name = "bynk", version, about = "The Bynk driver", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Check whether your machine is ready to compile, test, and deploy Bynk —
    /// and print the exact remedy for anything missing.
    ///
    /// Bare `bynk doctor` is informational: it surveys every capability and
    /// exits 0 unless `bynkc` itself is unusable. `--only <capability>` gates on
    /// one capability (exits non-zero if its tools are missing); `--strict`
    /// turns every warning into a failure, for CI.
    Doctor {
        /// Project directory to inspect (for project-local `node_modules/.bin`
        /// resolution). Defaults to the current directory.
        #[arg(default_value = ".")]
        input: PathBuf,
        /// Scope the check — and the exit code — to one capability.
        #[arg(long, value_enum)]
        only: Option<CapabilityArg>,
        /// Treat every warning (optional gaps, npx provisionability, minor
        /// version skew) as a failure. For an all-green CI gate.
        #[arg(long)]
        strict: bool,
        /// Output format. `human` (default) is a grouped table; `short` and
        /// `json` are the stable scriptable surface.
        #[arg(long, value_enum, default_value = "human")]
        format: FormatArg,
    },
    /// Build the project and serve it locally with `wrangler dev`, rebuilding
    /// on save — one step in place of the manual compile + `cd` + `wrangler
    /// dev` recipe.
    ///
    /// Compiles into a managed `.bynk/dev/` build dir and runs one `wrangler
    /// dev` per context from inside its worker dir, in local mode (Miniflare) —
    /// no namespace provisioning needed. Every context is served by default and
    /// the service bindings between them are wired (#552), so a cross-context
    /// call resolves locally; `--context` narrows to a subset. While serving,
    /// `.bynk` sources are watched (#524): saving a file rebuilds in place and
    /// the running workers hot-reload; a failing rebuild reports errors and
    /// keeps serving the last good build. Everything after `--` is forwarded to
    /// `wrangler dev` verbatim.
    Dev {
        /// Project directory to serve from (anywhere inside the project; the
        /// root is found by walking up for `bynk.toml`). Defaults to `.`.
        #[arg(default_value = ".")]
        path: PathBuf,
        #[command(flatten)]
        lock: LockArgs,
        /// Which context to serve, repeatable. Omit to serve every context in
        /// the project with the service bindings between them wired (#552);
        /// pass one or more to narrow to a subset. Accepts the dotted name or
        /// its dasherised worker-dir form.
        #[arg(long = "context", value_name = "NAME")]
        contexts: Vec<String>,
        /// First port of the per-context allocation (context *i* gets
        /// `--base-port` + *i*, in sorted order). Defaults to wrangler's 8787.
        /// A single context left on the default keeps `-- --port N` working.
        #[arg(long, value_name = "PORT")]
        base_port: Option<u16>,
        /// Serve with the V8 inspector enabled (slice 3, ADR 0104): `wrangler dev`
        /// starts with `--inspector-port` so a JavaScript debugger can attach.
        /// Breakpoints set in `.bynk` sources resolve through the emitted source
        /// maps, composed into the worker bundle. Prints the inspector URL on start.
        #[arg(long)]
        inspect: bool,
        /// First inspector port, allocated per context exactly as `--base-port`
        /// is (default 9229). Used under `--inspect`, and also whenever several
        /// contexts are served, so their always-on inspectors do not collide.
        #[arg(long, default_value_t = 9229)]
        inspect_port: u16,
        /// Which `bynk.deploy.lock` environment `-- --remote` reads the KV id
        /// from. `dev` never provisions, so this only selects among what
        /// `bynk deploy --env NAME` already recorded — irrelevant without
        /// `--remote`. Omit for today's single, unqualified default.
        #[arg(long, default_value = "default", value_name = "NAME")]
        env: String,
        /// A Worker outside the project, named by a `Service` instance
        /// (`@worker("NAME")`), to reach in your own locally running
        /// `wrangler dev` of it rather than in the stand-in `dev` binds by
        /// default. Repeatable.
        #[arg(long = "local-worker", value_name = "NAME")]
        local_workers: Vec<String>,
        /// Output format. `human` (default) is the readable stream on stderr.
        /// `json` adds one JSON event per line on stdout (`serving` with each
        /// context's ports, `rebuilding`, `rebuild_ok`, `rebuild_failed`,
        /// `stopped`), the stable scriptable surface for editors and CI.
        #[arg(long, value_enum, default_value = "human")]
        format: DevFormatArg,
        /// Once every served context answers an HTTP probe, write
        /// `{ "<worker>": "<url>" }` to FILE: the readiness signal for a test
        /// runner or CI script. A stale FILE is removed at start, and FILE is
        /// removed when the session stops.
        #[arg(long, value_name = "FILE")]
        ready_json: Option<PathBuf>,
        /// With `--format json`, also write each line a worker prints as a
        /// `log` event (`context`, `level`, `text`), so an editor can show the
        /// workers' logs per context and level. Implies `--rewrite-frames`.
        #[arg(long, requires = "format")]
        log_events: bool,
        /// Rewrite stack frames in the workers' output to `.bynk` locations
        /// even on a terminal. Off a terminal (an editor task, CI) it is
        /// always done; on one, wrangler's output is left attached so its
        /// colour and hotkeys keep working.
        #[arg(long)]
        rewrite_frames: bool,
        /// Arguments after `--`, forwarded to `wrangler dev` (e.g. `-- --remote`).
        /// Ports are the driver's to allocate: use `--base-port` / `--inspect-port`.
        #[arg(last = true)]
        wrangler_args: Vec<String>,
    },
    /// Provision each context's Cloudflare resources and deploy its Worker.
    /// The whole project ships in one command, in Service-Binding dependency
    /// order — Cloudflare rejects a Worker uploaded before a Worker it binds
    /// to. The generated configuration remains disposable: Cloudflare ids live
    /// in the committed `bynk.deploy.lock`.
    Deploy {
        /// Project directory to deploy from. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        #[command(flatten)]
        lock: LockArgs,
        /// Deploy this context alone, assuming the contexts it consumes are
        /// already live; a dependency that has never been deployed is reported
        /// rather than pushed into. Accepts the dotted name or its dasherised
        /// worker-dir form. Omit to deploy the whole project in order.
        #[arg(long, value_name = "NAME")]
        context: Option<String>,
        /// Target environment. Selects the `bynk.deploy.lock` section and,
        /// for any value other than `default`, synthesises an environment-
        /// scoped Wrangler config section (KV, queues, Service Bindings all
        /// qualified) since Cloudflare does not inherit bindings into a named
        /// environment. Omit to deploy today's single, unqualified default.
        #[arg(long, default_value = "default", value_name = "NAME")]
        env: String,
        /// Print the provisioning and deploy plan without changing Cloudflare
        /// or writing `bynk.deploy.lock`.
        #[arg(long, visible_alias = "plan")]
        dry_run: bool,
        /// Plan output format. `short` is line-oriented; `json` is for CI.
        #[arg(long, value_enum, default_value = "short")]
        format: DeployFormatArg,
        /// Skip the confirmation required before creating a namespace or
        /// publishing a Worker. Required for non-interactive automation.
        #[arg(long)]
        yes: bool,
        /// Read secret values from a dotenv-style `NAME=value` file. Supplies
        /// both names and values; never committed, never persisted. Values move
        /// to `wrangler secret put` and are dropped.
        #[arg(long, value_name = "PATH")]
        secrets_file: Option<PathBuf>,
        /// Set this named secret, taking its value from the environment (or a
        /// prompt). Repeatable. Use for a `bynk.Secrets` name, whose spelling
        /// the compiler cannot know — an actor's declared `auth` secret needs no
        /// flag. The environment is never scanned for names.
        #[arg(long = "secret", value_name = "NAME")]
        secrets: Vec<String>,
        /// Overwrite a secret that is already set. The default sets only the
        /// missing ones, so a re-deploy does not cut a fresh Cloudflare secret
        /// version for every secret every time.
        #[arg(long)]
        force: bool,
        /// Delete every reported orphan — a KV namespace or queue the ledger
        /// remembers that the current build no longer declares. Never
        /// deletes a Worker. Prompts separately from the creation
        /// confirmation unless --yes is also given; omit to report only.
        #[arg(long)]
        prune: bool,
        /// Confirm deleting this agent class's stored data, named as the plan
        /// prints it (`<context>.<Class>`). Repeatable. A `removed agent`
        /// declaration deletes data irreversibly, so `--yes` never stands in
        /// for this.
        #[arg(long = "confirm-delete", value_name = "CONTEXT.CLASS")]
        confirm_delete: Vec<String>,
        /// Arguments after `--`, forwarded to `wrangler deploy` verbatim.
        #[arg(last = true)]
        wrangler_args: Vec<String>,
    },
    /// Scaffold a new project: a complete, runnable single-context HTTP service
    /// you can serve immediately with `bynk dev`.
    ///
    /// Writes a `bynk.toml`, a `.gitignore`, and `src/<name>.bynk` into a new
    /// directory. Pure offline file-writing — it shells nothing and needs no
    /// toolchain, so you can run it before `bynkc`, Node, or `wrangler` are
    /// installed. The project name defaults to the target directory's final
    /// component; `--name` overrides it and must be a legal Bynk identifier.
    New {
        /// Directory to create for the new project (e.g. `hello` or `./hello`).
        path: PathBuf,
        /// Project name / context identifier. Defaults to PATH's final
        /// component; must be a legal Bynk identifier (a letter followed by
        /// letters, digits, or underscores).
        #[arg(long)]
        name: Option<String>,
    },
    /// Type-check a `.bynk` file or project without writing output — the
    /// `bynkc check` behaviour through the driver's compiler resolution (v0.138).
    ///
    /// Runs the compiler pipeline in-process (no `bynkc` binary required); with
    /// `BYNK_BYNKC` set, the pinned compiler is shelled instead so an
    /// externally-managed `bynkc` still governs the result.
    Check {
        /// Input `.bynk` file or project root. Defaults to the current directory.
        #[arg(default_value = ".")]
        input: PathBuf,
        /// Diagnostic output format. `rich` (default) is the ariadne
        /// source-context rendering; `short` emits one terse
        /// `path:line:col: severity[category]: message` line per diagnostic,
        /// for tooling (the VS Code problem-matcher, CI, scripts).
        /// In both, `path` is the file as you'd type it from the working
        /// directory (the input you passed, joined with the file's place in
        /// it), the same path `fmt` reports.
        #[arg(long, value_enum, default_value = "rich")]
        format: CheckFormatArg,
        #[command(flatten)]
        lock: LockArgs,
    },
    /// Compile a project to TypeScript in a directory — the `bynkc compile`
    /// behaviour through the driver's compiler resolution (#2471).
    ///
    /// Delegates to the `bynkc` the driver resolves, like `bynk test`, so an
    /// editor inherits that resolution. With `--sources-stdin` it compiles
    /// unsaved editor buffers: a JSON object `{ "<absolute path>": "<text>" }`
    /// on stdin wins over the files on disk, and `bynk.schema.lock` is neither
    /// read nor written. What "Bynk: Show Compiled Output" runs.
    Compile {
        /// Project directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        input: PathBuf,
        /// Output root directory.
        #[arg(short, long)]
        output: PathBuf,
        /// Compile the unsaved buffers given as JSON on stdin (see above).
        #[arg(long)]
        sources_stdin: bool,
        #[command(flatten)]
        lock: LockArgs,
        /// Run even when the resolved `bynkc` is a different *major* version
        /// from `bynk` (#1675). A minor skew only warns. Also settable as
        /// `BYNK_ALLOW_SKEW=1`.
        #[arg(long)]
        allow_skew: bool,
    },
    /// Format `.bynk` source files in place — the `bynkc fmt` behaviour through
    /// the driver (v0.138). Passing `-` reads from stdin and writes to stdout.
    ///
    /// Runs the formatter in-process (no `bynkc` binary required); with
    /// `BYNK_BYNKC` set, the pinned compiler is shelled instead — the style
    /// flags are forwarded to it either way.
    ///
    /// `--indent`, `--indent-width`, `--max-line-width` and
    /// `--no-trailing-comma` override the canonical style for this run; with
    /// none of them the output is the canonical formatting.
    Fmt {
        #[command(flatten)]
        args: bynk_driver::FmtArgs,
    },
    /// Discover and run test declarations in a project — the `bynkc test`
    /// behaviour through the driver (v0.138).
    ///
    /// Delegates to the `bynkc` the driver resolves (`BYNK_BYNKC` → PATH →
    /// sibling-of-`bynk`), so an editor or developer inherits the driver's
    /// richer compiler resolution instead of locating `bynkc` themselves.
    /// Requires `tsc` (with Node.js) or `tsx` on PATH, exactly as `bynkc test`.
    Test {
        #[command(flatten)]
        args: bynk_driver::test_runner::TestArgs,
        #[command(flatten)]
        lock: LockArgs,
        /// Run even when the resolved `bynkc` is a different *major* version
        /// from `bynk` (#1675). A minor skew only warns. Also settable as
        /// `BYNK_ALLOW_SKEW=1`.
        #[arg(long)]
        allow_skew: bool,
    },
    /// Resolve the project's dependencies, fetch the git ones missing from the
    /// store, and write `bynk.lock` (#2240). `check`, `test`, `dev` and
    /// `deploy` do this first anyway; `fetch` does only this — warm the store
    /// before an `--offline` CI step, or after editing `[dependencies]`.
    Fetch {
        /// Project (or workspace) directory. Defaults to `.`.
        #[arg(default_value = ".")]
        path: PathBuf,
        #[command(flatten)]
        lock: LockArgs,
        /// Output format. `json` prints one document on stdout (#2472).
        #[arg(long, value_enum, default_value_t)]
        format: DeployFormatArg,
    },
    /// Re-resolve git dependencies' revisions — a branch that moved upstream —
    /// and rewrite `bynk.lock` (#2240). Name aliases or packages to update only
    /// those; with none, every git dependency is re-resolved.
    Update {
        /// The aliases or `org/package` names to update. All when omitted.
        #[arg(value_name = "DEPENDENCY")]
        names: Vec<String>,
        /// Project (or workspace) directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Where each agent class's stored state stands, and the sweep that
    /// migrates every stored instance so old `migrate` steps can retire.
    Migrate {
        #[command(subcommand)]
        command: MigrateCommand,
    },
    /// Add a registry dependency to `bynk.toml` (#2274): the highest version
    /// its registry offers (or one matching `@<range>`), written as
    /// `<alias> = { package = "…", version = "^…" }`, then resolved and locked.
    Add {
        /// `org/package`, optionally `@<range>` (`acme/money@^1.4`).
        #[arg(value_name = "PACKAGE")]
        package: String,
        /// The alias to reach it by; defaults to the package segment.
        #[arg(long = "as", value_name = "ALIAS")]
        alias: Option<String>,
        /// Project directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Output format. `json` prints the entry written on stdout (#2472).
        #[arg(long, value_enum, default_value_t)]
        format: DeployFormatArg,
    },
    /// Look a package up on the registry its organisation routes to (#2472):
    /// every version its index lists, with the newest usable one. An exact
    /// `org/package` lookup — a registry's index has no free-text search.
    Search {
        /// `org/package`.
        #[arg(value_name = "PACKAGE")]
        package: String,
        /// The project whose `[registries]` route the organisation.
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Output format. `json` is for scripts and editors.
        #[arg(long, value_enum, default_value_t)]
        format: DeployFormatArg,
    },
    /// List the project's dependencies and what `bynk.lock` resolved each to
    /// (#2472). Offline.
    Deps {
        /// Project directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Output format. `json` is for scripts and editors.
        #[arg(long, value_enum, default_value_t)]
        format: DeployFormatArg,
    },
    /// Publish this package to the registry its organisation routes to
    /// (#2287): check it, pack it, upload it through the registry's publish
    /// API, and verify the signed line the registry returns.
    Publish {
        /// The package directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Check and pack, and report what would be published, without
        /// uploading.
        #[arg(long)]
        dry_run: bool,
        /// Refuse, before uploading anything, unless the package packs to
        /// exactly this checksum (the one a `--dry-run` reported).
        #[arg(long, value_name = "CHECKSUM")]
        expect_checksum: Option<String>,
        /// Output format. `json` prints the package, version, registry and
        /// file list on stdout (#2472).
        #[arg(long, value_enum, default_value_t)]
        format: DeployFormatArg,
    },
    /// Yank a published version through its registry's publish API (#2287):
    /// kept for locked builds, never newly chosen.
    Yank {
        /// `org/package`.
        package: String,
        /// The version.
        version: String,
        /// Clear the yank instead.
        #[arg(long)]
        undo: bool,
        /// The project whose `[registries]` route the organisation.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Sign in to a registry's accounts (#2378) through GitHub's device flow,
    /// and keep the account token in the credentials file.
    Login {
        #[command(flatten)]
        target: AccountTarget,
    },
    /// Sign out of a registry's accounts (#2378), here and on the server.
    Logout {
        #[command(flatten)]
        target: AccountTarget,
    },
    /// Manage an organisation on a registry with accounts (#2378): claim and
    /// block it, add and remove members, mint and revoke publish tokens.
    Org {
        #[command(subcommand)]
        command: OrgCommand,
    },
    /// Review and approve the npm packages that git and registry
    /// dependencies' adapters bring into the build (#2296), recording each
    /// approval in `bynk.lock`. With no package, asks about each pending one.
    Approve {
        /// Approve this npm package's pending requirements.
        #[arg(conflicts_with_all = ["all", "list"])]
        package: Option<String>,
        /// Approve every pending requirement without asking.
        #[arg(long, conflicts_with = "list")]
        all: bool,
        /// List pending and approved requirements; change nothing.
        #[arg(long)]
        list: bool,
        /// With `--list`: `short` (default) is one line per requirement;
        /// `json` is one document for tooling (the editor's review).
        #[arg(long, value_enum, default_value = "short", requires = "list")]
        format: DeployFormatArg,
        /// Project (or workspace) directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Build and maintain a static package registry in a directory (#2274) —
    /// served by any static host, or read as a `file://` index.
    Registry {
        #[command(subcommand)]
        command: RegistryCommand,
    },
    /// Explain a diagnostic code — the longer-form "what the rule is, why it
    /// exists, and how to fix it" behind a `bynk.*` error code (#853).
    ///
    /// Prints a curated blurb, a minimal before/after example, and a link to
    /// the relevant Book concept page. The blurb is offline-complete, so the
    /// substance is available without network. Codes without a curated
    /// explanation report that gracefully; an unrecognised code exits non-zero.
    /// The same explanations back the editor's clickable diagnostic-code links.
    Explain {
        /// The diagnostic code to explain, e.g. `bynk.resolve.unknown_type`.
        #[arg(value_name = "CODE")]
        code: String,
    },
}

/// `bynk migrate`'s subcommands (state-migrations S4, #2259).
#[derive(Subcommand, Debug)]
pub enum MigrateCommand {
    /// Each agent class's recorded version and floor, its `migrate` steps and
    /// the versions they cover, any version with no path forward, and the
    /// environments a floor-raising sweep must cover. Offline: reads
    /// `bynk.schema.lock`, the source, and `bynk.deploy.lock`.
    Status {
        /// Project directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        #[command(flatten)]
        lock: LockArgs,
        /// Output format. `json` is for scripts.
        #[arg(long, value_enum, default_value = "short")]
        format: DeployFormatArg,
    },
    /// Wake every stored instance of the project's agent classes in the
    /// deployed Workers, so each migrates to its class's current version, and
    /// raise each class's floor when the run covered every environment
    /// `bynk.deploy.lock` records.
    ///
    /// Lists the instances through Cloudflare's API, so it needs
    /// `CLOUDFLARE_API_TOKEN` (with "Workers Scripts: Edit") and
    /// `CLOUDFLARE_ACCOUNT_ID`. Each Worker gets a temporary secret,
    /// `BYNK_MIGRATE_KEY`, for the run; setting and deleting it each create a
    /// Worker version.
    Sweep {
        /// Project directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Sweep this context's classes alone (dotted name or Worker
        /// directory). A floor rises only for the classes swept.
        #[arg(long, value_name = "NAME")]
        context: Option<String>,
        /// Sweep this environment. Repeatable. Omit to sweep every environment
        /// the ledger records; a narrower run migrates but raises no floor.
        #[arg(long = "env", value_name = "NAME")]
        environments: Vec<String>,
        /// Instances woken per request to a Worker. The default keeps within
        /// the Free plan's 50 subrequests per invocation.
        #[arg(long, default_value_t = 40, value_parser = clap::value_parser!(u16).range(1..=1000))]
        batch: u16,
        /// Where a Worker that does not serve on `workers.dev` is reached, as
        /// `<deployed Worker name>=https://<host>`. Repeatable.
        #[arg(long = "url", value_name = "WORKER=URL")]
        urls: Vec<String>,
        /// Skip the confirmation. Required for non-interactive runs.
        #[arg(long)]
        yes: bool,
    },
}

/// `bynk check --format` selector, mirroring `bynkc`'s `DiagFormat` (rich/short)
/// so the two commands are drop-in equivalent.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, ValueEnum)]
pub enum CheckFormatArg {
    /// Ariadne rendering with full source context (the default).
    #[default]
    Rich,
    /// One terse `path:line:col: severity[category]: message` line per
    /// diagnostic — for the VS Code problem-matcher, CI, and scripts.
    Short,
}

impl CheckFormatArg {
    /// The `bynkc check --format` token this maps to when the pinned compiler
    /// is shelled under `BYNK_BYNKC`.
    pub fn as_bynkc_arg(self) -> &'static str {
        match self {
            CheckFormatArg::Rich => "rich",
            CheckFormatArg::Short => "short",
        }
    }
}

/// `--only` selector. Mirrors [`Capability`] minus the internal distinctions.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CapabilityArg {
    /// `bynkc` compile / check / fmt.
    Compile,
    /// `bynk test` — Node + tsc|tsx.
    Test,
    /// dev / deploy to Cloudflare — Node + wrangler.
    Deploy,
    /// Editor support — bynkc-lsp.
    Editor,
    /// Build Bynk from source — a Rust toolchain.
    Build,
}

impl From<CapabilityArg> for Capability {
    fn from(a: CapabilityArg) -> Self {
        match a {
            CapabilityArg::Compile => Capability::Compile,
            CapabilityArg::Test => Capability::Test,
            CapabilityArg::Deploy => Capability::Deploy,
            CapabilityArg::Editor => Capability::Editor,
            CapabilityArg::Build => Capability::BuildFromSource,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, ValueEnum)]
pub enum FormatArg {
    #[default]
    Human,
    Short,
    Json,
}

/// Output choices for `bynk dev` (#2094).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, ValueEnum)]
pub enum DevFormatArg {
    /// The readable stream on stderr.
    #[default]
    Human,
    /// Also one JSON lifecycle event per line on stdout.
    Json,
}

impl From<DevFormatArg> for crate::dev::DevFormat {
    fn from(f: DevFormatArg) -> Self {
        match f {
            DevFormatArg::Human => crate::dev::DevFormat::Human,
            DevFormatArg::Json => crate::dev::DevFormat::Json,
        }
    }
}

/// Scriptable output choices for `bynk deploy`'s plan.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, ValueEnum)]
pub enum DeployFormatArg {
    #[default]
    Short,
    Json,
}

impl From<FormatArg> for Format {
    fn from(f: FormatArg) -> Self {
        match f {
            FormatArg::Human => Format::Human,
            FormatArg::Short => Format::Short,
            FormatArg::Json => Format::Json,
        }
    }
}

/// Build the [`DoctorOptions`] from the parsed flags.
pub fn doctor_options(only: Option<CapabilityArg>, strict: bool) -> DoctorOptions {
    DoctorOptions {
        only: only.map(Into::into),
        strict,
    }
}

/// #2240: how a command may change `bynk.lock` and the store.
#[derive(Args, Debug, Clone, Copy, Default)]
pub struct LockArgs {
    /// Fail instead of changing `bynk.lock` — for CI, which should build
    /// exactly what was committed.
    #[arg(long)]
    pub locked: bool,
    /// Fail instead of fetching: every git dependency must already be in the
    /// store.
    #[arg(long)]
    pub offline: bool,
}

impl LockArgs {
    /// The fetch options these flags ask for.
    pub fn options(self) -> crate::fetch::FetchOptions {
        crate::fetch::FetchOptions {
            locked: self.locked,
            offline: self.offline,
            update: None,
        }
    }
}

/// #2378: which registry an account command talks to.
#[derive(Args, Debug, Clone)]
pub struct AccountTarget {
    /// The registry, by its `[registries]` name: by default, the one the
    /// organisation routes to (the first-party registry for `login` and
    /// `logout`).
    #[arg(long)]
    pub registry: Option<String>,
    /// The project whose `[registries]` name and route the registry.
    #[arg(long, default_value = ".")]
    pub path: PathBuf,
}

/// #2378: `bynk org` subcommands.
#[derive(Subcommand, Debug)]
pub enum OrgCommand {
    /// Claim an organisation, first come: you become its owner.
    Claim {
        org: String,
        /// Hold the name without publishing: every publish is refused.
        #[arg(long)]
        block: bool,
        /// Claim it for this GitHub user (an operator token only).
        #[arg(long)]
        owner: Option<String>,
        #[command(flatten)]
        target: AccountTarget,
    },
    /// Show an organisation's owner, members, whether it is blocked, and its
    /// publish tokens (never the tokens themselves).
    Show {
        org: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t)]
        format: DeployFormatArg,
        #[command(flatten)]
        target: AccountTarget,
    },
    /// Block an organisation: every publish to it is refused.
    Block {
        org: String,
        #[command(flatten)]
        target: AccountTarget,
    },
    /// Unblock an organisation.
    Unblock {
        org: String,
        #[command(flatten)]
        target: AccountTarget,
    },
    /// Add or remove members.
    Member {
        #[command(subcommand)]
        command: MemberCommand,
    },
    /// Mint or revoke publish tokens.
    Token {
        #[command(subcommand)]
        command: OrgTokenCommand,
    },
}

/// #2378: `bynk org member` subcommands.
#[derive(Subcommand, Debug)]
pub enum MemberCommand {
    /// Add a GitHub user as a member: they may mint publish tokens.
    Add {
        org: String,
        /// Their GitHub login.
        login: String,
        #[command(flatten)]
        target: AccountTarget,
    },
    /// Remove a member, and every token they minted.
    Remove {
        org: String,
        /// Their GitHub login, or numeric id.
        member: String,
        #[command(flatten)]
        target: AccountTarget,
    },
}

/// #2378: `bynk org token` subcommands.
#[derive(Subcommand, Debug)]
pub enum OrgTokenCommand {
    /// Mint a publish token, shown once on stdout (or saved with `--save`).
    Mint {
        org: String,
        /// What it may do: `publish`, `yank` (repeatable or comma-separated;
        /// both by default).
        #[arg(long = "can", value_delimiter = ',', default_values = ["publish", "yank"])]
        can: Vec<String>,
        /// How many days it lasts (1 to 365).
        #[arg(long, default_value_t = 90)]
        days: u32,
        /// A note to recognise it by in `bynk org show`.
        #[arg(long, default_value = "")]
        note: String,
        /// Save it as `[registries.<name>] token` in the credentials file,
        /// where `bynk publish` reads it, instead of printing it.
        #[arg(long)]
        save: bool,
        /// Let `--save` replace a different token.
        #[arg(long, requires = "save")]
        force: bool,
        #[command(flatten)]
        target: AccountTarget,
    },
    /// Revoke a publish token, by the id `bynk org show` prints.
    Revoke {
        org: String,
        /// The token's id, or a unique prefix of it.
        id: String,
        #[command(flatten)]
        target: AccountTarget,
    },
}

/// #2274: `bynk registry` subcommands.
#[derive(Subcommand, Debug)]
pub enum RegistryCommand {
    /// Create a registry: its `config.json`, and a signing key written outside
    /// the directory. Prints the `key = "…"` line consumers pin.
    Init {
        /// The registry directory to create.
        dir: PathBuf,
        /// The registry's id, which every signed line binds.
        #[arg(long)]
        id: String,
        /// Where to write the signing key (default `./<id>.key`).
        #[arg(long = "key-out")]
        key_out: Option<PathBuf>,
    },
    /// Pack a package, sign its index line, and add it to the registry.
    Add {
        /// The registry directory.
        dir: PathBuf,
        /// The package's directory (holding its `bynk.toml`).
        package: PathBuf,
        /// The registry's signing key.
        #[arg(long)]
        key: PathBuf,
    },
    /// Serve a registry directory with the publish API (#2287): plain HTTP,
    /// loopback by default — put a TLS-terminating proxy in front to serve
    /// beyond this machine.
    Serve {
        /// The registry directory.
        dir: PathBuf,
        /// The registry's signing key (never inside the directory).
        #[arg(long)]
        key: PathBuf,
        /// The tokens file `bynk registry token` writes (never inside the
        /// directory).
        #[arg(long)]
        tokens: PathBuf,
        /// The address to listen on.
        #[arg(long, default_value = crate::registry::serve::DEFAULT_LISTEN)]
        listen: String,
    },
    /// Append a link to the registry's key chain (#2316): a new key, signed
    /// by the current chain key. Clients that trust an older key follow it.
    Rotate {
        /// The registry directory (or a copy holding its `config.json`).
        dir: PathBuf,
        /// The current (newest) chain key.
        #[arg(long)]
        key: PathBuf,
        /// Where to write the new key (never inside the directory).
        #[arg(long)]
        key_out: PathBuf,
    },
    /// Delegate signing to a new key (#2316): it may sign index lines, and
    /// nothing else. Use it as `serve`'s or `add`'s `--key`, keeping the chain
    /// key offline.
    Delegate {
        /// The registry directory (or a copy holding its `config.json`).
        dir: PathBuf,
        /// A chain key, which signs the delegation.
        #[arg(long)]
        key: PathBuf,
        /// Where to write the delegated key (never inside the directory).
        #[arg(long)]
        delegate_out: PathBuf,
    },
    /// Revoke a delegated key (#2316): lines it signed are refused from then
    /// on, so re-sign them with another key first (`resign`).
    Revoke {
        /// The registry directory (or a copy holding its `config.json`).
        dir: PathBuf,
        /// The delegate to revoke, `ed25519:…`.
        target: String,
        /// A chain key, which signs the revocation.
        #[arg(long)]
        key: PathBuf,
        /// Revoke even if some lines would be left with no valid signer.
        #[arg(long)]
        force: bool,
    },
    /// Re-sign every index line with a key (#2316): the newest chain key or a
    /// valid delegate. Only each line's `sig` changes.
    Resign {
        /// The registry directory.
        dir: PathBuf,
        /// The signing key.
        #[arg(long)]
        key: PathBuf,
    },
    /// Check a registry against the protocol (#2311): every observable rule a
    /// server must keep, run against its URL. A full run publishes one
    /// throwaway package under `--org` and leaves it yanked; `--read-only`
    /// publishes nothing.
    Conformance {
        /// The registry's index URL.
        index: String,
        /// The organisation the token may publish and yank under.
        #[arg(long, required_unless_present = "read_only")]
        org: Option<String>,
        /// The registry name the token is found under
        /// (`BYNK_REGISTRY_<NAME>_TOKEN`, or the credentials file).
        #[arg(long, default_value = "conformance")]
        name: String,
        /// The registry's key to pin (`ed25519:…`); trusted on first use for
        /// the run otherwise.
        #[arg(long)]
        key: Option<String>,
        /// Publish nothing: check `config.json`, and `--package` if given.
        #[arg(long)]
        read_only: bool,
        /// The existing `org/package` a `--read-only` run reads back.
        #[arg(long, requires = "read_only")]
        package: Option<String>,
        /// An environment variable holding a token that can't act for
        /// `--org`, for the `auth.scope` check (skipped without one).
        #[arg(long)]
        narrow_token_env: Option<String>,
        /// The upload cap, in bytes, to test `413` against.
        #[arg(long, default_value_t = 64 * 1024 * 1024)]
        max_upload: u64,
        /// Output format.
        #[arg(long, value_enum, default_value_t)]
        format: DeployFormatArg,
    },
    /// Mint a publish token for `bynk registry serve` (#2287): printed once;
    /// only its hash is stored in the tokens file.
    Token {
        /// The tokens file to add it to.
        tokens: PathBuf,
        /// An organisation it may act for (repeatable).
        #[arg(long = "org", required = true)]
        organisations: Vec<String>,
        /// What it may do: `publish`, `yank` (repeatable or comma-separated; both by default).
        #[arg(long = "can", value_delimiter = ',', default_values = ["publish", "yank"])]
        can: Vec<String>,
    },
    /// Mark a version yanked — kept for locked builds, never newly chosen.
    Yank {
        /// The registry directory.
        dir: PathBuf,
        /// `org/package`.
        package: String,
        /// The version to yank.
        version: String,
        /// Clear the yank instead.
        #[arg(long)]
        undo: bool,
    },
    /// Withdraw a version (#2330): a takedown. Yanks its line, records the
    /// reason clients print, and deletes its archive. Final — there is no undo.
    Withdraw {
        /// The registry directory.
        dir: PathBuf,
        /// `org/package`.
        package: String,
        /// The version to withdraw.
        version: String,
        /// Why, as clients will print it (one line, at most 500 characters).
        #[arg(long)]
        reason: String,
    },
}
