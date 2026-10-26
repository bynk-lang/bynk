//! The `examples/` projects are showcase material — this gate keeps them
//! compiling as the language moves. They are also the source the "Bynk by
//! Example" gallery extracts (documentation track, slice 4), so every project
//! must keep building or the gallery would ship code that no longer compiles.
//! Each example must build the deployable Worker; `hello-world` is checked more
//! deeply (both targets + the Worker output shape) as the representative.

use std::path::{Path, PathBuf};

/// Every project under `examples/`. Kept in the `examples/README.md` reading
/// order. Adding an example here gates it and lets the gallery extract it.
const EXAMPLES: &[&str] = &[
    "hello-world",
    "greeter-i18n",
    "link-shortener",
    "feature-flags",
    "todo",
    "orders",
    "sessions",
    "event-log",
    "rate-limiter",
    "uptime-monitor",
    "webhook-relay",
    "semver",
    "markdown",
];

fn example_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples")
        .join(name)
}

#[test]
fn every_example_builds_for_workers() {
    for name in EXAMPLES {
        let root = example_root(name);
        let paths = bynkc::try_read_project_paths(&root).expect("well-formed fixture manifest");
        // Content-ownership track (#1086) slice 3: bynk-testkit's complete
        // sources map, proved here as one of the representative call sites,
        // instead of relying on bynk-emit's disk fallback.
        bynkc::compile_project(
            &bynk_testkit::compile_options_split(root.clone(), paths)
                .target(bynkc::BuildTarget::Workers),
        )
        .unwrap_or_else(|failure| {
            panic!(
                "examples/{name} failed on Workers: {:?}",
                failure
                    .errors
                    .iter()
                    .map(|e| (&e.source_path, e.error.category, &e.error.message))
                    .collect::<Vec<_>>()
            )
        });
    }
}

#[test]
fn hello_world_builds_on_both_targets() {
    let root = example_root("hello-world");
    let paths = bynkc::try_read_project_paths(&root).expect("well-formed fixture manifest");
    for target in [bynkc::BuildTarget::Bundle, bynkc::BuildTarget::Workers] {
        let out = bynkc::compile_project(
            &bynk_testkit::compile_options_split(root.clone(), paths.clone()).target(target),
        )
        .unwrap_or_else(|failure| {
            panic!(
                "examples/hello-world failed on {target:?}: {:?}",
                failure
                    .errors
                    .iter()
                    .map(|e| (&e.source_path, e.error.category, &e.error.message))
                    .collect::<Vec<_>>()
            )
        });
        assert!(!out.artefacts.docs.is_empty());
    }
    // The workers build must produce the deployable Worker directory.
    let workers = bynkc::compile_project(
        &bynk_testkit::compile_options_split(root, paths).target(bynkc::BuildTarget::Workers),
    )
    .unwrap_or_else(|_| panic!("workers build failed"));
    for needed in [
        "workers/hello-web/index.ts",
        "workers/hello-web/wrangler.toml",
        "__bynk_runtime/result.ts",
    ] {
        assert!(
            workers.artefacts.docs.contains_key(Path::new(needed)),
            "workers output must include {needed}"
        );
    }
}

/// #518: the examples' emitted TypeScript is type-checked, not just emitted.
/// Every example is staged into its own subdir under one temp root and the
/// whole tree is checked in a single `tsc --strict --noEmit` run (the same
/// one-invocation economics as `tsc_verify`). Skips loudly without a tsc
/// runner; `BYNK_REQUIRE_TSC=1` turns the skip into a failure (CI).
#[test]
fn every_example_output_passes_tsc_strict() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! EXAMPLES TYPESCRIPT VERIFICATION SKIPPED !!!\nneither `tsc` nor `npx` is on PATH.\n"
            );
            if std::env::var("BYNK_REQUIRE_TSC").is_ok() {
                panic!("BYNK_REQUIRE_TSC is set but no tsc runner was found");
            }
            return;
        }
    };

    // #527 emptied this list: the gate now covers all ten examples.
    const KNOWN_TSC_RED: &[&str] = &[];

    let root = std::env::temp_dir().join(format!("bynk-examples-tsc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    for name in EXAMPLES.iter().filter(|n| !KNOWN_TSC_RED.contains(n)) {
        // #1655: the example's *project form* (`bynk.toml`, `src/` and
        // `tests/`), exactly what `bynkc compile <example> --target workers`
        // produces. This gate compiled `src/` alone, so it missed a workers
        // build writing test modules that imported the bundle layout; a workers
        // build now emits none, and this pins that the whole output is
        // tsc-clean.
        let root_dir = example_root(name);
        let paths = bynkc::try_read_project_paths(&root_dir).expect("well-formed example manifest");
        let out = bynkc::compile_project(
            &bynk_testkit::compile_options_split(root_dir, paths)
                .target(bynkc::BuildTarget::Workers),
        )
        .unwrap_or_else(|_| panic!("examples/{name} failed to compile"));
        bynkc::write_output(&out, &root.join(name)).unwrap();
    }
    // One root tsconfig sweeps every staged example (`include: ["**/*.ts"]`);
    // all emitted imports are relative, so nothing resolves across subdirs.
    std::fs::write(
        root.join("tsconfig.json"),
        bynk_emit::emitter::emit_tsconfig(),
    )
    .unwrap();

    let (program, prefix) = &runner;
    let mut cmd = base_command(program);
    for p in prefix {
        cmd.arg(p);
    }
    cmd.args(["--strict", "--noEmit", "-p"])
        .arg(root.join("tsconfig.json"));
    let output = cmd.output().expect("launch tsc");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "examples emitted TypeScript failed tsc --strict:\n{combined}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The `discover_tsc` ladder shared with `tsc_verify`: a global `tsc`, else
/// `npx`-provisioned TypeScript (pinned major, `-p` package form — the bare
/// `npx typescript@N tsc` form fails with "could not determine executable").
fn discover_tsc() -> Option<(String, Vec<String>)> {
    let exists = |name: &str| which::which(name).is_ok();
    if exists("tsc") {
        return Some(("tsc".to_string(), vec![]));
    }
    if exists("npx") {
        return Some((
            "npx".to_string(),
            vec![
                "--yes".to_string(),
                "-p".to_string(),
                format!("typescript@{}", bynk_emit::TYPESCRIPT_MAJOR_TESTED),
                "tsc".to_string(),
            ],
        ));
    }
    None
}

/// Build a `Command` for `program`, routing through `cmd /C` on Windows so
/// npm's `.cmd` shims (`tsc.cmd`, `npx.cmd`) resolve — Rust's CreateProcess
/// refuses to run batch scripts directly (the BatBadBut hardening), so a
/// bare `Command::new("tsc")` fails there with "program not found" even
/// though `which` sees the shim. Same helper as `tsc_verify`.
fn base_command(program: &str) -> std::process::Command {
    if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        std::process::Command::new(program)
    }
}
