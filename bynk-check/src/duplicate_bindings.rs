//! #4550: a pattern that binds one name twice is refused
//! (`bynk.types.duplicate_pattern_binding`), at every pattern position: a
//! `match` arm, and through it `let … else`, a destructuring `let` and a lambda
//! parameter pattern (all desugared to a `match`), and the `is` operator. The
//! checker reports it once per whole pattern (its `check_pattern` and
//! `check_is`), so a suite case body is checked too.
//!
//! An or-pattern's alternatives must bind the *same* names (#474,
//! `bynk.types.or_pattern_binding_mismatch`), so a name repeated across its
//! alternatives is required, not a duplicate. Each alternative is therefore
//! walked on its own, against the names bound outside the or-pattern, and the
//! or-pattern as a whole binds what its first alternative binds.

use std::collections::HashMap;

use bynk_syntax::ast::{Ident, Pattern};
use bynk_syntax::error::CompileError;

/// The code a second binding of a name in one pattern is reported under.
pub(crate) const DUPLICATE_PATTERN_BINDING: &str = "bynk.types.duplicate_pattern_binding";

/// Each later binding of a name `pattern` already binds, with the binding it
/// repeats, in source order.
pub(crate) fn duplicate_bindings(pattern: &Pattern) -> Vec<(&Ident, &Ident)> {
    let mut seen = HashMap::new();
    let mut dups = Vec::new();
    walk(pattern, &mut seen, &mut dups);
    dups
}

/// The errors for every duplicate binding in `pattern`.
pub(crate) fn duplicate_binding_errors(pattern: &Pattern) -> Vec<CompileError> {
    duplicate_bindings(pattern)
        .into_iter()
        .map(|(again, first)| {
            CompileError::new(
                DUPLICATE_PATTERN_BINDING,
                again.span,
                format!("`{}` is bound more than once in this pattern", again.name),
            )
            .with_label(first.span, format!("`{}` is first bound here", first.name))
            .with_note("a pattern binds each name once; rename one of them, or use `_` where the value is not needed")
        })
        .collect()
}

type Seen<'p> = HashMap<&'p str, &'p Ident>;

fn walk<'p>(p: &'p Pattern, seen: &mut Seen<'p>, dups: &mut Vec<(&'p Ident, &'p Ident)>) {
    match p {
        Pattern::Wildcard(_) | Pattern::Literal { .. } | Pattern::Range(_) => {}
        Pattern::Binding(id) => bind(id, seen, dups),
        Pattern::Variant { bindings, .. } => {
            for b in bindings {
                walk(b.pattern(), seen, dups);
            }
        }
        Pattern::Record { fields, .. } => {
            // A field listed twice is `bynk.types.duplicate_pattern_field`'s
            // to report; its second listing is not walked, so one slip gets
            // one diagnostic.
            let mut listed = std::collections::HashSet::new();
            for (field, sub) in fields {
                if !listed.insert(field.name.as_str()) {
                    continue;
                }
                match sub {
                    Some(sub) => walk(sub, seen, dups),
                    // The shorthand `f` binds the field to a local named `f`.
                    None => bind(field, seen, dups),
                }
            }
        }
        Pattern::List { elems, rest, .. } => {
            for e in elems {
                walk(e, seen, dups);
            }
            if let Some(b) = rest.as_ref().and_then(|r| r.binder.as_ref()) {
                bind(b, seen, dups);
            }
        }
        Pattern::Refined { inner, .. } => walk(inner, seen, dups),
        // The inner pattern's names come first in source, so in `a as a` the
        // `as` name is the repeat.
        Pattern::As { inner, name, .. } => {
            walk(inner, seen, dups);
            bind(name, seen, dups);
        }
        Pattern::Or(alts, _) => {
            let outside = seen.clone();
            let mut first: Option<Seen<'p>> = None;
            for alt in alts {
                let mut own = outside.clone();
                walk(alt, &mut own, dups);
                first.get_or_insert(own);
            }
            if let Some(first) = first {
                *seen = first;
            }
        }
    }
}

fn bind<'p>(id: &'p Ident, seen: &mut Seen<'p>, dups: &mut Vec<(&'p Ident, &'p Ident)>) {
    match seen.get(id.name.as_str()) {
        Some(first) => dups.push((id, first)),
        None => {
            seen.insert(id.name.as_str(), id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DUPLICATE_PATTERN_BINDING;

    /// The codes a one-file project's analysis reports.
    fn codes(body: &str) -> Vec<&'static str> {
        let src = format!(
            "context app.probe\n\ntype Pt = {{ x: Int, y: Int }}\n\ntype Shape = | Dot(x: Int) | Line(a: Int, b: Int)\n\n{body}\n\nservice probe {{\n\ton call() -> Effect[Int] by Caller {{\n\t\tEffect.pure(0)\n\t}}\n}}\n"
        );
        crate::testkit::analyse(&[("app/probe.bynk", &src)])
            .analysis
            .errors
            .iter()
            .map(|a| a.error.category)
            .collect()
    }

    #[test]
    fn an_is_pattern_is_checked() {
        assert_eq!(
            codes("fn f(s: Shape) -> Bool {\n\ts is Line(a, a)\n}"),
            [DUPLICATE_PATTERN_BINDING]
        );
    }

    #[test]
    fn a_let_else_pattern_is_checked() {
        assert_eq!(
            codes("fn f(s: Shape) -> Int {\n\tlet Line(a, a) = s else { 0 }\n\ta\n}"),
            [DUPLICATE_PATTERN_BINDING]
        );
    }

    #[test]
    fn a_destructuring_let_is_checked() {
        assert_eq!(
            codes("fn f(p: Pt) -> Int {\n\tlet Pt { x, y: x, .. } = p\n\tx\n}"),
            [DUPLICATE_PATTERN_BINDING]
        );
    }

    #[test]
    fn a_lambda_parameter_pattern_is_checked() {
        assert_eq!(
            codes("fn f(ps: List[Pt]) -> List[Int] {\n\tps.map((Pt { x, y: x, .. }) => x)\n}"),
            [DUPLICATE_PATTERN_BINDING]
        );
    }

    /// An or-pattern's alternatives must share their names: that is not a
    /// duplicate, and the or-pattern rule is untouched.
    #[test]
    fn a_name_shared_across_or_alternatives_is_not_a_duplicate() {
        assert_eq!(
            codes("fn f(s: Shape) -> Int {\n\tmatch s {\n\t\tDot(x) | Line(x, _) => x,\n\t}\n}"),
            Vec::<&str>::new()
        );
        assert_eq!(
            codes("fn f(s: Shape) -> Int {\n\tmatch s {\n\t\tDot(x) | Line(_, y) => 0,\n\t}\n}"),
            ["bynk.types.or_pattern_binding_mismatch"],
            "the or-pattern rule still reports a different set of names"
        );
    }

    /// A name bound outside an or-pattern and again inside it is a duplicate.
    /// Bound first outside, each alternative that repeats it is reported;
    /// bound after, the or-pattern counts once, as its first alternative.
    #[test]
    fn a_name_bound_outside_an_or_pattern_and_inside_it_is_a_duplicate() {
        let before = codes(
            "type Wrap = | Wrapped(n: Int, s: Shape)\n\nfn f(w: Wrap) -> Int {\n\tmatch w {\n\t\tWrapped(n, Dot(n) | Line(n, _)) => n,\n\t}\n}",
        );
        assert_eq!(
            before,
            [DUPLICATE_PATTERN_BINDING, DUPLICATE_PATTERN_BINDING]
        );
        let after = codes(
            "type Wrap = | Wrapped(s: Shape, n: Int)\n\nfn f(w: Wrap) -> Int {\n\tmatch w {\n\t\tWrapped(Dot(n) | Line(n, _), n) => n,\n\t}\n}",
        );
        assert_eq!(after, [DUPLICATE_PATTERN_BINDING]);
    }

    /// A field listed twice is `duplicate_pattern_field`'s alone.
    #[test]
    fn a_field_listed_twice_is_reported_once() {
        assert_eq!(
            codes("fn f(p: Pt) -> Int {\n\tmatch p {\n\t\tPt { x, x, .. } => x,\n\t}\n}"),
            ["bynk.types.duplicate_pattern_field"]
        );
    }

    #[test]
    fn distinct_names_are_accepted() {
        assert_eq!(
            codes(
                "fn f(s: Shape) -> Int {\n\tmatch s {\n\t\tLine(a, b) as whole => a + b,\n\t\tDot(x) => x,\n\t}\n}"
            ),
            Vec::<&str>::new()
        );
    }
}
