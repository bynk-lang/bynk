//! #3941 decision H: what each `env(…)` constant resolves to, and where the
//! value came from, for the deploy plan. A second, read-only pass over the
//! project's sources, run only after the build has succeeded: the build itself
//! substitutes the literals and keeps no record of where they came from.

use std::collections::HashMap;

use bynk_check::project_model;
use bynk_project::vars::ResolvedConst;

use super::diagnostics::ErrorSink;
use super::entry::project_trees;
use super::options::CompileOptions;

/// Every `env(…)` constant of the project, resolved for `options.environment`.
/// Empty when the sources cannot be read: the build reports that.
pub fn env_const_report(options: &CompileOptions) -> Vec<ResolvedConst> {
    let trees = project_trees(options);
    let excludes = options.roots.excludes();
    let empty = HashMap::new();
    let overlay = options.sources.as_ref().unwrap_or(&empty);
    let mut errors = ErrorSink::new();
    let mut snapshots = Vec::new();
    let Ok(file_lists) = project_model::phase_discovery(&trees, &excludes, &mut errors) else {
        return Vec::new();
    };
    let Ok((mut parsed, _, _)) =
        project_model::phase_parse(&trees, &file_lists, overlay, &mut errors, &mut snapshots)
    else {
        return Vec::new();
    };
    let root = options.roots.project_root();
    let vars = bynk_project::vars::try_read_vars_with(root, overlay).unwrap_or_default();
    bynk_check::env_consts::resolve(&mut parsed, &vars, &options.environment).resolved
}
