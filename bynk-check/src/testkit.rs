//! In-process test helpers shared across `bynk-check`'s `#[cfg(test)]`
//! modules (#1830).
//!
//! [`analyse`] runs a small multi-unit project through the real
//! [`analyse_project`] pipeline — discovery, parse, group, `uses`/`consumes`
//! resolution, checking, test suites — and hands back an [`Analysed`] to
//! assert on. It is the unit-level alternative to a `bynk` fixture directory:
//! the program is a few string literals next to the test, and the assertion
//! is on what the checker decided (a diagnostic, the type of an expression,
//! the declaration a name resolved to) rather than on emitted text.
//!
//! ```ignore
//! let a = analyse(&[
//!     ("demo/money.bynk", "commons demo.money\n\ntype Cents = Int where NonNegative\n"),
//!     ("demo/shop.bynk", "context demo.shop\n\nuses demo.money\n\nfn price() -> Cents { 100 }\n"),
//! ]);
//! a.assert_clean();
//! assert_eq!(a.type_at("demo/shop.bynk", "100"), "Cents");
//! assert_eq!(a.resolves_to("demo/shop.bynk", "Cents").unit, "demo.money");
//! ```
//!
//! Each file is written under a fresh temporary root (discovery walks the
//! disk; the overlay then supplies every file's text), which is removed when
//! the [`Analysed`] is dropped. Paths are relative to that root, and a file's
//! path decides its unit's directory exactly as it does in a real project.
//!
//! `bynk-check` is outside `fs_below_driver`'s scope (`bynk-emit`,
//! `bynk-ide`, `bynk-fmt`), so the temporary root needs no exception there.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use bynk_project::Roots;
use bynk_syntax::error::Severity;

use crate::analysis::{ProjectAnalysis, analyse_project};
use crate::index::SymbolKey;

/// Unit tests run in parallel inside one process, so the process id alone
/// does not make a root unique.
static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

/// Analyse the project made of `files` (`(relative path, source)` pairs).
pub(crate) fn analyse(files: &[(&str, &str)]) -> Analysed {
    analyse_with(files, None)
}

/// [`analyse`] under a given stability policy (S3, #1963): how a test treats a
/// shipped item as experimental, since none is yet. `None` reads the project's
/// own, as [`analyse_project`] does.
pub(crate) fn analyse_with(
    files: &[(&str, &str)],
    policy: Option<crate::stability::StabilityPolicy>,
) -> Analysed {
    let root = std::env::temp_dir().join(format!(
        "bynk-check-testkit-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let mut overlay = HashMap::new();
    for (rel, src) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("a file path has a parent"))
            .expect("create the fixture directory");
        std::fs::write(&path, src).expect("write the fixture file");
        overlay.insert(path, (*src).to_string());
    }
    let roots = Roots::Single(root.clone());
    let analysis = match policy {
        Some(policy) => crate::analysis::analyse_project_with_policy(
            &roots,
            &overlay,
            policy,
            &bynk_project::SchemaLock::Off,
        ),
        None => analyse_project(&roots, &overlay, &bynk_project::SchemaLock::Off),
    };
    Analysed {
        root,
        sources: files
            .iter()
            .map(|(rel, src)| (PathBuf::from(rel), (*src).to_string()))
            .collect(),
        analysis,
    }
}

/// A finished analysis, plus the sources it was built from so a test can
/// locate an expression by its text instead of by a hand-counted offset.
pub(crate) struct Analysed {
    root: PathBuf,
    sources: HashMap<PathBuf, String>,
    pub(crate) analysis: ProjectAnalysis,
}

impl Drop for Analysed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Analysed {
    /// Every diagnostic's category, errors and warnings alike, in reported
    /// order.
    pub(crate) fn categories(&self) -> Vec<&'static str> {
        self.analysis
            .errors
            .iter()
            .map(|e| e.error.category)
            .collect()
    }

    /// The categories of error-severity diagnostics only.
    pub(crate) fn error_categories(&self) -> Vec<&'static str> {
        self.analysis
            .errors
            .iter()
            .filter(|e| Severity::for_error(&e.error) == Severity::Error)
            .map(|e| e.error.category)
            .collect()
    }

    /// Panic, listing every diagnostic, unless the project has none at all.
    pub(crate) fn assert_clean(&self) {
        assert!(
            self.analysis.errors.is_empty(),
            "expected a clean project, got:\n{}",
            self.render()
        );
    }

    /// Panic unless some diagnostic has exactly `category`.
    pub(crate) fn assert_reports(&self, category: &str) {
        assert!(
            self.categories().contains(&category),
            "expected a `{category}` diagnostic, got:\n{}",
            self.render()
        );
    }

    /// The displayed type of the expression spelled by the first occurrence
    /// of `needle` in `file`. The expression's span must cover `needle`
    /// exactly; the narrowest such span wins when several do (a parenthesised
    /// or block-wrapped expression shares its inner expression's text).
    ///
    /// A file that fails to check still records the types of whatever did
    /// check (ADR 0094, `check_pipeline::record_analyse_types`), so this works
    /// on a broken file too, for expressions outside the broken declaration.
    pub(crate) fn type_at(&self, file: &str, needle: &str) -> String {
        self.analysis.ty_intern.display(self.ty_id_at(file, needle))
    }

    /// [`type_at`](Self::type_at)'s type itself rather than its display, for
    /// a test that must tell apart two same-named types (a local `type Repo`
    /// and a used commons' `Repo` both display as `Repo`; their
    /// [`NamedKind`](crate::checker::NamedKind)s differ).
    pub(crate) fn ty_at(&self, file: &str, needle: &str) -> crate::checker::Ty {
        self.analysis
            .ty_intern
            .get(self.ty_id_at(file, needle))
            .as_ref()
            .clone()
    }

    fn ty_id_at(&self, file: &str, needle: &str) -> crate::checker::TyId {
        let (start, end) = self.locate(file, needle);
        let types = self
            .analysis
            .expr_types
            .iter()
            .find(|(path, _)| path.ends_with(file))
            .map(|(_, types)| types)
            .unwrap_or_else(|| {
                panic!(
                    "no expression types recorded for `{file}`:\n{}",
                    self.render()
                )
            });
        types
            .iter()
            .find(|(span, _)| span.start == start && span.end == end)
            .map(|(_, id)| *id)
            .unwrap_or_else(|| panic!("no expression spans exactly `{needle}` in `{file}`"))
    }

    /// The declaration the reference spelled by the first occurrence of
    /// `needle` in `file` resolved to, as the binding index recorded it.
    /// The occurrence must be a reference, not the declaration itself.
    pub(crate) fn resolves_to(&self, file: &str, needle: &str) -> &SymbolKey {
        let (start, _) = self.locate(file, needle);
        self.analysis
            .index
            .symbols
            .iter()
            .find(|(_, entry)| {
                entry
                    .refs
                    .iter()
                    .any(|r| r.path.ends_with(file) && r.span.range().contains(&start))
            })
            .map(|(key, _)| key)
            .unwrap_or_else(|| panic!("`{needle}` in `{file}` is not a recorded reference"))
    }

    /// The byte range of the first occurrence of `needle` in `file`.
    fn locate(&self, file: &str, needle: &str) -> (usize, usize) {
        let src = self
            .sources
            .get(Path::new(file))
            .unwrap_or_else(|| panic!("no file `{file}` in this project"));
        let start = src
            .find(needle)
            .unwrap_or_else(|| panic!("`{needle}` does not occur in `{file}`"));
        (start, start + needle.len())
    }

    /// Every diagnostic as `path: category: message`, one per line.
    pub(crate) fn render(&self) -> String {
        self.analysis
            .errors
            .iter()
            .map(|e| {
                let path = e
                    .source_path
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                format!("{path}: {}: {}\n", e.error.category, e.error.message)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONEY: &str = "commons demo.money\n\ntype Cents = Int where NonNegative\n";
    const SHOP: &str = "context demo.shop\n\nuses demo.money\n\nfn price() -> Cents { 100 }\n";

    #[test]
    fn a_clean_two_unit_project_types_and_resolves_across_uses() {
        let a = analyse(&[("demo/money.bynk", MONEY), ("demo/shop.bynk", SHOP)]);
        a.assert_clean();
        assert_eq!(a.type_at("demo/shop.bynk", "100"), "Cents");
        let key = a.resolves_to("demo/shop.bynk", "Cents");
        assert_eq!(
            (key.unit.as_str(), key.name.as_str()),
            ("demo.money", "Cents")
        );
    }

    #[test]
    fn a_broken_project_reports_its_diagnostic() {
        let a = analyse(&[(
            "demo/shop.bynk",
            "context demo.shop\n\nfn price() -> Int { \"free\" }\n",
        )]);
        assert!(!a.error_categories().is_empty(), "expected an error");
    }

    #[test]
    fn each_analysis_gets_its_own_root_and_removes_it() {
        let a = analyse(&[("demo/money.bynk", MONEY)]);
        let b = analyse(&[("demo/money.bynk", MONEY)]);
        assert_ne!(a.root, b.root);
        let root = a.root.clone();
        drop(a);
        assert!(!root.exists());
    }
}
