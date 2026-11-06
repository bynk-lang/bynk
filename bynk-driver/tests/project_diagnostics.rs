//! #696: a project-level validation diagnostic must render with ariadne source
//! context — the same rich, boxed report a single-file parse/resolve error gets
//! — not the plain `[category] message` fallback. The regression went unnoticed
//! because `expected_error.txt` fixtures assert category strings only (ADR 0198),
//! so the rendering path had no coverage. This drives the real project pipeline
//! and renders the way `bynk_driver::print_project_failure` does.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_driver::project;

/// A throwaway on-disk project, removed on drop (including on panic).
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scratch_project(tag: &str, files: &[(&str, &str)]) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "bynk_696_{tag}_{}_{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    for (rel, body) in files {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
    }
    Scratch(dir)
}

/// A `consumes` cycle — a project-level diagnostic built in `graph.rs` — must be
/// attributed to a file that is present in the failure's snapshots, carry a real
/// span (not the `0..0` fallback), and render with ariadne source context.
#[test]
fn a_consumes_cycle_renders_with_ariadne_source_context() {
    let root = scratch_project(
        "cycle",
        &[
            (
                "src/commerce/a.bynk",
                "context commerce.a {\n  consumes commerce.b\n}\n",
            ),
            (
                "src/commerce/b.bynk",
                "context commerce.b {\n  consumes commerce.a\n}\n",
            ),
        ],
    );

    // Root the project exactly as every `check`/`compile` command does.
    let opts = bynk_driver::project_options(&root.0).expect("scratch project must be readable");
    let failure = match project::compile_project(&opts) {
        Err(f) => f,
        Ok(_) => panic!("a `consumes` cycle must fail the build"),
    };

    let snapshots: HashMap<&Path, &str> = failure
        .snapshots
        .iter()
        .map(|(p, t)| (p.as_path(), t.as_str()))
        .collect();

    let cycle = failure
        .errors
        .iter()
        .find(|e| e.error.category == "bynk.context.consumes_cycle")
        .expect("the build must report a consumes cycle");

    // #696: the diagnostic is attributed to a real source file …
    let path = cycle
        .source_path
        .as_deref()
        .expect("the consumes cycle must be attributed to a file, not left unattributed");
    // … whose text is in the snapshot map (what makes the CLI take the rich branch) …
    let text = snapshots
        .get(path)
        .expect("the attributed file must have a snapshot to render against");
    // … and anchored on a real clause span, not the `0..0` project-level fallback.
    assert!(
        cycle.error.span.start > 0,
        "the cycle must be anchored on a real `consumes` clause span, got {:?}",
        cycle.error.span,
    );

    // Render exactly as `print_project_failure`'s rich branch does.
    let label = path.to_string_lossy().replace('\\', "/");
    let rendered =
        bynk_render::render_errors_plain(std::slice::from_ref(&cycle.error), text, &label);

    // The ariadne report frames the source excerpt with `╭─[<file>:<line>:<col>]`;
    // the plain fallback (`[category] message`) never does.
    assert!(
        rendered.contains("╭─["),
        "expected an ariadne source-context report, got:\n{rendered}",
    );
    assert!(
        rendered.contains(&format!("{label}:")),
        "the report must locate the diagnostic in its source file, got:\n{rendered}",
    );
}

/// The common cross-file-label case (#696): `kind_conflict` is *always* between
/// two files, so its "first declared here" provenance points into a different
/// file than the one the error is attributed to. It must render as a **note**
/// carrying the other file's path — never a secondary label that would underline
/// this file's own text (or panic on a mid-codepoint offset). The `consumes`
/// cycle above has no secondary label, so it cannot exercise this path.
#[test]
fn a_cross_file_kind_conflict_renders_the_other_file_as_a_note() {
    // Two files of the multi-file unit `shop.orders`, in the same directory,
    // declaring it as different kinds — a kind conflict with no path/name or
    // directory violation to muddy the diagnostics.
    let root = scratch_project(
        "kindconflict",
        &[
            (
                "src/shop/orders/a.bynk",
                "commons shop.orders {\n  type T = Int\n}\n",
            ),
            ("src/shop/orders/b.bynk", "context shop.orders {\n}\n"),
        ],
    );

    let opts = bynk_driver::project_options(&root.0).expect("scratch project must be readable");
    let failure = match project::compile_project(&opts) {
        Err(f) => f,
        Ok(_) => panic!("a kind conflict must fail the build"),
    };

    let snapshots: HashMap<&Path, &str> = failure
        .snapshots
        .iter()
        .map(|(p, t)| (p.as_path(), t.as_str()))
        .collect();

    let conflict = failure
        .errors
        .iter()
        .find(|e| e.error.category == "bynk.project.kind_conflict")
        .expect("the build must report a kind conflict");

    let path = conflict
        .source_path
        .as_deref()
        .expect("the kind conflict must be attributed to a file");
    let text = snapshots
        .get(path)
        .expect("the attributed file must have a snapshot");

    let label = path.to_string_lossy().replace('\\', "/");
    let rendered =
        bynk_render::render_errors_plain(std::slice::from_ref(&conflict.error), text, &label);

    // Rich source context for the primary span …
    assert!(
        rendered.contains("╭─["),
        "expected an ariadne source-context report, got:\n{rendered}",
    );
    // … and the other file's provenance survives as a note (not a label): the
    // note names the *other* file, which is never the one we rendered against.
    assert!(
        rendered.contains("first declared as a") && rendered.contains(".bynk`"),
        "the first-declaration site must render as a note naming the other file, got:\n{rendered}",
    );
    let other = if label.ends_with("a.bynk") {
        "b.bynk"
    } else {
        "a.bynk"
    };
    assert!(
        rendered.contains(other),
        "the note must point at the *other* file (`{other}`), got:\n{rendered}",
    );
}

/// #1081 review, finding 1: `project_options` populates `CompileOptions.sources`,
/// which made `run_checks` take the `discovered: Some(...)` branch — and until
/// `check_discovered_files` was pulled out to run unconditionally, that branch
/// skipped `phase_discovery` (and therefore `bynk.project.no_sources`) entirely.
/// An empty project compiled through the real CLI path must still report it.
#[test]
fn an_empty_project_still_reports_no_sources_through_the_cli_path() {
    let root = scratch_project("no_sources", &[(".keep", "")]);

    let opts = bynk_driver::project_options(&root.0).expect("scratch project must be readable");
    let check = project::check_project(&opts);
    assert!(
        check
            .errors
            .iter()
            .any(|e| e.error.category == "bynk.project.no_sources"),
        "expected `bynk.project.no_sources`, got: {:?}",
        check
            .errors
            .iter()
            .map(|e| &e.error.category)
            .collect::<Vec<_>>()
    );
}

/// The `file_and_directory` counterpart to the test above: a project with both
/// `x.bynk` and `x/` present must still be flagged through the CLI path, not
/// only through the LSP's `discovered: None` walk.
#[test]
fn a_file_directory_ambiguity_still_reports_through_the_cli_path() {
    let root = scratch_project(
        "file_and_dir",
        &[
            ("src/thing.bynk", "context thing\n"),
            ("src/thing/part.bynk", "context thing.part\n"),
        ],
    );

    let opts = bynk_driver::project_options(&root.0).expect("scratch project must be readable");
    let check = project::check_project(&opts);
    assert!(
        check
            .errors
            .iter()
            .any(|e| e.error.category == "bynk.project.file_and_directory"),
        "expected `bynk.project.file_and_directory`, got: {:?}",
        check
            .errors
            .iter()
            .map(|e| &e.error.category)
            .collect::<Vec<_>>()
    );
}

/// #1077 review: `project_options`/`try_project_options` now read `bynk.toml`
/// through their own overlay ([`manifest_overlay`]) rather than `bynk-emit`'s
/// disk fallback (`discovery::read_source`'s `fs::read_to_string` miss path) —
/// see that function's doc comment for why the CLI path silently depended on
/// it even after #1081. This is a real regression test for `[paths]` parsing
/// reaching `project_options` correctly through that new overlay-based read —
/// a non-conventional `[paths]` (no `src`/`tests` at all, one `include` root
/// the conventional default would never guess, and an `exclude` under it) —
/// but it does **not** by itself prove the overlay is load-bearing:
/// `read_source`'s disk fallback is still present, so a mis-keyed overlay
/// would silently miss and re-read the same on-disk `bynk.toml`, giving an
/// identical (still-passing) result. `manifest_overlay_keys_and_reads_a_real_bynk_toml`
/// (`bynk-driver/src/lib.rs`) is the test that actually pins the key match.
#[test]
fn a_non_conventional_manifest_include_and_exclude_are_honoured() {
    let root = scratch_project(
        "manifest_overlay",
        &[
            (
                "bynk.toml",
                "[paths]\ninclude = [\"lib\"]\nexclude = [\"lib/vendor\"]\n",
            ),
            ("lib/thing.bynk", "context thing\n"),
            ("lib/vendor/thing.bynk", "context thing\n"),
        ],
    );

    let opts = bynk_driver::project_options(&root.0).expect("scratch project must be readable");
    let sources = opts
        .sources
        .as_ref()
        .expect("project_options always populates sources (#1077/#1081)");

    assert!(
        sources.keys().any(|p| p.ends_with("lib/thing.bynk")),
        "expected the declared `include` root to be walked, got: {:?}",
        sources.keys().collect::<Vec<_>>()
    );
    assert!(
        !sources.keys().any(|p| p.ends_with("lib/vendor/thing.bynk")),
        "expected the declared `exclude` to be honoured, got: {:?}",
        sources.keys().collect::<Vec<_>>()
    );
}
