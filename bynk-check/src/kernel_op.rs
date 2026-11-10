//! The typed built-in method operation (#3164).
//!
//! A built-in method call is resolved by the checker to a [`KernelOp`] and
//! carried in [`crate::checker::Callee::Kernel`], so the emitter and the editor
//! tooling match on a resolved enum rather than re-deriving the operation from
//! the method's spelling. One enum per receiver kind, declared once here with
//! each operation's source spelling and display signature; the completion
//! tables in [`crate::kernel_methods`] are generated from the same declaration.

/// One built-in kernel method: its name and a display signature.
#[derive(Debug, Clone, Copy)]
pub struct KernelMethod {
    pub name: &'static str,
    pub signature: &'static str,
}

/// Declare the per-receiver operation enums. Each operation names its source
/// spelling and display signature; an `@hidden` operation is dispatched by the
/// checker but kept out of the completion table (it is not a general method).
macro_rules! kernel_ops {
    ($(
        $(#[$meta:meta])*
        $Enum:ident {
            $( $(#[$vmeta:meta])* $Var:ident = $name:literal, $sig:literal; )*
            $( @hidden $( $(#[$hmeta:meta])* $HVar:ident = $hname:literal; )* )?
        }
    )*) => {
        $(
            $(#[$meta])*
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum $Enum {
                $( $(#[$vmeta])* $Var, )*
                $($( $(#[$hmeta])* $HVar, )*)?
            }

            impl $Enum {
                /// Every operation, in declaration order.
                pub const ALL: &'static [$Enum] = &[
                    $( $Enum::$Var, )* $($( $Enum::$HVar, )*)?
                ];
                /// The completion table for this receiver kind (no hidden operations).
                pub const TABLE: &'static [KernelMethod] =
                    &[ $( KernelMethod { name: $name, signature: $sig }, )* ];

                /// The operation spelt `name` on this receiver kind, if any.
                pub fn from_name(name: &str) -> Option<Self> {
                    match name {
                        $( $name => Some($Enum::$Var), )*
                        $($( $hname => Some($Enum::$HVar), )*)?
                        _ => None,
                    }
                }

                /// The method's source spelling.
                pub const fn name(self) -> &'static str {
                    match self {
                        $( $Enum::$Var => $name, )*
                        $($( $Enum::$HVar => $hname, )*)?
                    }
                }
            }
        )*
    };
}

kernel_ops! {
/// `List[T]` (v0.20b; v0.88 adds the ADR 0116 query/collection vocabulary).
    ListOp {
        Length = "length", "length() -> Int";
        Get = "get", "get(index: Int) -> Option[T]";
        Prepend = "prepend", "prepend(item: T) -> List[T]";
        // #1889: the end-of-list counterparts of `prepend`.
        Append = "append", "append(item: T) -> List[T]";
        Concat = "concat", "concat(other: List[T]) -> List[T]";
        Fold = "fold", "fold(init: U, step: (U, T) -> U) -> U";
        FoldEff = "foldEff", "foldEff(init: U, step: (U, T) -> Effect[U]) -> Effect[U]";
        // v0.146 (ADR 0170): run an effectful step for each element, in order.
        ForEach = "forEach", "forEach(f: T -> Effect[()]) -> Effect[()]";
        // v0.147 (ADR 0171): run an effectful step for every element concurrently.
        ParTraverse = "parTraverse", "parTraverse(f: T -> Effect[()]) -> Effect[()]";
        // v0.148 (ADR 0172): collect every outcome (no short-circuit), sequential
        // and concurrent.
        TraverseAll = "traverseAll", "traverseAll(f: T -> Effect[Result[U, E]]) -> Effect[List[Result[U, E]]]";
        ParTraverseAll = "parTraverseAll", "parTraverseAll(f: T -> Effect[Result[U, E]]) -> Effect[List[Result[U, E]]]";
        // v0.150 (ADR 0174): short-circuit collect — stop at the first `Err`.
        TraverseTry = "traverseTry", "traverseTry(f: T -> Effect[Result[U, E]]) -> Effect[Result[List[U], E]]";
        ParTraverseTry = "parTraverseTry", "parTraverseTry(f: T -> Effect[Result[U, E]]) -> Effect[Result[List[U], E]]";
        // #2806: pure `Result` accumulation over a `List[Result[T, E]]` receiver.
        SequenceAll = "sequenceAll", "sequenceAll() -> Result[List[T], List[E]]";
        SequenceTry = "sequenceTry", "sequenceTry() -> Result[List[T], E]";
        // v0.88 (ADR 0116): eager in-memory builders + terminals.
        Map = "map", "map(f: T -> U) -> List[U]";
        Filter = "filter", "filter(p: T -> Bool) -> List[T]";
        FlatMap = "flatMap", "flatMap(f: T -> List[U]) -> List[U]";
        SortBy = "sortBy", "sortBy(key: T -> K) -> List[T]";
        SortByDescending = "sortByDescending", "sortByDescending(key: T -> K) -> List[T]";
        Take = "take", "take(n: Int) -> List[T]";
        Skip = "skip", "skip(n: Int) -> List[T]";
        Distinct = "distinct", "distinct() -> List[T]";
        DistinctBy = "distinctBy", "distinctBy(key: T -> K) -> List[T]";
        Count = "count", "count() -> Int";
        Any = "any", "any(p: T -> Bool) -> Bool";
        All = "all", "all(p: T -> Bool) -> Bool";
        First = "first", "first() -> Option[T]";
        FirstOrElse = "firstOrElse", "firstOrElse(default: T) -> T";
        Sum = "sum", "sum(key: T -> K) -> K";
        Min = "min", "min(key: T -> K) -> Option[K]";
        Max = "max", "max(key: T -> K) -> Option[K]";
        Average = "average", "average(key: T -> K) -> Option[Float]";
        // v0.94 (ADR 0116/0120): joins & grouping, combiner form (eager — mirrors
        // QUERY_METHODS' lazy `Query[V]` forms below, but returns `List[V]`).
        JoinOn = "joinOn", "joinOn(other: List[U], on: (T, U) -> Bool, into: (T, U) -> V) -> List[V]";
        LeftJoin = "leftJoin", "leftJoin(other: List[U], on: (T, U) -> Bool, into: (T, Option[U]) -> V) -> List[V]";
        Join = "join", "join(other: List[U], on: (T, U) -> Bool, into: (T, U) -> V) -> List[V]";
        GroupBy = "groupBy", "groupBy(key: T -> K, into: (K, List[T]) -> V) -> List[V]";
        @hidden
        // v0.119 (ADR 0155): `run.upTo(step)` on a synthesised history `Step`
        // list only; it never leaks onto ordinary lists.
        UpTo = "upTo";
    }
/// The `Set` kernel (K6a, #2596): what Bynk cannot write over a list. The
/// combinators (`union`, `intersect`, …) are the `bynk.set` commons.
    SetOp {
        Length = "length", "length() -> Int";
        Contains = "contains", "contains(item: T) -> Bool";
        Insert = "insert", "insert(item: T) -> Set[T]";
        Remove = "remove", "remove(item: T) -> Set[T]";
        ToList = "toList", "toList() -> List[T]";
    }
/// `Map[K, V]` (v0.20b).
    MapOp {
        Length = "length", "length() -> Int";
        Keys = "keys", "keys() -> List[K]";
        Values = "values", "values() -> List[V]";
        Get = "get", "get(key: K) -> Option[V]";
        Insert = "insert", "insert(key: K, value: V) -> Map[K, V]";
    }
/// `Query[T]` (v0.91, ADR 0115; joins/grouping v0.94, ADR 0116/0120) — the lazy
/// storage-query vocabulary a `store` collection lifts into (a `store Map`'s
/// `.entries`/`.keys`/`.values`, a `store Log`'s time-window roots, or a bare
/// `store Map` used as a value). Builders stay lazy (`Query[U]`); terminals
/// fold into the agent's storage capability (`Effect[T]`). Mirrors
/// [`ListOp`]'s eager vocabulary; dispatched in
/// `crate::checker::kernels::check_query_kernel_method`, which matches on it
/// exhaustively.
    QueryOp {
        Map = "map", "map(f: T -> U) -> Query[U]";
        Filter = "filter", "filter(p: T -> Bool) -> Query[T]";
        FlatMap = "flatMap", "flatMap(f: T -> Query[U]) -> Query[U]";
        SortBy = "sortBy", "sortBy(key: T -> K) -> Query[T]";
        SortByDescending = "sortByDescending", "sortByDescending(key: T -> K) -> Query[T]";
        Take = "take", "take(n: Int) -> Query[T]";
        Skip = "skip", "skip(n: Int) -> Query[T]";
        Distinct = "distinct", "distinct() -> Query[T]";
        DistinctBy = "distinctBy", "distinctBy(key: T -> K) -> Query[T]";
        JoinOn = "joinOn", "joinOn(other: Query[U], on: (T, U) -> Bool, into: (T, U) -> V) -> Query[V]";
        LeftJoin = "leftJoin", "leftJoin(other: Query[U], on: (T, U) -> Bool, into: (T, Option[U]) -> V) -> Query[V]";
        Join = "join", "join(other: Query[U], on: (T, U) -> Bool, into: (T, U) -> V) -> Query[V]";
        GroupBy = "groupBy", "groupBy(key: T -> K, into: (K, Query[T]) -> V) -> Query[V]";
        Collect = "collect", "collect() -> Effect[List[T]]";
        First = "first", "first() -> Effect[Option[T]]";
        FirstOrElse = "firstOrElse", "firstOrElse(default: T) -> Effect[T]";
        Count = "count", "count() -> Effect[Int]";
        Fold = "fold", "fold(init: U, step: (U, T) -> U) -> Effect[U]";
        Any = "any", "any(p: T -> Bool) -> Effect[Bool]";
        All = "all", "all(p: T -> Bool) -> Effect[Bool]";
        Sum = "sum", "sum(key: T -> K) -> Effect[K]";
        Min = "min", "min(key: T -> K) -> Effect[Option[K]]";
        Max = "max", "max(key: T -> K) -> Effect[Option[K]]";
        Average = "average", "average(key: T -> K) -> Effect[Option[Float]]";
        ForEach = "forEach", "forEach(f: T -> Effect[()]) -> Effect[()]";
        ParTraverse = "parTraverse", "parTraverse(f: T -> Effect[()]) -> Effect[()]";
        TraverseAll = "traverseAll", "traverseAll(f: T -> Effect[Result[U, E]]) -> Effect[List[Result[U, E]]]";
        ParTraverseAll = "parTraverseAll", "parTraverseAll(f: T -> Effect[Result[U, E]]) -> Effect[List[Result[U, E]]]";
        TraverseTry = "traverseTry", "traverseTry(f: T -> Effect[Result[U, E]]) -> Effect[Result[List[U], E]]";
        ParTraverseTry = "parTraverseTry", "parTraverseTry(f: T -> Effect[Result[U, E]]) -> Effect[Result[List[U], E]]";
    }
/// `Option[T]` combinators (v0.22a).
    OptionOp {
        Map = "map", "map(f: T -> U) -> Option[U]";
        AndThen = "andThen", "andThen(f: T -> Option[U]) -> Option[U]";
        GetOrElse = "getOrElse", "getOrElse(default: T) -> T";
        IsSome = "isSome", "isSome() -> Bool";
        OkOr = "okOr", "okOr(err: E) -> Result[T, E]";
        IsNone = "isNone", "isNone() -> Bool";
        OrElse = "orElse", "orElse(f: () -> Option[T]) -> Option[T]";
        Filter = "filter", "filter(p: T -> Bool) -> Option[T]";
    }
/// `Result[T, E]` combinators (v0.22a).
    ResultOp {
        Map = "map", "map(f: T -> U) -> Result[U, E]";
        AndThen = "andThen", "andThen(f: T -> Result[U, E]) -> Result[U, E]";
        MapErr = "mapErr", "mapErr(f: E -> F) -> Result[T, F]";
        GetOrElse = "getOrElse", "getOrElse(default: T) -> T";
        IsOk = "isOk", "isOk() -> Bool";
        IsErr = "isErr", "isErr() -> Bool";
        Ok = "ok", "ok() -> Option[T]";
    }
/// The `String` kernel (v0.22a; UTF-16 code units, except `chars`).
    StringOp {
        Length = "length", "length() -> Int";
        Split = "split", "split(sep: String) -> List[String]";
        Trim = "trim", "trim() -> String";
        ToUpper = "toUpper", "toUpper() -> String";
        ToLower = "toLower", "toLower() -> String";
        Contains = "contains", "contains(s: String) -> Bool";
        StartsWith = "startsWith", "startsWith(s: String) -> Bool";
        EndsWith = "endsWith", "endsWith(s: String) -> Bool";
        Replace = "replace", "replace(from: String, to: String) -> String";
        Slice = "slice", "slice(start: Int, end: Int) -> String";
        IndexOf = "indexOf", "indexOf(s: String) -> Option[Int]";
        Chars = "chars", "chars() -> List[String]";
        Concat = "concat", "concat(s: String) -> String";
        // K2 (#2594): RFC 3986 percent-encoding over the UTF-8 bytes.
        PercentEncode = "percentEncode", "percentEncode() -> String";
        PercentDecode = "percentDecode", "percentDecode() -> Option[String]";
        // K6c (#2600): regex methods over a string-literal pattern.
        MatchesRegex = "matchesRegex", "matchesRegex(pattern: String) -> Bool";
        ContainsRegex = "containsRegex", "containsRegex(pattern: String) -> Bool";
        FindRegex = "findRegex", "findRegex(pattern: String) -> Option[List[Option[String]]]";
        FindAllRegex = "findAllRegex", "findAllRegex(pattern: String) -> List[List[Option[String]]]";
        ReplaceRegex = "replaceRegex", "replaceRegex(pattern: String, with: String) -> String";
        SplitRegex = "splitRegex", "splitRegex(pattern: String) -> List[String]";
    }
/// The `Int` numeric kernel (v0.21).
    IntOp {
        ToFloat = "toFloat", "toFloat() -> Float";
        ToString = "toString", "toString() -> String";
        Abs = "abs", "abs() -> Int";
        Min = "min", "min(other: Int) -> Int";
        Max = "max", "max(other: Int) -> Int";
        Clamp = "clamp", "clamp(lo: Int, hi: Int) -> Int";
        // K6b (#2598): division companions (fault on zero) and an exact power.
        Div = "div", "div(other: Int) -> Int";
        Rem = "rem", "rem(other: Int) -> Int";
        Mod = "mod", "mod(other: Int) -> Int";
        Pow = "pow", "pow(exp: Int) -> Int";
        // #2621: bitwise operations over the whole safe-integer range, faulting
        // on a negative operand or a shift count outside 0..52.
        And = "and", "and(other: Int) -> Int";
        Or = "or", "or(other: Int) -> Int";
        Xor = "xor", "xor(other: Int) -> Int";
        AndNot = "andNot", "andNot(mask: Int) -> Int";
        Shl = "shl", "shl(count: Int) -> Int";
        Shr = "shr", "shr(count: Int) -> Int";
    }
/// The `Float` numeric kernel (v0.21).
    FloatOp {
        Round = "round", "round() -> Int";
        Floor = "floor", "floor() -> Int";
        Ceil = "ceil", "ceil() -> Int";
        Truncate = "truncate", "truncate() -> Int";
        ToString = "toString", "toString() -> String";
        Abs = "abs", "abs() -> Float";
        Min = "min", "min(other: Float) -> Float";
        Max = "max", "max(other: Float) -> Float";
        Clamp = "clamp", "clamp(lo: Float, hi: Float) -> Float";
        IsNaN = "isNaN", "isNaN() -> Bool";
        IsFinite = "isFinite", "isFinite() -> Bool";
        // K6b (#2598): `Math` functions, host-defined outside their domains.
        Sqrt = "sqrt", "sqrt() -> Float";
        Pow = "pow", "pow(exp: Float) -> Float";
        Exp = "exp", "exp() -> Float";
        Ln = "ln", "ln() -> Float";
        Log10 = "log10", "log10() -> Float";
        Log2 = "log2", "log2() -> Float";
        Sin = "sin", "sin() -> Float";
        Cos = "cos", "cos() -> Float";
        Tan = "tan", "tan() -> Float";
        Asin = "asin", "asin() -> Float";
        Acos = "acos", "acos() -> Float";
        Atan = "atan", "atan() -> Float";
        Atan2 = "atan2", "atan2(x: Float) -> Float";
    }
/// The `Duration` kernel (v0.86, ADR 0112). Comparison/arithmetic are operators
/// (D3/D4); the kernel is the explicit escape to raw milliseconds (D5).
    DurationOp {
        ToMillis = "toMillis", "toMillis() -> Int";
        ToString = "toString", "toString() -> String";
    }
/// The `Instant` kernel (v0.90, ADR 0114). Comparison/arithmetic are operators
/// (D3); the kernel is the explicit escape to raw epoch milliseconds (D6).
    InstantOp {
        ToEpochMillis = "toEpochMillis", "toEpochMillis() -> Int";
        ToString = "toString", "toString() -> String";
        // K3 (#2592): RFC 3339 in UTC, `YYYY-MM-DDTHH:mm:ss.sssZ`.
        ToIso = "toIso", "toIso() -> String";
    }
/// The `Bytes` kernel (v0.110, ADR 0142). Equality is an operator (D4, content
/// compare); the kernel is length plus the String-interop bridge (D3). #2016
/// (standard-library K1) adds `concat`, `slice`, `compare` (a method: `Bytes`
/// stays unorderable), hex and base64url.
    BytesOp {
        Length = "length", "length() -> Int";
        Concat = "concat", "concat(other: Bytes) -> Bytes";
        Slice = "slice", "slice(start: Int, end: Int) -> Bytes";
        Compare = "compare", "compare(other: Bytes) -> Int";
        ToHex = "toHex", "toHex() -> String";
        ToBase64 = "toBase64", "toBase64() -> String";
        ToBase64Url = "toBase64Url", "toBase64Url() -> String";
        DecodeUtf8 = "decodeUtf8", "decodeUtf8() -> Option[String]";
    }
/// The `Decimal` kernel (#2622). `round` and `div` take a `RoundingMode`
/// (`RoundingMode.HalfEven`), read syntactically at that argument.
    DecimalOp {
        Scale = "scale", "scale() -> Int";
        ToString = "toString", "toString() -> String";
        Round = "round", "round(scale: Int, mode: RoundingMode) -> Decimal";
        Div = "div", "div(other: Decimal, scale: Int, mode: RoundingMode) -> Decimal";
        Abs = "abs", "abs() -> Decimal";
        Min = "min", "min(other: Decimal) -> Decimal";
        Max = "max", "max(other: Decimal) -> Decimal";
        ToInt = "toInt", "toInt() -> Option[Int]";
        ToFloat = "toFloat", "toFloat() -> Float";
    }
/// `RequestHeaders` (#2341, L4): an HTTP handler's read-only view of the
/// request's headers.
    RequestHeadersOp {
        Get = "get", "get(name: String) -> Option[String]";
        Bearer = "bearer", "bearer() -> Option[String]";
        ClientIp = "clientIp", "clientIp() -> Option[String]";
    }
/// `RequestQuery` (#2854): an HTTP handler's read-only view of the request's
/// query string.
    RequestQueryOp {
        Get = "get", "get(name: String) -> Option[String]";
        All = "all", "all(name: String) -> List[String]";
        Params = "params", "params() -> Map[String, List[String]]";
    }
/// `TokenClaims` (#2380, L5): an `Oidc` actor's verified token claims.
    TokenClaimsOp {
        Get = "get", "get(name: String) -> Option[String]";
        Has = "has", "has(name: String) -> Bool";
    }
/// `TestConnection[F]` (#3051): a case's recording handle for a `from
/// websocket` connection; a test body only.
    TestConnectionOp {
        Sent = "sent", "sent() -> List[F]";
        Closed = "closed", "closed() -> Bool";
        Connection = "connection", "connection() -> Connection[F]";
    }

/// `Stream[T]` (v0.100): the lazy builders and the draining terminal. Not in
/// the completion registry ([`crate::kernel_methods::ALL_METHODS`]).
    StreamOp {
        Map = "map", "map(f: T -> U) -> Stream[U]";
        Take = "take", "take(n: Int) -> Stream[T]";
        Collect = "collect", "collect() -> Effect[List[T]]";
    }
/// `Connection[F]` (v0.102): the held-resource operations. Not in the
/// completion registry.
    ConnectionOp {
        Send = "send", "send(frame: F) -> Effect[()]";
        Close = "close", "close() -> Effect[()]";
    }
/// `Effect[T]` (#2683, #2711): `within` and `race`, on every `Effect`.
    EffectOp {
        // #2683: `e.within(d)` races `e` against a deadline and hands the
        // abandoned effect to the execution context's `waitUntil`; it is never
        // cancelled, so a `TimedOut` does not mean the work did not happen.
        Within = "within", "within(d: Duration) -> Effect[Result[T, TimedOut]]";
        // #2711: `a.race(b)`: the first side to succeed wins; the other is handed,
        // uncancelled, to the execution context's `waitUntil`, so its side effects
        // still happen. Meant for idempotent work.
        Race = "race", "race(other: Effect[T]) -> Effect[T]";
    }
/// The `Effect[Result[T, E]]` combinators (v0.152, ADR 0176; design doc §2.8.3),
/// beyond [`EffectOp`]: the compiler-synthesised methods on the universal
/// cross-context shape.
    EffectResultOp {
        MapOk = "mapOk", "mapOk(f: T -> U) -> Effect[Result[U, E]]";
        MapErr = "mapErr", "mapErr(f: E -> F) -> Effect[Result[T, F]]";
        FlatMapOk = "flatMapOk", "flatMapOk(f: T -> Effect[Result[U, E]]) -> Effect[Result[U, E]]";
        FlatMapErr = "flatMapErr", "flatMapErr(f: E -> Effect[Result[T, F]]) -> Effect[Result[T, F]]";
    }
}

/// A resolved built-in method: the receiver kind and the operation on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KernelOp {
    List(ListOp),
    Query(QueryOp),
    Stream(StreamOp),
    Connection(ConnectionOp),
    TestConnection(TestConnectionOp),
    Map(MapOp),
    Set(SetOp),
    Int(IntOp),
    Float(FloatOp),
    Duration(DurationOp),
    Instant(InstantOp),
    RequestHeaders(RequestHeadersOp),
    RequestQuery(RequestQueryOp),
    TokenClaims(TokenClaimsOp),
    Bytes(BytesOp),
    Decimal(DecimalOp),
    String(StringOp),
    Option(OptionOp),
    Result(ResultOp),
    /// `within`/`race`, on every `Effect[T]`.
    Effect(EffectOp),
    /// A combinator on an `Effect[Result[T, E]]`.
    EffectResult(EffectResultOp),
}

impl KernelOp {
    /// The method's source spelling.
    pub const fn name(self) -> &'static str {
        match self {
            KernelOp::List(o) => o.name(),
            KernelOp::Query(o) => o.name(),
            KernelOp::Stream(o) => o.name(),
            KernelOp::Connection(o) => o.name(),
            KernelOp::TestConnection(o) => o.name(),
            KernelOp::Map(o) => o.name(),
            KernelOp::Set(o) => o.name(),
            KernelOp::Int(o) => o.name(),
            KernelOp::Float(o) => o.name(),
            KernelOp::Duration(o) => o.name(),
            KernelOp::Instant(o) => o.name(),
            KernelOp::RequestHeaders(o) => o.name(),
            KernelOp::RequestQuery(o) => o.name(),
            KernelOp::TokenClaims(o) => o.name(),
            KernelOp::Bytes(o) => o.name(),
            KernelOp::Decimal(o) => o.name(),
            KernelOp::String(o) => o.name(),
            KernelOp::Option(o) => o.name(),
            KernelOp::Result(o) => o.name(),
            KernelOp::Effect(o) => o.name(),
            KernelOp::EffectResult(o) => o.name(),
        }
    }
}

impl KernelOp {
    /// The operation a *base* type's kernel offers under `name`: what a bare
    /// `Int`/`Float`/`String`/… receiver resolves to, and what a refined
    /// receiver inherits (#561). `None` for `Bool`, which has no kernel.
    pub fn for_base(base: bynk_syntax::ast::BaseType, name: &str) -> Option<KernelOp> {
        use bynk_syntax::ast::BaseType;
        match base {
            BaseType::Int => IntOp::from_name(name).map(KernelOp::Int),
            BaseType::Float => FloatOp::from_name(name).map(KernelOp::Float),
            BaseType::String => StringOp::from_name(name).map(KernelOp::String),
            BaseType::Duration => DurationOp::from_name(name).map(KernelOp::Duration),
            BaseType::Instant => InstantOp::from_name(name).map(KernelOp::Instant),
            BaseType::Bytes => BytesOp::from_name(name).map(KernelOp::Bytes),
            BaseType::Decimal => DecimalOp::from_name(name).map(KernelOp::Decimal),
            BaseType::Bool => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! round_trips {
        ($($Enum:ident),* $(,)?) => {$(
            for op in $Enum::ALL {
                // A duplicated spelling would resolve to its first variant.
                assert_eq!($Enum::from_name(op.name()), Some(*op), "{op:?}");
            }
            assert!($Enum::ALL.len() >= $Enum::TABLE.len());
        )*};
    }

    #[test]
    fn every_operation_round_trips_through_its_spelling() {
        round_trips!(
            ListOp,
            QueryOp,
            StreamOp,
            ConnectionOp,
            TestConnectionOp,
            MapOp,
            SetOp,
            IntOp,
            FloatOp,
            DurationOp,
            InstantOp,
            RequestHeadersOp,
            RequestQueryOp,
            TokenClaimsOp,
            BytesOp,
            DecimalOp,
            StringOp,
            OptionOp,
            ResultOp,
            EffectOp,
            EffectResultOp,
        );
    }

    #[test]
    fn an_unknown_spelling_resolves_to_nothing() {
        assert_eq!(ListOp::from_name("nope"), None);
        assert_eq!(
            KernelOp::for_base(bynk_syntax::ast::BaseType::Bool, "length"),
            None
        );
    }
}
