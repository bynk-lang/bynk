//! In-process test helpers shared across `bynk-emit`'s `#[cfg(test)]` modules
//! (#57, testing track). Before this existed, driving anything beyond a
//! self-contained single-file commons through the emitter meant going through
//! `bynkc`'s on-disk fixture directories — the only fs-free seam
//! (`project::compile_in_memory`) was private to one test module
//! (`emitter::conditional_runtime_import_tests`), so every other test that
//! needed the project pipeline (cross-context `uses`, multi-file `Split`
//! layouts, …) would have hand-rolled its own copy or fallen back to a disk
//! fixture. Promoted here so it can't happen twice.
//!
//! #1830 added the multi-file form: [`emit_project`] (any target) and
//! [`emit_workers`] compile several in-memory files — a context with the
//! contexts it consumes, or a unit with its `tests/` suite — and return every
//! emitted document by output path. No disk access: the files ride in the
//! overlay, as for [`crate::project::compile_in_memory`], so this module stays
//! clear of `fs_below_driver`.
//!
//! ```ignore
//! let out = emit_workers(&[("demo/shop.bynk", SHOP)]);
//! let index = out.text("workers/demo-shop/index.ts");
//! assert!(index.contains("if (path.startsWith(\"/_bynk/call/\"))"));
//! ```

/// Compile a self-contained single-file commons and return its TypeScript.
/// Panics with the source on a compile failure — a test fixture that doesn't
/// compile is a bug in the test, not a case to assert on.
pub(crate) fn emit_source(src: &str) -> String {
    crate::compile(src, "t.bynk").expect("fixture should compile")
}

/// Emit a `commons app.bundle` wrapping `body`, pre-declared with `uses
/// bynk.locale`/`uses bynk.locale.types`, and return its own module's
/// TypeScript out of the project pipeline.
///
/// A bundle needing `uses bynk.locale` can't go through [`emit_source`] — the
/// single-file path rejects `uses` by construction — so this drives
/// [`crate::project::compile_in_memory`] instead and picks the user's own
/// unit out of the returned module graph.
pub(crate) fn emit_bundle(body: &str) -> String {
    let src = format!("commons app.bundle\n\nuses bynk.locale\nuses bynk.locale.types\n\n{body}");
    let out = match crate::project::compile_in_memory(
        &src,
        crate::project::BuildTarget::Bundle,
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
/// failure, as [`emit_source`] does.
///
/// ```ignore
/// let out = emit_project(&[("demo/shop.bynk", SHOP)], BuildTarget::Workers);
/// assert!(out.text("workers/demo/shop.ts").contains("export class"));
/// ```
pub(crate) fn emit_project(files: &[(&str, &str)], target: crate::project::BuildTarget) -> Emitted {
    emit_project_with_tests(files, target, true)
}

/// [`emit_project`], choosing whether `suite`s are kept: `tests: false` is the
/// deployable build (`bynkc compile`, `bynk deploy`).
pub(crate) fn emit_project_with_tests(
    files: &[(&str, &str)],
    target: crate::project::BuildTarget,
    tests: bool,
) -> Emitted {
    match crate::project::compile_files_in_memory(files, target, Default::default(), tests) {
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
pub(crate) fn emit_workers(files: &[(&str, &str)]) -> Emitted {
    emit_project(files, crate::project::BuildTarget::Workers)
}

/// A compiled project's documents, by output path. Keyed by `PathBuf`, so a
/// `/`-separated lookup in [`Emitted::text`] matches on every platform.
pub(crate) struct Emitted {
    pub(crate) docs: std::collections::BTreeMap<std::path::PathBuf, String>,
}

impl Emitted {
    /// The text of the document at `path`; panics listing every path if
    /// there is none.
    pub(crate) fn text(&self, path: &str) -> &str {
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

#[cfg(test)]
mod tests {
    use super::*;

    const MATH: &str = "commons demo.math\n\nfn double(n: Int) -> Int { n * 2 }\n";

    #[test]
    fn a_workers_build_emits_one_worker_directory_per_context() {
        let shop = "context demo.shop\n\nservice api {\n  on call(n: Int) -> Effect[Int] {\n    Effect.pure(n)\n  }\n}\n";
        let out = emit_workers(&[("demo/shop.bynk", shop)]);
        for doc in ["index.ts", "compose.ts", "handlers.ts", "wrangler.toml"] {
            out.text(&format!("workers/demo-shop/{doc}"));
        }
    }

    #[test]
    fn a_unit_with_a_suite_emits_the_suite_module() {
        let suite = "suite demo.math\n\ncase \"doubles\" {\n  expect double(2) == 4\n}\n";
        let out = emit_project(
            &[("demo/math.bynk", MATH), ("tests/math.bynk", suite)],
            crate::project::BuildTarget::Bundle,
        );
        assert!(out.text("tests/demo_math.test.ts").contains("doubles"));
    }

    #[test]
    #[should_panic(expected = "project fixture should compile")]
    fn a_project_that_fails_to_compile_panics_with_its_diagnostics() {
        emit_project(
            &[(
                "demo/math.bynk",
                "commons demo.math\n\nfn f() -> Int { \"x\" }\n",
            )],
            crate::project::BuildTarget::Bundle,
        );
    }
}
