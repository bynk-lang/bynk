//! v0.213 (ADR 0239): `textDocument/codeAction` extract-variable —
//! `CodeActionKind::REFACTOR_EXTRACT`. The smallest expression node whose
//! span fully contains the requested selection is bound to a fresh `let`
//! inserted immediately above its enclosing statement (or block tail), and
//! the selection's span is replaced with the new name.
//!
//! `text` is whatever snapshot the caller passes in — for `code_action` that's
//! the last **committed** analysis round's snapshot (ADR 0235's
//! stale-while-revalidate posture), not necessarily the very latest keystroke.
//! Like `crate::structure`, this reparses that snapshot fresh each call — no
//! cached AST is retained in `Analysis`, and extraction is selection-driven
//! the same way folding/selection ranges are.
//!
//! Track #800 settles as: [`plan_function`] reuses the exact same
//! smallest-containing-expression selection algorithm, but lifts the
//! expression into a new top-level `fn` (threading its free identifiers as
//! parameters) rather than a local `let`. It is capability-free-only — `fn`
//! has no `given` clause to propagate a capability-using body into, unlike
//! `Handler`/`Provider` (that language-change question is deliberately not
//! taken on here) — so it declines whenever the selection's site carries any
//! recorded [`Requirement`], covered or not.
//!
//! #813 extends [`plan_function`] to a contiguous run of one or more full
//! statements (optionally including the block's tail), not just one
//! expression — offered when the selection's span aligns exactly with
//! statement boundaries (whitespace-trimmed at either end, so a real
//! "select these lines" gesture matches); a selection that doesn't align
//! falls back to the single-expression algorithm above unchanged. A run that
//! stops before the tail synthesises a `fn … -> ()` — or `-> Effect[()]`
//! when the run itself performs an effect, so `~>`/`do`/`<-` stay legal in
//! the lifted body — with no explicit tail, the same implicit-unit-tail
//! shape the parser already synthesises for any statements-only block
//! (v0.146, ADR 0170); the call site becomes `let _ = …(…)` (or `do …(…)`
//! in the effectful case) rather than a bare expression, since Bynk has no
//! expression-statement form. It declines whenever a `:=` (`Cell` store
//! write) statement falls inside the run — a lifted top-level `fn` has no
//! store fields to write, so this always fails to typecheck, not merely a
//! conservative guess — or whenever a binding the run introduces is still
//! referenced later in the same block: lifting it away would strand that
//! reference.

use bynk_check::checker::{TyId, Types};
use bynk_check::expr_types::type_at_span;
use bynk_check::locals::{LocalBinding, locals_at};
use bynk_check::requirements::Requirement;
use bynk_syntax::ast::*;
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;
use bynk_syntax::span::Span;
use std::collections::HashMap;
use tower_lsp::lsp_types::*;

/// Extract-variable actions for the requested range. Only offered for a
/// genuine selection (`requested.start != requested.end`) — a bare cursor
/// offers nothing, the refactor-vs-quickfix posture other editors take.
pub fn extract_variable(
    text: &str,
    requested: Span,
    uri: &Url,
    version: Option<i32>,
) -> Vec<CodeActionOrCommand> {
    if requested.start == requested.end {
        return Vec::new();
    }
    let Ok(tokens) = tokenize(text) else {
        return Vec::new();
    };
    let (Some(unit), _errs) = parse_unit_with_recovery(&tokens, text) else {
        return Vec::new();
    };
    let Some(site) = find_site(&unit, requested) else {
        return Vec::new();
    };
    // #1819: the `let` goes above the enclosing statement, outside any arm
    // pattern, lambda parameter or `is` binding the descent passed. A
    // selection reading one of those would bind it out of scope, so offer
    // nothing, as extract-function declines what it can't lift soundly. The
    // match is by name, deliberately conservative: a selection that rebinds
    // the name, or reads an outer binding sharing it, is declined too, never
    // hoisted wrongly.
    let mut read = Vec::new();
    collect_idents(site.expr, &mut read);
    if read.iter().any(|id| site.binders.binds(&id.name)) {
        return Vec::new();
    }

    let selected = &text[site.expr_span.start..site.expr_span.end];
    let name = fresh_name(text);
    let indent = line_indent(text, site.insertion_offset);
    let insert_pos = crate::position::offset_to_position(text, site.insertion_offset);

    let edits = vec![
        OneOf::Left(TextEdit {
            range: Range::new(insert_pos, insert_pos),
            new_text: format!("let {name} = {selected}\n{indent}"),
        }),
        OneOf::Left(TextEdit {
            range: crate::position::span_to_range(text, site.expr_span),
            new_text: name.clone(),
        }),
    ];
    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Extract variable `{name}`"),
        kind: Some(CodeActionKind::REFACTOR_EXTRACT),
        edit: Some(WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                edits,
            }])),
            change_annotations: None,
        }),
        ..Default::default()
    })]
}

/// Extract-function actions for the requested range (track #800). Locates
/// the same smallest-containing-expression the extract-variable selection
/// algorithm would, but only within a `Commons`/`Context` file's top-level
/// `fn`/`Provider`/`Service`/`Agent` items — a new top-level `fn` needs
/// somewhere top-level to live, so an `Adapter` (no Bynk bodies) or a `Suite`
/// test case (no enclosing top-level item to insert above) offers nothing.
///
/// Declines (offers nothing) rather than guessing whenever:
/// - the selection's site carries any recorded capability [`Requirement`],
///   covered or not — the settled capability-free-only surface (#2100: the
///   user's decision); a plain `fn` has no `given` to cover it once lifted.
///   The refusal carries a reason, shown as a disabled action.
/// - the selection contains a `?` outside a lambda (#2100): lifted, it would
///   return from the new `fn` instead.
/// - the selection's expression type, or any free variable's type, isn't
///   recorded in `expr_types`/the rendered `locals` types — no action rather
///   than a guessed type. Since ADR 0094 a file with an unrelated error
///   elsewhere still records both (best-effort partial types), so the action
///   is offered there too.
/// - two distinct outer-scope bindings share a free variable's name (a rare
///   nested-shadow collision) — threading either one as the parameter would
///   silently pick the wrong variable for the other occurrence.
/// - any expression inside the selection failed to type (its recorded type
///   is `Ty::Error`) — the selection's own type would render as
///   `<type error>`, and a binding whose value failed to type is never
///   recorded in `locals`, so a read of one would not be threaded as a
///   parameter and the lifted `fn` would name an unbound variable.
///
/// Free variables are the selection's `Ident` references (walked via
/// [`expr_children`], the same exhaustive child iterator `locate` uses) whose
/// nearest enclosing binding — [`locals_at`] at the reference's own offset —
/// sits outside the selection; an identifier with no local binding at all
/// (a top-level `fn`, a capability, a type name) is left alone, already
/// resolvable at the new `fn`'s own top-level scope.
///
/// #813: the selection may also resolve to a contiguous statement run (see
/// [`FunctionSelection::Stmts`]) rather than one expression — free-variable
/// synthesis is the same walk, just seeded from every selected statement's
/// values (via [`statement_exprs`]) instead of a single expression.
///
/// #2100 slice 2: the rules above live in [`plan_function`], which also
/// refuses a selection containing `?` (an early return). The server lists the
/// action lazily through [`function_listed`] and computes the edit in
/// `codeAction/resolve`. This eager form, the plan as one action with its
/// edit, is what the unit tests below drive.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub fn extract_function(
    text: &str,
    requested: Span,
    uri: &Url,
    version: Option<i32>,
    requirements: &[Requirement],
    locals: &[LocalBinding],
    expr_types: &[(Span, TyId)],
    tys: &Types,
) -> Vec<CodeActionOrCommand> {
    let Ok(plan) = plan_function(text, requested, requirements, locals, expr_types, tys) else {
        return Vec::new();
    };
    let edits = plan
        .edits
        .iter()
        .map(|(span, new_text)| {
            OneOf::Left(TextEdit {
                range: crate::position::span_to_range(text, *span),
                new_text: new_text.clone(),
            })
        })
        .collect();
    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Extract function `{}`", plan.name),
        kind: Some(CodeActionKind::REFACTOR_EXTRACT),
        edit: Some(WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                edits,
            }])),
            change_annotations: None,
        }),
        ..Default::default()
    })]
}

/// #2100 slice 2: a planned extract-function — the new `fn`'s name and the
/// two edits (insert the `fn`, replace the selection with its call).
pub struct FunctionPlan {
    pub name: String,
    pub edits: Vec<(Span, String)>,
}

/// #2100 slice 2: why extract-function is not offered for a selection.
/// `reason` is `Some` when the selection is a real candidate the user should
/// be told about (a client that supports it shows a disabled action with the
/// reason); `None` when there is simply nothing to extract there.
#[derive(Debug)]
pub struct FunctionRefusal {
    pub reason: Option<String>,
}

impl FunctionRefusal {
    fn silent() -> Self {
        FunctionRefusal { reason: None }
    }
}

/// #2100 slice 2: the extract-function plan for `requested`, or why there is
/// none. Extracted functions stay capability-free (a `fn` has no `given`
/// clause), and a selection containing `?` is refused: lifted into a new
/// `fn`, the `?` would return from that `fn` instead of the one it was
/// written in.
pub fn plan_function(
    text: &str,
    requested: Span,
    requirements: &[Requirement],
    locals: &[LocalBinding],
    expr_types: &[(Span, TyId)],
    tys: &Types,
) -> Result<FunctionPlan, FunctionRefusal> {
    let silent = || FunctionRefusal::silent();
    if requested.start == requested.end {
        return Err(silent());
    }
    let tokens = tokenize(text).map_err(|_| silent())?;
    let (Some(unit), _errs) = parse_unit_with_recovery(&tokens, text) else {
        return Err(silent());
    };
    let site = find_function_site(&unit, requested, text).ok_or_else(silent)?;
    let mut caps: Vec<&str> = requirements
        .iter()
        .filter(|r| contains(site.span, r.site))
        .map(|r| r.capability.as_str())
        .collect();
    if !caps.is_empty() {
        caps.sort_unstable();
        caps.dedup();
        let names = caps
            .iter()
            .map(|c| format!("`{c}`"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(FunctionRefusal {
            reason: Some(format!(
                "the selection uses {names}; an extracted function stays capability-free, \
                 since a `fn` has no `given` clause to carry {}",
                if caps.len() == 1 { "it" } else { "them" }
            )),
        });
    }
    if expr_types
        .iter()
        .any(|(s, t)| contains(site.span, *s) && t.is_error(tys))
    {
        return Err(silent());
    }

    let mut exprs: Vec<&Expr> = Vec::new();
    let (ret_ty_display, call_site_form): (String, CallSiteForm) = match &site.selection {
        FunctionSelection::Expr(expr) => {
            let Some(ret_ty) = type_at_span(expr_types, site.span) else {
                return Err(silent());
            };
            exprs.push(expr);
            (ret_ty.display(tys), CallSiteForm::Bare)
        }
        FunctionSelection::Stmts {
            stmts,
            tail,
            remainder,
        } => {
            // A lifted top-level `fn` has no `store` fields — a `:=` inside
            // the run (however deeply nested through `if`/`match`/block
            // sub-expressions) would always fail to resolve its target.
            if stmts_contain_assign_stmt(stmts) {
                return Err(silent());
            }
            for s in *stmts {
                statement_exprs(s, &mut exprs);
            }
            match tail {
                Some(t) => {
                    let Some(ret_ty) = type_at_span(expr_types, t.span) else {
                        return Err(silent());
                    };
                    exprs.push(t);
                    (ret_ty.display(tys), CallSiteForm::Bare)
                }
                None => {
                    if leaks_a_binding(stmts, remainder, locals, site.span) {
                        return Err(silent());
                    }
                    if stmts_contain_effect_stmt(stmts) {
                        ("Effect[()]".to_string(), CallSiteForm::Do)
                    } else {
                        ("()".to_string(), CallSiteForm::Discard)
                    }
                }
            }
        }
    };

    // An early return can't be lifted: inside a new `fn`, `?` would return
    // from that `fn` rather than the one the selection was written in.
    if exprs.iter().any(|e| has_early_return(e)) {
        return Err(FunctionRefusal {
            reason: Some(
                "the selection contains `?`, which would return from the extracted \
                 function instead of the one it is written in"
                    .to_string(),
            ),
        });
    }

    let mut idents = Vec::new();
    for e in &exprs {
        collect_idents(e, &mut idents);
    }
    let mut free: Vec<(&str, Span, &str)> = Vec::new();
    for id in &idents {
        let Some(binding) = locals_at(locals, id.span.start)
            .into_iter()
            .find(|b| b.name == id.name)
        else {
            continue;
        };
        if contains(site.span, binding.def_span) {
            continue; // bound inside the selection itself, not free
        }
        free.push((id.name.as_str(), binding.def_span, binding.ty.as_str()));
    }
    let mut def_by_name: HashMap<&str, Span> = HashMap::new();
    for (name, def_span, _) in &free {
        match def_by_name.get(name) {
            Some(prev) if *prev != *def_span => return Err(silent()),
            _ => {
                def_by_name.insert(name, *def_span);
            }
        }
    }
    let mut params: Vec<(&str, &str)> = Vec::new();
    for (name, _, ty) in &free {
        if !params.iter().any(|(n, _)| n == name) {
            params.push((name, ty));
        }
    }

    let fn_name = fresh_word(text, "extractedFn");
    let param_list = params
        .iter()
        .map(|(n, t)| format!("{n}: {t}"))
        .collect::<Vec<_>>()
        .join(", ");
    let arg_list = params
        .iter()
        .map(|(n, _)| n.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let selected = &text[site.span.start..site.span.end];
    let new_fn = format!("fn {fn_name}({param_list}) -> {ret_ty_display} {{\n  {selected}\n}}\n\n");

    // Bynk has no expression-statement form, so a tail-excluded statement
    // run's call can't stand alone as `fn_name(args)` — it needs `let _ =`
    // (pure) or `do` (effectful) to be a legal statement. A bare expression
    // (single-expression selection, or a run that includes the tail) needs
    // neither: it replaces an expression position, not a statement.
    let call_expr = format!("{fn_name}({arg_list})");
    let call_site_text = match call_site_form {
        CallSiteForm::Bare => call_expr,
        CallSiteForm::Discard => format!("let _ = {call_expr}"),
        CallSiteForm::Do => format!("do {call_expr}"),
    };

    Ok(FunctionPlan {
        name: fn_name,
        edits: vec![
            (Span::new(site.item_start, site.item_start), new_fn),
            (site.span, call_site_text),
        ],
    })
}

const FUNCTION_DATA_TAG: &str = "bynk.extractFunction";

/// #2100 slice 2: the listed extract-function action for a selection — no
/// edit yet (`codeAction/resolve` computes and re-checks it), or, when the
/// selection is refused for a reason worth telling and the client supports
/// it, a disabled action carrying that reason. `None` otherwise.
pub fn function_listed(
    planned: &Result<FunctionPlan, FunctionRefusal>,
    requested: Span,
    uri: &Url,
    version: Option<i32>,
    disabled_support: bool,
) -> Option<CodeActionOrCommand> {
    match planned {
        Ok(plan) => Some(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Extract function `{}`", plan.name),
            kind: Some(CodeActionKind::REFACTOR_EXTRACT),
            data: Some(serde_json::json!({
                "kind": FUNCTION_DATA_TAG,
                "uri": uri.as_str(),
                "version": version,
                "start": requested.start,
                "end": requested.end,
            })),
            ..Default::default()
        })),
        Err(FunctionRefusal {
            reason: Some(reason),
        }) if disabled_support => Some(CodeActionOrCommand::CodeAction(CodeAction {
            title: "Extract function".to_string(),
            kind: Some(CodeActionKind::REFACTOR_EXTRACT),
            disabled: Some(CodeActionDisabled {
                reason: reason.clone(),
            }),
            ..Default::default()
        })),
        Err(_) => None,
    }
}

/// #2100 slice 2: what a listed extract-function action's `data` names.
pub struct FunctionTarget {
    pub uri: Url,
    pub version: Option<i32>,
    pub requested: Span,
}

impl FunctionTarget {
    pub fn of(action: &CodeAction) -> Option<Self> {
        let data = action.data.as_ref()?;
        if data.get("kind")?.as_str()? != FUNCTION_DATA_TAG {
            return None;
        }
        Some(FunctionTarget {
            uri: Url::parse(data.get("uri")?.as_str()?).ok()?,
            version: data
                .get("version")
                .and_then(serde_json::Value::as_i64)
                .and_then(|v| i32::try_from(v).ok()),
            requested: Span::new(
                usize::try_from(data.get("start")?.as_u64()?).ok()?,
                usize::try_from(data.get("end")?.as_u64()?).ok()?,
            ),
        })
    }
}

/// #2100 slice 2: `e` contains a `?` that returns from the enclosing body —
/// one inside a lambda returns from the lambda, so it moves with it.
fn has_early_return(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Question(_) => true,
        ExprKind::Lambda(_) => false,
        _ => expr_children(e).into_iter().any(has_early_return),
    }
}

/// The smallest expression node fully containing the selection, and the byte
/// offset immediately before its enclosing statement (or block tail) — where
/// the new `let` line is inserted. `expr` is retained (not just its span) so
/// [`plan_function`] can walk it for free identifiers without a second
/// descent through the tree.
struct Site<'a> {
    insertion_offset: usize,
    expr_span: Span,
    expr: &'a Expr,
    /// #1819: the names bound between `insertion_offset` and the selection,
    /// by an expression-bodied arm's pattern or a lambda's parameters: in
    /// scope at the selection, not at the insertion point. A descent into a
    /// block resets them along with the offset, since the `let` then goes
    /// inside the scope that binds them.
    binders: Binders<'a>,
}

/// #1819: what is bound between a [`Site`]'s insertion point and its
/// selection, so out of scope where the `let` goes.
#[derive(Default)]
struct Binders<'a> {
    names: Vec<&'a str>,
    /// Inside an observation's `with` predicate, whose names are the observed
    /// operation's parameters: declared on the capability, not in the tree,
    /// so every name read there is treated as bound (#1880 review).
    opaque: bool,
}

impl<'a> Binders<'a> {
    fn extend(&mut self, names: impl IntoIterator<Item = &'a str>) {
        self.names.extend(names);
    }

    fn binds(&self, name: &str) -> bool {
        self.opaque || self.names.contains(&name)
    }
}

/// The names an `is` test binds for the operand to its right: `r is Ok(n)`
/// binds `n` on the right of `&&` / `implies`, through a chain of them and
/// parentheses, as the checker scopes them (#1880 review).
fn is_bindings(e: &Expr) -> Vec<&str> {
    match &e.kind {
        ExprKind::Is { pattern, .. } => pattern
            .bound_names()
            .into_iter()
            .map(|id| id.name.as_str())
            .collect(),
        ExprKind::BinOp(BinOp::And | BinOp::Implies, l, r) => {
            let mut names = is_bindings(l);
            names.extend(is_bindings(r));
            names
        }
        ExprKind::Paren(inner) => is_bindings(inner),
        _ => Vec::new(),
    }
}

/// Closed containment over half-open spans: `outer` fully contains `inner`.
fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// Finds the body (fn / handler / provider op / test case) whose span
/// contains `target`, then hands off to [`find_in_block`]. `None` when the
/// selection sits outside any body (a header, or a file that doesn't parse).
///
/// #2010: a selection whose smallest enclosing expression is the `match` a
/// `let … else` was desugared into — the statement itself, or a span crossing
/// from it into the rest of its block — has no expression of the author's to
/// lift, so it is `None` too.
fn find_site(unit: &SourceUnit, target: Span) -> Option<Site<'_>> {
    find_site_any(unit, target).filter(|site| !site.expr.is_let_else_match())
}

fn find_site_any(unit: &SourceUnit, target: Span) -> Option<Site<'_>> {
    match unit {
        SourceUnit::Commons(c) => find_in_items(&c.items, target),
        SourceUnit::Context(c) => find_in_items(&c.items, target),
        // Adapters carry no Bynk-bodied operations to extract from.
        SourceUnit::Adapter(_) => None,
        SourceUnit::Suite(t) => t
            .cases
            .iter()
            .find(|case| contains(case.body.span, target))
            .and_then(|case| find_in_block(&case.body, target)),
    }
}

fn find_in_items(items: &[CommonsItem], target: Span) -> Option<Site<'_>> {
    for item in items {
        match item {
            CommonsItem::Fn(f) if contains(f.body.span, target) => {
                return find_in_block(&f.body, target);
            }
            CommonsItem::Provider(p) => {
                if let Some(op) = p.ops.iter().find(|op| contains(op.body.span, target)) {
                    return find_in_block(&op.body, target);
                }
            }
            CommonsItem::Service(s) => {
                if let Some(h) = s.handlers.iter().find(|h| contains(h.body.span, target)) {
                    return find_in_block(&h.body, target);
                }
            }
            CommonsItem::Agent(a) => {
                if let Some(h) = a.handlers.iter().find(|h| contains(h.body.span, target)) {
                    return find_in_block(&h.body, target);
                }
            }
            _ => {}
        }
    }
    None
}

/// Finds the statement expression (or the tail) that fully contains `target`,
/// then narrows within it via [`locate`]. `None` when `target` doesn't sit
/// fully inside any single statement/tail (e.g. it spans the whole block,
/// braces included, or crosses a statement boundary).
fn find_in_block(block: &Block, target: Span) -> Option<Site<'_>> {
    for stmt in &block.statements {
        // #1851: whichever of the statement's expressions holds the selection.
        // Not the first: since #1766 a principal's identity comes before the
        // value it addresses.
        let mut exprs = Vec::new();
        statement_exprs(stmt, &mut exprs);
        if let Some(e) = exprs.into_iter().find(|e| contains(e.span, target)) {
            return Some(locate(e, target, stmt.span().start, Binders::default()));
        }
    }
    if contains(block.tail.span, target) {
        return Some(locate(
            &block.tail,
            target,
            block.tail.span.start,
            Binders::default(),
        ));
    }
    None
}

/// Descends into `expr` for the smallest node whose span fully contains
/// `target`. `Block`/`If`/`Match` carry their own nested statement/tail
/// slots, so descending into one resets `insertion_offset` to that slot's
/// start; every other kind bubbles the offset through unchanged, walked via
/// [`expr_children`] — `bynk_syntax`'s exhaustive-by-construction child
/// iterator, reused here rather than a second hand-rolled `ExprKind` match.
/// The remainder is enumerated explicitly rather than wildcarded, so an
/// `ExprKind` variant this doesn't handle fails to compile here instead of
/// silently falling through the extraction path — and a variant carrying a
/// `Block` field needs hand-matching above rather than appending to that
/// list, for the reason recorded at the arm itself.
fn locate<'a>(
    expr: &'a Expr,
    target: Span,
    insertion_offset: usize,
    mut binders: Binders<'a>,
) -> Site<'a> {
    match &expr.kind {
        ExprKind::Block(b) => find_in_block(b, target).unwrap_or(Site {
            insertion_offset,
            expr_span: expr.span,
            expr,
            binders,
        }),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            if contains(cond.span, target) {
                return locate(cond, target, insertion_offset, binders);
            }
            if contains(then_block.span, target)
                && let Some(site) = find_in_block(then_block, target)
            {
                return site;
            }
            if contains(else_block.span, target)
                && let Some(site) = find_in_block(else_block, target)
            {
                return site;
            }
            Site {
                insertion_offset,
                expr_span: expr.span,
                expr,
                binders,
            }
        }
        ExprKind::Match { discriminant, arms } => {
            if contains(discriminant.span, target) {
                return locate(discriminant, target, insertion_offset, binders);
            }
            // A selection inside a guard deliberately falls through to the
            // whole `match` below rather than descending (#1800 review): the
            // extracted `let` goes above the enclosing statement, outside the
            // arm, and a guard almost always reads the arm's pattern bindings,
            // which are not in scope there. Extracting the whole `match` is
            // sound.
            for arm in arms {
                if !contains(arm.body.span(), target) {
                    continue;
                }
                return match &arm.body {
                    MatchBody::Expr(e) => {
                        // #1819: an expression body has no block to insert
                        // into, so the arm's bindings stay out of scope at the
                        // insertion point.
                        binders.extend(arm.pattern.bound_names().iter().map(|id| id.name.as_str()));
                        locate(e, target, insertion_offset, binders)
                    }
                    MatchBody::Block(b) => find_in_block(b, target).unwrap_or(Site {
                        insertion_offset,
                        expr_span: expr.span,
                        expr,
                        binders,
                    }),
                };
            }
            Site {
                insertion_offset,
                expr_span: expr.span,
                expr,
                binders,
            }
        }
        // No variant below carries a `Block` *field*, so `expr_children`'s
        // total descent is complete for it and bubbling `insertion_offset`
        // through unchanged is correct — a block reached through a child (a
        // braced lambda body, say) comes back as an `Expr` and re-enters this
        // match at the `Block` arm above, which resets the offset via
        // `find_in_block`. A *new* variant that holds a `Block` directly must
        // be hand-matched up there alongside `Block`/`If`/`Match`: appending
        // it here descends past the block's own statement list *and* bubbles
        // the outer offset, so the returned `Site` inserts the extracted `fn`
        // outside the block the selection lives in — a wrong edit, not a
        // failed one.
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::DecimalLit { .. }
        | ExprKind::DurationLit { .. }
        | ExprKind::StrLit(_)
        | ExprKind::InterpStr(_)
        | ExprKind::BoolLit(_)
        | ExprKind::Ident(_)
        | ExprKind::Call { .. }
        | ExprKind::Lambda(_)
        | ExprKind::BinOp(..)
        | ExprKind::UnaryOp(..)
        | ExprKind::Paren(_)
        | ExprKind::Ok(_)
        | ExprKind::Err(_)
        | ExprKind::Question(_)
        | ExprKind::RecordConstruction { .. }
        | ExprKind::FieldAccess { .. }
        | ExprKind::MethodCall { .. }
        | ExprKind::Is { .. }
        | ExprKind::Some(_)
        | ExprKind::None
        | ExprKind::UnitLit
        | ExprKind::RecordSpread { .. }
        | ExprKind::EffectPure(_)
        | ExprKind::Expect(_)
        | ExprKind::Val { .. }
        | ExprKind::Wire(_)
        | ExprKind::ListLit(_)
        | ExprKind::Observation(_)
        | ExprKind::Faults(_)
        | ExprKind::Trace { .. } => {
            let children = expr_children(expr);
            match children.into_iter().find(|c| contains(c.span, target)) {
                Some(child) => {
                    // #1819: a lambda's parameters are in scope in its body
                    // only; an expression body bubbles the outer offset.
                    if let ExprKind::Lambda(l) = &expr.kind {
                        binders.extend(l.params.iter().map(|p| p.name.name.as_str()));
                    }
                    // #1880 review: an `is` on the left of `&&` / `implies`
                    // binds for the right operand only.
                    if let ExprKind::BinOp(BinOp::And | BinOp::Implies, lhs, rhs) = &expr.kind
                        && std::ptr::eq(child, rhs.as_ref())
                    {
                        binders.extend(is_bindings(lhs));
                    }
                    if let ExprKind::Observation(o) = &expr.kind
                        && let ObservationMatcher::Called {
                            with_pred: Some(pred),
                            ..
                        } = &o.matcher
                        && std::ptr::eq(child, pred.as_ref())
                    {
                        binders.opaque = true;
                    }
                    locate(child, target, insertion_offset, binders)
                }
                None => Site {
                    insertion_offset,
                    expr_span: expr.span,
                    expr,
                    binders,
                },
            }
        }
    }
}

/// `extracted`, `extracted2`, … — the first candidate that doesn't already
/// appear as a whole word anywhere in the file. A collision-avoiding
/// placeholder, not a scope-aware binder: the client's rename-on-extract is
/// the expected next step for a better name.
fn fresh_name(text: &str) -> String {
    fresh_word(text, "extracted")
}

/// Like [`fresh_name`], generalised to any base word — extract-function uses
/// `extractedFn`/`extractedFn2`/… so its generated name reads distinctly from
/// extract-variable's `extracted` when both actions are offered on the same
/// selection.
fn fresh_word(text: &str, base: &str) -> String {
    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_';
    let occurs_as_word = |candidate: &str| {
        text.match_indices(candidate).any(|(i, _)| {
            let before_ok = text[..i]
                .chars()
                .next_back()
                .is_none_or(|c| !is_ident_char(c));
            let after_ok = text[i + candidate.len()..]
                .chars()
                .next()
                .is_none_or(|c| !is_ident_char(c));
            before_ok && after_ok
        })
    };
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            base.to_string()
        } else {
            format!("{base}{n}")
        };
        if !occurs_as_word(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// The enclosing top-level item's span — where [`find_function_site`] inserts
/// the new `fn` (immediately above it). `None` for a `CommonsItem` extraction
/// can never target (a `type`/`context` declaration carries no body).
fn item_span(item: &CommonsItem) -> Option<Span> {
    match item {
        CommonsItem::Fn(f) => Some(f.span),
        CommonsItem::Provider(p) => Some(p.span),
        CommonsItem::Service(s) => Some(s.span),
        CommonsItem::Agent(a) => Some(a.span),
        _ => None,
    }
}

/// Where a [`FunctionSite`]'s selection resolves to. #813 adds [`Stmts`](Self::Stmts)
/// alongside the original track #800 single-expression shape.
enum FunctionSelection<'a> {
    /// The original shape: one AST expression, exactly as [`Site`] finds.
    Expr(&'a Expr),
    /// #813: a contiguous run of one or more full statements from a single
    /// block, optionally including the block's tail. `remainder` is every
    /// expression after the run within that same block — the tail plus
    /// nothing else when `tail` is `Some` (nothing follows it), or the tail
    /// and every later statement's value when `tail` is `None` — checked by
    /// [`leaks_a_binding`] so a binding the run introduces can't be lifted
    /// away out from under a live use.
    Stmts {
        stmts: &'a [Statement],
        tail: Option<&'a Expr>,
        remainder: Vec<&'a Expr>,
    },
}

/// How the call site replaces the selection. A tail-excluded statement run
/// has no expression position to drop a bare call into — Bynk has no
/// expression-statement form — so it needs a statement wrapper instead; see
/// [`plan_function`]'s call-site construction.
enum CallSiteForm {
    /// The call stands as a bare expression (a single-expression selection,
    /// or a statement run that includes the tail).
    Bare,
    /// A tail-excluded run whose lifted `fn` is pure (`-> ()`).
    Discard,
    /// A tail-excluded run whose lifted `fn` performs an effect
    /// (`-> Effect[()]`).
    Do,
}

/// Like [`find_site`], but only within `Commons`/`Context` files, and
/// additionally records the start of the enclosing top-level item — where the
/// new `fn` is inserted, immediately above it.
struct FunctionSite<'a> {
    item_start: usize,
    /// The exact selected span — the replacement range, and (for `Stmts`)
    /// the bound every containment check is stated against.
    span: Span,
    selection: FunctionSelection<'a>,
}

/// #813 tries a statement-run match first (against a whitespace-trimmed
/// `target`, since a "select these lines" gesture commonly pads onto
/// surrounding blank space); falling short of an exact statement-boundary
/// alignment there, this falls back to the original single-expression
/// algorithm unchanged.
fn find_function_site<'a>(
    unit: &'a SourceUnit,
    target: Span,
    text: &str,
) -> Option<FunctionSite<'a>> {
    let items = match unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(_) | SourceUnit::Suite(_) => return None,
    };
    let item = items
        .iter()
        .find(|it| item_span(it).is_some_and(|s| contains(s, target)))?;
    let item_start = item_span(item)?.start;

    let trimmed = trim_span(text, target);
    if trimmed.start < trimmed.end
        && let Some((stmts, tail, remainder)) = find_multi_stmt_in_item(item, trimmed)
    {
        return Some(FunctionSite {
            item_start,
            span: trimmed,
            selection: FunctionSelection::Stmts {
                stmts,
                tail,
                remainder,
            },
        });
    }

    let site = find_in_items(std::slice::from_ref(item), target)?;
    Some(FunctionSite {
        item_start,
        span: site.expr_span,
        selection: FunctionSelection::Expr(site.expr),
    })
}

/// Trims leading/trailing whitespace bytes from `span` against `text`: a
/// real editor selection of "these lines" commonly pads onto the
/// surrounding blank space, which isn't a partial-statement selection the
/// way clipping actual statement content would be.
fn trim_span(text: &str, span: Span) -> Span {
    let s = &text[span.start..span.end];
    let start = span.start + (s.len() - s.trim_start().len());
    let trimmed_len = s.trim().len();
    Span::new(start, start + trimmed_len)
}

/// A found statement run: the contiguous statement slice, the block's tail
/// when the run includes it, and — only populated by [`align_stmt_run`]'s
/// tail-excluded branch — the remainder [`leaks_a_binding`] scans.
type StmtRun<'a> = (&'a [Statement], Option<&'a Expr>, Vec<&'a Expr>);

/// Dispatches to the item's body/op/handler containing `target`, then hands
/// off to [`find_stmt_run`] — the statement-run counterpart of
/// [`find_in_items`].
fn find_multi_stmt_in_item(item: &CommonsItem, target: Span) -> Option<StmtRun<'_>> {
    match item {
        CommonsItem::Fn(f) if contains(f.body.span, target) => find_stmt_run(&f.body, target),
        CommonsItem::Provider(p) => p
            .ops
            .iter()
            .find(|op| contains(op.body.span, target))
            .and_then(|op| find_stmt_run(&op.body, target)),
        CommonsItem::Service(s) => s
            .handlers
            .iter()
            .find(|h| contains(h.body.span, target))
            .and_then(|h| find_stmt_run(&h.body, target)),
        CommonsItem::Agent(a) => a
            .handlers
            .iter()
            .find(|h| contains(h.body.span, target))
            .and_then(|h| find_stmt_run(&h.body, target)),
        _ => None,
    }
}

/// Descends to the smallest block whose own statement list aligns exactly
/// with `target` (#813): a nested block fully containing `target` is tried
/// first via [`find_stmt_run_in_expr`] (mirroring [`locate`]'s
/// descend-first policy), so a run inside an `if`/`match` branch resolves
/// there rather than at the outer level; [`align_stmt_run`] does the actual
/// boundary check once no deeper block matches. Unlike [`locate`], it also
/// descends into a `match` arm's guard (#1800): a run is lifted into a `fn`
/// whose parameters carry the guard's bindings, so nothing is hoisted out of
/// their scope.
fn find_stmt_run(block: &Block, target: Span) -> Option<StmtRun<'_>> {
    for stmt in &block.statements {
        let mut values = Vec::new();
        statement_exprs(stmt, &mut values);
        for v in values {
            if let Some(found) = find_stmt_run_in_expr(v, target) {
                return Some(found);
            }
        }
    }
    if let Some(found) = find_stmt_run_in_expr(&block.tail, target) {
        return Some(found);
    }
    align_stmt_run(block, target)
}

/// Looks for a nested block inside `e` whose statement list aligns with
/// `target` — `None` unless `e`'s span actually contains `target`. Falls
/// through to [`expr_children`] for anything other than `Block`/`If`/`Match`,
/// the same exhaustive walk [`locate`] and [`collect_idents`] use.
fn find_stmt_run_in_expr(e: &Expr, target: Span) -> Option<StmtRun<'_>> {
    if !contains(e.span, target) {
        return None;
    }
    match &e.kind {
        ExprKind::Block(b) => find_stmt_run(b, target),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => find_stmt_run_in_expr(cond, target)
            .or_else(|| find_stmt_run(then_block, target))
            .or_else(|| find_stmt_run(else_block, target)),
        // #1800: a guard is searched too, so a run inside one
        // (`Some(x) if if x > 0 { … } else { … } =>`) can be extracted.
        ExprKind::Match { discriminant, arms } => find_stmt_run_in_expr(discriminant, target)
            .or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.guard
                        .as_ref()
                        .and_then(|g| find_stmt_run_in_expr(g, target))
                        .or_else(|| match &arm.body {
                            MatchBody::Expr(e) => find_stmt_run_in_expr(e, target),
                            MatchBody::Block(b) => find_stmt_run(b, target),
                        })
                })
            }),
        // No variant below carries a `Block` *field*, so `expr_children`'s
        // total descent is complete for it — a block reached through a child
        // (a braced lambda body, say) comes back as an `Expr` and re-enters
        // this match at the `Block` arm above. A *new* variant that holds a
        // `Block` directly must be hand-matched up there alongside
        // `Block`/`If`/`Match`: appending it here descends past the block's
        // own statement list (`expr_children` flattens a block to its
        // statements' values), so a run inside it stops being extractable.
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::DecimalLit { .. }
        | ExprKind::DurationLit { .. }
        | ExprKind::StrLit(_)
        | ExprKind::InterpStr(_)
        | ExprKind::BoolLit(_)
        | ExprKind::Ident(_)
        | ExprKind::Call { .. }
        | ExprKind::Lambda(_)
        | ExprKind::BinOp(..)
        | ExprKind::UnaryOp(..)
        | ExprKind::Paren(_)
        | ExprKind::Ok(_)
        | ExprKind::Err(_)
        | ExprKind::Question(_)
        | ExprKind::RecordConstruction { .. }
        | ExprKind::FieldAccess { .. }
        | ExprKind::MethodCall { .. }
        | ExprKind::Is { .. }
        | ExprKind::Some(_)
        | ExprKind::None
        | ExprKind::UnitLit
        | ExprKind::RecordSpread { .. }
        | ExprKind::EffectPure(_)
        | ExprKind::Expect(_)
        | ExprKind::Val { .. }
        | ExprKind::Wire(_)
        | ExprKind::ListLit(_)
        | ExprKind::Observation(_)
        | ExprKind::Faults(_)
        | ExprKind::Trace { .. } => expr_children(e)
            .into_iter()
            .find_map(|c| find_stmt_run_in_expr(c, target)),
    }
}

/// Tries to align `target` exactly against `block`'s own statement list: its
/// start must equal some statement's start, and its end must equal either
/// another statement's end (the run stops before the tail) or the tail's own
/// end (the run includes it). No match — a partial-statement selection, or
/// one spanning more than this single block — returns `None`.
fn align_stmt_run(block: &Block, target: Span) -> Option<StmtRun<'_>> {
    // #2010: a `let … else` is a hidden binding plus the `match` it heads, which
    // holds the rest of the block; lifting the binding alone would strand the
    // rest of the block, so a run that reaches one is not offered.
    if block
        .statements
        .iter()
        .any(|s| matches!(s, Statement::Let(l) | Statement::EffectLet(l) if l.let_else.is_some()))
        && block
            .statements
            .last()
            .is_some_and(|s| s.span().end <= target.end)
    {
        return None;
    }
    let start_idx = block
        .statements
        .iter()
        .position(|s| s.span().start == target.start)?;
    if target.end == block.tail.span.end {
        let full = Span::new(
            block.statements[start_idx].span().start,
            block.tail.span.end,
        );
        if full == target {
            return Some((
                &block.statements[start_idx..],
                Some(block.tail.as_ref()),
                Vec::new(),
            ));
        }
    }
    let end_idx = block
        .statements
        .iter()
        .position(|s| s.span().end == target.end)?;
    if end_idx < start_idx {
        return None;
    }
    let full = Span::new(
        block.statements[start_idx].span().start,
        block.statements[end_idx].span().end,
    );
    if full != target {
        return None;
    }
    let mut remainder = Vec::new();
    for s in &block.statements[end_idx + 1..] {
        statement_exprs(s, &mut remainder);
    }
    remainder.push(&block.tail);
    Some((&block.statements[start_idx..=end_idx], None, remainder))
}

/// Whether any `let`/`<-` binding introduced by `stmts` (skipping `_`) is
/// still referenced in `remainder` — the expressions after the run within
/// the same block. A tail-excluded run whose binding leaks this way can't be
/// lifted away: the reference downstream would resolve to nothing once the
/// binding moves into the new `fn`.
fn leaks_a_binding(
    stmts: &[Statement],
    remainder: &[&Expr],
    locals: &[LocalBinding],
    run_span: Span,
) -> bool {
    let bound_names: Vec<&str> = stmts
        .iter()
        .filter_map(|s| match s {
            Statement::Let(l) | Statement::EffectLet(l) if l.name.name != "_" => {
                Some(l.name.name.as_str())
            }
            _ => None,
        })
        .collect();
    if bound_names.is_empty() {
        return false;
    }
    let mut used = Vec::new();
    for e in remainder {
        collect_idents(e, &mut used);
    }
    used.iter().any(|id| {
        bound_names.contains(&id.name.as_str())
            && locals_at(locals, id.span.start)
                .into_iter()
                .find(|b| b.name == id.name)
                .is_some_and(|b| contains(run_span, b.def_span))
    })
}

/// Whether any of `stmts` matches `pred` — checked on the statement itself
/// first, then recursively on every nested statement reachable through its
/// value expression's own `if`/`match`/block sub-expressions (arbitrarily
/// deep). The shared shape behind [`stmts_contain_effect_stmt`] and
/// [`stmts_contain_assign_stmt`]: both need "does this run perform/contain
/// X, however deeply nested" and neither can stop at the run's direct
/// top-level statements — a `~>`/`do`/`<-` or a `:=` can equally sit inside
/// an `if`/`match` branch that one of the run's own `let`s evaluates.
fn stmts_match(stmts: &[Statement], pred: &impl Fn(&Statement) -> bool) -> bool {
    stmts.iter().any(|s| pred(s) || stmt_value_matches(s, pred))
}

fn stmt_value_matches(s: &Statement, pred: &impl Fn(&Statement) -> bool) -> bool {
    // #1851: every expression of the statement, a principal's identity
    // included.
    let mut exprs = Vec::new();
    statement_exprs(s, &mut exprs);
    exprs.into_iter().any(|e| expr_matches(e, pred))
}

fn block_matches(b: &Block, pred: &impl Fn(&Statement) -> bool) -> bool {
    stmts_match(&b.statements, pred) || expr_matches(&b.tail, pred)
}

fn expr_matches(e: &Expr, pred: &impl Fn(&Statement) -> bool) -> bool {
    match &e.kind {
        ExprKind::Block(b) => block_matches(b, pred),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            expr_matches(cond, pred)
                || block_matches(then_block, pred)
                || block_matches(else_block, pred)
        }
        // #1800: a guard's statements count too. Skipping it let a run whose
        // guard held a `<-` be lifted to a `()`-returning fn, and one holding
        // a `:=` be lifted at all; neither typechecks.
        ExprKind::Match { discriminant, arms } => {
            expr_matches(discriminant, pred)
                || arms.iter().any(|arm| {
                    arm.guard.as_ref().is_some_and(|g| expr_matches(g, pred))
                        || match &arm.body {
                            MatchBody::Expr(e) => expr_matches(e, pred),
                            MatchBody::Block(b) => block_matches(b, pred),
                        }
                })
        }
        // No variant below carries a `Block` *field*, so `expr_children`'s
        // total descent is complete for it — a block reached through a child
        // (a braced lambda body, say) comes back as an `Expr` and re-enters
        // this match at the `Block` arm above. A *new* variant that holds a
        // `Block` directly must be hand-matched up there alongside
        // `Block`/`If`/`Match`: appending it here never reaches the block's
        // statement list (`expr_children` flattens a block to its statements'
        // values), so `pred` is never asked about the statements inside it.
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::DecimalLit { .. }
        | ExprKind::DurationLit { .. }
        | ExprKind::StrLit(_)
        | ExprKind::InterpStr(_)
        | ExprKind::BoolLit(_)
        | ExprKind::Ident(_)
        | ExprKind::Call { .. }
        | ExprKind::Lambda(_)
        | ExprKind::BinOp(..)
        | ExprKind::UnaryOp(..)
        | ExprKind::Paren(_)
        | ExprKind::Ok(_)
        | ExprKind::Err(_)
        | ExprKind::Question(_)
        | ExprKind::RecordConstruction { .. }
        | ExprKind::FieldAccess { .. }
        | ExprKind::MethodCall { .. }
        | ExprKind::Is { .. }
        | ExprKind::Some(_)
        | ExprKind::None
        | ExprKind::UnitLit
        | ExprKind::RecordSpread { .. }
        | ExprKind::EffectPure(_)
        | ExprKind::Expect(_)
        | ExprKind::Val { .. }
        | ExprKind::Wire(_)
        | ExprKind::ListLit(_)
        | ExprKind::Observation(_)
        | ExprKind::Faults(_)
        | ExprKind::Trace { .. } => expr_children(e).into_iter().any(|c| expr_matches(c, pred)),
    }
}

/// Whether any of `stmts` is (or contains, arbitrarily nested through
/// `if`/`match`/block sub-expressions) a `~>`/`do`/`<-` statement — the three
/// forms legal only in an effectful body (`ctx.effectful`, gated on the
/// enclosing return type being `Effect[_]`). Decides whether a tail-excluded
/// run's synthesised `fn` must return `Effect[()]` — so those forms stay
/// legal in the lifted body — rather than plain `()`.
fn stmts_contain_effect_stmt(stmts: &[Statement]) -> bool {
    stmts_match(stmts, &|s| {
        matches!(
            s,
            Statement::EffectLet(_) | Statement::Send(_) | Statement::Do(_) | Statement::Par(_)
        )
    })
}

/// Whether any of `stmts` is (or contains, arbitrarily nested through
/// `if`/`match`/block sub-expressions) a `:=` (`Cell` store write). A lifted
/// top-level `fn` has no `store` fields, so a run containing one anywhere —
/// not just as a direct top-level statement — would always fail to
/// typecheck once extracted.
fn stmts_contain_assign_stmt(stmts: &[Statement]) -> bool {
    stmts_match(stmts, &|s| matches!(s, Statement::Assign(_)))
}

/// `expr` and every `Ident` reference nested inside it — walked via
/// [`expr_children`], the same exhaustive-by-construction iterator `locate`
/// uses, so a future `ExprKind` variant can't silently drop out of either
/// walk without the other noticing.
///
/// #2100 slice 2: a called name (`f(…)`) counts too. It calls whatever `f`
/// resolves to, a local closure included, so extract-function threads a
/// called local through as a parameter, and the binder and leak checks see
/// it.
fn collect_idents<'a>(expr: &'a Expr, out: &mut Vec<&'a Ident>) {
    match &expr.kind {
        ExprKind::Ident(id) => out.push(id),
        ExprKind::Call { name, .. } => out.push(name),
        _ => {}
    }
    for child in expr_children(expr) {
        collect_idents(child, out);
    }
}

/// The whitespace-only run from `offset`'s line start up to `offset` — empty
/// if that run isn't pure whitespace (e.g. a single-line body has no
/// indentation to mirror; `bynk fmt` cleans up the result, the same
/// tolerance the quick-fix path takes).
fn line_indent(text: &str, offset: usize) -> String {
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let prefix = &text[line_start..offset];
    if prefix.chars().all(|c| c == ' ' || c == '\t') {
        prefix.to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #2100 slice 2: a `?` returns from the enclosing body unless it sits in
    /// a lambda, whose body is its own return scope.
    #[test]
    fn early_return_detection_stops_at_a_lambda() {
        let src = "commons c\n\nfn g(r: Result[Int, String]) -> Result[Int, String] {\n  let a = r? + 1\n  let f = (x: Result[Int, String]) => Ok(x? + 1)\n  f(r)\n}\n";
        let tokens = tokenize(src).unwrap();
        let (Some(SourceUnit::Commons(c)), _) = parse_unit_with_recovery(&tokens, src) else {
            panic!("parses");
        };
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!("a fn");
        };
        let value = |i: usize| match &f.body.statements[i] {
            Statement::Let(l) => &l.value,
            _ => panic!("a let"),
        };
        assert!(has_early_return(value(0)));
        assert!(!has_early_return(value(1)));
    }

    fn actions_for(text: &str, needle: &str) -> Vec<CodeActionOrCommand> {
        let start = text.find(needle).expect("needle present");
        let requested = Span::new(start, start + needle.len());
        let uri = Url::parse("file:///a.bynk").unwrap();
        extract_variable(text, requested, &uri, Some(3))
    }

    fn sole_edit(action: &CodeActionOrCommand) -> Vec<TextEdit> {
        let CodeActionOrCommand::CodeAction(a) = action else {
            panic!("expected a CodeAction");
        };
        assert_eq!(a.kind, Some(CodeActionKind::REFACTOR_EXTRACT));
        let Some(DocumentChanges::Edits(doc_edits)) = &a.edit.as_ref().unwrap().document_changes
        else {
            panic!("expected versioned document edits");
        };
        assert_eq!(doc_edits[0].text_document.version, Some(3));
        doc_edits[0]
            .edits
            .iter()
            .map(|e| {
                let OneOf::Left(edit) = e else {
                    panic!("expected a plain TextEdit")
                };
                edit.clone()
            })
            .collect()
    }

    #[test]
    fn extracts_a_sub_expression_of_a_let_value() {
        let src = "context c\n\nfn f() -> Int {\n  let y = 1 + 2\n  y\n}\n";
        let actions = actions_for(src, "1 + 2");
        assert_eq!(actions.len(), 1);
        let edits = sole_edit(&actions[0]);
        assert_eq!(edits.len(), 2);
        assert!(edits[0].new_text.starts_with("let extracted = 1 + 2\n"));
        assert_eq!(edits[1].new_text, "extracted");
        // The insertion is a zero-length range right at the start of the
        // `let y = …` line — same line the replacement (later on that line)
        // sits on, since positions are computed against the original text.
        assert_eq!(edits[0].range.start, edits[0].range.end);
        assert_eq!(edits[0].range.start.line, edits[1].range.start.line);
        assert!(edits[0].range.start.character < edits[1].range.start.character);
    }

    #[test]
    fn extracts_the_whole_tail_expression() {
        let src = "context c\n\nfn f() -> Int {\n  let x = 1\n  x + 1\n}\n";
        let actions = actions_for(src, "x + 1");
        let edits = sole_edit(&actions[0]);
        assert!(edits[0].new_text.starts_with("let extracted = x + 1\n"));
        assert_eq!(edits[1].new_text, "extracted");
    }

    #[test]
    fn resets_the_insertion_point_inside_a_nested_if_block() {
        let src = concat!(
            "context c\n\n",
            "fn f(n: Int) -> Int {\n",
            "  if n > 0 {\n",
            "    let z = n * 2\n",
            "    z\n",
            "  } else {\n",
            "    0\n",
            "  }\n",
            "}\n",
        );
        let actions = actions_for(src, "n * 2");
        let edits = sole_edit(&actions[0]);
        // Inserted right above `let z = …` inside the `if` branch, not above
        // the whole `if` statement.
        let insert_line = edits[0].range.start.line;
        let let_z_line = crate::position::offset_to_position(src, src.find("let z").unwrap()).line;
        assert_eq!(insert_line, let_z_line);
    }

    #[test]
    fn resets_the_insertion_point_inside_a_nested_match_arm_block() {
        let src = concat!(
            "context c\n\n",
            "fn f(n: Int) -> Int {\n",
            "  match n {\n",
            "    0 => {\n",
            "      let z = n * 2\n",
            "      z\n",
            "    }\n",
            "    _ => 0\n",
            "  }\n",
            "}\n",
        );
        let actions = actions_for(src, "n * 2");
        let edits = sole_edit(&actions[0]);
        // Inserted right above `let z = …` inside the arm's block, not above
        // the whole `match`.
        let insert_line = edits[0].range.start.line;
        let let_z_line = crate::position::offset_to_position(src, src.find("let z").unwrap()).line;
        assert_eq!(insert_line, let_z_line);
    }

    /// #1819: the `let` goes above the enclosing statement, where an
    /// expression arm's pattern bindings and a lambda's parameters are not in
    /// scope, so a selection reading one is declined, not hoisted unbound.
    #[test]
    fn declines_a_selection_reading_an_arm_or_lambda_binding() {
        let arm = concat!(
            "context c\n\n",
            "fn f(o: Option[Int]) -> Int {\n",
            "  let a = match o {\n",
            "    Some(x) => x + 1\n",
            "    _ => 0\n",
            "  }\n",
            "  a\n",
            "}\n",
        );
        assert!(actions_for(arm, "x + 1").is_empty(), "arm binding hoisted");
        let lambda = concat!(
            "context c\n\n",
            "fn f(xs: List[Int]) -> List[Int] {\n",
            "  let ys = xs.map((v) => v * 2)\n",
            "  ys\n",
            "}\n",
        );
        assert!(
            actions_for(lambda, "v * 2").is_empty(),
            "lambda parameter hoisted"
        );
        // A guard extracts the whole `match` (#1800 review), never the guard
        // alone, so the `let` never reads the arm's binding out of scope.
        let guard = concat!(
            "context c\n\n",
            "fn f(o: Option[Int]) -> Int {\n",
            "  let a = match o {\n",
            "    Some(x) if x > 1 => 1\n",
            "    _ => 0\n",
            "  }\n",
            "  a\n",
            "}\n",
        );
        let guard_actions = actions_for(guard, "x > 1");
        assert_eq!(
            guard_actions.len(),
            1,
            "a guard selection extracts the match"
        );
        {
            let edits = sole_edit(&guard_actions[0]);
            assert!(
                edits[0].new_text.starts_with("let extracted = match o {"),
                "a guard selection must extract the whole match: {:?}",
                edits[0].new_text
            );
        }
    }

    /// #1819's boundary: inside an arm or a lambda, a selection that reads no
    /// binding of theirs still extracts above the statement.
    #[test]
    fn still_extracts_inside_an_arm_or_lambda_when_no_binding_is_read() {
        let arm = concat!(
            "context c\n\n",
            "fn f(o: Option[Int], n: Int) -> Int {\n",
            "  let a = match o {\n",
            "    Some(x) => n + 1\n",
            "    _ => 0\n",
            "  }\n",
            "  a\n",
            "}\n",
        );
        let edits = sole_edit(&actions_for(arm, "n + 1")[0]);
        assert!(edits[0].new_text.starts_with("let extracted = n + 1\n"));
        let lambda = concat!(
            "context c\n\n",
            "fn f(xs: List[Int], k: Int) -> List[Int] {\n",
            "  let ys = xs.map((v) => k * 2)\n",
            "  ys\n",
            "}\n",
        );
        let edits = sole_edit(&actions_for(lambda, "k * 2")[0]);
        assert!(edits[0].new_text.starts_with("let extracted = k * 2\n"));
    }

    /// #1880 review: the same scope rule for the other binders. An `is` on the
    /// left of `&&` / `implies` binds for the right operand only, and an
    /// observation's `with` predicate reads the observed operation's
    /// parameters, which the tree doesn't bind.
    #[test]
    fn declines_a_selection_reading_an_is_binding_or_an_observed_parameter() {
        let and = concat!(
            "context c\n\n",
            "fn f(r: Result[Int, String]) -> Bool {\n",
            "  let ok = r is Ok(n) && n > 0\n",
            "  ok\n",
            "}\n",
        );
        assert!(actions_for(and, "n > 0").is_empty(), "`&&` binding hoisted");
        let implies = concat!(
            "context c\n\n",
            "fn f(r: Result[Int, String]) -> Bool {\n",
            "  let ok = r is Ok(n) implies n > 0\n",
            "  ok\n",
            "}\n",
        );
        assert!(
            actions_for(implies, "n > 0").is_empty(),
            "`implies` binding hoisted"
        );
        let condition = concat!(
            "context c\n\n",
            "fn f(r: Result[Int, String]) -> Int {\n",
            "  if r is Ok(n) && n > 0 {\n",
            "    1\n",
            "  } else {\n",
            "    0\n",
            "  }\n",
            "}\n",
        );
        assert!(
            actions_for(condition, "n > 0").is_empty(),
            "`if` condition binding hoisted"
        );
        let observed = concat!(
            "suite c {\n",
            "  case \"logs\" {\n",
            "    expect Logger.log called with msg == \"start\"\n",
            "  }\n",
            "}\n",
        );
        assert!(
            actions_for(observed, "msg == \"start\"").is_empty(),
            "observed parameter hoisted"
        );
        // The left operand is outside the `is` scope, so it still extracts.
        let lhs = concat!(
            "context c\n\n",
            "fn f(r: Result[Int, String], k: Int) -> Bool {\n",
            "  let ok = k > 1 && r is Ok(n)\n",
            "  ok\n",
            "}\n",
        );
        let edits = sole_edit(&actions_for(lhs, "k > 1")[0]);
        assert!(edits[0].new_text.starts_with("let extracted = k > 1\n"));
    }

    /// #1880 review: a block body puts the `let` inside the binding's scope,
    /// so a selection reading the binding still extracts there.
    #[test]
    fn still_extracts_a_binding_read_inside_a_block_body() {
        let arm = concat!(
            "context c\n\n",
            "fn f(o: Option[Int]) -> Int {\n",
            "  let a = match o {\n",
            "    Some(x) => {\n",
            "      let b = x + 1\n",
            "      b\n",
            "    }\n",
            "    _ => 0\n",
            "  }\n",
            "  a\n",
            "}\n",
        );
        let edits = sole_edit(&actions_for(arm, "x + 1")[0]);
        assert!(edits[0].new_text.starts_with("let extracted = x + 1\n"));
        let lambda = concat!(
            "context c\n\n",
            "fn f(xs: List[Int]) -> List[Int] {\n",
            "  let ys = xs.map((v) => {\n",
            "    let w = v * 2\n",
            "    w\n",
            "  })\n",
            "  ys\n",
            "}\n",
        );
        let edits = sole_edit(&actions_for(lambda, "v * 2")[0]);
        assert!(edits[0].new_text.starts_with("let extracted = v * 2\n"));
    }

    #[test]
    fn selection_crossing_a_statement_boundary_offers_nothing() {
        let src = "context c\n\nfn f() -> Int {\n  let a = 1\n  let b = 2\n  a\n}\n";
        // Spans from inside `let a`'s value into the start of `let b` — no
        // single statement/tail value fully contains it.
        let actions = actions_for(src, "1\n  let b");
        assert!(actions.is_empty());
    }

    #[test]
    fn selection_of_the_whole_block_braces_included_offers_nothing() {
        let src = "context c\n\nfn f() -> Int {\n  let a = 1\n  a\n}\n";
        // The full body span, braces included — larger than any single
        // statement/tail value, so no candidate node covers it.
        let actions = actions_for(src, "{\n  let a = 1\n  a\n}");
        assert!(actions.is_empty());
    }

    #[test]
    fn extracts_a_call_argument() {
        let src = "context c\n\nfn f() -> Int {\n  g(1 + 2, 3)\n}\n";
        let actions = actions_for(src, "1 + 2");
        let edits = sole_edit(&actions[0]);
        assert_eq!(edits[1].new_text, "extracted");
        assert!(edits[0].new_text.contains("let extracted = 1 + 2"));
    }

    #[test]
    fn bumps_the_suffix_on_a_name_collision() {
        let src = "context c\n\nfn f() -> Int {\n  let extracted = 0\n  1 + 2\n}\n";
        let actions = actions_for(src, "1 + 2");
        let edits = sole_edit(&actions[0]);
        assert!(edits[0].new_text.starts_with("let extracted2 = 1 + 2\n"));
        assert_eq!(edits[1].new_text, "extracted2");
    }

    #[test]
    fn empty_selection_offers_nothing() {
        let src = "context c\n\nfn f() -> Int {\n  1 + 2\n}\n";
        let off = src.find("1 + 2").unwrap();
        let uri = Url::parse("file:///a.bynk").unwrap();
        let actions = extract_variable(src, Span::new(off, off), &uri, Some(1));
        assert!(actions.is_empty());
    }

    #[test]
    fn selection_outside_any_body_offers_nothing() {
        let src = "context c\n\nfn f() -> Int {\n  1 + 2\n}\n";
        let actions = actions_for(src, "context c");
        assert!(actions.is_empty());
    }

    #[test]
    fn preserves_indentation_on_a_multi_statement_block() {
        let src = "context c\n\nfn f() -> Int {\n    let a = 1\n    let b = a + 2\n    b\n}\n";
        let actions = actions_for(src, "a + 2");
        let edits = sole_edit(&actions[0]);
        assert_eq!(edits[0].new_text, "let extracted = a + 2\n    ");
    }

    mod extract_function_tests {
        use super::*;
        use bynk_check::locals::LocalKind;
        use bynk_check::requirements::RequirementSource;

        fn function_actions_for(
            text: &str,
            needle: &str,
            requirements: &[Requirement],
            locals: &[LocalBinding],
            expr_types: &[(Span, TyId)],
        ) -> Vec<CodeActionOrCommand> {
            let start = text.find(needle).expect("needle present");
            let requested = Span::new(start, start + needle.len());
            let uri = Url::parse("file:///a.bynk").unwrap();
            extract_function(
                text,
                requested,
                &uri,
                Some(3),
                requirements,
                locals,
                expr_types,
                &TYS,
            )
        }

        fn nth_offset(text: &str, needle: &str, n: usize) -> usize {
            text.match_indices(needle)
                .nth(n)
                .expect("occurrence present")
                .0
        }

        // Permissive (whole-file) scope: these tests exercise extract_function's
        // own gating/free-variable logic, not locals_at's scope resolution
        // (already covered in bynk-check::locals's own tests).
        fn param(text: &str, name: &str, ty: &str) -> LocalBinding {
            let def_start = nth_offset(text, name, 0);
            LocalBinding {
                name: name.to_string(),
                def_span: Span::new(def_start, def_start + name.len()),
                kind: LocalKind::Param,
                ty: ty.to_string(),
                scope: Span::new(0, text.len()),
            }
        }

        use bynk_check::checker::Ty;

        /// One table for every fixture in this module — `Types` is
        /// `Send + Sync` (T3.6b), so a `LazyLock` static is the least
        /// ceremonious way to give the `TyId`s below something to resolve
        /// against.
        static TYS: std::sync::LazyLock<Types> = std::sync::LazyLock::new(Types::new);

        fn int_type(text: &str, needle: &str) -> (Span, TyId) {
            let start = text.find(needle).expect("needle present");
            (
                Span::new(start, start + needle.len()),
                TYS.intern(Ty::Base(BaseType::Int)),
            )
        }

        /// A `let`-bound local — its def site is `name`'s first occurrence,
        /// same convention as [`param`].
        fn let_binding(text: &str, name: &str, ty: &str) -> LocalBinding {
            let def_start = nth_offset(text, name, 0);
            LocalBinding {
                name: name.to_string(),
                def_span: Span::new(def_start, def_start + name.len()),
                kind: LocalKind::Let,
                ty: ty.to_string(),
                scope: Span::new(0, text.len()),
            }
        }

        fn capability_use(site_needle_offset: usize, len: usize) -> Requirement {
            Requirement {
                capability: "Clock".to_string(),
                site: Span::new(site_needle_offset, site_needle_offset + len),
                source: RequirementSource::DirectCall {
                    op: "now".to_string(),
                },
                covered: false,
                materialize: None,
            }
        }

        #[test]
        fn extracts_a_free_variable_as_a_parameter() {
            let src = "context c\n\nfn f(num: Int) -> Int {\n  num * 2\n}\n";
            let locals = vec![param(src, "num", "Int")];
            let types = vec![int_type(src, "num * 2")];
            let actions = function_actions_for(src, "num * 2", &[], &locals, &types);
            assert_eq!(actions.len(), 1);
            let edits = sole_edit(&actions[0]);
            assert_eq!(
                edits[0].new_text,
                "fn extractedFn(num: Int) -> Int {\n  num * 2\n}\n\n"
            );
            // Inserted right above the enclosing `fn f`, not at the selection.
            assert_eq!(edits[0].range.start, Position::new(2, 0));
            assert_eq!(edits[1].new_text, "extractedFn(num)");
        }

        /// #1760: a name read only in a match-arm guard is still free in the
        /// selection, so it's threaded as a parameter. `expr_children` didn't
        /// visit guards, so `lim` was dropped and the extracted `fn` read an
        /// unbound name.
        #[test]
        fn a_name_read_only_in_a_guard_is_a_parameter() {
            let src = "context c\n\nfn f(num: Int, lim: Int) -> Int {\n  match num {\n    n if n > lim => 1\n    _ => 0\n  }\n}\n";
            let selection = "match num {\n    n if n > lim => 1\n    _ => 0\n  }";
            let locals = vec![param(src, "num", "Int"), param(src, "lim", "Int")];
            let types = vec![int_type(src, selection)];
            let actions = function_actions_for(src, selection, &[], &locals, &types);
            assert_eq!(actions.len(), 1);
            let edits = sole_edit(&actions[0]);
            assert!(
                edits[0]
                    .new_text
                    .starts_with("fn extractedFn(num: Int, lim: Int) -> Int {"),
                "{}",
                edits[0].new_text
            );
            assert_eq!(edits[1].new_text, "extractedFn(num, lim)");
        }

        #[test]
        fn no_free_variables_yields_a_nullary_call() {
            let src = "context c\n\nfn f() -> Int {\n  1 + 2\n}\n";
            let types = vec![int_type(src, "1 + 2")];
            let actions = function_actions_for(src, "1 + 2", &[], &[], &types);
            let edits = sole_edit(&actions[0]);
            assert_eq!(
                edits[0].new_text,
                "fn extractedFn() -> Int {\n  1 + 2\n}\n\n"
            );
            assert_eq!(edits[1].new_text, "extractedFn()");
        }

        #[test]
        fn capability_using_selection_offers_nothing() {
            let src = "context c\n\nfn f() -> Int {\n  1 + 2\n}\n";
            let site = src.find("1 + 2").unwrap();
            let reqs = vec![capability_use(site, "1 + 2".len())];
            let types = vec![int_type(src, "1 + 2")];
            let actions = function_actions_for(src, "1 + 2", &reqs, &[], &types);
            assert!(actions.is_empty());
        }

        #[test]
        fn bumps_the_suffix_on_a_name_collision() {
            let src =
                "context c\n\nfn extractedFn() -> Int {\n  0\n}\n\nfn f() -> Int {\n  1 + 2\n}\n";
            let types = vec![int_type(src, "1 + 2")];
            let actions = function_actions_for(src, "1 + 2", &[], &[], &types);
            let edits = sole_edit(&actions[0]);
            assert!(edits[0].new_text.starts_with("fn extractedFn2("));
            assert_eq!(edits[1].new_text, "extractedFn2()");
        }

        #[test]
        fn selection_outside_any_body_offers_nothing() {
            let src = "context c\n\nfn f() -> Int {\n  1 + 2\n}\n";
            let types = vec![int_type(src, "1 + 2")];
            let actions = function_actions_for(src, "context c", &[], &[], &types);
            assert!(actions.is_empty());
        }

        #[test]
        fn adapter_files_offer_nothing() {
            // An `Adapter` carries no Bynk bodies — `find_function_site` (like
            // `find_site`) short-circuits to `None` regardless of the selection.
            let src = "adapter tokens {\n  binding \"./b.ts\"\n}\n";
            let actions = function_actions_for(src, "binding", &[], &[], &[]);
            assert!(actions.is_empty());
        }

        #[test]
        fn suite_test_cases_offer_nothing() {
            // Unlike extract-variable (which can extract a `let` inside a test
            // case), extract-function needs a top-level item to insert the new
            // `fn` above — a `Suite` case has none, so this declines even where
            // extract-variable would offer something.
            let src = "suite thing\n\ncase \"it works\" {\n  1 + 2\n}\n";
            let types = vec![int_type(src, "1 + 2")];
            let actions = function_actions_for(src, "1 + 2", &[], &[], &types);
            assert!(actions.is_empty());
        }

        #[test]
        fn a_dirty_file_with_no_recorded_type_offers_nothing() {
            let src = "context c\n\nfn f() -> Int {\n  1 + 2\n}\n";
            // No expr_types entry for the selection — an expression the
            // checker never typed.
            let actions = function_actions_for(src, "1 + 2", &[], &[], &[]);
            assert!(actions.is_empty());
        }

        #[test]
        fn ambiguous_same_name_shadow_from_two_outer_scopes_declines() {
            let src = "context c\n\nfn f() -> Int {\n  x + x\n}\n";
            let types = vec![int_type(src, "x + x")];
            let first_x = nth_offset(src, "x + x", 0); // the first `x`'s offset
            let second_x = first_x + "x + ".len(); // the second `x`'s offset
            // Two distinct external bindings named `x`, each in scope only at
            // one of the two occurrences — an occurrence-position-dependent
            // resolution a real program could never actually produce, used
            // here purely to force the ambiguous-shadow branch.
            let locals = vec![
                LocalBinding {
                    name: "x".to_string(),
                    def_span: Span::new(0, 1),
                    kind: LocalKind::Let,
                    ty: "Int".to_string(),
                    scope: Span::new(first_x, first_x),
                },
                LocalBinding {
                    name: "x".to_string(),
                    def_span: Span::new(1, 2),
                    kind: LocalKind::Let,
                    ty: "Int".to_string(),
                    scope: Span::new(second_x, second_x),
                },
            ];
            let actions = function_actions_for(src, "x + x", &[], &locals, &types);
            assert!(actions.is_empty());
        }

        mod multi_statement_tests {
            use super::*;

            #[test]
            fn a_run_including_the_tail_becomes_a_call_expression() {
                let src = "context c\n\nfn f(num: Int) -> Int {\n  let a = num + 1\n  let b = a * 2\n  b + num\n}\n";
                let locals = vec![
                    param(src, "num", "Int"),
                    let_binding(src, "a", "Int"),
                    let_binding(src, "b", "Int"),
                ];
                let types = vec![int_type(src, "b + num")];
                let needle = "let a = num + 1\n  let b = a * 2\n  b + num";
                let actions = function_actions_for(src, needle, &[], &locals, &types);
                assert_eq!(actions.len(), 1);
                let edits = sole_edit(&actions[0]);
                assert_eq!(
                    edits[0].new_text,
                    format!("fn extractedFn(num: Int) -> Int {{\n  {needle}\n}}\n\n")
                );
                assert_eq!(edits[1].new_text, "extractedFn(num)");
            }

            #[test]
            fn a_run_excluding_the_tail_yields_a_discard_call() {
                let src = "context c\n\nfn f(num: Int) -> Int {\n  let a = num + 1\n  let valC = a * 2\n  num\n}\n";
                let locals = vec![
                    param(src, "num", "Int"),
                    let_binding(src, "a", "Int"),
                    let_binding(src, "valC", "Int"),
                ];
                let needle = "let a = num + 1\n  let valC = a * 2";
                let actions = function_actions_for(src, needle, &[], &locals, &[]);
                assert_eq!(actions.len(), 1);
                let edits = sole_edit(&actions[0]);
                assert_eq!(
                    edits[0].new_text,
                    format!("fn extractedFn(num: Int) -> () {{\n  {needle}\n}}\n\n")
                );
                assert_eq!(edits[1].new_text, "let _ = extractedFn(num)");
            }

            #[test]
            fn an_effectful_run_excluding_the_tail_yields_an_effect_return_and_a_do_call() {
                // `do` is only legal in an effectful body semantically (a
                // checker rule), but the parser accepts it regardless of the
                // enclosing signature — extract_function does no
                // type-checking of its own, so this exercises the
                // return-type/call-site choice in isolation.
                let src = "context c\n\nfn f() -> Effect[()] {\n  do g()\n  do h()\n  ()\n}\n";
                let needle = "do g()\n  do h()";
                let actions = function_actions_for(src, needle, &[], &[], &[]);
                assert_eq!(actions.len(), 1);
                let edits = sole_edit(&actions[0]);
                assert_eq!(
                    edits[0].new_text,
                    format!("fn extractedFn() -> Effect[()] {{\n  {needle}\n}}\n\n")
                );
                assert_eq!(edits[1].new_text, "do extractedFn()");
            }

            #[test]
            fn a_run_whose_match_guard_binds_an_effect_yields_an_effect_return() {
                // #1800 review: the `<-` sits only in an arm's guard, so
                // `stmts_contain_effect_stmt` must search guards for the lifted
                // `fn` to return `Effect[()]` and the call site to be `do`.
                let src = concat!(
                    "context c\n\n",
                    "fn f(o: Option[Int]) -> Effect[()] {\n",
                    "  let a = match o {\n",
                    "    Some(n) if if n > 0 {\n",
                    "      let _ <- g()\n",
                    "      true\n",
                    "    } else {\n",
                    "      false\n",
                    "    } => n\n",
                    "    _ => 0\n",
                    "  }\n",
                    "  ()\n",
                    "}\n",
                );
                let needle = "let a = match o {\n    Some(n) if if n > 0 {\n      let _ <- g()\n      true\n    } else {\n      false\n    } => n\n    _ => 0\n  }";
                let actions = function_actions_for(src, needle, &[], &[], &[]);
                assert_eq!(actions.len(), 1);
                let edits = sole_edit(&actions[0]);
                assert!(
                    edits[0]
                        .new_text
                        .starts_with("fn extractedFn() -> Effect[()] {"),
                    "{}",
                    edits[0].new_text
                );
                assert_eq!(edits[1].new_text, "do extractedFn()");
            }

            #[test]
            fn a_binding_the_run_introduces_still_used_in_the_tail_declines() {
                let src = "context c\n\nfn f(num: Int) -> Int {\n  let a = num + 1\n  let b = a * 2\n  b\n}\n";
                let locals = vec![
                    param(src, "num", "Int"),
                    let_binding(src, "a", "Int"),
                    let_binding(src, "b", "Int"),
                ];
                // Excludes the tail `b`, which reads the run's own `let b` —
                // lifting the run away would strand that reference.
                let needle = "let a = num + 1\n  let b = a * 2";
                let actions = function_actions_for(src, needle, &[], &locals, &[]);
                assert!(actions.is_empty());
            }

            #[test]
            fn a_run_containing_a_cell_write_declines() {
                // A lifted top-level `fn` has no `store` fields — a `:=`
                // inside the run would always fail to resolve its target.
                let src = "context c\n\nfn f() -> Int {\n  cell := 1\n  x\n}\n";
                let actions = function_actions_for(src, "cell := 1", &[], &[], &[]);
                assert!(actions.is_empty());
            }

            #[test]
            fn a_run_containing_a_nested_cell_write_declines() {
                // The `:=` is nested inside an `if`-branch within the run's
                // own `let`, not a direct top-level statement in the run —
                // `stmts_contain_assign_stmt` must recurse to catch it, the
                // same way `stmts_contain_effect_stmt` already does for
                // `~>`/`do`/`<-`.
                let src = concat!(
                    "context c\n\n",
                    "fn f(cond: Bool) -> Int {\n",
                    "  let a = if cond {\n",
                    "    cell := 1\n",
                    "    0\n",
                    "  } else {\n",
                    "    1\n",
                    "  }\n",
                    "  a\n",
                    "}\n",
                );
                let needle = "let a = if cond {\n    cell := 1\n    0\n  } else {\n    1\n  }";
                let actions = function_actions_for(src, needle, &[], &[], &[]);
                assert!(actions.is_empty());
            }

            #[test]
            fn a_run_containing_a_cell_write_in_a_match_guard_declines() {
                // #1800: the `:=` sits in an arm's guard, which
                // `stmts_contain_assign_stmt` must search like an arm body.
                let src = concat!(
                    "context c\n\n",
                    "fn f(o: Option[Int]) -> Int {\n",
                    "  let a = match o {\n",
                    "    Some(x) if if x > 0 {\n",
                    "      cell := x\n",
                    "      true\n",
                    "    } else {\n",
                    "      false\n",
                    "    } => x\n",
                    "    _ => 0\n",
                    "  }\n",
                    "  a\n",
                    "}\n",
                );
                let needle = "let a = match o {\n    Some(x) if if x > 0 {\n      cell := x\n      true\n    } else {\n      false\n    } => x\n    _ => 0\n  }";
                let actions = function_actions_for(src, needle, &[], &[], &[]);
                assert!(actions.is_empty());
            }

            #[test]
            fn a_run_found_inside_a_match_guard() {
                // #1800: the run sits in the `then` block of a guard's `if`.
                // The guard's tail reads only `num`, so lifting the run
                // strands nothing.
                let src = concat!(
                    "context c\n\n",
                    "fn f(o: Option[Int]) -> Int {\n",
                    "  match o {\n",
                    "    Some(num) if if num > 0 {\n",
                    "      let valA = num * 2\n",
                    "      let valB = valA + 1\n",
                    "      num > 3\n",
                    "    } else {\n",
                    "      false\n",
                    "    } => num\n",
                    "    _ => 0\n",
                    "  }\n",
                    "}\n",
                );
                let locals = vec![
                    param(src, "num", "Int"),
                    let_binding(src, "valA", "Int"),
                    let_binding(src, "valB", "Int"),
                ];
                let needle = "let valA = num * 2\n      let valB = valA + 1";
                let actions = function_actions_for(src, needle, &[], &locals, &[]);
                assert_eq!(actions.len(), 1, "a run in a guard is extractable");
            }

            #[test]
            fn a_run_found_inside_a_nested_if_branch() {
                let src = concat!(
                    "context c\n\n",
                    "fn f(num: Int) -> Int {\n",
                    "  if num > 0 {\n",
                    "    let valA = num * 2\n",
                    "    let valB = valA + 1\n",
                    "    valB\n",
                    "  } else {\n",
                    "    0\n",
                    "  }\n",
                    "}\n",
                );
                let locals = vec![
                    param(src, "num", "Int"),
                    let_binding(src, "valA", "Int"),
                    let_binding(src, "valB", "Int"),
                ];
                let tail_val_b = nth_offset(src, "valB", 1); // skip `let valB`'s own def
                let types = vec![(
                    Span::new(tail_val_b, tail_val_b + "valB".len()),
                    TYS.intern(Ty::Base(BaseType::Int)),
                )];
                let needle = "let valA = num * 2\n    let valB = valA + 1\n    valB";
                let actions = function_actions_for(src, needle, &[], &locals, &types);
                assert_eq!(actions.len(), 1);
                let edits = sole_edit(&actions[0]);
                assert_eq!(
                    edits[0].new_text,
                    format!("fn extractedFn(num: Int) -> Int {{\n  {needle}\n}}\n\n")
                );
                // Inserted above the enclosing `fn f`, not inside the `if`.
                assert_eq!(edits[0].range.start, Position::new(2, 0));
                assert_eq!(edits[1].new_text, "extractedFn(num)");
            }

            /// #2010: a `let … else` line is the hidden binding that heads the
            /// rest of its block, so neither refactoring lifts it — alone or
            /// with what follows — while its value is still extractable.
            #[test]
            fn a_let_else_statement_is_not_extracted() {
                let src = "context c\n\nfn f(r: Result[Int, String]) -> Int {\n  let Ok(n) = load(r) else { 0 }\n  let m = n + 1\n  m\n}\n";
                let uri = Url::parse("file:///a.bynk").unwrap();
                let locals = vec![param(src, "r", "Result[Int, String]")];
                for sel in [
                    "let Ok(n) = load(r) else { 0 }",
                    "let Ok(n) = load(r) else { 0 }\n  let m = n + 1",
                    "let Ok(n) = load(r) else { 0 }\n  let m = n + 1\n  m",
                ] {
                    let start = src.find(sel).unwrap();
                    let requested = Span::new(start, start + sel.len());
                    assert!(
                        extract_function(src, requested, &uri, Some(3), &[], &locals, &[], &TYS)
                            .is_empty(),
                        "extract function must decline {sel:?}"
                    );
                    assert!(
                        extract_variable(src, requested, &uri, Some(3)).is_empty(),
                        "extract variable must decline {sel:?}"
                    );
                }
                let start = src.find("load(r)").unwrap();
                let requested = Span::new(start, start + "load(r)".len());
                let actions = extract_variable(src, requested, &uri, Some(3));
                assert_eq!(actions.len(), 1, "the value is the author's expression");
                let edits = sole_edit(&actions[0]);
                assert!(
                    edits
                        .iter()
                        .any(|e| e.new_text.starts_with("let extracted = load(r)")),
                    "{edits:?}"
                );
            }

            /// #2010: statements after a `let … else` live in its pattern's arm
            /// and are an ordinary run there.
            #[test]
            fn statements_after_a_let_else_still_extract() {
                let src = "context c\n\nfn f(r: Result[Int, String]) -> Int {\n  let Ok(n) = load(r) else { 0 }\n  let m = n + 1\n  let k = m * 2\n  k\n}\n";
                let uri = Url::parse("file:///a.bynk").unwrap();
                let sel = "let m = n + 1\n  let k = m * 2\n  k";
                let start = src.find(sel).unwrap();
                let requested = Span::new(start, start + sel.len());
                let locals = vec![
                    param(src, "r", "Result[Int, String]"),
                    let_binding(src, "m", "Int"),
                    let_binding(src, "k", "Int"),
                    LocalBinding {
                        name: "n".to_string(),
                        def_span: Span::new(nth_offset(src, "n)", 0), nth_offset(src, "n)", 0) + 1),
                        kind: LocalKind::Let,
                        ty: "Int".to_string(),
                        scope: Span::new(0, src.len()),
                    },
                ];
                // The run includes the tail, whose type becomes the lifted
                // `fn`'s return type.
                let tail = src.find("  k\n}").unwrap() + 2;
                let types = vec![(
                    Span::new(tail, tail + 1),
                    TYS.intern(Ty::Base(BaseType::Int)),
                )];
                let actions =
                    extract_function(src, requested, &uri, Some(3), &[], &locals, &types, &TYS);
                assert_eq!(actions.len(), 1, "{actions:#?}");
            }

            #[test]
            fn a_selection_extending_past_a_statement_boundary_declines() {
                let src = "context c\n\nfn f(num: Int) -> Int {\n  let a = num + 1\n  let valC = a * 2\n  num\n}\n";
                let start = src.find("let a = num + 1").unwrap();
                // One byte short of a full second statement — doesn't align
                // with any statement/tail boundary in either direction, so
                // neither the multi-statement nor the single-expression
                // algorithm matches.
                let end = start + "let a = num + 1\n  let valC = a * 2".len() - 1;
                let requested = Span::new(start, end);
                let uri = Url::parse("file:///a.bynk").unwrap();
                let actions = extract_function(src, requested, &uri, Some(3), &[], &[], &[], &TYS);
                assert!(actions.is_empty());
            }

            #[test]
            fn a_selection_padded_with_surrounding_whitespace_still_aligns() {
                let src = "context c\n\nfn f(num: Int) -> Int {\n  let a = num + 1\n  let valC = a * 2\n  num\n}\n";
                let inner = "let a = num + 1\n  let valC = a * 2";
                let start = src.find(inner).unwrap();
                // Pads onto the leading two-space indent and the trailing
                // newline — a common "select whole lines" editor gesture.
                let requested = Span::new(start - 2, start + inner.len() + 1);
                let locals = vec![
                    param(src, "num", "Int"),
                    let_binding(src, "a", "Int"),
                    let_binding(src, "valC", "Int"),
                ];
                let uri = Url::parse("file:///a.bynk").unwrap();
                let actions =
                    extract_function(src, requested, &uri, Some(3), &[], &locals, &[], &TYS);
                assert_eq!(actions.len(), 1);
                let edits = sole_edit(&actions[0]);
                assert_eq!(edits[1].new_text, "let _ = extractedFn(num)");
            }
        }

        /// Fixtures run through the real analysis rather than hand-built
        /// tables, so `expr_types`/`locals` carry exactly what the
        /// `code_action` handler sees — real `FileId`s on every span, and
        /// (ADR 0094) best-effort partial types for a file with errors, where
        /// an expression whose typing failed is recorded as `Ty::Error`.
        mod analysed_fixtures {
            use super::*;
            use std::path::Path;
            use std::sync::Arc;

            /// `src`'s `(expr_types, locals, tys)` as the single-file context
            /// `c.bynk` of a fresh on-disk project, asserting the file's
            /// diagnostic categories are exactly `expected`.
            fn analysed(
                test_name: &str,
                src: &str,
                expected: &[&str],
            ) -> (Vec<(Span, TyId)>, Vec<LocalBinding>, Arc<Types>) {
                let root = std::env::temp_dir().join(format!(
                    "bynk-lsp-extract-{test_name}-{}",
                    std::process::id()
                ));
                let _ = std::fs::remove_dir_all(&root);
                std::fs::create_dir_all(&root).expect("create test root");
                std::fs::write(root.join("c.bynk"), src).expect("write file");
                let root = root.canonicalize().expect("canonical root");
                let overlay = HashMap::from([(root.join("c.bynk"), src.to_string())]);
                let result = bynk_ide::diagnose_project(&root, &overlay);
                let _ = std::fs::remove_dir_all(&root);
                let rel = Path::new("c.bynk");
                let categories: Vec<&str> = result
                    .files
                    .iter()
                    .filter(|f| f.source_path == rel)
                    .flat_map(|f| f.diagnostics.iter().map(|d| d.error.category))
                    .collect();
                assert_eq!(categories, expected);
                (
                    result.expr_types.get(rel).cloned().unwrap_or_default(),
                    result.locals.get(rel).cloned().unwrap_or_default(),
                    Arc::clone(&result.ty_intern),
                )
            }

            /// The new `fn` each offered action would insert.
            fn offered(test_name: &str, src: &str, needle: &str, expected: &[&str]) -> Vec<String> {
                let (types, locals, tys) = analysed(test_name, src, expected);
                let start = src.find(needle).expect("needle present");
                let uri = Url::parse("file:///c.bynk").unwrap();
                extract_function(
                    src,
                    Span::new(start, start + needle.len()),
                    &uri,
                    Some(3),
                    &[],
                    &locals,
                    &types,
                    &tys,
                )
                .iter()
                .map(|a| sole_edit(a)[0].new_text.clone())
                .collect()
            }

            /// The handler reparses the buffer (spans with no file identity)
            /// but the analysis's spans carry the file's real `FileId` (T3.5),
            /// so matching a recorded type must compare positions only — an
            /// exact `Span` match never found one, and the action was never
            /// offered against real analysis output.
            #[test]
            fn a_clean_file_offers_the_action() {
                let src = "context c\n\nfn f(num: Int) -> Int {\n  num * 2\n}\n";
                assert_eq!(
                    offered("clean", src, "num * 2", &[]),
                    ["fn extractedFn(num: Int) -> Int {\n  num * 2\n}\n\n"]
                );
            }

            /// The selection itself failed to type: its recorded type is
            /// `Ty::Error`, which must decline, not render `-> <type error>`.
            #[test]
            fn an_error_typed_selection_declines() {
                let src = "context c\n\nfn f(num: Int) -> Int {\n  num + \"s\"\n}\n";
                let offered = offered(
                    "error-typed-selection",
                    src,
                    "num + \"s\"",
                    &["bynk.types.type_mismatch"],
                );
                assert!(offered.is_empty(), "expected no action; got {offered:?}");
            }

            /// A well-typed selection that reads an error-typed binding: the
            /// checker records no local for a `let` whose value failed to
            /// type, so `z` would not be threaded as a parameter and the
            /// lifted `fn` would read an unbound name.
            #[test]
            fn a_selection_reading_an_error_typed_binding_declines() {
                let src = "context c\n\nfn f(num: Int) -> Int {\n  let z = num + \"s\"\n  let w = if num > 0 {\n    let q = z\n    1\n  } else {\n    2\n  }\n  w\n}\n";
                let offered = offered(
                    "error-typed-binding",
                    src,
                    "if num > 0 {\n    let q = z\n    1\n  } else {\n    2\n  }",
                    &["bynk.types.type_mismatch"],
                );
                assert!(offered.is_empty(), "expected no action; got {offered:?}");
            }

            /// A tail-less statement run synthesises its `()` return type and
            /// never looks one up, so the `Ty::Error` gate is its only guard:
            /// `z`'s `let` is unrecorded in `locals`, so without the gate the
            /// run would lift with `z` unbound.
            #[test]
            fn a_statement_run_reading_an_error_typed_binding_declines() {
                let src = "context c\n\nfn f(num: Int) -> Int {\n  let z = num + \"s\"\n  let q = z\n  num\n}\n";
                let offered = offered(
                    "error-typed-stmts",
                    src,
                    "let q = z",
                    &["bynk.types.type_mismatch"],
                );
                assert!(offered.is_empty(), "expected no action; got {offered:?}");
            }

            /// An error elsewhere in the file doesn't block a selection that
            /// typed cleanly (ADR 0094's partial map).
            #[test]
            fn an_error_elsewhere_still_offers_the_action() {
                let src = "context c\n\nfn f(num: Int) -> Int {\n  num * 2\n}\n\nfn g(num: Int) -> Int {\n  num + \"s\"\n}\n";
                assert_eq!(
                    offered(
                        "error-elsewhere",
                        src,
                        "num * 2",
                        &["bynk.types.type_mismatch"]
                    ),
                    ["fn extractedFn(num: Int) -> Int {\n  num * 2\n}\n\n"]
                );
            }
        }
    }

    #[test]
    fn extracts_inside_the_value_of_a_let_with_a_call_site_principal() {
        // #1766 put a principal's identity first in `statement_exprs`; the
        // value is the statement's last expression, not its first.
        let src = "suite demo.s {\n  case \"c\" {\n    let who = \"alice\"\n    let item <- api.get(1 + 2) by User(who)\n    expect item\n  }\n}\n";
        assert_eq!(actions_for(src, "1 + 2").len(), 1);
    }
}
