use super::compact::{flatten_stmts, inline_block_fits_one_line, render_compact_stmts};
use super::decl::render_params;
use super::multiline::{
    render_bare_generics, render_multiline_array, render_multiline_conditional,
};
use super::precedence::{
    new_callee_needs_parens, render_binary_operand, render_cast_operand, render_operand,
};
use super::stmt::{render_block_stmts, render_stmt};
use super::types::render_type;
use super::{MergeTarget, indent};
use crate::program::{
    TsArrowBody, TsBinaryOp, TsExpr, TsLit, TsObjectEntry, TsParam, TsStmt, TsType, TsUnaryOp,
};

/// One [`TsObjectEntry`] rendered inline (no leading indent, no trailing
/// comma/newline) — [`render_expr`]'s own `TsExpr::Object` arm's per-entry
/// renderer, shared so the single-line and multi-line forms agree on what
/// each entry kind looks like. A `Method` entry has no real single-line
/// call site today (`workers.rs`'s own method entries are always the
/// multi-line `return { ... }` shape), but is still rendered plausibly
/// (not panicking) rather than left unreachable, matching this crate's own
/// "no wildcard, but still sensible on an unanticipated shape" posture
/// elsewhere (e.g. `render_block_body`'s own non-`Block` fallback).
fn render_object_entry_inline(
    out: &mut String,
    entry: &TsObjectEntry,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    match entry {
        TsObjectEntry::Prop {
            key: k,
            value: v,
            doc,
        } => {
            // As for a `Method`: a JSDoc block has no single-line form.
            debug_assert!(
                doc.is_none(),
                "render_object_entry_inline: a Prop entry's own doc comment has no single-line form"
            );
            out.push_str(k);
            out.push_str(": ");
            render_expr(out, v, depth, m);
        }
        TsObjectEntry::Shorthand(name) => out.push_str(name),
        TsObjectEntry::Spread(e) => {
            out.push_str("...");
            render_expr(out, e, depth, m);
        }
        TsObjectEntry::Method {
            name,
            is_async,
            generics,
            params,
            return_type,
            doc,
            inline,
            body,
            ..
        } => {
            // Review of #1338, finding 2: a JSDoc block has no single-line
            // form — not reachable through this function today: every real
            // `Method` entry (`inline` or not) is placed via
            // `render_multiline_object_entry`, never here (this function
            // renders one ENTRY inline for when the whole ENCLOSING OBJECT
            // is single-line, a different concept from a method's own body
            // being compact). Silently dropping a JSDoc block would be a
            // strictly worse failure mode than a loud check — the same
            // "not reachable today, but worth a loud check" posture
            // `render_compact_stmts`'s own `debug_assert!` (above) already
            // established for an analogous inline-rendering hazard.
            debug_assert!(
                doc.is_none(),
                "render_object_entry_inline: a Method entry's own doc comment has no single-line form"
            );
            if *is_async {
                out.push_str("async ");
            }
            out.push_str(name);
            render_bare_generics(out, generics);
            out.push('(');
            render_params(out, params);
            out.push(')');
            if let Some(rt) = return_type {
                out.push_str(": ");
                render_type(out, rt);
            }
            if *inline {
                out.push_str(" { ");
                render_compact_stmts(out, body, depth);
                out.push_str(" }");
            } else {
                render_block_stmts(out, body, 0, None);
            }
        }
    }
}

/// A unary operator and its operand: `!x`, `typeof x`, `-x`, `void x`, `++x`.
fn render_unary(
    out: &mut String,
    op: TsUnaryOp,
    expr: &TsExpr,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    out.push_str(match op {
        TsUnaryOp::Not => "!",
        TsUnaryOp::Typeof => "typeof ",
        TsUnaryOp::Neg => "-",
        TsUnaryOp::Void => "void ",
        TsUnaryOp::PreIncrement => "++",
    });
    // `- -x` must not print as the decrement `--x`.
    let nested_neg = op == TsUnaryOp::Neg
        && matches!(
            *expr,
            TsExpr::Unary {
                op: TsUnaryOp::Neg,
                ..
            }
        );
    if nested_neg {
        out.push('(');
        render_expr(out, expr, depth, m);
        out.push(')');
    } else {
        render_operand(out, expr, depth, m);
    }
}

/// `test ? consequent : alternate`.
pub(super) fn render_conditional(
    out: &mut String,
    test: &TsExpr,
    consequent: &TsExpr,
    alternate: &TsExpr,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    // `test` prints like a `ShortCircuitExpression` (the real JS/TS
    // grammar position) — an `Arrow` or a nested `Conditional` there
    // needs parens (grammar), a `Binary`/`As`/anything else doesn't.
    // `consequent`/`alternate` are `AssignmentExpression` positions
    // and admit a nested `Arrow`/`Conditional` with no parens at all
    // (right-associative chaining, e.g. `a ? b : c ? d : e`, is
    // already the correct reading) — neither needs the operand
    // machinery the way `test` does.
    let test_needs_parens = matches!(
        test,
        TsExpr::Arrow { .. }
            | TsExpr::Conditional { .. }
            | TsExpr::MultilineConditional { .. }
            | TsExpr::Yield(_)
    );
    if test_needs_parens {
        out.push('(');
        render_expr(out, test, depth, m);
        out.push(')');
    } else {
        render_expr(out, test, depth, m);
    }
    out.push_str(" ? ");
    render_expr(out, consequent, depth, m);
    out.push_str(" : ");
    render_expr(out, alternate, depth, m);
}

pub(super) fn render_expr(
    out: &mut String,
    expr: &TsExpr,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    match expr {
        TsExpr::Ident(name) => {
            // 1A.40 Decision F, restored in 1B.5e: an `Ident` holds an identifier path and
            // nothing else. The text classes are gone and the identifier audit covers every
            // module, so expression text here is a defect in whatever built it — it would hide
            // the names it uses from `name_closure` and its structure from this printer.
            debug_assert!(
                crate::name_closure::is_identifier_path(name),
                "an `Ident` holds expression text: {name}"
            );
            out.push_str(name);
        }
        // #1539: printed exactly as `Ident` always was — raw text, no
        // escaping of its own. The origin exists for `verbatim_origins`/
        // `verbatim_sites` (xtask's gated probes) to line-scan, not for the
        // printer to branch on.
        TsExpr::VerbatimExpr(text, _origin) => out.push_str(text),
        TsExpr::RawUsing(text, _) => out.push_str(text),
        TsExpr::Member { object, property } => {
            render_operand(out, object, depth, m);
            out.push('.');
            out.push_str(property);
        }
        TsExpr::OptionalMember { object, property } => {
            render_operand(out, object, depth, m);
            out.push_str("?.");
            out.push_str(property);
        }
        TsExpr::Index { object, index } => {
            render_operand(out, object, depth, m);
            out.push('[');
            render_expr(out, index, depth, m);
            out.push(']');
        }
        TsExpr::OptionalIndex { object, index } => {
            render_operand(out, object, depth, m);
            out.push_str("?.[");
            render_expr(out, index, depth, m);
            out.push(']');
        }
        TsExpr::Arrow {
            params,
            is_async,
            generics,
            return_type,
            body,
        } => {
            if *is_async {
                out.push_str("async ");
            }
            render_bare_generics(out, generics);
            out.push('(');
            render_params(out, params);
            out.push(')');
            if let Some(rt) = return_type {
                out.push_str(": ");
                render_type(out, rt);
            }
            out.push_str(" => ");
            render_arrow_body(out, body, depth, m);
        }
        // 1A.21 (#3722): `((name = default): ret => { body })()`.
        TsExpr::DefaultIife {
            name,
            default,
            return_type,
            body,
        } => {
            out.push_str("((");
            out.push_str(name);
            out.push_str(" = ");
            render_expr(out, default, depth, m);
            out.push_str("): ");
            render_type(out, return_type);
            out.push_str(" => ");
            render_arrow_body(out, &TsArrowBody::Multiline(body.clone()), depth, m);
            out.push_str(")()");
        }
        TsExpr::Call { callee, args } => {
            render_operand(out, callee, depth, m);
            out.push('(');
            render_expr_list(out, args, depth, m);
            out.push(')');
        }
        // 1A.20 (#3721, Decision A): the instantiation expression `f<A, B>`.
        // 1A.35a part B: `callee?.(args)`.
        TsExpr::OptionalCall { callee, args } => {
            render_operand(out, callee, depth, m);
            out.push_str("?.(");
            render_expr_list(out, args, depth, m);
            out.push(')');
        }
        TsExpr::TypeApp { callee, type_args } => {
            debug_assert!(
                !type_args.is_empty(),
                "TsExpr::TypeApp needs at least one type argument; `f<>` is a syntax error"
            );
            render_operand(out, callee, depth, m);
            out.push('<');
            for (i, arg) in type_args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_type(out, arg);
            }
            out.push('>');
        }
        TsExpr::New { callee, args } => {
            out.push_str("new ");
            // `new (f().g)()` is not `new f().g()`: a callee that reaches a
            // call through its member chain keeps its brackets.
            if new_callee_needs_parens(callee) {
                out.push('(');
                render_expr(out, callee, depth, m);
                out.push(')');
            } else {
                render_operand(out, callee, depth, m);
            }
            out.push('(');
            render_expr_list(out, args, depth, m);
            out.push(')');
        }
        // `multiline` is ignored here deliberately: this recursion has no
        // `depth` to render a multi-line object correctly against (see
        // `TsExpr::Object`'s own doc). Every real `bynk-emit` call site
        // that needs `multiline: true` reaches it through
        // `render_stmt_level_expr` instead, which intercepts the shape
        // before it gets here.
        TsExpr::Object { entries, .. } => {
            if entries.is_empty() {
                out.push_str("{}");
            } else {
                out.push_str("{ ");
                for (i, entry) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    render_object_entry_inline(out, entry, depth, m);
                }
                out.push_str(" }");
            }
        }
        // `multiline` is ignored here deliberately, for the same reason
        // `TsExpr::Object`'s own arm just above ignores it: this recursion
        // has no `depth` to render a multi-line array correctly against.
        // Every real `bynk-emit` call site that needs `multiline: true`
        // reaches it through `render_stmt_level_expr` instead. A nested one
        // that must break across lines is a [`TsExpr::LinesArray`].
        TsExpr::Array { items, .. } => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_array_item(out, item, depth, m);
            }
            out.push(']');
        }
        TsExpr::LinesArray(items) => render_multiline_array(out, items, depth, m),
        TsExpr::TemplateLit { parts, exprs } => {
            out.push('`');
            for (i, part) in parts.iter().enumerate() {
                out.push_str(part);
                if let Some(e) = exprs.get(i) {
                    out.push_str("${");
                    render_expr(out, e, depth, m);
                    out.push('}');
                }
            }
            out.push('`');
        }
        TsExpr::Await(inner) => {
            out.push_str("await ");
            render_operand(out, inner, depth, m);
        }
        TsExpr::As { expr, ty } => {
            // `(a ?? b) as T` / `((x) => y) as T` / `((a ? b : c)) as T` — a
            // binary expression, an arrow, or a conditional all need parens
            // before `as` (`(x) => y as T` binds the cast to the arrow's own
            // *body*, not the whole arrow — review of #1322, finding 1;
            // #1323 adds `Conditional` proactively, the same class of gap).
            // `Await` is deliberately NOT in this list (corrected by #1323):
            // `as` binds looser than `await`, so `await x as T` already
            // parses as `(await x) as T` with no parens needed — P7.8's
            // original reasoning conflated "this file's real text has
            // parens" with "parens are grammatically required"; `workers_
            // entry.rs`'s own real `await request.json() as JsonValue` has
            // none. A caller whose own real content *does* want them (like
            // `events_fanout.rs`'s own historical text) wraps its own
            // `Await` in an explicit [`TsExpr::Paren`] instead of relying on
            // an implicit per-shape rule here.
            render_cast_operand(out, expr, depth, m);
            out.push_str(" as ");
            render_type(out, ty);
        }
        // `<expr> as const`, bracketed exactly as `As` brackets its operand.
        TsExpr::AsConst(expr) => {
            render_cast_operand(out, expr, depth, m);
            out.push_str(" as const");
        }
        TsExpr::Unary { op, expr } => render_unary(out, *op, expr, depth, m),
        TsExpr::Binary { op, left, right } => {
            render_binary_operand(out, *op, left, true, depth, m);
            out.push_str(match op {
                TsBinaryOp::NullishCoalescing => " ?? ",
                TsBinaryOp::Or => " || ",
                TsBinaryOp::And => " && ",
                TsBinaryOp::StrictEq => " === ",
                TsBinaryOp::StrictNotEq => " !== ",
                TsBinaryOp::GreaterThan => " > ",
                TsBinaryOp::LessThan => " < ",
                TsBinaryOp::InstanceOf => " instanceof ",
                TsBinaryOp::Add => " + ",
                TsBinaryOp::In => " in ",
                TsBinaryOp::GreaterThanEq => " >= ",
                TsBinaryOp::LessThanEq => " <= ",
                TsBinaryOp::Mul => " * ",
                TsBinaryOp::Div => " / ",
                TsBinaryOp::Sub => " - ",
                TsBinaryOp::Exp => " ** ",
            });
            render_binary_operand(out, *op, right, false, depth, m);
        }
        TsExpr::Conditional {
            test,
            consequent,
            alternate,
        } => render_conditional(out, test, consequent, alternate, depth, m),
        TsExpr::MultilineConditional {
            test,
            consequent,
            alternate,
        } => render_multiline_conditional(out, test, consequent, alternate, depth, m),
        TsExpr::Paren(inner) => {
            out.push('(');
            render_expr(out, inner, depth, m);
            out.push(')');
        }
        // A spread element is legal only as an array item or a call argument, and
        // `render_array_item` prints it there. Reaching it from any other position (an
        // operand) would print `...a + b`, which reads as a different tree, so
        // it is a construction bug: fail loudly in a debug build.
        TsExpr::Spread(inner) => {
            debug_assert!(
                false,
                "TsExpr::Spread is legal only as an array item or a call argument; it was rendered elsewhere"
            );
            render_spread(out, inner, depth, m);
        }
        TsExpr::NonNull(inner) => render_non_null(out, inner, depth, m),
        TsExpr::Assign { target, value } => {
            render_expr(out, target, depth, m);
            out.push_str(" = ");
            render_expr(out, value, depth, m);
        }
        TsExpr::AddAssign { target, value } => {
            render_expr(out, target, depth, m);
            out.push_str(" += ");
            render_expr(out, value, depth, m);
        }
        TsExpr::NullishAssign { target, value } => {
            render_expr(out, target, depth, m);
            out.push_str(" ??= ");
            render_expr(out, value, depth, m);
        }
        TsExpr::Lit(lit) => render_lit(out, lit),
        TsExpr::Sequence(items) => render_sequence(out, items, depth, m),
        TsExpr::Function {
            is_async,
            is_generator,
            params,
            return_type,
            body,
        } => render_function(
            out,
            (*is_async, *is_generator),
            params,
            return_type.as_ref(),
            body,
            depth,
            m,
        ),
        TsExpr::Yield(inner) => {
            out.push_str("yield ");
            render_expr(out, inner, depth, m);
        }
    }
}

/// `(a, b)`, [`TsExpr::Sequence`]'s renderer: always bracketed, so a comma never reads as an
/// argument or element separator.
fn render_sequence(
    out: &mut String,
    items: &[TsExpr],
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    debug_assert!(
        items.len() >= 2,
        "TsExpr::Sequence needs at least two items; one is just the expression"
    );
    out.push('(');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        render_expr(out, item, depth, m);
    }
    out.push(')');
}

/// `async function* (params): ret { body }`, [`TsExpr::Function`]'s renderer: the body is the
/// multi-line form an arrow's [`TsArrowBody::Multiline`] prints, one level deeper than the line
/// the function starts on.
fn render_function(
    out: &mut String,
    (is_async, is_generator): (bool, bool),
    params: &[TsParam],
    return_type: Option<&TsType>,
    body: &[TsStmt],
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    if is_async {
        out.push_str("async ");
    }
    out.push_str(if is_generator {
        "function* ("
    } else {
        "function ("
    });
    render_params(out, params);
    out.push(')');
    if let Some(rt) = return_type {
        out.push_str(": ");
        render_type(out, rt);
    }
    out.push_str(" {\n");
    for s in body {
        render_stmt(out, s, depth + 1, m.as_mut().map(|t| t.reborrow()));
    }
    out.push_str(&indent(depth));
    out.push('}');
}

/// A postfix `!` binds tighter than `await` and unary `!`/`-`/`typeof`, so those operands are
/// bracketed here: `(await x)!`, `(!x)!` (1A.23 review, #3779). The bracket set is local to the
/// postfix, so `render_operand`'s other callers print as before.
fn render_non_null(
    out: &mut String,
    inner: &TsExpr,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    if matches!(inner, TsExpr::Await(_) | TsExpr::Unary { .. }) {
        out.push('(');
        render_expr(out, inner, depth, m);
        out.push(')');
    } else {
        render_operand(out, inner, depth, m);
    }
    out.push('!');
}

/// An array item or call argument: a [`TsExpr::Spread`] prints `...` and its operand,
/// anything else is an ordinary expression. The two places a spread is legal.
pub(super) fn render_array_item(
    out: &mut String,
    item: &TsExpr,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    if let TsExpr::Spread(inner) = item {
        render_spread(out, inner, depth, m);
    } else {
        render_expr(out, item, depth, m);
    }
}

fn render_spread(out: &mut String, inner: &TsExpr, depth: usize, m: &mut Option<MergeTarget<'_>>) {
    out.push_str("...");
    render_expr(out, inner, depth, m);
}

/// [`TsExpr::Arrow`]'s own `body` renderer — see [`TsArrowBody`]'s own doc
/// for why there are exactly two shapes. `Expr` is the ordinary case, no
/// braces, the same plain `render_expr` recursion every arrow used before
/// #1435. `Block` reuses [`render_compact_stmts`] (the same shared core
/// `TsStmtKind::InlineBlock`'s own renderer already reduces to), not a
/// fresh copy of its "one physical line, semicolon-separated" logic — every
/// real `Block` site today (`serialisation.rs`'s `Float` guard) is exactly
/// this one-line-IIFE shape, so no trailing newline is added here (unlike
/// `render_inline_block`'s own statement-position form): this text is
/// embedded mid-expression, not standing alone as a statement.
/// Whether `expr` prints with an object literal as its first token.
fn starts_with_object(expr: &TsExpr) -> bool {
    match expr {
        TsExpr::Object { .. } => true,
        TsExpr::Member { object, .. }
        | TsExpr::OptionalMember { object, .. }
        | TsExpr::Index { object, .. }
        | TsExpr::OptionalIndex { object, .. } => starts_with_object(object),
        TsExpr::Call { callee, .. }
        | TsExpr::OptionalCall { callee, .. }
        | TsExpr::TypeApp { callee, .. } => starts_with_object(callee),
        TsExpr::As { expr, .. } => starts_with_object(expr),
        TsExpr::AsConst(expr) => starts_with_object(expr),
        TsExpr::DefaultIife { .. } => false,
        TsExpr::Binary { left, .. } => starts_with_object(left),
        TsExpr::Conditional { test, .. } | TsExpr::MultilineConditional { test, .. } => {
            starts_with_object(test)
        }
        TsExpr::Ident(_)
        | TsExpr::VerbatimExpr(..)
        | TsExpr::RawUsing(..)
        | TsExpr::Arrow { .. }
        | TsExpr::New { .. }
        | TsExpr::Array { .. }
        | TsExpr::LinesArray(_)
        | TsExpr::TemplateLit { .. }
        | TsExpr::Await(_)
        | TsExpr::Unary { .. }
        | TsExpr::Paren(_)
        | TsExpr::Spread(_)
        | TsExpr::NonNull(_)
        | TsExpr::Assign { .. }
        | TsExpr::AddAssign { .. }
        | TsExpr::NullishAssign { .. }
        | TsExpr::Sequence(_)
        | TsExpr::Function { .. }
        | TsExpr::Yield(_)
        | TsExpr::Lit(_) => false,
    }
}

fn render_arrow_body(
    out: &mut String,
    body: &TsArrowBody,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    match body {
        TsArrowBody::Expr(expr) => {
            // An expression body that starts with an object literal reads as a
            // block, not a value, so it is bracketed: `(x) => ({ a: x })`. That
            // is the leftmost operand being an `Object`, directly or under a
            // member access, call, `as` or operator: what the emitter's text
            // path bracketed by the printed text starting `{`.
            if starts_with_object(expr) {
                out.push('(');
                render_expr(out, expr, depth, m);
                out.push(')');
            } else {
                render_expr(out, expr, depth, m);
            }
        }
        // An empty body is `{}`, not `{  }` (1A.35a part B: the no-op `settle`).
        TsArrowBody::Block(stmts) if stmts.is_empty() => out.push_str("{}"),
        // A statement that prints over several lines cannot sit on the one line the compact form
        // promises, so the body falls back to the multi-line form (1A.39a, Decision E): the
        // printer decides, where the emitter used to print a statement to measure it.
        TsArrowBody::Block(stmts) if !inline_block_fits_one_line(stmts) => {
            render_arrow_body(out, &TsArrowBody::Multiline(stmts.clone()), depth, m);
        }
        TsArrowBody::Block(stmts) => {
            out.push_str("{ ");
            render_compact_stmts(out, stmts, depth);
            out.push_str(" }");
        }
        // Each statement printed on its own, its lines trimmed and joined by single spaces.
        TsArrowBody::Flat(stmts) if stmts.is_empty() => out.push_str("{}"),
        TsArrowBody::Flat(stmts) => {
            out.push_str("{ ");
            out.push_str(&flatten_stmts(stmts));
            out.push_str(" }");
        }
        // The multi-line form: `{`, each statement on its own line one level
        // deeper than the arrow's own line, `}` back at that line's indent.
        // Each statement records its own span, and merges its own `nested_map`,
        // into the target the enclosing statement is printed against (1A.15,
        // Decision D of #3716: IIFE and lambda bodies are mapped).
        TsArrowBody::Multiline(stmts) => {
            out.push_str("{\n");
            for s in stmts {
                render_stmt(out, s, depth + 1, m.as_mut().map(|t| t.reborrow()));
            }
            out.push_str(&indent(depth));
            out.push('}');
        }
    }
}

/// A call's arguments. A [`TsExpr::Spread`] is legal here as well as in an array
/// (1A.32b), and prints `...` then its operand.
fn render_expr_list(
    out: &mut String,
    exprs: &[TsExpr],
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    for (i, e) in exprs.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        render_array_item(out, e, depth, m);
    }
}

pub(super) fn render_lit(out: &mut String, lit: &TsLit) {
    match lit {
        TsLit::Str(s) => {
            out.push('"');
            // One shared escaper (`crate::predicate::escape_ts_literal`): this printer,
            // `bynk-emit` and `bynk-check`'s wire defaults must never disagree.
            out.push_str(&crate::predicate::escape_ts_literal(s));
            out.push('"');
        }
        TsLit::Num(n) => out.push_str(n),
        TsLit::BigInt(digits) => {
            out.push_str(digits);
            out.push('n');
        }
        TsLit::Null => out.push_str("null"),
        TsLit::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        // The whole literal, printed exactly as given — see `TsLit::Raw`'s
        // own doc for why: this is the one place in this printer that must
        // NOT apply `Str`'s own escaping.
        TsLit::Raw(text) => out.push_str(text),
    }
}
