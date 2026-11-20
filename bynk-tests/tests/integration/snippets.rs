//! The By Example snippets gate (documentation track, slice 4).
//!
//! Every `site/src/snippets/**/*.bynk` is a bite-size, single-file program shown
//! in the docs with an "Open in playground" link. This gate compiles each one as
//! the playground does (in memory, the `Bundle` target on the browser platform),
//! so a snippet the docs invite you to open and run can never fall out of step
//! with the compiler (track §6/§10).
//!
//! The playground's Run calls only `service playground`'s `on call` handler
//! (#3767), so the gate also checks that every runnable `<Example>` on the site
//! plays a source that has one: its `playgroundSrc` when it names one, else its
//! `src`.

use std::fs;
use std::path::{Path, PathBuf};

use bynk_check::firstparty::Platform;
use bynk_driver::project::{BuildTarget, compile_in_memory};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn snippets_dir() -> PathBuf {
    repo_root().join("site/src/snippets")
}

fn gather(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            gather(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

/// Compile `source` as the playground does. `Ok` carries the services whose
/// `on call` handler Run could call.
fn compile_like_the_playground(source: &str) -> Result<Vec<String>, String> {
    compile_in_memory(source, BuildTarget::Bundle, Platform::Browser)
        .map(|out| out.entry_handlers.into_iter().map(|h| h.service).collect())
        .map_err(|failure| {
            failure
                .flatten()
                .iter()
                .map(|e| format!("{}: {}", e.category, e.message))
                .collect::<Vec<_>>()
                .join("; ")
        })
}

#[test]
fn every_snippet_compiles() {
    let mut files = Vec::new();
    gather(&snippets_dir(), "bynk", &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "no .bynk snippets under site/src/snippets"
    );

    let failures: Vec<String> = files
        .iter()
        .filter_map(|file| {
            let source = fs::read_to_string(file).unwrap();
            compile_like_the_playground(&source)
                .err()
                .map(|msg| format!("{}: {msg}", file.display()))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "snippet compilation gate failed:\n  {}",
        failures.join("\n  ")
    );
}

/// The value of a string attribute `name="…"` inside one `<Example …>` tag.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = tag
        .match_indices(&needle)
        // Not the tail of a longer attribute name (`src` inside `playgroundSrc`).
        .find(|(i, _)| *i == 0 || tag.as_bytes()[i - 1].is_ascii_whitespace())?
        .0
        + needle.len();
    let len = tag[start..].find('"')?;
    Some(&tag[start..start + len])
}

#[test]
fn every_runnable_example_has_a_playground_service() {
    let mut pages = Vec::new();
    gather(
        &repo_root().join("site/src/content/docs"),
        "mdx",
        &mut pages,
    );
    pages.sort();

    let mut runnable = 0;
    let mut failures = Vec::new();
    for page in &pages {
        let text = fs::read_to_string(page).unwrap();
        for (at, _) in text.match_indices("<Example") {
            let tag = &text[at..at + text[at..].find("/>").expect("an <Example> closes")];
            if !tag.split_whitespace().any(|w| w == "runnable") {
                continue;
            }
            runnable += 1;
            let Some(played) = attr(tag, "playgroundSrc").or_else(|| attr(tag, "src")) else {
                failures.push(format!(
                    "{}: a runnable <Example> with no string src",
                    page.display()
                ));
                continue;
            };
            let source = fs::read_to_string(repo_root().join(played))
                .unwrap_or_else(|e| panic!("{}: read {played}: {e}", page.display()));
            match compile_like_the_playground(&source) {
                Ok(services) if services.iter().any(|s| s == "playground") => {}
                Ok(_) => failures.push(format!(
                    "{}: {played} has no `service playground {{ on call() … }}`, so Run has nothing to call",
                    page.display()
                )),
                Err(msg) => failures.push(format!("{}: {played}: {msg}", page.display())),
            }
        }
    }
    assert!(runnable > 0, "found no runnable <Example> on the site");
    assert!(
        failures.is_empty(),
        "runnable-example gate failed:\n  {}",
        failures.join("\n  ")
    );
}
