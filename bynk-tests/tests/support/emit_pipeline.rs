//! The in-memory pipeline helpers the moved `bynk-emit` pipeline tests share (1B.2b, #4028).
//!
//! They were `bynk_emit::testkit::{emit_bundle, emit_project, emit_project_with_tests,
//! emit_workers, Emitted}`, which called `bynk_driver::project::compile_in_memory` and
//! `compile_files_in_memory` from a `bynk-emit` dev-dependency on the driver. That edge made a
//! cycle in the crate graph, so the helpers live here, beside the driver they drive. They return
//! only text, as the originals did, so each test's assertions are unchanged.
//!
//! No disk access: the files ride in the overlay, as for
//! [`bynk_driver::project::compile_files_in_memory`].
//!
//! ```ignore
//! let out = emit_workers(&[("demo/shop.bynk", SHOP)]);
//! let index = out.text("workers/demo-shop/index.ts");
//! assert!(index.contains("if (path.startsWith(\"/_bynk/call/\"))"));
//! ```

// Each test file includes this module and uses a subset of it.
#![allow(dead_code)]

/// Emit a `commons app.bundle` wrapping `body`, pre-declared with `uses
/// bynk.locale`/`uses bynk.locale.types`, and return its own module's
/// TypeScript out of the project pipeline.
///
/// A bundle needing `uses bynk.locale` can't go through `bynk_emit`'s single-file path, which
/// rejects `uses` by construction, so this drives
/// [`bynk_driver::project::compile_in_memory`] and picks the user's own unit out of the returned
/// module graph.
pub fn emit_bundle(body: &str) -> String {
    let src = format!("commons app.bundle\n\nuses bynk.locale\nuses bynk.locale.types\n\n{body}");
    let out = match bynk_driver::project::compile_in_memory(
        &src,
        bynk_check::project_model::BuildTarget::Bundle,
        Default::default(),
    ) {
        Ok(out) => out,
        Err(_) => panic!("bundle fixture should compile:\n{src}"),
    };
    out.artefacts
        .docs
        .iter()
        .find(|(path, _)| path.ends_with("bundle.ts"))
        .map(|(_, doc)| doc.text())
        .expect("the bundle's own module should be in the output")
}

/// Compile the project made of `files` (`(path, source)` pairs, paths
/// relative to a single-tree root: `demo/shop.bynk` for `context demo.shop`,
/// `tests/shop.bynk` for its suite) for `target`, and return every emitted
/// document's text by output path. Panics with the diagnostics on a compile
/// failure: a test fixture that doesn't compile is a bug in the test, not a case to assert on.
///
/// ```ignore
/// let out = emit_project(&[("demo/shop.bynk", SHOP)], BuildTarget::Workers);
/// assert!(out.text("workers/demo/shop.ts").contains("export class"));
/// ```
pub fn emit_project(
    files: &[(&str, &str)],
    target: bynk_check::project_model::BuildTarget,
) -> Emitted {
    emit_project_with_tests(files, target, true)
}

/// [`emit_project`], choosing whether `suite`s are kept: `tests: false` is the
/// deployable build (`bynk compile`, `bynk deploy`).
pub fn emit_project_with_tests(
    files: &[(&str, &str)],
    target: bynk_check::project_model::BuildTarget,
    tests: bool,
) -> Emitted {
    match bynk_driver::project::compile_files_in_memory(files, target, Default::default(), tests) {
        Ok(out) => Emitted {
            docs: out
                .artefacts
                .docs
                .iter()
                .map(|(path, doc)| (path.clone(), doc.text()))
                .collect(),
        },
        Err(failure) => panic!(
            "project fixture should compile, got:\n{}",
            failure
                .flatten()
                .iter()
                .map(|e| format!("{}: {}\n", e.category, e.message))
                .collect::<String>()
        ),
    }
}

/// [`emit_project`] for the Workers target.
pub fn emit_workers(files: &[(&str, &str)]) -> Emitted {
    emit_project(files, bynk_check::project_model::BuildTarget::Workers)
}

/// A compiled project's documents, by output path. Keyed by `PathBuf`, so a
/// `/`-separated lookup in [`Emitted::text`] matches on every platform.
pub struct Emitted {
    pub docs: std::collections::BTreeMap<std::path::PathBuf, String>,
}

impl Emitted {
    /// The text of the document at `path`; panics listing every path if
    /// there is none.
    pub fn text(&self, path: &str) -> &str {
        self.docs
            .get(std::path::Path::new(path))
            .map(String::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "no document `{path}`; emitted: {:?}",
                    self.docs.keys().collect::<Vec<_>>()
                )
            })
    }
}
