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
        // #3967: named `parForEach`, so `parTraverse` can return the values.
        ParForEach = "parForEach", "parForEach(f: T -> Effect[()]) -> Effect[()]";
        ParTraverse = "parTraverse", "parTraverse(f: T -> Effect[U]) -> Effect[List[U]]";
        // v0.148 (ADR 0172): collect every outcome (no short-circuit), sequential
        // and concurrent.
        TraverseAll = "traverseAll", "traverseAll(f: T -> Effect[Result[U, E]]) -> Effect[List[Result[U, E]]]";
        ParTraverseAll = "parTraverseAll", "parTraverseAll(f: T -> Effect[Result[U, E]]) -> Effect[List[Result[U, E]]]";
        // v0.150 (ADR 0174): short-circuit collect — stop at the first `Err`.
        TraverseTry = "traverseTry", "traverseTry(f: T -> Effect[Result[U, E]]) -> Effect[Result[List[U], E]]";
        ParTraverseTry = "parTraverseTry", "parTraverseTry(f: T -> Effect[Result[U, E]]) -> Effect[Result[List[U], E]]";
        // #3968: the concurrent forms with a bound on the steps in flight, taking a
        // `bynk` `ParOptions { limit }`; the plain forms are unchanged.
        ParForEachWith = "parForEachWith", "parForEachWith(f: T -> Effect[()], options: ParOptions) -> Effect[()]";
        ParTraverseWith = "parTraverseWith", "parTraverseWith(f: T -> Effect[U], options: ParOptions) -> Effect[List[U]]";
        ParTraverseAllWith = "parTraverseAllWith", "parTraverseAllWith(f: T -> Effect[Result[U, E]], options: ParOptions) -> Effect[List[Result[U, E]]]";
        ParTraverseTryWith = "parTraverseTryWith", "parTraverseTryWith(f: T -> Effect[Result[U, E]], options: ParOptions) -> Effect[Result[List[U], E]]";
        // #2806: pure `Result` accumulation over a `List[Result[T, E]]` receiver.
        SequenceAll = "sequenceAll", "sequenceAll() -> Result[List[T], List[E]]";
        SequenceTry = "sequenceTry", "sequenceTry() -> Result[List[T], E]";
        // v0.88 (ADR 0116): eager in-memory builders + terminals.
        Map = "map", "map(f: T -> U) -> List[U]";
        Filter = "filter", "filter(p: T -> Bool) -> List[T]";
        FlatMap = "flatMap", "flatMap(f: T -> List[U]) -> List[U]";
        SortBy = "sortBy", "sortBy(key: T -> K) -> List[T]";
        SortByDescending = "sortByDescending", "sortByDescending(key: T -> K) -> List[T]";
        SortWith = "sortWith", "sortWith(compare: (T, T) -> Ordering) -> List[T]";
        Take = "take", "take(n: Int) -> List[T]";
        Skip = "skip", "skip(n: Int) -> List[T]";
        Distinct = "distinct", "distinct() -> List[T]";
        DistinctBy = "distinctBy", "distinctBy(key: T -> K) -> List[T]";
        Count = "count", "count() -> Int, or count(p: T -> Bool) -> Int";
        Any = "any", "any(p: T -> Bool) -> Bool";
        All = "all", "all(p: T -> Bool) -> Bool";
        First = "first", "first() -> Option[T]";
        FirstOrElse = "firstOrElse", "firstOrElse(default: T) -> T";
        // #3962 (with #3828): the rest of `bynk.list`'s vocabulary, and the three
        // everyday methods that existed nowhere.
        Reverse = "reverse", "reverse() -> List[T]";
        Find = "find", "find(p: T -> Bool) -> Option[T]";
        Contains = "contains", "contains(item: T) -> Bool";
        Last = "last", "last() -> Option[T]";
        LastOrElse = "lastOrElse", "lastOrElse(default: T) -> T";
        IsEmpty = "isEmpty", "isEmpty() -> Bool";
        Sort = "sort", "sort() -> List[T]";
        Traverse = "traverse", "traverse(f: T -> Effect[U]) -> Effect[List[U]]";
        Sum = "sum", "sum(key: T -> K) -> K";
        Min = "min", "min(key: T -> K) -> Option[K], or min() -> Option[T] for an Ord element";
        Max = "max", "max(key: T -> K) -> Option[K], or max() -> Option[T] for an Ord element";
        Average = "average", "average(key: T -> K) -> Option[Float]";
        // #3963: the combinators a language without tuples needs most.
        FilterMap = "filterMap", "filterMap(f: T -> Option[U]) -> List[U]";
        ZipWith = "zipWith", "zipWith(other: List[U], f: (T, U) -> V) -> List[V]";
        TakeWhile = "takeWhile", "takeWhile(p: T -> Bool) -> List[T]";
        SkipWhile = "skipWhile", "skipWhile(p: T -> Bool) -> List[T]";
        // v0.94 (ADR 0116/0120): joins & grouping, combiner form (eager — mirrors
        // QUERY_METHODS' lazy `Query[V]` forms below, but returns `List[V]`).
        JoinOn = "joinOn", "joinOn(other: List[U], left: T -> K, right: U -> K, into: (T, U) -> V) -> List[V]";
        LeftJoin = "leftJoin", "leftJoin(other: List[U], left: T -> K, right: U -> K, into: (T, Option[U]) -> V) -> List[V]";
        Join = "join", "join(other: List[U], on: (T, U) -> Bool, into: (T, U) -> V) -> List[V], or join(sep: String) -> String for a List[String]";
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
        // #3961: the set algebra and the `List`-shaped vocabulary, as methods.
        Union = "union", "union(other: Set[T]) -> Set[T]";
        Intersect = "intersect", "intersect(other: Set[T]) -> Set[T]";
        Difference = "difference", "difference(other: Set[T]) -> Set[T]";
        IsSubsetOf = "isSubsetOf", "isSubsetOf(other: Set[T]) -> Bool";
        IsEmpty = "isEmpty", "isEmpty() -> Bool";
        Filter = "filter", "filter(p: T -> Bool) -> Set[T]";
        Map = "map", "map(f: T -> U) -> Set[U]";
        Fold = "fold", "fold(init: U, step: (U, T) -> U) -> U";
    }
/// `Map[K, V]` (v0.20b).
    MapOp {
        Length = "length", "length() -> Int";
        Keys = "keys", "keys() -> List[K]";
        Values = "values", "values() -> List[V]";
        Get = "get", "get(key: K) -> Option[V]";
        Insert = "insert", "insert(key: K, value: V) -> Map[K, V]";
        // #3961: removal, membership, entry access and the `List`-shaped vocabulary.
        Remove = "remove", "remove(key: K) -> Map[K, V]";
        Contains = "contains", "contains(key: K) -> Bool";
        GetOrElse = "getOrElse", "getOrElse(key: K, default: V) -> V";
        IsEmpty = "isEmpty", "isEmpty() -> Bool";
        Entries = "entries", "entries() -> List[MapEntry[K, V]]";
        Filter = "filter", "filter(p: (K, V) -> Bool) -> Map[K, V]";
        MapValues = "mapValues", "mapValues(f: V -> U) -> Map[K, U]";
        Fold = "fold", "fold(init: U, step: (U, K, V) -> U) -> U";
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
        JoinOn = "joinOn", "joinOn(other: Query[U], left: T -> K, right: U -> K, into: (T, U) -> V) -> Query[V]";
        LeftJoin = "leftJoin", "leftJoin(other: Query[U], left: T -> K, right: U -> K, into: (T, Option[U]) -> V) -> Query[V]";
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
        ParForEach = "parForEach", "parForEach(f: T -> Effect[()]) -> Effect[()]";
        TraverseAll = "traverseAll", "traverseAll(f: T -> Effect[Result[U, E]]) -> Effect[List[Result[U, E]]]";
        ParTraverseAll = "parTraverseAll", "parTraverseAll(f: T -> Effect[Result[U, E]]) -> Effect[List[Result[U, E]]]";
        TraverseTry = "traverseTry", "traverseTry(f: T -> Effect[Result[U, E]]) -> Effect[Result[List[U], E]]";
        ParTraverseTry = "parTraverseTry", "parTraverseTry(f: T -> Effect[Result[U, E]]) -> Effect[Result[List[U], E]]";
        @hidden
        // #4177: the old spelling of `parForEach` on a query. It is dispatched so the checker
        // can report the rename with a fix; it is not offered and never reaches lowering.
        ParTraverse = "parTraverse";
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
        // #3978: the everyday methods the kernel lacked. Widths and counts are in
        // code points (padding) or code units (`lastIndexOf`, like `indexOf`).
        IsEmpty = "isEmpty", "isEmpty() -> Bool";
        TrimStart = "trimStart", "trimStart() -> String";
        TrimEnd = "trimEnd", "trimEnd() -> String";
        PadStart = "padStart", "padStart(width: Int, fill: String) -> String";
        PadEnd = "padEnd", "padEnd(width: Int, fill: String) -> String";
        Repeat = "repeat", "repeat(n: Int) -> String";
        Lines = "lines", "lines() -> List[String]";
        LastIndexOf = "lastIndexOf", "lastIndexOf(sub: String) -> Option[Int]";
        ReplaceFirst = "replaceFirst", "replaceFirst(from: String, to: String) -> String";
        // K6c (#2600): regex methods over a string-literal pattern.
        MatchesRegex = "matchesRegex", "matchesRegex(pattern: String) -> Bool";
        ContainsRegex = "containsRegex", "containsRegex(pattern: String) -> Bool";
        FindRegex = "findRegex", "findRegex(pattern: String) -> Option[List[Option[String]]]";
        FindAllRegex = "findAllRegex", "findAllRegex(pattern: String) -> List[List[Option[String]]]";
        // #3979: the named capture groups of the first match, keyed by name.
        FindRegexNamed = "findRegexNamed", "findRegexNamed(pattern: String) -> Option[Map[String, Option[String]]]";
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
        // #3948: the whole-unit conversions round towards zero.
        ToSeconds = "toSeconds", "toSeconds() -> Int (whole seconds, rounding towards zero)";
        ToMinutes = "toMinutes", "toMinutes() -> Int (whole minutes, rounding towards zero)";
        ToHours = "toHours", "toHours() -> Int (whole hours, rounding towards zero)";
        ToDays = "toDays", "toDays() -> Int (whole days, rounding towards zero)";
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
/// #3955: the universal order, on every `Ord` receiver: `a.compare(b)` answers an
/// `Ordering`. It is not a per-type kernel, so it has its own receiver kind.
    OrdOp {
        Compare = "compare", "compare(other: T) -> Ordering";
    }
/// The `Bytes` kernel (v0.110, ADR 0142). Equality is an operator (D4, content
/// compare); the kernel is length plus the String-interop bridge (D3). #2016
/// (standard-library K1) adds `concat`, `slice`, `compare` (a method: `Bytes`
/// stays unorderable), hex and base64url.
    BytesOp {
        Length = "length", "length() -> Int";
        Concat = "concat", "concat(other: Bytes) -> Bytes";
        Slice = "slice", "slice(start: Int, end: Int) -> Bytes";
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
/// `HttpResult[T]` (#3277): attach a cookie to a response, clear one, or read the
/// cookies a result carries (a test's view of what the router will send).
    HttpResultOp {
        WithCookie = "withCookie", "withCookie(c: Cookie) -> HttpResult[T]";
        ClearCookie = "clearCookie", "clearCookie(c: Cookie) -> HttpResult[T]";
        Cookies = "cookies", "cookies() -> List[Cookie]";
    }
/// `RequestQuery` (#2854): an HTTP handler's read-only view of the request's
/// query string.
    RequestQueryOp {
        Get = "get", "get(name: String) -> Option[String]";
        All = "all", "all(name: String) -> List[String]";
        Params = "params", "params() -> Map[String, List[String]]";
    }
/// `RequestCookies` (#3277): an HTTP handler's read-only view of the request's
/// cookies, for a name a record field cannot spell (`__Host-sid`).
    RequestCookiesOp {
        Get = "get", "get(name: String) -> Option[String]";
        All = "all", "all(name: String) -> List[String]";
        Names = "names", "names() -> List[String]";
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
/// `Effect[T]` (#2683, #2711, #3437): `within`, `race` and `detach`, on every `Effect`.
    EffectOp {
        // #2683: `e.within(d)` races `e` against a deadline and hands the
        // abandoned effect to the execution context's `waitUntil`; it is never
        // cancelled, so a `TimedOut` does not mean the work did not happen.
        Within = "within", "within(d: Duration) -> Effect[Result[T, TimedOut]]";
        // #2711: `a.race(b)`: the first side to succeed wins; the other is handed,
        // uncancelled, to the execution context's `waitUntil`, so its side effects
        // still happen. Meant for idempotent work.
        Race = "race", "race(other: Effect[T]) -> Effect[T]";
        // #3437: `e.detach()` stops waiting for `e`: it returns once the effect
        // has been handed to the runtime, not once it has finished. A fault or
        // an `Err` in it is reported by the runtime, never lost.
        Detach = "detach", "detach() -> Effect[()]";
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
    HttpResult(HttpResultOp),
    RequestQuery(RequestQueryOp),
    RequestCookies(RequestCookiesOp),
    TokenClaims(TokenClaimsOp),
    Bytes(BytesOp),
    /// #3955: `compare` on any `Ord` value.
    Ord(OrdOp),
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
            KernelOp::HttpResult(o) => o.name(),
            KernelOp::RequestQuery(o) => o.name(),
            KernelOp::RequestCookies(o) => o.name(),
            KernelOp::TokenClaims(o) => o.name(),
            KernelOp::Bytes(o) => o.name(),
            KernelOp::Ord(o) => o.name(),
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

// ---------------------------------------------------------------------------
// Static intrinsics (1A.33, Decision B)
// ---------------------------------------------------------------------------

kernel_ops! {
/// `List.empty()`.
    ListStatic {
        Empty = "empty", "List.empty() -> List[T]";
    }
/// `Map.empty()`.
    MapStatic {
        Empty = "empty", "Map.empty() -> Map[K, V]";
        Of = "of", "Map.of(entries: List[MapEntry[K, V]]) -> Map[K, V]";
    }
/// `Set.empty()` / `Set.of(xs)` (K6a, #2596).
    SetStatic {
        Empty = "empty", "Set.empty() -> Set[T]";
        Of = "of", "Set.of(items: List[T]) -> Set[T]";
    }
/// `Int.parse(s)` (v0.22a).
    IntStatic {
        Parse = "parse", "Int.parse(s: String) -> Option[Int]";
    }
/// `Float.parse(s)`, and the K6b (#2598) constants `Float.pi()` / `Float.e()`.
    FloatStatic {
        Parse = "parse", "Float.parse(s: String) -> Option[Float]";
        Pi = "pi", "Float.pi() -> Float";
        E = "e", "Float.e() -> Float";
    }
/// `Decimal.parse(s)` / `Decimal.fromInt(n)` (#2622).
    DecimalStatic {
        Parse = "parse", "Decimal.parse(s: String) -> Option[Decimal]";
        FromInt = "fromInt", "Decimal.fromInt(n: Int) -> Decimal";
    }
/// `Duration.millis(n)` (v0.86, ADR 0112).
    DurationStatic {
        Millis = "millis", "Duration.millis(n: Int) -> Duration";
    }
/// `Instant.fromEpochMillis(n)` / `Instant.fromIso(s)`.
    InstantStatic {
        FromEpochMillis = "fromEpochMillis", "Instant.fromEpochMillis(n: Int) -> Instant";
        FromIso = "fromIso", "Instant.fromIso(s: String) -> Option[Instant]";
    }
/// The `Bytes` static constructors (v0.110, ADR 0142 D2).
    BytesStatic {
        FromUtf8 = "fromUtf8", "Bytes.fromUtf8(s: String) -> Bytes";
        FromBase64 = "fromBase64", "Bytes.fromBase64(s: String) -> Option[Bytes]";
        FromHex = "fromHex", "Bytes.fromHex(s: String) -> Option[Bytes]";
        FromBase64Url = "fromBase64Url", "Bytes.fromBase64Url(s: String) -> Option[Bytes]";
        Empty = "empty", "Bytes.empty() -> Bytes";
    }
/// `TokenClaims.of(lines)` (#2380, L5).
    TokenClaimsStatic {
        Of = "of", "TokenClaims.of(lines: List[String]) -> TokenClaims";
    }
/// `RequestHeaders.of(lines)` / `RequestHeaders.empty()` (#2341, L4).
    RequestHeadersStatic {
        Of = "of", "RequestHeaders.of(lines: List[String]) -> RequestHeaders";
        Empty = "empty", "RequestHeaders.empty() -> RequestHeaders";
    }
/// `RequestQuery.of(pairs)` / `RequestQuery.empty()` (#2854).
    RequestQueryStatic {
        Of = "of", "RequestQuery.of(pairs: List[String]) -> RequestQuery";
        Empty = "empty", "RequestQuery.empty() -> RequestQuery";
    }
/// `RequestCookies.of(pairs)` / `RequestCookies.empty()` (#3277).
    RequestCookiesStatic {
        Of = "of", "RequestCookies.of(pairs: List[String]) -> RequestCookies";
        Empty = "empty", "RequestCookies.empty() -> RequestCookies";
    }
/// `Stream.of(xs)` (v0.100).
    StreamStatic {
        Of = "of", "Stream.of(items: List[T]) -> Stream[T]";
    }
/// The typed `Json` codec statics (v0.22b, ADR 0045).
    JsonStatic {
        Encode = "encode", "Json.encode(value: T) -> String";
        EncodePlain = "encodePlain", "Json.encodePlain(value: T) -> String";
        Decode = "decode", "Json.decode[T](s: String) -> Result[T, DecodeError]";
        DecodeSql = "decodeSql", "Json.decodeSql[T](row: String) -> Result[T, DecodeError]";
    }
/// `Yaml.decode[T]` (#3214).
    YamlStatic {
        Decode = "decode", "Yaml.decode[T](s: String) -> Result[T, DecodeError]";
    }
/// `Form.decode[T]` / `Form.encode(v)` (#3276).
    FormStatic {
        Encode = "encode", "Form.encode(value: T) -> String";
        Decode = "decode", "Form.decode[T](s: String) -> Result[T, DecodeError]";
    }
/// `TestConnection[F]()`, the test-side connection constructor.
    TestConnectionStatic {
        New = "new", "TestConnection[F]() -> TestConnection[F]";
    }
/// `run.id`, a test run's identity (#2138).
    TestRunStatic {
        Id = "id", "run.id -> String";
    }
}

/// The namespace a [`crate::checker::Callee::Intrinsic`] belongs to: the built-in
/// type (or, for `TestRun`, the test-body value) a static operation hangs off.
/// Closed: every namespace the checker records is a variant, so a consumer
/// matches exhaustively rather than comparing against a `builtin_names` constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntrinsicNs {
    Json,
    Yaml,
    Form,
    List,
    Map,
    Set,
    Int,
    Float,
    Decimal,
    Duration,
    Instant,
    Bytes,
    TokenClaims,
    RequestHeaders,
    RequestQuery,
    RequestCookies,
    Stream,
    /// An `HttpResult` variant reference or construction (`NotFound`, `HttpResult.Ok(x)`).
    HttpResult,
    /// A `QueueResult` variant (`Ack`, `Retry(reason)`).
    QueueResult,
    TestConnection,
    /// `run.id` ([`crate::checker::RUN_ID_NS`]).
    TestRun,
}

impl IntrinsicNs {
    /// The namespace's source spelling (the `builtin_names` constant it replaces).
    pub const fn name(self) -> &'static str {
        use crate::builtin_names::types as t;
        match self {
            IntrinsicNs::Json => t::JSON,
            IntrinsicNs::Yaml => t::YAML,
            IntrinsicNs::Form => t::FORM,
            IntrinsicNs::List => t::LIST,
            IntrinsicNs::Map => t::MAP,
            IntrinsicNs::Set => t::SET,
            IntrinsicNs::Int => t::INT,
            IntrinsicNs::Float => t::FLOAT,
            IntrinsicNs::Decimal => t::DECIMAL,
            IntrinsicNs::Duration => t::DURATION,
            IntrinsicNs::Instant => t::INSTANT,
            IntrinsicNs::Bytes => t::BYTES,
            IntrinsicNs::TokenClaims => t::TOKEN_CLAIMS,
            IntrinsicNs::RequestHeaders => t::REQUEST_HEADERS,
            IntrinsicNs::RequestQuery => t::REQUEST_QUERY,
            IntrinsicNs::RequestCookies => t::REQUEST_COOKIES,
            IntrinsicNs::Stream => t::STREAM,
            IntrinsicNs::HttpResult => t::HTTP_RESULT,
            IntrinsicNs::QueueResult => t::QUEUE_RESULT,
            IntrinsicNs::TestConnection => "TestConnection",
            IntrinsicNs::TestRun => crate::checker::RUN_ID_NS,
        }
    }
}

/// A resolved static intrinsic: the operation a [`crate::checker::Callee::Intrinsic`]
/// names, in the closed vocabulary of its namespace. Built by the checker once the
/// call's method name is known to belong to the namespace, so an unknown method
/// never gets a callee.
///
/// The two sum-variant namespaces (`HttpResult`, `QueueResult`) carry the variant
/// table entry the parser already owns (`bynk_syntax::ast::HTTP_VARIANTS` /
/// `QUEUE_VARIANTS`), which is a closed static table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticOp {
    List(ListStatic),
    Map(MapStatic),
    Set(SetStatic),
    Int(IntStatic),
    Float(FloatStatic),
    Decimal(DecimalStatic),
    Duration(DurationStatic),
    Instant(InstantStatic),
    Bytes(BytesStatic),
    TokenClaims(TokenClaimsStatic),
    RequestHeaders(RequestHeadersStatic),
    RequestQuery(RequestQueryStatic),
    RequestCookies(RequestCookiesStatic),
    Stream(StreamStatic),
    Json(JsonStatic),
    Yaml(YamlStatic),
    Form(FormStatic),
    HttpResult(bynk_syntax::ast::HttpVariant),
    QueueResult(bynk_syntax::ast::QueueVariant),
    TestConnection(TestConnectionStatic),
    TestRun(TestRunStatic),
}

impl StaticOp {
    /// The namespace this operation belongs to.
    pub const fn ns(self) -> IntrinsicNs {
        match self {
            StaticOp::List(_) => IntrinsicNs::List,
            StaticOp::Map(_) => IntrinsicNs::Map,
            StaticOp::Set(_) => IntrinsicNs::Set,
            StaticOp::Int(_) => IntrinsicNs::Int,
            StaticOp::Float(_) => IntrinsicNs::Float,
            StaticOp::Decimal(_) => IntrinsicNs::Decimal,
            StaticOp::Duration(_) => IntrinsicNs::Duration,
            StaticOp::Instant(_) => IntrinsicNs::Instant,
            StaticOp::Bytes(_) => IntrinsicNs::Bytes,
            StaticOp::TokenClaims(_) => IntrinsicNs::TokenClaims,
            StaticOp::RequestHeaders(_) => IntrinsicNs::RequestHeaders,
            StaticOp::RequestQuery(_) => IntrinsicNs::RequestQuery,
            StaticOp::RequestCookies(_) => IntrinsicNs::RequestCookies,
            StaticOp::Stream(_) => IntrinsicNs::Stream,
            StaticOp::Json(_) => IntrinsicNs::Json,
            StaticOp::Yaml(_) => IntrinsicNs::Yaml,
            StaticOp::Form(_) => IntrinsicNs::Form,
            StaticOp::HttpResult(_) => IntrinsicNs::HttpResult,
            StaticOp::QueueResult(_) => IntrinsicNs::QueueResult,
            StaticOp::TestConnection(_) => IntrinsicNs::TestConnection,
            StaticOp::TestRun(_) => IntrinsicNs::TestRun,
        }
    }

    /// The operation's source spelling (a variant's name for the sum namespaces).
    pub const fn name(self) -> &'static str {
        match self {
            StaticOp::List(o) => o.name(),
            StaticOp::Map(o) => o.name(),
            StaticOp::Set(o) => o.name(),
            StaticOp::Int(o) => o.name(),
            StaticOp::Float(o) => o.name(),
            StaticOp::Decimal(o) => o.name(),
            StaticOp::Duration(o) => o.name(),
            StaticOp::Instant(o) => o.name(),
            StaticOp::Bytes(o) => o.name(),
            StaticOp::TokenClaims(o) => o.name(),
            StaticOp::RequestHeaders(o) => o.name(),
            StaticOp::RequestQuery(o) => o.name(),
            StaticOp::RequestCookies(o) => o.name(),
            StaticOp::Stream(o) => o.name(),
            StaticOp::Json(o) => o.name(),
            StaticOp::Yaml(o) => o.name(),
            StaticOp::Form(o) => o.name(),
            StaticOp::HttpResult(v) => v.name,
            StaticOp::QueueResult(v) => v.name,
            StaticOp::TestConnection(o) => o.name(),
            StaticOp::TestRun(o) => o.name(),
        }
    }

    /// The operation spelt `name` in namespace `ns`, if the namespace has one.
    pub fn resolve(ns: IntrinsicNs, name: &str) -> Option<StaticOp> {
        Some(match ns {
            IntrinsicNs::List => StaticOp::List(ListStatic::from_name(name)?),
            IntrinsicNs::Map => StaticOp::Map(MapStatic::from_name(name)?),
            IntrinsicNs::Set => StaticOp::Set(SetStatic::from_name(name)?),
            IntrinsicNs::Int => StaticOp::Int(IntStatic::from_name(name)?),
            IntrinsicNs::Float => StaticOp::Float(FloatStatic::from_name(name)?),
            IntrinsicNs::Decimal => StaticOp::Decimal(DecimalStatic::from_name(name)?),
            IntrinsicNs::Duration => StaticOp::Duration(DurationStatic::from_name(name)?),
            IntrinsicNs::Instant => StaticOp::Instant(InstantStatic::from_name(name)?),
            IntrinsicNs::Bytes => StaticOp::Bytes(BytesStatic::from_name(name)?),
            IntrinsicNs::TokenClaims => StaticOp::TokenClaims(TokenClaimsStatic::from_name(name)?),
            IntrinsicNs::RequestHeaders => {
                StaticOp::RequestHeaders(RequestHeadersStatic::from_name(name)?)
            }
            IntrinsicNs::RequestQuery => {
                StaticOp::RequestQuery(RequestQueryStatic::from_name(name)?)
            }
            IntrinsicNs::RequestCookies => {
                StaticOp::RequestCookies(RequestCookiesStatic::from_name(name)?)
            }
            IntrinsicNs::Stream => StaticOp::Stream(StreamStatic::from_name(name)?),
            IntrinsicNs::Json => StaticOp::Json(JsonStatic::from_name(name)?),
            IntrinsicNs::Yaml => StaticOp::Yaml(YamlStatic::from_name(name)?),
            IntrinsicNs::Form => StaticOp::Form(FormStatic::from_name(name)?),
            IntrinsicNs::HttpResult => StaticOp::HttpResult(bynk_syntax::ast::http_variant(name)?),
            IntrinsicNs::QueueResult => {
                StaticOp::QueueResult(bynk_syntax::ast::queue_variant(name)?)
            }
            IntrinsicNs::TestConnection => {
                StaticOp::TestConnection(TestConnectionStatic::from_name(name)?)
            }
            IntrinsicNs::TestRun => StaticOp::TestRun(TestRunStatic::from_name(name)?),
        })
    }
}

// ---------------------------------------------------------------------------
// Store entry operations (1A.33, Decision C)
// ---------------------------------------------------------------------------
//
// `MapOp`/`SetOp` above are the *value* kernels' vocabularies (`length`, `keys`,
// `insert`); a `store` field's entry operations are a different vocabulary
// (`put`, `remove`, `upsert`), enumerated for hover in [`crate::store_ops`]. They get
// their own enums here, declared once with the same macro.

kernel_ops! {
/// The entry operations of a `store Map[K, V]` and a `store Cache[K, V]` (the
/// same names; a cache's apply expiry). ADR 0110, ADR 0113.
    MapStoreOp {
        Put = "put", "put(key: K, value: V) -> Effect[()]";
        Get = "get", "get(key: K) -> Effect[Option[V]]";
        Remove = "remove", "remove(key: K) -> Effect[()]";
        Contains = "contains", "contains(key: K) -> Effect[Bool]";
        Size = "size", "size() -> Effect[Int]";
        Update = "update", "update(key: K, f: (V) -> V) -> Effect[()]";
        Upsert = "upsert", "upsert(key: K, initial: V, f: (V) -> V) -> Effect[()]";
    }
/// The entry operations of a `store Set[T]` (v0.83).
    SetStoreOp {
        Add = "add", "add(item: T) -> Effect[()]";
        Remove = "remove", "remove(item: T) -> Effect[()]";
        Contains = "contains", "contains(item: T) -> Effect[Bool]";
        Size = "size", "size() -> Effect[Int]";
    }
/// The one method-shaped operation of a `store Cell[T]` (v0.98, ADR 0125).
    CellStoreOp {
        Update = "update", "update(f: (T) -> T) -> Effect[()]";
    }
/// The one effectful write of a `store Log[T]` (v0.95, ADR 0121).
    LogStoreOp {
        Append = "append", "append(entry: T) -> Effect[()]";
    }
/// The time-window roots of a `store Log[T]`: they lift the log into a lazy `Query[T]`.
    LogWindowOp {
        Since = "since", "since(start: Instant) -> Query[T]";
        Before = "before", "before(end: Instant) -> Query[T]";
        Between = "between", "between(start: Instant, end: Instant) -> Query[T]";
        Recent = "recent", "recent(count: Int) -> Query[T]";
        Reversed = "reversed", "reversed() -> Query[T]";
    }
}

/// A resolved effectful entry operation on a `store` field, carried in
/// [`crate::checker::Callee::Store`]: the field's kind and the operation on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreEntryOp {
    Map(MapStoreOp),
    Cache(MapStoreOp),
    Set(SetStoreOp),
    Cell(CellStoreOp),
    Log(LogStoreOp),
}

impl StoreEntryOp {
    /// The operation's source spelling.
    pub const fn name(self) -> &'static str {
        match self {
            StoreEntryOp::Map(o) | StoreEntryOp::Cache(o) => o.name(),
            StoreEntryOp::Set(o) => o.name(),
            StoreEntryOp::Cell(o) => o.name(),
            StoreEntryOp::Log(o) => o.name(),
        }
    }

    /// Whether the operation writes the field (a write the handler's implicit
    /// commit persists). The same flags [`crate::store_ops`] carries for hover.
    pub fn mutates(self) -> bool {
        crate::store_ops::is_mutating(self.kind_head(), self.name())
    }

    /// The storage kind's head identifier (`"Map"`, `"Cache"`, …).
    const fn kind_head(self) -> &'static str {
        match self {
            StoreEntryOp::Map(_) => "Map",
            StoreEntryOp::Cache(_) => "Cache",
            StoreEntryOp::Set(_) => "Set",
            StoreEntryOp::Cell(_) => "Cell",
            StoreEntryOp::Log(_) => "Log",
        }
    }
}

/// A resolved lifting operation on a `store` field, carried in
/// [`crate::checker::Callee::Query`]: a query builder or terminal ([`QueryOp`]), or
/// a `Log`'s time-window root ([`LogWindowOp`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreQueryOp {
    Query(QueryOp),
    LogWindow(LogWindowOp),
}

impl StoreQueryOp {
    /// The operation's source spelling.
    pub const fn name(self) -> &'static str {
        match self {
            StoreQueryOp::Query(o) => o.name(),
            StoreQueryOp::LogWindow(o) => o.name(),
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
            HttpResultOp,
            RequestQueryOp,
            RequestCookiesOp,
            TokenClaimsOp,
            BytesOp,
            OrdOp,
            DecimalOp,
            StringOp,
            OptionOp,
            ResultOp,
            EffectOp,
            EffectResultOp,
            ListStatic,
            MapStatic,
            SetStatic,
            IntStatic,
            FloatStatic,
            DecimalStatic,
            DurationStatic,
            InstantStatic,
            BytesStatic,
            TokenClaimsStatic,
            RequestHeadersStatic,
            RequestQueryStatic,
            RequestCookiesStatic,
            StreamStatic,
            JsonStatic,
            YamlStatic,
            FormStatic,
            TestConnectionStatic,
            TestRunStatic,
            MapStoreOp,
            SetStoreOp,
            CellStoreOp,
            LogStoreOp,
            LogWindowOp,
        );
    }

    /// The display signature of `name` in a completion/hover table.
    fn signature(table: &[KernelMethod], name: &str) -> &'static str {
        table
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("no `{name}` in the table"))
            .signature
    }

    #[test]
    fn equi_joins_display_their_two_key_functions() {
        // `joinOn`/`leftJoin` are equi-joins checked with a left and a right
        // key function (`check_equi_join`), not a predicate; only `join` takes
        // `on: (T, U) -> Bool`.
        for (table, kind) in [(ListOp::TABLE, "List"), (QueryOp::TABLE, "Query")] {
            assert_eq!(
                signature(table, "joinOn"),
                format!(
                    "joinOn(other: {kind}[U], left: T -> K, right: U -> K, into: (T, U) -> V) -> {kind}[V]"
                ),
            );
            assert_eq!(
                signature(table, "leftJoin"),
                format!(
                    "leftJoin(other: {kind}[U], left: T -> K, right: U -> K, into: (T, Option[U]) -> V) -> {kind}[V]"
                ),
            );
            assert!(signature(table, "join").contains("on: (T, U) -> Bool"));
        }
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
