//! #2223 follow-up: what the editor needs to know about a project's path
//! dependencies.
//!
//! The compile folds a dependency's units in under canonical names
//! (`bynk_check::packages::apply_graph`), but completion, hover and the
//! unit-name rungs of go-to-definition read source text, where a consumer
//! writes `money.currency` and the dependency declares plain `currency`. A
//! [`DependencyView`] bridges the two: it resolves a written `<alias>.<unit>`
//! to the dependency and the unit it declares, says whether the unit is
//! exported, and tells which files belong to a dependency, so the editor
//! offers exactly what the compiler accepts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_project::{PackageGraph, PackageId};

/// A written dependency reference, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyTarget {
    /// The alias the reference was written with.
    pub alias: String,
    /// The dependency it names.
    pub package: PackageId,
    /// The unit name the dependency itself declares.
    pub unit: String,
    /// The compiler's canonical name for the unit.
    pub canonical: String,
    /// Whether the dependency's `exports` reach it.
    pub exported: bool,
}

/// The project's dependency graph, as the editor reads it.
#[derive(Debug, Clone)]
pub struct DependencyView {
    graph: PackageGraph,
}

impl DependencyView {
    /// The view for the project at `root`, reading manifests from `files`
    /// first, as the build does. `None` when it declares no dependencies.
    pub fn for_root(root: &Path, files: &HashMap<PathBuf, String>) -> Option<Self> {
        let (graph, _problems) = bynk_project::resolve_graph(root, files);
        graph.map(|graph| DependencyView { graph })
    }

    /// The view for the project whose manifest is in `files` — the content
    /// map the language server sweeps, which holds the project's `bynk.toml`
    /// beside its sources. A workspace root's manifest names no project, so it
    /// is skipped.
    pub fn from_files(files: &HashMap<PathBuf, String>) -> Option<Self> {
        let mut manifests: Vec<&PathBuf> = files
            .keys()
            .filter(|p| p.file_name().is_some_and(|n| n == "bynk.toml"))
            // A `[dependencies]` table (or `[dependencies.<alias>]`); the
            // graph reads it properly.
            .filter(|p| {
                files[*p]
                    .lines()
                    .any(|l| l.trim_start().starts_with("[dependencies"))
            })
            .collect();
        manifests.sort();
        let root = manifests.first()?.parent()?.to_path_buf();
        Self::for_root(&root, files)
    }

    /// The project's aliases, in order.
    pub fn aliases(&self) -> impl Iterator<Item = (&str, &PackageId)> {
        self.graph
            .root_aliases
            .iter()
            .map(|(alias, &i)| (alias.as_str(), &self.graph.packages[i].id))
    }

    /// Resolve `written` (`money.currency`) when its first segment is one of
    /// the project's aliases.
    pub fn resolve(&self, written: &str) -> Option<DependencyTarget> {
        let (alias, unit) = written.split_once('.')?;
        let &i = self.graph.root_aliases.get(alias)?;
        let package = &self.graph.packages[i];
        let exported = package
            .exports
            .as_ref()
            .is_none_or(|list| list.iter().any(|e| e == unit));
        Some(DependencyTarget {
            alias: alias.to_owned(),
            package: package.id.clone(),
            unit: unit.to_owned(),
            canonical: package.id.canonical(unit),
            exported,
        })
    }

    /// The dependency whose source trees hold `path`, if any. `path` may be
    /// relative; it is compared as `std::path::absolute` gives it, as the
    /// compile tags its files.
    pub fn package_of(&self, path: &Path) -> Option<&PackageId> {
        self.graph
            .package_of(path)
            .map(|i| &self.graph.packages[i].id)
    }

    /// Whether `path` is one of a dependency's files.
    pub fn is_dependency_file(&self, path: &Path) -> bool {
        self.package_of(path).is_some()
    }

    /// The aliases the project reaches `id` by.
    pub fn aliases_of(&self, id: &PackageId) -> Vec<&str> {
        self.aliases()
            .filter(|(_, p)| *p == id)
            .map(|(a, _)| a)
            .collect()
    }

    /// Whether `id` shares `unit` (its `exports`, or every unit when absent).
    pub fn exports(&self, id: &PackageId, unit: &str) -> bool {
        self.graph
            .by_id(id)
            .map(|i| &self.graph.packages[i])
            .is_some_and(|p| {
                p.exports
                    .as_ref()
                    .is_none_or(|l| l.iter().any(|e| e == unit))
            })
    }
}

/// The line hover appends for a dependency unit.
pub fn hover_note(target: &DependencyTarget) -> String {
    let mut note = format!(
        "From dependency `{}` (alias `{}`).",
        target.package.name(),
        target.alias
    );
    if !target.exported {
        note.push_str(" Not exported: `bynk check` refuses this reference.");
    }
    note
}
