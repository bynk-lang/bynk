//! A `Route` verdict's tags (#3887): `EmailActions { tag: Map.empty().insert("X-Ticket", "T-1") }`.
//!
//! A tag becomes a header on each forward, and Cloudflare keeps only `X-` headers there, so a name
//! must start with `X-` and hold letters, digits and `-`, and a value must have no line break. A
//! literal map that breaks this is `bynk.email.bad_tag`, found here; one built at run time has its
//! bad entries dropped by the runtime (logged by name only, never written).

use bynk_syntax::ast::{Block, Expr, ExprKind, ServiceDecl, ServiceProtocol};
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

fn check_block(body: &Block, errors: &mut Vec<CompileError>) {
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
            let mut inserts = Vec::new();
            literal_inserts(tag, &mut inserts);
            for (name, value) in inserts {
                if let ExprKind::StrLit(n) = &name.kind
                    && let Some(why) = name_problem(n)
                {
                    errors.push(
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
                    errors.push(
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
        stack.extend(bynk_syntax::ast::expr_children(e));
    }
}

/// Check every `EmailActions` literal tag map in a `from email` service's handlers.
pub fn check_route_tags(service: &ServiceDecl, errors: &mut Vec<CompileError>) {
    if !matches!(service.protocol, ServiceProtocol::Email { .. }) {
        return;
    }
    for handler in &service.handlers {
        check_block(&handler.body, errors);
    }
}
