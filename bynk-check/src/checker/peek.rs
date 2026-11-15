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
//! The handle is a case-local value, bound by `let` (`let db =
//! D1.peek(shop.orders.Orders)`, `bynk.test.peek_not_bound` otherwise), whose
//! only use is as the receiver of those four operations.

use super::*;
use std::collections::BTreeMap;

/// The type of a peek handle: `D1Peek[shop.orders.Orders]`, one per peeked
/// capability. It has no declaration and cannot be written in source. The
/// handle is tracked by this type, so a binding that shadows it, in a lambda,
/// a pattern or an inner block, is the new binding's, and one that leaves scope
/// leaves the outer one as it was.
const PEEK_TYPE: &str = "D1Peek";

fn peek_type_name(participant: &str, cap: &str) -> String {
    format!("{PEEK_TYPE}[{participant}.{cap}]")
}

/// The participant and capability a handle type names.
fn parse_peek_type(name: &str) -> Option<(String, String)> {
    let path = name
        .strip_prefix(PEEK_TYPE)?
        .strip_prefix('[')?
        .strip_suffix(']')?;
    let (participant, cap) = path.rsplit_once('.')?;
    Some((participant.to_string(), cap.to_string()))
}

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

/// Whether `sql` begins with a statement that writes, by its leading keyword.
/// It needs no schema, so it holds under `[check] d1_sql = false`; a write it
/// cannot see from the keyword (`WITH … DELETE`) is found from the prepared
/// statement's `readonly` flag when the schema is there, and at run time.
pub(crate) fn begins_with_write(sql: &str) -> bool {
    let mut rest = sql;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, tail)| tail);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, tail)| tail);
        } else {
            break;
        }
    }
    let word: String = rest
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_ascii_uppercase();
    matches!(
        word.as_str(),
        "INSERT"
            | "UPDATE"
            | "DELETE"
            | "REPLACE"
            | "CREATE"
            | "DROP"
            | "ALTER"
            | "VACUUM"
            | "REINDEX"
            | "ANALYZE"
            | "ATTACH"
            | "DETACH"
    )
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
    if let Some(ty) = ctx.lookup(&root.name) {
        let handle = match &*ctx.tys.get(ty) {
            Ty::Named { name, .. } => parse_peek_type(name),
            _ => None,
        }?;
        return Some(check_handle_op(
            &handle, root, method, type_args, args, span, expected, expr_id, ctx,
        ));
    }
    if root.name == "D1" && method.name == "peek" && !ctx.input.types.contains_key("D1") {
        return Some(check_create(args, span, expr_id, ctx));
    }
    None
}

fn refuse(code: &'static str, span: Span, message: String, note: &str, ctx: &mut Ctx) {
    ctx.errors
        .push(CompileError::new(code, span, message).with_note(note.to_string()));
}

fn check_create(args: &[Expr], span: Span, expr_id: ExprId, ctx: &mut Ctx) -> Option<TyId> {
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
    // The handle is a value of its own, held by a `let`: the emitted case
    // keeps it under a name made from that binding.
    if ctx.caps.peek_let_value != Some(expr_id) {
        refuse(
            "bynk.test.peek_not_bound",
            span,
            "`D1.peek` is bound with `let`, and is used only through the handle it binds"
                .to_string(),
            "write `let db = D1.peek(<participant>.<Capability>)` and call `db.all(…)`",
            ctx,
        );
        return None;
    }
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
    Some(tys.intern(Ty::Named {
        name: peek_type_name(&participant, &cap),
        kind: NamedKind::Record,
        args: Vec::new(),
    }))
}

/// The value of a plain `let`, which is where a `D1.peek` may stand.
pub(super) fn expect_let(value: &Expr, ctx: &mut Ctx) {
    ctx.caps.peek_let_value = Some(value.id);
}

#[allow(clippy::too_many_arguments)]
fn check_handle_op(
    handle: &(String, String),
    receiver: &Ident,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    expected: Option<TyId>,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let (participant, cap) = handle;
    let info = ctx
        .caps
        .peek_env
        .as_ref()
        .and_then(|e| e.participants.get(participant))
        .and_then(|c| c.get(cap))
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
    let present = ctx.caps.capabilities.insert(cap.clone(), info);
    let name = Ident {
        name: cap.clone(),
        span: receiver.span,
    };
    let out = super::calls::check_static_call(
        &name, method, type_args, args, span, expected, expr_id, ctx,
    );
    match present {
        Some(p) => {
            ctx.caps.capabilities.insert(cap.clone(), p);
        }
        None => {
            ctx.caps.capabilities.remove(cap);
        }
    }
    // A statement that writes is refused whether or not there is a schema to
    // prepare it against.
    if let Some(sql) = args.first()
        && let Some(text) = super::d1::sql_literal_text(sql, ctx)
        && begins_with_write(&text)
    {
        refuse(
            "bynk.test.peek_write",
            sql.span,
            format!(
                "the SQL of `D1.peek` reads, but this statement of `{}.{}` would write",
                receiver.name, method.name
            ),
            "a peeked database is read-only: a case makes the app write by driving the app",
            ctx,
        );
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
            site.peek = Some(participant.clone());
        }
    }
    out
}
