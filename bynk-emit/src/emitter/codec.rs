//! Codec witnesses for `Codable` (constrained-generics slice 3, #2715; the model
//! is DECISIONS B and C of the proposal and the track's Q4/Q7).
//!
//! Bynk generics are erased, so a `Codable`-bounded type parameter gets a hidden
//! trailing codec argument (a *witness*): `{ encode, decode }`, built at the call
//! site from the concrete type's generated `__serialise_X` / `__deserialise_X`
//! pair, so a generic call crosses the boundary exactly as a monomorphic one
//! does. This module builds that expression, in TypeScript, from a type:
//!
//! - a type with no type variable is the generated pair for it;
//! - a rigid type parameter is the enclosing function's own witness, forwarded;
//! - `List`, `Option`, `Set`, `Map` and `Result` over a type variable are the
//!   runtime combinators (`__bynkCodecList`, …) over the witnesses of their
//!   parts.
//!
//! - a user generic type over a type variable is its declaration's codec
//!   factory (`serialisation::emit_codec_factory`) applied to the witnesses of
//!   its arguments (#2792).

use std::collections::HashMap;

use bynk_check::checker::{Ty, TyId, TypedCommons};
use bynk_syntax::ast::TypeRef;
use bynk_ts::TsExpr;

use super::runtime_use::RuntimeUse;
use super::serialisation;

/// The hidden witness parameter's name for type parameter `param`.
pub(crate) fn witness_ident(param: &str) -> String {
    format!("__codec_{param}")
}

/// Whether `ty` mentions a type variable.
pub(crate) fn has_var(ty: TyId, commons: &TypedCommons) -> bool {
    let tys = commons.tys();
    match &*tys.get(ty) {
        Ty::Var(_) => true,
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => has_var(*a, commons),
        Ty::Result(a, b) | Ty::Map(a, b) => has_var(*a, commons) || has_var(*b, commons),
        Ty::Named { args, .. } => args.iter().any(|a| has_var(*a, commons)),
        _ => false,
    }
}

/// The variable-free parts of `ty` that a witness for it is built from: `ty`
/// itself when it has no type variable, else the ground parts of its containers.
/// These are the types whose generated codecs the module must emit.
pub(crate) fn ground_parts(ty: TyId, commons: &TypedCommons, out: &mut Vec<TyId>) {
    if !has_var(ty, commons) {
        out.push(ty);
        return;
    }
    let tys = commons.tys();
    match &*tys.get(ty) {
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => ground_parts(*a, commons, out),
        Ty::Result(a, b) | Ty::Map(a, b) => {
            ground_parts(*a, commons, out);
            ground_parts(*b, commons, out);
        }
        // #2792: a user generic type over a type variable: its arguments' ground
        // parts, beside the factory `generic_factories` names.
        Ty::Named { args, .. } => args.iter().for_each(|a| ground_parts(*a, commons, out)),
        _ => {}
    }
}

/// #2792: the user generic types applied to a type variable in `ty`, whose
/// codec is built by a factory (`serialisation::emit_codec_factory`).
pub(crate) fn generic_factories(ty: TyId, commons: &TypedCommons, out: &mut Vec<String>) {
    if !has_var(ty, commons) {
        return;
    }
    let tys = commons.tys();
    match &*tys.get(ty) {
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => generic_factories(*a, commons, out),
        Ty::Result(a, b) | Ty::Map(a, b) => {
            generic_factories(*a, commons, out);
            generic_factories(*b, commons, out);
        }
        Ty::Named { name, args, .. } => {
            out.push(name.clone());
            args.iter()
                .for_each(|a| generic_factories(*a, commons, out));
        }
        _ => {}
    }
}

/// Builds witness expressions. `scope` maps a rigid type parameter to the
/// identifier of its in-scope witness.
pub(crate) struct CodecGen<'a> {
    commons: &'a TypedCommons,
    runtime_use: &'a RuntimeUse,
    scope: &'a HashMap<String, String>,
}

impl<'a> CodecGen<'a> {
    pub(crate) fn new(
        commons: &'a TypedCommons,
        runtime_use: &'a RuntimeUse,
        scope: &'a HashMap<String, String>,
    ) -> Self {
        CodecGen {
            commons,
            runtime_use,
            scope,
        }
    }

    fn wrap(&self, helper: &'static str, parts: Vec<TsExpr>) -> TsExpr {
        self.runtime_use.note_cmp(helper);
        call(helper, parts)
    }

    /// The `{ encode, decode }` witness for `ty`, as a node.
    pub(crate) fn for_ty_node(&self, ty: TyId) -> TsExpr {
        let tys = self.commons.tys();
        if !has_var(ty, self.commons) {
            return self.ground(ty);
        }
        match &*tys.get(ty) {
            Ty::Var(name) => TsExpr::Ident(
                self.scope
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| witness_ident(name)),
            ),
            Ty::Option(a) => self.wrap("__bynkCodecOption", vec![self.for_ty_node(*a)]),
            Ty::List(a) => self.wrap("__bynkCodecList", vec![self.for_ty_node(*a)]),
            Ty::Set(a) => self.wrap("__bynkCodecSet", vec![self.for_ty_node(*a)]),
            Ty::Map(k, v) => self.wrap(
                "__bynkCodecMap",
                vec![self.for_ty_node(*k), self.for_ty_node(*v)],
            ),
            Ty::Result(a, b) => self.wrap(
                "__bynkCodecResult",
                vec![self.for_ty_node(*a), self.for_ty_node(*b)],
            ),
            // #2792: a user generic type over a type variable is its declaration's
            // factory applied to the witnesses of its arguments.
            Ty::Named { name, args, .. } => call(
                &serialisation::codec_factory_ident(name),
                args.iter().map(|a| self.for_ty_node(*a)).collect(),
            ),
            other => {
                unreachable!("bynk internal error: no codec over a type variable in {other:?}")
            }
        }
    }

    /// The generated pair for a variable-free type.
    fn ground(&self, ty: TyId) -> TsExpr {
        let tys = self.commons.tys();
        let tref: TypeRef = super::ty_to_type_ref(ty, tys)
            .unwrap_or_else(|| unreachable!("bynk internal error: no TypeRef for a codec target"));
        // #2773: the pair's signatures and inline base-type arms name `Result`,
        // `JsonValue` and `BoundaryError`, whether or not the type yields a
        // codec root; a module that curates its imports (a test module) needs
        // them brought in.
        self.runtime_use.note_json_codec();
        self.runtime_use.note_boundary_codec();
        serialisation::witness_codec_expr(&tref, self.runtime_use)
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

    const BASE: &str = "commons codecgen\n\
        fn enc[T: Codable](v: T) -> String { Json.encode(v) }\n\
        type Packed[T: Codable] = { json: String }\n\
        type Rec = { a: Int }\n";

    fn emitted(body: &str) -> String {
        emit_source(&format!("{BASE}{body}"))
    }

    /// The last argument of the `enc(v, …)` call in `return enc(v, …);`.
    fn witness_of(ts: &str, f: &str) -> String {
        let at = [
            format!("export function {f}<"),
            format!("export function {f}("),
        ]
        .iter()
        .find_map(|h| ts.find(h))
        .expect("fn emitted");
        let rest = &ts[at..];
        let from = rest.find("return enc(v, ").expect("a call") + "return enc(v, ".len();
        let end = rest[from..].find(");\n").expect("a terminator");
        rest[from..from + end].to_string()
    }

    fn imports(ts: &str, helper: &str) -> bool {
        ts.lines()
            .any(|l| l.starts_with("import") && l.contains(helper))
    }

    #[test]
    fn a_scoped_variable_and_each_wrapper_build_from_witnesses() {
        let ts = emitted(
            "fn a[T: Codable](v: T) -> String { enc(v) }\n\
             fn b[T: Codable](v: List[T]) -> String { enc(v) }\n\
             fn c[T: Codable](v: Option[T]) -> String { enc(v) }\n\
             fn d[T: Codable](v: Map[String, T]) -> String { enc(v) }\n\
             fn e[T: Codable](v: Result[T, String]) -> String { enc(v) }\n",
        );
        assert_eq!(witness_of(&ts, "a"), "__codec_T");
        assert_eq!(witness_of(&ts, "b"), "__bynkCodecList(__codec_T)");
        assert_eq!(witness_of(&ts, "c"), "__bynkCodecOption(__codec_T)");
        assert!(witness_of(&ts, "d").starts_with("__bynkCodecMap({ encode: "));
        assert!(witness_of(&ts, "d").ends_with(", __codec_T)"));
        assert!(witness_of(&ts, "e").starts_with("__bynkCodecResult(__codec_T, { encode: "));
        for h in [
            "__bynkCodecList",
            "__bynkCodecOption",
            "__bynkCodecMap",
            "__bynkCodecResult",
        ] {
            assert!(imports(&ts, h), "{h} not imported");
        }
    }

    #[test]
    fn a_generic_type_is_its_codec_factory_over_the_argument_witnesses() {
        let ts = emitted("fn f[T: Codable](v: Packed[T]) -> String { enc(v) }\n");
        assert_eq!(witness_of(&ts, "f"), "__codecOf_Packed(__codec_T)");
    }

    #[test]
    fn a_ground_type_is_the_generated_pair_node() {
        let ts = emitted(
            "fn base(v: Int) -> String { enc(v) }\n\
             fn rec(v: Rec) -> String { enc(v) }\n",
        );
        assert_eq!(
            witness_of(&ts, "rec"),
            "{ encode: __serialise_Rec, decode: __deserialise_Rec }"
        );
        assert!(witness_of(&ts, "base").starts_with("{ encode: (__v: number) => "));
        // The runtime import the boundary codec adds (`note_boundary_codec`).
        assert!(
            ts.lines().any(
                |l| l.starts_with("import type { __JsonValue, __BoundaryError }")
                    && l.contains("__bynk_runtime/boundary.js")
            ),
            "{ts}"
        );
    }
}
