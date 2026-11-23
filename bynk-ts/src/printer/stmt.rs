use super::compact::{
    inline_block_fits_one_line, render_compact_stmts, render_inline_block, render_inline_stmt,
};
use super::decl::render_decl;
use super::expr::render_expr;
use super::multiline::render_stmt_level_expr;
use super::types::render_type;
use super::{MergeTarget, indent, note_span, note_span_at};
use crate::program::{TsBindingName, TsDeclarator, TsExpr, TsStmt, TsStmtKind, TsSwitchCase};

/// `emit_doc_block`'s own real rendering, byte-for-byte (#1333): `/**`
/// opens; one ` * <line>` per non-blank source line, a literal `*/`
/// escaped to `*\/` (issue #720 — otherwise it would close the comment
/// early and let trailing text land as executable top-level TypeScript); a
/// blank source line prints as a bare ` *`, no trailing space; `*/`
/// closes. All at `depth`'s own 2-space-per-level indent.
pub(super) fn render_doc_comment(out: &mut String, text: &str, depth: usize) {
    let ind = indent(depth);
    out.push_str(&ind);
    out.push_str("/**\n");
    for line in text.lines() {
        let trimmed = line.trim_end();
        out.push_str(&ind);
        if trimmed.is_empty() {
            out.push_str(" *\n");
        } else {
            out.push_str(" * ");
            out.push_str(&trimmed.replace("*/", "*\\/"));
            out.push('\n');
        }
    }
    out.push_str(&ind);
    out.push_str(" */\n");
}

/// Render one statement, including its own leading indent.
///
/// `map`: #1477's own thread-through, checked *here*, once, rather than
/// re-checked at every one of this function's own many call sites (review
/// of #1488, finding 2: the first version of this mechanism only checked a
/// `Vec<TsStmt>` body's own *direct* children, inside `render_block_stmts`
/// — silently dropping a `nested_map` set on a statement handed to `print`/
/// `print_stmt_and_merge` directly, or on a statement nested one level
/// deeper still, inside a `Block`/`Switch` case/`try`/`catch` body within a
/// real function body). Checking unconditionally at this one entry point,
/// before dispatching on `stmt.kind`, means every caller — however deep —
/// gets the merge for free by simply forwarding `map` on, the same way
/// `depth` already threads through unconditionally; a caller that has
/// nothing to merge just passes `None`, as most still do.
pub(super) fn render_stmt(
    out: &mut String,
    stmt: &TsStmt,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    note_span(out.len(), stmt, map.as_mut());
    if let Some(nested) = &stmt.nested_map
        && let Some(target) = map.as_mut()
    {
        let base = out.len();
        render_stmt_kind(out, stmt, depth, None);
        target
            .map
            .merge(nested, &out[base..], out, base, target.source_id);
        return;
    }
    render_stmt_kind(out, stmt, depth, map);
}

/// `try { … }` with its optional `catch` and `finally` (1A.21, #3722, Decision C).
fn render_try_catch(
    out: &mut String,
    depth: usize,
    try_block: &TsStmt,
    catch_param: Option<&str>,
    catch_block: Option<&TsStmt>,
    finally_block: Option<&TsStmt>,
    mut map: Option<MergeTarget<'_>>,
) {
    debug_assert!(
        catch_block.is_some() || finally_block.is_some(),
        "TsStmtKind::TryCatch needs a `catch` or a `finally`; a bare `try` is a syntax error"
    );
    out.push_str(&indent(depth));
    out.push_str("try");
    render_block_body(out, try_block, depth, map.as_mut().map(|t| t.reborrow()));
    if let Some(catch_block) = catch_block {
        out.push_str(" catch");
        if let Some(p) = catch_param {
            out.push_str(" (");
            out.push_str(p);
            out.push(')');
        }
        render_block_body(out, catch_block, depth, map.as_mut().map(|t| t.reborrow()));
    }
    if let Some(finally_block) = finally_block {
        out.push_str(" finally");
        render_block_body(
            out,
            finally_block,
            depth,
            map.as_mut().map(|t| t.reborrow()),
        );
    }
    out.push('\n');
}

/// [`render_stmt`]'s own per-`kind` dispatch, split out so the nested-
/// checkpoint check above runs exactly once per statement regardless of
/// which arm below ends up recursing back into `render_stmt` for a child.
fn render_stmt_kind(
    out: &mut String,
    stmt: &TsStmt,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    match &stmt.kind {
        TsStmtKind::Verbatim { text, .. } => {
            out.push_str(text);
        }
        TsStmtKind::Decl(decl) => render_decl(out, decl, depth, map),
        TsStmtKind::Const { name, ty, init } => {
            out.push_str(&indent(depth));
            out.push_str("const ");
            render_binding_name(out, name);
            if let Some(ty) = ty {
                out.push_str(": ");
                render_type(out, ty);
            }
            out.push_str(" = ");
            render_stmt_level_expr(out, init, depth, &mut map);
            out.push_str(";\n");
        }
        TsStmtKind::Let {
            name,
            ty,
            init,
            hoisted,
        } => {
            out.push_str(&indent(depth));
            out.push_str(if *hoisted { "var " } else { "let " });
            render_binding_name(out, name);
            if let Some(ty) = ty {
                out.push_str(": ");
                render_type(out, ty);
            }
            if let Some(init) = init {
                out.push_str(" = ");
                render_stmt_level_expr(out, init, depth, &mut map);
            }
            out.push_str(";\n");
        }
        TsStmtKind::ExprStmt(expr) => {
            out.push_str(&indent(depth));
            render_expr(out, expr, depth, &mut map);
            out.push_str(";\n");
        }
        TsStmtKind::Return(expr) => {
            out.push_str(&indent(depth));
            out.push_str("return");
            if let Some(expr) = expr {
                out.push(' ');
                // #1321: `return { ... }` needs the same depth-aware
                // multiline-object handling `Const`/`Let`/`Assign` already
                // get — `workers.rs`'s own `emit_worker_compose` returns
                // its whole compose surface this way (one shorthand async
                // `Method` entry per wrapper, one per line), a shape
                // `render_expr`'s plain recursion can't render correctly
                // (see `TsExpr::Object`'s own doc).
                render_stmt_level_expr(out, expr, depth, &mut map);
            }
            out.push_str(";\n");
        }
        TsStmtKind::Throw(expr) => {
            out.push_str(&indent(depth));
            out.push_str("throw ");
            render_stmt_level_expr(out, expr, depth, &mut map);
            out.push_str(";\n");
        }
        TsStmtKind::If {
            cond,
            then_branch,
            else_branch,
            same_line_else,
        } => {
            out.push_str(&indent(depth));
            render_if_tail(
                out,
                cond,
                then_branch,
                else_branch,
                *same_line_else,
                depth,
                map,
            );
        }
        TsStmtKind::ForOf {
            binding,
            iter,
            body,
            is_await,
        } => {
            out.push_str(&indent(depth));
            out.push_str(if *is_await {
                "for await (const "
            } else {
                "for (const "
            });
            out.push_str(binding);
            out.push_str(" of ");
            render_expr(out, iter, depth, &mut map);
            out.push(')');
            render_branch(out, body, depth, map.as_mut().map(|t| t.reborrow()));
        }
        TsStmtKind::For {
            name,
            init,
            test,
            body,
        } => {
            out.push_str(&indent(depth));
            out.push_str("for (let ");
            out.push_str(name);
            out.push_str(" = ");
            render_expr(out, init, depth, &mut map);
            out.push_str("; ");
            render_expr(out, test, depth, &mut map);
            out.push_str("; ");
            out.push_str(name);
            out.push_str("++)");
            render_branch(out, body, depth, map.as_mut().map(|t| t.reborrow()));
        }
        TsStmtKind::While { cond, body } => render_while(out, cond, body, depth, map),
        TsStmtKind::Labelled { label, body } => render_labelled(out, label, body, depth, map),
        TsStmtKind::LetList(decls) => render_let_list(out, decls, depth, &mut map),
        TsStmtKind::TryCatch {
            try_block,
            catch_param,
            catch_block,
            finally_block,
        } => render_try_catch(
            out,
            depth,
            try_block,
            catch_param.as_deref(),
            catch_block.as_deref(),
            finally_block.as_deref(),
            map,
        ),
        TsStmtKind::Block(stmts) => {
            out.push_str(&indent(depth));
            out.push_str("{\n");
            for s in stmts {
                render_stmt(out, s, depth + 1, map.as_mut().map(|t| t.reborrow()));
            }
            out.push_str(&indent(depth));
            out.push_str("}\n");
        }
        TsStmtKind::Continue(label) => {
            out.push_str(&indent(depth));
            render_jump(out, "continue", label.as_deref());
        }
        TsStmtKind::Break(label) => {
            out.push_str(&indent(depth));
            render_jump(out, "break", label.as_deref());
        }
        TsStmtKind::Assign { target, value } => {
            out.push_str(&indent(depth));
            render_expr(out, target, depth, &mut map);
            out.push_str(" = ");
            render_stmt_level_expr(out, value, depth, &mut map);
            out.push_str(";\n");
        }
        TsStmtKind::Comment(text) => {
            for line in text.split('\n') {
                out.push_str(&indent(depth));
                out.push_str("// ");
                out.push_str(line);
                out.push('\n');
            }
        }
        TsStmtKind::DocComment(text) => render_doc_comment(out, text, depth),
        TsStmtKind::Blank => out.push('\n'),
        TsStmtKind::Switch {
            discriminant,
            cases,
        } => render_switch(out, discriminant, cases, depth, map),
        TsStmtKind::InlineBlock(stmts) => {
            out.push_str(&indent(depth));
            render_inline_block(out, stmts);
        }
        TsStmtKind::Increment(expr) => {
            out.push_str(&indent(depth));
            render_expr(out, expr, depth, &mut map);
            out.push_str("++;\n");
        }
        // Same rendering as `Verbatim` above — the text is printed exactly
        // as given, no `indent(depth)` prefix, no added punctuation (see
        // `TsStmtKind::Raw`'s own doc for why this is a distinct kind, not
        // a reuse of `Verbatim`).
        TsStmtKind::Raw(text, _) => out.push_str(text),
    }
}

/// `switch (<discriminant>) { <cases> }`, [`TsStmtKind::Switch`]'s renderer.
fn render_switch(
    out: &mut String,
    discriminant: &TsExpr,
    cases: &[TsSwitchCase],
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    out.push_str(&indent(depth));
    out.push_str("switch (");
    render_expr(out, discriminant, depth, &mut map);
    out.push_str(") {\n");
    for case in cases {
        debug_assert!(
            case.labels.is_empty() || !case.default_braced,
            "TsSwitchCase.default_braced only means something on the default (no labels) case — review of #1402's own nit"
        );
        // Review of #1446: the mirror invariant for `case_braced`
        // (Arc E slice 6, #1445) — it only means something on a
        // non-`default` (labelled) case; every real label-less call
        // site sets it `true` with no effect (`default_braced` owns
        // that arm's bracing instead), which would silently swallow a
        // future caller's `false` there.
        debug_assert!(
            !case.labels.is_empty() || case.case_braced,
            "TsSwitchCase.case_braced only means something on a non-default (labelled) case"
        );
        // 1A.13 (#3714): a case's span is recorded at the start of its
        // first label line, before its indentation.
        if let Some(span) = case.span {
            note_span_at(out.len(), span, map.as_mut());
        }
        if case.labels.is_empty() {
            out.push_str(&indent(depth + 1));
            if case.default_braced {
                out.push_str("default: {\n");
                for s in &case.body {
                    render_stmt(out, s, depth + 2, map.as_mut().map(|t| t.reborrow()));
                }
                out.push_str(&indent(depth + 1));
                out.push_str("}\n");
            } else {
                out.push_str("default:\n");
                for s in &case.body {
                    render_stmt(out, s, depth + 2, map.as_mut().map(|t| t.reborrow()));
                }
            }
            continue;
        }
        // Shared-body labels stack, one `case x:` line each; only the
        // last carries the opening brace.
        let last = case.labels.len() - 1;
        for (i, label) in case.labels.iter().enumerate() {
            out.push_str(&indent(depth + 1));
            out.push_str("case ");
            render_expr(out, label, depth, &mut map);
            if i < last {
                out.push_str(":\n");
            }
        }
        // Arc E slice 6 (#1445): `case_braced` is the non-`default`
        // mirror of `default_braced` — `emit_sum_codec`'s own
        // payload-free variant case (`case "Pending":\n  return {
        // kind: "Pending" };`) is real, unbraced content sitting
        // right beside a braced payload-carrying sibling in the same
        // switch (`212_json_codec`'s own `Status` fixture).
        if case.case_braced {
            out.push_str(": {\n");
            for s in &case.body {
                render_stmt(out, s, depth + 2, map.as_mut().map(|t| t.reborrow()));
            }
            out.push_str(&indent(depth + 1));
            out.push_str("}\n");
        } else {
            out.push_str(":\n");
            for s in &case.body {
                render_stmt(out, s, depth + 2, map.as_mut().map(|t| t.reborrow()));
            }
        }
    }
    out.push_str(&indent(depth));
    out.push_str("}\n");
}

/// `continue;` / `break;`, or `continue <label>;` / `break <label>;`, ending the line.
pub(super) fn render_jump(out: &mut String, keyword: &str, label: Option<&str>) {
    out.push_str(keyword);
    if let Some(label) = label {
        out.push(' ');
        out.push_str(label);
    }
    out.push_str(";\n");
}

/// `while (<cond>) <body>`, [`TsStmtKind::While`]'s renderer: the body is braced or inline
/// as for `for...of`.
fn render_while(
    out: &mut String,
    cond: &TsExpr,
    body: &TsStmt,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    out.push_str(&indent(depth));
    out.push_str("while (");
    render_expr(out, cond, depth, &mut map);
    out.push(')');
    render_branch(out, body, depth, map);
}

/// `<label>: <body>`, [`TsStmtKind::Labelled`]'s renderer: a block or loop body continues the
/// label's line (`label: {`, `label: while (…) {`), anything else follows it inline. A loop's own
/// body keeps the source map, so its statements record their checkpoints as they would unlabelled.
fn render_labelled(
    out: &mut String,
    label: &str,
    body: &TsStmt,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    out.push_str(&indent(depth));
    out.push_str(label);
    out.push(':');
    if let TsStmtKind::While { cond, body: inner } = &body.kind {
        out.push(' ');
        note_span(out.len(), body, map.as_mut());
        out.push_str("while (");
        render_expr(out, cond, depth, &mut map);
        out.push(')');
        render_branch(out, inner, depth, map);
    } else {
        render_branch(out, body, depth, map);
    }
}

/// `let a, b: T, c!: T, d = e;`, [`TsStmtKind::LetList`]'s renderer.
fn render_let_list(
    out: &mut String,
    decls: &[TsDeclarator],
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    out.push_str(&indent(depth));
    out.push_str("let ");
    for (i, d) in decls.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&d.name);
        debug_assert!(
            !d.definite || (d.ty.is_some() && d.init.is_none()),
            "TsDeclarator::definite needs a type and no initialiser (`{}!` is otherwise a syntax error)",
            d.name
        );
        if d.definite {
            out.push('!');
        }
        if let Some(ty) = &d.ty {
            out.push_str(": ");
            render_type(out, ty);
        }
        if let Some(init) = &d.init {
            out.push_str(" = ");
            render_expr(out, init, depth, m);
        }
    }
    out.push_str(";\n");
}

/// An `if` from its keyword on, the caller having written the indentation (or, for an
/// `else if`, the `else `): the condition, the branches and any `else`.
fn render_if_tail(
    out: &mut String,
    cond: &TsExpr,
    then_branch: &TsStmt,
    else_branch: &Option<Box<TsStmt>>,
    same_line_else: bool,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    out.push_str("if (");
    render_expr(out, cond, depth, &mut map);
    out.push(')');
    // `} else {` on one line — #1325's own real gap, all three of
    // `emit_test_main`'s own real `if`/`else` sites (one `Block`,
    // one `InlineBlock`). Only reachable when `then_branch` is
    // itself braced (`Block` or `InlineBlock` — every real
    // `same_line_else` site is one or the other); a brace-free
    // `then_branch` has no closing `}` for `else` to sit against,
    // so this falls back to the ordinary fresh-line rendering
    // rather than producing `<inline-stmt> else {`, which nothing
    // real needs. Review of #1326, finding 2: the two-variant
    // check needs no `Option<()>`/re-match/`unreachable!()` — a
    // plain `matches!` guard plus one `if let ... else` dispatch
    // says the same thing with no wildcard arm.
    let then_is_braced = matches!(
        &then_branch.kind,
        TsStmtKind::Block(_) | TsStmtKind::InlineBlock(_)
    );
    if same_line_else
        && then_is_braced
        && let Some(else_branch) = else_branch
    {
        // `render_inline_block`'s own `{ stmts }` shape, minus its
        // own trailing `\n` and leading space (added here instead,
        // matching `render_branch`'s own InlineBlock arm) so
        // `else` continues on the same physical line;
        // `render_block_body`'s own `{ stmts }` (no trailing
        // newline, the exact shape `TryCatch`'s own `} catch (e) {`
        // continuation already reuses) for the `Block` case,
        // guaranteed by `then_is_braced` above.
        if let TsStmtKind::InlineBlock(stmts) = &then_branch.kind {
            out.push_str(" { ");
            render_compact_stmts(out, stmts, depth);
            out.push_str(" }");
        } else {
            render_block_body(out, then_branch, depth, map.as_mut().map(|t| t.reborrow()));
        }
        out.push_str(" else");
        render_else_branch(out, else_branch, depth, map.as_mut().map(|t| t.reborrow()));
    } else {
        render_branch(out, then_branch, depth, map.as_mut().map(|t| t.reborrow()));
        if let Some(else_branch) = else_branch {
            out.push_str(&indent(depth));
            out.push_str("else");
            render_else_branch(out, else_branch, depth, map.as_mut().map(|t| t.reborrow()));
        }
    }
}

/// An `else` branch. An `else if` continues the chain at the same `depth`, so its
/// blocks nest where the first `if`'s do (1A.38a); any other branch is rendered as
/// [`render_branch`] renders one.
fn render_else_branch(
    out: &mut String,
    branch: &TsStmt,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    if let TsStmtKind::If {
        cond,
        then_branch,
        else_branch,
        same_line_else,
    } = &branch.kind
    {
        out.push(' ');
        note_span(out.len(), branch, map.as_mut());
        render_if_tail(
            out,
            cond,
            then_branch,
            else_branch,
            *same_line_else,
            depth,
            map,
        );
    } else {
        render_branch(out, branch, depth, map);
    }
}

/// `if`/`for...of`'s own body: braces (and a nested block) when `branch` is
/// itself a `Block`, otherwise printed inline on the same line — matching
/// `events_fanout.rs`'s own `if (!Array.isArray(subs)) continue;` (no
/// braces) alongside its always-braced `for...of` bodies.
pub(super) fn render_branch(
    out: &mut String,
    branch: &TsStmt,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    match &branch.kind {
        TsStmtKind::Block(_) => {
            render_block_body(out, branch, depth, map);
            out.push('\n');
        }
        TsStmtKind::InlineBlock(stmts) => {
            if inline_block_fits_one_line(stmts) {
                out.push(' ');
                render_inline_block(out, stmts);
            } else {
                // 1A.21 (#3722): a statement that holds a multi-line expression (a typed
                // decoder's block-bodied arrow, in a failure message) cannot be written on one
                // line, so the branch takes the braced, indented form instead.
                render_block_body(
                    out,
                    &TsStmt::block(stmts.clone(), None),
                    depth,
                    map.as_mut().map(|t| t.reborrow()),
                );
                out.push('\n');
            }
        }
        TsStmtKind::Verbatim { .. }
        | TsStmtKind::Decl(_)
        | TsStmtKind::Const { .. }
        | TsStmtKind::Let { .. }
        | TsStmtKind::ExprStmt(_)
        | TsStmtKind::Return(_)
        | TsStmtKind::Throw(_)
        | TsStmtKind::If { .. }
        | TsStmtKind::ForOf { .. }
        | TsStmtKind::For { .. }
        | TsStmtKind::While { .. }
        | TsStmtKind::Labelled { .. }
        | TsStmtKind::LetList(_)
        | TsStmtKind::TryCatch { .. }
        | TsStmtKind::Continue(_)
        | TsStmtKind::Break(_)
        | TsStmtKind::Assign { .. }
        | TsStmtKind::Comment(_)
        | TsStmtKind::DocComment(_)
        | TsStmtKind::Blank
        | TsStmtKind::Switch { .. }
        | TsStmtKind::Increment(_)
        | TsStmtKind::Raw(..) => {
            out.push(' ');
            note_span(out.len(), branch, map.as_mut());
            render_inline_stmt(out, branch, depth);
        }
    }
}

/// `{ <stmts> }`, with the closing brace at `depth`'s own indent and no
/// trailing newline — the shared shape [`TsStmtKind::TryCatch`] needs to
/// keep writing on the same generated line (`} catch (e) {`) and
/// [`render_branch`]'s own `Block` case needs with a `\n` appended after.
/// `block` is expected to be a [`TsStmtKind::Block`]; anything else is
/// still rendered sensibly (as a single-statement body) rather than
/// panicking.
///
/// `map`: forwarded into `stmts` when `block` is a real `Block` (#1477) —
/// `TryCatch`'s own `try`/`catch` bodies, and (1A.15) every `if`/`for` branch
/// body, so a nested statement's own span is recorded wherever it sits.
pub(super) fn render_block_body(
    out: &mut String,
    block: &TsStmt,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    out.push_str(" {\n");
    if let TsStmtKind::Block(stmts) = &block.kind {
        for s in stmts {
            render_stmt(out, s, depth + 1, map.as_mut().map(|t| t.reborrow()));
        }
    } else if let TsStmtKind::InlineBlock(stmts) = &block.kind {
        // The braces sit on their own lines here (this function's own usual
        // shape, e.g. `TryCatch`'s `catch (e) { ... }`), but the body itself
        // is one compact line — `workers_entry.rs`'s own real queue-consumer
        // catch clause (`console.error(...); msg.retry();`).
        out.push_str(&indent(depth + 1));
        // `depth + 1` has no test: the debug assertion in `render_compact_stmts`
        // forbids a multi-line child here, so no printed output depends on it.
        render_compact_stmts(out, stmts, depth + 1);
        out.push('\n');
    } else {
        render_stmt(out, block, depth + 1, map);
    }
    out.push_str(&indent(depth));
    out.push('}');
}

fn render_binding_name(out: &mut String, name: &TsBindingName) {
    match name {
        TsBindingName::Ident(s) => out.push_str(s),
        TsBindingName::ObjectPattern(names) => {
            out.push_str("{ ");
            for (i, n) in names.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(n);
            }
            out.push_str(" }");
        }
        TsBindingName::ArrayPattern(names) => {
            out.push('[');
            out.push_str(&names.join(", "));
            out.push(']');
        }
    }
}

/// `{ <stmts> }` for a constructor/method body — same shape as
/// [`render_block_body`] but over a plain `&[TsStmt]` (a `TsClassCtor`/
/// `TsClassMethod`'s own `body` field, not a boxed `TsStmt`), and always
/// followed by the caller's own `\n` (constructor/method declarations, not
/// a same-line `try`/`catch` continuation).
///
/// `map: Some(_)` when the caller wants a `nested_map`-bearing statement
/// reachable from `stmts` merged into a module source map as it prints
/// (#1477) — see [`crate::program::TsStmt::nested_map`]'s own doc for the
/// problem this solves. Reborrowed once per child and forwarded to
/// [`render_stmt`], which does the actual check — this function itself
/// holds none of that logic.
pub(super) fn render_block_stmts(
    out: &mut String,
    stmts: &[TsStmt],
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    out.push_str(" {\n");
    for s in stmts {
        render_stmt(out, s, depth + 1, map.as_mut().map(|t| t.reborrow()));
    }
    out.push_str(&indent(depth));
    out.push('}');
}
