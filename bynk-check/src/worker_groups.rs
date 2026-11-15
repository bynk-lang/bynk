//! Worker groups against the project's units (#3562, slice G1).
//!
//! `bynk.toml`'s `[deploy.workers.<key>]` shape, and what needs no project
//! model, is `bynk-project`'s to check ([`bynk_project::worker_groups`]).
//! Whether each member is a context the project declares, and whether a
//! group's key or name collides with the Worker of a context in no group,
//! needs the project's units, so it is checked here once they are known, and
//! attributed to the manifest like the `[package] exports` diagnostics
//! ([`crate::exports`]). The build and the editor read the table the same way.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use bynk_project::UnitKind;
use bynk_project::worker_groups::{
    ProjectUnits, WorkerGroups, try_read_worker_groups_with, validate_against,
};
use bynk_syntax::CompileError;
use bynk_syntax::span::Span;

/// The `[deploy.workers]` diagnostics for the project at `project_root`,
/// against its units' `kinds` (as `phase_group` builds them). No project root,
/// no table, or a malformed one (its shape is reported with the rest of the
/// manifest) is nothing to check here.
pub fn manifest_diagnostics(
    project_root: Option<&Path>,
    overlay: &HashMap<PathBuf, String>,
    kinds: &BTreeMap<String, UnitKind>,
) -> Vec<CompileError> {
    let groups = project_root
        .and_then(|root| try_read_worker_groups_with(root, overlay).ok())
        .unwrap_or_default();
    check_groups(&groups, kinds)
}

/// The diagnostics for `groups` against `kinds`. A first-party unit is never
/// the project's own, so it counts as undeclared.
pub fn check_groups(
    groups: &WorkerGroups,
    kinds: &BTreeMap<String, UnitKind>,
) -> Vec<CompileError> {
    if groups.is_empty() {
        return Vec::new();
    }
    let of_kind = |wanted: fn(&UnitKind) -> bool| -> BTreeSet<String> {
        kinds
            .iter()
            .filter(|(name, kind)| wanted(kind) && crate::firstparty::unit(name).is_none())
            .map(|(name, _)| name.clone())
            .collect()
    };
    let contexts = of_kind(|k| matches!(k, UnitKind::Context));
    let other_units = of_kind(|k| !matches!(k, UnitKind::Context));
    validate_against(
        groups,
        &ProjectUnits {
            contexts: &contexts,
            other_units: &other_units,
        },
    )
    .into_iter()
    .map(|issue| CompileError::new(issue.code, Span::default(), issue.message))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds() -> BTreeMap<String, UnitKind> {
        BTreeMap::from([
            ("money".to_string(), UnitKind::Commons),
            ("shop.orders".to_string(), UnitKind::Context),
            ("shop.pricing".to_string(), UnitKind::Context),
            ("shop.payment".to_string(), UnitKind::Context),
        ])
    }

    fn codes(manifest: &str) -> Vec<&'static str> {
        let root = Path::new("/project");
        let overlay = HashMap::from([(root.join("bynk.toml"), manifest.to_string())]);
        manifest_diagnostics(Some(root), &overlay, &kinds())
            .iter()
            .map(|e| e.category)
            .collect()
    }

    #[test]
    fn contexts_are_groupable() {
        assert!(
            codes("[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.pricing\"]\n")
                .is_empty()
        );
    }

    #[test]
    fn a_commons_is_not_a_context() {
        assert_eq!(
            codes("[deploy.workers.core]\ncontexts = [\"money\"]\n"),
            ["bynk.deploy.not_a_context"]
        );
    }

    #[test]
    fn an_undeclared_or_first_party_unit_is_unknown() {
        assert_eq!(
            codes("[deploy.workers.core]\ncontexts = [\"shop.ordres\", \"bynk.cloudflare\"]\n"),
            ["bynk.deploy.unknown_context", "bynk.deploy.unknown_context"]
        );
    }

    #[test]
    fn a_key_may_not_be_an_ungrouped_contexts_worker() {
        assert_eq!(
            codes("[deploy.workers.shop-payment]\ncontexts = [\"shop.orders\"]\n"),
            ["bynk.deploy.group_name_conflict"]
        );
    }
}
