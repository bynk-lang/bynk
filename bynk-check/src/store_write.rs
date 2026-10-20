//! #2194/#2195: recognising a write to an agent's own state, shared by the
//! sequence diagram (state-write notes) and the agent state model.
//!
//! Syntactic by design (agent-state-views track §3.3): the checker's resolved
//! `Callee` table is not retained for the IDE, so a store operation is
//! recognised from the AST against the agent's `store` fields and the
//! store-op registry's `mutates` flag, with local shadowing respected.

use std::collections::HashMap;

use crate::store_ops;
use bynk_syntax::ast::*;

/// A mutating store operation on one of the agent's own fields:
/// `field.method(args)`.
pub struct StoreWrite<'a> {
    pub field: &'a Ident,
    pub method: &'a Ident,
    pub args: &'a [Expr],
}

/// `expr` as a mutating store operation, when its receiver is a bare name
/// that is one of the agent's store fields (`store`: name → kind head), not
/// shadowed by a name in `bound`, and its method is one the registry marks as
/// mutating for that field's kind. `None` for a read, a query, any other call,
/// or a shadowed name.
pub fn store_write<'a>(
    store: &HashMap<String, String>,
    bound: &[String],
    expr: &'a Expr,
) -> Option<StoreWrite<'a>> {
    let ExprKind::MethodCall {
        receiver,
        method,
        args,
        ..
    } = &expr.kind
    else {
        return None;
    };
    let ExprKind::Ident(field) = &receiver.kind else {
        return None;
    };
    let head = store.get(&field.name)?;
    if bound.contains(&field.name) || !store_ops::is_mutating(head, &method.name) {
        return None;
    }
    Some(StoreWrite {
        field,
        method,
        args,
    })
}

/// The agent's store fields as name → kind head (`"Map"`, `"Cell"`, …).
pub fn store_kinds(agent: &AgentDecl) -> HashMap<String, String> {
    agent
        .store_fields
        .iter()
        .map(|f| (f.name.name.clone(), f.kind.head.name.clone()))
        .collect()
}
