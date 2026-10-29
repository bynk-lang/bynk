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
use std::path::PathBuf;

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

pub use bynk_ide::doc_link_urls::*;
