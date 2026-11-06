//! The project-level strip: rewrite a compiled [`ProjectOutput`] from TypeScript into a
//! JavaScript artefact (the in-browser track's first-class JS output, ADR 0137).
//!
//! These functions walk a `ProjectOutput`, so they live with the rest of the orchestration
//! in `bynk-driver` (crate-hygiene D1, #2982, PR 4); `bynk-strip` keeps the text-level
//! strip (`strip_types`, `strip_types_mapped`) on `bynk-ts` and `oxc` alone, so it builds
//! alongside the checker instead of after code generation.

use std::path::PathBuf;

use bynk_emit::emit_model::{Artefacts, Document, ProjectOutput, sibling_path};
pub use bynk_strip::StripError;

/// Rewrite a compiled [`ProjectOutput`] from
/// TypeScript into a JavaScript artefact (the in-browser track's first-class JS
/// output — ADR 0137). The emitter always produces TypeScript; a JS artefact is
/// that same output with types stripped, which is total because the emitter is
/// strip-only (ADR 0136).
///
/// Dispatches on [`Document`] directly (P7.6,
/// #1309) — no `CompiledFile` text intermediate. Every `Ts` document is
/// printed through the one printer that owns a character (R7.3), then
/// type-stripped and renamed to `.js` as its own `Document::Js`; `tsconfig.json`
/// is dropped (a TypeScript-compiler config with no role for a JS artefact);
/// `wrangler.toml`'s real tree gets its `main` entry patched *structurally*,
/// in place — no print-then-reparse (Decision E) — since the Workers
/// manifest names its entry module and a `main` left pointing at the
/// stripped `.ts` breaks `wrangler dev`/deploy on the emitted output. Source
/// maps and the debug sidecar are dropped — they map into the `.ts` the JS
/// replaces. Import specifiers are already `.js` (the default `ImportExt`),
/// so the renamed tree resolves as-is.
pub fn strip_project_to_js(out: ProjectOutput) -> Result<ProjectOutput, StripError> {
    strip_project(out, None).map(|(out, _)| out)
}

/// The per-`.js`-path Bynk-to-JavaScript source maps of a stripped project.
pub type JsMaps = std::collections::BTreeMap<PathBuf, String>;

/// [`strip_project_to_js`] that also returns, per emitted `.js` path, a
/// Bynk-to-JavaScript v3 source map composed from the emitter's own
/// Bynk-to-TypeScript map and the strip's TypeScript-to-JavaScript map (#2705).
/// Only the modules in `only` (JS output paths; the host passes the user's own)
/// are mapped, so the rest skip the work. The returned `ProjectOutput` is identical to
/// [`strip_project_to_js`]'s: the maps are not added as documents.
pub fn strip_project_to_js_mapped(
    out: ProjectOutput,
    only: &std::collections::HashSet<PathBuf>,
) -> Result<(ProjectOutput, JsMaps), StripError> {
    strip_project(out, Some(only))
}

fn strip_project(
    out: ProjectOutput,
    only: Option<&std::collections::HashSet<PathBuf>>,
) -> Result<(ProjectOutput, JsMaps), StripError> {
    use std::collections::BTreeMap;

    let bynk_maps: BTreeMap<PathBuf, String> = if only.is_some() {
        out.artefacts
            .docs
            .iter()
            .filter_map(|(p, d)| match d {
                Document::SourceMap(s) => Some((p.clone(), s.clone())),
                _ => None,
            })
            .collect()
    } else {
        BTreeMap::new()
    };
    let mut js_maps: JsMaps = BTreeMap::new();
    let mut docs: BTreeMap<PathBuf, Document> = BTreeMap::new();
    for (path, doc) in out.artefacts.docs {
        match doc {
            Document::Ts(program) => {
                let printed = bynk_ts::print(&program, "", "", &path.to_string_lossy());
                let js_path = path.with_extension("js");
                let want_map = only.is_some_and(|o| o.contains(&js_path));
                let bynk_map = if want_map {
                    bynk_maps
                        .get(&sibling_path(&path, "map"))
                        .map(String::as_str)
                } else {
                    None
                };
                let (js, map) = bynk_strip::strip_types_mapped(
                    &printed.text,
                    &path.to_string_lossy(),
                    bynk_map,
                )?;
                if let Some(map) = map {
                    js_maps.insert(js_path.clone(), map);
                }
                docs.insert(js_path, Document::Js(js));
            }
            Document::Toml(mut toml) => {
                if path.file_name().and_then(|n| n.to_str()) == Some("wrangler.toml")
                    && !toml.set_main("index.js")
                {
                    return Err(StripError {
                        filename: path.to_string_lossy().into_owned(),
                        message: "wrangler.toml has no root `main` key".to_string(),
                    });
                }
                docs.insert(path, Document::Toml(toml));
            }
            Document::Json(s) => {
                if path.file_name().and_then(|n| n.to_str()) == Some("tsconfig.json") {
                    continue;
                }
                docs.insert(path, Document::Json(s));
            }
            // Source maps/debug sidecars map into the `.ts` the JS replaces.
            Document::SourceMap(_) | Document::DebugSidecar(_) => {}
            // `bynk-emit` never produces this itself, but `strip_project_to_js`
            // is `pub` over a `pub` `Artefacts`, so an already-stripped
            // `Document::Js` reaching this function (e.g. run twice, or a
            // caller assembling `Artefacts` by hand) is a real, reachable
            // input — pass it through rather than panicking on it (review,
            // #1309/#1310: `unreachable!` here aborted the process on input
            // the public API permits).
            Document::Js(s) => {
                docs.insert(path, Document::Js(s));
            }
            // P4b (#2608): a migration is SQL, not code.
            Document::Sql(s) => {
                docs.insert(path, Document::Sql(s));
            }
        }
    }
    // #1883, slice 0: static assets are served as-is by the platform; type
    // stripping has nothing to do to them.
    Ok((
        ProjectOutput {
            artefacts: Artefacts {
                docs,
                assets: out.artefacts.assets,
                modules: out.artefacts.modules,
            },
            ..out
        },
        js_maps,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P7.4 (#1305)/P7.6 (#1309): `strip_project_to_js`'s `wrangler.toml`
    /// patch, exercised through the real function against a *real* compiled
    /// `Document::Toml` tree (`compile_in_memory`, `BuildTarget::Workers`) —
    /// not a hand-typed text fixture standing in for one, which is exactly
    /// the shortcut P7.4's own defect report warns against (a hand-built
    ///2-key snippet can't prove a whole-document reformat didn't happen).
    /// P7.6's own Decision E moved this from a text re-parse
    /// (`toml_edit`) to a structural, in-tree mutation
    /// (`TomlDocument::set_main`) — asserting the *printed* text still
    /// pins "nothing but `main` changed", the same discipline as before.
    #[test]
    fn strip_project_to_js_patches_wrangler_toml_main_structurally() {
        use crate::project::{BuildTarget, Document, compile_in_memory};

        let out = compile_in_memory(
            "context api {\n  service routes from http {\n    on GET(\"/\") () -> Effect[HttpResult[String]] by v: Visitor {\n      Ok(\"ok\")\n    }\n  }\n}\n",
            BuildTarget::Workers,
            Default::default(),
        )
        .unwrap_or_else(|f| {
            panic!(
                "workers-target fixture should compile: {:?}",
                f.flatten()
            )
        });
        let wrangler_path = out
            .artefacts
            .docs
            .keys()
            .find(|p| p.file_name().and_then(|n| n.to_str()) == Some("wrangler.toml"))
            .cloned()
            .expect("a Workers-target compile emits a wrangler.toml");
        let original_text = out.artefacts.docs[&wrangler_path].text();
        assert!(
            original_text.contains("main = \"index.ts\""),
            "expected the pre-strip manifest to point at the .ts entry:\n{original_text}"
        );

        let stripped = strip_project_to_js(out).expect("wrangler.toml patches cleanly");
        let patched = stripped
            .artefacts
            .docs
            .get(&wrangler_path)
            .unwrap_or_else(|| panic!("{} should survive stripping", wrangler_path.display()));
        assert!(
            matches!(patched, Document::Toml(_)),
            "wrangler.toml should stay a real tree through stripping, not become opaque text"
        );
        let expected = original_text.replacen("main = \"index.ts\"", "main = \"index.js\"", 1);
        assert_eq!(
            patched.text(),
            expected,
            "only `main` should differ from the pre-strip manifest"
        );
        // No leftover `.ts` index — `Document::Ts` entries are all renamed
        // to their own `.js` sibling.
        let index_ts = wrangler_path.with_file_name("index.ts");
        assert!(
            !stripped.artefacts.docs.contains_key(&index_ts),
            "the stripped output must not keep the pre-strip .ts path: {:?}",
            stripped.artefacts.docs.keys().collect::<Vec<_>>()
        );
    }

    /// #1883, slice 0: static assets are served as-is, so stripping a
    /// project's types must hand `Artefacts.assets` through byte-for-byte —
    /// `bynkc compile --emit js` strips before it writes, and an asset dropped
    /// here would be missing from that output. One is added to a real
    /// build's output by hand, so this tests stripping alone.
    #[test]
    fn strip_project_to_js_passes_static_assets_through_unchanged() {
        use crate::project::{BuildTarget, compile_in_memory, worker_assets_dir};

        let mut out = compile_in_memory(
            "commons only\n\nfn f() -> Int {\n  1\n}\n",
            BuildTarget::Workers,
            Default::default(),
        )
        .unwrap_or_else(|f| panic!("fixture should compile: {:?}", f.flatten()));
        let path = worker_assets_dir("only").join("_assets/icon.9b1d4e.png");
        let bytes = vec![0x89, b'P', b'N', b'G', 0xff, 0x00];
        out.artefacts.assets.insert(path.clone(), bytes.clone());

        let stripped = strip_project_to_js(out).expect("strips cleanly");
        assert_eq!(stripped.artefacts.assets.len(), 1);
        assert_eq!(stripped.artefacts.assets.get(&path), Some(&bytes));
    }
}
