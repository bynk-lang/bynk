//! The scalar kernels (back-half paydown 1A.29, #3730, Decision D): `Int`, `Float`,
//! `Duration`, `Instant`, `Bytes`, `String` and `Decimal`. Moved out of `lower.rs` byte
//! for byte, then converted to nodes.

use bynk_check::kernel_op::{BytesOp, DecimalOp, DurationOp, FloatOp, InstantOp, IntOp, StringOp};
use bynk_syntax::ast::{Expr, ExprKind};

use super::super::lower::{Lowered, Pre};
use super::super::lower_ctx::LowerCtx;

/// v0.21/v0.22a: lower a built-in numeric kernel method. `toFloat` is the
/// identity at runtime (the Int/Float distinction is erased); everything
/// else maps onto `Math.*` / `Number.*`.
pub(in crate::emitter) fn lower_int_kernel(
    receiver: &Expr,
    op: IntOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (op, args) {
        (IntOp::ToFloat, []) => Some(pre.lower(receiver, cx)),
        (IntOp::Abs, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Math.abs({recv})"))
        }
        (IntOp::Min | IntOp::Max, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            Some(format!("globalThis.Math.{}({recv}, {other})", op.name()))
        }
        (IntOp::Clamp, [lo, hi]) => {
            let recv = pre.lower(receiver, cx);
            let lo = pre.lower(lo, cx);
            let hi = pre.lower(hi, cx);
            Some(format!(
                "globalThis.Math.min(globalThis.Math.max({recv}, {lo}), {hi})"
            ))
        }
        // v0.42 (ADR 0074): host number→string — `String(n)` is ECMAScript's
        // Number::toString (shortest round-trip; `1e21`/`Infinity`/`NaN` as the
        // host renders them). The normative contract is the platform's.
        (IntOp::ToString, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("String({recv})"))
        }
        // K6b (#2598): the `Int` division companions and exact power, through
        // `int.ts` helpers so their fault branches stay out of coverage.
        (
            IntOp::Div
            | IntOp::Rem
            | IntOp::Mod
            | IntOp::Pow
            | IntOp::And
            | IntOp::Or
            | IntOp::Xor
            | IntOp::AndNot
            | IntOp::Shl
            | IntOp::Shr,
            [other],
        ) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            let helper = match op {
                IntOp::Div => "__bynkIntDivFloor",
                IntOp::Rem => "__bynkIntRem",
                IntOp::Mod => "__bynkIntMod",
                // #2621: the bitwise operations, split hi/lo in the runtime.
                IntOp::And => "__bynkIntAnd",
                IntOp::Or => "__bynkIntOr",
                IntOp::Xor => "__bynkIntXor",
                IntOp::AndNot => "__bynkIntAndNot",
                IntOp::Shl => "__bynkIntShl",
                IntOp::Shr => "__bynkIntShr",
                _ => "__bynkIntPow",
            };
            cx.note_int();
            Some(format!("{helper}({recv}, {other})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// The `Float` kernel's lowering (see [`lower_int_kernel`]).
pub(in crate::emitter) fn lower_float_kernel(
    receiver: &Expr,
    op: FloatOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (op, args) {
        (FloatOp::Abs, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Math.abs({recv})"))
        }
        // #1657 (runtime-semantics track §3.4, decided in S8): a Float→`Int`
        // conversion traps when the result is not a safe integer — a
        // non-finite `Float`, or one past ±2^53 — rather than producing an
        // `Int` outside the domain (`__bynkToInt`). The same posture as `Int`
        // division by zero.
        // Returning `Option[Int]` was rejected: a signature change to four
        // kernel methods.
        (FloatOp::Round | FloatOp::Floor | FloatOp::Ceil | FloatOp::Truncate, []) => {
            let recv = pre.lower(receiver, cx);
            let f = if op == FloatOp::Truncate {
                "trunc"
            } else {
                op.name()
            };
            cx.note_int();
            Some(format!(
                "__bynkToInt(globalThis.Math.{f}({recv}), \"{}\")",
                op.name()
            ))
        }
        (FloatOp::Min | FloatOp::Max, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            Some(format!("globalThis.Math.{}({recv}, {other})", op.name()))
        }
        (FloatOp::Clamp, [lo, hi]) => {
            let recv = pre.lower(receiver, cx);
            let lo = pre.lower(lo, cx);
            let hi = pre.lower(hi, cx);
            Some(format!(
                "globalThis.Math.min(globalThis.Math.max({recv}, {lo}), {hi})"
            ))
        }
        (FloatOp::IsNaN | FloatOp::IsFinite, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Number.{}({recv})", op.name()))
        }
        (FloatOp::ToString, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("String({recv})"))
        }
        // K6b (#2598): the `Float` functions, straight onto JS `Math`.
        (
            FloatOp::Sqrt
            | FloatOp::Exp
            | FloatOp::Log10
            | FloatOp::Log2
            | FloatOp::Sin
            | FloatOp::Cos
            | FloatOp::Tan
            | FloatOp::Asin
            | FloatOp::Acos
            | FloatOp::Atan,
            [],
        ) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Math.{}({recv})", op.name()))
        }
        (FloatOp::Ln, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Math.log({recv})"))
        }
        (FloatOp::Pow, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            Some(format!("globalThis.Math.pow({recv}, {other})"))
        }
        // `y.atan2(x)`: the angle of the point `(x, y)`.
        (FloatOp::Atan2, [x]) => {
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!("globalThis.Math.atan2({recv}, {x})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.86 (ADR 0112): lower a `Duration` kernel method. `toMillis` is the
/// identity (a `Duration` lowers to its milliseconds); `toString` renders it.
pub(in crate::emitter) fn lower_duration_kernel(
    receiver: &Expr,
    op: DurationOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (op, args) {
        (DurationOp::ToMillis, []) => Some(pre.lower(receiver, cx)),
        (DurationOp::ToString, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("String({recv})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.90 (ADR 0114): lower an `Instant` kernel method. `toEpochMillis` is the
/// identity (an `Instant` lowers to its epoch milliseconds); `toString` renders
/// it.
pub(in crate::emitter) fn lower_instant_kernel(
    receiver: &Expr,
    op: InstantOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (op, args) {
        (InstantOp::ToEpochMillis, []) => Some(pre.lower(receiver, cx)),
        (InstantOp::ToString, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("String({recv})"))
        }
        // K3 (#2592): RFC 3339 in UTC.
        (InstantOp::ToIso, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_time_ops();
            Some(format!("__bynkInstantToIso({recv})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.110 (ADR 0142 D3/D4): lower a `Bytes` kernel method. `length` is the
/// `Uint8Array.length` (octet count, not any string length); `toBase64` is a
/// total encode; `decodeUtf8` is a guarded fatal decode returning `Option`
/// (`None` on an invalid UTF-8 sequence).
pub(in crate::emitter) fn lower_bytes_kernel(
    receiver: &Expr,
    op: BytesOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (op, args) {
        (BytesOp::Length, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).length"))
        }
        (BytesOp::ToBase64, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_bytes();
            Some(format!("__bynkBytesToBase64({recv})"))
        }
        (BytesOp::DecodeUtf8, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_bytes();
            Some(format!("__bynkBytesDecodeUtf8({recv})"))
        }
        // #2016 (standard-library K1). `slice` clamps negative indices to 0 as
        // `String.slice` does, and `Uint8Array.slice` copies, so a slice never
        // aliases a buffer a binding may still hold.
        (BytesOp::Slice, [lo, hi]) => {
            let recv = pre.lower(receiver, cx);
            let lo = pre.lower(lo, cx);
            let hi = pre.lower(hi, cx);
            Some(format!(
                "({recv}).slice(globalThis.Math.max(0, {lo}), globalThis.Math.max(0, {hi}))"
            ))
        }
        (BytesOp::Concat, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            cx.note_bytes_ops();
            Some(format!("__bynkBytesConcat({recv}, {other})"))
        }
        (BytesOp::Compare, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            cx.note_bytes_ops();
            Some(format!("__bynkBytesCompare({recv}, {other})"))
        }
        (BytesOp::ToHex, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_bytes_ops();
            Some(format!("__bynkBytesToHex({recv})"))
        }
        (BytesOp::ToBase64Url, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_bytes_ops();
            Some(format!("__bynkBytesToBase64Url({recv})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.22a: lower a built-in `String` kernel method (ADR 0046). Pinned
/// semantics: `replace` is replace-**all** (`replaceAll`) and inserts its
/// replacement literally (#1650); `chars()` is
/// code **points** (`[...s]`), not code units; `slice` clamps negative
/// indices to `0` (no TS wrap-around); `indexOf` turns `-1` into `None`.
/// K6c (#2600): a regex method's pattern literal as a `RegExp` construction,
/// its inline flags moved into the flags argument, plus `extra` flags (`g` for
/// a global scan, `y` for a sticky one). `anchored` pins it to the whole
/// string with lookarounds rather than `^…$`, which an `m` flag would turn into
/// line anchors. V8 caches a `RegExp`'s compilation by source and
/// flags, so building one per call compiles each pattern once; a fresh object
/// also keeps no `lastIndex` state between calls.
fn regex_construction(literal: &str, extra: &str, anchored: bool) -> String {
    let (pat, flags) = bynk_check::checker::split_regex_flags(literal);
    let source = if anchored {
        format!("(?<![\\s\\S])(?:{pat})(?![\\s\\S])")
    } else {
        pat
    };
    format!(
        "new globalThis.RegExp(\"{}\", \"{flags}{extra}\")",
        crate::emitter::escape_ts_string(&source)
    )
}

pub(in crate::emitter) fn lower_string_kernel(
    receiver: &Expr,
    op: StringOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    // K6c (#2600): the checker admits a regex method only with a literal
    // pattern; anything else never reaches here.
    let pattern = match args.first().map(|a| &a.kind) {
        Some(ExprKind::StrLit(literal))
            if matches!(
                op,
                StringOp::MatchesRegex
                    | StringOp::ContainsRegex
                    | StringOp::FindRegex
                    | StringOp::FindAllRegex
                    | StringOp::ReplaceRegex
                    | StringOp::SplitRegex
            ) =>
        {
            Some(literal.clone())
        }
        _ => None,
    };
    if let Some(literal) = pattern {
        let recv = pre.lower(receiver, cx);
        let groups = "(__m: globalThis.RegExpMatchArray): Option<string>[] => \
                      [...__m].map((__g: string | undefined): Option<string> => \
                      __g === undefined ? None : Some(__g))";
        let text = match (op, args.len()) {
            (StringOp::MatchesRegex, 1) => Some(format!(
                "{}.test({recv})",
                regex_construction(&literal, "", true)
            )),
            (StringOp::ContainsRegex, 1) => Some(format!(
                "{}.test({recv})",
                regex_construction(&literal, "", false)
            )),
            (StringOp::FindRegex, 1) => Some(format!(
                "((__m: globalThis.RegExpExecArray | null): Option<readonly Option<string>[]> => \
                 __m === null ? None : Some(({groups})(__m)))({}.exec({recv}))",
                regex_construction(&literal, "", false)
            )),
            (StringOp::FindAllRegex, 1) => Some(format!(
                "([...({recv}).matchAll({})].map({groups}) as readonly (readonly Option<string>[])[])",
                regex_construction(&literal, "g", false)
            )),
            (StringOp::ReplaceRegex, 2) => {
                let with = pre.lower(&args[1], cx);
                Some(format!(
                    "({recv}).replace({}, {with})",
                    regex_construction(&literal, "g", false)
                ))
            }
            // JS `split` with a regex, minus the captured groups it splices in:
            // its own algorithm over a sticky scan, an empty match at the last
            // split point advancing by one code unit.
            (StringOp::SplitRegex, 1) => Some(format!(
                "((__s: string, __r: globalThis.RegExp): readonly string[] => {{ \
                 if (__s.length === 0) {{ return __r.exec(__s) === null ? [\"\"] : []; }} \
                 const __out: string[] = []; let __p = 0; let __q = 0; \
                 while (__q < __s.length) {{ __r.lastIndex = __q; const __hit = __r.exec(__s); \
                 const __e = __r.lastIndex; \
                 if (__hit === null || __e === __p) {{ __q++; }} \
                 else {{ __out.push(__s.slice(__p, __q)); __p = __e; __q = __p; }} }} \
                 __out.push(__s.slice(__p)); return __out; }})({recv}, {})",
                regex_construction(&literal, "y", false)
            )),
            _ => None,
        };
        return text.map(|expr| pre.finish_text(expr));
    }
    let text: Option<String> = match (op, args) {
        (StringOp::Length, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).length"))
        }
        (StringOp::Trim, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("{recv}.trim()"))
        }
        (StringOp::ToUpper, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("{recv}.toUpperCase()"))
        }
        (StringOp::ToLower, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("{recv}.toLowerCase()"))
        }
        (StringOp::Chars, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("[...{recv}]"))
        }
        (StringOp::Split, [sep]) => {
            let recv = pre.lower(receiver, cx);
            let sep = pre.lower(sep, cx);
            Some(format!("{recv}.split({sep})"))
        }
        (StringOp::Contains, [sub]) => {
            let recv = pre.lower(receiver, cx);
            let sub = pre.lower(sub, cx);
            Some(format!("{recv}.includes({sub})"))
        }
        (StringOp::StartsWith | StringOp::EndsWith, [sub]) => {
            let recv = pre.lower(receiver, cx);
            let sub = pre.lower(sub, cx);
            Some(format!("{recv}.{}({sub})", op.name()))
        }
        (StringOp::Concat, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            Some(format!("{recv}.concat({other})"))
        }
        (StringOp::Replace, [from, to]) => {
            let recv = pre.lower(receiver, cx);
            let from = pre.lower(from, cx);
            let to = pre.lower(to, cx);
            // #1650: a *string* replacement is `$`-expanded by JS (`$&`, `$1`,
            // `` $` ``, `$'`, `$$`); a function replacer's return value never is,
            // so the replacement is inserted literally. `to` is a pure
            // expression, so its result is the same either way, but it is
            // evaluated once per match (and not at all with no match), so a
            // costly replacement is recomputed.
            Some(format!("{recv}.replaceAll({from}, () => {to})"))
        }
        (StringOp::Slice, [lo, hi]) => {
            let recv = pre.lower(receiver, cx);
            let lo = pre.lower(lo, cx);
            let hi = pre.lower(hi, cx);
            Some(format!(
                "{recv}.slice(globalThis.Math.max(0, {lo}), globalThis.Math.max(0, {hi}))"
            ))
        }
        (StringOp::IndexOf, [sub]) => {
            let recv = pre.lower(receiver, cx);
            let sub = pre.lower(sub, cx);
            Some(format!(
                "((__i: number) => __i < 0 ? None : Some(__i))({recv}.indexOf({sub}))"
            ))
        }
        // K2 (#2594): percent-encoding over the UTF-8 bytes.
        (StringOp::PercentEncode, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_url_ops();
            Some(format!("__bynkPercentEncode({recv})"))
        }
        (StringOp::PercentDecode, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_url_ops();
            Some(format!("__bynkPercentDecode({recv})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// #2622: lower a `Decimal` kernel method to its runtime helper. `round` and
/// `div` carry a rounding mode, read from the `RoundingMode.<mode>` argument
/// and lowered to its name as a string literal.
pub(in crate::emitter) fn lower_decimal_kernel(
    receiver: &Expr,
    op: DecimalOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let mode = |a: &Expr| bynk_syntax::ast::rounding_mode(a).map(|m| format!("\"{}\"", m.name()));
    let text: Option<String> = match (op, args) {
        (DecimalOp::Scale, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).scale"))
        }
        (DecimalOp::ToString, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_decimal("__bynkDecimalToString");
            Some(format!("__bynkDecimalToString({recv})"))
        }
        (DecimalOp::Abs, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_decimal("__bynkDecimalAbs");
            Some(format!("__bynkDecimalAbs({recv})"))
        }
        (DecimalOp::ToInt, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_decimal("__bynkDecimalToInt");
            Some(format!("__bynkDecimalToInt({recv})"))
        }
        (DecimalOp::ToFloat, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_decimal("__bynkDecimalToFloat");
            Some(format!("__bynkDecimalToFloat({recv})"))
        }
        (DecimalOp::Min | DecimalOp::Max, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            let (helper, name) = if op == DecimalOp::Min {
                ("__bynkDecimalMin", "__bynkDecimalMin")
            } else {
                ("__bynkDecimalMax", "__bynkDecimalMax")
            };
            cx.note_decimal(name);
            Some(format!("{helper}({recv}, {other})"))
        }
        (DecimalOp::Round, [scale, m]) => {
            let mode = mode(m)?;
            let recv = pre.lower(receiver, cx);
            let scale = pre.lower(scale, cx);
            cx.note_decimal("__bynkDecimalRound");
            Some(format!("__bynkDecimalRound({recv}, {scale}, {mode})"))
        }
        (DecimalOp::Div, [other, scale, m]) => {
            let mode = mode(m)?;
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            let scale = pre.lower(scale, cx);
            cx.note_decimal("__bynkDecimalDiv");
            Some(format!(
                "__bynkDecimalDiv({recv}, {other}, {scale}, {mode})"
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}
