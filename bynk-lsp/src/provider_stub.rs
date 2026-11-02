//! #2100 slice 4: generate a provider stub. For a capability declared in this
//! file that nothing in its context provides, the action scaffolds
//! `provides Cap = CapImpl { … }` right below the declaration, with every
//! operation's signature copied and its body a typed default.
//!
//! It is offered as a quick fix on `bynk.exports.capability_not_provided`
//! (an exported capability with no provider), and as a `refactor` with the
//! cursor on the capability's name. The Workers-only
//! `bynk.capability.not_provided` is never raised in the editor, whose
//! analysis runs as the bundle target, so the refactor is what covers a
//! required capability.
//!
//! The language has no placeholder expression, so a stub body must be a value
//! that type-checks (DECISION B re-checks the edit). Each body is a default of
//! the operation's return type: the unrefined scalars, `()`, `None`, `[]`, or
//! one of those inside an `Effect`. A capability with an operation returning
//! anything else (a record, a user sum, a `Result`) gets no action. So does a
//! derived capability, one declared outside a context, or one with generic
//! operations (whose type parameters the stub would have to repeat).
//!
//! Whether a capability already has a provider is read from the committed
//! index's implementation edges, so a provider in a sibling file counts. Like
//! fill-arms, the action is listed without an edit, and `codeAction/resolve`
//! computes and re-checks it. A `bynk-fmt`-canonical file is re-formatted and
//! the edit narrowed to the changed region.

use bynk_check::index::{ProjectIndex, SymbolKey, SymbolKind};
use bynk_syntax::ast::{BaseType, CapabilityDecl, CommonsItem, SourceUnit, TypeRef};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::*;

const DATA_TAG: &str = "bynk.generateProvider";
const CATEGORIES: [&str; 1] = ["bynk.exports.capability_not_provided"];

/// A planned stub: the capability, the provider's name, and the edits.
pub struct Plan {
    pub capability: String,
    pub provider: String,
    pub edits: Vec<(Span, String)>,
}

/// One capability the request points at, and how: through a "no provider"
/// diagnostic (a quick fix) or its declaration's name (a refactor).
pub struct Request {
    pub capability: String,
    pub quick_fix: bool,
}

/// The capabilities at `requested`, by diagnostic first, each named once.
pub fn requested(
    text: &str,
    diagnostics: &[bynk_ide::Diagnostic],
    requested: Span,
) -> Vec<Request> {
    let mut out: Vec<Request> = Vec::new();
    let touches = |s: Span| s.start <= requested.end && requested.start <= s.end;
    for d in diagnostics {
        if CATEGORIES.contains(&d.error.category) && touches(d.error.span) {
            let name = text
                .get(d.error.span.start..d.error.span.end)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !name.is_empty() && !out.iter().any(|r| r.capability == name) {
                out.push(Request {
                    capability: name,
                    quick_fix: true,
                });
            }
        }
    }
    if let Some(unit) = parse(text) {
        for cap in capabilities(&unit) {
            if touches(cap.name.span) && !out.iter().any(|r| r.capability == cap.name.name) {
                out.push(Request {
                    capability: cap.name.name.clone(),
                    quick_fix: false,
                });
            }
        }
    }
    out
}

/// The stub for `capability` in `text`, or `None` when there is none to offer
/// (see the module docs).
pub fn plan(text: &str, capability: &str, index: &ProjectIndex) -> Option<Plan> {
    let unit = parse(text)?;
    let SourceUnit::Context(ctx) = &unit else {
        return None;
    };
    let cap = capabilities(&unit)
        .into_iter()
        .find(|c| c.name.name == capability)?;
    if cap.derived.is_some() || cap.ops.is_empty() {
        return None;
    }
    let key = SymbolKey {
        unit: ctx.name.joined(),
        kind: SymbolKind::Capability,
        name: capability.to_string(),
    };
    if index.impls_of(&key).next().is_some() {
        return None;
    }
    // Names the context already declares anywhere (a sibling file's too,
    // from the committed index) are taken, as well as words in this file.
    let unit = ctx.name.joined();
    let declared: Vec<&str> = index
        .symbols
        .keys()
        .filter(|k| k.unit == unit)
        .map(|k| k.name.as_str())
        .collect();
    let provider = fresh_name(text, &declared, &format!("{capability}Impl"));

    let mut ops = Vec::new();
    for op in &cap.ops {
        if !op.type_params.is_empty() {
            return None;
        }
        let body = default_of(&op.return_type)?;
        let signature = text.get(op.span.start..op.return_type.span().end)?;
        if signature.contains("//") || !signature.trim_start().starts_with("fn") {
            return None;
        }
        ops.push(format!("  {} {{\n    {body}\n  }}", signature.trim()));
    }
    let stub = format!(
        "\n\nprovides {capability} = {provider} {{\n{}\n}}",
        ops.join("\n\n")
    );
    let mut edits = vec![(Span::new(cap.span.end, cap.span.end), stub)];

    let opts = bynk_fmt::FormatOptions::default();
    if bynk_fmt::format_source(text, &opts).is_ok_and(|f| f == text) {
        let stubbed = crate::fix_all::apply(text, &edits);
        let formatted = bynk_fmt::format_source(&stubbed, &opts).ok()?;
        edits = vec![crate::inline::changed_region(text, &formatted)];
    }
    Some(Plan {
        capability: capability.to_string(),
        provider,
        edits,
    })
}

fn parse(text: &str) -> Option<SourceUnit> {
    let tokens = tokenize(text).ok()?;
    match parse_unit_with_recovery(&tokens, text) {
        (Some(unit), errs) if errs.is_empty() => Some(unit),
        _ => None,
    }
}

fn capabilities(unit: &SourceUnit) -> Vec<&CapabilityDecl> {
    let SourceUnit::Context(ctx) = unit else {
        return Vec::new();
    };
    ctx.items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Capability(c) => Some(c),
            _ => None,
        })
        .collect()
}

/// A body sure to type-check as `ty` (the #852 `field_default_init` set plus
/// unit), looking through one `Effect` (an op body auto-lifts).
fn default_of(ty: &TypeRef) -> Option<&'static str> {
    let ty = match ty {
        TypeRef::Effect(inner, _) => inner,
        other => other,
    };
    Some(match ty {
        TypeRef::Base(BaseType::Int, _) => "0",
        TypeRef::Base(BaseType::Float, _) => "0.0",
        TypeRef::Base(BaseType::Decimal, _) => "0d",
        TypeRef::Base(BaseType::String, _) => "\"\"",
        TypeRef::Base(BaseType::Bool, _) => "false",
        TypeRef::Unit(_) => "()",
        TypeRef::Option(..) => "None",
        TypeRef::List(..) => "[]",
        _ => return None,
    })
}

/// `base`, or `base2`, `base3`, … — the first that is neither a word of
/// `text` nor one of the context's `declared` names.
fn fresh_name(text: &str, declared: &[&str], base: &str) -> String {
    let taken = |name: &str| {
        declared.contains(&name)
            || text.match_indices(name).any(|(i, _)| {
                let before = text[..i].chars().next_back();
                let after = text[i + name.len()..].chars().next();
                !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                    && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
            })
    };
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|name| !taken(name))
        .expect("an unused name")
}

/// The listed (unresolved) action for `plan`.
pub fn listed(
    plan: &Plan,
    quick_fix: bool,
    uri: &Url,
    version: Option<i32>,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title: format!(
            "Generate provider `{}` for `{}`",
            plan.provider, plan.capability
        ),
        kind: Some(if quick_fix {
            CodeActionKind::QUICKFIX
        } else {
            CodeActionKind::REFACTOR
        }),
        data: Some(serde_json::json!({
            "kind": DATA_TAG,
            "uri": uri.as_str(),
            "version": version,
            "capability": plan.capability,
        })),
        ..Default::default()
    })
}

/// What a listed stub action's `data` names.
pub struct Target {
    pub uri: Url,
    pub version: Option<i32>,
    pub capability: String,
}

impl Target {
    pub fn of(action: &CodeAction) -> Option<Self> {
        let data = action.data.as_ref()?;
        if data.get("kind")?.as_str()? != DATA_TAG {
            return None;
        }
        Some(Target {
            uri: Url::parse(data.get("uri")?.as_str()?).ok()?,
            version: data
                .get("version")
                .and_then(serde_json::Value::as_i64)
                .and_then(|v| i32::try_from(v).ok()),
            capability: data.get("capability")?.as_str()?.to_string(),
        })
    }
}
