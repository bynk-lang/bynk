//! #2471: `bynk/emittedOutput` — the generated TypeScript for one source file,
//! with the emitter's source map decoded, for the editor's "Show Compiled
//! Output" view.
//!
//! File-scoped, no position (the view follows the cursor client-side, from the
//! lookup tables this answer carries). Served from the committed round (#733):
//! the round's snapshots are compiled in memory with `compile_project`, so an
//! unsaved buffer is what is shown and nothing is written to disk. The server
//! keeps no typed program between rounds, so emission re-runs the checker; that
//! cost is paid only when the view asks (on open, and on each save while it is
//! open), never per keystroke. No refresh nudge, like every custom request here.
//!
//! **What "the file's output" is.** A source file emits to the TypeScript
//! document its map names it in: one per unit, or several in a Workers build
//! (a context's worker entry and its handlers). Every output whose map lists
//! the file as a source is returned, in output-path order. The per-module
//! runtime (`__bynk_runtime/<module>`, ADR 0615) is project-wide support code
//! with no source file, so it is never part of a file's output; the emitted
//! modules import it by relative path.
//!
//! **The source map.** The emitter's maps are line-level: one segment per
//! generated line, mapping it to the source line of the nearest enclosing
//! statement (ADR 0103). [`WireEmittedFile::lines`] is that table, decoded:
//! entry *n* is generated line *n*'s `[source id, source line]` (both 0-based),
//! or `null` before the first mapped line. The client derives both directions
//! of the highlight from it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_emit::project::{CompileOptions, Document, sibling_path};

/// The `bynk/emittedOutput` request payload. camelCase on the wire; see
/// `wire_contract_request`'s params doc for the missing-field bug a bare
/// snake_case field would cause.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmittedOutputParams {
    pub text_document: tower_lsp::lsp_types::TextDocumentIdentifier,
}

/// One generated TypeScript document and the decoded map back to its sources.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WireEmittedFile {
    /// The output path, relative to the output root (`greet.ts`).
    pub path: String,
    /// The generated TypeScript.
    pub text: String,
    /// The map's sources, as the emitter named them (absolute, `/`-separated).
    pub sources: Vec<String>,
    /// Which entry of [`Self::sources`] is the requested file.
    pub source_index: usize,
    /// Per generated line, `[source id, source line]` (0-based), or `null`.
    pub lines: Vec<Option<[usize; 2]>>,
}

/// The `bynk/emittedOutput` response.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WireEmittedOutput {
    pub files: Vec<WireEmittedFile>,
}

/// Compile the round's `snapshots` (project-relative path → text) and return
/// the outputs that originate in `rel`. `None` when the project does not
/// compile (the editor already shows why) or is not a manifest-backed project
/// root the build can be rooted at.
pub fn emitted_output_for(
    project_root: &Path,
    snapshots: &HashMap<PathBuf, String>,
    rel: &Path,
) -> Option<WireEmittedOutput> {
    let mut sources: HashMap<PathBuf, String> = HashMap::new();
    for (p, text) in snapshots {
        let abs = project_root.join(p);
        sources.insert(abs.canonicalize().unwrap_or(abs), text.clone());
    }
    // The manifest only decides the layout: handed to the build as a source it
    // would be parsed as a `.bynk` file.
    let manifest = project_root.join("bynk.toml");
    let manifest_overlay: HashMap<PathBuf, String> = std::fs::read_to_string(&manifest)
        .map(|text| HashMap::from([(manifest.canonicalize().unwrap_or(manifest), text)]))
        .unwrap_or_default();
    let paths = bynk_project::try_read_project_paths_with(project_root, &manifest_overlay)
        .unwrap_or_else(|_| bynk_project::ProjectPaths::conventional(project_root));
    // `assets/` and `embeds/` are listed from disk: a declared asset must
    // resolve for the build to succeed, and the views never edit them.
    let mut options = CompileOptions::split(project_root.to_path_buf(), paths).sources(sources);
    if let Ok(f) = bynk_emit::project::assets::AssetFolder::on_disk(project_root) {
        options = options.asset_files(f);
    }
    if let Ok(f) = bynk_emit::project::assets::AssetFolder::embeds_on_disk(project_root) {
        options = options.embed_files(f);
    }
    let output = bynk_emit::project::compile_project(&options).ok()?;

    let wanted = project_root.join(rel);
    let wanted = wanted.canonicalize().unwrap_or(wanted);
    let wanted = wanted.to_string_lossy().replace('\\', "/");

    let mut files = Vec::new();
    for (path, doc) in &output.artefacts.docs {
        let Document::Ts(_) = doc else { continue };
        let Some(Document::SourceMap(map)) = output.artefacts.docs.get(&sibling_path(path, "map"))
        else {
            continue;
        };
        let Some(decoded) = decode_map(map) else {
            continue;
        };
        let Some(source_index) = decoded.sources.iter().position(|s| *s == wanted) else {
            continue;
        };
        files.push(WireEmittedFile {
            path: path.to_string_lossy().replace('\\', "/"),
            text: doc.text(),
            sources: decoded.sources,
            source_index,
            lines: decoded.lines,
        });
    }
    Some(WireEmittedOutput { files })
}

struct DecodedMap {
    sources: Vec<String>,
    lines: Vec<Option<[usize; 2]>>,
}

/// Decode a v3 map of the shape `bynk_ts::SourceMapBuilder::to_v3` writes: at
/// most one segment per generated line, generated column 0.
fn decode_map(json: &str) -> Option<DecodedMap> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let sources = v
        .get("sources")?
        .as_array()?
        .iter()
        .map(|s| s.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    let mappings = v.get("mappings")?.as_str()?;
    let (mut src, mut line) = (0i64, 0i64);
    let mut lines = Vec::new();
    for seg in mappings.split(';') {
        if seg.is_empty() {
            lines.push(None);
            continue;
        }
        let fields = vlq_decode(seg)?;
        // [generated column, source id, source line, source column]
        if fields.len() < 4 {
            lines.push(None);
            continue;
        }
        src += fields[1];
        line += fields[2];
        if src < 0 || line < 0 {
            return None;
        }
        lines.push(Some([src as usize, line as usize]));
    }
    Some(DecodedMap { sources, lines })
}

/// Decode a run of base64 VLQ values.
fn vlq_decode(s: &str) -> Option<Vec<i64>> {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut shift, mut acc) = (0u32, 0i64);
    for &c in s.as_bytes() {
        let d = B64.iter().position(|&b| b == c)? as i64;
        acc += (d & 0b11111) << shift;
        if d & 0b100000 != 0 {
            shift += 5;
            if shift > 60 {
                return None;
            }
        } else {
            out.push(if acc & 1 == 1 { -(acc >> 1) } else { acc >> 1 });
            shift = 0;
            acc = 0;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vlq_round_trips_signed_values() {
        // "AAAA" = 0,0,0,0; "gBAC" = 16,0,1; "DAAD" = -1,0,0,-1.
        assert_eq!(vlq_decode("AAAA"), Some(vec![0, 0, 0, 0]));
        assert_eq!(vlq_decode("gBAC"), Some(vec![16, 0, 1]));
        assert_eq!(vlq_decode("DAAD"), Some(vec![-1, 0, 0, -1]));
        assert_eq!(vlq_decode("!"), None);
    }

    #[test]
    fn decode_map_accumulates_source_lines_and_keeps_unmapped_gaps() {
        // Line 0 unmapped; line 1 → src 0 line 2; line 2 → src 0 line 5.
        let json = r#"{"version":3,"sources":["/p/a.bynk"],"mappings":";AAEA;AAGA"}"#;
        let d = decode_map(json).unwrap();
        assert_eq!(d.sources, vec!["/p/a.bynk".to_string()]);
        assert_eq!(d.lines, vec![None, Some([0, 2]), Some([0, 5])]);
    }

    #[test]
    fn decode_map_rejects_a_malformed_map() {
        assert!(decode_map("not json").is_none());
        assert!(decode_map(r#"{"sources":[],"mappings":"!"}"#).is_none());
    }
}
