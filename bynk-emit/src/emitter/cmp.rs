//! Comparator witnesses for `Ord` (constrained-generics slice 1, #2660; the
//! model is DECISION D of the proposal and the track's Q4/Q5).
//!
//! Bynk generics are erased, so an `Ord`-bounded type parameter gets a hidden
//! trailing comparator argument (a *witness*) built at the call site from the
//! concrete type. This module builds that expression, in TypeScript, from a type:
//!
//! - a base type is a runtime comparator (`__bynkCmpInt`, `__bynkCmpFloat`, …);
//! - `Option[T]` and `List[T]` wrap the comparator of `T`;
//! - a rigid type parameter is the enclosing function's own witness, forwarded;
//! - a refined or opaque type compares as its representation;
//! - a record or sum that `derives Ord` is built from its declaration with the
//!   runtime combinators (`__bynkCmpRecord`, `__bynkCmpSum`), the type arguments
//!   substituted for the type's own parameters.
//!
//! A comparator for a user type is built where it is used, from the declaration,
//! rather than emitted once beside the type. That keeps witnesses working across
//! units (an imported type's declaration is in the checker's table) without
//! exporting, importing and re-branding a comparator per type. A recursive type
//! names itself through `__bynkCmpFix`.
//!
//! Every helper used is noted on the module's [`RuntimeUse`], which decides the
//! runtime import list.

use std::collections::HashMap;

use bynk_check::checker::{NamedKind, Ty, TyId, TypedCommons};
use bynk_syntax::ast::{BaseType, TypeBody, TypeDecl, TypeRef};
use bynk_ts::{TsArrowBody, TsExpr, TsLit, TsObjectEntry, TsParam, print_expr};

use super::runtime_use::RuntimeUse;

/// The hidden witness parameter's name for type parameter `param`.
pub(crate) fn witness_ident(param: &str) -> String {
    format!("__cmp_{param}")
}

/// A comparator being built for a user type, so a reference back to the same
/// instantiation becomes a self reference.
struct Frame {
    name: String,
    /// The printed form of each argument witness, used only to compare two
    /// instantiations for recursion (`TsExpr` has no `PartialEq`). Nothing
    /// printed for a key reaches the output (an internal use of printing, not a
    /// text exit; track 1A.16, Decision C).
    args: Vec<String>,
    self_ident: String,
    used: bool,
}

/// Builds comparator expressions. `scope` maps a rigid type parameter to the
/// identifier of its in-scope witness.
pub(crate) struct CmpGen<'a> {
    commons: &'a TypedCommons,
    runtime_use: &'a RuntimeUse,
    scope: &'a HashMap<String, String>,
}

impl<'a> CmpGen<'a> {
    pub(crate) fn new(
        commons: &'a TypedCommons,
        runtime_use: &'a RuntimeUse,
        scope: &'a HashMap<String, String>,
    ) -> Self {
        CmpGen {
            commons,
            runtime_use,
            scope,
        }
    }

    /// The comparator expression for `ty`, as a node.
    pub(crate) fn for_ty_node(&self, ty: TyId) -> TsExpr {
        self.ty(ty, &mut Vec::new())
    }

    /// The comparator for a base type, noting the runtime helper it uses.
    fn for_base(&self, b: BaseType) -> TsExpr {
        let name = match b {
            BaseType::Int | BaseType::Duration | BaseType::Instant => "__bynkCmpInt",
            BaseType::String => "__bynkCmpString",
            BaseType::Float => "__bynkCmpFloat",
            BaseType::Bytes => "__bynkCmpBytes",
            BaseType::Decimal => "__bynkCmpDecimal",
            // The checker rejects ordering a `Bool`; a comparator for one is
            // never requested.
            BaseType::Bool => unreachable!("bynk internal error: Bool is not Ord"),
        };
        self.runtime_use.note_cmp(name);
        TsExpr::Ident(name.to_string())
    }

    fn wrap(&self, helper: &'static str, inner: TsExpr) -> TsExpr {
        self.runtime_use.note_cmp(helper);
        call(helper, vec![inner])
    }

    fn ty(&self, ty: TyId, stack: &mut Vec<Frame>) -> TsExpr {
        let tys = self.commons.tys();
        match &*tys.get(ty) {
            Ty::Base(b) => self.for_base(*b),
            Ty::Option(t) => {
                let inner = self.ty(*t, stack);
                self.wrap("__bynkCmpOption", inner)
            }
            Ty::List(t) => {
                let inner = self.ty(*t, stack);
                self.wrap("__bynkCmpList", inner)
            }
            Ty::Var(name) => TsExpr::Ident(
                self.scope
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| witness_ident(name)),
            ),
            Ty::Named {
                name, kind, args, ..
            } => match kind {
                NamedKind::Refined(b) | NamedKind::Opaque(b)
                    if self.derive_source(name).is_none() =>
                {
                    self.for_base(*b)
                }
                _ => {
                    let arg_ws: Vec<TsExpr> = args.iter().map(|a| self.ty(*a, stack)).collect();
                    self.derive(name, arg_ws, stack)
                }
            },
            other => unreachable!("bynk internal error: {other:?} is not Ord"),
        }
    }

    /// The declaration to derive from: a record or sum (an opaque type compares
    /// as its representation).
    fn derive_source(&self, name: &str) -> Option<&'a TypeDecl> {
        let decl = self.commons.types.get(name)?;
        matches!(decl.body, TypeBody::Record(_) | TypeBody::Sum(_)).then_some(decl.as_ref())
    }

    /// A record or sum's comparator, with `args` the comparators of its type
    /// arguments, in parameter order.
    fn derive(&self, name: &str, args: Vec<TsExpr>, stack: &mut Vec<Frame>) -> TsExpr {
        let key: Vec<String> = args.iter().map(print_expr).collect();
        if let Some(frame) = stack.iter_mut().find(|f| f.name == name && f.args == key) {
            frame.used = true;
            return TsExpr::Ident(frame.self_ident.clone());
        }
        let Some(decl) = self.derive_source(name) else {
            // A refined or opaque type reached by name: its representation.
            return match self.commons.types.get(name).map(|d| &d.body) {
                Some(TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. }) => {
                    self.for_base(*base)
                }
                _ => unreachable!("bynk internal error: no declaration for `{name}`"),
            };
        };
        let env: HashMap<String, TsExpr> = decl
            .type_params
            .iter()
            .map(|p| p.name.name.clone())
            .zip(args.iter().cloned())
            .collect();
        // The checker rejects a recursive reference that changes the type's
        // arguments (`derives_field_not_orderable`), which is the only way the
        // expansion could fail to stop.
        assert!(
            stack.len() < 64,
            "bynk internal error: comparator for `{name}` does not terminate"
        );
        let self_ident = format!("__self{}", stack.len());
        stack.push(Frame {
            name: name.to_string(),
            args: key,
            self_ident: self_ident.clone(),
            used: false,
        });
        let body = match &decl.body {
            TypeBody::Record(r) => {
                let fields: Vec<(String, &TypeRef)> = r
                    .fields
                    .iter()
                    .map(|f| (f.name.name.clone(), &f.type_ref))
                    .collect();
                self.runtime_use.note_cmp("__bynkCmpRecord");
                call("__bynkCmpRecord", vec![self.fields(&fields, &env, stack)])
            }
            TypeBody::Sum(s) => {
                let order: Vec<TsExpr> = s
                    .variants
                    .iter()
                    .map(|v| TsExpr::Lit(TsLit::Str(v.name.name.clone())))
                    .collect();
                let mut payloads = Vec::new();
                for v in &s.variants {
                    if v.payload.is_empty() {
                        continue;
                    }
                    let fields: Vec<(String, &TypeRef)> = v
                        .payload
                        .iter()
                        .map(|p| (p.name.name.clone(), &p.type_ref))
                        .collect();
                    // The key prints verbatim, so it carries its own quotes.
                    payloads.push(TsObjectEntry::Prop(
                        format!("\"{}\"", v.name.name),
                        self.fields(&fields, &env, stack),
                    ));
                }
                self.runtime_use.note_cmp("__bynkCmpSum");
                call(
                    "__bynkCmpSum",
                    vec![TsExpr::array(order), TsExpr::object_entries(payloads)],
                )
            }
            _ => unreachable!("derive_source returns only records and sums"),
        };
        let frame = stack.pop().expect("the frame pushed above");
        if frame.used {
            self.runtime_use.note_cmp("__bynkCmpFix");
            call(
                "__bynkCmpFix",
                vec![TsExpr::Arrow {
                    params: vec![TsParam::new(self_ident, None)],
                    is_async: false,
                    generics: Vec::new(),
                    return_type: None,
                    body: Box::new(TsArrowBody::Expr(Box::new(body))),
                }],
            )
        } else {
            body
        }
    }

    /// `[["name", cmp], …]` for a record's fields or one variant's payload.
    fn fields(
        &self,
        fields: &[(String, &TypeRef)],
        env: &HashMap<String, TsExpr>,
        stack: &mut Vec<Frame>,
    ) -> TsExpr {
        let parts: Vec<TsExpr> = fields
            .iter()
            .map(|(name, t)| {
                TsExpr::array(vec![
                    TsExpr::Lit(TsLit::Str(name.clone())),
                    self.type_ref(t, env, stack),
                ])
            })
            .collect();
        TsExpr::array(parts)
    }

    /// The comparator for a declared field type, with the declaring type's own
    /// parameters bound by `env`.
    fn type_ref(
        &self,
        t: &TypeRef,
        env: &HashMap<String, TsExpr>,
        stack: &mut Vec<Frame>,
    ) -> TsExpr {
        match t {
            TypeRef::Base(b, _) => self.for_base(*b),
            TypeRef::Option(a, _) => {
                let inner = self.type_ref(a, env, stack);
                self.wrap("__bynkCmpOption", inner)
            }
            TypeRef::List(a, _) => {
                let inner = self.type_ref(a, env, stack);
                self.wrap("__bynkCmpList", inner)
            }
            TypeRef::Named(id) => match env.get(&id.name) {
                Some(w) => w.clone(),
                None => self.derive(&id.name, Vec::new(), stack),
            },
            TypeRef::App { name, args, .. } => {
                let ws: Vec<TsExpr> = args.iter().map(|a| self.type_ref(a, env, stack)).collect();
                self.derive(&name.name, ws, stack)
            }
            other => unreachable!("bynk internal error: {other:?} is not Ord"),
        }
    }
}

/// `helper(args…)`.
fn call(helper: &str, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::Call {
        callee: Box::new(TsExpr::Ident(helper.to_string())),
        args,
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::emit_source;

    /// The comparator printed for `x < y` over `ty`, declared among `decls`.
    fn cmp_of(decls: &str, ty: &str) -> String {
        let src = format!("commons cmpgen\n{decls}\nfn lt(x: {ty}, y: {ty}) -> Bool {{ x < y }}\n");
        let ts = emit_source(&src);
        let from = ts.find("export function lt(").expect("lt emitted");
        let rest = &ts[from..];
        let ret = rest.find("return ").expect("a return") + "return ".len();
        let end = rest[ret..].find("(x, y) < 0;").expect("a comparison");
        rest[ret..ret + end].to_string()
    }

    /// Whether the module's runtime import list names `helper`.
    fn imports(decls: &str, ty: &str, helper: &str) -> bool {
        let src = format!("commons cmpgen\n{decls}\nfn lt(x: {ty}, y: {ty}) -> Bool {{ x < y }}\n");
        emit_source(&src)
            .lines()
            .any(|l| l.starts_with("import") && l.contains(helper))
    }

    #[test]
    fn base_option_and_list_comparators() {
        assert_eq!(
            cmp_of("", "List[Option[String]]"),
            "__bynkCmpList(__bynkCmpOption(__bynkCmpString))"
        );
        assert!(imports("", "List[Option[String]]", "__bynkCmpOption"));
        assert!(imports("", "List[Option[String]]", "__bynkCmpList"));
        assert!(imports("", "List[Option[String]]", "__bynkCmpString"));
        assert_eq!(cmp_of("", "List[Float]"), "__bynkCmpList(__bynkCmpFloat)");
        assert_eq!(
            cmp_of("", "List[Decimal]"),
            "__bynkCmpList(__bynkCmpDecimal)"
        );
        assert_eq!(cmp_of("", "List[Bytes]"), "__bynkCmpList(__bynkCmpBytes)");
        assert_eq!(cmp_of("", "List[Duration]"), "__bynkCmpList(__bynkCmpInt)");
    }

    #[test]
    fn a_derived_record_is_an_array_of_name_and_comparator_pairs() {
        let d = "type P = { a: Int, b: Option[Float] } derives Ord";
        assert_eq!(
            cmp_of(d, "P"),
            r#"__bynkCmpRecord([["a", __bynkCmpInt], ["b", __bynkCmpOption(__bynkCmpFloat)]])"#
        );
        assert!(imports(d, "P", "__bynkCmpRecord"));
    }

    #[test]
    fn a_derived_sum_keys_payloads_by_quoted_variant_name() {
        let d = "type S = | A | B(n: Int, s: String) | C derives Ord";
        assert_eq!(
            cmp_of(d, "S"),
            r#"__bynkCmpSum(["A", "B", "C"], { "B": [["n", __bynkCmpInt], ["s", __bynkCmpString]] })"#
        );
        assert!(imports(d, "S", "__bynkCmpSum"));
        assert_eq!(
            cmp_of("type S = | A | C derives Ord", "S"),
            r#"__bynkCmpSum(["A", "C"], {})"#
        );
    }

    #[test]
    fn a_generic_derived_type_substitutes_its_arguments() {
        let d = "type Pair[T] = { l: T, r: T } derives Ord";
        assert_eq!(
            cmp_of(d, "Pair[Int]"),
            r#"__bynkCmpRecord([["l", __bynkCmpInt], ["r", __bynkCmpInt]])"#
        );
        assert_eq!(
            cmp_of(d, "Pair[Option[String]]"),
            r#"__bynkCmpRecord([["l", __bynkCmpOption(__bynkCmpString)], ["r", __bynkCmpOption(__bynkCmpString)]])"#
        );
    }

    #[test]
    fn a_scoped_type_variable_is_its_witness_parameter() {
        let src = "commons cmpgen\n\
            fn big[T: Ord](x: List[T], y: List[T]) -> Bool { x < y }\n";
        let ts = emit_source(src);
        assert!(ts.contains("__bynkCmpList(__cmp_T)(x, y) < 0"), "{ts}");
    }

    #[test]
    fn a_self_recursive_type_names_itself_through_fix() {
        let d = "type L = | Nil | Cons(h: Int, t: L) derives Ord";
        assert_eq!(
            cmp_of(d, "L"),
            r#"__bynkCmpFix((__self0) => __bynkCmpSum(["Nil", "Cons"], { "Cons": [["h", __bynkCmpInt], ["t", __self0]] }))"#
        );
        assert!(imports(d, "L", "__bynkCmpFix"));
    }

    #[test]
    fn mutually_recursive_types_fix_the_outer_type() {
        let d = "type A = | AEnd | AMore(b: B) derives Ord\n\
                 type B = | BEnd | BMore(a: A) derives Ord";
        // Only the outer type is referred back to, so only it needs a fix.
        assert_eq!(
            cmp_of(d, "A"),
            r#"__bynkCmpFix((__self0) => __bynkCmpSum(["AEnd", "AMore"], { "AMore": [["b", __bynkCmpSum(["BEnd", "BMore"], { "BMore": [["a", __self0]] })]] }))"#
        );
    }

    /// Decision B: a variant or field name is a Bynk identifier, so Rust's
    /// `{:?}` (what the text builder used) and the printer's string escaping
    /// produce the same quoted text for every name an identifier can be.
    #[test]
    fn identifier_names_print_the_same_through_debug_and_the_printer() {
        for name in ["a", "A", "Variant1", "snake_case", "__x", "z9", "é", "日本"] {
            let node = bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(name.to_string()));
            assert_eq!(bynk_ts::print_expr(&node), format!("{name:?}"), "{name}");
        }
    }
}
