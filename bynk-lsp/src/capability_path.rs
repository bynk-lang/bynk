//! #2469: "why does this need `Logger`?" — the editor face of the checker's
//! capability paths ([`bynk_check::capability_paths`]).
//!
//! Hovering a `given` entry shows the **shortest** chain from the declaration to
//! the capability's use, each step a link to source. The "Show capability path"
//! code action lists **every** chain: it carries a client-side command
//! ([`SHOW_COMMAND`]) that opens the editor's references peek over the steps'
//! sites. Both read the checker's recorded graph; nothing here re-derives a
//! path from the source text, and the full set is computed on demand by
//! [`CapabilityGraph::all_paths`] rather than stored per entry.
//!
//! Because the command is executed by the *client*, the action is offered only
//! to a client that says it implements it —
//! `experimental.capabilityPathCommand: true` in its `ClientCapabilities` — the
//! same gate as the "Explain …" action ([`crate::explain_request`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_check::capability_paths::{CapabilityGraph, FileCapabilityPaths, PathStep};
use bynk_syntax::span::{FileId, Span};
use tower_lsp::lsp_types::{CodeAction, CodeActionKind, CodeActionOrCommand, Command, Url};

use crate::position::offset_to_position;

/// The client-side command the code action invokes. The VS Code extension
/// registers it (`vscode-bynk/src/capabilityPath.ts`).
pub const SHOW_COMMAND: &str = "bynk.showCapabilityPath";

/// The `ClientCapabilities.experimental` key a client sets to `true` to say it
/// implements [`SHOW_COMMAND`].
pub const CLIENT_CAPABILITY: &str = "capabilityPathCommand";

/// Whether the client advertised `experimental.capabilityPathCommand: true`.
pub fn client_supports_command(experimental: Option<&serde_json::Value>) -> bool {
    experimental
        .and_then(|v| v.get(CLIENT_CAPABILITY))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

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
    fn graph(&self) -> Option<&CapabilityGraph> {
        self.graphs.get(self.rel)
    }

    /// The project-relative file a span lies in. A unit's declarations can span
    /// several files, so a step may live in a sibling; each graph records the
    /// checker's id for its own file, which maps ids back to paths. An unmapped
    /// id falls back to the cursor's file.
    fn file_of(&self, id: FileId) -> &Path {
        self.graphs
            .iter()
            .find(|(_, g)| g.file == id)
            .map_or(self.rel, |(p, _)| p.as_path())
    }

    fn text_of(&self, span: Span) -> Option<(&Path, &str)> {
        let path = self.file_of(span.file);
        Some((path, self.snapshots.get(path)?.as_str()))
    }

    /// `file://` target of `span`, with the `#L<line>,<column>` fragment VS Code's
    /// opener reads (as intra-doc links do — `symbols::doc_link_target`).
    fn link(&self, span: Span) -> Option<Url> {
        let (path, text) = self.text_of(span)?;
        let mut url = Url::from_file_path(self.project_root.join(path)).ok()?;
        let pos = offset_to_position(text, span.start);
        url.set_fragment(Some(&format!("L{},{}", pos.line + 1, pos.character + 1)));
        Some(url)
    }

    fn step_label(&self, step: &PathStep, capability: &str) -> String {
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

/// A location the client peeks at: a step's site.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StepLocation {
    pub uri: Url,
    pub range: tower_lsp::lsp_types::Range,
}

/// The full set of chains for the entry at `offset`, flattened to the sites the
/// peek lists (path by path, shortest first, a shared site once), with the
/// capability, the entry's position, the number of chains, and whether the
/// checker's cap cut the list short.
pub struct AllPaths {
    pub capability: String,
    pub entry: tower_lsp::lsp_types::Position,
    pub paths: usize,
    pub truncated: bool,
    pub locations: Vec<StepLocation>,
}

pub fn all_paths(view: &PathView<'_>, offset: usize) -> Option<AllPaths> {
    let graph = view.graph()?;
    let (entry, _) = graph.entry_at(offset)?;
    let (capability, paths, truncated) = graph.all_paths(entry)?;
    if paths.is_empty() {
        return None;
    }
    let text = view.snapshots.get(view.rel)?;
    let mut locations: Vec<StepLocation> = Vec::new();
    for path in &paths {
        for step in &path.steps {
            let Some(loc) = location(view, step.site()) else {
                continue;
            };
            if !locations.contains(&loc) {
                locations.push(loc);
            }
        }
    }
    Some(AllPaths {
        capability,
        entry: offset_to_position(text, entry.start),
        paths: paths.len(),
        truncated,
        locations,
    })
}

fn location(view: &PathView<'_>, span: Span) -> Option<StepLocation> {
    let (path, text) = view.text_of(span)?;
    let uri = Url::from_file_path(view.project_root.join(path)).ok()?;
    let positions = crate::position::PositionMap::new(text);
    Some(StepLocation {
        uri,
        range: positions.range(span),
    })
}

/// The "Show capability path" action for the `given` entry at `offset`, or none
/// off an entry. Its command carries `[uri, position, locations]` for the
/// client to hand to the editor's references peek.
pub fn show_action(view: &PathView<'_>, uri: &Url, offset: usize) -> Option<CodeActionOrCommand> {
    let all = all_paths(view, offset)?;
    let title = if all.paths == 1 {
        format!("Show capability path for `{}`", all.capability)
    } else {
        format!(
            "Show capability paths for `{}` ({}{})",
            all.capability,
            all.paths,
            if all.truncated { "+" } else { "" }
        )
    };
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: title.clone(),
        kind: Some(CodeActionKind::QUICKFIX),
        command: Some(Command {
            title,
            command: SHOW_COMMAND.to_string(),
            arguments: Some(vec![
                serde_json::json!(uri),
                serde_json::json!(all.entry),
                serde_json::json!(all.locations),
            ]),
        }),
        is_preferred: Some(false),
        ..Default::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_needs_an_explicit_client_opt_in() {
        assert!(!client_supports_command(None));
        assert!(!client_supports_command(Some(&serde_json::json!({}))));
        assert!(!client_supports_command(Some(
            &serde_json::json!({ "capabilityPathCommand": "yes" })
        )));
        assert!(client_supports_command(Some(
            &serde_json::json!({ "capabilityPathCommand": true })
        )));
    }
}
