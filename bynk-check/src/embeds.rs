//! #2387 (embedded text): the checker-side rules for `embed name = "path"`
//! declarations — the path rules, the name rule, the composition of a unit's
//! visible embeds through `uses` — mirroring [`crate::assets`] for the part an
//! embed shares with an asset.
//!
//! An embed is a project file's text compiled into the code as a `String`
//! constant. Unlike an asset it is never served, so it is private, and it
//! comes from the project's `embeds/` folder instead of `assets/`. The folder
//! listing, the file's content rules and its emission arrive in a later slice;
//! this one checks the declaration and refuses to build it.

use std::collections::{BTreeMap, HashMap};

use bynk_syntax::ast::{CommonsItem, EmbedDecl, FnName, Ident};
use bynk_syntax::error::CompileError;

use crate::project_model::UnitInfo;

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

/// `bynk.embed.not_yet_emitted` for `e`: this compiler checks an embed but
/// cannot yet build it. Raised by every build, project and single-file;
/// checking is unaffected. The slice that emits embeds removes the gate.
pub fn not_yet_emitted_error(e: &EmbedDecl) -> CompileError {
    CompileError::new(
        "bynk.embed.not_yet_emitted",
        e.span,
        format!(
            "embed `{}` cannot be built yet: this compiler checks `embed` declarations but does not emit them",
            e.name.name
        ),
    )
}

/// [`not_yet_emitted_error`] for every embed among `items` — the project
/// pipeline's build-mode gate, applied per parsed file.
pub fn not_yet_emitted_errors(items: &[CommonsItem]) -> Vec<CompileError> {
    items
        .iter()
        .filter_map(|item| match item {
            CommonsItem::Embed(e) => Some(not_yet_emitted_error(e)),
            _ => None,
        })
        .collect()
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
/// refusal ([`not_yet_emitted_error`]). Placement needs no check here: a
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
            errors.push(not_yet_emitted_error(e));
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
        for (embed_name, decl) in &used_info.table.embeds {
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
