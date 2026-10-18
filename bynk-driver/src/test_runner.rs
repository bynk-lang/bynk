//! `bynkc test` / `bynk test`'s shared command body (Wave 5 §5.4, findings
//! #40/#72/#20/#21 remainder): compile the project's test declarations, write
//! them, and run them via `tsc → node` (falling back to `tsx`), folding the
//! result into the pinned [`crate::test_json::TestRun`] document in
//! `--format json` mode. Moved down from `bynkc` — both `bynkc` and `bynk`
//! need one implementation instead of two.
//!
//! Runners are found with [`crate::probe::detect`] and
//! [`crate::probe::DetectOpts::default()`] (no project-local search, no `npx`
//! fallback), the one detection implementation both CLIs share for
//! `doctor`/`dev`. #1758: each is spawned by the path detection resolved, not by
//! its bare name, so a Windows npm shim (`tsc.cmd`) runs, and a runner that is
//! found but won't start is reported as such rather than as missing.

use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, ExitCode, Stdio};

use bynk_emit::project::{BuildTarget, ImportExt, ProjectOutput};
use clap::ValueEnum;

use crate::probe::{DetectOpts, SystemToolbox};
use crate::test_json::{Case, Location, Suite, TestRun};

/// Where `name` is installed on `PATH`, if it is. #1758: callers spawn this
/// path, not the bare name, so a Windows npm shim (`tsc.cmd`) that detection
/// finds is also the program that runs.
fn resolve_tool(name: &str) -> Option<PathBuf> {
    crate::probe::detect(&SystemToolbox, name, DetectOpts::default())
        .provenance
        .path()
        .map(Path::to_path_buf)
}

/// #1758: a runner that was found but would not start. Recorded instead of
/// silently skipped, so a run with no working runner reports the real cause
/// rather than advising an install of something already installed.
fn start_failure(path: &Path, e: &std::io::Error) -> String {
    format!(
        "`{}` was found but could not be started: {e}",
        path.display()
    )
}

/// `test --format` selector, shared by `bynkc test` and `bynk test` (review
/// findings #40/#72): one enum both CLIs' `Test` subcommand uses, instead of
/// two structurally-identical copies that must be hand-kept in sync.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, ValueEnum)]
pub enum TestFormat {
    /// The grouped `✓ / ✗` human output (the default; unchanged behaviour).
    #[default]
    Rich,
    /// A single pinned JSON document of results, for tooling and CI.
    Json,
}

impl TestFormat {
    /// The `--format` token this maps to when `bynk test` shells a resolved
    /// `bynkc` (`TestFormat::as_str` isn't `ValueEnum`-derived, since the
    /// wire value and the token clap parses happen to coincide, but the two
    /// concerns — "what did the user type" vs. "what do I forward" — are
    /// worth keeping textually distinct call sites for).
    pub fn as_bynkc_arg(self) -> &'static str {
        match self {
            TestFormat::Rich => "rich",
            TestFormat::Json => "json",
        }
    }
}

/// The `test` subcommand's flags — the one contract `bynkc test` and `bynk
/// test` both `#[command(flatten)]` (review findings #40/#72/#20/#21
/// remainder), replacing four independent hand-spellings (`bynkc::cli`,
/// `bynk::cli`, `bynk::test::TestArgs`, and the argv-literal rebuild that
/// turned the latter back into flags for the `bynkc` shell-out) with one.
/// Field docs here are the CLI help text for both commands' flags.
#[derive(clap::Args, Debug)]
pub struct TestArgs {
    /// Input project root directory. Defaults to the current directory.
    #[arg(default_value = ".")]
    pub input: PathBuf,
    /// Where to write compiled TypeScript test runner modules.
    /// Defaults to `<input>/out`.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Skip the runner invocation. With `--format rich` this emits the
    /// generated test files (for CI flows that drive the runner separately);
    /// with `--format json` it emits a discovery document listing every
    /// suite and case (each `outcome: "discovered"`) without running them —
    /// a pure compile, no `tsc`/Node.
    #[arg(long)]
    pub no_run: bool,
    /// Output format. `rich` (default) is the grouped ✓ / ✗ human output;
    /// `json` is a single pinned JSON document of results, for tooling.
    #[arg(long, value_enum, default_value_t = TestFormat::Rich)]
    pub format: TestFormat,
    /// Compile a debug build and launch the test runner under Node's
    /// inspector (`node --inspect-brk`), printing the inspector URL for a
    /// JavaScript debugger to attach. The emitted `.ts` runs directly under
    /// Node's line-preserving type-stripping, so source maps resolve
    /// breakpoints back to `.bynk`. Requires Node ≥ 22.18 (or ≥ 23.6
    /// unflagged). Does not run `tsc`.
    #[arg(long)]
    pub inspect: bool,
    /// The root seed for generative `property` tests, as hex (e.g. `0x5f3a`).
    /// A failing property prints the seed it used; re-running with `--seed
    /// <hex>` reproduces that run byte-for-byte. Omitted, each run draws a
    /// fresh random seed.
    #[arg(long)]
    pub seed: Option<String>,
    /// Run only test cases whose name matches `<name>`, skipping the rest —
    /// the filter behind the editor's per-case `▷ Run Test` lens. Matches by
    /// exact case name across suites; omitted, every case runs. No effect
    /// with `--no-run` (discovery lists all cases regardless).
    #[arg(long, value_name = "NAME")]
    pub case: Option<String>,
    /// After the suite runs, report statement/line coverage attributed to
    /// `.bynk` source (a rich summary table, or a `coverage` block in
    /// `--format json`). Requires the `tsc → node` path: incompatible with
    /// `--inspect` and `--no-run`, and errors if only `tsx` is available.
    #[arg(long)]
    pub coverage: bool,
}

/// Normalise a `--seed` value (`0x5f3a` or `5f3a`) to the bare-hex form the
/// runner reads from `BYNK_TEST_SEED` (JS `parseInt(_, 16)` does not accept a
/// `0x` prefix). Returns `None` for a non-hex value, so a typo is ignored rather
/// than silently seeding to zero.
fn normalise_seed(raw: &str) -> Option<String> {
    let hex = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(raw);
    if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(hex.to_string())
}

/// In `--format json` mode the deterministic surface is the document on stdout,
/// so a `<program> test:` line on stderr is fine but must never reach stdout.
/// `program` prefixes stderr messages (`"bynkc"` or `"bynk"`) so they read the
/// same as before the move.
pub fn run_test(program: &str, args: TestArgs) -> ExitCode {
    let TestArgs {
        input,
        output,
        no_run,
        format,
        inspect,
        seed,
        case,
        coverage,
    } = args;
    let json = format == TestFormat::Json;
    // #854 DECISION C: `--coverage` requires the `tsc → node` path — the CI-shaped
    // path with real `.js.map`s. `--inspect` is a debug path with no `tsc` (and a
    // different map role), and `--no-run` never launches a process to observe.
    // Reject both up front with an actionable message rather than producing
    // silently-wrong or empty numbers.
    if coverage && inspect {
        return coverage_unsupported(
            program,
            json,
            "`--coverage` cannot be combined with `--inspect` — coverage needs the `tsc → node` run, not the inspector.",
        );
    }
    if coverage && no_run {
        return coverage_unsupported(
            program,
            json,
            "`--coverage` cannot be combined with `--no-run` — there is no run to measure.",
        );
    }
    // v0.127 (editor-currency slice 6): the per-case run filter. An empty
    // `--case` is treated as unset (run all) rather than "match the empty name".
    let case_filter = case.filter(|c| !c.is_empty());
    // v0.114: the root seed for generative `property` tests, threaded to the
    // runner via `BYNK_TEST_SEED` (bare hex). An unparseable value is dropped
    // with a warning so a run still proceeds with a fresh seed.
    let seed_hex = match seed.as_deref() {
        Some(raw) => match normalise_seed(raw) {
            Some(hex) => Some(hex),
            None => {
                if !json {
                    eprintln!(
                        "{program} test: ignoring --seed `{raw}` (not a hex value like 0x5f3a)"
                    );
                }
                None
            }
        },
        None => None,
    };
    let output_root = output.unwrap_or_else(|| input.join("out"));
    if !input.is_dir() {
        eprintln!(
            "{program} test: input `{}` must be a project directory containing `.bynk` files",
            input.display()
        );
        return ExitCode::FAILURE;
    }
    // v0.9.1: rooting strategy (#46: shared with check/compile via
    // `project_options`) — a `bynk.toml` or `src/` subdir selects split-paths
    // mode (sources under `[paths] src`, tests under `[paths] tests`); else the
    // legacy single-tree where `<input>` is both the source and tests root.
    // `--inspect` compiles a debug build: `.ts` import specifiers so the emitted
    // entry runs directly under Node's strip-only type-stripping (slice 2), where
    // slice 1's source maps apply unchanged. A normal run keeps `.js` specifiers
    // for the `tsc → node` path.
    let options = {
        // v0.115: `test` compiles the dev/test profile — the function
        // contract call-site guard is emitted (DECISION J). `compile`
        // leaves it off, so contract checks never reach production.
        // #2078: reconcile events against the committed `bynk.schema.lock`,
        // read-only — a non-additive change fails `test` as it fails a
        // build, and `out.schema_lock` is never written back.
        let o = match crate::try_project_options(&input)
            .map_err(|e| e.to_string())
            .and_then(|o| Ok(o.schema_registry(crate::schema_lock::read_only(&input)?)))
        {
            Ok(o) => o.contracts(true),
            Err(e) => {
                if json {
                    print!("{}", TestRun::runtime_error(e, None).render());
                } else {
                    eprintln!("{program} test: {e}");
                }
                return ExitCode::FAILURE;
            }
        };
        if inspect {
            o.import_ext(ImportExt::Ts)
        } else {
            o
        }
    };
    let out = match bynk_emit::project::compile_project(&options) {
        Ok(out) => out,
        Err(failure) => {
            if json {
                print!(
                    "{}",
                    TestRun::compile_error(crate::project_failure_short_lines(&failure)).render()
                );
            } else {
                crate::print_project_failure(&failure);
            }
            return ExitCode::FAILURE;
        }
    };
    // v0.67: `--no-run --format json` is pure discovery — render the suite/case
    // manifest the compile retained and stop. No TS is written, no `tsc`/`node`
    // runs, and the integration workers re-compile below is skipped (the manifest
    // already carries integration suites from the compile above). A compile
    // failure took the `compile`-error path above, exactly as a run would.
    if no_run && json {
        print!("{}", TestRun::discovered(discovery_suites(&out)).render());
        return ExitCode::SUCCESS;
    }

    // Write every artefact to disk under the output root.
    let mut wrote_any_test = false;
    let mut has_integration = false;
    for (path, doc) in &out.artefacts.docs {
        // Map-aware write (slice 2): carries the `.ts.map` siblings + trailers so
        // a debug run (`--inspect`) can resolve `.bynk` breakpoints. Harmless for a
        // normal run, which transpiles via `tsc` and ignores the trailer.
        if let Err(e) = crate::write_document(path, doc, &out.artefacts.docs, &output_root) {
            eprintln!(
                "{program} test: could not write `{}`: {e}",
                output_root.join(path).display()
            );
            return ExitCode::FAILURE;
        }
        let rel = path.to_string_lossy();
        if rel.starts_with("tests/") {
            wrote_any_test = true;
        }
        if rel.starts_with("tests/integration_") {
            has_integration = true;
        }
    }

    // v0.16: integration tests stand their participants up as real Workers, so
    // they import the workers-mode output (`workers/**`) and the serialise/
    // deserialise helpers the workers commons emit. The bundle compile above
    // does not produce those, so run a second compile in workers mode and
    // overlay everything except the `tests/` tree (whose unit modules import
    // the bundle output). The workers commons are a strict superset of the
    // bundle ones, so overwriting them is safe for the bundle code too.
    if has_integration {
        // v0.115/slice 2: reuse `options` (not a fresh `project_options(&input)`)
        // so this second compile keeps `contracts(true)` and, under `--inspect`,
        // `import_ext(Ts)` — a from-scratch rebuild silently dropped both.
        let workers_out =
            bynk_emit::project::compile_project(&options.clone().target(BuildTarget::Workers));
        let workers_out = match workers_out {
            Ok(o) => o,
            Err(failure) => {
                if json {
                    print!(
                        "{}",
                        TestRun::compile_error(crate::project_failure_short_lines(&failure))
                            .render()
                    );
                } else {
                    crate::print_project_failure(&failure);
                }
                return ExitCode::FAILURE;
            }
        };
        for (path, doc) in &workers_out.artefacts.docs {
            if path.to_string_lossy().starts_with("tests/") {
                continue;
            }
            if let Err(e) =
                crate::write_document(path, doc, &workers_out.artefacts.docs, &output_root)
            {
                eprintln!(
                    "{program} test: could not write `{}`: {e}",
                    output_root.join(path).display()
                );
                return ExitCode::FAILURE;
            }
        }
    }

    if !wrote_any_test {
        if json {
            print!("{}", empty_run().render());
        } else {
            eprintln!(
                "{program} test: no test declarations found in `{}`",
                input.display()
            );
        }
        return ExitCode::SUCCESS;
    }

    let main_ts = output_root.join("tests").join("main.ts");
    if no_run {
        // Rich `--no-run` is the CI emit helper: write the runner modules and
        // report where they landed. (JSON `--no-run` already returned above with
        // the discovery document — it never reaches here.)
        eprintln!("{program} test: tests emitted to {}", main_ts.display());
        return ExitCode::SUCCESS;
    }

    // Slice 2 (ADR 0104): launch the emitted `.ts` test entry directly under
    // Node's inspector. No `tsc` — the `.ts` runs under line-preserving
    // type-stripping, so the source maps written above resolve `.bynk`
    // breakpoints. Node prints its inspector URL; a debugger attaches there.
    if inspect {
        return run_inspect(
            program,
            &main_ts,
            seed_hex.as_deref(),
            case_filter.as_deref(),
        );
    }

    let tsconfig = output_root.join("tsconfig.json");
    // #854: coverage needs tsc's `.js.map`s (remap hop 1). Overwrite the default
    // tsconfig the compile wrote with the `sourceMap: true` variant, kept
    // coverage-only so a normal test run / deployment build ships no maps.
    if coverage
        && let Err(e) = std::fs::write(
            &tsconfig,
            bynk_emit::emitter::emit_tsconfig_with_source_maps(),
        )
    {
        return coverage_unsupported(
            program,
            json,
            format!("could not enable source maps for coverage: {e}"),
        );
    }
    // Preferred: `tsc -p out/tsconfig.json` → `node out-js/tests/main.js`.
    // tsc gives us full type-checking before execution and matches what a
    // production deployment build would do. If tsc is missing, fall back to
    // tsx, which compiles-and-runs in one step. We also try npx-mediated
    // variants so a developer with `npm` available doesn't need a global
    // install. If nothing works, emit an actionable error message.
    let out_js_root = output_root
        .parent()
        .map(|p| p.join("out-js"))
        .unwrap_or_else(|| PathBuf::from("out-js"));
    let main_js = out_js_root.join("tests").join("main.js");

    // #2146: the emitted modules are ES modules (`.js`-suffixed imports, no
    // `require`), but `out/` carries no `package.json` declaring
    // `"type": "module"`, so under the emitted tsconfig's `module: NodeNext`
    // tsc would down-level them to CommonJS. That transform reserves the
    // top-level names `require` and `exports` (and, from TypeScript 7,
    // `Object`), so a user declaration of one of them (`type Object`, #1653)
    // failed with TS2441. Compile as ESM instead, and mark `out-js` as ESM so
    // Node runs the result as such. The `out/` tree is left as the compiler
    // wrote it.
    if let Err(e) = write_esm_marker(&out_js_root) {
        let msg = format!(
            "could not write {}: {e}",
            out_js_root.join("package.json").display()
        );
        if json {
            print!("{}", TestRun::runtime_error(msg, None).render());
        } else {
            eprintln!("{program} test: {msg}");
        }
        return ExitCode::FAILURE;
    }

    // Try a sequence of (program, prefix args) tsc invocations. In JSON mode the
    // tsc step is captured so its output never reaches stdout (the document is
    // the only thing on stdout); a tsc failure on the emitted TS is a
    // toolchain/internal problem, surfaced as a `runtime` error.
    // #1672: the `npx` fallback provisions the TypeScript major the output is
    // verified against and users get, not an older pin.
    let typescript_pkg = format!("typescript@{}", bynk_emit::TYPESCRIPT_MAJOR_TESTED);
    let mut start_failures: Vec<String> = Vec::new();
    let tsc_runners: Vec<(&str, Vec<&str>)> = vec![
        ("tsc", vec![]),
        ("npx", vec!["--yes", "-p", typescript_pkg.as_str(), "tsc"]),
    ];
    for (prog, prefix) in &tsc_runners {
        let Some(path) = resolve_tool(prog) else {
            continue;
        };
        let mut cmd = ProcCommand::new(&path);
        for p in prefix {
            cmd.arg(p);
        }
        cmd.arg("-p").arg(&tsconfig);
        // #2146: emit ES modules whatever `package.json` scope `out/` is in.
        cmd.args(TSC_ESM_ARGS);
        let tsc_ok = if json {
            match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output() {
                Ok(out) if out.status.success() => true,
                Ok(out) => {
                    // tsc writes its diagnostics to *stdout*; capturing only
                    // stderr left the document's most useful field empty.
                    let mut detail = String::from_utf8_lossy(&out.stdout).into_owned();
                    let err_text = String::from_utf8_lossy(&out.stderr);
                    if !err_text.trim().is_empty() {
                        if !detail.is_empty() {
                            detail.push('\n');
                        }
                        detail.push_str(&err_text);
                    }
                    print!(
                        "{}",
                        TestRun::runtime_error(
                            "tsc rejected the generated TypeScript",
                            Some(detail),
                        )
                        .render()
                    );
                    return ExitCode::FAILURE;
                }
                Err(e) => {
                    start_failures.push(start_failure(&path, &e));
                    continue;
                }
            }
        } else {
            match cmd
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
            {
                Ok(s) if s.success() => true,
                Ok(_) => {
                    eprintln!(
                        "{program} test: tsc reported errors against {}",
                        tsconfig.display()
                    );
                    return ExitCode::FAILURE;
                }
                Err(e) => {
                    start_failures.push(start_failure(&path, &e));
                    continue;
                }
            }
        };
        if tsc_ok {
            let mut node_cmd = ProcCommand::new(crate::probe::program_path("node"));
            node_cmd.arg(&main_js);
            // #854: the coverage path owns the node launch (it sets
            // `NODE_V8_COVERAGE` and reads the result back), so it does not go
            // through `finish_runner`.
            if coverage {
                return run_with_coverage(
                    program,
                    node_cmd,
                    json,
                    seed_hex.as_deref(),
                    case_filter.as_deref(),
                    &out_js_root,
                    &output_root,
                    &input,
                );
            }
            return match finish_runner(node_cmd, json, seed_hex.as_deref(), case_filter.as_deref())
            {
                Ok(code) => code,
                Err(e) => {
                    if json {
                        print!(
                            "{}",
                            TestRun::runtime_error(format!("could not run node: {e}"), None)
                                .render()
                        );
                    } else {
                        eprintln!(
                            "{program} test: tsc succeeded but `node {}` failed: {e}",
                            main_js.display()
                        );
                    }
                    ExitCode::FAILURE
                }
            };
        }
    }

    // #854 DECISION C: with `--coverage`, the `tsc → node` path is required — do
    // not silently fall through to `tsx`, whose on-the-fly transform muddies
    // which map applies. Fail clearly instead.
    if coverage {
        // #1761 review: a `tsc` that is installed but won't start is reported as
        // such here too, not as missing.
        if !start_failures.is_empty() {
            return report_start_failures(program, json, &start_failures, true);
        }
        return coverage_unsupported(
            program,
            json,
            "`--coverage` requires `tsc` and `node` on PATH (the CI-shaped path with `.js.map`s); the `tsx` fallback is not supported for coverage.",
        );
    }

    // tsx fallback chain.
    let tsx_runners: Vec<(&str, Vec<&str>)> = vec![("tsx", vec![]), ("npx", vec!["--yes", "tsx"])];
    for (prog, prefix) in &tsx_runners {
        let Some(path) = resolve_tool(prog) else {
            continue;
        };
        let mut cmd = ProcCommand::new(&path);
        for p in prefix {
            cmd.arg(p);
        }
        cmd.arg(&main_ts);
        match finish_runner(cmd, json, seed_hex.as_deref(), case_filter.as_deref()) {
            Ok(code) => return code,
            Err(e) => start_failures.push(start_failure(&path, &e)),
        }
    }

    if !start_failures.is_empty() {
        return report_start_failures(program, json, &start_failures, false);
    }

    if json {
        print!(
            "{}",
            TestRun::runtime_error(
                "no test runner found: requires `tsc` (with Node.js) or `tsx` on PATH",
                None
            )
            .render()
        );
    } else {
        eprintln!(
            "{program} test: requires either `tsc` (with Node.js) or `tsx` on PATH. \
             Install one of:\n{}",
            install_advice(false)
        );
    }
    ExitCode::FAILURE
}

/// #1758: the run found runners but none would start. Each failure names the
/// resolved path and the error. In rich mode the install advice follows, since
/// installing another runner is often the fix for a broken one. `coverage`
/// limits that advice to `tsc`, the only runner `--coverage` accepts.
fn report_start_failures(
    program: &str,
    json: bool,
    failures: &[String],
    coverage: bool,
) -> ExitCode {
    if json {
        print!(
            "{}",
            TestRun::runtime_error("no test runner could be started", Some(failures.join("\n")))
                .render()
        );
    } else {
        eprintln!("{program} test: no test runner could be started:");
        for failure in failures {
            eprintln!("  - {failure}");
        }
        eprintln!("Or install another:\n{}", install_advice(coverage));
    }
    ExitCode::FAILURE
}

/// The install advice for a missing or broken runner, one indented line each.
/// With `coverage`, only `tsc`, since `--coverage` doesn't accept `tsx`.
fn install_advice(coverage: bool) -> String {
    let tsc = format!(
        "  - `npm install -g typescript@{}` (provides tsc, which type-checks; requires Node.js to run output)",
        bynk_emit::TYPESCRIPT_MAJOR_TESTED
    );
    if coverage {
        return tsc;
    }
    format!(
        "{tsc}\n  - `npm install -g tsx` (runs TypeScript in one step, without type-checking)\n  Or run inside a project where `npx tsc` / `npx tsx` resolves."
    )
}

/// A normal run with no suites — the JSON-mode document for a project with no
/// tests, or `--no-run`.
fn empty_run() -> TestRun {
    TestRun::empty()
}

/// v0.67: map the compile's retained test manifest into discovery [`Suite`]s for
/// the `--no-run --format json` document. Each case is `outcome: "discovered"`,
/// carrying its declaration `location` (when known) for editor click-through.
fn discovery_suites(out: &ProjectOutput) -> Vec<Suite> {
    out.discovered
        .iter()
        .map(|s| Suite {
            name: s.name.clone(),
            kind: s.kind.to_string(),
            cases: s
                .cases
                .iter()
                .map(|c| Case {
                    name: c.name.clone(),
                    outcome: "discovered".to_string(),
                    message: None,
                    location: c.location.as_ref().map(|l| Location {
                        path: l.path.clone(),
                        line: l.line,
                        col: l.col,
                    }),
                })
                .collect(),
        })
        .collect()
}

/// Execute the built runner command and produce its exit code. In JSON mode the
/// runner's stdout (NDJSON) and stderr are captured, folded into the pinned
/// document, and printed; otherwise stdio is inherited so the human ✓ / ✗ output
/// flows straight through. Either way the **exit code follows the runner's own
/// process status**, so a mid-run crash (a complete NDJSON prefix but no
/// `run-end`) is never reported as success.
fn finish_runner(
    mut cmd: ProcCommand,
    json: bool,
    seed_hex: Option<&str>,
    case: Option<&str>,
) -> std::io::Result<ExitCode> {
    if let Some(hex) = seed_hex {
        cmd.env("BYNK_TEST_SEED", hex);
    }
    if let Some(name) = case {
        cmd.env("BYNK_TEST_CASE", name);
    }
    if json {
        cmd.env("BYNK_TEST_FORMAT", "ndjson");
        let out = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output()?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let doc = crate::test_json::parse_ndjson(&stdout).into_document(&stderr);
        print!("{}", doc.render());
        Ok(exit_from(out.status.success()))
    } else {
        let status = cmd
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        Ok(exit_from(status.success()))
    }
}

fn exit_from(success: bool) -> ExitCode {
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// #854: a `--coverage` request that cannot be honoured (unsupported flag combo,
/// no `tsc → node`, or a setup error). In JSON mode it is a `runtime` error so
/// the document stays the only thing on stdout; in rich mode a plain stderr line.
fn coverage_unsupported(program: &str, json: bool, message: impl Into<String>) -> ExitCode {
    let message = message.into();
    if json {
        print!("{}", TestRun::runtime_error(message, None).render());
    } else {
        eprintln!("{program} test --coverage: {message}");
    }
    ExitCode::FAILURE
}

/// #854: run the emitted runner under V8 coverage and attribute the result to
/// `.bynk` source. Owns the `node` launch: it points `NODE_V8_COVERAGE` at a
/// scratch dir, runs the suite exactly as [`finish_runner`] would, then remaps
/// the V8 output through the emitted source maps ([`crate::coverage`]). In rich
/// mode the summary table is appended after the human ✓ / ✗ output; in JSON mode
/// the `coverage` block is folded into the pinned document. The **exit code
/// follows the run's own status** — coverage is a report about the run, never a
/// gate on it (a partial or unreadable map degrades the numbers, not the code).
#[allow(clippy::too_many_arguments)]
fn run_with_coverage(
    program: &str,
    mut cmd: ProcCommand,
    json: bool,
    seed_hex: Option<&str>,
    case: Option<&str>,
    out_js_root: &Path,
    out_root: &Path,
    source_root: &Path,
) -> ExitCode {
    // A scratch dir beside the build output; cleared first so a prior run's JSON
    // never leaks in. `NODE_V8_COVERAGE` writes one file per process on exit.
    let cov_dir = out_root.join(".v8-coverage");
    let _ = std::fs::remove_dir_all(&cov_dir);
    if let Err(e) = std::fs::create_dir_all(&cov_dir) {
        return coverage_unsupported(
            program,
            json,
            format!("could not create the coverage dir: {e}"),
        );
    }
    cmd.env("NODE_V8_COVERAGE", &cov_dir);
    if let Some(hex) = seed_hex {
        cmd.env("BYNK_TEST_SEED", hex);
    }
    if let Some(name) = case {
        cmd.env("BYNK_TEST_CASE", name);
    }

    let collect = || {
        crate::coverage::collect_coverage(&cov_dir, out_js_root, out_root, source_root)
            .unwrap_or_default()
    };

    let code = if json {
        cmd.env("BYNK_TEST_FORMAT", "ndjson");
        let out = match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output() {
            Ok(o) => o,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&cov_dir);
                return coverage_unsupported(program, true, format!("could not run node: {e}"));
            }
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let report = collect();
        let doc = crate::test_json::parse_ndjson(&stdout)
            .into_document(&stderr)
            .with_coverage(&report);
        print!("{}", doc.render());
        exit_from(out.status.success())
    } else {
        let status = cmd
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        let status = match status {
            Ok(s) => s,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&cov_dir);
                eprintln!("{program} test --coverage: could not run node: {e}");
                return ExitCode::FAILURE;
            }
        };
        let report = collect();
        print!("{}", crate::coverage::render_rich(&report));
        exit_from(status.success())
    };
    let _ = std::fs::remove_dir_all(&cov_dir);
    code
}

/// #2146: command-line overrides that make the runner's `tsc` emit ES modules.
/// Under the emitted tsconfig's `module: NodeNext`, the output format follows
/// the nearest `package.json`, and `out/` has none declaring `"type":
/// "module"`, so tsc would emit CommonJS (and reserve `require`, `exports` and
/// `Object` at module top level). `Bundler` resolution accepts the emitted
/// `.js`-suffixed imports exactly as `NodeNext` does. `moduleDetection force`
/// treats every file as a module, as Node does an ES module: an emitted file
/// with nothing to export (an empty commons part) is still one, and is
/// re-exported by its parent.
const TSC_ESM_ARGS: [&str; 6] = [
    "--module",
    "ES2022",
    "--moduleResolution",
    "Bundler",
    "--moduleDetection",
    "force",
];

/// #2146: mark the tsc output tree as ES modules, so Node runs the ESM that
/// [`TSC_ESM_ARGS`] makes tsc emit.
fn write_esm_marker(out_js_root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(out_js_root)?;
    std::fs::write(
        out_js_root.join("package.json"),
        "{\n  \"type\": \"module\"\n}\n",
    )
}

/// Node's default inspector port.
const NODE_INSPECT_PORT: u16 = 9229;

/// The `--inspect-brk` flag for `run_inspect` (#2115). Node's default port,
/// 9229, when it is free, so a debugger that looks there by default (Chrome's
/// `chrome://inspect`) still finds the runner. When something else holds it
/// (another inspected process, a stray `workerd`), plain `--inspect-brk` would
/// print `Starting inspector on 127.0.0.1:9229 failed: address already in use`
/// and run the tests *without* an inspector, so no debugger could attach. Then
/// ask for port 0: Node picks a free one and prints it in the `ws://` URL, which
/// is what attaching clients (the VS Code extension included) read.
fn inspect_brk_arg(default_port_free: bool) -> &'static str {
    if default_port_free {
        "--inspect-brk"
    } else {
        "--inspect-brk=127.0.0.1:0"
    }
}

/// Slice 2 (ADR 0104): launch the emitted `.ts` test entry under Node's inspector
/// and hand off. Node prints its inspector `ws://` URL to stderr and pauses at the
/// first line (`--inspect-brk`) until a JavaScript debugger attaches; breakpoints
/// set in `.bynk` resolve through the emitted source maps. `--experimental-strip-types`
/// runs the `.ts` directly under line-preserving type-stripping (Node ≥ 22.6;
/// unflagged ≥ 23.6) — no `tsc`, so slice 1's `.ts.map` applies to the running file.
fn run_inspect(
    program: &str,
    entry: &Path,
    seed_hex: Option<&str>,
    case: Option<&str>,
) -> ExitCode {
    let Some(node) = resolve_tool("node") else {
        eprintln!("{program} test --inspect: `node` was not found on PATH");
        return ExitCode::FAILURE;
    };
    eprintln!("{program} test --inspect: launching the test runner under Node's inspector.");
    eprintln!("  Attach a JavaScript debugger to the inspector URL below; breakpoints set");
    eprintln!("  in `.bynk` sources resolve through the emitted source maps.");
    eprintln!("  (Requires Node \u{2265} 22.6 for TypeScript type-stripping.)");
    let mut cmd = ProcCommand::new(node);
    if let Some(hex) = seed_hex {
        cmd.env("BYNK_TEST_SEED", hex);
    }
    if let Some(name) = case {
        cmd.env("BYNK_TEST_CASE", name);
    }
    let default_free = std::net::TcpListener::bind(("127.0.0.1", NODE_INSPECT_PORT)).is_ok();
    if !default_free {
        eprintln!(
            "  Port {NODE_INSPECT_PORT} is in use, so the inspector takes a free port; use the URL below."
        );
    }
    cmd.arg("--experimental-strip-types")
        .arg(inspect_brk_arg(default_free))
        .arg(entry);
    match cmd
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
    {
        Ok(s) => exit_from(s.success()),
        Err(e) => {
            eprintln!("{program} test --inspect: could not run node: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod inspect_port_tests {
    use super::inspect_brk_arg;

    #[test]
    fn uses_the_default_port_when_free_and_a_free_port_otherwise() {
        assert_eq!(inspect_brk_arg(true), "--inspect-brk");
        assert_eq!(inspect_brk_arg(false), "--inspect-brk=127.0.0.1:0");
    }
}
