//! #848/#2455: the Markdown link targets of intra-doc links, and the rewrite of
//! hover Markdown that uses them. Shared by the language server and `bynk mcp`
//! (#2520), so both render the same hover.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use url::Url;

use crate::index_queries::DocLinkContext;

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
        let (line, character) =
            bynk_syntax::span::LineIndex::new(text).utf16_line_col(text, def.span.start);
        url.set_fragment(Some(&format!("L{},{}", line + 1, character + 1)));
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
    doc_link_url_in(
        index,
        doc_scope,
        project_root,
        snapshots,
        owner_unit,
        name,
        None,
    )
}

/// #2454: [`doc_link_url`] resolving through
/// `crate::index_queries::resolve_doc_link_in`, so `ctx` (the declaration the
/// doc comment is attached to) lets its own fields and parameters link.
pub fn doc_link_url_in(
    index: &bynk_check::index::ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    project_root: &Path,
    snapshots: &HashMap<PathBuf, String>,
    owner_unit: &str,
    name: &str,
    ctx: Option<&DocLinkContext<'_>>,
) -> Option<Url> {
    if let Some(resolved) =
        crate::index_queries::resolve_doc_link_in(index, doc_scope, owner_unit, name, ctx)
    {
        return doc_link_target(project_root, &resolved.def, snapshots);
    }
    // A project declaration of the same spelling shadows the first-party name,
    // including when `resolve_doc_link` declined it as ambiguous.
    let owner = name.split('.').next().unwrap_or(name);
    if crate::index_queries::doc_link_owner_declared(index, doc_scope, owner_unit, owner) {
        return None;
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
    linkify_doc_links_in(
        content,
        index,
        doc_scope,
        project_root,
        snapshots,
        owner_unit,
        None,
    )
}

/// #2454: [`linkify_doc_links`] resolving through
/// `crate::index_queries::resolve_doc_link_in`, so `ctx` (the declaration the
/// prose documents) lets its own fields and parameters link.
pub fn linkify_doc_links_in(
    content: &str,
    index: &bynk_check::index::ProjectIndex,
    doc_scope: &HashMap<String, Vec<String>>,
    project_root: &Path,
    snapshots: &HashMap<PathBuf, String>,
    owner_unit: &str,
    ctx: Option<&DocLinkContext<'_>>,
) -> String {
    let candidates = crate::symbols::scan_doc_link_candidates(content);
    if candidates.is_empty() {
        return content.to_string();
    }
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    for cand in candidates {
        let Some(url) = doc_link_url_in(
            index,
            doc_scope,
            project_root,
            snapshots,
            owner_unit,
            &cand.name,
            ctx,
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
