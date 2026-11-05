//! v0.9.1: TypeScript verification of emitted output.
//!
//! For every positive fixture, project-form and (#1767) single-file alike,
//! compile to a temp directory and run `tsc --strict --noEmit` over the
//! result. Any TypeScript error fails the fixture and surfaces the raw `tsc`
//! output.
//!
//! This catches emitter bugs that visual review (and the snapshot tests)
//! miss — it's the backstop for every emission change. Per the v0.9.1 spec
//! §4.3:
//!
//! - If `tsc` is unavailable in the environment, the stage **must skip
//!   loudly** so a CI breakage isn't quietly mistaken for green.
//! - In CI the skip is a hard failure; locally it's permitted with a
//!   visible warning. Setting `BYNK_REQUIRE_TSC=1` enforces the strict CI
//!   behaviour; otherwise an unavailable tsc prints a warning and passes.
//! - #1671: the same switch governs the Node strip-types checks (ADR 0136's
//!   strip-only invariant, which `bynkc test --inspect` and in-browser eval
//!   depend on). Under it, a missing Node, or one too old for
//!   `stripTypeScriptTypes` (< 22.13), fails rather than skips.

mod require;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

/// Whether the local `node` exists and is recent enough to be worth invoking for
/// a strip check. The committed `strip_check.mjs` harness performs the precise
/// API gate (`stripTypeScriptTypes`, Node ≥ 22.13) and exits 2 when it is
/// unavailable; this coarse check just avoids spawning an ancient/absent Node.
fn node_present() -> bool {
    Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(Clone)]
struct TscRunner {
    program: String,
    args: Vec<String>,
}

fn discover_tsc() -> Option<TscRunner> {
    if tool_exists("tsc") {
        return Some(TscRunner {
            program: "tsc".to_string(),
            args: vec![],
        });
    }
    if tool_exists("npx") {
        // Pin TypeScript to avoid surprising upgrades: #1672, the newest
        // verified major, the one users get. CI installs both verified majors
        // explicitly, so this fallback is for a local run without `tsc`.
        return Some(TscRunner {
            program: "npx".to_string(),
            args: vec![
                "--yes".to_string(),
                "-p".to_string(),
                format!("typescript@{}", bynk_emit::TYPESCRIPT_MAJOR_TESTED),
                "tsc".to_string(),
            ],
        });
    }
    None
}

/// Build a `Command` for `program`, routing through `cmd /C` on Windows so
/// npm's `.cmd` shims (`tsc.cmd`, `npx.cmd`) resolve — Rust's CreateProcess
/// deliberately refuses to run batch scripts directly (the BatBadBut
/// hardening), so a bare `Command::new("npx")` fails there.
fn base_command(program: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    }
}

fn tool_exists(name: &str) -> bool {
    // `where` is the Windows counterpart of `which`.
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn fixture_dirs(category: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(category);
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(&root) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn fixture_target(dir: &Path) -> bynkc::BuildTarget {
    let marker = dir.join("target.txt");
    if let Ok(s) = fs::read_to_string(&marker)
        && s.trim() == "workers"
    {
        return bynkc::BuildTarget::Workers;
    }
    bynkc::BuildTarget::Bundle
}

/// v0.18: read the deploy-platform marker from a fixture root, if present.
fn fixture_platform(dir: &Path) -> bynkc::Platform {
    let marker = dir.join("platform.txt");
    if let Ok(s) = fs::read_to_string(&marker) {
        match s.trim() {
            "node" => return bynkc::Platform::Node,
            "browser" => return bynkc::Platform::Browser,
            _ => {}
        }
    }
    bynkc::Platform::Cloudflare
}

fn compile_fixture(
    fixture_root: &Path,
    target: bynkc::BuildTarget,
) -> Result<bynkc::ProjectOutput, Vec<bynkc::CompileError>> {
    let bynk_toml = fixture_root.join("bynk.toml");
    if bynk_toml.exists() {
        let paths =
            bynkc::try_read_project_paths(fixture_root).expect("well-formed fixture manifest");
        bynkc::compile_project(
            &bynk_testkit::compile_options_split(fixture_root.to_path_buf(), paths).target(target),
        )
        .map_err(bynkc::ProjectFailure::flatten)
    } else {
        let src_dir = fixture_root.join("src");
        let platform = fixture_platform(fixture_root);
        bynkc::compile_project(
            &bynk_testkit::compile_options_single(src_dir)
                .target(target)
                .platform(platform),
        )
        .map_err(bynkc::ProjectFailure::flatten)
    }
}

fn write_outputs(out: &bynkc::ProjectOutput, root: &Path) -> std::io::Result<()> {
    for (path, doc) in &out.artefacts.docs {
        let target = root.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        // Only write TypeScript / JSON / TOML artefacts; `tsc` will
        // process the .ts and read tsconfig.json. wrangler.toml is left
        // in place but ignored by tsc.
        fs::write(&target, doc.text())?;
    }
    Ok(())
}

/// The heap ceiling for the single-pass `tsc`, in MiB.
///
/// The single pass type-checks every staged fixture (507 at the time of
/// writing) in one process, and its working set grows with the corpus. On
/// 15 October 2026 it crossed Node's default old-space limit (~2 GiB) on the
/// `macos-latest` runner: `FATAL ERROR: Ineffective mark-compacts near heap
/// limit`, reported as an unattributable tsc failure, failing every push to
/// `main` from then on. (Linux and Windows still fitted, and PRs don't run
/// macOS, so it first showed up post-merge.) 4 GiB leaves headroom on the
/// 7 GB macOS runner alongside nextest's other test processes.
const TSC_MAX_OLD_SPACE_MIB: u32 = 4096;

/// `NODE_OPTIONS` for the `tsc` child: the caller's own options, plus
/// [`TSC_MAX_OLD_SPACE_MIB`] unless the caller already set a heap limit (an
/// explicit local choice wins).
fn tsc_node_options() -> String {
    let existing = std::env::var("NODE_OPTIONS").unwrap_or_default();
    if existing.contains("--max-old-space-size") {
        existing
    } else {
        format!("{existing} --max-old-space-size={TSC_MAX_OLD_SPACE_MIB}")
            .trim()
            .to_string()
    }
}

fn run_tsc_in(runner: &TscRunner, project_dir: &Path) -> (bool, String) {
    run_tsc_with_config(runner, &project_dir.join("tsconfig.json"))
}

fn run_tsc_with_config(runner: &TscRunner, tsconfig: &Path) -> (bool, String) {
    let mut cmd = base_command(&runner.program);
    for a in &runner.args {
        cmd.arg(a);
    }
    cmd.arg("--strict").arg("--noEmit").arg("-p").arg(tsconfig);
    cmd.env("NODE_OPTIONS", tsc_node_options());
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => return (false, format!("could not launch tsc: {e}")),
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let combined = if stderr.is_empty() {
        stdout
    } else if stdout.is_empty() {
        stderr
    } else {
        format!("{stdout}\n{stderr}")
    };
    (output.status.success(), combined)
}

/// #1767: stage a single-file fixture in `fixture_root`: its compiled output
/// as `main.ts`, and beside it a copy of `emit_runtime_module()`'s
/// `runtime.ts`, written here for each such fixture. `Ok(false)` when `dir`
/// holds no `input.bynk`; `Err` with a failure section when it can't be read,
/// doesn't compile, or can't be written.
fn stage_single_file(dir: &Path, fixture_root: &Path) -> Result<bool, String> {
    let input = dir.join("input.bynk");
    if !input.is_file() {
        return Ok(false);
    }
    let source = fs::read_to_string(&input).map_err(|e| {
        format!(
            "\n=== {} ===\nfailed to read {}: {e}",
            dir.display(),
            input.display()
        )
    })?;
    let name = input.display().to_string();
    let ts = bynkc::compile(&source, &name).map_err(|errors| {
        format!(
            "\n=== {} ===\nexpected compile success but got errors:\n{}",
            dir.display(),
            bynkc::render_errors(&errors, &source, &name),
        )
    })?;
    let write = || -> std::io::Result<()> {
        fs::create_dir_all(fixture_root)?;
        fs::write(fixture_root.join("main.ts"), &ts)?;
        bynk_testkit::write_runtime_modules(fixture_root)
    };
    write().map_err(|e| {
        format!(
            "\n=== {} ===\nfailed to stage emitted output in {}: {e}",
            dir.display(),
            fixture_root.display(),
        )
    })?;
    Ok(true)
}

/// Run `tsc --strict --noEmit` against every positive fixture's emitted
/// TypeScript, project-form and (#1767) single-file alike. Fixtures that
/// compile but don't type-check are failures here.
///
/// **One `tsc` for all fixtures.** Spawning `tsc` per fixture paid its multi-
/// second startup (process launch + `lib.*.d.ts` load + program construction)
/// ~165 times over — the dominant cost, while the actual type-check of each
/// small fixture is cheap. Instead we stage every fixture into its own subdir
/// under one temp root and type-check the whole tree in a single `tsc`
/// invocation, so that startup is paid once. The fixtures stay isolated: every
/// import in the emitted output is relative (`./`, `../`) so nothing resolves
/// across subdir boundaries, and the emitted modules declare no `declare
/// global` / ambient / triple-slash surface that would collide in one program.
#[test]
fn emitted_typescript_passes_tsc_strict() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            let require = require::is_required(REQUIRE_ENV);
            eprintln!(
                "\n!!! TYPESCRIPT VERIFICATION SKIPPED !!!\n\
                 neither `tsc` nor `npx` is on PATH.\n\
                 Emitted TypeScript was NOT type-checked.\n\
                 Install TypeScript locally (`npm i -g typescript`) or run in an environment with `npx`.\n"
            );
            if require {
                panic!(
                    "{REQUIRE_ENV} is set but no tsc runner was found — refusing to skip TypeScript verification",
                );
            }
            return;
        }
    };

    let dirs = fixture_dirs("positive");
    assert!(!dirs.is_empty(), "no positive fixtures found");

    // One temp root; each fixture is staged into `root/<fixture-name>/…`,
    // preserving its emitted layout (its own `runtime.ts`, `src/…`, etc.).
    let root = std::env::temp_dir().join(format!("bynk-tsc-verify-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create tsc-verify temp root");

    let mut failures: Vec<String> = Vec::new();
    // Fixture names successfully staged into the tree — the set the single
    // `tsc` run actually type-checks.
    let mut staged: Vec<String> = Vec::new();
    for dir in &dirs {
        let name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("fixture")
            .to_string();
        let src_dir = dir.join("src");
        if !src_dir.is_dir() {
            // #1767: a single-file fixture (`input.bynk` → one `.ts`) imports
            // nothing but `./runtime.js`, so staged beside the runtime it is a
            // complete module graph like any project's.
            match stage_single_file(dir, &root.join(&name)) {
                Ok(true) => staged.push(name),
                Ok(false) => {}
                Err(failure) => failures.push(failure),
            }
            continue;
        }
        let target = fixture_target(dir);
        let compiled = match compile_fixture(dir, target) {
            Ok(out) => out,
            Err(errors) => {
                let rendered = bynkc::render_project_errors(&errors);
                failures.push(format!(
                    "\n=== {} ===\nexpected compile success but got errors:\n{}",
                    dir.display(),
                    rendered,
                ));
                continue;
            }
        };
        let fixture_root = root.join(&name);
        if let Err(e) = write_outputs(&compiled, &fixture_root) {
            failures.push(format!(
                "\n=== {} ===\nfailed to write emitted output to {}: {e}",
                dir.display(),
                fixture_root.display(),
            ));
            continue;
        }
        staged.push(name);
    }

    // A root `tsconfig.json` whose `include: ["**/*.ts"]` (from
    // `emit_tsconfig`) sweeps every staged fixture subdir, so one `tsc`
    // checks them all. The per-fixture `tsconfig.json` files in each subdir
    // are inert here — `tsc -p` consults only the root config.
    fs::write(
        root.join("tsconfig.json"),
        bynk_emit::emitter::emit_tsconfig(),
    )
    .expect("write root tsconfig");

    let (ok, output) = run_tsc_in(&runner, &root);
    if !ok {
        // Attribute each error to the fixture whose subdir name appears in the
        // diagnostic path, so a failure still points at the offending fixture
        // rather than a wall of paths.
        let mut blamed: Vec<&String> = staged
            .iter()
            .filter(|name| {
                output.contains(&format!("{name}/")) || output.contains(&format!("{name}\\"))
            })
            .collect();
        blamed.sort();
        let blame_line = if blamed.is_empty() {
            "(could not attribute errors to a specific fixture — see output)".to_string()
        } else {
            format!(
                "fixtures with errors: {}",
                blamed
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        failures.push(format!(
            "\n=== single-pass tsc --strict --noEmit reported errors ===\n{blame_line}\n{output}",
        ));
    } else {
        // Best-effort cleanup; leave on failure for inspection.
        let _ = fs::remove_dir_all(&root);
    }

    let checked = if ok { staged.len() } else { 0 };
    assert!(
        !staged.is_empty(),
        "no positive fixtures were staged for tsc"
    );
    if !failures.is_empty() {
        panic!(
            "tsc verification failed ({} fixtures staged, {} clean, {} failure section(s)):\n{}",
            staged.len(),
            checked,
            failures.len(),
            failures.join("\n"),
        );
    }
    // #1767: every fixture is staged, whatever its layout. Until #1767 the
    // single-file ones were skipped, and `734_is_bindings_and_chain` carried
    // TS2339s in its golden unnoticed. Checked after `failures`, so a fixture
    // that failed to compile reports its errors rather than this count.
    assert_eq!(
        staged.len(),
        dirs.len(),
        "every positive fixture must be staged for tsc"
    );
}

/// #1538: `bynk_ts::verbatim_violations` scans a `Verbatim`-tagged text blob
/// for the six constructs R7.1 forbids the tree from ever representing
/// (`enum`, `namespace`, a decorator, a constructor parameter property,
/// `: any`/`as any`) — a byte-golden fixture is blind to what's *inside* an
/// opaque block, so this is the one check that actually watches it. Written
/// (#1307/#1387) and left uncalled: `lint.rs`'s own module doc said wiring it
/// in was "meaningful only once real `Verbatim` content exists to check" —
/// that trigger fired (`project.rs`'s adapter-binding copy loop and
/// `runtime.ts` staging, both `TsStmt::verbatim`; #1539 added `TsExpr::
/// VerbatimExpr` sites in `bynk-emit/src/emitter/emit.rs`) with nothing
/// wired up since.
///
/// Runs `verbatim_violations` over every `Verbatim`/`VerbatimExpr` leaf
/// found by walking each positive fixture's compiled `Document::Ts` tree
/// (`TsProgram::verbatim_content`, #1538) — not the tree's whole printed
/// text. An earlier version of this test scanned each document's entire
/// printed output instead, on the theory that a real (non-`Verbatim`) node
/// can never *produce* one of the six banned constructs, so scanning
/// everything would find exactly what scanning only the opaque portions
/// would. That theory is false: `TsType::named("any")` is a real,
/// structured node — `bynk-emit`'s own already-tracked `any` residue
/// (`xtask`'s `ts_any` gated probe) — that prints the literal substring
/// `any` from real tree structure, not a smuggled `Verbatim` blob; scanning
/// whole documents flagged ~571 such lines across the fixture corpus, almost
/// none of them an actual escape-hatch violation. Walking only the opaque
/// leaves is what this check is actually for, and is what #1538 asked for:
/// every `Verbatim`/`VerbatimExpr` leaf, checked, not trusted by
/// construction.
///
/// **Every origin is linted, including `NotYetConverted`.** #1538 names the
/// two `NotYetConverted` sites specifically (`runtime.ts` and each
/// platform's `bynk-<platform>.ts` adapter binding) as the ones that were
/// "trusted by construction" — and that trust was misplaced: both carried a
/// real `(... as any)` cast. `verbatim_sites`' own argued floor ("neither
/// ever generated by bynk-emit") is about why that content can't be
/// *converted* to real tree nodes; it says nothing about whether its content
/// gets to violate R7.1's `any` rule, whose rationale (an `any` anywhere in
/// emitted output blinds `tsc --strict`, the strip-only fast path can't
/// erase it either way) applies to shipped runtime/binding code exactly as
/// much as to tree output. Both real violations are fixed at their source
/// (`bynk-emit/runtime/src/boundary.ts`, re-bundled via `npm run bundle`;
/// `bynk-std/lib/core/bindings/bynk-{node,cloudflare}.ts`), not
/// exempted here.
///
/// **A known, separate gap this check cannot yet close:** no
/// `VerbatimOrigin::Emit` leaf (#1539's expression-level residue,
/// `bynk-emit/src/emitter/emit.rs`) can currently reach a returned
/// `TsProgram` at all — every real construction site builds the node, prints
/// it immediately (`bynk_ts::print_expr`/`print_stmt`), and splices the
/// resulting text into a plain `String` buffer that a caller further up
/// wraps in `TsStmt::raw` (no `VerbatimOrigin`, walked but never attributed
/// — see `TsStmtKind::Raw`'s own doc). `TsExpr::VerbatimExpr`'s own doc
/// promises this content is "visible to `crate::verbatim_violations`"; today
/// that's only true at the moment of construction, not once the surrounding
/// document is returned. Closing it needs a real Arc C conversion slice
/// (keeping `emit.rs`'s own `TsStmt`/`TsExpr` construction live instead of
/// printing it away), not a change to this test.
///
/// No `tsc` dependency — a pure Rust-side text scan, so this always runs,
/// `tsc`/`npx` on `PATH` or not.
#[test]
fn emitted_typescript_has_no_verbatim_violations() {
    let dirs = fixture_dirs("positive");
    assert!(!dirs.is_empty(), "no positive fixtures found");

    let mut failures: Vec<String> = Vec::new();
    let mut leaves_checked = 0usize;
    for dir in &dirs {
        let target = fixture_target(dir);
        // A fixture that fails to compile has no output to check here —
        // `emitted_typescript_passes_tsc_strict` (and the checker/compile
        // stages themselves) already cover that failure mode.
        let Ok(compiled) = compile_fixture(dir, target) else {
            continue;
        };
        for (path, doc) in &compiled.artefacts.docs {
            let bynkc::Document::Ts(program) = doc else {
                continue;
            };
            for (origin, text) in program.verbatim_content() {
                leaves_checked += 1;
                let violations = bynk_ts::verbatim_violations(text);
                if !violations.is_empty() {
                    failures.push(format!(
                        "\n=== {} :: {} ({origin:?}) ===\n{}",
                        dir.display(),
                        path.display(),
                        violations
                            .iter()
                            .map(|v| format!("  [{}] {}", v.construct, v.line))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ));
                }
            }
        }
    }
    assert!(
        leaves_checked > 0,
        "no non-NotYetConverted Verbatim/VerbatimExpr leaves were found to check"
    );
    if !failures.is_empty() {
        panic!(
            "verbatim_violations found R7.1-forbidden constructs in emitted TypeScript \
             ({} finding(s), {} leaf/leaves checked):\n{}",
            failures.len(),
            leaves_checked,
            failures.join("\n"),
        );
    }
}

/// v0.48: the embedded runtime (`emitter/runtime.ts`) — which now
/// carries the Bearer JWT verifier — must pass `tsc --strict` standalone, not
/// only transitively inside a fixture. It is self-contained (no imports), so a
/// temp dir with just `runtime.ts` + the emitted `tsconfig.json` type-checks it
/// in isolation. (The `.ts` *bindings* import the emitted adapter surface, so
/// they are checked beside a project built for them: `binding_scaffold.rs`,
/// standard-library S2, #1952.)
/// #1704: a context's rebrand of a `uses`-commons type carries an *optional*
/// brand (`Point = __CommonsPoint & { readonly __ctxBrand?: "left" }`), so a
/// plain literal or commons value fits it. The brand's job is still to keep two
/// contexts' views of one commons type apart, and no golden can show a
/// rejection, so this stages `1704_context_brands_distinct` with a probe module
/// asserting both halves under `tsc --strict`.
#[test]
fn context_brands_admit_commons_values_and_reject_other_contexts() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! TYPESCRIPT VERIFICATION SKIPPED (context brands) !!!\n\
                 neither `tsc` nor `npx` is on PATH.\n"
            );
            if require::is_required(REQUIRE_ENV) {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/1704_context_brands_distinct");
    let out = compile_fixture(&fixture, bynkc::BuildTarget::Bundle)
        .unwrap_or_else(|e| panic!("fixture failed to compile: {e:?}"));
    let tmp = std::env::temp_dir().join(format!("bynk-brand-tsc-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    write_outputs(&out, &tmp).unwrap();
    fs::write(
        tmp.join("brand_probe.ts"),
        r#"import type { Point as GeoPoint } from "./geo.js";
import type { Point as LeftPoint } from "./left.js";
import type { Point as RightPoint } from "./right.js";

declare const plain: GeoPoint;
declare const left: LeftPoint;

// A commons value and a literal fit a context's branded type.
export const fromCommons: LeftPoint = plain;
export const fromLiteral: LeftPoint = { x: 1, y: 2 };
// A context's value still fits the commons type.
export const toCommons: GeoPoint = left;
// @ts-expect-error another context's brand is rejected
export const crossed: RightPoint = left;
"#,
    )
    .unwrap();
    let (ok, output) = run_tsc_in(&runner, &tmp);
    if ok {
        let _ = fs::remove_dir_all(&tmp);
    }
    assert!(ok, "context brand probe failed tsc --strict:\n{output}");
}

#[test]
fn embedded_runtime_passes_tsc_strict_standalone() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            let require = require::is_required(REQUIRE_ENV);
            eprintln!(
                "\n!!! TYPESCRIPT VERIFICATION SKIPPED (runtime standalone) !!!\n\
                 neither `tsc` nor `npx` is on PATH.\n"
            );
            if require {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    let tmp = std::env::temp_dir().join(format!("bynk-runtime-tsc-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    bynk_testkit::write_runtime_modules(&tmp).unwrap();
    fs::write(
        tmp.join("tsconfig.json"),
        bynk_emit::emitter::emit_tsconfig(),
    )
    .unwrap();
    let (ok, output) = run_tsc_in(&runner, &tmp);
    if ok {
        let _ = fs::remove_dir_all(&tmp);
    }
    assert!(
        ok,
        "embedded runtime.ts failed tsc --strict --noEmit:\n{output}"
    );
}

/// v0.104 (real-time track slice 3b): the embedded runtime must also be
/// **type-strippable** — Node runs the emitted `.ts` directly under
/// `--experimental-strip-types` on the `--inspect` debug path (and the bundle
/// test runner), which is *stricter* than `tsc`: it rejects non-erasable TS such
/// as constructor **parameter properties**, `enum`, and `namespace`. `tsc`
/// accepts all of these, so the standalone-tsc check above does not catch them; a
/// parameter property in the runtime silently broke every `--inspect` debug
/// session (the module fails to parse before any breakpoint binds). This guards
/// that class of regression cheaply and deterministically — no debugger, no hang.
#[test]
fn embedded_runtime_strips_types_under_node() {
    if !node_present() {
        // #1671: fails rather than skips under `BYNK_REQUIRE_TSC`.
        if require::is_required(REQUIRE_ENV) {
            panic!(
                "{REQUIRE_ENV} is set but `node` is not on PATH — refusing to skip the \
                 strip-types check"
            );
        }
        eprintln!("\n!!! NODE STRIP-TYPES CHECK SKIPPED (runtime) !!!\n`node` is not on PATH.\n");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-runtime-strip-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    bynk_testkit::write_runtime_modules(&tmp).unwrap();
    // The same oracle as `all_emitted_typescript_strips_under_node`, not
    // `node --experimental-strip-types --check`: Node 24's `--check` doesn't
    // strip types at all, so it rejects the first type annotation in the file.
    let out = run_strip_check(&tmp);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    match out.status.code() {
        Some(0) => {
            let _ = fs::remove_dir_all(&tmp);
        }
        Some(2) => {
            if require::is_required(REQUIRE_ENV) {
                panic!(
                    "{REQUIRE_ENV} is set but `stripTypeScriptTypes` is unavailable (Node \
                     >= 22.13 is required) — refusing to skip the strip-types check:\n{}",
                    stderr.trim()
                );
            }
            eprintln!(
                "\n!!! NODE STRIP-TYPES CHECK SKIPPED (runtime) !!!\n{}\n",
                stderr.trim()
            );
            let _ = fs::remove_dir_all(&tmp);
        }
        _ => panic!(
            "embedded runtime.ts is not type-strippable (a non-erasable construct such as a \
             constructor parameter property, `enum`, or `namespace` — these break every \
             `--inspect` debug session):\n{}\n{}",
            stdout.trim(),
            stderr.trim(),
        ),
    }
}

/// Run the committed strip-only harness (`tests/support/strip_check.mjs`) over
/// every `.ts` under `root`: exit 0 when all strip, 1 on a non-erasable file,
/// 2 when `stripTypeScriptTypes` is unavailable (Node < 22.13).
fn run_strip_check(root: &Path) -> std::process::Output {
    let harness = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("support")
        .join("strip_check.mjs");
    Command::new("node")
        .arg("--no-warnings")
        .arg(&harness)
        .arg(root)
        .output()
        .expect("run strip_check.mjs")
}

/// In-browser track, slice 0 — the **strip-only emission invariant** (ADR 0136):
/// *every* `.ts` bynkc emits must be erasable by pure type-stripping, not just the
/// embedded `runtime.ts` covered above. Node runs emitted `.ts` directly under
/// `--experimental-strip-types` on the `--inspect` debug path (and the in-browser
/// eval this track is built toward), which rejects non-erasable TS — parameter
/// properties, `enum`, `namespace`. `tsc` accepts all of those, so
/// `emitted_typescript_passes_tsc_strict` cannot catch them; this test is the
/// guard that makes the invariant load-bearing across the whole emitted surface:
/// the de-sugared provider `given` constructor (`emit.rs`), the first-party
/// bindings (`bynk-{cloudflare,node}.ts`, `cloudflare.binding.ts`), the emitted
/// test scaffolding, and any user binding copied into a fixture.
///
/// Every positive fixture, project-form and (#1767) single-file, is compiled
/// and its emitted output staged into one temp root; then each `.ts` is
/// checked by `strip_check.mjs` (`node:module`'s `stripTypeScriptTypes`) in
/// one Node process.
#[test]
fn all_emitted_typescript_strips_under_node() {
    if !node_present() {
        // #1671: fails rather than skips under `BYNK_REQUIRE_TSC`.
        if require::is_required(REQUIRE_ENV) {
            panic!(
                "{REQUIRE_ENV} is set but `node` is not on PATH — refusing to skip the \
                 strip-types check"
            );
        }
        eprintln!(
            "\n!!! NODE STRIP-TYPES CHECK SKIPPED (all emitted output) !!!\n\
             `node` is not on PATH.\n"
        );
        return;
    }

    let dirs = fixture_dirs("positive");
    assert!(!dirs.is_empty(), "no positive fixtures found");

    let root = std::env::temp_dir().join(format!("bynk-strip-verify-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create strip-verify temp root");

    let mut staged: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for dir in &dirs {
        let name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("fixture")
            .to_string();
        // #1767: a single-file fixture's output is the same emitter's, so the
        // strip-only invariant governs it too.
        if !dir.join("src").is_dir() {
            match stage_single_file(dir, &root.join(&name)) {
                Ok(true) => staged.push(name),
                Ok(false) => {}
                Err(failure) => failures.push(failure),
            }
            continue;
        }
        let target = fixture_target(dir);
        match compile_fixture(dir, target) {
            Ok(out) => {
                let fixture_root = root.join(&name);
                if let Err(e) = write_outputs(&out, &fixture_root) {
                    failures.push(format!(
                        "\n=== {} ===\nfailed to write emitted output: {e}",
                        dir.display()
                    ));
                } else {
                    staged.push(name);
                }
            }
            Err(errors) => failures.push(format!(
                "\n=== {} ===\nexpected compile success but got errors:\n{}",
                dir.display(),
                bynkc::render_project_errors(&errors),
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "staging for strip verification failed:\n{}",
        failures.join("\n")
    );
    assert!(
        !staged.is_empty(),
        "no positive fixtures were staged for strip verification"
    );
    // #1767: every fixture is staged, as in `emitted_typescript_passes_tsc_strict`.
    assert_eq!(
        staged.len(),
        dirs.len(),
        "every positive fixture must be staged for strip verification"
    );

    // One Node process strips the whole staged tree (see strip_check.mjs).
    let out = run_strip_check(&root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    match out.status.code() {
        Some(0) => {
            let _ = fs::remove_dir_all(&root);
        }
        // `stripTypeScriptTypes` unavailable (Node < 22.13): skip loudly, like the
        // `tsc`-presence and runtime strip gates above — and, #1671, fail instead
        // under `BYNK_REQUIRE_TSC`.
        Some(2) => {
            if require::is_required(REQUIRE_ENV) {
                panic!(
                    "{REQUIRE_ENV} is set but `stripTypeScriptTypes` is unavailable (Node \
                     >= 22.13 is required) — refusing to skip the strip-types check:\n{}",
                    stderr.trim()
                );
            }
            eprintln!(
                "\n!!! NODE STRIP-TYPES CHECK SKIPPED (all emitted output) !!!\n\
                 {}\n",
                stderr.trim()
            );
            let _ = fs::remove_dir_all(&root);
        }
        _ => panic!(
            "emitted TypeScript is not strip-removable — a non-erasable construct (a constructor \
             parameter property, `enum`, or `namespace`) breaks both `--inspect` debug sessions \
             and the in-browser eval path. De-sugar it in the emitter (e.g. a declared field + \
             assigning constructor instead of a parameter property).\nFAIL <file> <code> \
             <reason>:\n{}\n{}",
            stdout.trim(),
            stderr.trim(),
        ),
    }
}

// ---------------------------------------------------------------------------
// In-browser track, slice 1 — the first-class JS artefact (ADR 0137).
//
// `bynkc compile --emit js` type-strips the emitted TypeScript to JavaScript via
// `bynkc::strip_project_to_js` (oxc). These two tests guard that path: the first
// is pure-Rust and always runs (the strip must succeed on every fixture and yield
// a JS-shaped tree); the second runs the emitted `.js` through `node --check` to
// confirm it is valid, runnable ES module syntax with no TypeScript residue.
// ---------------------------------------------------------------------------

/// Every project fixture must strip to a JavaScript artefact without error, and
/// the result must be JS-shaped: no `.ts` files survive, the `tsconfig.json` is
/// dropped, and at least one `.js` module is produced. Pure Rust — the primary
/// guard, run even where Node is absent. A strip failure here means the emitter
/// produced TypeScript oxc could not erase (it should be strip-only — ADR 0136).
#[test]
fn js_artefact_strips_every_fixture_to_js_shape() {
    let dirs = fixture_dirs("positive");
    assert!(!dirs.is_empty(), "no positive fixtures found");

    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for dir in &dirs {
        if !dir.join("src").is_dir() {
            continue;
        }
        let name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("fixture")
            .to_string();
        let target = fixture_target(dir);
        // Compile failures are the TS tests' concern, not this one.
        let Ok(out) = compile_fixture(dir, target) else {
            continue;
        };
        match bynkc::strip_project_to_js(out) {
            Ok(js_out) => {
                checked += 1;
                let mut js_count = 0;
                for path in js_out.artefacts.docs.keys() {
                    let p = path.to_string_lossy();
                    if p.ends_with(".ts") {
                        failures.push(format!("{name}: a .ts file survived JS emission: {p}"));
                    }
                    if path.file_name().and_then(|n| n.to_str()) == Some("tsconfig.json") {
                        failures.push(format!("{name}: tsconfig.json was not dropped"));
                    }
                    if p.ends_with(".js") {
                        js_count += 1;
                    }
                }
                if js_count == 0 {
                    failures.push(format!("{name}: JS emission produced no .js modules"));
                }
            }
            Err(e) => failures.push(format!("{name}: strip failed: {e}")),
        }
    }

    assert!(checked > 0, "no project-form fixtures were stripped");
    assert!(
        failures.is_empty(),
        "JS artefact issues across {checked} fixture(s):\n{}",
        failures.join("\n"),
    );
}

/// Stage every project fixture's `--emit js` output into one tree and run
/// `node --check` over every `.js` — proof the stripped output is valid, runnable
/// ES-module JavaScript (a surviving type annotation would fail the check). Node
/// is the only requirement; skips loudly when it is absent, like the `tsc` gate.
#[test]
fn emitted_javascript_is_valid_under_node() {
    if !node_present() {
        eprintln!("\n!!! NODE JS-CHECK SKIPPED (emitted JavaScript) !!!\n`node` is not on PATH.\n");
        return;
    }

    let dirs = fixture_dirs("positive");
    assert!(!dirs.is_empty(), "no positive fixtures found");

    let root = std::env::temp_dir().join(format!("bynk-js-verify-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create js-verify temp root");
    // Make Node parse the emitted `.js` as ES modules regardless of Node version.
    fs::write(root.join("package.json"), "{\n  \"type\": \"module\"\n}\n")
        .expect("write root package.json");

    let mut staged = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for dir in &dirs {
        if !dir.join("src").is_dir() {
            continue;
        }
        let name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("fixture")
            .to_string();
        let target = fixture_target(dir);
        let Ok(out) = compile_fixture(dir, target) else {
            continue;
        };
        let js_out = match bynkc::strip_project_to_js(out) {
            Ok(o) => o,
            Err(e) => {
                failures.push(format!("{name}: strip failed: {e}"));
                continue;
            }
        };
        if let Err(e) = write_outputs(&js_out, &root.join(&name)) {
            failures.push(format!("{name}: failed to write JS output: {e}"));
            continue;
        }
        staged += 1;
    }
    assert!(
        failures.is_empty(),
        "staging JS output failed:\n{}",
        failures.join("\n")
    );
    assert!(
        staged > 0,
        "no project-form fixtures were staged for JS verification"
    );

    let harness = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("support")
        .join("js_check.mjs");
    let out = Command::new("node")
        .arg(&harness)
        .arg(&root)
        .output()
        .expect("run js_check.mjs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        let _ = fs::remove_dir_all(&root);
    } else {
        panic!(
            "emitted JavaScript is not valid under `node --check` — a TypeScript construct survived \
             stripping, or the JS is malformed.\nFAIL <file> <reason>:\n{}\n{}",
            stdout.trim(),
            stderr.trim(),
        );
    }
}
