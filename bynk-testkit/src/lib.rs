//! Content-ownership track (#1086) slice 3: the cross-crate replacement for
//! `diagnose_project(&root, &HashMap::new())` and bare
//! `CompileOptions::single`/`::split`'s reliance on `bynk-emit`'s disk
//! fallback (`design/tracks/content-ownership.md` §3.3, §4).
//!
//! Every helper here walks a project exactly the way production code already
//! does — `bynk_ide::discover_files` for `diagnose_project*`-style callers,
//! the same `bynk_emit::project::Roots` a `CompileOptions` will itself
//! compile for `compile_options_*` — and reads every file into a complete
//! sources map, instead of reimplementing the walk. There is no second
//! resolution to drift from the first: a test built on these helpers cannot
//! silently miss a file because this crate's notion of "the project's files"
//! diverged from the compiler's own.
//!
//! Dev-only: invisible to `fs_below_driver`'s probe
//! (`design/greenfield-status.md`), which is scoped to the crates below the
//! driver (`bynk-emit`, `bynk-ide`, `bynk-fmt`) — not to dev-only crates like
//! this one, which is why this file's own real `std::fs` reads (`read_all`,
//! the `bynk.toml` read below) never register, not because the probe walks
//! only production code.

use std::collections::HashMap;
use std::path::PathBuf;

/// Keyed by the literal discovered path, **not** canonicalised —
/// `bynk-driver`'s own `sources_for_roots`/`read_bynk_tree` (the proven,
/// production `CompileOptions.sources` populator, #1077/#1081) key theirs
/// the same way. `CompileOptions.sources`'s own doc says filesystem
/// discovery is skipped entirely once `sources` is `Some`, so whatever shape
/// a file's identity/consistency checks expect has to come from these keys
/// directly — canonicalising here (found the hard way: it broke
/// `bynk.project.inconsistent_commons_name`'s path-shape check against a
/// real multi-root example) would hand the compiler a different path shape
/// than `discover_bynk_files`'s own walk produces.
fn read_all(paths: Vec<PathBuf>) -> HashMap<PathBuf, String> {
    paths
        .into_iter()
        .filter_map(|p| {
            let content = std::fs::read_to_string(&p).ok()?;
            Some((p, content))
        })
        .collect()
}

/// A complete `(path, content)` map for `roots`, resolved and enumerated the
/// same way `bynk_ide::diagnose_project_with`'s own callers already do — the
/// direct replacement for `diagnose_project(&root, &HashMap::new())`'s
/// reliance on `bynk-emit`'s disk fallback filling in what the (empty)
/// overlay doesn't cover.
///
/// Content-ownership track (#1086) slice 5 correction: for
/// [`bynk_ide::AnalysisRoots::Project`], also reads `roots`'s own
/// `bynk.toml` and includes it in the returned map — `bynk_ide::discover_files`
/// needs it to resolve a non-conventional `[paths] include`/`exclude`, and a
/// caller re-lowering `roots` against this map (e.g. a subsequent
/// `diagnose_project_with`) needs it too. `bynk-ide` can no longer fall back
/// to a disk read for a miss itself (R2.3), so this crate — a dev-only test
/// seam, not gated by R2.3 at all — is where that real read belongs.
pub fn read_project_sources(roots: &bynk_ide::AnalysisRoots) -> HashMap<PathBuf, String> {
    let overlay = manifest_overlay(roots);
    let mut sources = read_all(bynk_ide::discover_files(roots, &overlay));
    sources.extend(overlay);
    sources
}

/// `bynk.toml`'s real on-disk content for [`bynk_ide::AnalysisRoots::Project`],
/// as a one-entry map — empty for `SingleTree` (no manifest consulted) or an
/// unreadable/absent manifest, both already-handled "no manifest" cases.
fn manifest_overlay(roots: &bynk_ide::AnalysisRoots) -> HashMap<PathBuf, String> {
    let bynk_ide::AnalysisRoots::Project(root) = roots else {
        return HashMap::new();
    };
    let toml_path = root.join("bynk.toml");
    match std::fs::read_to_string(&toml_path) {
        Ok(text) => HashMap::from([(toml_path, text)]),
        Err(_) => HashMap::new(),
    }
}

/// `CompileOptions::single(root)`, with every source pre-read — the direct
/// replacement for its reliance on `bynk-emit`'s disk fallback.
pub fn compile_options_single(root: impl Into<PathBuf>) -> bynk_emit::project::CompileOptions {
    let root = root.into();
    let sources = read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let assets = list_assets(&root);
    let embeds = list_embeds(&root);
    bynk_emit::project::CompileOptions::single(root)
        .sources(sources)
        .asset_files(assets)
        .embed_files(embeds)
}

/// `CompileOptions::split(project_root, paths)`, with every source pre-read.
/// Discovers exactly the files `CompileOptions::split` itself would compile —
/// built from the same `paths` this call also hands to `split`, not
/// re-derived from `bynk.toml` through a second read, so the two can never
/// disagree.
pub fn compile_options_split(
    project_root: impl Into<PathBuf>,
    paths: bynk_emit::project::ProjectPaths,
) -> bynk_emit::project::CompileOptions {
    let project_root = project_root.into();
    let roots = bynk_emit::project::Roots::Split {
        project_root: project_root.clone(),
        paths: paths.clone(),
    };
    let sources = read_all(bynk_emit::project::discover_project_files(&roots));
    let assets = list_assets(&project_root);
    let embeds = list_embeds(&project_root);
    bynk_emit::project::CompileOptions::split(project_root, paths)
        .sources(sources)
        .asset_files(assets)
        .embed_files(embeds)
}

/// #1883, slice 1b: the project's `assets/` folder, as the driver lists it
/// for a real build — so a fixture's `assets/` reaches the compiler.
fn list_assets(project_root: &std::path::Path) -> bynk_emit::project::assets::AssetFolder {
    bynk_emit::project::assets::AssetFolder::on_disk(project_root)
        .unwrap_or_else(|e| panic!("list `{}/assets`: {e}", project_root.display()))
}

/// #2387, slice 2: the project's `embeds/` folder, listed the same way.
fn list_embeds(project_root: &std::path::Path) -> bynk_emit::project::assets::EmbedFolder {
    bynk_emit::project::assets::AssetFolder::embeds_on_disk(project_root)
        .unwrap_or_else(|e| panic!("list `{}/embeds`: {e}", project_root.display()))
}

/// Stage the whole runtime (`__bynk_runtime/*.ts`) under `dir`, for a test that
/// compiles emitted code by hand beside a runtime (ADR 0615: a project's own
/// output carries only the modules it reaches).
pub fn write_runtime_modules(dir: &std::path::Path) -> std::io::Result<()> {
    for (path, source) in bynk_emit::emitter::emit_all_runtime_modules() {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, source)?;
    }
    Ok(())
}
