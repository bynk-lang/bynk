//! Honest diagnostic transcripts.
//!
//! Each `site/src/diagnostics/<id>.bynk` is a deliberately *failing* Bynk program.
//! This test compiles it through the same path the doc-example gate uses,
//! asserts it fails, and renders the real diagnostic — colour-disabled, with a
//! stable `<id>.bynk` filename label — into the committed transcript
//! `site/src/diagnostics/<id>.txt`. The Book `{{#include}}` both files, so a page
//! showing "the compiler refuses this, and here is what it says" cannot drift
//! from the compiler.
//!
//! Regenerate the transcripts after a diagnostic change with:
//!     BYNK_BLESS=1 cargo test -p bynkc --test it doc_diagnostics::

use std::fs;
use std::path::PathBuf;

use bynkc::CompileError;

fn diagnostics_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../site/src/diagnostics")
}

fn first_line(body: &str) -> &str {
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

/// Compile a fixture through the same path as the doc-example gate: a `commons`
/// block in-process, a `context` block as a one-file temp project.
fn compile_fixture(id: &str, source: &str) -> Result<(), Vec<CompileError>> {
    let first = first_line(source);
    if first.starts_with("commons ") {
        bynkc::compile(source, &format!("{id}.bynk")).map(|_| ())
    } else if first.starts_with("context ") {
        let name = first
            .strip_prefix("context")
            .unwrap_or("")
            .trim()
            .trim_end_matches('{')
            .trim();
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("doc-diag-{id}"));
        let _ = fs::remove_dir_all(&root);
        let rel: PathBuf = name.split('.').collect::<PathBuf>().with_extension("bynk");
        let file = root.join(&rel);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, source).unwrap();
        let result = bynkc::compile_project(&bynk_testkit::compile_options_single(root.clone()))
            .map_err(bynkc::ProjectFailure::flatten)
            .map(|_| ());
        let _ = fs::remove_dir_all(&root);
        result
    } else {
        panic!("fixture {id}.bynk must start with `commons` or `context` (got `{first}`)");
    }
}

#[test]
fn diagnostic_transcripts_are_up_to_date() {
    let dir = diagnostics_dir();
    let mut fixtures: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("read site/src/diagnostics")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "bynk"))
        .collect();
    fixtures.sort();
    assert!(
        !fixtures.is_empty(),
        "no .bynk fixtures under site/src/diagnostics"
    );

    let bless = std::env::var_os("BYNK_BLESS").is_some();
    let mut failures: Vec<String> = Vec::new();

    for bynk in fixtures {
        let id = bynk.file_stem().unwrap().to_str().unwrap().to_string();
        let source = fs::read_to_string(&bynk).unwrap();

        // A diagnostics fixture that compiles is a bug — it has nothing to show.
        let errors = match compile_fixture(&id, &source) {
            Ok(()) => {
                failures.push(format!(
                    "{id}.bynk compiled, but a diagnostics fixture must fail to compile. \
                     Make it error (or remove it)."
                ));
                continue;
            }
            Err(errors) => errors,
        };

        let transcript = bynkc::render_errors_plain(&errors, &source, &format!("{id}.bynk"));
        let txt = dir.join(format!("{id}.txt"));

        if bless {
            fs::write(&txt, &transcript).unwrap();
            continue;
        }

        let current = fs::read_to_string(&txt).unwrap_or_default();
        if current != transcript {
            failures.push(format!(
                "site/src/diagnostics/{id}.txt is out of date with the compiler.\n\
                 Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it doc_diagnostics::\n\
                 --- committed ---\n{current}\n--- current ---\n{transcript}"
            ));
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
