//! A `Route` verdict's tags (#3887): `EmailActions { tag: Map.empty().insert("X-Ticket", "T-1") }`.
//!
//! A tag becomes a header on each forward, and Cloudflare keeps only `X-` headers there, so a name
//! must start with `X-` and hold letters, digits and `-`, and a value must have no line break. A
//! literal map that breaks this is `bynk.email.bad_tag`, found here wherever the literal is written,
//! in a handler body or in a helper `fn` (#4486); one built at run time has its bad entries dropped
//! by the runtime (logged by name only, never written).

use std::collections::HashMap;
use std::sync::Arc;

use bynk_syntax::ast::{Block, Expr, ExprKind, FnDecl, ServiceDecl, ServiceProtocol};
use bynk_syntax::error::CompileError;

fn name_problem(name: &str) -> Option<&'static str> {
    match name.strip_prefix("X-").or_else(|| name.strip_prefix("x-")) {
        None => Some(
            "a tag name starts with `X-`: Cloudflare removes every other header from a forward",
        ),
        Some(rest) if rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => None,
        Some(_) => Some("a tag name holds only letters, digits and `-` after `X-`"),
    }
}

/// The `insert("name", "value")` calls of a literal tag map, outermost last: `Map.empty()` chains.
fn literal_inserts<'a>(map: &'a Expr, out: &mut Vec<(&'a Expr, &'a Expr)>) {
    if let ExprKind::MethodCall {
        receiver,
        method,
        args,
        ..
    } = &map.kind
        && method.name == "insert"
        && args.len() == 2
    {
        literal_inserts(receiver, out);
        out.push((&args[0], &args[1]));
    }
}

/// Check every `EmailActions` literal tag map written in `body`. A `tag:` that calls a function of
/// `fns` by name is followed into that function: the literal map it returns is checked at the place
/// it is written, so a helper that builds the tags is checked as a handler body that writes them is
/// (#4486). Only a call by a bare name is followed, and only a helper whose result is a literal map
/// chain; any other tag map is built at run time, where the runtime drops a bad entry.
pub(crate) fn check_block(
    body: &Block,
    fns: &HashMap<String, Arc<FnDecl>>,
    errors: &mut Vec<CompileError>,
) {
    let mut stack: Vec<&Expr> = Vec::new();
    for s in &body.statements {
        bynk_syntax::ast::statement_exprs(s, &mut stack);
    }
    stack.push(&body.tail);
    while let Some(e) = stack.pop() {
        if let ExprKind::RecordConstruction {
            type_name, fields, ..
        } = &e.kind
            && type_name.name == "EmailActions"
            && let Some(tag) = fields
                .iter()
                .find(|f| f.name.name == "tag")
                .and_then(|f| f.value.as_ref())
        {
            check_tag_map(tag, errors);
            if let ExprKind::Call { name, .. } = &tag.kind
                && let Some(helper) = fns.get(&name.name)
            {
                check_tag_map(&helper.body.tail, errors);
            }
        }
        stack.extend(bynk_syntax::ast::expr_children(e));
    }
}

/// Report each entry of the literal tag map `tag` that is not a header a forward can carry, once per
/// place: a helper that two literals call is reported once.
fn check_tag_map(tag: &Expr, errors: &mut Vec<CompileError>) {
    let mut inserts = Vec::new();
    literal_inserts(tag, &mut inserts);
    let mut report = |err: CompileError| {
        if !errors
            .iter()
            .any(|e| e.category == err.category && e.span == err.span)
        {
            errors.push(err);
        }
    };
    for (name, value) in inserts {
        if let ExprKind::StrLit(n) = &name.kind
            && let Some(why) = name_problem(n)
        {
            report(
                CompileError::new(
                    "bynk.email.bad_tag",
                    name.span,
                    format!("`{n}` is not a header a forward can carry: {why}"),
                )
                .with_note(
                    "write a name like `X-Ticket`; a run-time tag that breaks this is dropped, not written",
                ),
            );
        }
        if let ExprKind::StrLit(v) = &value.kind
            && v.contains(['\r', '\n'])
        {
            report(
                CompileError::new(
                    "bynk.email.bad_tag",
                    value.span,
                    "a tag value cannot contain a line break: it would end the header and start another",
                )
                .with_note(
                    "a value is one line; a run-time tag that breaks this is dropped, not written",
                ),
            );
        }
    }
}

/// Check every `EmailActions` literal tag map in a `from email` service's handlers.
pub fn check_route_tags(
    service: &ServiceDecl,
    fns: &HashMap<String, Arc<FnDecl>>,
    errors: &mut Vec<CompileError>,
) {
    if !matches!(service.protocol, ServiceProtocol::Email { .. }) {
        return;
    }
    for handler in &service.handlers {
        check_block(&handler.body, fns, errors);
    }
}

/// Check every `EmailActions` literal tag map written in a helper function of the context (#4486).
pub fn check_fn_tags(fns: &HashMap<String, Arc<FnDecl>>, errors: &mut Vec<CompileError>) {
    let mut sorted: Vec<&Arc<FnDecl>> = fns.values().collect();
    // Source order, so the diagnostics read the same on every run.
    sorted.sort_by_key(|f| f.span);
    for f in sorted {
        check_block(&f.body, fns, errors);
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::analyse;

    /// A `from email` service whose handler routes `route`, with `helpers` declared before it.
    fn source(helpers: &str, route: &str) -> String {
        format!(
            "context shop.support\n\nconsumes bynk.cloudflare {{\n    InboundEmail,\n    EmailVerdict,\n    EmailActions,\n}}\n\n{helpers}\nservice support from email(\"support@shop.example\") {{\n    on message(mail: InboundEmail) -> Effect[EmailVerdict] by Sender {{\n        {route}\n    }}\n}}\n"
        )
    }

    fn bad_tags(src: &str) -> usize {
        analyse(&[("shop/support.bynk", src)])
            .error_categories()
            .iter()
            .filter(|c| **c == "bynk.email.bad_tag")
            .count()
    }

    #[test]
    fn a_good_tag_in_a_helper_fn_is_accepted_in_both_shapes() {
        let literal = source(
            "fn routed() -> EmailActions {\n    EmailActions { forward: [\"b@shop.example\"], tag: Map.empty().insert(\"X-Ticket\", \"a\") }\n}\n",
            "Route(routed())",
        );
        assert_eq!(bad_tags(&literal), 0);
        let map = source(
            "fn tags() -> Map[String, String] {\n    Map.empty().insert(\"X-Ticket\", \"a\")\n}\n",
            "Route(EmailActions { forward: [\"b@shop.example\"], tag: tags() })",
        );
        assert_eq!(bad_tags(&map), 0);
    }

    #[test]
    fn a_bad_tag_in_a_helper_fn_is_refused_in_both_shapes() {
        let literal = source(
            "fn routed() -> EmailActions {\n    EmailActions { forward: [\"b@shop.example\"], tag: Map.empty().insert(\"Subject\", \"a\") }\n}\n",
            "Route(routed())",
        );
        assert_eq!(bad_tags(&literal), 1);
        let map = source(
            "fn tags() -> Map[String, String] {\n    Map.empty().insert(\"Subject\", \"a\")\n}\n",
            "Route(EmailActions { forward: [\"b@shop.example\"], tag: tags() })",
        );
        assert_eq!(bad_tags(&map), 1);
    }

    #[test]
    fn a_helper_two_literals_call_is_reported_once() {
        let twice = source(
            "fn tags() -> Map[String, String] {\n    Map.empty().insert(\"Subject\", \"a\")\n}\n\nfn other() -> EmailActions {\n    EmailActions { forward: [\"b@shop.example\"], tag: tags() }\n}\n",
            "Route(EmailActions { forward: [\"b@shop.example\"], tag: tags() })",
        );
        assert_eq!(bad_tags(&twice), 1);
    }

    #[test]
    fn a_tag_map_built_at_run_time_is_left_to_the_runtime() {
        let runtime = source(
            "fn tags(name: String) -> Map[String, String] {\n    Map.empty().insert(name, \"a\")\n}\n",
            "Route(EmailActions { forward: [\"b@shop.example\"], tag: tags(\"Subject\") })",
        );
        assert_eq!(bad_tags(&runtime), 0);
    }
}
