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

use super::runtime_use::RuntimeUse;

/// The hidden witness parameter's name for type parameter `param`.
pub(crate) fn witness_ident(param: &str) -> String {
    format!("__cmp_{param}")
}

/// A comparator being built for a user type, so a reference back to the same
/// instantiation becomes a self reference.
struct Frame {
    name: String,
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

    /// The comparator expression for `ty`.
    pub(crate) fn for_ty(&self, ty: TyId) -> String {
        self.ty(ty, &mut Vec::new())
    }

    /// The comparator for a base type, noting the runtime helper it uses.
    pub(crate) fn for_base(&self, b: BaseType) -> String {
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
        name.to_string()
    }

    fn wrap(&self, helper: &'static str, inner: String) -> String {
        self.runtime_use.note_cmp(helper);
        format!("{helper}({inner})")
    }

    fn ty(&self, ty: TyId, stack: &mut Vec<Frame>) -> String {
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
            Ty::Var(name) => self
                .scope
                .get(name)
                .cloned()
                .unwrap_or_else(|| witness_ident(name)),
            Ty::Named {
                name, kind, args, ..
            } => match kind {
                NamedKind::Refined(b) | NamedKind::Opaque(b)
                    if self.derive_source(name).is_none() =>
                {
                    self.for_base(*b)
                }
                _ => {
                    let arg_ws: Vec<String> = args.iter().map(|a| self.ty(*a, stack)).collect();
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
    fn derive(&self, name: &str, args: Vec<String>, stack: &mut Vec<Frame>) -> String {
        if let Some(frame) = stack.iter_mut().find(|f| f.name == name && f.args == args) {
            frame.used = true;
            return frame.self_ident.clone();
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
        let env: HashMap<String, String> = decl
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
            args,
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
                format!("__bynkCmpRecord({})", self.fields(&fields, &env, stack))
            }
            TypeBody::Sum(s) => {
                let order: Vec<String> = s
                    .variants
                    .iter()
                    .map(|v| format!("{:?}", v.name.name))
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
                    payloads.push(format!(
                        "{:?}: {}",
                        v.name.name,
                        self.fields(&fields, &env, stack)
                    ));
                }
                self.runtime_use.note_cmp("__bynkCmpSum");
                format!(
                    "__bynkCmpSum([{}], {{{}}})",
                    order.join(", "),
                    payloads.join(", ")
                )
            }
            _ => unreachable!("derive_source returns only records and sums"),
        };
        let frame = stack.pop().expect("the frame pushed above");
        if frame.used {
            self.runtime_use.note_cmp("__bynkCmpFix");
            format!("__bynkCmpFix(({self_ident}) => {body})")
        } else {
            body
        }
    }

    /// `[["name", cmp], …]` for a record's fields or one variant's payload.
    fn fields(
        &self,
        fields: &[(String, &TypeRef)],
        env: &HashMap<String, String>,
        stack: &mut Vec<Frame>,
    ) -> String {
        let parts: Vec<String> = fields
            .iter()
            .map(|(name, t)| format!("[{name:?}, {}]", self.type_ref(t, env, stack)))
            .collect();
        format!("[{}]", parts.join(", "))
    }

    /// The comparator for a declared field type, with the declaring type's own
    /// parameters bound by `env`.
    fn type_ref(
        &self,
        t: &TypeRef,
        env: &HashMap<String, String>,
        stack: &mut Vec<Frame>,
    ) -> String {
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
                let ws: Vec<String> = args.iter().map(|a| self.type_ref(a, env, stack)).collect();
                self.derive(&name.name, ws, stack)
            }
            other => unreachable!("bynk internal error: {other:?} is not Ord"),
        }
    }
}
