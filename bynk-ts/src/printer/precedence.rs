use super::MergeTarget;
use super::expr::render_expr;
use crate::program::{TsBinaryOp, TsExpr};

/// Whether `expr`, printed in a position member access, indexing, a call
/// callee, `await`, or unary `!`/`typeof` binds tighter than, needs its own
/// parens to preserve the built tree's meaning — `Binary`/`As`/`Arrow`/
/// `Conditional` all bind looser than any of those (review of #1314, finding
/// 2: `!a ?? b`/`a ?? b.c`/`await a ?? b` all silently changed meaning
/// without this; review of #1322, finding 1: `Arrow` was missing from this
/// same rule — `(x) => x(1)` is a call whose *body* is `x(1)`, not the IIFE
/// `((x) => x)(1)` a `Call { callee: Arrow, .. }` node means; #1323 adds
/// `Conditional` proactively, the same class of gap, before any real content
/// or review round hits it). Deliberately conservative for *these* contexts
/// specifically: a nested `Binary`/`Arrow`/`Conditional` is always
/// parenthesized here regardless of which operator/shape it is — correct in
/// every case that class of bug can hit (none of these
/// six positions is ever real, grounded content in this file today), at
/// the cost of an occasional pair a real precedence table could omit.
///
/// **Not** used for `Binary`'s own left/right operands — see
/// [`render_binary_operand`]/[`binary_precedence`] for why that context
/// needed to become precedence-aware once #1321 added more than one binary
/// operator (this function's own always-parenthesize rule would print
/// `(__authz === null) || …` for `workers.rs`'s own real
/// `__authz === null || !__authz.startsWith(...)`, which the byte-golden
/// fixtures don't have parens around).
fn needs_parens_as_operand(expr: &TsExpr) -> bool {
    matches!(
        expr,
        TsExpr::Binary { .. }
            | TsExpr::As { .. }
            | TsExpr::AsConst(_)
            | TsExpr::Arrow { .. }
            | TsExpr::Conditional { .. }
            | TsExpr::MultilineConditional { .. }
            | TsExpr::Assign { .. }
            | TsExpr::AddAssign { .. }
            | TsExpr::NullishAssign { .. }
            | TsExpr::Function { .. }
            | TsExpr::Yield(_)
    )
}

/// Whether a `new` callee must be bracketed: a member chain that passes
/// through a call or a `new` (`new (f().g)()`), which `new f().g()` would
/// re-read as `(new f()).g()`.
pub(super) fn new_callee_needs_parens(callee: &TsExpr) -> bool {
    match callee {
        TsExpr::Call { .. }
        | TsExpr::OptionalCall { .. }
        | TsExpr::DefaultIife { .. }
        | TsExpr::New { .. } => true,
        TsExpr::Member { object, .. }
        | TsExpr::OptionalMember { object, .. }
        | TsExpr::Index { object, .. }
        | TsExpr::OptionalIndex { object, .. } => new_callee_needs_parens(object),
        TsExpr::TypeApp { callee, .. } => new_callee_needs_parens(callee),
        TsExpr::Ident(_)
        | TsExpr::VerbatimExpr(..)
        | TsExpr::RawUsing(..)
        | TsExpr::LinesArray(_)
        | TsExpr::Arrow { .. }
        | TsExpr::Object { .. }
        | TsExpr::Array { .. }
        | TsExpr::TemplateLit { .. }
        | TsExpr::Await(_)
        | TsExpr::As { .. }
        | TsExpr::AsConst(_)
        | TsExpr::Unary { .. }
        | TsExpr::Binary { .. }
        | TsExpr::Conditional { .. }
        | TsExpr::MultilineConditional { .. }
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

pub(super) fn render_operand(
    out: &mut String,
    expr: &TsExpr,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    if needs_parens_as_operand(expr) {
        out.push('(');
        render_expr(out, expr, depth, m);
        out.push(')');
    } else {
        render_expr(out, expr, depth, m);
    }
}

/// Standard JS/TS relative precedence for the operators [`TsBinaryOp`]
/// currently has — only used to decide a nested `Binary` operand's own
/// parenthesisation (see [`render_binary_operand`]); higher binds tighter.
fn binary_precedence(op: TsBinaryOp) -> u8 {
    match op {
        TsBinaryOp::NullishCoalescing => 1,
        TsBinaryOp::Or => 2,
        TsBinaryOp::And => 3,
        TsBinaryOp::StrictEq | TsBinaryOp::StrictNotEq => 4,
        TsBinaryOp::GreaterThan
        | TsBinaryOp::LessThan
        | TsBinaryOp::InstanceOf
        | TsBinaryOp::In
        | TsBinaryOp::GreaterThanEq
        | TsBinaryOp::LessThanEq => 5,
        TsBinaryOp::Add | TsBinaryOp::Sub => 6,
        TsBinaryOp::Mul | TsBinaryOp::Div => 7,
        TsBinaryOp::Exp => 8,
    }
}

/// The left-associative arithmetic operators (`+ - * /`): the ones whose
/// same-tier left nesting prints flat. Everything else keeps the
/// conservative rule in [`render_binary_operand`].
fn is_left_assoc_arith(op: TsBinaryOp) -> bool {
    matches!(
        op,
        TsBinaryOp::Add | TsBinaryOp::Sub | TsBinaryOp::Mul | TsBinaryOp::Div
    )
}

/// Render `expr` as one side of a `Binary { op: outer_op, .. }` — #1321's
/// own real gap (`workers.rs`'s own `__authz === null ||
/// !__authz.startsWith(...)` / `__authz !== null &&
/// __authz.startsWith(...)`): with only `??` in the algebra,
/// [`render_operand`]'s "always parenthesize a nested `Binary`" rule never
/// mismatched real content; a comparison (`===`/`!==`) nested inside `||`/
/// `&&` needs *no* parens to preserve meaning (`===` binds tighter), and
/// the byte-golden fixtures have none — so this context needed to become
/// precedence-aware rather than reusing [`render_operand`] unchanged.
/// `??` mixed with `||`/`&&` is a real TS syntax error without explicit
/// parens (not just a precedence question), so that combination always
/// parenthesizes regardless of the numeric table above; nothing in
/// `bynk-emit` builds that combination today, but a future caller that did
/// still gets correct, parseable output rather than a silent `tsc` error.
///
/// Equal precedence still parenthesizes for two *different* operators
/// (`<=`, not `<`) — deliberately preserving [`render_operand`]'s own
/// pre-#1321 "always parenthesize a same/lower-precedence nested chain,
/// even where associativity would read the same without it" conservatism
/// for that case; only a *strictly higher*-precedence nested operator — the
/// one real, grounded case #1321 needs — omits parens there. #1323 adds one
/// more real case: the exact *same* operator nested in itself omits parens
/// too, for `||`/`&&` specifically (both associative, and real content —
/// `emit_call_handler_dispatch`'s 3-term `||` chain — needs the flat,
/// unparenthesized reading) — `??` keeps its brackets when it is right-nested
/// (pinned by `parenthesises_a_nested_binary_operand_of_another_binary`'s own
/// test, still `a ?? (b ?? c)`), but, being left-associative, prints a
/// left-nested chain flat (`a ?? b ?? c`, 1B.5d), as `+` does: a driver's
/// secret read is such a chain.
pub(super) fn render_binary_operand(
    out: &mut String,
    outer_op: TsBinaryOp,
    expr: &TsExpr,
    is_left: bool,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    let needs_parens = match expr {
        // `Arrow`/`Conditional` are the two lowest-precedence expression
        // forms in JS/TS — `(x) => y || z` as a binary operand must print
        // `(x) => (y || z)`, never the bare `(x) => y || z` (which reparses
        // as the arrow's own *body* being `y || z`, not the arrow itself
        // being one operand of `||`); a `Conditional` operand needs the
        // identical protection for the identical reason (review of #1322,
        // finding 1, for `Arrow`; #1323 adds `Conditional` proactively, the
        // same class of gap).
        TsExpr::As { .. }
        | TsExpr::AsConst(_)
        | TsExpr::Arrow { .. }
        | TsExpr::Conditional { .. }
        | TsExpr::MultilineConditional { .. }
        | TsExpr::Assign { .. }
        | TsExpr::AddAssign { .. }
        | TsExpr::NullishAssign { .. }
        | TsExpr::Yield(_) => true,
        // #2955 review: a unary or `await` operand directly left of `**` is a
        // JS syntax error (`!a ** b`, `typeof a ** b`, `await a ** b`), not a
        // precedence question, so it always parenthesizes there.
        TsExpr::Unary { .. } | TsExpr::Await(_) if outer_op == TsBinaryOp::Exp && is_left => true,
        TsExpr::Binary { op: inner_op, .. } => {
            let mixes_nullish = (outer_op == TsBinaryOp::NullishCoalescing)
                != (*inner_op == TsBinaryOp::NullishCoalescing);
            if mixes_nullish {
                true
            } else if is_left_assoc_arith(outer_op)
                && is_left_assoc_arith(*inner_op)
                && binary_precedence(*inner_op) == binary_precedence(outer_op)
            {
                // Slice 1A.5 (#3703): `+`/`-` and `*`/`/` are left-associative
                // within their tier, so a left-nested operand prints flat
                // (`a - b - c`, `a * b / c`) and a right-nested one keeps its
                // brackets (`a - (b - c)`, `a / (b * c)`) — `+` right-nested in
                // `+` included (see the same-operator note below).
                !is_left
            } else if *inner_op == outer_op {
                // Same operator chained with itself: only `||`/`&&` are
                // associative, so only they may print flat with no parens —
                // #1323's own `typeof args !== "object" || args === null ||
                // Array.isArray(args)` (`emit_call_handler_dispatch`), a
                // genuine 3-term chain the pre-#1323 "always parenthesize
                // equal precedence" rule would have wrongly printed as
                // `(typeof args !== "object" || args === null) ||
                // Array.isArray(args)`. Every other operator — including
                // `??` (kept at its pre-#1321 conservative choice; nothing in
                // real content needs a flattened `??` chain, and
                // `parenthesises_a_nested_binary_operand_of_another_binary`
                // pins that `a ?? (b ?? c)` still gets its parens) and, as of
                // this fix, the equality/relational operators (`===`/`!==`/
                // `>`) — is NOT associative: `a === b === c` parses as
                // `(a === b) === c`, not the tree's real `a === (b === c)`,
                // so a same-operator nesting of any of those still needs its
                // parens regardless of which side it's nested on (review of
                // #1324, finding 1 — the original fix wrongly generalized
                // the `||`/`&&` exemption to every operator). `+` (Arc C,
                // step (11), #1388) does NOT join that exemption on both
                // sides — review of #1389, finding 1: `||`/`&&` are
                // SEMANTICALLY associative (`(a || b) || c` and
                // `a || (b || c)` always agree), but `+` is only
                // GRAMMATICALLY left-associative — with a mixed number/string
                // chain, `1 + (2 + "3")` (`"123"`) and `(1 + 2) + "3"`
                // (`"33"`) disagree, so flattening a RIGHT-nested `Add` would
                // silently change the value. `join_plus`
                // (`bynk-emit/src/emitter/emit.rs`) only ever left-folds, so
                // this asymmetry never showed up in that caller's own
                // zero-diff fixture run — but `bynk-ts` is a shared
                // primitive, not scoped to that one caller's own usage.
                match outer_op {
                    TsBinaryOp::Or | TsBinaryOp::And => false,
                    // `??` is left-associative, so a left-nested chain prints flat (`a ?? b ?? c`),
                    // as `+` does, and a right-nested one keeps its brackets (1B.5d).
                    TsBinaryOp::Add | TsBinaryOp::Sub | TsBinaryOp::NullishCoalescing => !is_left,
                    TsBinaryOp::StrictEq
                    | TsBinaryOp::StrictNotEq
                    | TsBinaryOp::GreaterThan
                    | TsBinaryOp::InstanceOf
                    | TsBinaryOp::LessThan
                    | TsBinaryOp::In
                    | TsBinaryOp::GreaterThanEq
                    | TsBinaryOp::LessThanEq
                    | TsBinaryOp::Mul
                    | TsBinaryOp::Div
                    | TsBinaryOp::Exp => true,
                }
            } else {
                binary_precedence(*inner_op) <= binary_precedence(outer_op)
            }
        }
        TsExpr::Ident(_)
        | TsExpr::VerbatimExpr(..)
        | TsExpr::RawUsing(..)
        | TsExpr::Member { .. }
        | TsExpr::OptionalMember { .. }
        | TsExpr::Index { .. }
        | TsExpr::OptionalIndex { .. }
        | TsExpr::TypeApp { .. }
        | TsExpr::Call { .. }
        | TsExpr::OptionalCall { .. }
        | TsExpr::DefaultIife { .. }
        | TsExpr::New { .. }
        | TsExpr::Object { .. }
        | TsExpr::Array { .. }
        | TsExpr::LinesArray(_)
        | TsExpr::TemplateLit { .. }
        | TsExpr::Await(_)
        | TsExpr::Unary { .. }
        | TsExpr::Paren(_)
        | TsExpr::Spread(_)
        | TsExpr::NonNull(_)
        | TsExpr::Sequence(_)
        | TsExpr::Function { .. }
        | TsExpr::Lit(_) => false,
    };
    if needs_parens {
        out.push('(');
        render_expr(out, expr, depth, m);
        out.push(')');
    } else {
        render_expr(out, expr, depth, m);
    }
}

/// The operand of an `as T` / `as const` cast: a binary expression, an arrow
/// or a conditional is bracketed (see the `As` arm of [`render_expr`] for
/// why `Await` is not); anything else prints bare.
pub(super) fn render_cast_operand(
    out: &mut String,
    expr: &TsExpr,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    let needs_parens = matches!(
        expr,
        TsExpr::Binary { .. }
            | TsExpr::Arrow { .. }
            | TsExpr::Conditional { .. }
            | TsExpr::MultilineConditional { .. }
            | TsExpr::Yield(_)
    );
    if needs_parens {
        out.push('(');
        render_expr(out, expr, depth, m);
        out.push(')');
    } else {
        render_expr(out, expr, depth, m);
    }
}
