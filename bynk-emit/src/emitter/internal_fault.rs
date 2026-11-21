//! The one policy for a checker fact the emitter needs and does not find.
//!
//! A checked program carries every fact lowering reads: an expression's type,
//! a call's callee, the arity of an argument list, a field's default. When one
//! is absent the checker and the emitter disagree, which no program the user
//! wrote can cause. The emitter used to answer that in four ways (an `any` in a
//! type position, an `undefined` in a value position, a panic, and a silent
//! `Ty::Error`), so a compiler fault could compile to a program that meant
//! something else. It now answers it in one: [`LowerCtx::missing_fact`] records
//! an [`InternalFault`] against the `.bynk` span and returns the `undefined`
//! identifier so lowering carries on and reports every fault of the unit, not
//! only the first. The unit's output is never used; the driver turns each
//! fault into a `bynk.internal.missing_checker_fact` diagnostic.
//!
//! Three situations stay out of this path, and the sweep classified each
//! fallback in `bynk-emit` as exactly one of them:
//! 1. **a missing fact**: `missing_fact` / `missing_fact_ty`;
//! 2. **a named widening**: the receiver type arguments of a generic instance
//!    method, spelled by [`unnameable_receiver_type_arg`] (#2795);
//! 3. **a legitimate default**: an `Option` in the Rust, rendered once as the
//!    `undefined` identifier ([`undefined_node`]).
//!
//! The sink has the shape of [`RuntimeUse`](super::RuntimeUse): interior
//! mutability, so it threads through the `&`-taking lowering tree without a
//! `&mut` chain, and it lives on `RuntimeUse` itself because that is the one
//! per-module accumulator every lowering context already shares.

use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;
use bynk_ts::{TsExpr, TsType};

/// The diagnostic code every [`InternalFault`] is reported under.
pub const MISSING_CHECKER_FACT: &str = "bynk.internal.missing_checker_fact";

/// Where to report a compiler fault.
const REPORT_URL: &str = "https://github.com/bynk-lang/bynk/issues";

/// One checker fact the emitter needed and did not find: the `.bynk` span of
/// the construct being lowered, and a description of the fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalFault {
    pub span: Span,
    pub what: String,
}

impl InternalFault {
    /// The diagnostic the user sees: the code, the span, and the words
    /// `bynk internal error` with where to report it.
    pub fn into_compile_error(self) -> CompileError {
        CompileError::new(
            MISSING_CHECKER_FACT,
            self.span,
            [
                "bynk internal error: the checker recorded no ",
                self.what.as_str(),
                ", which the code generator needs here. This is a compiler fault, not a problem \
                 in your program; please report it at ",
                REPORT_URL,
            ]
            .concat(),
        )
    }
}

/// Where a missing fact is reported: the module's sink and the `.bynk` span of
/// the construct being lowered. `Copy`, so a lowering helper that holds neither
/// the context nor the expression (the `Query` arms) carries one.
#[derive(Clone, Copy)]
pub(crate) struct FaultSite<'a> {
    pub(crate) sink: &'a super::RuntimeUse,
    pub(crate) span: Span,
}

impl FaultSite<'_> {
    /// A missing fact in a value position: records it and returns the
    /// `undefined` identifier ([`undefined_node`]).
    pub(crate) fn missing_fact(&self, what: impl std::fmt::Display) -> TsExpr {
        self.sink.note_internal_fault(self.span, what.to_string());
        undefined_node()
    }

    /// A missing fact in a type position: records it and returns
    /// [`fault_type`].
    pub(crate) fn missing_fact_ty(&self, what: impl std::fmt::Display) -> TsType {
        self.sink.note_internal_fault(self.span, what.to_string());
        fault_type()
    }
}

/// The `undefined` identifier: what a fault lowers to (its output is never
/// used) and what a legitimate default, an absent optional slot, renders as.
pub(crate) fn undefined_node() -> TsExpr {
    TsExpr::Ident("undefined".to_string())
}

/// The type placeholder a fault in a type position lowers to. `never`, not
/// `any`: the unit's output is discarded, and the placeholder must not look like
/// a fallback to the `emit_silent_fallbacks` probe.
pub(crate) fn fault_type() -> TsType {
    TsType::named("never")
}

/// The one deliberate widening in the lowering (#2795), and not a fallback.
///
/// A generic instance method's explicit type arguments are spelled receiver's
/// own parameters first, so a call with no argument of type `U` still types as
/// written. A receiver argument is spelled only when it is a base or built-in
/// type, which is in scope in every emitted module. Any other (a type the
/// calling module may never import, or a type variable that may not be this
/// function's) is `any`: the checker has already typed the call, so `any` in a
/// receiver slot loses nothing. Changing it to `unknown` or a named type is a
/// type-level output change that can break `tsc` inference.
pub(crate) fn unnameable_receiver_type_arg() -> TsType {
    super::lower_list::any_ty()
}

#[cfg(test)]
mod tests {
    //! Decision C (1A.34): one test per class of missing fact. Each takes one fact out of a
    //! checked program, then asserts the fault is recorded at the right span and that lowering
    //! continued (the whole unit lowered, so every fault of it is reported). The argument-arity
    //! class needs a context (an agent construction, a cross-context call), which the single-file
    //! pipeline here has not: it is held by `lower_call::agents` and `lower_call::cross_context`'s
    //! own tests. The driver's mapping to a diagnostic is held by
    //! `bynk_driver::project::tests`.
    use super::*;
    use crate::emitter::{BaseType, Expr, ExprKind, Ident, TypeBody};
    use bynk_check::checker::{Callee, TypedCommons, certify};
    use bynk_check::kernel_op::{KernelOp, OptionOp};

    const SOURCE: &str = "commons demo\n\ntype P = { n: Int = 1 }\n\nfn total(xs: List[Int]) -> Int { xs.fold(0, (a, b) => a + b) }\n\nfn make() -> P { P { } }\n\nfn P.size(self) -> Int { self.n }\n\nfn probe(p: P) -> Int { p.size() }\n";

    /// Check `SOURCE`, let `mutate` take a fact out, certify and emit. The faults, in order.
    fn faults_after(mutate: impl FnOnce(&mut TypedCommons)) -> (String, Vec<InternalFault>) {
        let (mut typed, warnings) =
            crate::check_single_typed(SOURCE, crate::SingleFileMode::Build).expect("checks");
        mutate(&mut typed);
        let program = certify(typed, warnings).expect("certifies");
        match crate::emitter::emit(&program) {
            Ok(ts) => (ts, Vec::new()),
            Err(faults) => (String::new(), faults),
        }
    }

    /// The span of `needle`'s first occurrence in [`SOURCE`].
    fn span_of(needle: &str) -> Span {
        let start = SOURCE.find(needle).expect("in the source");
        Span::new(start, start + needle.len())
    }

    /// The expression id of the call spelled `needle`.
    fn call_id(typed: &TypedCommons, needle: &str) -> bynk_check::checker::ExprId {
        let span = span_of(needle);
        *typed
            .expr_types
            .iter()
            .find(|(_, te)| te.span == span)
            .map(|(id, _)| id)
            .expect("the call is typed")
    }

    #[test]
    fn a_checked_program_reports_no_fault() {
        let (ts, faults) = faults_after(|_| {});
        assert!(faults.is_empty(), "{faults:?}");
        assert!(ts.contains("export function total"), "{ts}");
    }

    #[test]
    fn a_missing_type_is_recorded_at_the_expression_and_lowering_continues() {
        let (_, faults) = faults_after(|typed| {
            let fold = call_id(typed, "xs.fold(0, (a, b) => a + b)");
            typed.expr_types.remove(&fold);
        });
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert_eq!(faults[0].span, span_of("xs.fold(0, (a, b) => a + b)"));
        assert!(
            faults[0].what.contains("type for a `fold` call"),
            "{faults:?}"
        );
    }

    #[test]
    fn a_callee_that_disagrees_with_the_receiver_is_recorded_at_the_call() {
        let (_, faults) = faults_after(|typed| {
            let fold = call_id(typed, "xs.fold(0, (a, b) => a + b)");
            let int = typed
                .tys()
                .intern(bynk_check::checker::Ty::Base(BaseType::Int));
            // `Option.map` resolved against an `Int` receiver.
            typed.callees.insert(
                fold,
                Callee::Kernel {
                    op: KernelOp::Option(OptionOp::Map),
                    recv: int,
                },
            );
        });
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert_eq!(faults[0].span, span_of("xs.fold(0, (a, b) => a + b)"));
        assert!(
            faults[0]
                .what
                .contains("receiver type matching the resolved kernel op"),
            "{faults:?}"
        );
    }

    /// 1A.35a part C: an instance call whose receiver has no recorded type has no namespace to
    /// call into, which used to lower to a `/* unknown */` placeholder; it is a fault now.
    #[test]
    fn a_receiver_with_no_namespace_is_recorded_at_the_receiver() {
        let (_, faults) = faults_after(|typed| {
            let at = SOURCE.find("p.size()").unwrap();
            let receiver = typed
                .expr_types
                .iter()
                .find(|(_, te)| te.span == Span::new(at, at + 1))
                .map(|(id, _)| *id)
                .expect("the receiver is typed");
            typed.expr_types.remove(&receiver);
        });
        assert!(
            faults
                .iter()
                .any(|f| f.what.contains("receiver namespace for an instance call")),
            "{faults:?}"
        );
    }

    #[test]
    fn a_missing_field_default_is_recorded_at_the_construction() {
        let (_, faults) = faults_after(|typed| {
            // The checker required a closed-form default; take that away.
            let mut decl = (**typed.types.get("P").expect("P")).clone();
            if let TypeBody::Record(r) = &mut decl.body {
                let field = &mut r.fields[0];
                field.init = Some(Expr {
                    id: bynk_check::checker::ExprId::SYNTHETIC,
                    kind: ExprKind::Ident(Ident {
                        name: "missing".to_string(),
                        span: Span::new(0, 0),
                    }),
                    span: Span::new(0, 0),
                });
            }
            typed
                .types
                .insert("P".to_string(), std::sync::Arc::new(decl));
        });
        assert_eq!(faults.len(), 1, "{faults:?}");
        // The construction's type name, `P`, in `P { }`.
        let construction = SOURCE.find("P { }").unwrap();
        assert_eq!(faults[0].span, Span::new(construction, construction + 1));
        assert!(
            faults[0].what.contains("default value for field `n`"),
            "{faults:?}"
        );
    }

    #[test]
    fn every_fault_of_a_unit_is_reported_not_only_the_first() {
        let (_, faults) = faults_after(|typed| {
            let fold = call_id(typed, "xs.fold(0, (a, b) => a + b)");
            typed.expr_types.remove(&fold);
            let mut decl = (**typed.types.get("P").expect("P")).clone();
            if let TypeBody::Record(r) = &mut decl.body {
                r.fields[0].init = Some(Expr {
                    id: bynk_check::checker::ExprId::SYNTHETIC,
                    kind: ExprKind::Ident(Ident {
                        name: "missing".to_string(),
                        span: Span::new(0, 0),
                    }),
                    span: Span::new(0, 0),
                });
            }
            typed
                .types
                .insert("P".to_string(), std::sync::Arc::new(decl));
        });
        assert_eq!(faults.len(), 2, "{faults:?}");
    }

    #[test]
    fn a_fault_becomes_the_coded_diagnostic() {
        let fault = InternalFault {
            span: Span::new(4, 9),
            what: "type for a `fold` call".to_string(),
        };
        let error = fault.into_compile_error();
        assert_eq!(error.category, MISSING_CHECKER_FACT);
        assert_eq!(MISSING_CHECKER_FACT, "bynk.internal.missing_checker_fact");
        assert_eq!((error.span.start, error.span.end), (4, 9));
        assert!(
            error.message.starts_with("bynk internal error:"),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("compiler fault"),
            "{}",
            error.message
        );
        assert!(error.message.contains(REPORT_URL), "{}", error.message);
    }
}
