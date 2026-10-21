//! Packaging slice 2a (#2223): loading a project's path dependencies into
//! one compile.
//!
//! [`apply_graph`] runs once, right after parsing, on both the build's and the
//! editor's pipeline. It resolves the project's package graph
//! ([`bynk_project::resolve_graph`]), then rewrites the parsed files so every
//! later phase sees one flat set of units, as it always has:
//!
//! - each dependency file is tagged with its package, and its unit gets the
//!   canonical name source can't spell ([`bynk_project::PackageId::canonical`],
//!   decision C) — `currency` in `acme/money` becomes `$acme_money.currency`;
//! - every `uses`/`consumes` target is resolved against its own file's
//!   package: `money.currency`, where `money` is one of that package's
//!   aliases, becomes the dependency's canonical name, and a dependency's
//!   reference to its own unit becomes its own canonical name;
//! - a whole-unit `consumes` of a dependency unit keeps the spelling source
//!   wrote as its alias, so `money.fx.Rates` still names the capability;
//! - only exported units cross a package boundary (decision D), only the
//!   units a consumer actually reaches are kept (P-5), and a dependency's
//!   contexts and suites are never compiled (ADR 0542). A reached dependency
//!   adapter's npm `requires` fold into the build with the project's own
//!   (#2255).
//!
//! The root project's own units and the first-party units keep their names,
//! so a project without dependencies compiles byte-identically.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use bynk_project::{PackageGraph, PackageId, ParsedFile, UnitKind};
use bynk_syntax::CompileError;
use bynk_syntax::ast::{CommonsItem, Ident, QualifiedName, SourceUnit};
use bynk_syntax::span::Span;

use crate::project_model::ErrorSink;

/// A consumer reaches a unit its dependency doesn't export.
pub const UNIT_NOT_EXPORTED: &str = "bynk.package.unit_not_exported";
/// A consumer reaches a unit its dependency doesn't declare.
pub const UNKNOWN_UNIT: &str = "bynk.package.unknown_unit";
/// An alias is also the first segment of one of the project's own units.
pub const ALIAS_SHADOWS_UNIT: &str = "bynk.package.alias_shadows_unit";
/// A dependency declares an `asset`, which needs a per-package namespace not
/// yet designed.
pub const UNSUPPORTED_IN_DEPENDENCY: &str = "bynk.package.unsupported_in_dependency";
/// #2240: a non-suite unit reaches a `[dev-dependencies]` alias.
pub const DEV_DEPENDENCY_IN_SOURCE: &str = "bynk.package.dev_dependency_in_source";

/// Resolve the package graph of the project at `project_root` and fold the
/// dependencies' files in `parsed` into the compile. A project with no
/// dependencies is untouched.
///
/// `tests` says whether this build checks suites: when it doesn't (a
/// deployable build), suites reach nothing, so a unit only a suite uses —
/// every `[dev-dependencies]` unit — is never compiled (#2240).
pub fn apply_graph(
    project_root: Option<&Path>,
    overlay: &HashMap<PathBuf, String>,
    parsed: &mut Vec<ParsedFile>,
    tests: bool,
    errors: &mut ErrorSink,
) {
    let Some(project_root) = project_root else {
        return;
    };
    let (graph, problems) = bynk_project::resolve_graph(project_root, overlay);
    let manifest = Path::new(crate::stability::MANIFEST);
    for p in problems {
        let mut err = CompileError::new(p.code, Span::default(), p.message);
        if let Some(note) = p.note {
            err = err.with_note(note);
        }
        errors.push_for(Some(manifest), err);
    }
    let Some(graph) = graph else {
        return;
    };

    // Tag each dependency file with its package; drop what a dependency
    // never contributes (contexts, suites).
    let mut package_of: Vec<Option<usize>> = Vec::with_capacity(parsed.len());
    for pf in parsed.iter_mut() {
        let idx = (!pf.is_synthetic())
            .then(|| pf.abs_path())
            .flatten()
            .and_then(|abs| graph.package_of(&abs));
        if let Some(i) = idx {
            pf.assign_package(graph.packages[i].id.clone());
        }
        package_of.push(idx);
    }
    let mut keep = vec![true; parsed.len()];
    for (k, pf) in parsed.iter().enumerate() {
        if package_of[k].is_some() && !matches!(pf.kind(), UnitKind::Commons | UnitKind::Adapter) {
            keep[k] = false;
        }
    }

    // The project's own top-level names, for the shadowing check.
    let own_first: BTreeSet<String> = parsed
        .iter()
        .enumerate()
        // A suite's name is its target, not a unit it declares.
        .filter(|(k, pf)| {
            package_of[*k].is_none()
                && !pf.is_synthetic()
                && !matches!(pf.kind(), UnitKind::Test | UnitKind::Integration)
        })
        .filter_map(|(_, pf)| pf.unit().name().parts.first().map(|i| i.name.clone()))
        .collect();
    for alias in graph.root_aliases.keys() {
        if own_first.contains(alias) {
            errors.push_for(
                Some(manifest),
                CompileError::new(
                    ALIAS_SHADOWS_UNIT,
                    Span::default(),
                    format!(
                        "the dependency alias `{alias}` is also the first segment of one of this project's own units, so `{alias}.…` would be ambiguous"
                    ),
                )
                .with_note("rename the alias in `[dependencies]` (the key is the alias), or the unit"),
            );
        }
    }

    // Each dependency's declared units, by the name it declares them under.
    let mut declared: Vec<BTreeMap<String, UnitKind>> = vec![BTreeMap::new(); graph.packages.len()];
    for (k, pf) in parsed.iter().enumerate() {
        if let Some(i) = package_of[k]
            && keep[k]
        {
            declared[i].insert(pf.unit().name().joined(), pf.kind());
        }
    }

    // Rename each dependency unit to its canonical name.
    for (k, pf) in parsed.iter_mut().enumerate() {
        if let Some(i) = package_of[k] {
            let enc = graph.packages[i].id.encoded();
            prefix_name(unit_name_mut(pf.unit_mut()), &enc);
        }
    }

    // Resolve every target against its own file's package.
    for (k, pf) in parsed.iter_mut().enumerate() {
        if !keep[k] || pf.is_synthetic() {
            continue;
        }
        let file = pf.identity_path();
        let from = package_of[k];
        let in_suite = matches!(pf.kind(), UnitKind::Test | UnitKind::Integration);
        let mut found: Vec<CompileError> = Vec::new();
        let resolve = |target: &mut QualifiedName, found: &mut Vec<CompileError>| {
            resolve_target(&graph, &declared, from, in_suite, target, found)
        };
        let uses_pass = |uses: &mut Vec<bynk_syntax::ast::UsesDecl>,
                         found: &mut Vec<CompileError>| {
            uses.retain_mut(|u| resolve(&mut u.target, found) != Resolved::Dropped);
        };
        let consumes_pass = |consumes: &mut Vec<bynk_syntax::ast::ConsumesDecl>,
                             found: &mut Vec<CompileError>| {
            consumes.retain_mut(|cd| {
                let written = cd.target.clone();
                match resolve(&mut cd.target, found) {
                    Resolved::Dropped => false,
                    Resolved::Dependency => {
                        // A whole-unit `consumes` keeps the spelling
                        // source wrote as its alias, so `money.fx.Rates`
                        // still names the capability.
                        if cd.alias.is_none() && cd.selected.is_none() {
                            cd.alias = Some(Ident {
                                name: written.joined(),
                                span: written.span,
                            });
                        }
                        true
                    }
                    Resolved::Unchanged => true,
                }
            });
        };
        match pf.unit_mut() {
            SourceUnit::Commons(c) => uses_pass(&mut c.uses, &mut found),
            SourceUnit::Context(c) => {
                uses_pass(&mut c.uses, &mut found);
                consumes_pass(&mut c.consumes, &mut found);
            }
            SourceUnit::Adapter(a) => {
                uses_pass(&mut a.uses, &mut found);
                consumes_pass(&mut a.consumes, &mut found);
            }
            SourceUnit::Suite(s) => {
                resolve(&mut s.target, &mut found);
                uses_pass(&mut s.uses, &mut found);
            }
        }
        errors.extend_for(Some(&file), found);
    }

    // What a dependency may not yet carry.
    for (k, pf) in parsed.iter().enumerate() {
        let Some(i) = package_of[k] else { continue };
        if !keep[k] {
            continue;
        }
        let pkg = graph.packages[i].id.name();
        // #2255: a dependency adapter's npm `requires` fold into the build
        // with the project's own (`phase_group`), so only an `asset` is
        // refused here.
        for item in pf.items() {
            if let CommonsItem::Asset(a) = item {
                errors.push_for(
                    Some(&pf.identity_path()),
                    CompileError::new(
                        UNSUPPORTED_IN_DEPENDENCY,
                        a.span,
                        format!(
                            "dependency `{pkg}` declares an `asset`, and a dependency's assets aren't supported yet: it would resolve against the consuming project's `assets/`"
                        ),
                    ),
                );
            }
        }
    }

    // Keep only the dependency units a consumer reaches (P-5).
    let reached = reached_units(parsed, &keep, &package_of, tests);
    for (k, pf) in parsed.iter().enumerate() {
        if package_of[k].is_some() && !reached.contains(&pf.unit().name().joined()) {
            keep[k] = false;
        }
    }
    let mut k = 0;
    parsed.retain(|_| {
        let kept = keep[k];
        k += 1;
        kept
    });
}

/// What resolving one target did.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Resolved {
    /// Not a dependency reference; left as written.
    Unchanged,
    /// Now names a dependency unit by its canonical name.
    Dependency,
    /// Names nothing in the dependency; reported here, so the clause is
    /// dropped rather than reported again as an unknown unit.
    Dropped,
}

/// Resolve one `uses`/`consumes` target written in a file of package `from`
/// (`None`: the project itself), rewriting `target` in place. A unit the
/// dependency declares but doesn't export is reported and still resolved, so
/// checking carries on without a second error for the same clause.
fn resolve_target(
    graph: &PackageGraph,
    declared: &[BTreeMap<String, UnitKind>],
    from: Option<usize>,
    in_suite: bool,
    target: &mut QualifiedName,
    found: &mut Vec<CompileError>,
) -> Resolved {
    let Some(first) = target.parts.first().map(|p| p.name.clone()) else {
        return Resolved::Unchanged;
    };
    if first == "bynk" {
        return Resolved::Unchanged;
    }
    if from.is_none() && target.parts.len() >= 2 && graph.unresolved_aliases.contains(&first) {
        // Its package wasn't resolved, and that is already reported.
        return Resolved::Dropped;
    }
    let aliases = match from {
        None => &graph.root_aliases,
        Some(i) => &graph.packages[i].aliases,
    };
    if target.parts.len() >= 2
        && let Some(&dep) = aliases.get(&first)
    {
        let pkg = &graph.packages[dep];
        if from.is_none() && !in_suite && graph.dev_aliases.contains(&first) {
            found.push(
                CompileError::new(
                    DEV_DEPENDENCY_IN_SOURCE,
                    target.span,
                    format!(
                        "`{}` reaches dependency `{}` through `{first}`, which is a `[dev-dependencies]` alias: only a suite may use it",
                        target.joined(),
                        pkg.id.name()
                    ),
                )
                .with_note("move it to `[dependencies]` to use it here"),
            );
            return Resolved::Dropped;
        }
        let unit = target.parts[1..]
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(".");
        match declared[dep].get(&unit) {
            None => {
                found.push(
                    CompileError::new(
                        UNKNOWN_UNIT,
                        target.span,
                        format!(
                            "`{}` names no commons or adapter in dependency `{}` (alias `{first}`)",
                            target.joined(),
                            pkg.id.name()
                        ),
                    )
                    .with_note(exports_note(pkg, &declared[dep])),
                );
                return Resolved::Dropped;
            }
            Some(_) => {
                let exported = pkg.exports.as_ref().is_none_or(|list| list.contains(&unit));
                if !exported {
                    found.push(
                        CompileError::new(
                            UNIT_NOT_EXPORTED,
                            target.span,
                            format!("`{unit}` isn't exported by dependency `{}`", pkg.id.name()),
                        )
                        .with_note(exports_note(pkg, &declared[dep])),
                    );
                }
            }
        }
        prefix_name_from(target, &pkg.id, 1);
        return Resolved::Dependency;
    }
    if let Some(i) = from {
        // A dependency's reference to one of its own units.
        let joined = target.joined();
        if declared[i].contains_key(&joined) {
            prefix_name(target, &graph.packages[i].id.encoded());
            return Resolved::Dependency;
        }
    }
    Resolved::Unchanged
}

/// The note listing what a dependency shares.
fn exports_note(pkg: &bynk_project::Package, declared: &BTreeMap<String, UnitKind>) -> String {
    let shared: Vec<String> = match &pkg.exports {
        Some(list) => list.clone(),
        None => declared
            .iter()
            .filter(|(_, k)| matches!(k, UnitKind::Commons | UnitKind::Adapter))
            .map(|(n, _)| n.clone())
            .collect(),
    };
    if shared.is_empty() {
        format!("`{}` shares no units", pkg.id.name())
    } else {
        format!(
            "`{}` shares {}",
            pkg.id.name(),
            shared
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// The canonical names of every unit reachable from the project's own files
/// (and the first-party units), through `uses` and `consumes`.
fn reached_units(
    parsed: &[ParsedFile],
    keep: &[bool],
    package_of: &[Option<usize>],
    tests: bool,
) -> BTreeSet<String> {
    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut roots: BTreeSet<String> = BTreeSet::new();
    for (k, pf) in parsed.iter().enumerate() {
        if !keep[k] {
            continue;
        }
        let name = pf.unit().name().joined();
        let targets = edges.entry(name.clone()).or_default();
        for u in pf.uses() {
            targets.insert(u.target.joined());
        }
        for c in pf.consumes() {
            targets.insert(c.target.joined());
        }
        let suite = matches!(pf.kind(), UnitKind::Test | UnitKind::Integration);
        if package_of[k].is_none() && (tests || !suite) {
            roots.insert(name);
        }
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = roots.into_iter().collect();
    while let Some(n) = stack.pop() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(next) = edges.get(&n) {
            stack.extend(next.iter().cloned());
        }
    }
    seen
}

/// A unit declaration's name, mutably.
fn unit_name_mut(unit: &mut SourceUnit) -> &mut QualifiedName {
    match unit {
        SourceUnit::Commons(c) => &mut c.name,
        SourceUnit::Context(c) => &mut c.name,
        SourceUnit::Suite(s) => &mut s.target,
        SourceUnit::Adapter(a) => &mut a.name,
    }
}

/// Prepend the encoded package segment to `name`.
fn prefix_name(name: &mut QualifiedName, encoded: &str) {
    let span = name.parts.first().map_or(name.span, |p| p.span);
    name.parts.insert(
        0,
        Ident {
            name: encoded.to_owned(),
            span,
        },
    );
}

/// Replace `name`'s first `drop` segments (the alias) with `id`'s encoded
/// segment.
fn prefix_name_from(name: &mut QualifiedName, id: &PackageId, drop: usize) {
    let span = name.parts.first().map_or(name.span, |p| p.span);
    name.parts.drain(..drop);
    name.parts.insert(
        0,
        Ident {
            name: id.encoded(),
            span,
        },
    );
}
