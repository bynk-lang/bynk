//! #2469: "why does this need `Logger`?" — the editor face of the checker's
//! capability paths ([`bynk_check::capability_paths`]).
//!
//! Hovering a `given` entry shows the **shortest** chain from the declaration to
//! the capability's use, each step a link to source. The "Show capability path"
//! code action lists **every** chain: it carries a client-side command
//! ([`SHOW_COMMAND`]) that opens the editor's references peek over the steps'
//! sites. Both read the checker's recorded graph; nothing here re-derives a
//! path from the source text, and the full set is computed on demand by
//! [`bynk_check::capability_paths::CapabilityGraph::all_paths`] rather than stored per entry.
//!
//! Because the command is executed by the *client*, the action is offered only
//! to a client that says it implements it —
//! `experimental.capabilityPathCommand: true` in its `ClientCapabilities` — the
//! same gate as the "Explain …" action ([`crate::explain_request`]).

use bynk_syntax::span::Span;
use tower_lsp::lsp_types::Url;
use tower_lsp::lsp_types::{CodeAction, CodeActionKind, CodeActionOrCommand, Command};

use crate::position::offset_to_position;

/// The client-side command the code action invokes. The VS Code extension
/// registers it (`src/capabilityPath.ts` in bynk-lang/vscode-bynk).
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

pub use bynk_ide::capability_path::{PathView, hover_section};

/// A location the client peeks at: a step's site.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StepLocation {
    pub uri: Url,
    pub range: tower_lsp::lsp_types::Range,
}

/// The full set of chains for the entry at `offset`, flattened to the sites the
/// peek lists (path by path, shortest first, a shared site once), with the
/// capability, the entry's position, the number of chains, and whether the
/// checker's cap cut the list short. [`AllPaths::by_path`] keeps each chain's
/// own sites, a shared site repeated, so a client can mark where one path ends
/// and the next begins (#2534).
pub struct AllPaths {
    pub capability: String,
    pub entry: tower_lsp::lsp_types::Position,
    pub paths: usize,
    pub truncated: bool,
    pub locations: Vec<StepLocation>,
    /// Each chain's sites in order, shortest chain first (parallel to the
    /// chains counted by `paths`).
    pub by_path: Vec<Vec<StepLocation>>,
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
    let mut by_path: Vec<Vec<StepLocation>> = Vec::with_capacity(paths.len());
    for path in &paths {
        let mut own = Vec::with_capacity(path.steps.len());
        for step in &path.steps {
            let Some(loc) = location(view, step.site()) else {
                continue;
            };
            if !locations.contains(&loc) {
                locations.push(loc.clone());
            }
            own.push(loc);
        }
        by_path.push(own);
    }
    Some(AllPaths {
        capability,
        entry: offset_to_position(text, entry.start),
        paths: paths.len(),
        truncated,
        locations,
        by_path,
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
/// client to hand to the editor's references peek, then (#2534) `by_path`, each
/// chain's sites with shared ones repeated, for a client that groups the peek
/// by path; a client that reads three arguments is unaffected.
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
                serde_json::json!(all.by_path),
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
