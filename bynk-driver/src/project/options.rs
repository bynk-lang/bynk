use bynk_check::project_model::BuildTarget;
use bynk_emit::emit_model::ImportExt;
use bynk_emit::emit_model::RemoteTests;
use bynk_project::ProjectPaths;
use bynk_project::Roots;
use bynk_project::SchemaLock;
use bynk_project::assets;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use bynk_check::firstparty::Platform;

/// #3100: one step of a build, reported as it happens. Only a `context` is
/// reported: commons and test units compile as part of the build, not as steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgressEvent {
    /// A context's check and emission began.
    Started(String),
    /// A context checked and emitted cleanly. A context with errors reports
    /// no `Finished`: its diagnostics follow.
    Finished(String),
}

/// Where a build reports [`ProgressEvent`]s. Called in compilation order
/// (dependencies first), on the building thread.
pub type ProgressSink = Arc<dyn Fn(&ProgressEvent) + Send + Sync>;

/// Options for [`compile_project`](super::compile_project). Construct with [`CompileOptions::single`] or
/// [`CompileOptions::split`], then chain `.target(…)` / `.platform(…)` /
/// `.import_ext(…)` to override the bundle/default-platform/`.js` defaults.
#[derive(Clone)]
pub struct CompileOptions {
    /// #3477: the directory the build will be written to, when the caller knows
    /// it (`bynk build` and `bynk compile`), so a container's build paths can be
    /// written from the Worker's output directory back to the project. `None`
    /// uses the project's `[build] out`.
    pub out_dir: Option<PathBuf>,
    pub target: BuildTarget,
    pub platform: Platform,
    pub roots: Roots,
    /// The import-specifier extension (slice 2). `Js` (default) for normal builds;
    /// `Ts` for the `bynk test --inspect` debug build.
    pub import_ext: ImportExt,
    /// v0.115 (testing track slice 3, DECISION J): the build profile for function
    /// contracts. `true` (dev/test) emits the call-site guard around a contracted
    /// `fn`; `false` (release/deploy) strips it entirely for zero runtime cost.
    /// `bynk test` and `--inspect` set it on; `bynk compile` leaves it off.
    pub contracts: bool,
    /// #3580 (the dev request-traces track): emit the per-request trace recording
    /// (`__bynk_runtime/trace`) into a Workers build, for `bynk dev
    /// --trace-events`. `false` for every other build, so a deploy build's
    /// output is byte-identical. Dev-only by construction: only `bynk dev` sets it.
    pub trace_requests: bool,
    /// #3581 (the dev agent-inspection track): emit the dev-only agent registry,
    /// the `/_bynk/inspect` routes and the instance recording into a Workers
    /// build, for `bynk dev`. `false` for every other build, so a deploy build's
    /// output is byte-identical. Dev-only by construction: only `bynk dev` sets it.
    pub inspect_agents: bool,
    /// #3945: `true` (`bynk test`) makes a call to a generic capability operation carry its
    /// type arguments' identity as a trailing argument, so a test's `stub Cap.op[T]` can tell
    /// the calls apart. Never set by a build for deploy, whose output is unchanged.
    pub typed_capability_calls: bool,
    /// #1821 (ADR 0147 D3): whether `suite`s are type-checked and emitted (the
    /// `tests/` modules and `tests/main.ts`). On by default; `bynk compile`
    /// and the driver's dev/deploy build turn it off, so a suite never reaches
    /// the deployable. `bynk test` leaves it on. Off strips suites after
    /// parsing: a file that does not parse still fails the build, since it
    /// cannot be told apart from production source.
    pub tests: bool,
    /// #57 (testing track): when `Some`, every file `roots` would otherwise
    /// discover on disk is instead read from here — keyed the same way
    /// `discovery::read_source`'s overlay is (a canonicalised absolute path,
    /// falling back to the literal path when the file has no on-disk
    /// counterpart to canonicalise). Filesystem discovery is skipped entirely;
    /// `roots` still supplies `src_root`/`tests_root` and their prefixes, so a
    /// `Roots::Split` project can drive both trees in-memory.
    ///
    /// #1077/#1081: `bynk-driver`'s `project_options`/`try_project_options`
    /// *do* set this now — the real CLI entry points walk and read every
    /// project file themselves and hand the result here, so `bynk-emit`
    /// itself no longer discovers or reads anything on disk for the CLI
    /// path. `bynk`/the LSP still don't: `bynk` routes through
    /// `bynk-driver`, and the LSP (`bynk_check::analysis::analyse_project`
    /// since P4.2) never goes through `CompileOptions` at all — it has its
    /// own, separate discovery path (#1079). `bynk-emit`'s own tests also
    /// set this, to exercise the full
    /// project pipeline (cross-context `uses`, multi-file layouts,
    /// workers-mode emission, …) without an on-disk fixture tree.
    pub sources: Option<HashMap<PathBuf, String>>,
    /// Events track, slice 3c (#980): reconcile against `bynk.schema.lock`.
    /// **Off by default** — `bynk compile`'s directory branch and `bynk`'s
    /// deploy/dev build turn it on and write the reconciled lock back;
    /// `check`'s and `test`'s directory paths (#2078) turn it on read-only,
    /// so a non-additive change fails them too, and never write it. Every
    /// library/test caller (in-memory builds, `bynk-tests/tests/e2e.rs`'s
    /// in-place fixture compiles, `bynk-emit`'s own `sources`-driven tests,
    /// the LSP) leaves it off, so a compile never mutates a project tree it
    /// wasn't asked to.
    ///
    /// #1078: this carries the lock's pre-read content, not just an on/off
    /// switch — `bynk-emit` reads and writes no disk for this file (the same
    /// move #1077/#1081 already made for `.bynk` source content). See
    /// [`SchemaLock`]'s own doc for the read side; the reconciled content
    /// comes back out on [`ProjectOutput::schema_lock`](bynk_emit::emit_model::ProjectOutput::schema_lock) for the caller to
    /// persist.
    pub schema_registry: SchemaLock,
    /// #2554: `bynk.contracts.lock`'s pre-read content: the older contracts
    /// each callee also accepts. `None` (every library, test and LSP caller,
    /// and a project without the file) accepts none, so a callee answers only
    /// the contract it has. `bynk-driver`'s `try_project_options` reads it, as
    /// it reads the manifest, so every CLI build honours it; a build only ever
    /// reads it, and the driver writes it.
    pub contracts_lock: Option<String>,
    /// #1890: the project's extra Cloudflare compatibility flags, from
    /// `bynk.toml`'s `[workers] compatibility_flags`, in manifest order.
    /// A Workers build writes them into every `wrangler.toml` after
    /// [`bynk_emit::DEFAULT_COMPATIBILITY_FLAGS`]; a duplicate is dropped with a
    /// `bynk.project.duplicate_compatibility_flag` warning. Empty by default:
    /// `bynk-driver`'s `project_options`/`try_project_options` read the
    /// manifest and set it, as does `bynk-tests/tests/e2e.rs` for a fixture.
    pub compatibility_flags: Vec<String>,
    /// `bynk.toml`'s `[workers] workers_dev`: written into every
    /// `wrangler.toml` when set, left out when `None`.
    pub workers_dev: Option<bool>,
    /// `bynk.toml`'s `[workers.observability]` (#4127): the project's Workers
    /// Logs settings, which a worker group may override.
    pub observability: bynk_project::observability::Observability,
    /// #1883, slice 1b: the project's `assets/` folder — its files, keyed by
    /// normalised project-relative path (`assets/board.css`), listed by the
    /// caller, and a reader for their bytes
    /// ([`assets::AssetFolder`], usually `AssetFolder::on_disk`). The build
    /// reads only the files an `asset` declaration names, once each, through
    /// that reader, so `bynk-emit` never touches the disk and a large file
    /// nothing declares is never read. `bynk-driver`'s
    /// `project_options`/`try_project_options` and `bynk-testkit`'s
    /// `compile_options_*` set it.
    ///
    /// `None` means no filesystem was consulted (`compile_in_memory`, the
    /// playground). A check then skips the existence check; a build treats it
    /// as an empty folder, so an asset there is `bynk.asset.missing_file`
    /// rather than a URL made up for a file nobody read.
    pub asset_files: Option<assets::AssetFolder>,
    /// #2387, slice 2: the project's `embeds/` folder, listed by the caller
    /// with a reader for the bytes of only the files an `embed` declaration
    /// names ([`assets::AssetFolder::embeds_on_disk`]), exactly as
    /// [`CompileOptions::asset_files`] does for `assets/`. `None` means no
    /// filesystem was consulted: a check then skips the existence check, and a
    /// build treats it as an empty folder, so an embed there is
    /// `bynk.embed.missing_file` rather than text made up for a file nobody
    /// read.
    pub embed_files: Option<assets::EmbedFolder>,
    /// #2841: the project's `fixtures/` folder, listed by the caller with a
    /// reader for the bytes of only the files a suite's `fixture` declaration
    /// names ([`assets::AssetFolder::fixtures_on_disk`]), exactly as
    /// [`CompileOptions::embed_files`] does for `embeds/`. Fixtures are test
    /// input: nothing in a deployable reads them. `None` means no filesystem
    /// was consulted.
    pub fixture_files: Option<assets::FixtureFolder>,
    /// P4b (#2608): the project's `migrations/` folder, listed by the caller
    /// ([`assets::AssetFolder::migrations_on_disk`]). Every file in a declared
    /// `D1` database's directory is read, checked, copied beside the Worker's
    /// `wrangler.toml` and run on each test database. `None` means no
    /// filesystem was consulted.
    pub migration_files: Option<assets::AssetFolder>,
    /// #2842: the project's `snapshots/` folder, read by the caller
    /// ([`bynk_project::snapshots::read_snapshot_files`]): each file's text by its
    /// project-relative path. A test module inlines the file for its own suite
    /// file, so the harness reads nothing at test time. `None` (and an empty map)
    /// means no snapshot is recorded, so every `matches snapshot` claim is missing.
    pub snapshot_files: Option<BTreeMap<String, String>>,
    /// #3100: where [`compile_project`](super::compile_project) reports each context as it compiles.
    /// `None` (the default, and every library, test and LSP caller) reports
    /// nothing, so a build behaves exactly as it did before.
    pub progress: Option<ProgressSink>,
    /// #2138 (end-to-end testing track §3.5, §3.7): `Some` when `bynk test
    /// --against` runs the `system` suites against a real runtime. The build
    /// then emits only the `system` suites, routes their drivers to the
    /// target's URLs (`BYNK_TEST_TARGETS`) instead of the in-process harness,
    /// and refuses (or, under `--skip-unsupported`, skips) a case that uses
    /// what a remote run can't honour. `None` for every other build.
    pub remote_tests: Option<RemoteTests>,
}

impl CompileOptions {
    /// Single-root project (`src == tests`), bundle target, default platform.
    pub fn single(root: impl Into<PathBuf>) -> Self {
        Self {
            out_dir: None,
            target: BuildTarget::Bundle,
            platform: Platform::default(),
            roots: Roots::Single(root.into()),
            import_ext: ImportExt::default(),
            contracts: false,
            trace_requests: false,
            inspect_agents: false,
            typed_capability_calls: false,
            tests: true,
            sources: None,
            schema_registry: SchemaLock::Off,
            contracts_lock: None,
            compatibility_flags: Vec::new(),
            workers_dev: None,
            observability: Default::default(),
            asset_files: None,
            embed_files: None,
            fixture_files: None,
            migration_files: None,
            snapshot_files: None,
            progress: None,
            remote_tests: None,
        }
    }

    /// v0.9.1 split layout (source and test units in separate subdirectories
    /// under `project_root`), bundle target, default platform. Use this from
    /// `bynk test` so its rooting matches `bynk compile`'s.
    pub fn split(project_root: impl Into<PathBuf>, paths: ProjectPaths) -> Self {
        Self {
            out_dir: None,
            target: BuildTarget::Bundle,
            platform: Platform::default(),
            roots: Roots::Split {
                project_root: project_root.into(),
                paths,
            },
            import_ext: ImportExt::default(),
            contracts: false,
            trace_requests: false,
            inspect_agents: false,
            typed_capability_calls: false,
            tests: true,
            sources: None,
            schema_registry: SchemaLock::Off,
            contracts_lock: None,
            compatibility_flags: Vec::new(),
            workers_dev: None,
            observability: Default::default(),
            asset_files: None,
            embed_files: None,
            fixture_files: None,
            migration_files: None,
            snapshot_files: None,
            progress: None,
            remote_tests: None,
        }
    }

    /// #3477: the directory the build is written to ([`CompileOptions::out_dir`]).
    pub fn out_dir(mut self, out: Option<PathBuf>) -> Self {
        self.out_dir = out;
        self
    }

    /// #3100: report each context as it compiles ([`CompileOptions::progress`]).
    pub fn progress(mut self, sink: Option<ProgressSink>) -> Self {
        self.progress = sink;
        self
    }

    /// #2138: a remote test build ([`CompileOptions::remote_tests`]).
    pub fn remote_tests(mut self, remote: Option<RemoteTests>) -> Self {
        self.remote_tests = remote;
        self
    }

    /// Select the build target. `Bundle` (default) is the v0.6+ single-bundle
    /// layout; `Workers` (v0.8) emits per-context Cloudflare Workers.
    pub fn target(mut self, target: BuildTarget) -> Self {
        self.target = target;
        self
    }

    /// Slice 2: select the import-specifier extension. `Ts` is the debug build
    /// for `bynk test --inspect` (run the `.ts` directly under Node strip-only).
    pub fn import_ext(mut self, ext: ImportExt) -> Self {
        self.import_ext = ext;
        self
    }

    /// v0.115: enable the function-contract call-site guard (dev/test profile).
    /// `bynk test` and `--inspect` call this; the deploy build leaves it off so
    /// contract checks never reach production (DECISION J).
    pub fn contracts(mut self, on: bool) -> Self {
        self.contracts = on;
        self
    }

    /// #3580: record per-request traces in a Workers build (`bynk dev
    /// --trace-events`). Off by default, and never set by `bynk compile` or
    /// `bynk deploy`.
    pub fn trace_requests(mut self, on: bool) -> Self {
        self.trace_requests = on;
        self
    }

    /// #3581: emit the dev-only agent inspection into a Workers build (`bynk
    /// dev`). Off by default, and never set by `bynk compile` or `bynk deploy`.
    /// #3945: see [`Self::typed_capability_calls`].
    pub fn typed_capability_calls(mut self, on: bool) -> Self {
        self.typed_capability_calls = on;
        self
    }

    pub fn inspect_agents(mut self, on: bool) -> Self {
        self.inspect_agents = on;
        self
    }

    /// #1821 (ADR 0147 D3): whether to type-check and emit `suite`s. A build
    /// that produces a deployable (`bynk compile`, `bynk dev`/`deploy`) turns
    /// it off; a suite is test-only.
    pub fn tests(mut self, on: bool) -> Self {
        self.tests = on;
        self
    }

    /// v0.17: select the deploy [`Platform`] (selects the `bynk` surface
    /// binding). The MVP ships `cloudflare` only.
    pub fn platform(mut self, platform: Platform) -> Self {
        self.platform = platform;
        self
    }

    /// #57 (testing track): supply every file in-memory instead of walking
    /// `roots` on disk — keyed the same way `discovery::read_source`'s
    /// overlay is (see the `sources` field's own doc).
    pub fn sources(mut self, sources: HashMap<PathBuf, String>) -> Self {
        self.sources = Some(sources);
        self
    }

    /// Events track, slice 3c (#980): turn on `bynk.schema.lock`
    /// reconciliation for this build, with its pre-read content (or
    /// verified-absent `None`, for a fresh project). See [`SchemaLock`] and
    /// the field's own doc for who calls this and why everyone else leaves
    /// it off.
    pub fn schema_registry(mut self, mode: SchemaLock) -> Self {
        self.schema_registry = mode;
        self
    }

    /// #2554: the contracts lock's pre-read content. See the field's own doc.
    pub fn contracts_lock(mut self, content: Option<String>) -> Self {
        self.contracts_lock = content;
        self
    }

    /// #1890: the project's extra Cloudflare compatibility flags, from
    /// `bynk.toml`'s `[workers]` table. See the field's own doc.
    pub fn compatibility_flags(mut self, flags: Vec<String>) -> Self {
        self.compatibility_flags = flags;
        self
    }

    /// #4127: the project's `[workers.observability]`. See the field's own doc.
    pub fn observability(
        mut self,
        observability: bynk_project::observability::Observability,
    ) -> Self {
        self.observability = observability;
        self
    }

    /// The project's `[workers] workers_dev`. See the field's own doc.
    pub fn workers_dev(mut self, workers_dev: Option<bool>) -> Self {
        self.workers_dev = workers_dev;
        self
    }

    /// #1883, slice 1b: the listed `assets/` folder. See the field's own doc.
    pub fn asset_files(mut self, folder: assets::AssetFolder) -> Self {
        self.asset_files = Some(folder);
        self
    }

    /// #2387, slice 2: the listed `embeds/` folder. See the field's own doc.
    pub fn embed_files(mut self, folder: assets::EmbedFolder) -> Self {
        self.embed_files = Some(folder);
        self
    }

    /// #2841: the listed `fixtures/` folder. See the field's own doc.
    pub fn fixture_files(mut self, folder: assets::FixtureFolder) -> Self {
        self.fixture_files = Some(folder);
        self
    }

    /// #2842: the project's `snapshots/` files. See the field's own doc.
    pub fn snapshot_files(mut self, files: BTreeMap<String, String>) -> Self {
        self.snapshot_files = Some(files);
        self
    }

    /// P4b (#2608): the listed `migrations/` folder. See the field's own doc.
    pub fn migration_files(mut self, folder: assets::AssetFolder) -> Self {
        self.migration_files = Some(folder);
        self
    }
}
