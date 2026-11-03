//! #2838: the `let … else` suggestion for a nested `match` staircase.
//!
//! A block whose tail is a `match` with exactly one *continuing* arm
//! (`Some(x)`/`Ok(x)`) and one *fallback* arm (`None`/`Err(…)`/`_`), where the
//! continuing arm in turn ends in such a `match`, reads as a staircase of
//! fallible steps. `let P = e else { … }` says the same thing flat (§5.6: the
//! `let … else` is the desugaring of this `match` in reverse), so the shape is
//! reported as a [`Severity::Hint`](crate::error::Severity::Hint) carrying a
//! machine-applicable rewrite. It fires from nesting depth 2: a single two-arm
//! `match` stays idiomatic.
//!
//! The detection is syntactic — the patterns `Some`/`None`/`Ok`/`Err` already
//! say which type is matched — so it runs where the source text is at hand,
//! and one run serves the editor, `check --fix` and the diagnostics of a
//! failed build alike.

use super::*;
use crate::error::Applicability;

/// One step of a staircase: a two-arm `match` with a continuing arm and a
/// fallback arm.
struct Link<'e> {
    scrutinee: &'e Expr,
    pattern: &'e Pattern,
    continuation: &'e MatchBody,
    fallback_pattern: &'e Pattern,
    fallback: &'e MatchBody,
}

/// `Some(x)`/`Ok(x)` with a plain name or `_` inside: the pattern of a
/// continuing arm, and which fallback variant it pairs with.
fn continuing(pattern: &Pattern) -> Option<&'static str> {
    let Pattern::Variant {
        type_name: None,
        variant,
        bindings,
        ..
    } = pattern
    else {
        return None;
    };
    let [binding] = bindings.as_slice() else {
        return None;
    };
    if !matches!(
        binding.pattern(),
        Pattern::Binding(_) | Pattern::Wildcard(_)
    ) {
        return None;
    }
    match variant.name.as_str() {
        "Some" => Some("None"),
        "Ok" => Some("Err"),
        _ => None,
    }
}

/// Whether `pattern` is the fallback that pairs with `want` (`None`/`Err`), or
/// `_`.
fn is_fallback(pattern: &Pattern, want: &str) -> bool {
    match pattern {
        Pattern::Wildcard(_) => true,
        Pattern::Variant {
            type_name: None,
            variant,
            bindings,
            ..
        } if variant.name == want => match bindings.as_slice() {
            [] => want == "None",
            [b] => {
                want == "Err" && matches!(b.pattern(), Pattern::Binding(_) | Pattern::Wildcard(_))
            }
            _ => false,
        },
        _ => false,
    }
}

fn link(m: &Expr) -> Option<Link<'_>> {
    if m.is_let_else_match() {
        return None;
    }
    let ExprKind::Match { discriminant, arms } = &m.kind else {
        return None;
    };
    let [a, b] = arms.as_slice() else {
        return None;
    };
    if a.guard.is_some() || b.guard.is_some() {
        return None;
    }
    let (cont, fallback) = match (continuing(&a.pattern), continuing(&b.pattern)) {
        (Some(want), None) if is_fallback(&b.pattern, want) => (a, b),
        (None, Some(want)) if is_fallback(&a.pattern, want) => (b, a),
        _ => return None,
    };
    Some(Link {
        scrutinee: discriminant,
        pattern: &cont.pattern,
        continuation: &cont.body,
        fallback_pattern: &fallback.pattern,
        fallback: &fallback.body,
    })
}

/// The `match` a link's continuing arm ends in.
fn next_expr<'e>(l: &Link<'e>) -> Option<&'e Expr> {
    match l.continuation {
        MatchBody::Expr(e) => Some(e),
        MatchBody::Block(b) => Some(&b.tail),
    }
}

/// The links of the staircase `m` heads, outermost first.
fn chain(m: &Expr) -> Vec<(&Expr, Link<'_>)> {
    let mut out = Vec::new();
    let mut cur = Some(m);
    while let Some(e) = cur {
        let Some(l) = link(e) else { break };
        cur = next_expr(&l);
        out.push((e, l));
    }
    out
}

impl<'a> Parser<'a> {
    /// Called on the tail of every block: report the staircase it heads, if it
    /// heads one at least two steps deep, and take back the reports of the
    /// steps inside it (parsed first, so already reported on their own).
    pub(crate) fn check_staircase(&mut self, tail: &Expr) {
        let steps = chain(tail);
        if steps.len() < 2 {
            return;
        }
        let inner: Vec<usize> = steps.iter().skip(1).map(|(e, _)| e.span.start).collect();
        self.warnings
            .retain(|w| w.category != STAIRCASE_CODE || !inner.contains(&w.span.start));
        let head = tail.span.start;
        let span = Span::new_in(tail.span.file, head, steps[0].1.scrutinee.span.end);
        let mut err = CompileError::new(
            STAIRCASE_CODE,
            span,
            format!(
                "these {} nested `match`es can be flattened with `let … else`",
                steps.len()
            ),
        )
        .with_note(
            "each `match` has one arm that carries on and one that gives up; \
             `let P = e else { … }` says that without the nesting",
        );
        if let Some(flat) = self.flatten(&steps, tail.span) {
            err = err.with_suggestion(
                "flatten the nested `match`es into `let … else`",
                vec![(tail.span, flat)],
                Applicability::MachineApplicable,
            );
        }
        self.warnings.push(err);
    }

    /// The flat `let … else` form of `steps`, replacing the head `match` at
    /// `whole`. `None` when it cannot be written without losing something: a
    /// comment inside the staircase, which the rewrite would drop.
    fn flatten(&self, steps: &[(&Expr, Link<'_>)], whole: Span) -> Option<String> {
        if self.slice(whole).contains("//") {
            return None;
        }
        let base = self.indent_of(whole.start);
        let mut out = String::new();
        let mut first = true;
        let line = |out: &mut String, first: &mut bool, text: &str| {
            if !*first {
                out.push('\n');
                out.push_str(&base);
            }
            *first = false;
            out.push_str(text);
        };
        for (i, (_, l)) in steps.iter().enumerate() {
            let pat = self.slice(l.pattern.span());
            let scrut = self.slice(l.scrutinee.span);
            let binds = matches!(
                l.fallback_pattern,
                Pattern::Variant { bindings, .. }
                    if bindings.iter().any(|b| matches!(b.pattern(), Pattern::Binding(_)))
            );
            let fallback = match (binds, l.fallback) {
                (false, MatchBody::Expr(e)) => format!("{{ {} }}", self.slice(e.span)),
                (false, MatchBody::Block(b)) => self.slice(b.span).to_string(),
                (true, body) => {
                    let body = match body {
                        MatchBody::Expr(e) => self.slice(e.span),
                        MatchBody::Block(b) => self.slice(b.span),
                    };
                    format!(
                        "match {{ {} => {} }}",
                        self.slice(l.fallback_pattern.span()),
                        body
                    )
                }
            };
            line(
                &mut out,
                &mut first,
                &format!("let {pat} = {scrut} else {fallback}"),
            );
            let last = i + 1 == steps.len();
            match l.continuation {
                MatchBody::Block(b) => {
                    if b.implicit_tail {
                        return None;
                    }
                    let prefix = &self.source[b.span.start + 1..b.tail.span.start];
                    for text in self.reindent(prefix) {
                        line(&mut out, &mut first, &text);
                    }
                    if last {
                        for text in self.reindent(self.slice(b.tail.span)) {
                            line(&mut out, &mut first, &text);
                        }
                    }
                }
                MatchBody::Expr(e) => {
                    if last {
                        for text in self.reindent(self.slice(e.span)) {
                            line(&mut out, &mut first, &text);
                        }
                    }
                }
            }
        }
        Some(out)
    }

    /// The leading whitespace of the line `at` is on.
    fn indent_of(&self, at: usize) -> String {
        let start = self.source[..at].rfind('\n').map_or(0, |i| i + 1);
        self.source[start..at]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect()
    }

    /// `text`'s lines with their common indentation taken off (the first line,
    /// which starts mid-line, is only trimmed), blank lines dropped at the ends.
    fn reindent(&self, text: &str) -> Vec<String> {
        let text = text.trim();
        let lines: Vec<&str> = text.lines().collect();
        let common = lines
            .iter()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if i == 0 {
                    l.trim_start().to_string()
                } else {
                    l.chars().skip(common).collect()
                }
            })
            .collect()
    }
}

pub(crate) const STAIRCASE_CODE: &str = "bynk.parse.let_else_staircase";
