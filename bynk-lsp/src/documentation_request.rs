//! #847: `bynk/documentationModel` — the documentation-view custom LSP request.
//!
//! The second custom request in this server (after #846's `bynk/sequenceModel`),
//! and the same posture: no `workspace/*/refresh` nudge exists for a custom
//! method and none is needed — Tier 1 is on-demand, the client re-issues the
//! request each time "Bynk: Show Documentation" fires (Decision D).
//!
//! Unlike `bynk/sequenceModel`, the request carries **no cursor position**: a
//! documentation page is the *whole unit's* declarations, so the params are a
//! bare `TextDocumentIdentifier`. The page was file-scoped (Decision A) until
//! #1885, which merges every file of a multi-file unit
//! ([`documentation_model_for`]). The wire shape is a plain serde mirror of
//! [`bynk_ide::documentation::DocModel`], each `Span` lowered to an LSP `Range`
//! against the committed snapshot text of the file it is in — the same
//! convention `sequence_request`/`SerKey` use — with a per-entry `uri` naming
//! that file (the `{uri, range}` pattern `architecture_request` uses).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_ide::documentation;
use tower_lsp::lsp_types::Url;

/// The `bynk/documentationModel` request payload. A bare text-document
/// identifier — no cursor position (the page is the whole unit).
///
/// `rename_all = "camelCase"` is load-bearing: the client sends the LSP wire
/// name `textDocument`, so the field must deserialize from camelCase, not the
/// Rust `text_document`. (Without it, every request fails with a missing-field
/// error — a wire-shape bug no direct `documentation_model_for` test would
/// catch, only a deserialize test; see `documentation_request.rs`'s params
/// test.)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentationModelParams {
    pub text_document: tower_lsp::lsp_types::TextDocumentIdentifier,
}

// -- Wire shape: a plain serde mirror of `bynk_ide::documentation::DocModel`,
// -- `Span` lowered to `Range` against the committed snapshot text.

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireDocModel {
    #[serde(rename = "unitName")]
    pub unit_name: String,
    #[serde(rename = "unitKind")]
    pub unit_kind: &'static str,
    #[serde(rename = "unitDoc")]
    pub unit_doc: Option<String>,
    /// #2462: as [`WireDocEntry::links`], for `unit_doc`.
    #[serde(rename = "unitLinks")]
    pub unit_links: Vec<WireDocLink>,
    #[serde(rename = "unitRange")]
    pub unit_range: tower_lsp::lsp_types::Range,
    pub entries: Vec<WireDocEntry>,
}

/// #2462: where a click on an intra-doc link goes. Resolved server-side by
/// `resolve_doc_link`, so the webview only routes the click.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WireDocLink {
    /// The target is documented on this page: scroll to `entries[entry]`.
    Entry { entry: usize },
    /// Open the declaration in the editor.
    Source {
        uri: Url,
        range: tower_lsp::lsp_types::Range,
    },
    /// An external page (opened through the host's http(s) gate): a
    /// first-party name's entry in the Book (#2455).
    External { href: String },
}

/// #2462: what resolving the intra-doc links on a page needs — the round's
/// index and doc-link scope (the inputs `resolve_doc_link` takes).
#[derive(Clone, Copy)]
pub struct DocLinkContext<'a> {
    pub index: &'a bynk_check::index::ProjectIndex,
    pub doc_scope: &'a HashMap<String, Vec<String>>,
    /// #2523: the round's per-file local bindings, from which a link in a
    /// fn / handler / op's doc reads its parameters.
    pub locals: &'a bynk_check::locals::FileLocals,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WireDocEntry {
    pub name: String,
    pub kind: &'static str,
    pub depth: u32,
    pub markdown: String,
    /// #2462: the resolved targets of the intra-doc links in `markdown`;
    /// each is spelled in the Markdown as `[text](#bynk-doc-link-N)`, `N`
    /// indexing this list.
    pub links: Vec<WireDocLink>,
    pub documented: bool,
    pub range: tower_lsp::lsp_types::Range,
    /// #1885: the document `range` is in. Always sent: the page is merged
    /// across a unit's files, so an entry may live in a file other than the
    /// requested one (and a one-file unit's entries simply all name it).
    pub uri: tower_lsp::lsp_types::Url,
}

/// #1885: the wire page for the document at `rel` (project-relative), merged
/// across every file of its unit. The unit's files come from the round's
/// `unit_sources` (qualified name → project-relative paths) and their text
/// from `snapshots`; each entry's range is lowered against *its own* file and
/// carries that file's `file://` URI, so click-to-code opens the right one.
/// An entry whose file has no snapshot or no URI is dropped rather than sent
/// with a wrong location. `None` for a `suite`, a file with no header, or a
/// `rel` with no snapshot.
pub fn documentation_model_for(
    rel: &Path,
    snapshots: &HashMap<PathBuf, String>,
    unit_sources: &HashMap<String, Vec<PathBuf>>,
    project_root: &Path,
    links: Option<DocLinkContext<'_>>,
) -> Option<WireDocModel> {
    let text = snapshots.get(rel)?;
    let siblings: Vec<(&Path, &str)> = bynk_ide::symbols::own_declaration_name(text)
        .and_then(|(name, _)| unit_sources.get(&name))
        .into_iter()
        .flatten()
        .filter_map(|p| snapshots.get(p).map(|t| (p.as_path(), t.as_str())))
        .collect();
    let model = documentation::documentation_model_merged((rel, text.as_str()), siblings)?;
    // Each entry's range is lowered once, against the snapshot of the file
    // its span is in. `placed` keeps the model entry's `(file, span)` beside
    // its wire entry, which is where an in-page link target is looked up.
    let mut placed: Vec<(PathBuf, bynk_syntax::span::Span)> = Vec::new();
    let mut entries: Vec<WireDocEntry> = Vec::new();
    for e in &model.entries {
        let file = e.file.as_deref().unwrap_or(rel);
        let Some(file_text) = snapshots.get(file) else {
            continue;
        };
        let Ok(uri) = Url::from_file_path(project_root.join(file)) else {
            continue;
        };
        placed.push((file.to_path_buf(), e.span));
        entries.push(WireDocEntry {
            name: e.name.clone(),
            kind: e.kind,
            depth: e.depth,
            markdown: e.markdown.clone(),
            links: Vec::new(),
            documented: e.documented,
            range: crate::position::span_to_range(file_text, e.span),
            uri,
        });
    }
    let mut unit_doc = model.unit_doc;
    let mut unit_links = Vec::new();
    if let Some(ctx) = links {
        let linker = Linker {
            ctx,
            owner_unit: &model.unit_name,
            snapshots,
            project_root,
            placed: &placed,
        };
        for (e, (file, span)) in entries.iter_mut().zip(&placed) {
            let (markdown, links) = linker.linkify(&e.markdown, file, span.start);
            e.markdown = markdown;
            e.links = links;
        }
        if let Some(doc) = &unit_doc {
            let (markdown, links) = linker.linkify(doc, rel, model.unit_span.start);
            unit_doc = Some(markdown);
            unit_links = links;
        }
    }
    Some(WireDocModel {
        unit_name: model.unit_name,
        unit_kind: model.unit_kind,
        unit_doc,
        unit_links,
        unit_range: crate::position::span_to_range(text, model.unit_span),
        entries,
    })
}

/// #2462: rewrites a block of Markdown's resolvable intra-doc links.
struct Linker<'a> {
    ctx: DocLinkContext<'a>,
    owner_unit: &'a str,
    snapshots: &'a HashMap<PathBuf, String>,
    project_root: &'a Path,
    /// `(file, name span)` per wire entry, index-aligned with the page.
    placed: &'a [(PathBuf, bynk_syntax::span::Span)],
}

impl Linker<'_> {
    /// Rewrite each resolvable candidate in `markdown` to
    /// `[text](#bynk-doc-link-N)` and return the targets, `N`-indexed. An
    /// unresolved candidate is left as authored (the render-only decision of
    /// #848). The resolution is `resolve_doc_link`'s — the same as hover and
    /// `documentLink`.
    ///
    /// #2523: `file` / `attached` are the declaration the doc is attached to
    /// (its file and the byte offset of its name), so its own fields and
    /// parameters link as they do in the editor.
    fn linkify(&self, markdown: &str, file: &Path, attached: usize) -> (String, Vec<WireDocLink>) {
        let dc = self
            .snapshots
            .get(file)
            .map(|text| crate::index_queries::DocLinkContext {
                path: file,
                text: text.as_str(),
                attached,
                locals: self.ctx.locals.get(file).map_or(&[], |l| l.as_slice()),
            });
        let mut out = String::with_capacity(markdown.len());
        let mut links = Vec::new();
        let mut last = 0;
        for cand in bynk_ide::symbols::scan_doc_link_candidates(markdown) {
            let Some(target) = self.resolve(&cand.name, dc.as_ref()) else {
                continue;
            };
            out.push_str(&markdown[last..cand.span.start]);
            out.push_str(&format!(
                "[{}](#bynk-doc-link-{})",
                cand.display,
                links.len()
            ));
            links.push(target);
            last = cand.span.end;
        }
        out.push_str(&markdown[last..]);
        (out, links)
    }

    /// The project's own declaration first; only when the project has none of
    /// that spelling, the first-party Book entry (`doc_link_url`, which keeps
    /// a project declaration shadowing a first-party name).
    fn resolve(
        &self,
        name: &str,
        dc: Option<&crate::index_queries::DocLinkContext<'_>>,
    ) -> Option<WireDocLink> {
        if let Some(hit) = crate::index_queries::resolve_doc_link_in(
            self.ctx.index,
            self.ctx.doc_scope,
            self.owner_unit,
            name,
            dc,
        ) {
            return self.target(&hit.def);
        }
        let url = crate::symbols::doc_link_url_in(
            self.ctx.index,
            self.ctx.doc_scope,
            self.project_root,
            self.snapshots,
            self.owner_unit,
            name,
            dc,
        )?;
        matches!(url.scheme(), "http" | "https").then(|| WireDocLink::External {
            href: url.to_string(),
        })
    }

    /// An in-page anchor when the page documents the declaration (decision A
    /// of #2462), else the declaration in the editor.
    fn target(&self, def: &bynk_check::index::SiteRef) -> Option<WireDocLink> {
        if let Some(entry) = self
            .placed
            .iter()
            .position(|(file, span)| *file == def.path && span.start == def.span.start)
        {
            return Some(WireDocLink::Entry { entry });
        }
        let text = self.snapshots.get(&def.path)?;
        Some(WireDocLink::Source {
            uri: Url::from_file_path(self.project_root.join(&def.path)).ok()?,
            range: crate::position::span_to_range(text, def.span),
        })
    }
}
