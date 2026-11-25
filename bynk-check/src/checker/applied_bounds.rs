//! #3958: a type's declared bounds are checked at every application.
//!
//! `type Sorted[T: Ord]` promises that every `Sorted` holds orderable items. A
//! written argument that is a concrete type is checked by the resolver
//! (`not_orderable`, `not_keyable`, `not_comparable`); this module checks the
//! other written case, a type *parameter* as the argument (`Sorted[U]`, in a
//! signature, a field, an annotation or a constant), which must carry the same
//! bounds. It reports with the codes and the machine-applicable fix the function
//! bound rules already use (`missing_{eq,ord,key,codable}_bound`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bynk_syntax::ast::{TypeDecl, TypeRef};
use bynk_syntax::error::{Applicability, CompileError};

use super::equality::EqScope;

/// The constraints a type parameter can be bound by, in the order they report.
const CONSTRAINTS: [&str; 4] = ["Eq", "Ord", "Key", "Codable"];

/// What `constraint` allows the bounded position to do, for the diagnostic's
/// note (the wording the function-level rules use).
fn why(constraint: &str) -> &'static str {
    match constraint {
        "Eq" => "a type parameter that is compared must say so",
        "Ord" => "a type parameter that is ordered must say so",
        "Key" => "a type parameter that keys a `Map` or `Set` must say so",
        _ => "a type parameter that crosses the boundary must say so",
    }
}

/// Every application of a bounded type in `r` whose argument is one of
/// `params` that lacks a bound the type declares, as `missing_*_bound` errors.
/// `scope` is where `params` are declared (a function's, or a type's own).
pub(crate) fn applied_bound_errors(
    r: &TypeRef,
    params: &HashSet<String>,
    scope: &EqScope,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Vec<CompileError> {
    let mut out = Vec::new();
    walk(r, params, scope, types, &mut out);
    out
}

fn walk(
    r: &TypeRef,
    params: &HashSet<String>,
    scope: &EqScope,
    types: &HashMap<String, Arc<TypeDecl>>,
    out: &mut Vec<CompileError>,
) {
    match r {
        TypeRef::App { name, args, .. } => {
            if let Some(decl) = types.get(&name.name) {
                for (tp, arg) in decl.type_params.iter().zip(args) {
                    let TypeRef::Named(id) = arg else { continue };
                    if !params.contains(&id.name) {
                        continue;
                    }
                    for constraint in CONSTRAINTS {
                        if !tp.bounds.iter().any(|b| b.name == constraint)
                            || scope.carries(&id.name, constraint)
                            || !scope.first_report(&id.name, constraint)
                        {
                            continue;
                        }
                        out.push(missing_bound(name, tp, &id.name, id.span, constraint, scope));
                    }
                }
            }
            for a in args {
                walk(a, params, scope, types, out);
            }
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            walk(a, params, scope, types, out);
            walk(b, params, scope, types, out);
        }
        TypeRef::Option(a, _)
        | TypeRef::Effect(a, _)
        | TypeRef::HttpResult(a, _)
        | TypeRef::List(a, _)
        | TypeRef::Set(a, _)
        | TypeRef::Query(a, _)
        | TypeRef::Stream(a, _)
        | TypeRef::Connection(a, _)
        | TypeRef::History(a, _) => walk(a, params, scope, types, out),
        TypeRef::Fn(ps, ret, _) => {
            for p in ps {
                walk(p, params, scope, types, out);
            }
            walk(ret, params, scope, types, out);
        }
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_) => {}
    }
}

fn missing_bound(
    applied: &bynk_syntax::ast::Ident,
    declared_on: &bynk_syntax::ast::TypeParam,
    param: &str,
    span: bynk_syntax::span::Span,
    constraint: &str,
    scope: &EqScope,
) -> CompileError {
    let code = match constraint {
        "Eq" => "bynk.generics.missing_eq_bound",
        "Ord" => "bynk.generics.missing_ord_bound",
        "Key" => "bynk.generics.missing_key_bound",
        _ => "bynk.generics.missing_codable_bound",
    };
    let mut err = CompileError::new(
        code,
        span,
        format!(
            "`{}` requires `{}: {constraint}`, but type parameter `{param}` has no `{constraint}` bound",
            applied.name, declared_on.name.name,
        ),
    )
    .with_note(format!("{}: declare `{param}: {constraint}`", why(constraint)));
    if let Some((msg, edits)) = scope.fix_for(param, constraint) {
        err = err.with_suggestion(msg, edits, Applicability::MachineApplicable);
    }
    err
}

/// #3958: an instantiation the checker formed itself (a record or variant
/// literal whose type arguments were inferred) is checked against the type's
/// declared bounds as a written argument would be: a concrete argument must
/// satisfy each bound, and a type parameter must carry it. Reports the codes
/// the function-bound rules use, and returns `false` after the first violation.
pub(crate) fn check_applied_ty_bounds(
    decl: &TypeDecl,
    args: &[super::TyId],
    span: bynk_syntax::span::Span,
    ctx: &mut super::Ctx,
) -> bool {
    use super::codable::{CodableSite, check_codable};
    use super::keying::{KeySite, check_key};
    use super::ordering::{OrdSite, check_ord};

    let callee = decl.name.name.as_str();
    for (tp, &arg) in decl.type_params.iter().zip(args) {
        let param = tp.name.name.as_str();
        let has = |c: &str| tp.bounds.iter().any(|b| b.name == c);
        if has("Ord") && !check_ord(ctx, arg, span, &OrdSite::Call { callee, param }) {
            return false;
        }
        if has("Key") && !check_key(ctx, arg, span, &KeySite::Call { callee, param }) {
            return false;
        }
        if has("Codable") && !check_codable(ctx, arg, span, &CodableSite::Call { callee, param }) {
            return false;
        }
        // `Ord` and `Key` imply `Eq`, and their checks above cover it.
        if has("Eq") && !has("Ord") && !has("Key") {
            let eq = [(param.to_string(), Some(tp.span))];
            let subst = HashMap::from([(param.to_string(), arg)]);
            if !super::equality::check_declared_eq_args(callee, span, &eq, &subst, ctx) {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use crate::testkit::analyse;

    fn body(src: &str) -> crate::testkit::Analysed {
        analyse(&[("app/a.bynk", src)])
    }

    /// The bound each of `Eq`, `Ord`, `Key` and `Codable` requires is checked
    /// where a bounded type is applied to a type parameter, in a signature.
    #[test]
    fn a_signature_applying_a_bounded_type_to_a_parameter_requires_the_bound() {
        for (bound, code) in [
            ("Eq", "bynk.generics.missing_eq_bound"),
            ("Ord", "bynk.generics.missing_ord_bound"),
            ("Key", "bynk.generics.missing_key_bound"),
            ("Codable", "bynk.generics.missing_codable_bound"),
        ] {
            let src = format!(
                "commons app.a\n\ntype Box[T: {bound}] = {{ items: List[T] }}\n\n\
                 fn wrap[U](xs: List[U]) -> Box[U] {{\n  Box {{ items: xs }}\n}}\n"
            );
            body(&src).assert_reports(code);
            let ok = format!(
                "commons app.a\n\ntype Box[T: {bound}] = {{ items: List[T] }}\n\n\
                 fn wrap[U: {bound}](xs: List[U]) -> Box[U] {{\n  Box {{ items: xs }}\n}}\n"
            );
            assert!(
                !body(&ok).categories().contains(&code),
                "`U: {bound}` satisfies the bound"
            );
        }
    }

    /// A field of a generic type applying a bounded type to the type's own
    /// parameter needs the bound declared there (Decision D: declared, not
    /// inferred from the field).
    #[test]
    fn a_field_applying_a_bounded_type_to_a_parameter_requires_the_bound() {
        let src = "commons app.a\n\ntype Index[K: Key, V] = { m: Map[K, V] }\n\n\
                   type Wrap[U] = { i: Index[U, Int] }\n";
        body(src).assert_reports("bynk.generics.missing_key_bound");
        let ok = "commons app.a\n\ntype Index[K: Key, V] = { m: Map[K, V] }\n\n\
                  type Wrap[U: Key] = { i: Index[U, Int] }\n";
        assert!(
            !body(ok)
                .categories()
                .contains(&"bynk.generics.missing_key_bound")
        );
    }

    /// An inferred instantiation, a record literal whose element type comes
    /// from its value, is checked as a written argument would be.
    #[test]
    fn an_inferred_instantiation_holds_the_declared_bound() {
        let src = "commons app.a\n\ntype Sorted[T: Ord] = { items: List[T] }\n\n\
                   fn direct() -> Int {\n  let s = Sorted { items: [true] }\n  s.items.length()\n}\n";
        body(src).assert_reports("bynk.types.not_orderable");
        let ok = "commons app.a\n\ntype Sorted[T: Ord] = { items: List[T] }\n\n\
                  fn direct() -> Int {\n  let s = Sorted { items: [1, 2] }\n  s.items.length()\n}\n";
        body(ok).assert_clean();
    }

    /// The `Key` and `Codable` bounds are checked on an inferred instantiation
    /// too, and a type parameter standing for the argument must carry them.
    #[test]
    fn an_inferred_instantiation_through_a_parameter_requires_the_bound() {
        let src = "commons app.a\n\ntype Index[K: Key] = { ks: List[K] }\n\n\
                   fn build[U](xs: List[U]) -> Int {\n  let i = Index { ks: xs }\n  i.ks.length()\n}\n";
        body(src).assert_reports("bynk.generics.missing_key_bound");
    }

    /// A bound a method's receiver type declares is already held inside the
    /// method (Decision C): no declaration is repeated.
    #[test]
    fn a_method_of_a_bounded_type_holds_its_receivers_bounds() {
        let src = "commons app.a\n\ntype Sorted[T: Ord] = { items: List[T] }\n\n\
                   fn Sorted.rewrap(self) -> Sorted[T] {\n  Sorted { items: self.items }\n}\n";
        let a = body(src);
        assert!(
            !a.categories().contains(&"bynk.generics.missing_ord_bound"),
            "{}",
            a.render()
        );
    }

    /// A static or generic call that makes the argument a type the bound
    /// refuses is reported at the call, as for any bounded parameter.
    #[test]
    fn a_call_through_a_bounded_type_checks_the_argument() {
        let src = "commons app.a\n\ntype Sorted[T: Ord] = { items: List[T] }\n\n\
                   fn mk[U: Ord](xs: List[U]) -> Sorted[U] {\n  Sorted { items: xs }\n}\n\n\
                   fn use() -> Int {\n  let s = mk([true])\n  s.items.length()\n}\n";
        body(src).assert_reports("bynk.types.not_orderable");
    }
}
