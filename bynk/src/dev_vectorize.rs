//! #3479: `bynk dev` binds each `Vectorize` instance to an index of the
//! developer's own.
//!
//! Vectorize has no local simulation, so under `bynk dev` a call reaches a
//! real index (`remote = true`). Which one is the developer's to say, per
//! capability, and never the deploy's derived name, so a development session
//! cannot write to a deployed index by accident:
//!
//! ```toml
//! [dev.vectorize-index]
//! Docs = "docs-dev"
//! ```
//!
//! A capability with no entry is unbound: its stanza is left out of the
//! development configuration, and a call to it faults saying how to bind it.
//! `bynk dev` never creates the index (it never provisions), and the binding
//! checks the index against the declaration on its first call. A Worker run
//! with `--remote` keeps the account's deployed names, so none of this applies.

use std::collections::BTreeMap;
use std::path::Path;

/// What binding the development configurations did, for the one note a
/// session prints.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Bound {
    /// `(capability as bynk.toml writes it, index)` per bound instance.
    pub(crate) bound: Vec<(String, String)>,
    /// The capabilities (as `bynk.toml` writes them) left unbound.
    pub(crate) unbound: Vec<String>,
}

/// The capability name a binding stands for as `bynk.toml` writes it
/// (`VECTORIZE_MY_DOCS` is `MyDocs`): it always matches back to the same binding.
pub(crate) fn capability_of(binding: &str) -> String {
    let prefix = format!("{}_", bynk_check::firstparty::VECTORIZE_BINDING_PREFIX);
    binding
        .strip_prefix(&prefix)
        .unwrap_or(binding)
        .split('_')
        .map(|word| {
            let lower = word.to_ascii_lowercase();
            let mut chars = lower.chars();
            chars
                .next()
                .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

/// Rewrite each served Worker's generated `wrangler.toml` so its `Vectorize`
/// instances are bound to the indexes `[dev.vectorize-index]` names. `all` is
/// every Worker of the build and `serving` those being served. An entry that
/// names no `Vectorize` capability of the project is an error: it would
/// otherwise be ignored, leaving the instance unbound with no hint why.
pub(crate) fn bind_for_dev(
    project_root: &Path,
    workers_dir: &Path,
    all: &[String],
    serving: &[String],
) -> Result<Bound, String> {
    let resources = crate::deploy::project_resources(workers_dir, all)?;
    if resources.values().all(|r| r.vectorize_indexes.is_empty()) {
        return Ok(Bound::default());
    }
    let settings =
        bynk_project::vectorize::try_read_vectorize_dev(project_root).map_err(|e| e.to_string())?;
    let decls: Vec<&crate::deploy::VectorizeDecl> = resources
        .values()
        .flat_map(|r| r.vectorize_indexes.iter())
        .collect();
    for capability in settings.keys() {
        if !decls.iter().any(|d| d.matches_capability(capability)) {
            let wanted = bynk_check::worker_bindings::instance_binding_name(
                bynk_check::firstparty::VECTORIZE_BINDING_PREFIX,
                capability,
            );
            let hint = bynk_project::paths::did_you_mean(
                &wanted,
                decls.iter().map(|d| d.binding.as_str()),
            )
            .map(|b| format!("; did you mean `{}`?", capability_of(b)))
            .unwrap_or_default();
            return Err(format!(
                "`[dev.vectorize-index] {capability}` names no `Vectorize` capability in this project{hint}"
            ));
        }
    }
    let mut report = Bound::default();
    for worker in serving {
        let Some(declared) = resources.get(worker) else {
            continue;
        };
        if declared.vectorize_indexes.is_empty() {
            continue;
        }
        let mut dev = BTreeMap::new();
        let mut checks = BTreeMap::new();
        for decl in &declared.vectorize_indexes {
            let named = settings
                .iter()
                .find(|(capability, _)| decl.matches_capability(capability));
            match named {
                Some((capability, index)) => {
                    dev.insert(decl.binding.clone(), index.clone());
                    checks.insert(
                        decl.binding.clone(),
                        format!("{}:{}", decl.dimensions, decl.metric),
                    );
                    report.bound.push((capability.clone(), index.clone()));
                }
                None => report.unbound.push(capability_of(&decl.binding)),
            }
        }
        let path = workers_dir.join(worker).join("wrangler.toml");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read `{worker}`'s generated configuration: {e}"))?;
        let patched = bynk_emit::emitter::wrangler::bind_vectorize_for_dev(&text, &dev, &checks)
            .map_err(|e| {
                format!("could not bind `{worker}`'s Vectorize indexes for development: {e}")
            })?;
        if patched != text {
            std::fs::write(&path, patched).map_err(|e| {
                format!("could not write `{worker}`'s development configuration: {e}")
            })?;
        }
    }
    Ok(report)
}

/// The note `bynk dev` prints once, or `None` when there was nothing to bind.
pub(crate) fn report_note(bound: &Bound) -> Option<String> {
    let mut lines = Vec::new();
    for (capability, index) in &bound.bound {
        lines.push(format!(
            "bynk dev: `{capability}` is bound to your index `{index}` (remote; it must exist with the declaration's dimensions and metric)"
        ));
    }
    for capability in &bound.unbound {
        lines.push(format!(
            "bynk dev: `{capability}` has no development index, so calls to it fail until you set `[dev.vectorize-index] {capability} = \"<index-name>\"` in bynk.toml"
        ));
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binding_names_its_capability_as_bynk_toml_writes_it() {
        assert_eq!(capability_of("VECTORIZE_DOCS"), "Docs");
        assert_eq!(capability_of("VECTORIZE_MY_DOCS"), "MyDocs");
        // It always matches back to the same binding.
        for binding in ["VECTORIZE_DOCS", "VECTORIZE_MY_DOCS"] {
            assert_eq!(
                bynk_check::worker_bindings::instance_binding_name(
                    bynk_check::firstparty::VECTORIZE_BINDING_PREFIX,
                    &capability_of(binding)
                ),
                binding
            );
        }
    }

    #[test]
    fn the_note_names_what_is_bound_and_what_is_not() {
        let bound = Bound {
            bound: vec![("Docs".into(), "docs-dev".into())],
            unbound: vec!["Notes".into()],
        };
        let note = report_note(&bound).unwrap();
        assert!(
            note.contains("`Docs` is bound to your index `docs-dev`"),
            "{note}"
        );
        assert!(
            note.contains("`[dev.vectorize-index] Notes = \"<index-name>\"`"),
            "{note}"
        );
        assert_eq!(report_note(&Bound::default()), None);
    }

    fn project(bynk_toml: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "bynk-dev-vz-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let workers = root.join("out").join("workers");
        let dir = workers.join("shop");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(root.join("bynk.toml"), bynk_toml).unwrap();
        std::fs::write(
            dir.join("wrangler.toml"),
            "name = \"shop\"\n\n[[vectorize]]\nbinding = \"VECTORIZE_DOCS\"\nindex_name = \"shop-docs\"\n\n[[vectorize]]\nbinding = \"VECTORIZE_NOTES\"\nindex_name = \"shop-notes\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(bynk_emit::emitter::wrangler::VECTORIZE_MANIFEST),
            "{ \"version\": 1, \"vectorize\": [{ \"binding\": \"VECTORIZE_DOCS\", \"dimensions\": 768, \"metric\": \"cosine\", \"indexed\": [] }, { \"binding\": \"VECTORIZE_NOTES\", \"dimensions\": 32, \"metric\": \"euclidean\", \"indexed\": [] }] }",
        )
        .unwrap();
        (root, workers)
    }

    #[test]
    fn a_named_index_is_bound_remotely_and_an_unnamed_one_is_left_out() {
        let (root, workers) = project(
            "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n\n[dev.vectorize-index]\nDocs = \"docs-dev\"\n",
        );
        let all = vec!["shop".to_string()];
        let bound = bind_for_dev(&root, &workers, &all, &all).unwrap();
        assert_eq!(bound.bound, [("Docs".to_string(), "docs-dev".to_string())]);
        assert_eq!(bound.unbound, ["Notes".to_string()]);
        let config = std::fs::read_to_string(workers.join("shop").join("wrangler.toml")).unwrap();
        assert!(
            config.contains("index_name = \"docs-dev\"") && config.contains("remote = true"),
            "{config}"
        );
        assert!(
            config.contains("BYNK_DEV_INDEX_VECTORIZE_DOCS = \"768:cosine\""),
            "{config}"
        );
        assert!(
            !config.contains("VECTORIZE_NOTES") && !config.contains("shop-docs"),
            "{config}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_entry_naming_no_capability_is_refused_with_the_nearest() {
        let (root, workers) = project(
            "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n\n[dev.vectorize-index]\nDoc = \"docs-dev\"\n",
        );
        let all = vec!["shop".to_string()];
        let err = bind_for_dev(&root, &workers, &all, &all).unwrap_err();
        assert!(
            err.contains("`[dev.vectorize-index] Doc` names no `Vectorize` capability")
                && err.contains("did you mean `Docs`?"),
            "{err}"
        );
        // Nothing was rewritten.
        let config = std::fs::read_to_string(workers.join("shop").join("wrangler.toml")).unwrap();
        assert!(config.contains("shop-docs"), "{config}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
