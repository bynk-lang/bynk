//! #3147: `D1.peek(<participant>.<Cap>)`, the case-only handle on a
//! participant's test D1 database.
//!
//! A `system` (or `integration`) case drives the whole deployable app through
//! its real boundaries, so it cannot name a capability. `D1.peek` is the one
//! built-in that reaches behind them, and only to **read**: the handle offers
//! `all`, `first`, `select[T]` and `firstAs[T]`, with the capability's own
//! signatures, and the SQL of each is checked against the migrations' schema
//! like any other D1 call. A write is refused: by operation here, and by the
//! prepared statement's `readonly` flag in [`crate::d1_sql`] (and at run time).
//!
//! The handle is a case-local value (`let db = D1.peek(shop.orders.Orders)`)
//! whose only use is as the receiver of those four operations.

use super::*;
use std::collections::BTreeMap;

/// The type of a peek handle. It has no declaration and cannot be written in
/// source; it exists so the binding has a name to show in a hint.
pub const PEEK_TYPE: &str = "D1Peek";

/// The operations a handle offers: the reads, as the capability declares them.
const READ_OPS: &[&str] = &["all", "first", "select", "firstAs"];

/// The operations that would write, named so the refusal can say why. Any
/// other name is simply not an operation of the handle.
const WRITE_OPS: &[&str] = &["run", "batch"];

/// The capabilities a case can peek: each participant's own, by participant
/// name. Present only while checking an `integration` or `system` case.
#[derive(Debug, Clone, Default)]
pub struct PeekEnv {
    pub participants: BTreeMap<String, BTreeMap<String, CapabilityInfo>>,
}

/// What a peek handle names: a participant and its D1 capability.
#[derive(Debug, Clone)]
pub struct PeekHandle {
    pub participant: String,
    pub cap: String,
}

/// The dotted path an expression spells (`shop.orders.Orders`), as segments.
fn path_of(e: &Expr) -> Option<Vec<String>> {
    match &e.kind {
        ExprKind::Ident(id) => Some(vec![id.name.clone()]),
        ExprKind::FieldAccess { receiver, field } => {
            let mut head = path_of(receiver)?;
            head.push(field.name.clone());
            Some(head)
        }
        _ => None,
    }
}

fn is_d1(info: &CapabilityInfo) -> bool {
    matches!(&info.derived_from, Some((unit, base))
        if unit == crate::firstparty::CLOUDFLARE_UNIT && base == "D1")
}

/// `D1.peek(…)` and `<handle>.<op>(…)`: `Some(result)` when the call is one of
/// them, `None` to let the ordinary call checks have it.
#[allow(clippy::too_many_arguments)]
pub(super) fn check_peek_call(
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    expected: Option<TyId>,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    let ExprKind::Ident(root) = &receiver.kind else {
        return None;
    };
    if ctx.lookup(&root.name).is_some() {
        let handle = ctx.caps.peek_handles.get(&root.name).cloned()?;
        return Some(check_handle_op(
            &handle, root, method, type_args, args, span, expected, expr_id, ctx,
        ));
    }
    if root.name == "D1" && method.name == "peek" && !ctx.input.types.contains_key("D1") {
        return Some(check_create(receiver, method, args, span, expr_id, ctx));
    }
    None
}

fn refuse(code: &'static str, span: Span, message: String, note: &str, ctx: &mut Ctx) {
    ctx.errors
        .push(CompileError::new(code, span, message).with_note(note.to_string()));
}

fn check_create(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let Some(env) = ctx.caps.peek_env.clone() else {
        refuse(
            "bynk.test.peek_outside_case",
            span,
            "`D1.peek` is only valid inside an `integration` or `system` case".to_string(),
            "it reads a participant's test database, which only those tiers stand up; a `unit` case reads the capability it stubs, and the deployed app has no such handle",
            ctx,
        );
        return None;
    };
    let _ = (receiver, method);
    let [arg] = args else {
        refuse(
            "bynk.test.peek_unknown_capability",
            span,
            format!(
                "`D1.peek` takes one argument, a participant's D1 capability, but {} were given",
                args.len()
            ),
            "write `D1.peek(<participant>.<Capability>)`, for example `D1.peek(shop.orders.Orders)`",
            ctx,
        );
        return None;
    };
    let Some(path) = path_of(arg).filter(|p| p.len() >= 2) else {
        refuse(
            "bynk.test.peek_unknown_capability",
            arg.span,
            "`D1.peek` names a capability by its participant, as `<participant>.<Capability>`"
                .to_string(),
            "for example `D1.peek(shop.orders.Orders)`",
            ctx,
        );
        return None;
    };
    let (cap, participant) = (
        path[path.len() - 1].clone(),
        path[..path.len() - 1].join("."),
    );
    let Some(caps) = env.participants.get(&participant) else {
        refuse(
            "bynk.test.peek_unknown_capability",
            arg.span,
            format!("`{participant}` is not a participant of this suite"),
            &format!(
                "the participants here are: {}",
                env.participants
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            ctx,
        );
        return None;
    };
    let Some(info) = caps.get(&cap) else {
        refuse(
            "bynk.test.peek_unknown_capability",
            arg.span,
            format!("`{participant}` declares no capability `{cap}`"),
            "name a capability the participant declares, `capability <Name> = D1`",
            ctx,
        );
        return None;
    };
    if !is_d1(info) {
        refuse(
            "bynk.test.peek_not_a_database",
            arg.span,
            format!("`D1.peek` reads a D1 database, but `{participant}.{cap}` is not a D1 binding"),
            "only a capability derived from `D1` has a test database to read",
            ctx,
        );
        return None;
    }
    ctx.caps
        .peek_resolved
        .insert(expr_id, PeekHandle { participant, cap });
    let _ = span;
    Some(tys.intern(Ty::Named {
        name: PEEK_TYPE.to_string(),
        kind: NamedKind::Record,
        args: Vec::new(),
    }))
}

/// A `let` binding's value: if it is a resolved `D1.peek(…)`, the name is a
/// handle from here on; any other binding of the name ends one.
pub(super) fn note_let(name: &str, value: &Expr, ctx: &mut Ctx) {
    ctx.caps.peek_handles.remove(name);
    if let Some(h) = ctx.caps.peek_resolved.get(&value.id).cloned() {
        ctx.caps.peek_handles.insert(name.to_string(), h);
    }
}

/// The effect-`let` form (`let x <- …`) can only end a handle's name.
pub(super) fn forget(name: &str, ctx: &mut Ctx) {
    ctx.caps.peek_handles.remove(name);
}

#[allow(clippy::too_many_arguments)]
fn check_handle_op(
    handle: &PeekHandle,
    receiver: &Ident,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    expected: Option<TyId>,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let info = ctx
        .caps
        .peek_env
        .as_ref()
        .and_then(|e| e.participants.get(&handle.participant))
        .and_then(|c| c.get(&handle.cap))
        .cloned();
    let info = info?;
    if !READ_OPS.contains(&method.name.as_str()) {
        if WRITE_OPS.contains(&method.name.as_str()) {
            refuse(
                "bynk.test.peek_write",
                method.span,
                format!("`{}` writes, and `D1.peek` is read-only", method.name),
                "the handle offers `all`, `first`, `select[T]` and `firstAs[T]`; a case makes the app write by driving the app",
                ctx,
            );
        } else {
            refuse(
                "bynk.capability.unknown_operation",
                method.span,
                format!(
                    "a `D1.peek` handle has no operation named `{}`",
                    method.name
                ),
                "the handle offers `all`, `first`, `select[T]` and `firstAs[T]`",
                ctx,
            );
        }
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    // The ordinary capability call, over the participant's own signatures.
    let present = ctx.caps.capabilities.insert(handle.cap.clone(), info);
    let name = Ident {
        name: handle.cap.clone(),
        span: receiver.span,
    };
    let out = super::calls::check_static_call(
        &name, method, type_args, args, span, expected, expr_id, ctx,
    );
    match present {
        Some(p) => {
            ctx.caps.capabilities.insert(handle.cap.clone(), p);
        }
        None => {
            ctx.caps.capabilities.remove(&handle.cap);
        }
    }
    // The statement is the participant's: the project phase prepares it
    // against that participant's migrations, and refuses a write.
    if let Some(sql) = args.first()
        && let Some(callee) = ctx.callees.get_mut(&sql.id)
    {
        let site = match callee {
            Callee::D1Sql(site) => Some(site),
            Callee::Const(_, Some(site)) => Some(site.as_mut()),
            _ => None,
        };
        if let Some(site) = site {
            site.peek = Some(handle.participant.clone());
        }
    }
    out
}
