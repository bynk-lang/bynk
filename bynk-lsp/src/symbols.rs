//! Re-exports `bynk-ide`'s symbol logic (#808) — moved there so it's
//! reachable from `bynk-wasm` too, which can't depend on this crate's
//! `tower-lsp`/`tokio` stack. See `bynk_ide::symbols` for the real module.
//!
//! The one exception is the cross-file lookup trio below: `bynk-ide`'s
//! versions are `PathBuf`-keyed (wasm has no `Url`), so this crate re-wraps
//! them in the `Url` shape the LSP handlers actually use. Rust's shadowing
//! rule lets these locally-defined names win over the glob import.
pub use bynk_ide::symbols::*;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_syntax::span::Span;
use tower_lsp::lsp_types::Url;

/// A cross-file declaration lookup result: the URI of the file containing
/// the declaration, the declaration's source span, and the full source
/// text of that file (returned because callers need it to convert the
/// span to an LSP range and to build hover content).
pub struct CrossFileSymbol {
    pub uri: Url,
    pub span: Span,
    pub source: String,
}

/// Find `name`'s declaration in any project file other than `current_uri`.
/// `files` is a pre-read `(path, content)` map (content-ownership track,
/// #1086, slice 1) — see `bynk_ide::symbols::find_declaration_cross_file`
/// for the pure logic.
pub fn find_declaration_cross_file(
    files: &HashMap<PathBuf, String>,
    current_uri: &Url,
    name: &str,
) -> Option<CrossFileSymbol> {
    let current_path = current_uri.to_file_path().ok()?;
    let found = bynk_ide::symbols::find_declaration_cross_file(files, &current_path, name)?;
    Some(CrossFileSymbol {
        uri: Url::from_file_path(&found.path).ok()?,
        span: found.span,
        source: found.source,
    })
}

/// Markdown hover content for `name` from any project file other than
/// `current_uri`, plus the URI of the file that contributed it. See
/// `bynk_ide::symbols::describe_symbol_cross_file` for the pure logic.
pub fn describe_symbol_cross_file(
    files: &HashMap<PathBuf, String>,
    current_uri: &Url,
    name: &str,
) -> Option<(Url, String)> {
    let current_path = current_uri.to_file_path().ok()?;
    let (path, desc) = bynk_ide::symbols::describe_symbol_cross_file(files, &current_path, name)?;
    Some((Url::from_file_path(&path).ok()?, desc))
}

/// #848: the `file://` target a resolved intra-doc link opens — the defining
/// file, with a `#L<line>,<column>` fragment (1-based, the form VS Code's
/// opener reads) placing the cursor on the declaration. Without the fragment
/// a click only opens the file, which for a same-file link (the common case)
/// does nothing visible. `snapshots` is the round's project-relative path →
/// analysed text map, against which `def.span` is measured; a target missing
/// from it still links, to the file alone.
pub fn doc_link_target(
    project_root: &Path,
    def: &bynk_check::index::SiteRef,
    snapshots: &HashMap<PathBuf, String>,
) -> Option<Url> {
    let mut url = Url::from_file_path(project_root.join(&def.path)).ok()?;
    if let Some(text) = snapshots.get(&def.path) {
        let pos = crate::position::offset_to_position(text, def.span.start);
        url.set_fragment(Some(&format!("L{},{}", pos.line + 1, pos.character + 1)));
    }
    Some(url)
}

/// #2455: the target of one doc-link candidate — the project scope first
/// (`resolve_doc_link`, a `file://` URL), then, only if that finds nothing, the
/// first-party Book entry (`first_party_docs`, an `https://` URL). Shared by
/// `document_link` and `linkify_doc_links`.
pub fn doc_link_url(
    index: &bynk_check::index::ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    project_root: &Path,
    snapshots: &HashMap<PathBuf, String>,
    owner_unit: &str,
    name: &str,
) -> Option<Url> {
    if let Some(def) = crate::index_queries::resolve_doc_link(index, doc_scope, owner_unit, name) {
        return doc_link_target(project_root, def, snapshots);
    }
    let external =
        crate::first_party_docs::resolve_first_party_doc_link(doc_scope, owner_unit, name)?;
    Url::parse(&external).ok()
}

/// #848: rewrite every resolvable intra-doc-link candidate in `content`
/// (hover Markdown — a ```bynk fenced signature, optionally followed by the
/// declaration's doc-comment prose) into a Markdown link, resolved against
/// `owner_unit`'s doc-link scope order (`crate::index_queries::resolve_doc_link`).
/// The fenced signature block (and any author example fence) is never
/// touched — `scan_doc_link_candidates` skips fenced regions itself. An
/// unresolved candidate is left exactly as authored — no diagnostic,
/// matching the render-only decision.
pub fn linkify_doc_links(
    content: &str,
    index: &bynk_check::index::ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    project_root: &Path,
    snapshots: &HashMap<PathBuf, String>,
    owner_unit: &str,
) -> String {
    let candidates = bynk_ide::symbols::scan_doc_link_candidates(content);
    if candidates.is_empty() {
        return content.to_string();
    }
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    for cand in candidates {
        let Some(url) = doc_link_url(
            index,
            doc_scope,
            project_root,
            snapshots,
            owner_unit,
            &cand.name,
        ) else {
            continue;
        };
        out.push_str(&content[last..cand.span.start]);
        out.push_str(&format!("[{}]({url})", cand.display));
        last = cand.span.end;
    }
    out.push_str(&content[last..]);
    out
}
