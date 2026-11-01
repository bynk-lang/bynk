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
//! The signatures are human-readable Bynk-surface display strings (generic in
//! the element/key/value type), for completion `detail` — not the checker's
//! `Ty`-typed signatures. A drift test (`kernel_registry_pins_dispatch`)
//! drives every listed method through the real checker and asserts none is
//! rejected as `method_not_found`, so the table can't list a phantom method.

use crate::checker::{NamedKind, Ty, TyId, Types};
use bynk_syntax::ast::BaseType;

/// One built-in kernel method: its name and a display signature.
#[derive(Debug, Clone, Copy)]
pub struct KernelMethod {
    pub name: &'static str,
    pub signature: &'static str,
}

const fn m(name: &'static str, signature: &'static str) -> KernelMethod {
    KernelMethod { name, signature }
}

/// `List[T]` (v0.20b; v0.88 adds the ADR 0116 query/collection vocabulary).
pub const LIST_METHODS: &[KernelMethod] = &[
    m("length", "length() -> Int"),
    m("get", "get(index: Int) -> Option[T]"),
    m("prepend", "prepend(item: T) -> List[T]"),
    // #1889: the end-of-list counterparts of `prepend`.
    m("append", "append(item: T) -> List[T]"),
    m("concat", "concat(other: List[T]) -> List[T]"),
    m("fold", "fold(init: U, step: (U, T) -> U) -> U"),
    m(
        "foldEff",
        "foldEff(init: U, step: (U, T) -> Effect[U]) -> Effect[U]",
    ),
    // v0.146 (ADR 0170): run an effectful step for each element, in order.
    m("forEach", "forEach(f: T -> Effect[()]) -> Effect[()]"),
    // v0.147 (ADR 0171): run an effectful step for every element concurrently.
    m(
        "parTraverse",
        "parTraverse(f: T -> Effect[()]) -> Effect[()]",
    ),
    // v0.148 (ADR 0172): collect every outcome (no short-circuit), sequential
    // and concurrent.
    m(
        "traverseAll",
        "traverseAll(f: T -> Effect[Result[U, E]]) -> Effect[List[Result[U, E]]]",
    ),
    m(
        "parTraverseAll",
        "parTraverseAll(f: T -> Effect[Result[U, E]]) -> Effect[List[Result[U, E]]]",
    ),
    // v0.150 (ADR 0174): short-circuit collect — stop at the first `Err`.
    m(
        "traverseTry",
        "traverseTry(f: T -> Effect[Result[U, E]]) -> Effect[Result[List[U], E]]",
    ),
    m(
        "parTraverseTry",
        "parTraverseTry(f: T -> Effect[Result[U, E]]) -> Effect[Result[List[U], E]]",
    ),
    // v0.88 (ADR 0116): eager in-memory builders + terminals.
    m("map", "map(f: T -> U) -> List[U]"),
    m("filter", "filter(p: T -> Bool) -> List[T]"),
    m("flatMap", "flatMap(f: T -> List[U]) -> List[U]"),
    m("sortBy", "sortBy(key: T -> K) -> List[T]"),
    m("take", "take(n: Int) -> List[T]"),
    m("skip", "skip(n: Int) -> List[T]"),
    m("distinct", "distinct() -> List[T]"),
    m("distinctBy", "distinctBy(key: T -> K) -> List[T]"),
    m("count", "count() -> Int"),
    m("any", "any(p: T -> Bool) -> Bool"),
    m("all", "all(p: T -> Bool) -> Bool"),
    m("first", "first() -> Option[T]"),
    m("firstOrElse", "firstOrElse(default: T) -> T"),
    m("sum", "sum(key: T -> K) -> K"),
    m("min", "min(key: T -> K) -> Option[K]"),
    m("max", "max(key: T -> K) -> Option[K]"),
    m("average", "average(key: T -> K) -> Option[Float]"),
    // v0.94 (ADR 0116/0120): joins & grouping, combiner form (eager — mirrors
    // QUERY_METHODS' lazy `Query[V]` forms below, but returns `List[V]`).
    m(
        "joinOn",
        "joinOn(other: List[U], on: (T, U) -> Bool, into: (T, U) -> V) -> List[V]",
    ),
    m(
        "leftJoin",
        "leftJoin(other: List[U], on: (T, U) -> Bool, into: (T, Option[U]) -> V) -> List[V]",
    ),
    m(
        "join",
        "join(other: List[U], on: (T, U) -> Bool, into: (T, U) -> V) -> List[V]",
    ),
    m(
        "groupBy",
        "groupBy(key: T -> K, into: (K, List[T]) -> V) -> List[V]",
    ),
];

/// `Map[K, V]` (v0.20b).
/// The `Set` kernel (K6a, #2596): what Bynk cannot write over a list. The
/// combinators (`union`, `intersect`, …) are the `bynk.set` commons.
pub const SET_METHODS: &[KernelMethod] = &[
    m("length", "length() -> Int"),
    m("contains", "contains(item: T) -> Bool"),
    m("insert", "insert(item: T) -> Set[T]"),
    m("remove", "remove(item: T) -> Set[T]"),
    m("toList", "toList() -> List[T]"),
];

pub const MAP_METHODS: &[KernelMethod] = &[
    m("length", "length() -> Int"),
    m("keys", "keys() -> List[K]"),
    m("values", "values() -> List[V]"),
    m("get", "get(key: K) -> Option[V]"),
    m("insert", "insert(key: K, value: V) -> Map[K, V]"),
];

/// `Query[T]` (v0.91, ADR 0115; joins/grouping v0.94, ADR 0116/0120) — the lazy
/// storage-query vocabulary a `store` collection lifts into (a `store Map`'s
/// `.entries`/`.keys`/`.values`, a `store Log`'s time-window roots, or a bare
/// `store Map` used as a value). Builders stay lazy (`Query[U]`); terminals
/// fold into the agent's storage capability (`Effect[T]`). Mirrors
/// [`LIST_METHODS`]'s eager vocabulary; dispatched in
/// `crate::checker::kernels::check_query_kernel_method`, which this table pins
/// (`kernel_registry_pins_dispatch`).
pub const QUERY_METHODS: &[KernelMethod] = &[
    m("map", "map(f: T -> U) -> Query[U]"),
    m("filter", "filter(p: T -> Bool) -> Query[T]"),
    m("flatMap", "flatMap(f: T -> Query[U]) -> Query[U]"),
    m("sortBy", "sortBy(key: T -> K) -> Query[T]"),
    m("take", "take(n: Int) -> Query[T]"),
    m("skip", "skip(n: Int) -> Query[T]"),
    m("distinct", "distinct() -> Query[T]"),
    m("distinctBy", "distinctBy(key: T -> K) -> Query[T]"),
    m(
        "joinOn",
        "joinOn(other: Query[U], on: (T, U) -> Bool, into: (T, U) -> V) -> Query[V]",
    ),
    m(
        "leftJoin",
        "leftJoin(other: Query[U], on: (T, U) -> Bool, into: (T, Option[U]) -> V) -> Query[V]",
    ),
    m(
        "join",
        "join(other: Query[U], on: (T, U) -> Bool, into: (T, U) -> V) -> Query[V]",
    ),
    m(
        "groupBy",
        "groupBy(key: T -> K, into: (K, Query[T]) -> V) -> Query[V]",
    ),
    m("collect", "collect() -> Effect[List[T]]"),
    m("first", "first() -> Effect[Option[T]]"),
    m("firstOrElse", "firstOrElse(default: T) -> Effect[T]"),
    m("count", "count() -> Effect[Int]"),
    m("fold", "fold(init: U, step: (U, T) -> U) -> Effect[U]"),
    m("any", "any(p: T -> Bool) -> Effect[Bool]"),
    m("all", "all(p: T -> Bool) -> Effect[Bool]"),
    m("sum", "sum(key: T -> K) -> Effect[K]"),
    m("min", "min(key: T -> K) -> Effect[Option[K]]"),
    m("max", "max(key: T -> K) -> Effect[Option[K]]"),
    m("average", "average(key: T -> K) -> Effect[Option[Float]]"),
    m("forEach", "forEach(f: T -> Effect[()]) -> Effect[()]"),
    m(
        "parTraverse",
        "parTraverse(f: T -> Effect[()]) -> Effect[()]",
    ),
    m(
        "traverseAll",
        "traverseAll(f: T -> Effect[Result[U, E]]) -> Effect[List[Result[U, E]]]",
    ),
    m(
        "parTraverseAll",
        "parTraverseAll(f: T -> Effect[Result[U, E]]) -> Effect[List[Result[U, E]]]",
    ),
    m(
        "traverseTry",
        "traverseTry(f: T -> Effect[Result[U, E]]) -> Effect[Result[List[U], E]]",
    ),
    m(
        "parTraverseTry",
        "parTraverseTry(f: T -> Effect[Result[U, E]]) -> Effect[Result[List[U], E]]",
    ),
];

/// `Option[T]` combinators (v0.22a).
pub const OPTION_METHODS: &[KernelMethod] = &[
    m("map", "map(f: T -> U) -> Option[U]"),
    m("andThen", "andThen(f: T -> Option[U]) -> Option[U]"),
    m("getOrElse", "getOrElse(default: T) -> T"),
    m("isSome", "isSome() -> Bool"),
    m("okOr", "okOr(err: E) -> Result[T, E]"),
];

/// `Result[T, E]` combinators (v0.22a).
pub const RESULT_METHODS: &[KernelMethod] = &[
    m("map", "map(f: T -> U) -> Result[U, E]"),
    m("andThen", "andThen(f: T -> Result[U, E]) -> Result[U, E]"),
    m("mapErr", "mapErr(f: E -> F) -> Result[T, F]"),
    m("getOrElse", "getOrElse(default: T) -> T"),
    m("isOk", "isOk() -> Bool"),
];
/// #2683: `e.within(d)`, on every `Effect[T]`. It races `e` against a deadline
/// and hands the abandoned effect to the execution context's `waitUntil`; it is
/// never cancelled, so a `TimedOut` does not mean the work did not happen.
const WITHIN: KernelMethod = m(
    "within",
    "within(d: Duration) -> Effect[Result[T, TimedOut]]",
);

/// #2711: `a.race(b)`, on every `Effect[T]`. The first side to succeed wins; the
/// other is handed, uncancelled, to the execution context's `waitUntil`, so its
/// side effects still happen. Meant for idempotent work.
const RACE: KernelMethod = m("race", "race(other: Effect[T]) -> Effect[T]");

/// The methods of every `Effect[T]` (#2683, #2711): `within` and `race`.
pub const EFFECT_METHODS: &[KernelMethod] = &[WITHIN, RACE];

/// The `Effect[Result[T, E]]` combinators (v0.152, ADR 0176 — design doc
/// §2.8.3): the compiler-synthesised methods on the universal cross-context
/// shape. `mapOk`/`mapErr` transform the success/error value; `flatMapOk`
/// chains a further effectful-fallible step; `flatMapErr` attempts an effectful
/// recovery.
pub const EFFECT_RESULT_METHODS: &[KernelMethod] = &[
    // #2683: `within` is on every `Effect[T]` (see [`EFFECT_METHODS`]), so an
    // `Effect[Result]` offers it, and `race` (#2711), alongside the combinators.
    WITHIN,
    RACE,
    m("mapOk", "mapOk(f: T -> U) -> Effect[Result[U, E]]"),
    m("mapErr", "mapErr(f: E -> F) -> Effect[Result[T, F]]"),
    m(
        "flatMapOk",
        "flatMapOk(f: T -> Effect[Result[U, E]]) -> Effect[Result[U, E]]",
    ),
    m(
        "flatMapErr",
        "flatMapErr(f: E -> Effect[Result[T, F]]) -> Effect[Result[T, F]]",
    ),
];

/// The `String` kernel (v0.22a; UTF-16 code units, except `chars`).
pub const STRING_METHODS: &[KernelMethod] = &[
    m("length", "length() -> Int"),
    m("split", "split(sep: String) -> List[String]"),
    m("trim", "trim() -> String"),
    m("toUpper", "toUpper() -> String"),
    m("toLower", "toLower() -> String"),
    m("contains", "contains(s: String) -> Bool"),
    m("startsWith", "startsWith(s: String) -> Bool"),
    m("endsWith", "endsWith(s: String) -> Bool"),
    m("replace", "replace(from: String, to: String) -> String"),
    m("slice", "slice(start: Int, end: Int) -> String"),
    m("indexOf", "indexOf(s: String) -> Option[Int]"),
    m("chars", "chars() -> List[String]"),
    m("concat", "concat(s: String) -> String"),
    // K2 (#2594): RFC 3986 percent-encoding over the UTF-8 bytes.
    m("percentEncode", "percentEncode() -> String"),
    m("percentDecode", "percentDecode() -> Option[String]"),
    // K6c (#2600): regex methods over a string-literal pattern.
    m("matchesRegex", "matchesRegex(pattern: String) -> Bool"),
    m("containsRegex", "containsRegex(pattern: String) -> Bool"),
    m(
        "findRegex",
        "findRegex(pattern: String) -> Option[List[Option[String]]]",
    ),
    m(
        "findAllRegex",
        "findAllRegex(pattern: String) -> List[List[Option[String]]]",
    ),
    m(
        "replaceRegex",
        "replaceRegex(pattern: String, with: String) -> String",
    ),
    m("splitRegex", "splitRegex(pattern: String) -> List[String]"),
];

/// The `Int` numeric kernel (v0.21).
pub const INT_METHODS: &[KernelMethod] = &[
    m("toFloat", "toFloat() -> Float"),
    m("toString", "toString() -> String"),
    m("abs", "abs() -> Int"),
    m("min", "min(other: Int) -> Int"),
    m("max", "max(other: Int) -> Int"),
    m("clamp", "clamp(lo: Int, hi: Int) -> Int"),
    // K6b (#2598): division companions (fault on zero) and an exact power.
    m("div", "div(other: Int) -> Int"),
    m("rem", "rem(other: Int) -> Int"),
    m("mod", "mod(other: Int) -> Int"),
    m("pow", "pow(exp: Int) -> Int"),
];

/// The `Float` numeric kernel (v0.21).
pub const FLOAT_METHODS: &[KernelMethod] = &[
    m("round", "round() -> Int"),
    m("floor", "floor() -> Int"),
    m("ceil", "ceil() -> Int"),
    m("truncate", "truncate() -> Int"),
    m("toString", "toString() -> String"),
    m("abs", "abs() -> Float"),
    m("min", "min(other: Float) -> Float"),
    m("max", "max(other: Float) -> Float"),
    m("clamp", "clamp(lo: Float, hi: Float) -> Float"),
    m("isNaN", "isNaN() -> Bool"),
    m("isFinite", "isFinite() -> Bool"),
    // K6b (#2598): `Math` functions, host-defined outside their domains.
    m("sqrt", "sqrt() -> Float"),
    m("pow", "pow(exp: Float) -> Float"),
    m("exp", "exp() -> Float"),
    m("ln", "ln() -> Float"),
    m("log10", "log10() -> Float"),
    m("log2", "log2() -> Float"),
    m("sin", "sin() -> Float"),
    m("cos", "cos() -> Float"),
    m("tan", "tan() -> Float"),
    m("asin", "asin() -> Float"),
    m("acos", "acos() -> Float"),
    m("atan", "atan() -> Float"),
    m("atan2", "atan2(x: Float) -> Float"),
];

/// The `Duration` kernel (v0.86, ADR 0112). Comparison/arithmetic are operators
/// (D3/D4); the kernel is the explicit escape to raw milliseconds (D5).
pub const DURATION_METHODS: &[KernelMethod] = &[
    m("toMillis", "toMillis() -> Int"),
    m("toString", "toString() -> String"),
];

/// The `Instant` kernel (v0.90, ADR 0114). Comparison/arithmetic are operators
/// (D3); the kernel is the explicit escape to raw epoch milliseconds (D6).
pub const INSTANT_METHODS: &[KernelMethod] = &[
    m("toEpochMillis", "toEpochMillis() -> Int"),
    m("toString", "toString() -> String"),
    // K3 (#2592): RFC 3339 in UTC, `YYYY-MM-DDTHH:mm:ss.sssZ`.
    m("toIso", "toIso() -> String"),
];

/// The `Bytes` kernel (v0.110, ADR 0142). Equality is an operator (D4, content
/// compare); the kernel is length plus the String-interop bridge (D3). #2016
/// (standard-library K1) adds `concat`, `slice`, `compare` (a method: `Bytes`
/// stays unorderable), hex and base64url.
pub const BYTES_METHODS: &[KernelMethod] = &[
    m("length", "length() -> Int"),
    m("concat", "concat(other: Bytes) -> Bytes"),
    m("slice", "slice(start: Int, end: Int) -> Bytes"),
    m("compare", "compare(other: Bytes) -> Int"),
    m("toHex", "toHex() -> String"),
    m("toBase64", "toBase64() -> String"),
    m("toBase64Url", "toBase64Url() -> String"),
    m("decodeUtf8", "decodeUtf8() -> Option[String]"),
];

/// `RequestHeaders` (#2341, L4): an HTTP handler's read-only view of the
/// request's headers.
pub const REQUEST_HEADERS_METHODS: &[KernelMethod] = &[
    m("get", "get(name: String) -> Option[String]"),
    m("bearer", "bearer() -> Option[String]"),
    m("clientIp", "clientIp() -> Option[String]"),
];

/// `TokenClaims` (#2380, L5): an `Oidc` actor's verified token claims.
pub const TOKEN_CLAIMS_METHODS: &[KernelMethod] = &[
    m("get", "get(name: String) -> Option[String]"),
    m("has", "has(name: String) -> Bool"),
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
        Ty::Base(BaseType::String) => STRING_METHODS,
        Ty::RequestHeaders => REQUEST_HEADERS_METHODS,
        Ty::TokenClaims => TOKEN_CLAIMS_METHODS,
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
            BaseType::String => STRING_METHODS,
            BaseType::Bool => &[],
        },
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refined(tys: &Types, base: BaseType) -> TyId {
        tys.intern(Ty::Named {
            name: "R".to_string(),
            kind: NamedKind::Refined(base),
            args: Vec::new(),
        })
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
