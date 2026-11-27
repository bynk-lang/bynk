//! Typed HTML templates (typed-HTML track, spine #3927, slice 1; the
//! template-declaration and context-model ADRs).
//!
//! `template name: F = "embeds/…"` is an embed whose text is HTML. The build and
//! the editor read its file as they read an embed's, and this module checks it:
//! the markup is tokenised ([`crate::html_template`]), each `\(name)`
//! placeholder must name a field of the declared record `F` whose type its
//! context accepts, every field must be used, and a record that carries a
//! secret is refused. A template that passes has a [`Plan`]: its literal pieces
//! and, for each placeholder, the field and the escape it gets, which the
//! emitter turns into a function of the fields (§8).
//!
//! The declaration, its fields type and every form that makes an `Html` are
//! experimental, behind the single gate `bynk.html.typed` (§14).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use bynk_syntax::ast::{BaseType, CommonsItem, EmbedDecl, Ident, TypeBody, TypeDecl, TypeRef};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::html_template::{self, At, Context};
use crate::project_model::ErrorSink;
use crate::symbols::UnitTable;
use bynk_project::ParsedFile;

/// The `[unstable] allow` entry that admits typed HTML.
pub const GATE: &str = "bynk.html.typed";
/// The synthetic path a `template` declaration is checked under (§14).
pub const TEMPLATE_ITEM: &str = "bynk.html.typed.template";
/// `bynk.html`'s `Html`, as an item path.
pub const HTML_ITEM: &str = "bynk.html.Html";

pub const AT_BOUNDARY: &str = "bynk.html.at_boundary";
pub const UNKNOWN_PLACEHOLDER: &str = "bynk.template.unknown_placeholder";
pub const UNUSED_FIELD: &str = "bynk.template.unused_field";
pub const AMBIGUOUS_PLACEHOLDER: &str = "bynk.template.ambiguous_placeholder";
pub const CONTEXT_TYPE: &str = "bynk.template.context_type";
pub const HTML_IN_RCDATA: &str = "bynk.template.html_in_rcdata";
pub const SECRET_PLACEHOLDER: &str = "bynk.template.secret_placeholder";
pub const FIELDS_NOT_RECORD: &str = "bynk.template.fields_not_record";
pub const NEEDS_BYNK_HTML: &str = "bynk.template.needs_bynk_html";
pub const PLACEHOLDER_WITHOUT_FIELDS: &str = "bynk.template.placeholder_without_fields";

/// Whether `decl` is `bynk.html`'s `Html`, found by where it is declared (as
/// [`crate::context_checks::secret_kind`] finds a secret), so a project's own
/// `type Html` is never mistaken for it.
pub fn is_html_decl(decl: &TypeDecl) -> bool {
    if !matches!(decl.body, TypeBody::Opaque { .. }) || decl.name.name != "Html" {
        return false;
    }
    crate::firstparty::unit(bynk_std::HTML_UNIT)
        .is_some_and(|unit| decl.span.file == crate::project_model::firstparty_file_id(unit))
}

/// Whether the type named `name` in `types` is `bynk.html`'s `Html`.
pub fn is_html_type(name: &str, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    types.get(name).is_some_and(|d| is_html_decl(d))
}

/// Whether `name` in `types` is `Html`, or holds one at any depth in a record
/// field or a variant's payload: a type that is not data at a boundary
/// (`bynk.html.at_boundary`).
pub fn type_carries_html(name: &str, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    fn walk(
        name: &str,
        types: &HashMap<String, Arc<TypeDecl>>,
        seen: &mut BTreeSet<String>,
    ) -> bool {
        let Some(decl) = types.get(name) else {
            return false;
        };
        if is_html_decl(decl) {
            return true;
        }
        if !seen.insert(name.to_string()) {
            return false;
        }
        let fields: Vec<&TypeRef> = match &decl.body {
            TypeBody::Record(r) => r.fields.iter().map(|f| &f.type_ref).collect(),
            TypeBody::Sum(s) => s
                .variants
                .iter()
                .flat_map(|v| v.payload.iter().map(|f| &f.type_ref))
                .collect(),
            _ => Vec::new(),
        };
        fields.into_iter().any(|f| {
            bynk_syntax::ast::type_ref_heads(f)
                .iter()
                .any(|h| walk(h, types, seen))
        })
    }
    walk(name, types, &mut BTreeSet::new())
}

/// `bynk.html.at_boundary`: a type that is or holds `Html`, written at `id` in `what`.
pub fn html_at_boundary(id: &Ident, what: &str) -> CompileError {
    CompileError::new(
        AT_BOUNDARY,
        id.span,
        format!("`{}` cannot appear in {what}: `Html` is not data at a boundary", id.name),
    )
    .with_note("markup from outside would skip the compiler's escaping, so `Html` is never decoded, stored, received or sent; send its `toString()`, or keep the text and build the markup where it is shown")
}

/// `bynk.html.at_boundary` for a `Json`, `Yaml` or `Form` codec over `t`, when it
/// is or holds `Html` (at any depth of a container, record or sum).
pub fn html_codec_error(
    t: crate::checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &crate::checker::Types,
    span: Span,
) -> Option<CompileError> {
    carried_html_name(t, types, tys)?;
    Some(
        CompileError::new(
            AT_BOUNDARY,
            span,
            format!("`{}` is or holds `Html`, which no codec encodes or decodes", t.display(tys)),
        )
        .with_note("markup from outside would skip the compiler's escaping, so `Html` is never decoded, stored, received or sent; encode its `toString()` instead"),
    )
}

/// The named type in `t` that is or holds `Html`, if any.
fn carried_html_name(
    t: crate::checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &crate::checker::Types,
) -> Option<String> {
    use crate::checker::Ty;
    match &*tys.get(t) {
        Ty::Named { name, args, .. } => {
            if type_carries_html(name, types) {
                Some(name.clone())
            } else {
                args.iter().find_map(|a| carried_html_name(*a, types, tys))
            }
        }
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => carried_html_name(*a, types, tys),
        Ty::Result(a, b) | Ty::Map(a, b) => {
            carried_html_name(*a, types, tys).or_else(|| carried_html_name(*b, types, tys))
        }
        _ => None,
    }
}

/// How a field's type meets a context.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FieldKind {
    /// A scalar, or a refinement of one: the base type decides its text.
    Scalar(BaseType),
    Html,
    Secret,
    /// Anything else, by its written name.
    Other(String),
}

fn is_scalar(b: BaseType) -> bool {
    matches!(
        b,
        BaseType::String
            | BaseType::Int
            | BaseType::Float
            | BaseType::Bool
            | BaseType::Decimal
            | BaseType::Duration
            | BaseType::Instant
    )
}

fn field_kind(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> FieldKind {
    match r {
        TypeRef::Base(b, _) if is_scalar(*b) => FieldKind::Scalar(*b),
        TypeRef::Named(id) => match types.get(&id.name) {
            Some(d) if is_html_decl(d) => FieldKind::Html,
            Some(d) if crate::context_checks::secret_kind(d).is_some() => FieldKind::Secret,
            Some(d) => match &d.body {
                TypeBody::Refined { base, .. } if is_scalar(*base) => FieldKind::Scalar(*base),
                _ => FieldKind::Other(id.name.clone()),
            },
            // A secret kind a unit `consumes` from `bynk` is not in the view the template is
            // checked in; its name alone is enough, since a type of the unit's own would be.
            None if matches!(id.name.as_str(), "Secret" | "Authorization" | "ApiKeyValue") => {
                FieldKind::Secret
            }
            None => FieldKind::Other(id.name.clone()),
        },
        other => FieldKind::Other(bynk_syntax::print::type_ref_to_string(other)),
    }
}

/// What a placeholder's value becomes in the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Treatment {
    /// A scalar, written as its text and escaped with `escapeText` (C1, C2).
    Text(BaseType),
    /// A scalar, written as its text and escaped with `escapeAttr` (C3).
    Attr(BaseType),
    /// An `Html` value, inserted as is (C1).
    Html,
}

/// One placeholder of a checked template: the field it reads and its treatment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanHole {
    pub field: String,
    pub treatment: Treatment,
}

/// A checked template: its literal pieces, verbatim, with a hole between each
/// two. `pieces.len() == holes.len() + 1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub pieces: Vec<String>,
    pub holes: Vec<PlanHole>,
}

/// What a template is checked against: the types and the asset names in scope
/// in its unit.
pub struct Scope<'a> {
    pub types: &'a HashMap<String, Arc<TypeDecl>>,
    pub assets: &'a BTreeSet<String>,
}

/// Where a template diagnostic is: in the `.bynk` file that declares the
/// template, or in the template file itself.
#[derive(Debug, Clone)]
pub enum Located {
    Declaration(CompileError),
    Template(CompileError),
}

/// The fields record a template declares, or why it has none that works.
fn fields_record<'a>(
    e: &EmbedDecl,
    fields: &TypeRef,
    types: &'a HashMap<String, Arc<TypeDecl>>,
) -> Result<&'a Arc<TypeDecl>, CompileError> {
    let not_record = |what: String| {
        CompileError::new(
            FIELDS_NOT_RECORD,
            fields.span(),
            format!("template `{}`'s fields type {what}", e.name.name),
        )
        .with_note("a template's fields are a record type declared in Bynk, such as `type PageFields = { title: String }`, which `fill` takes whole")
    };
    let TypeRef::Named(id) = fields else {
        return Err(not_record(format!(
            "`{}` is not a record",
            bynk_syntax::print::type_ref_to_string(fields)
        )));
    };
    let Some(decl) = types.get(&id.name) else {
        return Err(CompileError::new(
            "bynk.resolve.unknown_type",
            id.span,
            format!("unknown type `{}`", id.name),
        ));
    };
    if !matches!(decl.body, TypeBody::Record(_)) || !decl.type_params.is_empty() {
        return Err(not_record(format!(
            "`{}` is not a non-generic record",
            id.name
        )));
    }
    Ok(decl)
}

fn secret_refusal(e: &EmbedDecl, fields: &TypeRef, record: &TypeDecl) -> CompileError {
    CompileError::new(
        SECRET_PLACEHOLDER,
        fields.span(),
        format!(
            "template `{}`'s fields `{}` carry a secret, which can never reach a page",
            e.name.name, record.name.name
        ),
    )
    .with_note("`Secret`, `Authorization` and `ApiKeyValue` have no string form and are refused in every context (ADR 1038); a page shows a secret only through an explicit `reveal()`")
}

/// The record's fields, by name, with their types and name spans.
fn record_fields(record: &TypeDecl) -> Vec<(String, TypeRef, Span)> {
    match &record.body {
        TypeBody::Record(r) => r
            .fields
            .iter()
            .map(|f| (f.name.name.clone(), f.type_ref.clone(), f.name.span))
            .collect(),
        _ => Vec::new(),
    }
}

fn nearest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .map(|c| (crate::exports::strsim(name, c), c))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

fn refusal_error(r: &html_template::Refusal, span: Span) -> CompileError {
    let mut err = CompileError::new(r.code, span, r.message.clone());
    if let Some(note) = &r.note {
        err = err.with_note(note.clone());
    }
    err
}

/// Check one template's text against its declaration, in `scope`. The plan,
/// when the template passes, and every diagnostic either way.
pub fn check_template(
    e: &EmbedDecl,
    text: &str,
    scope: &Scope<'_>,
) -> (Option<Plan>, Vec<Located>) {
    let mut out = Vec::new();
    let Some(head) = &e.template else {
        return (None, out);
    };
    if !scope.types.values().any(|d| is_html_decl(d)) {
        out.push(Located::Declaration(
            CompileError::new(
                NEEDS_BYNK_HTML,
                e.name.span,
                format!("template `{}` needs `uses bynk.html`", e.name.name),
            )
            .with_note("a template's value is `Html`, which `bynk.html` declares; add `uses bynk.html` to this unit"),
        ));
        return (None, out);
    }
    let mut fields: Vec<(String, TypeRef, Span)> = Vec::new();
    if let Some(ty) = &head.fields {
        match fields_record(e, ty, scope.types) {
            Ok(record) => {
                if crate::context_checks::type_carries_secret(&record.name.name, scope.types) {
                    out.push(Located::Declaration(secret_refusal(e, ty, record)));
                    return (None, out);
                }
                fields = record_fields(record);
            }
            Err(err) => {
                out.push(Located::Declaration(err));
                return (None, out);
            }
        }
    }
    let analysed = html_template::analyse_template(text);
    for r in &analysed.analysis.markup {
        let at = match r.at {
            At::Offset(o) => Span::new(
                o,
                o + text[o.min(text.len())..]
                    .chars()
                    .next()
                    .map_or(0, char::len_utf8),
            ),
            At::Hole(h) => {
                let p = &analysed.split.placeholders[h];
                Span::new(p.start, p.end)
            }
        };
        out.push(Located::Template(
            refusal_error(r, at).with_label(e.path_span, "in this template"),
        ));
    }
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut holes = Vec::new();
    for (n, p) in analysed.split.placeholders.iter().enumerate() {
        let span = Span::new(p.start, p.end);
        let field = fields.iter().find(|(f, _, _)| *f == p.name);
        let asset = scope.assets.contains(&p.name);
        if field.is_some() {
            used.insert(p.name.clone());
        }
        if head.fields.is_none() {
            out.push(Located::Template(
                CompileError::new(
                    PLACEHOLDER_WITHOUT_FIELDS,
                    span,
                    format!("template `{}` has no fields, so `\\({})` fills nothing", e.name.name, p.name),
                )
                .with_label(e.name.span, "declared without a fields type")
                .with_note(format!("declare the fields, `template {}: Fields = \"…\"`, or write `&#92;({})` for the text itself", e.name.name, p.name)),
            ));
            continue;
        }
        if field.is_some() && asset {
            out.push(Located::Template(
                CompileError::new(
                    AMBIGUOUS_PLACEHOLDER,
                    span,
                    format!("`\\({})` names both a field and an asset in scope", p.name),
                )
                .with_label(e.name.span, "in this template")
                .with_note(
                    "rename the field or the asset, so the placeholder can only mean one of them",
                ),
            ));
            continue;
        }
        let Some((_, ty, field_span)) = field else {
            let mut err = CompileError::new(
                UNKNOWN_PLACEHOLDER,
                span,
                format!(
                    "`\\({})` names no field of template `{}`",
                    p.name, e.name.name
                ),
            )
            .with_label(e.name.span, "in this template");
            if asset {
                err = err.with_note(format!("`{}` is an asset; an asset fills only a code-loading attribute, such as `<script src>`", p.name));
            } else if let Some(near) = nearest(&p.name, fields.iter().map(|(f, _, _)| f.as_str())) {
                err = err.with_note(format!("did you mean `{near}`?"));
            }
            out.push(Located::Template(err));
            continue;
        };
        let context = match analysed.analysis.holes.get(n).cloned().flatten() {
            Some(Ok(c)) => c,
            Some(Err(r)) => {
                let mut err = refusal_error(&r, span).with_label(e.name.span, "in this template");
                if matches!(
                    r.code,
                    html_template::tokeniser::IN_SCRIPT
                        | html_template::tokeniser::IN_STYLE
                        | html_template::tokeniser::IN_RAW_TEXT
                ) {
                    err = err.with_note(format!(
                        "if `\\({})` here is text, not a placeholder, rewrite it so it is not a backslash, `(`, a name and `)`",
                        p.name
                    ));
                }
                out.push(Located::Template(err));
                continue;
            }
            // Not judged: the markup was refused before it.
            None => continue,
        };
        match treatment(&context, &field_kind(ty, scope.types)) {
            Ok(t) => holes.push(PlanHole {
                field: p.name.clone(),
                treatment: t,
            }),
            Err((code, message, note)) => out.push(Located::Template(
                CompileError::new(code, span, message)
                    .with_note(note)
                    .with_label(*field_span, "the field is declared here"),
            )),
        }
    }
    for (name, _, span) in &fields {
        if !used.contains(name) {
            out.push(Located::Declaration(
                CompileError::new(
                    UNUSED_FIELD,
                    e.name.span,
                    format!(
                        "field `{name}` is used by no placeholder of template `{}`",
                        e.name.name
                    ),
                )
                .with_label(*span, "the field is declared here")
                .with_note(format!(
                    "add `\\({name})` to `{}`, or remove the field",
                    e.path
                )),
            ));
        }
    }
    if !out.is_empty() {
        return (None, out);
    }
    let pieces = analysed.split.pieces.into_iter().map(|(_, s)| s).collect();
    (Some(Plan { pieces, holes }), out)
}

type Mismatch = (&'static str, String, String);

/// The treatment a field of `kind` gets in `context`, or why it is refused.
fn treatment(context: &Context, kind: &FieldKind) -> Result<Treatment, Mismatch> {
    let shown = match kind {
        FieldKind::Scalar(b) => b.name().to_string(),
        FieldKind::Html => "Html".to_string(),
        FieldKind::Secret => "a secret".to_string(),
        FieldKind::Other(n) => n.clone(),
    };
    const SCALARS: &str =
        "`String`, `Int`, `Float`, `Bool`, `Decimal`, `Duration`, `Instant` or a refinement of one";
    match (context, kind) {
        (_, FieldKind::Secret) => Err((
            SECRET_PLACEHOLDER,
            "a secret can never reach a page".to_string(),
            "`Secret`, `Authorization` and `ApiKeyValue` are refused in every context (ADR 1038)".to_string(),
        )),
        (Context::Text, FieldKind::Scalar(b)) | (Context::SelectText, FieldKind::Scalar(b)) => {
            Ok(Treatment::Text(*b))
        }
        (Context::Rcdata { .. }, FieldKind::Scalar(b)) => Ok(Treatment::Text(*b)),
        (Context::Attr { .. }, FieldKind::Scalar(b)) => Ok(Treatment::Attr(*b)),
        (Context::Text, FieldKind::Html) => Ok(Treatment::Html),
        (Context::Rcdata { element }, FieldKind::Html) => Err((
            HTML_IN_RCDATA,
            format!("an `Html` value cannot be the text of `<{element}>`"),
            format!("the text of `<{element}>` is never parsed as markup, so it takes a scalar, escaped as text; pass the text instead"),
        )),
        (Context::SelectText, FieldKind::Html) => Err((
            CONTEXT_TYPE,
            "an `Html` value cannot be inside `<select>`".to_string(),
            "inside `<select>` a browser ignores most tags, so markup could change how the rest of the page is read; pass the text instead".to_string(),
        )),
        (Context::Attr { attr, .. }, FieldKind::Html) => Err((
            CONTEXT_TYPE,
            format!("an `Html` value cannot be the value of `{attr}`"),
            format!("an attribute value takes a scalar ({SCALARS}), escaped for an attribute; markup is not text"),
        )),
        (Context::Text, FieldKind::Other(_)) => Err((
            CONTEXT_TYPE,
            format!("`{shown}` cannot fill element text"),
            format!("element text takes {SCALARS}, escaped as text, or `Html`, inserted as is"),
        )),
        (_, FieldKind::Other(_)) => Err((
            CONTEXT_TYPE,
            format!("`{shown}` cannot fill this placeholder"),
            format!("it takes {SCALARS}, escaped for where it sits"),
        )),
    }
}

/// The asset names visible in `unit`: its own and those of the commons it
/// `uses` (a used commons' `private` assets stay in it).
fn visible_assets(
    unit: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    if let Some(t) = unit_tables.get(unit) {
        out.extend(t.assets.keys().cloned());
    }
    for used in unit_uses.get(unit).into_iter().flatten() {
        if let Some(t) = unit_tables.get(used) {
            out.extend(
                t.assets
                    .iter()
                    .filter(|(_, d)| d.private.is_none())
                    .map(|(n, _)| n.clone()),
            );
        }
    }
    out
}

/// Every checked template, by declaring unit and name.
pub type Plans = BTreeMap<(String, String), Plan>;

/// The project's units as a template check needs them.
pub struct Units<'a> {
    pub parsed: &'a [ParsedFile],
    pub tables: &'a HashMap<String, UnitTable>,
    pub uses: &'a HashMap<String, Vec<String>>,
}

/// Check every template the project declares against its file's text (keyed by
/// normalised path, as `embeds::read_declared_embeds` reads it), and the gate.
/// A template whose text was not read (a missing or refused file, reported
/// already) is skipped. Each template file read is added to `snapshots` under
/// its path, so a renderer can show a diagnostic at its line.
pub fn phase_templates(
    units: &Units<'_>,
    texts: &BTreeMap<String, String>,
    policy: &crate::stability::StabilityPolicy,
    errors: &mut ErrorSink,
) -> Plans {
    let mut plans = Plans::new();
    for pf in units.parsed {
        for item in pf.items() {
            let CommonsItem::Embed(e) = item else {
                continue;
            };
            let Some(head) = &e.template else {
                continue;
            };
            if !pf.is_synthetic()
                && let Some(err) = policy.check(TEMPLATE_ITEM, head.keyword)
            {
                errors.push_for(Some(&pf.identity_path()), err);
            }
            if crate::embeds::path_errors(e).is_some() {
                continue;
            }
            let key = bynk_project::assets::normalise_asset_path(&e.path);
            let Some(text) = texts.get(&key) else {
                continue;
            };
            let unit = pf.unit().name().joined();
            let types = crate::symbols::combined_types_for(&unit, units.tables, units.uses);
            let assets = visible_assets(&unit, units.tables, units.uses);
            let scope = Scope {
                types: &types,
                assets: &assets,
            };
            let (plan, diagnostics) = check_template(e, text, &scope);
            for d in diagnostics {
                match d {
                    Located::Declaration(err) => errors.push_for(Some(&pf.identity_path()), err),
                    Located::Template(err) => errors.push_for(Some(Path::new(&key)), err),
                }
            }
            if let Some(plan) = plan {
                plans.insert((unit, e.name.name.clone()), plan);
            }
        }
    }
    plans
}

/// The texts of the declared template files, as the editor reads them: from
/// the overlay (an unsaved buffer) or the disk, under the embed size cap. Only
/// files a `template` declaration names are read.
pub fn read_template_texts(
    project_root: &Path,
    parsed: &[ParsedFile],
    overlay: &HashMap<std::path::PathBuf, String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for pf in parsed {
        for item in pf.items() {
            let CommonsItem::Embed(e) = item else {
                continue;
            };
            if !e.is_template() || crate::embeds::path_errors(e).is_some() {
                continue;
            }
            let key = bynk_project::assets::normalise_asset_path(&e.path);
            if out.contains_key(&key) {
                continue;
            }
            let abs = project_root.join(&key);
            let bytes = match overlay.get(&abs) {
                Some(text) => text.clone().into_bytes(),
                None => match std::fs::read(&abs) {
                    Ok(b) => b,
                    Err(_) => continue,
                },
            };
            if bytes.len() > crate::embeds::MAX_EMBED_BYTES {
                continue;
            }
            if let Ok(text) = crate::embeds::embedded_text(&bytes) {
                out.insert(key, text);
            }
        }
    }
    out
}

/// A template's name as a placeholder-less value, or a `.fill` receiver: the
/// declaration, when `name` names a template in `embeds`.
pub fn template_named<'a>(
    name: &Ident,
    embeds: &'a HashMap<String, EmbedDecl>,
) -> Option<&'a EmbedDecl> {
    embeds.get(&name.name).filter(|e| e.is_template())
}

/// The hidden type names written in a unit (`resolver::check_hidden_type_names`)
/// and, typed HTML (#3927, §14), its uses of `bynk.html`'s experimental types
/// while the project's policy blocks them.
pub fn hidden_or_gated_types(
    resolved: &crate::resolver::ResolvedCommons,
    hidden: &BTreeMap<String, String>,
) -> Vec<CompileError> {
    let items = &resolved.commons.items;
    let mut out = crate::resolver::check_hidden_type_names(items, hidden);
    let policy = &resolved.stability.policy;
    let gated: BTreeMap<String, String> = resolved
        .types
        .iter()
        .filter(|(_, d)| is_html_decl(d))
        .map(|(name, _)| (name.clone(), HTML_ITEM.to_string()))
        .filter(|(_, path)| policy.blocks(path))
        .collect();
    if !gated.is_empty() {
        out.extend(crate::resolver::check_gated_type_names(
            items, &gated, policy,
        ));
    }
    out
}

/// What [`editor_phase`] reads: the project root, the editor's overlay, the parsed files, the
/// unit tables and each unit's `uses`.
pub type EditorUnits<'a> = (
    &'a Path,
    &'a HashMap<std::path::PathBuf, String>,
    &'a [ParsedFile],
    &'a HashMap<String, UnitTable>,
    &'a HashMap<String, Vec<String>>,
);

/// The editor's embed and template phase (`analysis::analyse_project_inner`): every declared embed
/// names a file in `embeds/` (`embeds::phase_embed_files`), then each template's text is read from
/// the overlay or the disk ([`read_template_texts`]) and checked ([`phase_templates`]), and joins
/// `snapshots`, so the language server publishes its diagnostics at the template's own line.
pub fn editor_phase(
    (project_root, overlay, parsed, tables, uses): EditorUnits<'_>,
    policy: &crate::stability::StabilityPolicy,
    snapshots: &mut Vec<(std::path::PathBuf, String)>,
    errors: &mut ErrorSink,
) {
    let available = bynk_project::assets::list_embed_files(project_root).ok();
    crate::embeds::phase_embed_files(parsed, available.as_ref(), errors);
    let texts = read_template_texts(project_root, parsed, overlay);
    let units = Units {
        parsed,
        tables,
        uses,
    };
    phase_templates(&units, &texts, policy, errors);
    for (key, text) in texts {
        let path = std::path::PathBuf::from(&key);
        if !snapshots.iter().any(|(p, _)| *p == path) {
            snapshots.push((path, text));
        }
    }
}
