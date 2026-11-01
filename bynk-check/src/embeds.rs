//! #2387 (embedded text): the checker-side rules for `embed name = "path"`
//! declarations — the path rules, the name rule, the composition of a unit's
//! visible embeds through `uses` — mirroring [`crate::assets`] for the part an
//! embed shares with an asset.
//!
//! An embed is a project file's text compiled into the code as a `String`
//! constant. Unlike an asset it is never served, so it is private, and it
//! comes from the project's `embeds/` folder instead of `assets/`.
//!
//! The checker stays disk-free: the folder reaches it from the caller as a
//! [`bynk_project::assets::EmbedFolder`], and [`read_declared_embeds`] reads
//! only the files a declaration names, applying the content rules
//! ([`embedded_text`]) and the size caps.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bynk_syntax::ast::{CommonsItem, EmbedDecl, FnName, Ident};
use bynk_syntax::error::CompileError;

use crate::project_model::{ErrorSink, UnitInfo};
use bynk_project::ParsedFile;

/// `bynk.embed.unsafe_path` for `e`, when its path is empty, absolute or
/// contains a `..` segment. The same rule as an asset's path
/// ([`crate::assets::unsafe_asset_path_reason`]): the path is joined onto the
/// project root to find the file, so nothing outside the project may be named.
pub fn unsafe_path_error(e: &EmbedDecl) -> Option<CompileError> {
    let reason = crate::assets::unsafe_asset_path_reason(&e.path)?;
    Some(
        CompileError::new(
            "bynk.embed.unsafe_path",
            e.path_span,
            format!("embed `{}` has an unsafe path: {reason}", e.name.name),
        )
        .with_note("an embed path is relative to the project root and must stay inside it"),
    )
}

/// `bynk.embed.outside_embeds_dir` for `e`, when its path is safe but does not
/// lie inside the project's embeds folder
/// ([`bynk_project::assets::PROJECT_EMBEDS_DIR`]). An embed is compiled into
/// the deployed code, so it comes only from that folder, and a declaration can
/// never pull `.env` or a source file into a bundle by mistake.
pub fn outside_embeds_dir_error(e: &EmbedDecl) -> Option<CompileError> {
    if crate::assets::unsafe_asset_path_reason(&e.path).is_some()
        || bynk_project::assets::is_in_embeds_dir(&e.path)
    {
        return None;
    }
    let dir = bynk_project::assets::PROJECT_EMBEDS_DIR;
    Some(
        CompileError::new(
            "bynk.embed.outside_embeds_dir",
            e.path_span,
            format!(
                "embed `{}` must name a file in the project's `{dir}/` folder",
                e.name.name
            ),
        )
        .with_note(format!(
            "an embed is compiled into the deployed code, so it comes only from `{dir}/`; move the file there and name it `{dir}/…`"
        )),
    )
}

/// The path rules for `e`, in order: [`unsafe_path_error`], then
/// [`outside_embeds_dir_error`] (at most one of the two applies).
pub fn path_errors(e: &EmbedDecl) -> Option<CompileError> {
    unsafe_path_error(e).or_else(|| outside_embeds_dir_error(e))
}

/// `bynk.embed.outside_context_or_commons` for `e`.
pub fn placement_error(e: &EmbedDecl) -> CompileError {
    CompileError::new(
        "bynk.embed.outside_context_or_commons",
        e.span,
        "`embed` declarations are only allowed inside a context or a commons",
    )
    .with_note(
        "an embed is compiled into the Worker of each context that declares it or `uses` a commons that does",
    )
}

/// The largest embedded file, in bytes. The text lands in the Worker bundle
/// and counts against the platform's script size limits, so an embed is for
/// markup and prose, not data.
pub const MAX_EMBED_BYTES: usize = 256 * 1024;

/// The most bytes a project's declared embeds may total (each file counted
/// once, however many declarations name it).
pub const MAX_EMBEDS_TOTAL_BYTES: usize = 1024 * 1024;

/// `bynk.embed.needs_project` for `e`: a single-file build emits one module
/// and has no project root to find `embeds/` under, so it cannot read the
/// file. Checking a single file is unaffected; building one that declares an
/// embed is refused. Build the project instead.
pub fn needs_project_error(e: &EmbedDecl) -> CompileError {
    CompileError::new(
        "bynk.embed.needs_project",
        e.span,
        format!(
            "embed `{}` cannot be built from a single file",
            e.name.name
        ),
    )
    .with_note(
        "an embed reads a file from the project's `embeds/` folder, so it needs a project build (`bynk dev`, `bynkc compile <project dir>`)",
    )
}

/// The text a file's bytes embed as, or why they cannot be: valid UTF-8
/// (`bynk.embed.invalid_encoding`) with no NUL byte (`bynk.embed.binary_content`).
/// A leading byte-order mark is dropped and `\r\n` and a bare `\r` become
/// `\n`, so the same source builds to the same constant whatever a checkout's
/// line-ending settings do. Everything else, including a trailing newline, is
/// kept as is.
pub fn embedded_text(bytes: &[u8]) -> Result<String, &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| "bynk.embed.invalid_encoding")?;
    if text.contains('\0') {
        return Err("bynk.embed.binary_content");
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// `bynk.embed.missing_file` for each embed whose (otherwise valid) path names
/// no file in `available` — the project's `embeds/` folder as the caller read
/// it. `None` skips the check: nothing on disk was consulted (an in-memory
/// check). A build passes `Some`, an empty set when it had no filesystem, so
/// it never invents text for a file nobody read.
///
/// A path that already fails [`path_errors`] is not also reported missing.
pub fn phase_embed_files(
    parsed: &[ParsedFile],
    available: Option<&BTreeSet<String>>,
    errors: &mut ErrorSink,
) {
    let Some(available) = available else { return };
    for pf in parsed {
        for item in pf.items() {
            let CommonsItem::Embed(e) = item else {
                continue;
            };
            if path_errors(e).is_some() {
                continue;
            }
            let key = bynk_project::assets::normalise_asset_path(&e.path);
            if !available.contains(&key) {
                errors.push_for(
                    Some(&pf.identity_path()),
                    CompileError::new(
                        "bynk.embed.missing_file",
                        e.path_span,
                        format!("embed `{}` names `{key}`, which does not exist", e.name.name),
                    )
                    .with_note(
                        "an embed must be a regular file in the project's `embeds/` folder; symbolic links are not followed",
                    ),
                );
            }
        }
    }
}

/// The text of every declared embed file in `folder`, keyed by normalised
/// path — read through the folder's reader once each, and only for a file a
/// declaration names. A declaration whose path fails [`path_errors`] or names
/// no listed file is skipped ([`phase_embed_files`] reports the latter). A
/// listed file the reader fails on is `bynk.embed.unreadable_file`; one that is
/// not text is `bynk.embed.invalid_encoding` or `bynk.embed.binary_content`; one over
/// [`MAX_EMBED_BYTES`], or that takes the declared files past
/// [`MAX_EMBEDS_TOTAL_BYTES`], is `bynk.embed.too_large`. Each is reported on
/// every declaration that names the file, and the file is left out.
pub fn read_declared_embeds(
    parsed: &[ParsedFile],
    folder: &bynk_project::assets::EmbedFolder,
    errors: &mut ErrorSink,
) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    // A file already judged and refused: its error, rebuilt for each further
    // declaration that names it.
    let mut refused: BTreeMap<String, (&'static str, String)> = BTreeMap::new();
    let mut total = 0usize;
    for pf in parsed {
        for item in pf.items() {
            let CommonsItem::Embed(e) = item else {
                continue;
            };
            if path_errors(e).is_some() {
                continue;
            }
            let key = bynk_project::assets::normalise_asset_path(&e.path);
            if !folder.files().contains(&key) || out.contains_key(&key) {
                continue;
            }
            if !refused.contains_key(&key) {
                match read_one(folder, &key, total) {
                    Ok((text, raw_len)) => {
                        total += raw_len;
                        out.insert(key.clone(), text);
                        continue;
                    }
                    Err(refusal) => {
                        refused.insert(key.clone(), refusal);
                    }
                }
            }
            let (code, detail) = &refused[&key];
            errors.push_for(
                Some(&pf.identity_path()),
                refusal_error(code, e, &key, detail),
            );
        }
    }
    out
}

/// One file's text, or the diagnostic code and detail that refuse it.
/// `total` is what the project's embeds already come to.
fn read_one(
    folder: &bynk_project::assets::EmbedFolder,
    key: &str,
    total: usize,
) -> Result<(String, usize), (&'static str, String)> {
    let bytes = folder.read(key).map_err(|e| {
        (
            "bynk.embed.unreadable_file",
            format!("the read failed: {e}"),
        )
    })?;
    if bytes.len() > MAX_EMBED_BYTES {
        return Err((
            "bynk.embed.too_large",
            format!(
                "it is {} bytes, over the {MAX_EMBED_BYTES}-byte limit for one embedded file",
                bytes.len()
            ),
        ));
    }
    if total + bytes.len() > MAX_EMBEDS_TOTAL_BYTES {
        return Err((
            "bynk.embed.too_large",
            format!(
                "with it the project's embedded files would pass {MAX_EMBEDS_TOTAL_BYTES} bytes in all"
            ),
        ));
    }
    embedded_text(&bytes)
        .map(|text| (text, bytes.len()))
        .map_err(|code| {
            let detail = if code == "bynk.embed.invalid_encoding" {
                "it is not valid UTF-8"
            } else {
                "it contains a NUL byte, so it is a binary file"
            };
            (code, detail.to_string())
        })
}

fn refusal_error(code: &'static str, e: &EmbedDecl, key: &str, detail: &str) -> CompileError {
    let (what, note) = match code {
        "bynk.embed.unreadable_file" => ("could not be read", None),
        "bynk.embed.invalid_encoding" => (
            "is not UTF-8 text",
            Some(
                "an embed is compiled into the code as a `String`; serve a binary file with `asset` instead",
            ),
        ),
        "bynk.embed.binary_content" => (
            "is not text",
            Some(
                "an embed is compiled into the code as a `String`; serve a binary file with `asset` instead",
            ),
        ),
        _ => (
            "is too large to embed",
            Some(
                "an embed is for markup and prose; the text lands in the Worker bundle, so keep large data out of it",
            ),
        ),
    };
    let mut err = CompileError::new(
        code,
        e.path_span,
        format!("embed `{}` names `{key}`, which {what}", e.name.name),
    )
    .with_note(detail.to_string());
    if let Some(note) = note {
        err = err.with_note(note);
    }
    err
}

/// The name rule for embed `e`, given what its unit already declares under
/// that name: an earlier embed is `bynk.embed.duplicate`; an asset, a free
/// function or a type is `bynk.resolve.name_conflict` (an embed is a value, in
/// the function namespace, and a type name in value position is an error).
/// `None` when the name is free. One rule for the project pipeline
/// (`symbols::build_unit_table`) and the single-file compile, which has no
/// unit table.
pub fn name_error(
    e: &EmbedDecl,
    prev_embed: Option<&Ident>,
    prev_asset: Option<&Ident>,
    prev_fn: Option<&Ident>,
    prev_type: Option<&Ident>,
) -> Option<CompileError> {
    let name = &e.name.name;
    if let Some(prev) = prev_embed {
        return Some(
            CompileError::new(
                "bynk.embed.duplicate",
                e.name.span,
                format!("embed `{name}` is already declared"),
            )
            .with_label(prev.span, "previously declared here"),
        );
    }
    let (kind, prev) = prev_asset
        .map(|p| ("an asset", p))
        .or_else(|| prev_fn.map(|p| ("a function", p)))
        .or_else(|| prev_type.map(|p| ("a type", p)))?;
    Some(
        CompileError::new(
            "bynk.resolve.name_conflict",
            e.name.span,
            format!("embed `{name}` conflicts with {kind} of the same name"),
        )
        .with_label(prev.span, "declared here"),
    )
}

/// The single-file compile's embed diagnostics, in declaration order: each
/// embed's path check and name rule ([`name_error`], against the file's own
/// assets, functions, types and earlier embeds), and, when `build` is set, its
/// refusal ([`needs_project_error`]). Placement needs no check here: a
/// single file is always a commons (an `adapter` is a parse error).
pub fn single_file_embed_errors(items: &[CommonsItem], build: bool) -> Vec<CompileError> {
    let mut fns: HashMap<&str, &Ident> = HashMap::new();
    let mut types: HashMap<&str, &Ident> = HashMap::new();
    let mut assets: HashMap<&str, &Ident> = HashMap::new();
    for item in items {
        match item {
            CommonsItem::Fn(f) => {
                if let FnName::Free(id) = &f.name {
                    fns.entry(id.name.as_str()).or_insert(id);
                }
            }
            CommonsItem::Type(t) => {
                types.entry(t.name.name.as_str()).or_insert(&t.name);
            }
            CommonsItem::Event(ev) => {
                types.entry(ev.name.name.as_str()).or_insert(&ev.name);
            }
            CommonsItem::Asset(a) => {
                assets.entry(a.name.name.as_str()).or_insert(&a.name);
            }
            _ => {}
        }
    }
    let mut embeds: HashMap<&str, &Ident> = HashMap::new();
    let mut errors = Vec::new();
    for item in items {
        let CommonsItem::Embed(e) = item else {
            continue;
        };
        errors.extend(path_errors(e));
        let key = e.name.name.as_str();
        match name_error(
            e,
            embeds.get(key).copied(),
            assets.get(key).copied(),
            fns.get(key).copied(),
            types.get(key).copied(),
        ) {
            Some(err) => errors.push(err),
            None => {
                embeds.insert(key, &e.name);
            }
        }
        if build {
            errors.push(needs_project_error(e));
        }
    }
    errors
}

/// Every embed visible in unit `name`: its own, plus those of each commons it
/// `uses` (one level, the same mixin rule [`crate::assets::compose_unit_assets`]
/// applies to assets). A local declaration of any kind wins over an imported
/// embed of the same name; two used commons declaring the same name is
/// `bynk.uses.name_conflict` (`phase_uses_name_conflicts`), so which of them is
/// kept here is never observable in a program that checks.
pub fn compose_unit_embeds(
    name: &str,
    unit_info: &BTreeMap<String, UnitInfo>,
) -> HashMap<String, EmbedDecl> {
    let Some(info) = unit_info.get(name) else {
        return HashMap::new();
    };
    let mut combined = info.table.embeds.clone();
    for used in &info.uses {
        let Some(used_info) = unit_info.get(used) else {
            continue;
        };
        // #2618: a used commons' `private` embeds stay in it.
        for (embed_name, decl) in used_info
            .table
            .embeds
            .iter()
            .filter(|(_, d)| d.private.is_none())
        {
            if crate::symbols::local_value_kind(&info.table, embed_name).is_some() {
                continue;
            }
            combined
                .entry(embed_name.clone())
                .or_insert_with(|| decl.clone());
        }
    }
    combined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_utf8_without_a_bom_or_carriage_returns() {
        assert_eq!(embedded_text(b"a\nb\n").unwrap(), "a\nb\n");
        // A leading byte-order mark is dropped; one anywhere else is text.
        assert_eq!(embedded_text(b"\xef\xbb\xbfhi").unwrap(), "hi");
        assert_eq!(
            embedded_text("a\u{feff}b".as_bytes()).unwrap(),
            "a\u{feff}b"
        );
        // CRLF and a bare CR both become LF; a trailing newline is kept.
        assert_eq!(embedded_text(b"a\r\nb\rc\r\n").unwrap(), "a\nb\nc\n");
        // An empty file is the empty string.
        assert_eq!(embedded_text(b"").unwrap(), "");
        assert_eq!(embedded_text("é\u{2028}".as_bytes()).unwrap(), "é\u{2028}");
    }

    #[test]
    fn non_text_is_refused_with_its_code() {
        assert_eq!(
            embedded_text(b"\xff\xfe"),
            Err("bynk.embed.invalid_encoding")
        );
        // Truncated multi-byte sequence.
        assert_eq!(
            embedded_text(b"\xe2\x82"),
            Err("bynk.embed.invalid_encoding")
        );
        assert_eq!(embedded_text(b"a\0b"), Err("bynk.embed.binary_content"));
        // Invalid UTF-8 is judged before NUL.
        assert_eq!(embedded_text(b"\xff\0"), Err("bynk.embed.invalid_encoding"));
    }
}
