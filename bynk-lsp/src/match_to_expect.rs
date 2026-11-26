//! #4252: a quick fix that rewrites a two-arm `match` ending a test case, with
//! an `expect false` fallback arm, into the statement form of `expect … is`
//! (#3981):
//!
//! ```bynk
//! match r {                       expect r is Ok(v)
//!     Ok(v) => { expect v.total == 10 }     →     expect v.total == 10
//!     _ => { expect false }
//! }
//! ```
//!
//! Offered only when the `match` is the tail of a `case` body (a statement, not
//! a value), has exactly two arms, and one arm's body is only `expect false`.
//! The other arm becomes `expect <subject> is <pattern>` followed by its own
//! statements, unindented into the case block.
//!
//! Declines when:
//! - either arm has a guard, or the `expect false` arm comes first and the two
//!   patterns are not two different variants (`expect … is P2` would pass on a
//!   value `P1` sent to `expect false`: record, list and range patterns
//!   overlap);
//! - the kept pattern binds a name below the first level (`Some(Ok(x))`):
//!   `expect … is` binds only a payload's direct names (ADR 0169);
//! - the kept pattern is a bare name or `_`, or the pattern is an `|`
//!   alternative or refined pattern that binds a name;
//! - the subject is not a plain name, call, field access or method call (it
//!   would need parentheses the formatter might not keep);
//! - a name the pattern binds is already declared in the case: by a `let`, an
//!   earlier `is` test (including another rewrite of this kind) or a `par`
//!   block;
//! - a comment sits in the `match` outside the kept arm's body (the rewrite
//!   would drop it).

use bynk_syntax::ast::{
    Block, Expr, ExprKind, MatchArm, MatchBody, Pattern, SourceUnit, Statement, expr_children,
    statement_exprs,
};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::*;

/// The quick fix for the `match` under `requested`, if one is offered.
pub fn quick_fix(
    text: &str,
    requested: Span,
    uri: &Url,
    version: Option<i32>,
) -> Vec<CodeActionOrCommand> {
    let Some((span, new_text)) = plan(text, requested) else {
        return Vec::new();
    };
    let edit = TextEdit {
        range: crate::position::span_to_range(text, span),
        new_text,
    };
    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: "Rewrite match as `expect … is`".to_string(),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                edits: vec![OneOf::Left(edit)],
            }])),
            change_annotations: None,
        }),
        ..Default::default()
    })]
}

/// The edit (span, replacement) for the rewrite of the case-ending `match`
/// that `requested` touches, or `None` (see the module docs).
pub fn plan(text: &str, requested: Span) -> Option<(Span, String)> {
    let tokens = tokenize(text).ok()?;
    let (Some(unit), errs) = parse_unit_with_recovery(&tokens, text) else {
        return None;
    };
    if !errs.is_empty() {
        return None;
    }
    let SourceUnit::Suite(suite) = &unit else {
        return None;
    };
    let case = suite
        .cases
        .iter()
        .find(|c| touches(c.body.span, requested))?;
    let tail = &*case.body.tail;
    if !touches(tail.span, requested) {
        return None;
    }
    let ExprKind::Match { discriminant, arms } = &tail.kind else {
        return None;
    };
    let rewrite = rewrite_of(text, &case.body, tail.span, discriminant, arms)?;
    // A canonical file stays canonical: re-format the result and narrow the
    // edit to where the two texts differ.
    let opts = bynk_fmt::FormatOptions::default();
    if bynk_fmt::format_source(text, &opts).is_ok_and(|f| f == text) {
        let mut edited = text.to_string();
        edited.replace_range(tail.span.start..tail.span.end, &rewrite);
        let formatted = bynk_fmt::format_source(&edited, &opts).ok()?;
        return Some(crate::inline::changed_region(text, &formatted));
    }
    Some((tail.span, rewrite))
}

fn touches(span: Span, requested: Span) -> bool {
    requested.start <= span.end && requested.end >= span.start
}

/// The replacement text for the `match`, after every decline check.
fn rewrite_of(
    text: &str,
    case_body: &Block,
    match_span: Span,
    subject: &Expr,
    arms: &[MatchArm],
) -> Option<String> {
    let [first, second] = arms else {
        return None;
    };
    if first.guard.is_some() || second.guard.is_some() {
        return None;
    }
    let kept = if is_expect_false(&second.body) {
        first
    } else if is_expect_false(&first.body) && disjoint_variants(&first.pattern, &second.pattern) {
        second
    } else {
        return None;
    };
    // `expect false` in both arms leaves nothing worth keeping.
    if is_expect_false(&kept.body) || !binds_only_first_level(&kept.pattern) {
        return None;
    }
    if !matches!(
        subject.kind,
        ExprKind::Ident(_)
            | ExprKind::Call { .. }
            | ExprKind::FieldAccess { .. }
            | ExprKind::MethodCall { .. }
    ) {
        return None;
    }
    let declared = declared_names(&case_body.statements);
    if kept
        .pattern
        .bound_names()
        .iter()
        .any(|n| declared.contains(&n.name))
    {
        return None;
    }
    let body = body_text(text, &kept.body)?;
    // A comment anywhere in the match but the kept body would be dropped.
    let outside = format!(
        "{}{}",
        text.get(match_span.start..kept.body.span().start)?,
        text.get(kept.body.span().end..match_span.end)?
    );
    if outside.contains("//") {
        return None;
    }
    let subject_text = text.get(subject.span.start..subject.span.end)?;
    let pattern_text = text.get(kept.pattern.span().start..kept.pattern.span().end)?;
    let indent = line_indent(text, match_span.start);
    let mut out = format!("expect {subject_text} is {pattern_text}");
    for line in body.lines() {
        out.push('\n');
        if !line.trim().is_empty() {
            out.push_str(indent);
            out.push_str(line);
        }
    }
    Some(out)
}

/// When the `expect false` arm comes first, `expect s is P2` matches the
/// original only if `P1` and `P2` can never match the same value. That is
/// provable here only for two variant patterns naming different variants:
/// record and list patterns are open and range patterns overlap, so any other
/// pair is declined.
fn disjoint_variants(first: &Pattern, second: &Pattern) -> bool {
    matches!(
        (first, second),
        (
            Pattern::Variant { variant: a, .. },
            Pattern::Variant { variant: b, .. }
        ) if a.name != b.name
    )
}

/// Every name an earlier statement of the case declares: a `let`, a name an
/// `is` test binds (a statement-form `expect x is Ok(v)` keeps it for the rest
/// of the case), at any depth, and the same inside a `par` block.
fn declared_names(statements: &[Statement]) -> std::collections::HashSet<String> {
    fn is_names(e: &Expr, out: &mut std::collections::HashSet<String>) {
        if let ExprKind::Is { pattern, .. } = &e.kind {
            out.extend(pattern.bound_names().iter().map(|n| n.name.clone()));
        }
        for child in expr_children(e) {
            is_names(child, out);
        }
    }
    fn walk(statements: &[Statement], out: &mut std::collections::HashSet<String>) {
        for s in statements {
            match s {
                Statement::Let(l) | Statement::EffectLet(l) => {
                    out.insert(l.name.name.clone());
                }
                Statement::Par(p) => walk(&p.statements, out),
                _ => {}
            }
            let mut exprs = Vec::new();
            statement_exprs(s, &mut exprs);
            for e in exprs {
                is_names(e, out);
            }
        }
    }
    let mut out = std::collections::HashSet::new();
    walk(statements, &mut out);
    out
}

/// An arm body that is exactly `expect false`: `=> expect false` or
/// `=> { expect false }`.
fn is_expect_false(body: &MatchBody) -> bool {
    let is_false = |e: &Expr| matches!(e.kind, ExprKind::BoolLit(false));
    match body {
        MatchBody::Expr(e) => matches!(&e.kind, ExprKind::Expect(v) if is_false(v)),
        MatchBody::Block(b) => match b.statements.as_slice() {
            [Statement::Expect(s)] => s.snapshot.is_none() && is_false(&s.value),
            [] => matches!(&b.tail.kind, ExprKind::Expect(v) if is_false(v)),
            _ => false,
        },
    }
}

/// Whether `expect … is` binds every name the pattern introduces: only a
/// variant payload's, record field's or list element's own name (depth one).
fn binds_only_first_level(p: &Pattern) -> bool {
    let shallow = |sub: &Pattern| {
        matches!(sub, Pattern::Binding(_) | Pattern::Wildcard(_))
            || sub.bound_names().is_empty()
            // #3971: `is` binds an `as` name at this depth, so long as the
            // pattern inside binds nothing deeper.
            || matches!(sub, Pattern::As { inner, .. } if inner.bound_names().is_empty())
    };
    match p {
        Pattern::As { inner, .. } => binds_only_first_level(inner),
        Pattern::Wildcard(_) | Pattern::Binding(_) => false,
        Pattern::Literal { .. } | Pattern::Range(_) => true,
        Pattern::Variant { bindings, .. } => bindings.iter().all(|b| shallow(b.pattern())),
        Pattern::Record { fields, .. } => fields
            .iter()
            .all(|(_, sub)| sub.as_ref().is_none_or(shallow)),
        Pattern::List { elems, .. } => elems.iter().all(shallow),
        Pattern::Refined { .. } | Pattern::Or(..) => p.bound_names().is_empty(),
    }
}

/// The kept arm's statements as source text, unindented one level.
fn body_text(text: &str, body: &MatchBody) -> Option<String> {
    let span = body.span();
    let raw = text.get(span.start..span.end)?;
    let inner = match body {
        MatchBody::Expr(_) => raw,
        MatchBody::Block(_) => raw.strip_prefix('{')?.strip_suffix('}')?,
    };
    let inner = inner.trim();
    if inner.is_empty() {
        return None;
    }
    // Lines after the first drop their common indentation; the first line
    // starts right after `{`, so it has none to drop.
    let rest_indent = inner
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut lines = inner.lines();
    let mut out = lines.next().unwrap_or("").to_string();
    for l in lines {
        out.push('\n');
        out.push_str(
            l.get(rest_indent.min(l.len() - l.trim_start().len())..)
                .unwrap_or(l),
        );
    }
    Some(out)
}

/// The leading whitespace of the line `offset` is on.
fn line_indent(text: &str, offset: usize) -> &str {
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[start..];
    let len = line.len() - line.trim_start_matches([' ', '\t']).len();
    &line[..len]
}
