//! #3068: a `tsc` failure on generated TypeScript is reported as
//! `bynk.codegen.tsc_failed`, at the Bynk source line the failing generated line
//! came from.
//!
//! No program the checker accepts should make `tsc` fail, so the fault is
//! injected: compile a small project, write its output (with the `.ts.map`s),
//! splice a type error onto one generated line, run the real `tsc`, and
//! feed its output to the same `diagnose` `bynk test` and `bynk check --emit` use.

#[path = "../require/mod.rs"]
mod require;

use std::path::{Path, PathBuf};
use std::process::Command;

const SOURCE: &str =
    "commons greeting\n\nfn greet(name: String) -> String {\n  let msg = \"hi {name}\"\n  msg\n}\n";

fn tsc_runner() -> Option<Command> {
    let ok = |name: &str| {
        Command::new(name)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if ok("tsc") {
        return Some(Command::new("tsc"));
    }
    None
}

fn scratch(tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("tsc-failure-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

/// Compile the project and write its output under `<dir>/out`. Returns the
/// build, for its snapshots.
fn build(dir: &Path) -> bynk_driver::project::ProjectOutput {
    std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"p\"\n").unwrap();
    std::fs::write(dir.join("src/greeting.bynk"), SOURCE).unwrap();
    let out = bynk_driver::project::compile_project(&bynk_testkit::compile_options_single(
        dir.join("src"),
    ))
    .map_err(bynk_driver::project::ProjectFailure::flatten)
    .expect("compiles");
    for (path, doc) in &out.artefacts.docs {
        bynk_driver::write_document(path, doc, &out.artefacts.docs, &dir.join("out")).unwrap();
    }
    bynk_testkit::write_runtime_modules(&dir.join("out")).unwrap();
    out
}

/// Run `tsc` over `<dir>/out`; its combined output (empty when it passes).
fn tsc(dir: &Path, mut cmd: Command) -> String {
    let o = cmd
        .current_dir(dir)
        .args(["-p", "out/tsconfig.json", "--noEmit", "--pretty", "false"])
        .output()
        .expect("tsc runs");
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn a_tsc_error_maps_back_to_the_bynk_line() {
    let Some(cmd) = tsc_runner() else {
        assert!(
            !require::is_required("BYNK_REQUIRE_TSC"),
            "BYNK_REQUIRE_TSC is set but `tsc` is not available"
        );
        eprintln!("skipping: `tsc` is not available");
        return;
    };
    let dir = scratch("mapped");
    let out = build(&dir);
    // The generated `let msg` line: prefix a type error onto it, so no line moves.
    let ts_path = dir.join("out/greeting.ts");
    let ts = std::fs::read_to_string(&ts_path).unwrap();
    let broken = ts.replace(
        "  const msg = ",
        "  const __bad: number = \"x\"; const msg = ",
    );
    assert_ne!(ts, broken, "the fault was injected");
    std::fs::write(&ts_path, broken).unwrap();

    let text = tsc(&dir, cmd);
    assert!(text.contains("TS2322"), "tsc rejected it: {text}");

    let failure = bynk_driver::tsc_report::diagnose(
        &text,
        &dir.join("out"),
        &out.snapshots,
        &out.display_root,
    )
    .expect("a TS error record is a codegen failure");
    assert_eq!(failure.errors.len(), 1, "{text}");
    let e = &failure.errors[0];
    assert_eq!(e.error.category, "bynk.codegen.tsc_failed");
    let src = e.source_path.as_deref().expect("mapped to a Bynk file");
    assert!(src.ends_with("greeting.bynk"), "{src:?}");
    // The span covers `let msg = "hi {name}"` — the Bynk line the generated
    // `const msg` statement came from.
    let text_of = &out
        .snapshots
        .iter()
        .find(|(p, _)| p.as_path() == src)
        .unwrap()
        .1;
    assert_eq!(
        &text_of[e.error.span.start..e.error.span.end],
        "let msg = \"hi {name}\""
    );
    let notes = e.error.notes.join("\n");
    assert!(notes.contains("bug in the Bynk compiler"), "{notes}");
    assert!(
        notes.contains("TS2322"),
        "the TS error is attached: {notes}"
    );
    assert!(
        notes.contains("generated: "),
        "the generated line is attached: {notes}"
    );

    // The short line a machine consumer parses points at the Bynk line.
    let short = bynk_driver::project_failure_short_lines(&failure);
    assert!(short[0].contains("greeting.bynk:4:"), "{short:?}");
    assert!(short[0].contains("bynk.codegen.tsc_failed"), "{short:?}");
}

#[test]
fn an_unmappable_error_is_still_reported() {
    let Some(cmd) = tsc_runner() else {
        assert!(
            !require::is_required("BYNK_REQUIRE_TSC"),
            "BYNK_REQUIRE_TSC is set but `tsc` is not available"
        );
        return;
    };
    let dir = scratch("unmapped");
    let out = build(&dir);
    // Drop the map: the generated file is intact, but nothing maps it back.
    std::fs::remove_file(dir.join("out/greeting.ts.map")).unwrap();
    let ts_path = dir.join("out/greeting.ts");
    let ts = std::fs::read_to_string(&ts_path).unwrap();
    std::fs::write(
        &ts_path,
        ts.replace(
            "  const msg = ",
            "  const __bad: number = \"x\"; const msg = ",
        ),
    )
    .unwrap();

    let text = tsc(&dir, cmd);
    let failure = bynk_driver::tsc_report::diagnose(
        &text,
        &dir.join("out"),
        &out.snapshots,
        &out.display_root,
    )
    .expect("still a codegen failure");
    let e = &failure.errors[0];
    assert_eq!(e.error.category, "bynk.codegen.tsc_failed");
    assert!(e.source_path.is_none());
    assert!(e.error.message.contains("no Bynk source location"));
    assert!(e.error.notes.join("\n").contains("TS2322"));
}

#[test]
fn clean_output_passes_and_a_crash_is_not_a_compiler_bug() {
    let Some(cmd) = tsc_runner() else {
        return;
    };
    let dir = scratch("clean");
    let out = build(&dir);
    let text = tsc(&dir, cmd);
    assert!(
        text.trim().is_empty(),
        "generated output type-checks: {text}"
    );
    assert!(
        bynk_driver::tsc_report::diagnose(
            "FATAL ERROR: heap out of memory",
            &dir.join("out"),
            &out.snapshots,
            &out.display_root
        )
        .is_none()
    );
}

/// Run `bynk <args>` in `cwd` with `PATH` as given (or inherited).
fn bynk(cwd: &Path, args: &[&str], path: Option<&str>) -> (bool, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"));
    cmd.args(args).current_dir(cwd).env("NO_COLOR", "1");
    if let Some(p) = path {
        cmd.env("PATH", p);
    }
    let o = cmd.output().expect("bynk runs");
    (
        o.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
    )
}

fn project(tag: &str) -> PathBuf {
    let dir = scratch(tag);
    std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"p\"\n").unwrap();
    std::fs::write(dir.join("src/greeting.bynk"), SOURCE).unwrap();
    dir
}

#[test]
fn check_emit_passes_a_clean_project_and_writes_nothing() {
    if tsc_runner().is_none() {
        assert!(
            !require::is_required("BYNK_REQUIRE_TSC"),
            "BYNK_REQUIRE_TSC is set but `tsc` is not available"
        );
        return;
    }
    let dir = project("emit-clean");
    let (ok, text) = bynk(&dir, &["check", "--emit", "."], None);
    assert!(ok, "{text}");
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["bynk.toml", "src"],
        "check --emit writes nothing into the project"
    );
}

#[test]
fn check_emit_refuses_a_single_file() {
    let dir = project("emit-file");
    let (ok, text) = bynk(&dir, &["check", "--emit", "src/greeting.bynk"], None);
    assert!(!ok);
    assert!(text.contains("takes a project directory"), "{text}");
}

#[test]
fn check_emit_without_tsc_says_so_and_fails() {
    let dir = project("emit-no-tsc");
    // An empty directory as the whole PATH: no `tsc`, no `npx`.
    let empty = dir.join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let (ok, text) = bynk(&dir, &["check", "--emit", "."], empty.to_str());
    assert!(
        !ok,
        "a check that could not type-check must not pass: {text}"
    );
    assert!(text.contains("needs `tsc`"), "{text}");
    assert!(text.contains("npm install -g typescript@"), "{text}");
}
