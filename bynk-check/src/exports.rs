//! The shareable-unit set (packaging slice 1, #2207; track D1): a package's
//! `[package] exports` lists the units it shares, and each must be a commons
//! or an adapter the project declares. A context is never shareable — it is a
//! deployment unit, whose Worker, secrets and deploy state belong to the
//! project that deploys it.
//!
//! `bynk.toml`'s shape is `bynk-project`'s to check; whether each entry names
//! a shareable unit needs the project's units, so it is checked here, once
//! they are known, and attributed to the manifest like the `[unstable]`
//! diagnostics ([`crate::stability::MANIFEST`]). The build and the editor read
//! `[package]` the same way ([`bynk_project::read_package_config`]).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use bynk_project::UnitKind;
use bynk_syntax::CompileError;
use bynk_syntax::span::Span;

/// The error for an `exports` entry naming a context.
pub const EXPORT_CONTEXT: &str = "bynk.project.export_context";
/// The error for an `exports` entry naming no unit the project declares.
pub const EXPORT_UNKNOWN: &str = "bynk.project.export_unknown";

/// The `[package] exports` diagnostics for the project at `project_root`,
/// against its units' `kinds` (as `phase_group` builds them). No project root,
/// no `[package]`, or no `exports` list is nothing to check.
pub fn manifest_diagnostics(
    project_root: Option<&Path>,
    overlay: &HashMap<PathBuf, String>,
    kinds: &BTreeMap<String, UnitKind>,
) -> Vec<CompileError> {
    let exports = project_root
        .and_then(|root| bynk_project::read_package_config(root, overlay))
        .and_then(|package| package.exports)
        .unwrap_or_default();
    check_exports(&exports, kinds)
}

/// The diagnostics for `exports` against `kinds`. A first-party unit is never
/// the project's own, so it counts as undeclared.
pub fn check_exports(exports: &[String], kinds: &BTreeMap<String, UnitKind>) -> Vec<CompileError> {
    let mut out = Vec::new();
    for entry in exports {
        let kind = kinds
            .get(entry)
            .filter(|_| crate::firstparty::unit(entry).is_none());
        match kind {
            Some(UnitKind::Commons | UnitKind::Adapter) => {}
            Some(UnitKind::Context) => out.push(
                CompileError::new(
                    EXPORT_CONTEXT,
                    Span::default(),
                    format!(
                        "`bynk.toml`'s `[package] exports` lists `{entry}`, which is a context — a package shares commons and adapters, never a context"
                    ),
                )
                .with_note(
                    "a context is a deployment unit, owned by the project that deploys it; remove it from `exports`",
                ),
            ),
            _ => out.push(
                CompileError::new(
                    EXPORT_UNKNOWN,
                    Span::default(),
                    format!(
                        "`bynk.toml`'s `[package] exports` lists `{entry}`, which isn't a commons or adapter this project declares"
                    ),
                )
                .with_note(match did_you_mean(entry, kinds) {
                    Some(name) => format!("did you mean `{name}`?"),
                    None => "an entry names a commons or adapter declared in this project's sources"
                        .to_string(),
                }),
            ),
        }
    }
    out
}

/// The shareable unit nearest `entry`, within two edits.
fn did_you_mean<'a>(entry: &str, kinds: &'a BTreeMap<String, UnitKind>) -> Option<&'a str> {
    kinds
        .iter()
        .filter(|(name, kind)| {
            matches!(kind, UnitKind::Commons | UnitKind::Adapter)
                && crate::firstparty::unit(name).is_none()
        })
        .map(|(name, _)| (strsim(entry, name), name.as_str()))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, name)| name)
}

/// Levenshtein distance over chars.
fn strsim(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds() -> BTreeMap<String, UnitKind> {
        BTreeMap::from([
            ("money".to_string(), UnitKind::Commons),
            ("payments".to_string(), UnitKind::Adapter),
            ("shop.checkout".to_string(), UnitKind::Context),
            ("bynk".to_string(), UnitKind::Commons),
        ])
    }

    fn codes(exports: &[&str]) -> Vec<&'static str> {
        let exports: Vec<String> = exports.iter().map(|s| s.to_string()).collect();
        check_exports(&exports, &kinds())
            .iter()
            .map(|e| {
                if e.category == EXPORT_CONTEXT {
                    EXPORT_CONTEXT
                } else {
                    EXPORT_UNKNOWN
                }
            })
            .collect()
    }

    #[test]
    fn commons_and_adapters_are_shareable() {
        assert!(codes(&["money", "payments"]).is_empty());
    }

    #[test]
    fn a_context_is_never_shareable() {
        assert_eq!(codes(&["shop.checkout"]), [EXPORT_CONTEXT]);
    }

    #[test]
    fn an_undeclared_or_first_party_unit_is_unknown() {
        assert_eq!(codes(&["mony", "bynk"]), [EXPORT_UNKNOWN, EXPORT_UNKNOWN]);
        let notes = &check_exports(&["mony".to_string()], &kinds())[0].notes;
        assert_eq!(notes, &["did you mean `money`?".to_string()]);
    }
}
