//! #3980: what a failed `expect <subject> is …`, and an `expect false` in a `match` arm, say
//! about the value they tested. The failure names the subject (`r was <value>`), so a test's
//! author does not have to re-run it with a `println`.
//!
//! The value is shown by the test runtime's value printer, only when the expectation fails (a
//! thunk the runtime calls). A subject with no codec shows its type; a value of a redacted
//! type (a key, token claims) shows `<redacted>`. The subject must be a name or a field path,
//! so showing it re-reads a place instead of repeating a call.

use bynk_check::checker::{Ty, TyId};
use bynk_syntax::ast::{ChainLink, Expr, ExprKind, MatchArm, MatchBody};
use bynk_syntax::span::Span;

use super::super::lower_ctx::LowerCtx;
use super::expect::expect_source_text_raw;
use crate::ts_build::{ident, num_lit, str_lit};

/// How a failure shows the tested value.
pub(in crate::emitter) enum Was {
    /// Text that needs no run-time work: a type name or `<redacted>`.
    Static(String),
    /// The printed value, built by `thunk` (`() => __bynkShow(…)`) on failure.
    Encoded { thunk: bynk_ts::TsExpr },
}

/// A report: the subject's name and how to show it.
pub(in crate::emitter) struct Report {
    pub label: String,
    pub was: Was,
}

impl Report {
    /// `head` with the static report appended, or `head` unchanged for an encoded one (the
    /// runtime appends that line itself).
    pub fn detail(&self, head: &str) -> String {
        match &self.was {
            Was::Static(text) => [head, "\n  ", self.label.as_str(), " was ", text].concat(),
            Was::Encoded { .. } => head.to_string(),
        }
    }

    /// The runtime's `label` and `was` arguments, for an encoded report.
    pub fn runtime_args(&self) -> Vec<bynk_ts::TsExpr> {
        match &self.was {
            Was::Encoded { thunk } => vec![str_lit(self.label.clone()), thunk.clone()],
            Was::Static(_) => Vec::new(),
        }
    }
}

/// The scrutinee of a `match` being lowered, and its arms, so an `expect false` arm can say
/// which arm it is and what was matched.
pub(in crate::emitter) struct ScrutineeFrame {
    label: String,
    ty: TyId,
    path: bynk_ts::TsExpr,
    /// Each arm's body span and its pattern's source text.
    arms: Vec<(Span, String)>,
}

/// The frame for a `match` over `discriminant` (lowered to `path`), or `None` when the
/// discriminant is not a place or has no type.
pub(in crate::emitter) fn scrutinee_frame(
    discriminant: &Expr,
    path: &bynk_ts::TsExpr,
    arms: &[MatchArm],
    cx: &LowerCtx,
) -> Option<ScrutineeFrame> {
    if !is_place(discriminant) || cx.roots_at_const(discriminant) {
        return None;
    }
    let ty = cx
        .commons()
        .expr_types
        .get(&discriminant.id)
        .map(|te| te.ty)?;
    Some(ScrutineeFrame {
        label: expect_source_text_raw(cx, discriminant.span),
        ty,
        path: path.clone(),
        arms: arms
            .iter()
            .filter_map(|arm| match &arm.body {
                MatchBody::Expr(e) => {
                    Some((e.span, expect_source_text_raw(cx, arm.pattern.span())))
                }
                MatchBody::Block(_) => None,
            })
            .collect(),
    })
}

/// A name or a field path: re-reading it has no effect.
fn is_place(e: &Expr) -> bool {
    match e.kind.chain_link() {
        Some(ChainLink::Root(_)) => true,
        Some(ChainLink::Field { receiver, .. } | ChainLink::Paren(receiver)) => is_place(receiver),
        None => false,
    }
}

/// The report for `expect <subject> is <pattern>`, when the subject is a place.
pub(in crate::emitter) fn subject_report(predicate: &Expr, cx: &mut LowerCtx) -> Option<Report> {
    let ExprKind::Is { value, .. } = &predicate.kind else {
        return None;
    };
    if !is_place(value) || cx.roots_at_const(value) {
        return None;
    }
    let ty = cx.commons().expr_types.get(&value.id).map(|te| te.ty)?;
    let label = expect_source_text_raw(cx, value.span);
    let mut pre = super::Pre::new();
    let node = pre.lower_node(value, cx);
    if !pre.is_empty() {
        return None;
    }
    Some(Report {
        was: was_of(ty, &node, cx),
        label,
    })
}

/// The report for an `expect false` that is a `match` arm's whole body: the head names the
/// arm, and the value is the scrutinee.
pub(in crate::emitter) fn arm_report(inner: &Expr, cx: &mut LowerCtx) -> Option<(String, Report)> {
    if !matches!(inner.kind, ExprKind::BoolLit(false)) {
        return None;
    }
    let (arm, label, ty, path) = cx.scrutinee_frames().iter().rev().find_map(|f| {
        f.arms
            .iter()
            .find(|(span, _)| span.start <= inner.span.start && inner.span.end <= span.end)
            .map(|(_, arm)| (arm.clone(), f.label.clone(), f.ty, f.path.clone()))
    })?;
    let head = [
        "(in the `",
        arm.as_str(),
        "` arm of `match ",
        label.as_str(),
        "`)",
    ]
    .concat();
    let was = was_of(ty, &path, cx);
    Some((head, Report { label, was }))
}

fn was_of(ty: TyId, value: &bynk_ts::TsExpr, cx: &mut LowerCtx) -> Was {
    if is_redacted(ty, cx) {
        return Was::Static("<redacted>".to_string());
    }
    // Plain data at run time, an `HttpResult` or `QueueResult` included: the runtime's own value
    // printer (`__bynkShow`) shows it, with no codec for the test module to generate or import.
    if is_tagged_outcome(ty, cx) || is_plain_data(ty, cx) {
        // #4229: a `Duration` or `Instant` inside the value prints in Bynk's form.
        let shape =
            crate::testgen::show_shape::show_shape(ty, &cx.commons().types, cx.commons().tys());
        let (printer, args) = match shape {
            Some(shape) => ("__bynkShowTyped", vec![value.clone(), shape]),
            None => ("__bynkShow", vec![value.clone()]),
        };
        return Was::Encoded {
            thunk: bynk_ts::TsExpr::Arrow {
                params: Vec::new(),
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(
                    bynk_ts::TsExpr::Call {
                        callee: Box::new(ident(printer)),
                        args,
                    },
                ))),
            },
        };
    }
    let shown = cx.commons().tys().display(ty);
    let article = if shown.starts_with(['A', 'E', 'I', 'O', 'U']) {
        "an"
    } else {
        "a"
    };
    Was::Static([article, " ", shown.as_str()].concat())
}

/// Whether `ok` holds for `ty` and every type inside it. A leaf with no inner type is judged by
/// `leaf`; a function, stream, connection or other non-data type is `leaf`'s to judge too.
fn all_parts(ty: TyId, cx: &LowerCtx, named: &dyn Fn(&str) -> bool, other: bool) -> bool {
    let tys = cx.commons().tys();
    match &*tys.get(ty) {
        Ty::Named { name, args, .. } => {
            named(name) && args.iter().all(|a| all_parts(*a, cx, named, other))
        }
        Ty::Result(a, b) | Ty::Map(a, b) => {
            all_parts(*a, cx, named, other) && all_parts(*b, cx, named, other)
        }
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => all_parts(*a, cx, named, other),
        Ty::Base(_) | Ty::Unit => true,
        Ty::Error
        | Ty::Never
        | Ty::Effect(_)
        | Ty::HttpResult(_, _)
        | Ty::QueueResult
        | Ty::HttpOutcome(_)
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::RequestQuery
        | Ty::RequestCookies
        | Ty::TokenClaims
        | Ty::Query(_)
        | Ty::Stream(_)
        | Ty::Connection(_)
        | Ty::TestConnection(_)
        | Ty::ValidationError
        | Ty::DecodeError
        | Ty::Actor(_)
        | Ty::ActorSum(_)
        | Ty::Fn { .. }
        | Ty::Var(_) => other,
    }
}

/// A type with no function, stream or connection in it: plain data at run time.
fn is_plain_data(ty: TyId, cx: &LowerCtx) -> bool {
    all_parts(ty, cx, &|_| true, false)
}

fn is_tagged_outcome(ty: TyId, cx: &LowerCtx) -> bool {
    matches!(
        &*cx.commons().tys().get(ty),
        Ty::HttpResult(_, _) | Ty::HttpOutcome(_) | Ty::QueueResult
    )
}

/// A value of a type the runtime refuses to log: a `SigningKey`, a `VerifyKey`, a `CipherKey` or token claims,
/// alone or inside a container.
/// The type walk need not enter record fields or sum payloads: the checker refuses a key or
/// token claims there (`bynk.types.key_at_boundary`, `bynk.types.claims_at_boundary`; fixtures
/// `2327_key_at_boundary_record` and `2380_claims_at_boundary`), so a user type never holds one.
fn is_redacted(ty: TyId, cx: &LowerCtx) -> bool {
    let tys = cx.commons().tys();
    match &*tys.get(ty) {
        Ty::TokenClaims => true,
        Ty::Named { name, args, .. } => {
            matches!(
                bynk_check::symbols::display_type_name(name),
                "CipherKey" | "SigningKey" | "VerifyKey"
            ) || args.iter().any(|a| is_redacted(*a, cx))
        }
        Ty::Result(a, b) | Ty::Map(a, b) => is_redacted(*a, cx) || is_redacted(*b, cx),
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => is_redacted(*a, cx),
        Ty::Error
        | Ty::Never
        | Ty::Base(_)
        | Ty::Effect(_)
        | Ty::HttpResult(_, _)
        | Ty::QueueResult
        | Ty::HttpOutcome(_)
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::RequestQuery
        | Ty::RequestCookies
        | Ty::Query(_)
        | Ty::Stream(_)
        | Ty::Connection(_)
        | Ty::TestConnection(_)
        | Ty::ValidationError
        | Ty::DecodeError
        | Ty::Unit
        | Ty::Actor(_)
        | Ty::ActorSum(_)
        | Ty::Fn { .. }
        | Ty::Var(_) => false,
    }
}

/// `if (!(cond)) { throw __bynkExpectFailure(location, start, end, detail, label, was); }`:
/// the statement form of an encoded report.
pub(in crate::emitter) fn guard(
    cond: bynk_ts::TsExpr,
    location: &str,
    (start, end): (usize, usize),
    detail: &str,
    report: &Report,
) -> bynk_ts::TsStmt {
    use bynk_ts::{TsExpr, TsStmt, TsUnaryOp};
    let mut args = vec![
        str_lit(location),
        num_lit(start.to_string()),
        num_lit(end.to_string()),
        str_lit(detail),
    ];
    args.extend(report.runtime_args());
    let throw = TsStmt::throw_stmt(
        TsExpr::Call {
            callee: Box::new(ident("__bynkExpectFailure")),
            args,
        },
        None,
    );
    TsStmt::if_stmt(
        TsExpr::Unary {
            op: TsUnaryOp::Not,
            expr: Box::new(TsExpr::Paren(Box::new(cond))),
        },
        TsStmt::inline_block(vec![throw], None),
        None,
    )
}
