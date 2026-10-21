//! End-to-end fixture-driven tests.
//!
//! Each subdirectory under `tests/fixtures/positive/` is one fixture. There
//! are two supported shapes:
//!
//! - **Single-file**: `input.bynk` + `expected.ts`. The compiler runs in
//!   single-file mode and the output is compared against `expected.ts`.
//! - **Project**: a `src/` directory and an `expected/` directory mirroring
//!   the same source tree, with `.bynk` files rewritten to `.ts`. The
//!   compiler runs in project mode (`compile_project`) and every generated
//!   file is compared against its counterpart under `expected/`.
//!
//! Either shape may additionally carry an optional `expected_contains.txt`
//! and/or `expected_absent.txt` (#58) — each a newline-separated list of
//! substrings (`#`-prefixed lines and blanks ignored) that must, respectively,
//! appear or not appear somewhere in the emitted output. These compose with
//! the whole-file snapshot above rather than replacing it — a granularity
//! between "byte-identical" and "nothing at all" for a fixture that only
//! cares about one emitted fragment. See [`check_expected_contains_absent`].
//!
//! Each subdirectory under `tests/fixtures/negative/` contains either an
//! `input.bynk` (single-file) or a `src/` (project) input, plus at least one
//! of: an `expected_error.txt` listing `category message` substrings the
//! diagnostics must contain, or (project-mode only, #58) an
//! `expected_diagnostics.txt` listing exact `category<TAB>path:line:col`
//! lines — see [`check_expected_diagnostics`].

use std::fs;
use std::path::{Path, PathBuf};

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

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
}

/// Project-wide boilerplate, emitted identically for every project and unit
/// tested separately: `runtime.ts`, `tsconfig.json`, and a `package.json` that
/// is only the `"type": "module"` marker (#2148). A `package.json` carrying an
/// adapter's npm dependencies is per-fixture and stays snapshotted.
fn is_project_boilerplate(path: &Path, doc: &bynkc::Document) -> bool {
    match path.to_str() {
        Some("runtime.ts" | "tsconfig.json") => true,
        Some("package.json") => doc.text() == "{\n  \"type\": \"module\"\n}\n",
        _ => false,
    }
}

fn collect_expected_ts(expected_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![expected_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(rd) = fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    // Every kind the emitter writes: `.ts`, `wrangler.toml`,
                    // and JSON (`package.json` for an adapter fixture,
                    // `bynk-secrets.json` for a context declaring an auth
                    // secret). Bless writes *all* emitted files, so anything
                    // this walk skips is invisible to the expected-side
                    // comparison and then flagged as a surplus output — which
                    // is why the JSON case is a bare extension check rather
                    // than the one-filename special case it started as.
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if matches!(ext, "ts" | "toml" | "json") {
                        out.push(p);
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// #58: `FileCheck`-style substring assertions (rustc's `tests/codegen`
/// precedent) — a granularity between whole-file byte identity and a bare
/// diagnostic category string. `expected_contains.txt` lines must each appear
/// somewhere in `haystack`; `expected_absent.txt` lines must each not. Either
/// file is optional and independent of the other, and both compose with a
/// fixture's ordinary `expected.ts`/`expected/` snapshot rather than replacing
/// it — a fixture that only cares about one emitted fragment doesn't have to
/// stop pinning the rest of the file to also assert that fragment precisely.
fn check_expected_contains_absent(dir: &Path, haystack: &str, failures: &mut Vec<String>) {
    let contains = dir.join("expected_contains.txt");
    if contains.exists() {
        for needle in read(&contains)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            if !haystack.contains(needle) {
                failures.push(format!(
                    "\n=== {} ===\nexpected_contains.txt: expected output to contain `{}`, but it did not",
                    dir.display(),
                    needle,
                ));
            }
        }
    }
    let absent = dir.join("expected_absent.txt");
    if absent.exists() {
        for needle in read(&absent)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            if haystack.contains(needle) {
                failures.push(format!(
                    "\n=== {} ===\nexpected_absent.txt: expected output to NOT contain `{}`, but it did",
                    dir.display(),
                    needle,
                ));
            }
        }
    }
}

/// Whether `doc` is one of the compiler's own generated outputs, as opposed
/// to a `.map`/`.bynkdbg.json` sidecar `bynk-emit::project::build_output`
/// splits into its own `Artefacts.docs` entry (P7.6, #1309) — the old
/// `ProjectOutput.files` never carried those as separate entries (they were
/// `CompiledFile::source_map`/`debug_metadata` fields), so every fixture
/// comparison here filters them out to keep comparing the same set.
fn is_main_document(doc: &bynkc::Document) -> bool {
    !matches!(
        doc,
        bynkc::Document::SourceMap(_) | bynkc::Document::DebugSidecar(_)
    )
}

/// Read the build target marker from a fixture root, if present.
/// Defaults to bundle when no `target.txt` is present.
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
/// Defaults to cloudflare when no `platform.txt` is present.
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

/// v0.9.1: compile a project-form fixture. If the fixture root has a
/// `bynk.toml`, use split-paths mode rooted at the fixture root. Otherwise,
/// fall back to the legacy single-tree mode rooted at `src/`.
fn compile_fixture(
    fixture_root: &Path,
    target: bynkc::BuildTarget,
) -> Result<bynkc::ProjectOutput, bynkc::ProjectFailure> {
    let bynk_toml = fixture_root.join("bynk.toml");
    if bynk_toml.exists() {
        // Split-paths mode compiles at the default platform — a bynk.toml
        // fixture with a platform.txt would silently ignore it.
        assert!(
            !fixture_root.join("platform.txt").exists(),
            "{}: platform.txt is not supported in bynk.toml fixtures",
            fixture_root.display()
        );
        let paths =
            bynkc::try_read_project_paths(fixture_root).expect("well-formed fixture manifest");
        // #1890: a fixture's `[workers] compatibility_flags` reaches its
        // `wrangler.toml`, as `bynk-driver`'s option builders arrange.
        let workers =
            bynkc::try_read_workers_config(fixture_root).expect("well-formed fixture manifest");
        bynkc::compile_project(
            &bynk_testkit::compile_options_split(fixture_root.to_path_buf(), paths)
                .target(target)
                .compatibility_flags(workers.compatibility_flags),
        )
    } else {
        let src_dir = fixture_root.join("src");
        let platform = fixture_platform(fixture_root);
        bynkc::compile_project(
            &bynk_testkit::compile_options_single(src_dir)
                .target(target)
                .platform(platform),
        )
    }
}

/// #1684: positive goldens compare byte for byte, trailing newlines included,
/// so the emitter's output and a blessed golden are the same bytes. A mismatch
/// only in trailing whitespace prints as two identical-looking blocks, so the
/// failure names it.
fn whitespace_note(actual: &str, want: &str) -> &'static str {
    if actual.trim_end() == want.trim_end() {
        " (differs only in trailing whitespace; re-bless)"
    } else {
        ""
    }
}

/// `whitespace_note` names a trailing-only difference and nothing else, so a
/// later edit can't silently drop the hint (or attach it to a real difference).
#[test]
fn whitespace_note_names_only_trailing_differences() {
    let want = "a\nb\n";
    assert!(
        !whitespace_note("a\nb\n\n", want).is_empty(),
        "extra trailing newline"
    );
    assert!(
        !whitespace_note("a\nb", want).is_empty(),
        "missing final newline"
    );
    assert_eq!(
        whitespace_note("a\nc\n", want),
        "",
        "substantive difference"
    );
    assert_eq!(
        whitespace_note("a \nb\n", want),
        "",
        "interior trailing space"
    );
}

#[test]
fn positive_fixtures() {
    let dirs = fixture_dirs("positive");
    assert!(!dirs.is_empty(), "no positive fixtures found");
    let mut failures = Vec::new();
    for dir in dirs {
        let input = dir.join("input.bynk");
        let src_dir = dir.join("src");
        if input.exists() {
            let expected = dir.join("expected.ts");
            let source = read(&input);
            let name = input.display().to_string();
            match bynkc::compile(&source, &name) {
                Ok(actual) => {
                    let want = read(&expected);
                    if actual != want {
                        failures.push(format!(
                            "\n=== {}{} ===\n--- expected ---\n{}\n--- actual ---\n{}\n",
                            dir.display(),
                            whitespace_note(&actual, &want),
                            want,
                            actual,
                        ));
                    }
                    check_expected_contains_absent(&dir, &actual, &mut failures);
                }
                Err(errors) => {
                    let rendered = bynkc::render_errors(&errors, &source, &name);
                    failures.push(format!(
                        "\n=== {} ===\nexpected compile success but got errors:\n{}",
                        dir.display(),
                        rendered,
                    ));
                }
            }
        } else if src_dir.is_dir() {
            let expected_dir = dir.join("expected");
            let target = fixture_target(&dir);
            match compile_fixture(&dir, target) {
                Ok(out) => {
                    // Build expected set by walking expected_dir.
                    let expected_files = collect_expected_ts(&expected_dir);
                    let mut actual_by_path: std::collections::HashMap<PathBuf, String> =
                        std::collections::HashMap::new();
                    // Skip project-wide boilerplate (runtime.ts, tsconfig.json):
                    // emitted identically for every project, separately unit
                    // tested. Excluding them keeps per-fixture snapshots focused
                    // on the per-context emission.
                    for (path, doc) in &out.artefacts.docs {
                        if !is_main_document(doc) {
                            continue;
                        }
                        if is_project_boilerplate(path, doc) {
                            continue;
                        }
                        actual_by_path.insert(path.clone(), doc.text());
                    }
                    // For each expected .ts file, compare.
                    let mut all_ok = true;
                    let mut report = String::new();
                    for ef in &expected_files {
                        let rel = ef.strip_prefix(&expected_dir).unwrap().to_path_buf();
                        let want = read(ef);
                        let actual = actual_by_path.get(&rel);
                        match actual {
                            Some(a) => {
                                if *a != want {
                                    all_ok = false;
                                    report.push_str(&format!(
                                        "\n--- {}{} ---\n--- expected ---\n{}\n--- actual ---\n{}\n",
                                        rel.display(),
                                        whitespace_note(a, &want),
                                        want,
                                        a,
                                    ));
                                }
                            }
                            None => {
                                all_ok = false;
                                report.push_str(&format!(
                                    "\n--- missing output: {} ---\n",
                                    rel.display()
                                ));
                            }
                        }
                    }
                    // Check there are no surplus outputs we didn't expect.
                    let mut expected_rels: std::collections::HashSet<PathBuf> = expected_files
                        .iter()
                        .map(|p| p.strip_prefix(&expected_dir).unwrap().to_path_buf())
                        .collect();
                    for (path, doc) in &out.artefacts.docs {
                        if !is_main_document(doc) {
                            continue;
                        }
                        if is_project_boilerplate(path, doc) {
                            continue;
                        }
                        if !expected_rels.remove(path) {
                            all_ok = false;
                            report.push_str(&format!(
                                "\n--- unexpected output: {} ---\n--- actual ---\n{}\n",
                                path.display(),
                                doc.text(),
                            ));
                        }
                    }
                    // ADR 0117: assert the build's non-failing warnings against
                    // `expected_warnings.txt` (substrings, like
                    // `expected_error.txt`). Absent file ⇒ the build must emit
                    // no warnings — so a positive fixture cannot start warning
                    // silently.
                    let rendered_warnings: String = out
                        .warnings
                        .iter()
                        .map(|w| format!("{} {}\n", w.error.category, w.error.message))
                        .collect();
                    let want_warn = dir.join("expected_warnings.txt");
                    if want_warn.exists() {
                        for line in read(&want_warn)
                            .lines()
                            .map(str::trim)
                            .filter(|l| !l.is_empty() && !l.starts_with('#'))
                        {
                            if !rendered_warnings.contains(line) {
                                all_ok = false;
                                report.push_str(&format!(
                                    "\n--- missing warning `{line}` ---\n--- got ---\n{rendered_warnings}\n"
                                ));
                            }
                        }
                    } else if !out.warnings.is_empty() {
                        all_ok = false;
                        report.push_str(&format!(
                            "\n--- unexpected warning(s) ---\n{rendered_warnings}\n"
                        ));
                    }
                    if !all_ok {
                        failures.push(format!("\n=== {} ==={}", dir.display(), report));
                    }
                    // #58: check across every emitted file — fixture-level, not
                    // per-file, since a substring assertion doesn't need to name
                    // which file it expects the fragment in.
                    let haystack: String = out
                        .artefacts
                        .docs
                        .values()
                        .filter(|doc| is_main_document(doc))
                        .map(|doc| doc.text())
                        .collect::<Vec<_>>()
                        .join("\n");
                    check_expected_contains_absent(&dir, &haystack, &mut failures);
                }
                Err(failure) => {
                    let rendered = bynkc::render_project_errors(&failure.flatten());
                    failures.push(format!(
                        "\n=== {} ===\nexpected compile success but got errors:\n{}",
                        dir.display(),
                        rendered,
                    ));
                }
            }
        } else {
            failures.push(format!(
                "\n=== {} ===\nfixture has neither `input.bynk` nor `src/`",
                dir.display()
            ));
        }
    }
    if !failures.is_empty() {
        panic!("positive fixtures failed:\n{}", failures.join("\n"));
    }
}

/// Regenerate the `expected/` snapshot of every project-form positive fixture
/// from the current emitter. Gated behind `BYNK_BLESS=1` so it never runs in a
/// normal `cargo test`. Use after an intentional emission change, then review
/// the resulting `git diff` to confirm the changes are what you meant.
#[test]
fn bless_positive_fixtures() {
    if std::env::var("BYNK_BLESS").is_err() {
        return;
    }
    for dir in fixture_dirs("positive") {
        let src_dir = dir.join("src");
        if !src_dir.is_dir() {
            // #527: single-file fixtures (input.bynk → expected.ts) bless too;
            // previously an emission change left them to fail by hand.
            let input = dir.join("input.bynk");
            if input.exists() {
                let source = read(&input);
                let ts =
                    bynkc::compile(&source, &input.display().to_string()).unwrap_or_else(|e| {
                        panic!("bless: {} failed to compile: {e:?}", dir.display())
                    });
                fs::write(dir.join("expected.ts"), ts).unwrap();
            }
            continue;
        }
        let target = fixture_target(&dir);
        let out = match compile_fixture(&dir, target) {
            Ok(out) => out,
            Err(failure) => {
                panic!(
                    "bless: {} failed to compile:\n{}",
                    dir.display(),
                    bynkc::render_project_errors(&failure.flatten())
                );
            }
        };
        let expected_dir = dir.join("expected");
        let _ = fs::remove_dir_all(&expected_dir);
        for (path, doc) in &out.artefacts.docs {
            if !is_main_document(doc) {
                continue;
            }
            // Mirror the comparison's skip list: project-wide boilerplate is
            // unit-tested separately and excluded from per-fixture snapshots.
            if is_project_boilerplate(path, doc) {
                continue;
            }
            let target_path = expected_dir.join(path);
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&target_path, doc.text()).unwrap();
        }
    }
}

/// The `/* unknown */` placeholder must never reach emitted output. It is the
/// signature of an unresolved lowering path (historically the agent
/// method-call regression). This guard compiles every positive fixture and
/// fails if any emitted file contains the marker — the regression backstop the
/// v0.7 fix lacked.
#[test]
fn no_unknown_placeholder_in_emitted_output() {
    // Every emitter placeholder is the signature of an unresolved lowering path
    // that would otherwise ship invalid TypeScript. `/* unknown */` is the
    // instance-method-call fallback (the historical v0.7 regression). Markers
    // use distinct text, so the guard checks for each one explicitly — add new
    // markers here as they are introduced. (`complex is-receiver`, the `is`
    // receiver fallback, is now an internal-error panic instead (#1668).)
    const MARKERS: &[&str] = &["/* unknown */"];
    let hit = |s: &str| MARKERS.iter().any(|m| s.contains(m));
    let mut offenders = Vec::new();
    for dir in fixture_dirs("positive") {
        let input = dir.join("input.bynk");
        let src_dir = dir.join("src");
        if input.exists() {
            let source = read(&input);
            if let Ok(actual) = bynkc::compile(&source, &input.display().to_string())
                && hit(&actual)
            {
                offenders.push(dir.display().to_string());
            }
        } else if src_dir.is_dir() {
            let target = fixture_target(&dir);
            if let Ok(out) = compile_fixture(&dir, target) {
                for (path, doc) in &out.artefacts.docs {
                    if is_main_document(doc) && hit(&doc.text()) {
                        offenders.push(format!("{} :: {}", dir.display(), path.display()));
                    }
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "emitted output contains an emitter placeholder ({MARKERS:?}) in:\n{}",
        offenders.join("\n"),
    );
}

/// #2054: on every internal path a Worker entry answers, the signature check
/// (`__verifyCall`, I2 #1971) runs before anything else that can answer, so an
/// unauthenticated request gets a bare `401` and never a `404` that names a
/// service, a `409` that echoes the contract hash, or a `400` from the body.
///
/// Checked over every blessed `workers/<unit>/index.ts`: each
/// `path.startsWith("/_bynk/…")` block must open with `__verifyCall` (the
/// `/_bynk/call/` block may put only its `405` method gate first, which reads
/// nothing but the method), immediately followed by the gate that returns its
/// refusal. Every `"/_bynk/…"` literal in the entry must be one of those
/// blocks' opening test or their `servicePath` slice, for `/_bynk/call/` or
/// `/_bynk/event/`, so an internal path in any other form (another prefix,
/// `path === "/_bynk/…"`, a `__matchPath` pattern) fails here until it is
/// gated and added. `/_bynk/agent/` and `/_bynk/fanout` are Durable Object
/// paths, reached only through a stub, and never appear in a Worker entry.
#[test]
fn every_internal_path_verifies_the_caller_before_any_other_answer() {
    const PREFIXES: &[&str] = &["/_bynk/call/", "/_bynk/event/"];
    let mut blocks = [0usize; 2];
    let mut offenders = Vec::new();
    for dir in fixture_dirs("positive") {
        let expected = dir.join("expected");
        if !expected.is_dir() {
            continue;
        }
        for path in collect_expected_ts(&expected) {
            let is_entry = path.file_name().is_some_and(|n| n == "index.ts")
                && path
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .is_some_and(|n| n == "workers");
            if !is_entry {
                continue;
            }
            let text = read(&path);
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if !line.contains("\"/_bynk/") {
                    continue;
                }
                let trimmed = line.trim();
                // Any `"/_bynk/…"` literal, in any form (`===`, `__matchPath`, a
                // new prefix), must be one of the known shapes: a gated block's
                // opening test, or the `servicePath` slice inside one.
                if PREFIXES
                    .iter()
                    .any(|p| trimmed == format!("const servicePath = path.slice(\"{p}\".length);"))
                {
                    continue;
                }
                let Some(k) = PREFIXES
                    .iter()
                    .position(|p| trimmed == format!("if (path.startsWith(\"{p}\")) {{"))
                else {
                    offenders.push(format!(
                        "{}:{}: an internal path this guard does not know: {trimmed}",
                        path.display(),
                        i + 1
                    ));
                    continue;
                };
                let prefix = PREFIXES[k];
                blocks[k] += 1;
                let mut body = lines[i + 1..].iter().map(|l| l.trim());
                let mut first = body.next().unwrap_or("");
                if k == 0 && first.starts_with("if (method !== \"POST\") return ") {
                    first = body.next().unwrap_or("");
                }
                let gate = body.next().unwrap_or("");
                if !first.starts_with("const __body = await __verifyCall(") {
                    offenders.push(format!(
                        "{}:{}: `{prefix}` answers before `__verifyCall`: {first}",
                        path.display(),
                        i + 1
                    ));
                } else if gate != "if (typeof __body !== \"string\") return __body;" {
                    // A dropped or inverted gate would run on unverified.
                    offenders.push(format!(
                        "{}:{}: `{prefix}` does not return `__verifyCall`'s refusal: {gate}",
                        path.display(),
                        i + 1
                    ));
                }
            }
        }
    }
    assert!(
        blocks.iter().all(|&n| n > 0),
        "expected `/_bynk/call/` and `/_bynk/event/` blocks to check, found {blocks:?}"
    );
    assert!(
        offenders.is_empty(),
        "an internal path does not verify the caller first:\n{}",
        offenders.join("\n"),
    );
}

/// #2053: every fault handler in an emitted Worker entry logs the error's
/// stack, not the error as a value, so `wrangler tail` shows the frames, not
/// only the message. #2192: those frames stay in the bundle's coordinates,
/// since Cloudflare maps an uploaded source map only onto an exception it
/// records, so each handler also reports the fault (`__bynkReportFault`),
/// whose recorded stack is mapped. Checked over every blessed
/// `workers/<unit>/index.ts`: each `catch (e)` (the `fetch` wrapper that
/// covers routes, `/_bynk/call/` and `/_bynk/event/`, and the queue
/// consumer's per-message catch) must log the stack expression and report the
/// fault. A new catch site that logs `e` bare, or doesn't report, fails here.
/// #2193: the `fetch` catch (the one whose log names the faulted route) also
/// answers through `__faultResponse`, which marks an `on call` fault so the
/// caller can name the callee.
#[test]
fn every_worker_entry_fault_handler_logs_the_stack() {
    const STACK: &str = "e instanceof globalThis.Error ? (e.stack || globalThis.String(e)) : e";
    const REPORT: &str = "__bynkReportFault(ctx, e);";
    let mut sites = 0usize;
    let mut offenders = Vec::new();
    for dir in fixture_dirs("positive") {
        let expected = dir.join("expected");
        if !expected.is_dir() {
            continue;
        }
        for path in collect_expected_ts(&expected) {
            let is_entry = path.file_name().is_some_and(|n| n == "index.ts")
                && path
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .is_some_and(|n| n == "workers");
            if !is_entry {
                continue;
            }
            let text = read(&path);
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if !line.contains("catch (e) {") {
                    continue;
                }
                sites += 1;
                // The log and the report sit on the catch's own line (the
                // queue consumer's compact form) or on the two lines after it
                // (`fetch`).
                let body = lines[i..lines.len().min(i + 3)].join("\n");
                if !(body.contains("globalThis.console.error(")
                    && body.contains(STACK)
                    && body.contains(REPORT))
                {
                    offenders.push(format!("{}:{}", path.display(), i + 1));
                }
                let fetch_catch = body.contains(" faulted`");
                let answer = lines[i..lines.len().min(i + 4)].join("\n");
                if fetch_catch && !answer.contains("return __faultResponse(") {
                    offenders.push(format!("{}:{} (no __faultResponse)", path.display(), i + 1));
                }
            }
        }
    }
    assert!(sites > 0, "no Worker entry fault handler found to check");
    assert!(
        offenders.is_empty(),
        "a Worker entry fault handler does not log the error's stack ({STACK}) and report it ({REPORT}):\n{}",
        offenders.join("\n"),
    );
}

/// #58 (adjacent to #696): checks a project fixture's `expected_diagnostics.txt`
/// — optional, `category<TAB>path:line:col` lines, plus an optional `# exact`
/// line (#1663) — against the *attributed*
/// errors a project-mode compile produced. Unlike `expected_error.txt`'s
/// substring-over-`category message` check, this pins the file and position
/// too: `project.rs:5013-5017`'s and `bynk-driver/tests/project_diagnostics.rs`'s
/// (#696) escapes were both an identity collision the category-only check
/// could not have caught, because neither carried a path to disagree on.
fn check_expected_diagnostics(
    dir: &Path,
    errors: &[bynkc::AttributedError],
    snapshots: &[(PathBuf, String)],
    failures: &mut Vec<String>,
) {
    let expected = dir.join("expected_diagnostics.txt");
    if !expected.exists() {
        return;
    }
    let want = read(&expected);
    let actual: Vec<String> = errors
        .iter()
        .map(|e| {
            let path = e
                .source_path
                .as_deref()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| "<unattributed>".to_string());
            let source = e
                .source_path
                .as_deref()
                .and_then(|p| snapshots.iter().find(|(sp, _)| sp == p))
                .map(|(_, s)| s.as_str())
                .unwrap_or("");
            let (line, col) = bynk_syntax::span::line_col(source, e.error.span.start);
            format!("{}\t{path}:{line}:{col}", e.error.category)
        })
        .collect();
    // #1663: `# exact` makes this an exact-set check too (see
    // `match_needles`), which is what pins a *mislocated* diagnostic: the
    // same fault reported again at another file's position.
    let exact = want.lines().any(|l| l.trim() == "# exact");
    let (missing, extra) = match_needles(&needles_of(&want), &actual, |n, a| a == n, exact);
    for needle in missing {
        failures.push(format!(
            "\n=== {} ===\nexpected_diagnostics.txt: expected `{needle}`, but got:\n{}",
            dir.display(),
            actual.join("\n"),
        ));
    }
    if !extra.is_empty() {
        failures.push(format!(
            "\n=== {} ===\nexpected_diagnostics.txt `# exact`: {} diagnostic(s) matched no line:\n{}",
            dir.display(),
            extra.len(),
            extra.join("\n"),
        ));
    }
}

/// Check a negative fixture's `expected_error.txt` needles against its
/// diagnostics, one rendered line each.
///
/// By default this is a **subset** check: every needle must be a substring of
/// some line, and extra diagnostics pass. A `# exact` line (#1663) makes it an
/// **exact-set** check: the needles and the diagnostics must pair up one to
/// one, each needle matching a distinct line, with nothing left over. That is
/// what pins "no spurious diagnostics" — a cascade the subset check cannot
/// see. `# exact` is a comment line, so a parse-time fixture's first line can
/// still be the bare code its tree-sitter conformance reads.
fn check_expected_errors(dir: &Path, want: &str, lines: &[String], failures: &mut Vec<String>) {
    let exact = want.lines().any(|l| l.trim() == "# exact");
    let needles = needles_of(want);
    let got = lines.join("\n");
    let (missing, extra) = match_needles(&needles, lines, |n, l| l.contains(n), exact);
    for needle in missing {
        failures.push(format!(
            "\n=== {} ===\nexpected error containing `{needle}`, but got:\n{got}",
            dir.display(),
        ));
    }
    if !extra.is_empty() {
        failures.push(format!(
            "\n=== {} ===\n`# exact`: {} diagnostic(s) matched no needle:\n{}",
            dir.display(),
            extra.len(),
            extra.join("\n"),
        ));
    }
}

/// The non-blank, non-comment lines of an expectation file, trimmed.
fn needles_of(want: &str) -> Vec<&str> {
    want.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// #1663: match expectation needles to diagnostic lines. Returns the needles
/// with no line, and (when `exact`) the lines with no needle.
///
/// A subset check only asks that each needle `fits` some line. An exact check
/// pairs needles and lines one to one: a maximum bipartite matching (augmenting
/// paths), so the outcome never depends on the order the needles are listed in
/// — a needle that fits two lines can never take the one a later, narrower
/// needle needed, and a failure always means a real mismatch.
fn match_needles<'a>(
    needles: &[&'a str],
    lines: &'a [String],
    fits: impl Fn(&str, &str) -> bool,
    exact: bool,
) -> (Vec<&'a str>, Vec<&'a str>) {
    if !exact {
        let missing = needles
            .iter()
            .copied()
            .filter(|n| !lines.iter().any(|l| fits(n, l)))
            .collect();
        return (missing, Vec::new());
    }
    // `owner[l]`: the needle line `l` is matched to.
    let mut owner: Vec<Option<usize>> = vec![None; lines.len()];
    fn augment(
        n: usize,
        needles: &[&str],
        lines: &[String],
        fits: &dyn Fn(&str, &str) -> bool,
        seen: &mut [bool],
        owner: &mut [Option<usize>],
    ) -> bool {
        for l in 0..lines.len() {
            if seen[l] || !fits(needles[n], &lines[l]) {
                continue;
            }
            seen[l] = true;
            if owner[l].is_none_or(|m| augment(m, needles, lines, fits, seen, owner)) {
                owner[l] = Some(n);
                return true;
            }
        }
        false
    }
    let missing = (0..needles.len())
        .filter(|&n| {
            let mut seen = vec![false; lines.len()];
            !augment(n, needles, lines, &fits, &mut seen, &mut owner)
        })
        .map(|n| needles[n])
        .collect();
    let extra = (0..lines.len())
        .filter(|&l| owner[l].is_none())
        .map(|l| lines[l].as_str())
        .collect();
    (missing, extra)
}

#[test]
fn negative_fixtures() {
    let dirs = fixture_dirs("negative");
    assert!(!dirs.is_empty(), "no negative fixtures found");
    let mut failures = Vec::new();
    for dir in dirs {
        let input = dir.join("input.bynk");
        let src_dir = dir.join("src");
        let expected_error = dir.join("expected_error.txt");
        let has_expected_error = expected_error.exists();
        // `expected_diagnostics.txt` needs path attribution, which only a
        // project-mode compile carries — a single-file fixture must still
        // supply `expected_error.txt`, or nothing here would ever check it.
        let has_expected_diagnostics =
            src_dir.is_dir() && dir.join("expected_diagnostics.txt").exists();
        assert!(
            has_expected_error || has_expected_diagnostics,
            "{}: negative fixture has neither expected_error.txt nor \
             (a project-mode) expected_diagnostics.txt",
            dir.display()
        );
        let want = if has_expected_error {
            read(&expected_error)
        } else {
            String::new()
        };
        let want = want.trim();
        if input.exists() {
            let source = read(&input);
            let name = input.display().to_string();
            match bynkc::compile(&source, &name) {
                Ok(_) => {
                    failures.push(format!(
                        "\n=== {} ===\nexpected compile failure but compilation succeeded",
                        dir.display(),
                    ));
                }
                Err(errors) => {
                    // Each line carries the diagnostic's 1-indexed position as
                    // an ` @ line:col` suffix, so a fixture can pin the span
                    // (`code message @ 5:22`) — plain `code message` needles
                    // keep matching as substrings of the same line.
                    let lines: Vec<String> = errors
                        .iter()
                        .map(|e| {
                            let (line, col) = bynk_syntax::span::line_col(&source, e.span.start);
                            format!("{} {} @ {line}:{col}", e.category, e.message)
                        })
                        .collect();
                    check_expected_errors(&dir, want, &lines, &mut failures);
                }
            }
        } else if src_dir.is_dir() {
            let target = fixture_target(&dir);
            match compile_fixture(&dir, target) {
                Ok(_) => {
                    failures.push(format!(
                        "\n=== {} ===\nexpected compile failure but compilation succeeded",
                        dir.display(),
                    ));
                }
                Err(failure) => {
                    let lines: Vec<String> = failure
                        .errors
                        .iter()
                        .map(|e| format!("{} {}", e.error.category, e.error.message))
                        .collect();
                    check_expected_errors(&dir, want, &lines, &mut failures);
                    check_expected_diagnostics(
                        &dir,
                        &failure.errors,
                        &failure.snapshots,
                        &mut failures,
                    );
                }
            }
        } else {
            failures.push(format!(
                "\n=== {} ===\nfixture has neither `input.bynk` nor `src/`",
                dir.display()
            ));
        }
    }
    if !failures.is_empty() {
        panic!("negative fixtures failed:\n{}", failures.join("\n"));
    }
}
