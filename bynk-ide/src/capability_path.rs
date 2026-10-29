//! #2469: the hover face of the checker's capability paths
//! ([`bynk_check::capability_paths`]), shared by the language server and
//! `bynk mcp` (#2520).
//!
//! Hovering a `given` entry shows the **shortest** chain from the declaration to
//! the capability's use, each step a link to source. The language server's "Show
//! capability path" code action reads the same [`PathView`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_check::capability_paths::{CapabilityGraph, FileCapabilityPaths, PathStep};
use bynk_syntax::span::{FileId, LineIndex, Span};
use url::Url;

/// The round's tables, positioned on one file.
pub struct PathView<'a> {
    pub graphs: &'a FileCapabilityPaths,
    /// Project-relative path → analysed text, which the spans index into.
    pub snapshots: &'a HashMap<PathBuf, String>,
    pub project_root: &'a Path,
    /// The cursor's file (project-relative).
    pub rel: &'a Path,
}

impl PathView<'_> {
    /// The graph for the cursor's file.
    pub fn graph(&self) -> Option<&CapabilityGraph> {
        self.graphs.get(self.rel)
    }

    /// The project-relative file a span lies in. A unit's declarations can span
    /// several files, so a step may live in a sibling; each graph records the
    /// checker's id for its own file, which maps ids back to paths. An id no
    /// graph owns yields `None` (the step is dropped rather than linked to the
    /// wrong file); a span with no file identity belongs to the cursor's file.
    pub fn file_of(&self, id: FileId) -> Option<&Path> {
        if id == FileId::UNKNOWN {
            return Some(self.rel);
        }
        self.graphs
            .iter()
            .find(|(_, g)| g.file == id)
            .map(|(p, _)| p.as_path())
    }

    pub fn text_of(&self, span: Span) -> Option<(&Path, &str)> {
        let path = self.file_of(span.file)?;
        Some((path, self.snapshots.get(path)?.as_str()))
    }

    /// `file://` target of `span`, with the `#L<line>,<column>` fragment VS Code's
    /// opener reads (as intra-doc links do — `doc_link_urls::doc_link_target`).
    pub fn link(&self, span: Span) -> Option<Url> {
        let (path, text) = self.text_of(span)?;
        let mut url = Url::from_file_path(self.project_root.join(path)).ok()?;
        let (line, character) = LineIndex::new(text).utf16_line_col(text, span.start);
        url.set_fragment(Some(&format!("L{},{}", line + 1, character + 1)));
        Some(url)
    }

    pub fn step_label(&self, step: &PathStep, capability: &str) -> String {
        match step {
            PathStep::Call { agent, handler, .. } => format!("{agent}.{handler}"),
            PathStep::Use { source, .. } => source.reason(capability),
        }
    }
}

/// One step as a Markdown list line: the label as a link when it resolves.
fn step_line(view: &PathView<'_>, n: usize, step: &PathStep, capability: &str) -> String {
    let label = view.step_label(step, capability);
    let body = match (step, view.link(step.site())) {
        (PathStep::Call { .. }, Some(url)) => format!("calls [`{label}`]({url})"),
        (PathStep::Call { .. }, None) => format!("calls `{label}`"),
        (PathStep::Use { .. }, Some(url)) => format!("[{label}]({url})"),
        (PathStep::Use { .. }, None) => label,
    };
    format!("{n}. {body}")
}

/// The hover Markdown section for the `given` entry at `offset`: the shortest
/// chain, one linked step per line. `None` off an entry or when the entry
/// reaches no use (it is unused — `bynk.given.unused_capability` says so).
pub fn hover_section(view: &PathView<'_>, offset: usize) -> Option<String> {
    let graph = view.graph()?;
    let (entry, _) = graph.entry_at(offset)?;
    let w = graph.witness(entry)?;
    let heading = if w.path.is_direct() {
        format!("`{}` needs `{}` directly:", w.declaration, w.capability)
    } else {
        format!(
            "`{}` needs `{}` through {} call(s):",
            w.declaration,
            w.capability,
            w.path.steps.len() - 1
        )
    };
    let lines: Vec<String> = w
        .path
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| step_line(view, i + 1, s, &w.capability))
        .collect();
    Some(format!("{heading}\n\n{}", lines.join("\n")))
}
