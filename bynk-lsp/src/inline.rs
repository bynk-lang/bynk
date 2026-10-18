//! #2100 slice 1: the inline-variable refactor (`refactor.inline`). It
//! replaces every use of a `let` binding with the binding's initialiser and
//! deletes the `let`.
//!
//! Only a pure `let name = value` is inlined. A `let name <- value` performs an
//! effect where it stands, so moving or repeating it would change what the
//! program does: it is never offered. A `let =` initialiser has no effects (in
//! Bynk an effect needs `<-`), so repeating it at several uses keeps the
//! program's meaning (DECISION B's re-check still runs).
//!
//! The action is offered with the cursor on the binding's name or on any use.
//! It declines when:
//! - the binding is `_`, a `let … else` desugaring, or has no uses;
//! - a name the initialiser reads would resolve to a different binding at a
//!   use (an inner `let` or parameter shadowing it). The re-check can't see
//!   that, because the program would still type-check;
//! - the `let` shares its lines with other code, or carries a comment
//!   (deleting it would drop or orphan the comment).
//!
//! A use as a record shorthand (`T { name }`) becomes `T { name: value }`. A
//! non-atomic initialiser is parenthesised at each use. Like fill-arms the
//! action rides the unfiltered lightbulb request: it is listed with no edit,
//! and `codeAction/resolve` computes the edit and re-checks it.

use bynk_check::locals::{LocalBinding, locals_at};
use bynk_syntax::ast::{
    Block, CommonsItem, Expr, ExprKind, Ident, LetStmt, MatchBody, SourceUnit, Statement,
    expr_children, statement_exprs,
};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::*;

const DATA_TAG: &str = "bynk.inlineVariable";

/// A planned inline: the binding's name, its definition span, and the edits.
pub struct Plan {
    pub name: String,
    pub def: Span,
    pub edits: Vec<(Span, String)>,
}

/// The inline of the local binding at `offset` (its name or a use) in `text`,
/// given the round's local bindings for this file. `None` when there is none
/// to offer (see the module docs).
pub fn plan(text: &str, offset: usize, locals: &[LocalBinding]) -> Option<Plan> {
    let sites = crate::locals_nav::local_sites_at(locals, text, offset)?;
    let (def, uses) = sites.split_first()?;
    let tokens = tokenize(text).ok()?;
    let (Some(unit), errs) = parse_unit_with_recovery(&tokens, text) else {
        return None;
    };
    if !errs.is_empty() {
        return None;
    }
    let mut found = Found::default();
    walk_unit(&unit, def.start, &mut found);
    let let_stmt = found.let_stmt?;
    if let_stmt.name.name == "_" || let_stmt.let_else.is_some() || found.effectful {
        return None;
    }
    if !let_stmt.trivia.leading.is_empty() || let_stmt.trivia.trailing.is_some() {
        return None;
    }
    let value = &let_stmt.value;
    let value_text = text.get(value.span.start..value.span.end)?;
    let replacement = if atomic(value) {
        value_text.to_string()
    } else {
        format!("({value_text})")
    };

    // Free names of the initialiser must resolve the same way at every use.
    let mut idents = Vec::new();
    collect_idents(value, &mut idents);
    let resolve = |name: &str, at: usize| -> Option<Span> {
        locals_at(locals, at)
            .into_iter()
            .find(|b| b.name == name)
            .map(|b| b.def_span)
    };
    let same = |a: Option<Span>, b: Option<Span>| {
        a.map(|s| (s.start, s.end)) == b.map(|s| (s.start, s.end))
    };

    let mut edits = Vec::new();
    for u in uses {
        let shorthand = found
            .shorthands
            .iter()
            .any(|s| (s.start, s.end) == (u.start, u.end));
        let expr_use = found
            .idents
            .iter()
            .any(|s| (s.start, s.end) == (u.start, u.end));
        if !shorthand && !expr_use {
            continue; // a field or pattern name spelled the same, not a use
        }
        for id in &idents {
            if !same(
                resolve(&id.name, value.span.start),
                resolve(&id.name, u.start),
            ) {
                return None;
            }
        }
        let new_text = if shorthand {
            format!("{}: {value_text}", let_stmt.name.name)
        } else {
            replacement.clone()
        };
        edits.push((Span::new(u.start, u.end), new_text));
    }
    if edits.is_empty() {
        return None;
    }

    // Delete the `let`'s whole line(s).
    let s = let_stmt.span;
    let line_start = text[..s.start].rfind('\n').map_or(0, |i| i + 1);
    if !text[line_start..s.start].trim().is_empty() {
        return None;
    }
    let line_end = text[s.end..].find('\n').map_or(text.len(), |i| s.end + i);
    if !text[s.end..line_end].trim().is_empty() {
        return None;
    }
    let delete_end = (line_end + 1).min(text.len());
    edits.push((Span::new(line_start, delete_end), String::new()));
    edits.sort_by_key(|(span, _)| (span.start, span.end));

    // A canonical file stays canonical: the inlined text is re-formatted
    // (deleting a `let` can leave a body `bynk-fmt` prints on one line) and
    // the edit becomes the one region where the two differ.
    let opts = bynk_fmt::FormatOptions::default();
    if bynk_fmt::format_source(text, &opts).is_ok_and(|f| f == text) {
        let inlined = crate::fix_all::apply(text, &edits);
        let formatted = bynk_fmt::format_source(&inlined, &opts).ok()?;
        edits = vec![changed_region(text, &formatted)];
    }
    Some(Plan {
        name: let_stmt.name.name.clone(),
        def: *def,
        edits,
    })
}

/// The one edit turning `old` into `new`: the span between their common
/// prefix and common suffix, on char boundaries.
fn changed_region(old: &str, new: &str) -> (Span, String) {
    let prefix: usize = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let suffix: usize = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    (
        Span::new(prefix, old.len() - suffix),
        new[prefix..new.len() - suffix].to_string(),
    )
}

/// An initialiser that needs no parentheses wherever an expression goes.
fn atomic(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::IntLit { .. }
            | ExprKind::FloatLit { .. }
            | ExprKind::DurationLit { .. }
            | ExprKind::StrLit(_)
            | ExprKind::InterpStr(_)
            | ExprKind::BoolLit(_)
            | ExprKind::Ident(_)
            | ExprKind::Call { .. }
            | ExprKind::Paren(_)
            | ExprKind::Ok(_)
            | ExprKind::Err(_)
            | ExprKind::RecordConstruction { .. }
            | ExprKind::FieldAccess { .. }
            | ExprKind::MethodCall { .. }
            | ExprKind::Some(_)
            | ExprKind::None
            | ExprKind::UnitLit
            | ExprKind::ListLit(_)
    )
}

fn collect_idents<'a>(e: &'a Expr, out: &mut Vec<&'a Ident>) {
    if let ExprKind::Ident(id) = &e.kind {
        out.push(id);
    }
    for c in expr_children(e) {
        collect_idents(c, out);
    }
}

/// What the walk over the unit found: the `let` whose name starts at the
/// definition offset, and the spans of every identifier expression and
/// record shorthand field.
#[derive(Default)]
struct Found<'a> {
    let_stmt: Option<&'a LetStmt>,
    effectful: bool,
    idents: Vec<Span>,
    shorthands: Vec<Span>,
}

fn walk_unit<'a>(unit: &'a SourceUnit, def: usize, found: &mut Found<'a>) {
    let items: &[CommonsItem] = match unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(_) => &[],
        SourceUnit::Suite(t) => {
            for case in &t.cases {
                walk_block(&case.body, def, found);
            }
            &[]
        }
    };
    for item in items {
        match item {
            CommonsItem::Fn(f) => walk_block(&f.body, def, found),
            CommonsItem::Provider(p) => {
                p.ops.iter().for_each(|op| walk_block(&op.body, def, found))
            }
            CommonsItem::Service(s) => s
                .handlers
                .iter()
                .for_each(|h| walk_block(&h.body, def, found)),
            CommonsItem::Agent(a) => a
                .handlers
                .iter()
                .for_each(|h| walk_block(&h.body, def, found)),
            _ => {}
        }
    }
}

fn walk_block<'a>(b: &'a Block, def: usize, found: &mut Found<'a>) {
    for s in &b.statements {
        if let Statement::Let(l) | Statement::EffectLet(l) = s
            && l.name.span.start == def
        {
            found.let_stmt = Some(l);
            found.effectful = matches!(s, Statement::EffectLet(_));
        }
        let mut exprs = Vec::new();
        statement_exprs(s, &mut exprs);
        for e in exprs {
            walk_expr(e, def, found);
        }
    }
    walk_expr(&b.tail, def, found);
}

fn walk_expr<'a>(e: &'a Expr, def: usize, found: &mut Found<'a>) {
    match &e.kind {
        ExprKind::Ident(id) => found.idents.push(id.span),
        ExprKind::RecordConstruction { fields, .. } => {
            for f in fields.iter().filter(|f| f.value.is_none()) {
                found.shorthands.push(f.name.span);
            }
        }
        _ => {}
    }
    match &e.kind {
        ExprKind::Block(b) => walk_block(b, def, found),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            walk_expr(cond, def, found);
            walk_block(then_block, def, found);
            walk_block(else_block, def, found);
        }
        ExprKind::Match { discriminant, arms } => {
            walk_expr(discriminant, def, found);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_expr(g, def, found);
                }
                match &arm.body {
                    MatchBody::Expr(e) => walk_expr(e, def, found),
                    MatchBody::Block(b) => walk_block(b, def, found),
                }
            }
        }
        _ => {
            for c in expr_children(e) {
                walk_expr(c, def, found);
            }
        }
    }
}

/// The listed (unresolved) `refactor.inline` action for `plan`.
pub fn listed(plan: &Plan, uri: &Url, version: Option<i32>) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Inline `{}`", plan.name),
        kind: Some(CodeActionKind::REFACTOR_INLINE),
        data: Some(serde_json::json!({
            "kind": DATA_TAG,
            "uri": uri.as_str(),
            "version": version,
            "def": plan.def.start,
        })),
        ..Default::default()
    })
}

/// What a listed inline action's `data` names.
pub struct Target {
    pub uri: Url,
    pub version: Option<i32>,
    pub def: usize,
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
            def: usize::try_from(data.get("def")?.as_u64()?).ok()?,
        })
    }
}
