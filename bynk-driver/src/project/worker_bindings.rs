//! The driver's half of the Worker bindings: the build's checks and the paths
//! that need the filesystem.
//!
//! The bindings themselves are `bynk-emit`'s ([`bynk_emit::emitter::WorkerBindings`],
//! derived once per build in `run_checks`). The duplicate check
//! (`bynk.context.duplicate_binding`) is
//! [`bynk_check::worker_bindings::phase_duplicate_bindings`], and it reads that
//! model. What stays here is the Bundle's env threading, the check's
//! `[deploy.workers]` input, and the container paths, which canonicalise.

use std::collections::{BTreeMap, HashMap};

use bynk_check::firstparty::Platform;
use bynk_emit::emitter::bindings::WorkerBinding;
use bynk_emit::emitter::native_platforms_of_context;

use super::diagnostics::ErrorSink;
use bynk_project::discovery::ParsedFile;

use super::{BuildTarget, UnitKind, UnitTable};

/// v0.19 (decision 0025): each context whose in-process closure reaches a
/// platform-native unit, with those platforms. Drives the Bundle's env
/// threading.
pub(super) fn contexts_native(
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
) -> HashMap<String, BTreeMap<Platform, String>> {
    kinds
        .iter()
        .filter(|(_, k)| **k == UnitKind::Context)
        .filter_map(|(name, _)| {
            let table = unit_tables.get(name)?;
            let native = native_platforms_of_context(
                name,
                table,
                unit_tables,
                unit_consumes,
                unit_consumes_aliases,
                unit_flattened,
                true,
            );
            (!native.is_empty()).then(|| (name.clone(), native))
        })
        .collect()
}

/// `bynk.context.duplicate_binding`: on a Workers build, each context's
/// bindings (the model's, [`bynk_emit::emitter::WorkerBindings::by_context`]), checked by
/// [`bynk_check::worker_bindings::phase_duplicate_bindings`].
///
/// #3562: and the members of each `[deploy.workers]` group declare each binding
/// name once, or declare it identically
/// ([`bynk_check::worker_bindings::phase_group_binding_collisions`]).
pub(super) fn phase_duplicate_bindings(
    target: BuildTarget,
    project: (&std::path::Path, &HashMap<std::path::PathBuf, String>),
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    // `None` while the program has errors: the plan, and so the model, walks the provider
    // graph, whose acyclicity the earlier checks establish.
    bindings: Option<&BTreeMap<String, Vec<WorkerBinding>>>,
    errors: &mut ErrorSink,
) {
    let Some(bindings) = bindings.filter(|_| errors.is_empty()) else {
        return;
    };
    if target != BuildTarget::Workers {
        return;
    }
    bynk_check::worker_bindings::phase_duplicate_bindings(parsed, groups, bindings, errors);
    let worker_groups =
        bynk_project::worker_groups::try_read_worker_groups_with(project.0, project.1)
            .unwrap_or_default();
    bynk_check::worker_bindings::phase_group_binding_collisions(
        &worker_groups,
        parsed,
        groups,
        bindings,
        errors,
    );
}

/// #3477: the path back to the project root from a Worker's output directory,
/// `<out>/workers/<worker>`: one `..` for each part of the output directory
/// below the project and two more (`../../..` for the default `out`). The
/// output directory is the one the build is written to (`out_dir`), else the
/// manifest's `[build] out` (default `out`). `Err` holds the resolved output
/// directory and project when the output directory is not inside the project.
pub(super) fn container_root(
    project_root: &std::path::Path,
    out_dir: Option<&std::path::Path>,
) -> Result<String, (std::path::PathBuf, std::path::PathBuf)> {
    let project = resolved(project_root);
    let out = match out_dir {
        Some(out) => resolved(out),
        None => {
            let out = bynk_project::try_read_build_config(project_root)
                .ok()
                .and_then(|c| c.out)
                .unwrap_or_else(|| "out".to_string());
            resolved(&project_root.join(out))
        }
    };
    let Ok(below) = out.strip_prefix(&project) else {
        return Err((out, project));
    };
    let depth = below
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .count();
    Ok(vec![".."; depth + 2].join("/"))
}

/// `path` made absolute and lexically normalised, with symlinks resolved as far
/// as it exists (a not-yet-created output directory is placed by its parent).
fn resolved(path: &std::path::Path) -> std::path::PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut normal = std::path::PathBuf::new();
    for component in abs.components() {
        match component {
            std::path::Component::ParentDir => {
                normal.pop();
            }
            std::path::Component::CurDir => {}
            other => normal.push(other),
        }
    }
    let mut rest = Vec::new();
    let mut base = normal.as_path();
    loop {
        if let Ok(real) = base.canonicalize() {
            return rest.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                base = parent;
            }
            _ => return normal,
        }
    }
}

/// `bynk.container.output_outside_project`: one error for each container of a
/// build whose output directory is not inside the project, naming the container
/// and both paths.
pub(super) fn container_output_errors(
    unit_tables: &HashMap<String, UnitTable>,
    outside: &(std::path::PathBuf, std::path::PathBuf),
) -> Vec<bynk_project::AttributedError> {
    let (out, project) = outside;
    let mut names: Vec<(&String, &String)> = unit_tables
        .iter()
        .flat_map(|(unit, table)| {
            table
                .derived
                .iter()
                .filter(|(_, d)| d.is_container())
                .map(move |(capability, _)| (unit, capability))
        })
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|(unit, capability)| {
            let mut error = bynk_syntax::CompileError::new(
                "bynk.container.output_outside_project",
                bynk_syntax::span::Span::default(),
                format!(
                    "the container `{unit}.{capability}` is built from a directory of the project `{}`, but the output directory `{}` is outside it",
                    project.display(),
                    out.display()
                ),
            );
            error.notes.push(
                "`wrangler` reaches the container's Dockerfile from the Worker's output directory by a relative path, which needs the output directory to be inside the project: build into one that is, for example `bynk build --out out`, or set `[build] out`".to_string(),
            );
            bynk_project::AttributedError::new(None, error)
        })
        .collect()
}
