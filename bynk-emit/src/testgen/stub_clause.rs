//! The per-clause pieces of a stub class (#4382): how a clause is matched against a call's
//! arguments and type arguments, and how its reply is encoded for the call site. Split out of
//! `stubs.rs`, which is the emission itself.

use super::*;
use bynk_check::checker::Types;
use bynk_check::test_suites::ResolvedStub;
use bynk_syntax::ast::{ArgPattern, BaseType, CapabilityOp, StubClause, TypeRef};
use bynk_ts::{TsExpr, TsStmt};
use std::sync::Arc;

/// #3945: `__tk && (the key matches)`: the clause answers the call made at its type arguments,
/// or at a longer resolved list that starts with them (#4382).
pub(super) fn type_key_cond(
    type_args: &[bynk_syntax::ast::TypeRef],
    key_types: &HashMap<String, Arc<bynk_syntax::ast::TypeDecl>>,
    tys: &Arc<Types>,
) -> TsExpr {
    and_expr(
        ident("__tk"),
        crate::ts_build::type_key_matches(
            member(ident("__tk"), "__bynkTypeKey"),
            &bynk_check::checker::capability_type_key(type_args, key_types, tys),
        ),
    )
}

/// The call's arguments, aliased ahead of every clause block: a clause binds its suite's
/// test-only commons, whose names may equal a parameter's, and the match test must compare the
/// argument, not that name. Only parameters some clause tests by value are aliased.
pub(super) fn alias_tested_args(
    body: &mut Vec<TsStmt>,
    rp: &ResolvedStub,
    clause_idxs: &[usize],
    op: &CapabilityOp,
) {
    for (i, param) in op.params.iter().enumerate() {
        let tested = clause_idxs
            .iter()
            .any(|&c| matches!(rp.clauses[c].args.get(i), Some(ArgPattern::Value(_))));
        if tested {
            body.push(const_(arg_alias(i), ident(param.name.name.clone())));
        }
    }
}

/// The alias of the call's `i`th argument.
pub(super) fn arg_alias(i: usize) -> String {
    ["__arg_", &i.to_string()].concat()
}

/// #4382: [`encoded_reply_payload`] for a clause of `rp`, against the operation as declared.
pub(super) fn clause_reply_payload(
    rp: &ResolvedStub,
    clause: &StubClause,
    clause_op: &CapabilityOp,
) -> Option<TypeRef> {
    let declared = rp
        .cap_decl
        .ops
        .iter()
        .find(|o| o.name.name == clause.method.name)
        .unwrap_or(clause_op);
    encoded_reply_payload(declared, clause_op)
}

/// #4382: the payload type a stubbed operation's reply travels as plain JSON in, when the
/// operation declares an `Effect[P]` reply that mentions one of its `Codable` type parameters
/// (a typed read: `Idempotency.dedup[T]`). The call site decodes that reply whole with the
/// codec of `P`, so a stub's value must be encoded with the same one. `clause_op` is the
/// operation with the clause's written type arguments substituted, so the payload is concrete;
/// `None` when the reply is not encoded, or a type parameter is still open.
fn encoded_reply_payload(orig: &CapabilityOp, clause_op: &CapabilityOp) -> Option<TypeRef> {
    let codable = bynk_check::checker::codable::codable_op_params(orig);
    let TypeRef::Effect(orig_payload, _) = &orig.return_type else {
        return None;
    };
    let TypeRef::Effect(payload, _) = &clause_op.return_type else {
        return None;
    };
    let hidden: HashMap<String, TypeRef> = codable
        .iter()
        .map(|c| (c.clone(), TypeRef::Base(BaseType::Int, orig.span)))
        .collect();
    let shown = bynk_syntax::print::type_ref_to_string;
    let mentions =
        shown(&bynk_check::wire::subst_type_ref(orig_payload, &hidden)) != shown(orig_payload);
    let open = clause_op.type_params.iter().any(|tp| {
        shown(&bynk_check::wire::subst_type_ref(
            payload,
            &HashMap::from([(
                tp.name.name.clone(),
                TypeRef::Base(BaseType::Int, orig.span),
            )]),
        )) != shown(payload)
    });
    (mentions && !open).then(|| (**payload).clone())
}
