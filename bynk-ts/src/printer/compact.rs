use super::expr::render_expr;
use super::stmt::{render_branch, render_jump, render_stmt};
#[cfg(doc)]
use crate::program::TsArrowBody;
use crate::program::{TsStmt, TsStmtKind};

/// Whether every statement of an [`TsStmtKind::InlineBlock`] renders on one line, so that the
/// block may be written `{ a; b; }`.
pub(super) fn inline_block_fits_one_line(stmts: &[TsStmt]) -> bool {
    stmts.iter().all(|s| {
        let mut piece = String::new();
        render_inline_stmt(&mut piece, s, 0);
        !piece.trim_end_matches('\n').contains('\n')
    })
}

/// `{ stmt; stmt; ... }` on one generated line — [`TsStmtKind::InlineBlock`]'s
/// own renderer, shared by [`render_branch`] (an `if`/`else` branch) and
/// [`render_stmt`]'s own top-level/nested-statement-list arm (not reachable
/// from any real content at that position today, but every `TsStmtKind`
/// variant still needs a real rendering, not a wildcard). Each statement
/// renders through [`render_inline_stmt`] (reusing its own exhaustive
/// per-kind handling) with its own trailing newline trimmed, then a single
/// space — matching the real, hand-written `{ a; b; c; }` shape exactly.
pub(super) fn render_inline_block(out: &mut String, stmts: &[TsStmt]) {
    out.push_str("{ ");
    render_compact_stmts(out, stmts, 0);
    out.push_str(" }\n");
}

/// [`TsArrowBody::Flat`]'s text: each statement printed at depth 0, split into lines, every line
/// trimmed, the empty ones dropped, and all of it joined with single spaces.
///
/// Trimming rewrites a line's leading whitespace, so a statement whose printed form has a template
/// literal spanning lines would be silently changed (the whitespace is part of the string). A
/// debug build asserts there is none: a single-line template literal is fine.
pub(super) fn flatten_stmts(stmts: &[TsStmt]) -> String {
    stmts
        .iter()
        .map(|st| {
            let printed = crate::print_stmt(st, 0);
            debug_assert!(
                !has_multiline_template(&printed),
                "a `Flat` arrow body holds a multi-line template literal; flattening would trim its text"
            );
            printed
                .split('\n')
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether printed code has a template literal (backtick to backtick) with a newline inside it.
/// Backticks inside a `"..."` or `'...'` string are text, not delimiters, and `\`-escapes are
/// skipped in both; inside a template literal quotes are text. Debug-assertion support only.
pub(super) fn has_multiline_template(printed: &str) -> bool {
    #[derive(PartialEq)]
    enum Mode {
        Code,
        Str(char),
        Template,
    }
    let mut mode = Mode::Code;
    let mut chars = printed.chars();
    while let Some(c) = chars.next() {
        match (&mode, c) {
            (_, '\\') => {
                chars.next();
            }
            (Mode::Code, '"' | '\'') => mode = Mode::Str(c),
            (Mode::Code, '`') => mode = Mode::Template,
            (Mode::Str(q), _) if c == *q => mode = Mode::Code,
            (Mode::Template, '`') => mode = Mode::Code,
            (Mode::Template, '\n') => return true,
            _ => {}
        }
    }
    false
}

/// `stmt1; stmt2; ...` — every statement in `stmts` rendered on the current
/// line, semicolon-and-space-separated, no leading/trailing space or
/// surrounding braces of its own. The shared core both real compact shapes
/// need: [`TsStmtKind::InlineBlock`]'s own single-line-*braced* form
/// (`render_inline_block`, braces on the same line as the content) and
/// [`render_block_body`](super::stmt::render_block_body)'s own special case for a `TryCatch` block whose
/// body is an `InlineBlock` (braces on their own lines, per that shape's
/// usual convention, but the body itself still one compact line) —
/// `workers_entry.rs`'s own two real shapes, distinguished by whether the
/// pre-conversion `writeln!` code put the opening brace on the same
/// physical line as the content or not.
pub(super) fn render_compact_stmts(out: &mut String, stmts: &[TsStmt], depth: usize) {
    for (i, s) in stmts.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut piece = String::new();
        render_inline_stmt(&mut piece, s, depth);
        // Review of #1324, finding 2: only `trim_end_matches('\n')`'s own
        // trailing newline is stripped — an *embedded* newline (a genuinely
        // multi-line statement, e.g. `Block`/`Switch`/`TryCatch`, rendered
        // via `render_inline_stmt`'s own `render_stmt(out, stmt, 0)`
        // fallback) would break the one-line shape this function promises.
        // Not reachable today — `workers_entry.rs`'s own three real
        // `InlineBlock` sites are `ExprStmt`/`Continue` only — but worth a
        // loud check rather than a silent multi-line break if a future
        // slice's `InlineBlock` ever holds one.
        debug_assert!(
            !piece.trim_end_matches('\n').contains('\n'),
            "render_compact_stmts: a genuinely multi-line statement can't render on one line"
        );
        out.push_str(piece.trim_end_matches('\n'));
    }
}

/// A statement rendered without its own leading indent, for the "no braces"
/// half of [`render_branch`] — only the shapes `events_fanout.rs` actually
/// needs inline (`continue`, `return`, a bare expression) get dedicated
/// handling; anything else falls back to a normal indented render (depth 0)
/// rather than panicking, so an unanticipated future shape still prints
/// something plausible instead of crashing.
pub(super) fn render_inline_stmt(out: &mut String, stmt: &TsStmt, depth: usize) {
    match &stmt.kind {
        TsStmtKind::Continue(label) => render_jump(out, "continue", label.as_deref()),
        TsStmtKind::Break(label) => render_jump(out, "break", label.as_deref()),
        // A loop or label heads its own line and nests at the real `depth`, which the
        // fallback below (rendered at depth 0) would not.
        TsStmtKind::While { cond, body } => {
            out.push_str("while (");
            render_expr(out, cond, depth, &mut None);
            out.push(')');
            render_branch(out, body, depth, None);
        }
        TsStmtKind::Labelled { label, body } => {
            out.push_str(label);
            out.push(':');
            render_branch(out, body, depth, None);
        }
        TsStmtKind::Return(expr) => {
            out.push_str("return");
            if let Some(expr) = expr {
                out.push(' ');
                render_expr(out, expr, depth, &mut None);
            }
            out.push_str(";\n");
        }
        TsStmtKind::ExprStmt(expr) => {
            render_expr(out, expr, depth, &mut None);
            out.push_str(";\n");
        }
        // Review of #1317/#1318, finding 2: every other variant in the
        // fallback group below is safe to render via `render_stmt`'s own
        // top-level `//`-line-comment form — `Comment` is not. A `//`
        // comment run through the generic fallback (`if (cond) // text`)
        // comments out the rest of the physical line, leaving the `if`
        // with no body at all — a TypeScript parse error, not merely an
        // unlikely shape. Not reachable today (`events_fanout.rs` never
        // puts a bare `Comment` in a brace-free `if`/`for...of` body), but
        // the fallback group exists precisely so a future slice trips over
        // a missing case at compile time, not at parse time in emitted
        // output — so `Comment` needs its own real inline shape now, a
        // block comment (`/* text */`), which cannot swallow anything
        // after it. Embedded newlines flatten to spaces, keeping this
        // strictly one generated line like every other inline shape here.
        // Review of #1324, finding 2: `Blank`'s own top-level rendering is a
        // bare `'\n'` (see `render_stmt`), which the generic fallback below
        // would inherit unchanged — reaching `render_branch`'s brace-free
        // arm (`if (cond) <inline>`), that prints `if (cond) \n`, and since
        // JS/TS statement grammar doesn't care about the newline, the very
        // next statement in the enclosing block silently becomes the `if`'s
        // own body. Exactly the same swallowing-bug class the #1317/#1318
        // review already fixed for `Comment` just above — an honest empty
        // statement (`;`) is what a "no code here" body actually means in an
        // inline TS position, and cannot swallow anything after it.
        TsStmtKind::Blank => out.push_str(";\n"),
        TsStmtKind::Comment(text) => {
            out.push_str("/* ");
            out.push_str(&text.replace('\n', " "));
            out.push_str(" */\n");
        }
        // No inline shape of its own — `events_fanout.rs` never puts one of
        // these in a brace-free `if`/`for...of` body — but listed by name,
        // not a wildcard (review of #1314, finding 3): a future Arc C
        // slice's new `TsStmtKind` variant must fail to compile here rather
        // than silently inherit this fallback, the same exhaustiveness
        // discipline `VerbatimOrigin`'s own doc states for `bynk-emit`.
        TsStmtKind::Verbatim { .. }
        | TsStmtKind::Decl(_)
        | TsStmtKind::Const { .. }
        | TsStmtKind::Let { .. }
        // 1A.35a part B: one line, like `Let`.
        | TsStmtKind::LetList(_)
        | TsStmtKind::If { .. }
        | TsStmtKind::ForOf { .. }
        // Arc E slice 7 (#1447): not reachable today — `ListInst`/
        // `MapInst`'s two real `For` sites both use a braced `Block` body,
        // which `render_branch` dispatches to its own multi-line arm before
        // this fallback group is ever consulted — but safe for the same
        // reason `ForOf` already is: a brace-free `For` body renders through
        // `render_stmt`'s own indented, newline-terminated form, which is
        // exactly what this fallback provides.
        | TsStmtKind::For { .. }
        | TsStmtKind::TryCatch { .. }
        | TsStmtKind::Block(_)
        | TsStmtKind::Assign { .. }
        | TsStmtKind::Switch { .. }
        | TsStmtKind::InlineBlock(_)
        // #1325: `passed++;`/`failed++;` inside `emit_test_main`'s own
        // compact `{ passed++; console.log(...); }` shape — always complete,
        // single-line, semicolon-terminated content (never a bare newline
        // the way `Blank`'s own top-level form is), so the same fallback
        // that's safe for `Const`/`If`/etc. above is safe here too.
        | TsStmtKind::Increment(_)
        // #1353: `emit_contract_guarded_body`'s own precondition/
        // postcondition guards are exactly this fallback's own target case —
        // a real, reachable `InlineBlock` site (`if (!(pred)) { const __e =
        // ...; __e.name = ...; throw __e; }`), not a defensive placeholder.
        // Safe for the same reason as `Const`/`Assign` above, with the same
        // caveat those two already carry: `render_stmt_level_expr` renders a
        // `multiline: true` operand with embedded newlines, which would
        // break this fallback's one-line contract — untested today only
        // because the one real call site (`throw __e;`, a bare `Ident`)
        // never reaches that case, not because `Throw` structurally cannot.
        | TsStmtKind::Throw(_)
        // #1333: not reachable today — every real `DocComment` reaches the
        // printer only via `print_stmt`, never through a `TsProgram`'s own
        // tree — which is what actually makes this fallback safe: unlike
        // `Comment`'s own `//` form, a `/** ... */` block always closes
        // itself before any subsequent text, so it cannot swallow the next
        // statement the way an unterminated `//` line comment could — but
        // the fallback's own `render_stmt(out, stmt, 0)` still emits the
        // block's embedded newlines, which would break an `InlineBlock`'s
        // single-line contract if this arm ever became reachable through
        // one. Listed by name per this group's own exhaustiveness
        // discipline, not folded into a wildcard.
        | TsStmtKind::DocComment(_)
        // #1337: not reachable today — every real `Raw`-bearing body
        // (`TsObjectEntry::Method`, and since #1352/#1369 also
        // `TsDecl::Function`) renders through `render_block_stmts`, never
        // through `render_inline_stmt`'s own call path (an `if`/`for...of`
        // branch or an `InlineBlock`). Unsafe if it ever became reachable
        // there for the same reason `DocComment` is: `Raw`'s own text is
        // typically a whole multi-statement function body, never
        // single-line, so the fallback's embedded newlines would break an
        // `InlineBlock`'s single-line contract. Since #1369 added
        // `TsDecl::Function.inline`, this is now one field flip away from
        // live rather than purely hypothetical — `render_compact_stmts`'s
        // own `debug_assert!` is the actual backstop if a future call site
        // ever combines `inline: true` with a `Raw` body statement;
        // `emit_agent`'s own rehydrate function (#1369) deliberately keeps
        // `inline: false` specifically to avoid exercising this arm.
        // Listed by name, not folded into a wildcard, for the same reason
        // as every other arm in this group.
        | TsStmtKind::Raw(..) => render_stmt(out, stmt, 0, None),
    }
}
