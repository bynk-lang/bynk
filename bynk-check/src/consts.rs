//! #2617 (named constants): the checker-side rules for `const Name: Type =
//! <static expression>` declarations — the name rules, the closed set of
//! static initialisers (ADR 0004, plus references to other constants), the
//! cycle rule and the composition of a unit's visible constants through
//! `uses`.
//!
//! A constant is a value in its unit's single value namespace (spec §5.1),
//! like a function, an asset or an embed: a unit that `uses` a commons sees
//! its constants, and a sibling file of the same unit imports them as it
//! imports a function. Its type is its annotation, resolved where it is used.
//! It emits as a hoisted, cached function (spec §7.3.1a), so neither the
//! order of a file's constants nor the order in which a unit's file modules
//! evaluate matters.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use bynk_syntax::ast::{
    CommonsItem, ConstDecl, Expr, ExprKind, FnName, Ident, TypeBody, TypeDecl, UnaryOp,
};
use bynk_syntax::error::CompileError;

use crate::project_model::UnitInfo;

/// Whether `name` is upper-camel case: the lexer's `constant_name` class,
/// `/[A-Z][A-Za-z0-9_]*/`.
pub fn is_constant_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `bynk.resolve.const_name_case` for `c`, when its name is not upper-camel.
pub fn name_case_error(c: &ConstDecl) -> Option<CompileError> {
    if is_constant_name(&c.name.name) {
        return None;
    }
    let mut err = CompileError::new(
        "bynk.resolve.const_name_case",
        c.name.span,
        format!(
            "constant `{}` must be named in upper-camel case",
            c.name.name
        ),
    )
    .with_note(
        "a constant's name starts with a capital letter and continues with letters, digits or `_` (`MaxRetries`)",
    );
    if let Some(suggested) = upper_camel(&c.name.name) {
        err = err.with_note(format!("rename it `{suggested}`"));
    }
    Some(err)
}

/// A best-effort upper-camel spelling of `name` (`max_retries` → `MaxRetries`),
/// or `None` when that gives nothing usable.
fn upper_camel(name: &str) -> Option<String> {
    let mut out = String::new();
    let mut upper_next = true;
    for c in name.chars() {
        if c == '_' || c == '-' {
            upper_next = true;
        } else if upper_next {
            out.extend(c.to_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    is_constant_name(&out).then_some(out)
}

/// `bynk.resolve.const_outside_context_or_commons` for `c`.
pub fn placement_error(c: &ConstDecl) -> CompileError {
    CompileError::new(
        "bynk.resolve.const_outside_context_or_commons",
        c.span,
        "`const` declarations are only allowed inside a context or a commons",
    )
    .with_note("an adapter is the host boundary; declare the constant in a commons or a context")
}

/// The name rule for constant `c`, given what its unit already declares under
/// that name: an earlier constant is `bynk.resolve.duplicate_const`; an embed,
/// an asset, a free function or a type is `bynk.resolve.name_conflict` (a
/// constant is a value, in the same namespace as a free function). `None`
/// when the name is free. One rule for the project pipeline
/// (`symbols::build_unit_table`) and the single-file compile.
pub fn name_error(
    c: &ConstDecl,
    prev_const: Option<&Ident>,
    prev_embed: Option<&Ident>,
    prev_asset: Option<&Ident>,
    prev_fn: Option<&Ident>,
    prev_type: Option<&Ident>,
) -> Option<CompileError> {
    let name = &c.name.name;
    if let Some(prev) = prev_const {
        return Some(
            CompileError::new(
                "bynk.resolve.duplicate_const",
                c.name.span,
                format!("constant `{name}` is already declared"),
            )
            .with_label(prev.span, "previously declared here"),
        );
    }
    let (kind, prev) = prev_embed
        .map(|p| ("an embed", p))
        .or_else(|| prev_asset.map(|p| ("an asset", p)))
        .or_else(|| prev_fn.map(|p| ("a function", p)))
        .or_else(|| prev_type.map(|p| ("a type", p)))?;
    Some(
        CompileError::new(
            "bynk.resolve.name_conflict",
            c.name.span,
            format!("constant `{name}` conflicts with {kind} of the same name"),
        )
        .with_label(prev.span, "declared here"),
    )
}

/// `bynk.resolve.name_conflict` for a constant named like a built-in type
/// (`List`, `Map`): the parser reads such a name as the type in every later
/// reference, so the constant could never be used (the same reservation
/// `bynk.resolve.reserved_builtin_type` makes for a `type` declaration).
pub fn builtin_name_error(c: &ConstDecl) -> Option<CompileError> {
    bynk_syntax::keywords::is_builtin_type_name(&c.name.name).then(|| {
        CompileError::new(
            "bynk.resolve.name_conflict",
            c.name.span,
            format!(
                "constant `{}` conflicts with the built-in type of the same name",
                c.name.name
            ),
        )
        .with_note("choose another name for the constant")
    })
}

/// `bynk.resolve.name_conflict` for a constant that shares its name with a
/// variant of a sum type in scope, or with a predeclared `HttpResult` variant:
/// a bare `Name` would read as either, so the constant may not take the name.
pub fn variant_conflict_error(
    c: &ConstDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<CompileError> {
    let mut owners: Vec<&TypeDecl> = types
        .values()
        .map(Arc::as_ref)
        .filter(|t| {
            matches!(&t.body, TypeBody::Sum(s)
                if s.variants.iter().any(|v| v.name.name == c.name.name))
        })
        .collect();
    owners.sort_by(|a, b| a.name.name.cmp(&b.name.name));
    // A predeclared `HttpResult` variant (`NotFound`, `Accepted`) is a bare
    // name in every unit.
    let owner = match owners.first() {
        Some(t) => t.name.name.as_str(),
        None if bynk_syntax::ast::http_variant(&c.name.name).is_some() => "HttpResult",
        None => return None,
    };
    Some(
        CompileError::new(
            "bynk.resolve.name_conflict",
            c.name.span,
            format!(
                "constant `{}` conflicts with a variant of `{owner}`",
                c.name.name
            ),
        )
        .with_note("a bare name would read as either; choose another name for the constant"),
    )
}

/// The first part of `init` outside the closed set of static initialisers
/// (decision A of #2617, after ADR 0004), as `bynk.resolve.bad_const_initialiser`
/// on that part. `consts` are the constants in scope (a reference to one is
/// static); `types` let a variant constructor be told from a function call.
pub fn static_error(
    c: &ConstDecl,
    consts: &HashMap<String, ConstDecl>,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<CompileError> {
    let (span, what) = first_non_static(&c.init, consts, types)?;
    Some(
        CompileError::new(
            "bynk.resolve.bad_const_initialiser",
            span,
            format!(
                "the initialiser of constant `{}` must be a static value, but {what} is not",
                c.name.name
            ),
        )
        .with_note(
            "a constant's initialiser is a literal (a string without interpolation, or a negative \
             number), a `Duration` literal, a record, list or sum-variant construction over static \
             values, `Some`/`None`/`Ok`/`Err`, `T.unsafe(lit)` for an opaque type, \
             `List.empty()`/`Map.empty()`/`Set.empty()`, or another constant",
        ),
    )
}

fn sum_has_variant(types: &HashMap<String, Arc<TypeDecl>>, ty: &str, variant: &str) -> bool {
    types.get(ty).is_some_and(|t| {
        matches!(&t.body, TypeBody::Sum(s) if s.variants.iter().any(|v| v.name.name == variant))
    })
}

fn is_any_variant(types: &HashMap<String, Arc<TypeDecl>>, variant: &str) -> bool {
    types.values().any(|t| {
        matches!(&t.body, TypeBody::Sum(s) if s.variants.iter().any(|v| v.name.name == variant))
    })
}

fn is_literal(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::StrLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::UnitLit => true,
        ExprKind::UnaryOp(UnaryOp::Neg, inner) => {
            matches!(
                inner.kind,
                ExprKind::IntLit { .. } | ExprKind::FloatLit { .. }
            )
        }
        ExprKind::Paren(inner) => is_literal(inner),
        _ => false,
    }
}

/// The span and a short description of the first non-static part of `e`.
fn first_non_static(
    e: &Expr,
    consts: &HashMap<String, ConstDecl>,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<(bynk_syntax::span::Span, &'static str)> {
    let all = |items: std::slice::Iter<'_, Expr>| {
        items
            .map(|x| first_non_static(x, consts, types))
            .find(Option::is_some)
            .flatten()
    };
    match &e.kind {
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::DurationLit { .. }
        | ExprKind::StrLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::UnitLit
        | ExprKind::None => None,
        ExprKind::UnaryOp(UnaryOp::Neg, inner)
            if matches!(
                inner.kind,
                ExprKind::IntLit { .. } | ExprKind::FloatLit { .. }
            ) =>
        {
            None
        }
        ExprKind::Paren(inner)
        | ExprKind::Some(inner)
        | ExprKind::Ok(inner)
        | ExprKind::Err(inner) => first_non_static(inner, consts, types),
        ExprKind::ListLit(items) => all(items.iter()),
        ExprKind::RecordConstruction { fields, .. } => {
            for f in fields {
                match &f.value {
                    Some(v) => {
                        if let Some(bad) = first_non_static(v, consts, types) {
                            return Some(bad);
                        }
                    }
                    None => return Some((f.span, "a shorthand field (a reference to a binding)")),
                }
            }
            None
        }
        // A reference to another constant, or a bare nullary variant.
        ExprKind::Ident(id) if consts.contains_key(&id.name) || is_any_variant(types, &id.name) => {
            None
        }
        // A bare variant constructor call `Circle(3)`.
        ExprKind::Call { name, args, .. } if is_any_variant(types, &name.name) => all(args.iter()),
        // `Shape.Circle`, a qualified nullary variant.
        ExprKind::FieldAccess { receiver, field } if matches!(&receiver.kind, ExprKind::Ident(t) if sum_has_variant(types, &t.name, &field.name)) => {
            None
        }
        ExprKind::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            let ExprKind::Ident(t) = &receiver.kind else {
                return Some((e.span, "a method call"));
            };
            if sum_has_variant(types, &t.name, &method.name) {
                // `Shape.Circle(3)`, a qualified variant constructor.
                all(args.iter())
            } else if method.name == "empty"
                && args.is_empty()
                && matches!(t.name.as_str(), "List" | "Map" | "Set")
            {
                None
            } else if method.name == "unsafe"
                && types
                    .get(&t.name)
                    .is_some_and(|d| matches!(d.body, TypeBody::Opaque { .. }))
            {
                match args.as_slice() {
                    [lit] if is_literal(lit) => None,
                    _ => Some((e.span, "`.unsafe` of anything but a literal")),
                }
            } else {
                Some((e.span, "a function or method call"))
            }
        }
        ExprKind::Ident(_) => Some((e.span, "a reference to something other than a constant")),
        ExprKind::Call { .. } => Some((e.span, "a function call")),
        ExprKind::InterpStr(_) => Some((e.span, "an interpolated string")),
        ExprKind::BinOp(..) | ExprKind::UnaryOp(..) => Some((e.span, "an operator expression")),
        ExprKind::RecordSpread { .. } => Some((e.span, "a record spread")),
        ExprKind::FieldAccess { .. } => Some((e.span, "a field access")),
        ExprKind::Lambda(_) => Some((e.span, "a lambda")),
        ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::Block(_) => {
            Some((e.span, "a control-flow expression"))
        }
        _ => Some((e.span, "this expression")),
    }
}

/// The constants `e` refers to, by name, in source order (with repeats).
pub fn referenced_consts<'a>(
    e: &'a Expr,
    consts: &HashMap<String, ConstDecl>,
    out: &mut Vec<&'a Ident>,
) {
    if let ExprKind::Ident(id) = &e.kind
        && consts.contains_key(&id.name)
    {
        out.push(id);
    }
    for child in bynk_syntax::ast::expr_children(e) {
        referenced_consts(child, consts, out);
    }
}

/// `bynk.resolve.const_cycle` for each cycle through the constants `local`
/// (this file's, in declaration order) among `consts` (every constant in
/// scope). Each cycle is reported once, at the member declared first here.
pub fn cycle_errors(
    local: &[&ConstDecl],
    consts: &HashMap<String, ConstDecl>,
) -> Vec<CompileError> {
    let order: HashMap<&str, usize> = local
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.name.as_str(), i))
        .collect();
    let mut errors = Vec::new();
    for (i, c) in local.iter().enumerate() {
        let Some(path) = find_cycle(&c.name.name, consts) else {
            continue;
        };
        // Reported at the member of the cycle declared first in this file.
        if path
            .iter()
            .any(|n| order.get(n.as_str()).is_some_and(|&j| j < i))
        {
            continue;
        }
        let shown = path
            .iter()
            .chain(std::iter::once(&c.name.name))
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(" → ");
        errors.push(
            CompileError::new(
                "bynk.resolve.const_cycle",
                c.name.span,
                format!("constant `{}` refers to itself: {shown}", c.name.name),
            )
            .with_note(
                "a constant's value cannot depend on itself; break the cycle with a literal",
            ),
        );
    }
    errors
}

/// A path `start → … → (back to start)` through the constants' references,
/// without the closing `start`, or `None` when `start` is on no cycle.
fn find_cycle(start: &str, consts: &HashMap<String, ConstDecl>) -> Option<Vec<String>> {
    fn dfs(
        node: &str,
        start: &str,
        consts: &HashMap<String, ConstDecl>,
        path: &mut Vec<String>,
        visited: &mut HashSet<String>,
    ) -> bool {
        let Some(decl) = consts.get(node) else {
            return false;
        };
        path.push(node.to_string());
        let mut refs = Vec::new();
        referenced_consts(&decl.init, consts, &mut refs);
        for r in refs {
            if r.name == start {
                return true;
            }
            if visited.insert(r.name.clone()) && dfs(&r.name, start, consts, path, visited) {
                return true;
            }
        }
        path.pop();
        false
    }
    let mut path = Vec::new();
    let mut visited = HashSet::new();
    dfs(start, start, consts, &mut path, &mut visited).then_some(path)
}

/// The single-file compile's declaration diagnostics for constants, in
/// declaration order: each constant's name rules ([`name_error`], against the
/// file's own embeds, assets, functions, types and earlier constants, and
/// [`builtin_name_error`]). Placement needs no check here: a single file is
/// always a commons. The case, static-ness, type and cycle rules are the
/// checker's, on every path.
pub fn single_file_const_errors(items: &[CommonsItem]) -> Vec<CompileError> {
    let mut fns: HashMap<&str, &Ident> = HashMap::new();
    let mut types: HashMap<&str, &Ident> = HashMap::new();
    let mut assets: HashMap<&str, &Ident> = HashMap::new();
    let mut embeds: HashMap<&str, &Ident> = HashMap::new();
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
            CommonsItem::Embed(e) => {
                embeds.entry(e.name.name.as_str()).or_insert(&e.name);
            }
            _ => {}
        }
    }
    let mut consts: HashMap<&str, &Ident> = HashMap::new();
    let mut errors = Vec::new();
    for item in items {
        let CommonsItem::Const(c) = item else {
            continue;
        };
        let key = c.name.name.as_str();
        match name_error(
            c,
            consts.get(key).copied(),
            embeds.get(key).copied(),
            assets.get(key).copied(),
            fns.get(key).copied(),
            types.get(key).copied(),
        )
        .or_else(|| builtin_name_error(c))
        {
            Some(err) => errors.push(err),
            None => {
                consts.insert(key, &c.name);
            }
        }
    }
    errors
}

/// Every constant visible in unit `name`: its own, plus those of each commons
/// it `uses` (one level, the mixin rule [`crate::embeds::compose_unit_embeds`]
/// applies to embeds). A local declaration of any kind wins over an imported
/// constant of the same name; two used commons declaring the same name is
/// `bynk.uses.name_conflict`, so which is kept here is never observable in a
/// program that checks.
pub fn compose_unit_consts(
    name: &str,
    unit_info: &BTreeMap<String, UnitInfo>,
) -> HashMap<String, ConstDecl> {
    let Some(info) = unit_info.get(name) else {
        return HashMap::new();
    };
    let mut combined = info.table.consts.clone();
    for used in &info.uses {
        let Some(used_info) = unit_info.get(used) else {
            continue;
        };
        for (const_name, decl) in &used_info.table.consts {
            if crate::symbols::local_value_kind(&info.table, const_name).is_some() {
                continue;
            }
            combined
                .entry(const_name.clone())
                .or_insert_with(|| decl.clone());
        }
    }
    combined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_names_are_upper_camel() {
        assert!(is_constant_name("MaxRetries"));
        assert!(is_constant_name("A"));
        assert!(is_constant_name("HTTP_2"));
        assert!(!is_constant_name("maxRetries"));
        assert!(!is_constant_name("_Max"));
        assert!(!is_constant_name(""));
        assert_eq!(upper_camel("max_retries").as_deref(), Some("MaxRetries"));
        assert_eq!(upper_camel("timeout").as_deref(), Some("Timeout"));
    }
}
