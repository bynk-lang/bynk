//! v0.30.2 (ADR 0063): the enumerable kernel-method registry.
//!
//! The value methods of the built-in kernels (`List`/`Map`/`Option`/`Result`/
//! `String`/`Int`/`Float`) are dispatched by the checker in
//! [`crate::checker`]'s `check_*_kernel_method` functions, where the method
//! names live in `match` arms — authoritative for *typing*, but not
//! enumerable. This module is the enumerable view the LSP reads for `.`-member
//! completion: per-kernel `(name, signature)` tables and a [`methods_for`]
//! mapping from a receiver [`Ty`] to its methods.
//!
//! The tables are generated from the typed operations in [`crate::kernel_op`]
//! (#3164), where each method is declared once with its spelling and signature;
//! the checker resolves a call to that enum and carries it in
//! [`crate::checker::Callee::Kernel`].
//!
//! The signatures are human-readable Bynk-surface display strings (generic in
//! the element/key/value type), for completion `detail` — not the checker's
//! `Ty`-typed signatures. The checker matches on the typed operations
//! exhaustively, so the table can neither list a method the checker rejects nor
//! omit one it accepts.

use crate::checker::{NamedKind, Ty, TyId, Types};
use bynk_syntax::ast::BaseType;

pub use crate::kernel_op::KernelMethod;
use crate::kernel_op::{
    BytesOp, DecimalOp, DurationOp, EffectOp, EffectResultOp, FloatOp, HttpResultOp, InstantOp,
    IntOp, ListOp, MapOp, OptionOp, QueryOp, RequestHeadersOp, RequestQueryOp, ResultOp, SetOp,
    StringOp, TestConnectionOp, TokenClaimsOp,
};

/// `List[T]` (v0.20b; v0.88 adds the ADR 0116 query/collection vocabulary).
pub const LIST_METHODS: &[KernelMethod] = ListOp::TABLE;
/// The `Set` kernel (K6a, #2596): what Bynk cannot write over a list. The
/// combinators (`union`, `intersect`, …) are the `bynk.set` commons.
pub const SET_METHODS: &[KernelMethod] = SetOp::TABLE;
/// `Map[K, V]` (v0.20b).
pub const MAP_METHODS: &[KernelMethod] = MapOp::TABLE;
/// `Query[T]` (v0.91, ADR 0115; joins/grouping v0.94, ADR 0116/0120): the lazy
/// storage-query vocabulary a `store` collection lifts into.
pub const QUERY_METHODS: &[KernelMethod] = QueryOp::TABLE;
/// `Option[T]` combinators (v0.22a).
pub const OPTION_METHODS: &[KernelMethod] = OptionOp::TABLE;
/// `Result[T, E]` combinators (v0.22a).
pub const RESULT_METHODS: &[KernelMethod] = ResultOp::TABLE;
/// The methods of every `Effect[T]` (#2683, #2711): `within` and `race`.
pub const EFFECT_METHODS: &[KernelMethod] = EffectOp::TABLE;
/// The `Effect[Result[T, E]]` combinators (v0.152, ADR 0176): `within` and
/// `race` (on every `Effect`) alongside the compiler-synthesised combinators.
pub const EFFECT_RESULT_METHODS: &[KernelMethod] = &[
    EffectOp::TABLE[0],
    EffectOp::TABLE[1],
    EffectResultOp::TABLE[0],
    EffectResultOp::TABLE[1],
    EffectResultOp::TABLE[2],
    EffectResultOp::TABLE[3],
];
/// The `String` kernel (v0.22a; UTF-16 code units, except `chars`).
pub const STRING_METHODS: &[KernelMethod] = StringOp::TABLE;
/// The `Int` kernel.
pub const INT_METHODS: &[KernelMethod] = IntOp::TABLE;
/// The `Float` kernel.
pub const FLOAT_METHODS: &[KernelMethod] = FloatOp::TABLE;
/// The `Duration` kernel (v0.86, ADR 0112).
pub const DURATION_METHODS: &[KernelMethod] = DurationOp::TABLE;
/// The `Instant` kernel (v0.90, ADR 0114).
pub const INSTANT_METHODS: &[KernelMethod] = InstantOp::TABLE;
/// The `Bytes` kernel (v0.110, ADR 0142).
pub const BYTES_METHODS: &[KernelMethod] = BytesOp::TABLE;
/// The `Decimal` kernel (#2622).
pub const DECIMAL_METHODS: &[KernelMethod] = DecimalOp::TABLE;
/// An HTTP handler's `headers` (#2341).
pub const REQUEST_HEADERS_METHODS: &[KernelMethod] = RequestHeadersOp::TABLE;
/// `HttpResult[T]` (#3277): `withCookie`, `clearCookie`, `cookies`.
pub const HTTP_RESULT_METHODS: &[KernelMethod] = HttpResultOp::TABLE;
/// An HTTP handler's `query` (#2854).
pub const REQUEST_QUERY_METHODS: &[KernelMethod] = RequestQueryOp::TABLE;
/// An `Oidc` binder's claims (#2380).
pub const TOKEN_CLAIMS_METHODS: &[KernelMethod] = TokenClaimsOp::TABLE;
/// A case's recording `TestConnection[F]` (#3051).
pub const TEST_CONNECTION_METHODS: &[KernelMethod] = TestConnectionOp::TABLE;

/// Every method table above, by the type a user writes. The one enumerable
/// registry (read by the wasm `bynk_reference`); a test pins it against the
/// `pub const *_METHODS` declarations, so a new table cannot be left out.
pub const ALL_METHODS: &[(&str, &[KernelMethod])] = &[
    ("Int", INT_METHODS),
    ("Float", FLOAT_METHODS),
    ("Decimal", DECIMAL_METHODS),
    ("String", STRING_METHODS),
    ("Bytes", BYTES_METHODS),
    ("Duration", DURATION_METHODS),
    ("Instant", INSTANT_METHODS),
    ("List", LIST_METHODS),
    ("Map", MAP_METHODS),
    ("Set", SET_METHODS),
    ("Query", QUERY_METHODS),
    ("Option", OPTION_METHODS),
    ("Result", RESULT_METHODS),
    ("Effect", EFFECT_METHODS),
    ("Effect[Result[T, E]]", EFFECT_RESULT_METHODS),
    ("RequestHeaders", REQUEST_HEADERS_METHODS),
    ("HttpResult", HTTP_RESULT_METHODS),
    ("RequestQuery", REQUEST_QUERY_METHODS),
    ("TokenClaims", TOKEN_CLAIMS_METHODS),
    ("TestConnection", TEST_CONNECTION_METHODS),
];

/// The value methods of a receiver type, or `&[]` for a type with no kernel
/// methods (record/sum named types, `Bool`, `Effect`, …). Record *fields* are
/// resolved separately by the LSP (they need the type declaration).
pub fn methods_for(ty: TyId, tys: &Types) -> &'static [KernelMethod] {
    match &*tys.get(ty) {
        Ty::Base(BaseType::Int) => INT_METHODS,
        Ty::Base(BaseType::Float) => FLOAT_METHODS,
        Ty::Base(BaseType::Duration) => DURATION_METHODS,
        Ty::Base(BaseType::Instant) => INSTANT_METHODS,
        Ty::Base(BaseType::Bytes) => BYTES_METHODS,
        Ty::Base(BaseType::Decimal) => DECIMAL_METHODS,
        Ty::Base(BaseType::String) => STRING_METHODS,
        Ty::RequestHeaders => REQUEST_HEADERS_METHODS,
        Ty::HttpResult(_) => HTTP_RESULT_METHODS,
        Ty::RequestQuery => REQUEST_QUERY_METHODS,
        Ty::TokenClaims => TOKEN_CLAIMS_METHODS,
        Ty::TestConnection(_) => TEST_CONNECTION_METHODS,
        Ty::List(_) => LIST_METHODS,
        Ty::Map(_, _) => MAP_METHODS,
        Ty::Set(_) => SET_METHODS,
        Ty::Query(_) => QUERY_METHODS,
        Ty::Option(_) => OPTION_METHODS,
        Ty::Result(_, _) => RESULT_METHODS,
        // §2.8.3: an `Effect[Result[T, E]]` receiver offers the cross-context
        // combinators (and `within`, `race`); any other `Effect[_]` offers those two alone.
        Ty::Effect(inner) if matches!(&*tys.get(*inner), Ty::Result(_, _)) => EFFECT_RESULT_METHODS,
        Ty::Effect(_) => EFFECT_METHODS,
        // #561: a refined receiver inherits its base type's read-only kernel
        // methods (after its own declared methods, which the completion layer
        // merges in), so `.`-member completion offers them. `Bool` has no
        // kernel; opaque types deliberately do not widen, so both surface none.
        Ty::Named {
            kind: NamedKind::Refined(base),
            ..
        } => match base {
            BaseType::Int => INT_METHODS,
            BaseType::Float => FLOAT_METHODS,
            BaseType::Duration => DURATION_METHODS,
            BaseType::Instant => INSTANT_METHODS,
            BaseType::Bytes => BYTES_METHODS,
            BaseType::Decimal => DECIMAL_METHODS,
            BaseType::String => STRING_METHODS,
            BaseType::Bool => &[],
        },
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_methods_lists_every_method_table() {
        let declared = include_str!("kernel_methods.rs")
            .lines()
            .filter(|l| l.starts_with("pub const ") && l.contains("_METHODS: &[KernelMethod]"))
            .count();
        assert_eq!(
            ALL_METHODS.len(),
            declared,
            "a *_METHODS table is missing from ALL_METHODS"
        );
    }

    fn refined(tys: &Types, base: BaseType) -> TyId {
        tys.intern(Ty::Named {
            name: "R".to_string(),
            kind: NamedKind::Refined(base),
            args: Vec::new(),
        })
    }

    #[test]
    fn decimal_receiver_offers_its_kernel_and_a_refined_one_inherits_it() {
        let tys = &Types::new();
        // #2622: bare and refined `Decimal` receivers both surface the kernel.
        let names = |ms: &[KernelMethod]| ms.iter().map(|m| m.name).collect::<Vec<_>>();
        let bare = tys.intern(Ty::Base(BaseType::Decimal));
        for n in ["scale", "toString", "round", "div", "toInt"] {
            assert!(names(methods_for(bare, tys)).contains(&n), "{n}");
        }
        assert_eq!(
            methods_for(refined(tys, BaseType::Decimal), tys).len(),
            DECIMAL_METHODS.len()
        );
    }

    #[test]
    fn refined_receiver_inherits_base_kernel_methods() {
        let tys = &Types::new();
        // #561: completion on a refined receiver offers the base kernel.
        assert_eq!(
            methods_for(refined(tys, BaseType::String), tys).len(),
            STRING_METHODS.len()
        );
        assert_eq!(
            methods_for(refined(tys, BaseType::Int), tys).len(),
            INT_METHODS.len()
        );
        assert_eq!(
            methods_for(refined(tys, BaseType::Float), tys).len(),
            FLOAT_METHODS.len()
        );
        // `Bool` has no kernel, so a `Bool`-based refinement inherits nothing.
        assert!(methods_for(refined(tys, BaseType::Bool), tys).is_empty());
        // Opaque types do not widen — no inherited kernel.
        let opaque = tys.intern(Ty::Named {
            name: "O".to_string(),
            kind: NamedKind::Opaque(BaseType::String),
            args: Vec::new(),
        });
        assert!(methods_for(opaque, tys).is_empty());
    }

    /// #596: a `Query[T]` receiver (a `store Map`'s `.entries`/`.keys`/
    /// `.values`, or a bare store map used as a value, ADR 0120) offers the
    /// full lazy query vocabulary — previously unmapped, so `.`-member
    /// completion on any `Query` offered nothing.
    #[test]
    fn query_receiver_offers_the_query_vocabulary() {
        let tys = &Types::new();
        let int = tys.intern(Ty::Base(BaseType::Int));
        let query = tys.intern(Ty::Query(int));
        assert_eq!(methods_for(query, tys).len(), QUERY_METHODS.len());
        assert!(methods_for(query, tys).iter().any(|m| m.name == "filter"));
        assert!(methods_for(query, tys).iter().any(|m| m.name == "collect"));
    }
}
