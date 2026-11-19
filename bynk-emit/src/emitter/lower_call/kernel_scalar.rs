//! The scalar kernels (back-half paydown 1A.29, #3730, Decision D): `Int`, `Float`,
//! `Duration`, `Instant`, `Bytes`, `String` and `Decimal`. Moved out of `lower.rs` byte
//! for byte, then converted to nodes: each arm builds a node when its receiver and
//! arguments are nodes and prints today's text otherwise (`node_or_text`).
//!
//! The arms that stay text are marked `// 1A:`: the identity kernels behind #3770, which a
//! node would change (the printer would bracket the operand), and the one regex arm whose
//! body is a statement block `bynk_ts` cannot yet express in an expression position.

use bynk_check::kernel_op::{BytesOp, DecimalOp, DurationOp, FloatOp, InstantOp, IntOp, StringOp};
use bynk_ts::{TsArrowBody, TsBinaryOp, TsExpr, TsType};

use super::super::lower::{Built, Lowered, LoweredExpr, Pre, node_or_text, str_node};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{arrow, call, id, member, param, spread, take, zero};
use super::super::{Expr, ExprKind, rounding_mode};
use super::kernel::receiver_object;

/// The receiver and then each argument, lowered in order as bridge values.
fn operands(pre: &mut Pre, cx: &mut LowerCtx, parts: &[&Expr]) -> Vec<LoweredExpr> {
    parts.iter().map(|e| pre.lower_node(e, cx)).collect()
}

/// A kernel's exit: the built node, or its text through the one shared text exit.
fn finish(pre: Pre, built: Option<Built>) -> Option<Lowered> {
    built.map(|b| pre.finish_built(b))
}

fn global_math(name: &str) -> TsExpr {
    member(member(id("globalThis"), "Math"), name)
}

/// `globalThis.Math.<name>(…)` over the lowered operands.
fn math_call(name: &str, kids: Vec<LoweredExpr>) -> Built {
    node_or_text(
        kids,
        |n| call(global_math(name), n),
        |t| format!("globalThis.Math.{name}({})", t.join(", ")),
    )
}

/// `globalThis.Math.min(globalThis.Math.max(recv, lo), hi)`.
fn clamp(kids: Vec<LoweredExpr>) -> Built {
    node_or_text(
        kids,
        |n| {
            let [recv, lo, hi] = take::<3, _>(n);
            call(
                global_math("min"),
                vec![call(global_math("max"), vec![recv, lo]), hi],
            )
        },
        |t| {
            format!(
                "globalThis.Math.min(globalThis.Math.max({}, {}), {})",
                t[0], t[1], t[2]
            )
        },
    )
}

/// `name(…)` for a runtime helper or a global function (`String`).
fn plain_call(name: &str, kids: Vec<LoweredExpr>) -> Built {
    node_or_text(
        kids,
        |n| call(id(name), n),
        |t| format!("{name}({})", t.join(", ")),
    )
}

/// `recv.prop`. `bracketed` is the text fallback's spelling (`(recv).prop`); the node form
/// is bracketed only where precedence needs it.
fn member_of(recv: LoweredExpr, prop: &str, bracketed: bool) -> Built {
    node_or_text(
        vec![recv],
        |n| {
            let [recv] = take::<1, _>(n);
            member(receiver_object(recv), prop)
        },
        |t| {
            if bracketed {
                format!("({}).{prop}", t[0])
            } else {
                format!("{}.{prop}", t[0])
            }
        },
    )
}

/// `recv.method(args)`; the first child is the receiver. `bracketed` is the text fallback's
/// spelling (`(recv).method(args)`).
fn method_of(kids: Vec<LoweredExpr>, method: &str, bracketed: bool) -> Built {
    node_or_text(
        kids,
        |mut n| {
            let recv = receiver_object(n.remove(0));
            call(member(recv, method), n)
        },
        |t| {
            let args = t[1..].join(", ");
            if bracketed {
                format!("({}).{method}({args})", t[0])
            } else {
                format!("{}.{method}({args})", t[0])
            }
        },
    )
}

/// `lo` and `hi` clamped at zero as `String.slice` does: `globalThis.Math.max(0, x)`.
fn at_least_zero(x: TsExpr) -> TsExpr {
    call(global_math("max"), vec![zero(), x])
}

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
    let built: Option<Built> = match (op, args) {
        // 1A: #3770, identity arm kept text so this paydown slice does not change
        // behaviour; fixed in its own PR.
        (IntOp::ToFloat, []) => Some(Err(pre.lower(receiver, cx))),
        (IntOp::Abs, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some(math_call("abs", kids))
        }
        (IntOp::Min | IntOp::Max, [other]) => {
            let kids = operands(&mut pre, cx, &[receiver, other]);
            Some(math_call(op.name(), kids))
        }
        (IntOp::Clamp, [lo, hi]) => {
            let kids = operands(&mut pre, cx, &[receiver, lo, hi]);
            Some(clamp(kids))
        }
        // v0.42 (ADR 0074): host number→string — `String(n)` is ECMAScript's
        // Number::toString (shortest round-trip; `1e21`/`Infinity`/`NaN` as the
        // host renders them). The normative contract is the platform's.
        (IntOp::ToString, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some(plain_call("String", kids))
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
            let kids = operands(&mut pre, cx, &[receiver, other]);
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
            Some(plain_call(helper, kids))
        }
        _ => None,
    };
    finish(pre, built)
}

/// The `Float` kernel's lowering (see [`lower_int_kernel`]).
pub(in crate::emitter) fn lower_float_kernel(
    receiver: &Expr,
    op: FloatOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let built: Option<Built> = match (op, args) {
        (FloatOp::Abs, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some(math_call("abs", kids))
        }
        // #1657 (runtime-semantics track §3.4, decided in S8): a Float→`Int`
        // conversion traps when the result is not a safe integer — a
        // non-finite `Float`, or one past ±2^53 — rather than producing an
        // `Int` outside the domain (`__bynkToInt`). The same posture as `Int`
        // division by zero.
        // Returning `Option[Int]` was rejected: a signature change to four
        // kernel methods.
        (FloatOp::Round | FloatOp::Floor | FloatOp::Ceil | FloatOp::Truncate, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            let f = if op == FloatOp::Truncate {
                "trunc"
            } else {
                op.name()
            };
            cx.note_int();
            Some(node_or_text(
                kids,
                |n| {
                    call(
                        id("__bynkToInt"),
                        vec![call(global_math(f), n), str_node(op.name())],
                    )
                },
                |t| {
                    format!(
                        "__bynkToInt(globalThis.Math.{f}({}), \"{}\")",
                        t[0],
                        op.name()
                    )
                },
            ))
        }
        (FloatOp::Min | FloatOp::Max, [other]) => {
            let kids = operands(&mut pre, cx, &[receiver, other]);
            Some(math_call(op.name(), kids))
        }
        (FloatOp::Clamp, [lo, hi]) => {
            let kids = operands(&mut pre, cx, &[receiver, lo, hi]);
            Some(clamp(kids))
        }
        (FloatOp::IsNaN | FloatOp::IsFinite, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            let name = op.name();
            Some(node_or_text(
                kids,
                |n| call(member(member(id("globalThis"), "Number"), name), n),
                |t| format!("globalThis.Number.{name}({})", t[0]),
            ))
        }
        (FloatOp::ToString, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some(plain_call("String", kids))
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
            let kids = operands(&mut pre, cx, &[receiver]);
            Some(math_call(op.name(), kids))
        }
        (FloatOp::Ln, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some(math_call("log", kids))
        }
        (FloatOp::Pow, [other]) => {
            let kids = operands(&mut pre, cx, &[receiver, other]);
            Some(math_call("pow", kids))
        }
        // `y.atan2(x)`: the angle of the point `(x, y)`.
        (FloatOp::Atan2, [x]) => {
            let kids = operands(&mut pre, cx, &[receiver, x]);
            Some(math_call("atan2", kids))
        }
        _ => None,
    };
    finish(pre, built)
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
    let built: Option<Built> = match (op, args) {
        // 1A: #3770, identity arm kept text so this paydown slice does not change
        // behaviour; fixed in its own PR.
        (DurationOp::ToMillis, []) => Some(Err(pre.lower(receiver, cx))),
        (DurationOp::ToString, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some(plain_call("String", kids))
        }
        _ => None,
    };
    finish(pre, built)
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
    let built: Option<Built> = match (op, args) {
        // 1A: #3770, identity arm kept text so this paydown slice does not change
        // behaviour; fixed in its own PR.
        (InstantOp::ToEpochMillis, []) => Some(Err(pre.lower(receiver, cx))),
        (InstantOp::ToString, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some(plain_call("String", kids))
        }
        // K3 (#2592): RFC 3339 in UTC.
        (InstantOp::ToIso, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            cx.note_time_ops();
            Some(plain_call("__bynkInstantToIso", kids))
        }
        _ => None,
    };
    finish(pre, built)
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
    let built: Option<Built> = match (op, args) {
        (BytesOp::Length, []) => {
            let [recv] = take::<1, _>(operands(&mut pre, cx, &[receiver]));
            Some(member_of(recv, "length", true))
        }
        (BytesOp::ToBase64, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            cx.note_bytes();
            Some(plain_call("__bynkBytesToBase64", kids))
        }
        (BytesOp::DecodeUtf8, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            cx.note_bytes();
            Some(plain_call("__bynkBytesDecodeUtf8", kids))
        }
        // #2016 (standard-library K1). `slice` clamps negative indices to 0 as
        // `String.slice` does, and `Uint8Array.slice` copies, so a slice never
        // aliases a buffer a binding may still hold.
        (BytesOp::Slice, [lo, hi]) => {
            let kids = operands(&mut pre, cx, &[receiver, lo, hi]);
            Some(slice_clamped(kids, true))
        }
        (BytesOp::Concat, [other]) => {
            let kids = operands(&mut pre, cx, &[receiver, other]);
            cx.note_bytes_ops();
            Some(plain_call("__bynkBytesConcat", kids))
        }
        (BytesOp::Compare, [other]) => {
            let kids = operands(&mut pre, cx, &[receiver, other]);
            cx.note_bytes_ops();
            Some(plain_call("__bynkBytesCompare", kids))
        }
        (BytesOp::ToHex, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            cx.note_bytes_ops();
            Some(plain_call("__bynkBytesToHex", kids))
        }
        (BytesOp::ToBase64Url, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            cx.note_bytes_ops();
            Some(plain_call("__bynkBytesToBase64Url", kids))
        }
        _ => None,
    };
    finish(pre, built)
}

/// `recv.slice(globalThis.Math.max(0, lo), globalThis.Math.max(0, hi))`, the `Bytes` and
/// `String` slice (`bracketed` is the text fallback's receiver spelling).
fn slice_clamped(kids: Vec<LoweredExpr>, bracketed: bool) -> Built {
    node_or_text(
        kids,
        |n| {
            let [recv, lo, hi] = take::<3, _>(n);
            call(
                member(receiver_object(recv), "slice"),
                vec![at_least_zero(lo), at_least_zero(hi)],
            )
        },
        |t| {
            let recv = if bracketed {
                format!("({})", t[0])
            } else {
                t[0].clone()
            };
            format!(
                "{recv}.slice(globalThis.Math.max(0, {}), globalThis.Math.max(0, {}))",
                t[1], t[2]
            )
        },
    )
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
    let (source, flags) = regex_parts(literal, extra, anchored);
    format!(
        "new globalThis.RegExp(\"{}\", \"{flags}\")",
        crate::emitter::escape_ts_string(&source)
    )
}

/// The `RegExp` source and flags [`regex_construction`] writes.
fn regex_parts(literal: &str, extra: &str, anchored: bool) -> (String, String) {
    let (pat, flags) = bynk_check::checker::split_regex_flags(literal);
    let source = if anchored {
        format!("(?<![\\s\\S])(?:{pat})(?![\\s\\S])")
    } else {
        pat
    };
    (source, format!("{flags}{extra}"))
}

/// [`regex_construction`] as a node: `new globalThis.RegExp("<source>", "<flags>")`. The `Str`
/// printer escapes with the very function the text form uses (`escape_ts_literal`).
fn regex_node(literal: &str, extra: &str, anchored: bool) -> TsExpr {
    let (source, flags) = regex_parts(literal, extra, anchored);
    TsExpr::New {
        callee: Box::new(member(id("globalThis"), "RegExp")),
        args: vec![str_node(source), str_node(flags)],
    }
}

fn ts_named(name: &str) -> TsType {
    TsType::named(name)
}

/// `Option<string>`.
fn option_string() -> TsType {
    TsType::named_with_args("Option", vec![ts_named("string")])
}

/// The capture-group mapper the `find` and `findAll` regex arms share, as text:
/// `(__m: RegExpMatchArray): Option<string>[] => [...__m].map(…)`.
const GROUPS_TEXT: &str = "(__m: globalThis.RegExpMatchArray): Option<string>[] => \
                           [...__m].map((__g: string | undefined): Option<string> => \
                           __g === undefined ? None : Some(__g))";

/// `(name: ty): ret => body`, a one-parameter arrow with a declared return type.
fn typed_arrow(name: &str, ty: TsType, ret: TsType, body: TsExpr) -> TsExpr {
    TsExpr::Arrow {
        params: vec![param(name, Some(ty))],
        is_async: false,
        generics: Vec::new(),
        return_type: Some(ret),
        body: Box::new(TsArrowBody::Expr(Box::new(body))),
    }
}

/// `test ? None : Some(…)`, where `test` is `name === rhs`.
fn none_or_some(name: &str, rhs: TsExpr, some: TsExpr) -> TsExpr {
    TsExpr::Conditional {
        test: Box::new(TsExpr::Binary {
            op: TsBinaryOp::StrictEq,
            left: Box::new(id(name)),
            right: Box::new(rhs),
        }),
        consequent: Box::new(id("None")),
        alternate: Box::new(some),
    }
}

/// [`GROUPS_TEXT`] as a node.
fn groups_node() -> TsExpr {
    let map_group = typed_arrow(
        "__g",
        TsType::union(vec![ts_named("string"), ts_named("undefined")]),
        option_string(),
        none_or_some("__g", id("undefined"), call(id("Some"), vec![id("__g")])),
    );
    typed_arrow(
        "__m",
        ts_named("globalThis.RegExpMatchArray"),
        TsType::array(option_string()),
        call(
            member(TsExpr::array(vec![spread(id("__m"))]), "map"),
            vec![map_group],
        ),
    )
}

/// `((__m: RegExpExecArray | null): Option<readonly Option<string>[]> => __m === null ? None
/// : Some((groups)(__m)))(regex.exec(recv))`, the `find` arm's node form.
fn find_regex_node(regex: TsExpr, recv: TsExpr) -> TsExpr {
    let mapper = typed_arrow(
        "__m",
        TsType::union(vec![
            ts_named("globalThis.RegExpExecArray"),
            ts_named("null"),
        ]),
        TsType::named_with_args("Option", vec![TsType::readonly_array(option_string())]),
        none_or_some(
            "__m",
            TsExpr::Lit(bynk_ts::TsLit::Null),
            call(id("Some"), vec![call(groups_node(), vec![id("__m")])]),
        ),
    );
    call(mapper, vec![call(member(regex, "exec"), vec![recv])])
}

/// One regex arm over a lowered receiver (and the replacement for `replaceRegex`). The node
/// and the text are the same expression; the `find`, `findAll` and `split` arms carry typed
/// arrows, written out once each.
fn regex_arm(op: StringOp, literal: &str, kids: Vec<LoweredExpr>) -> Option<Built> {
    Some(match op {
        StringOp::MatchesRegex => node_or_text(
            kids,
            |n| {
                let [recv] = take::<1, _>(n);
                call(member(regex_node(literal, "", true), "test"), vec![recv])
            },
            |t| format!("{}.test({})", regex_construction(literal, "", true), t[0]),
        ),
        StringOp::ContainsRegex => node_or_text(
            kids,
            |n| {
                let [recv] = take::<1, _>(n);
                call(member(regex_node(literal, "", false), "test"), vec![recv])
            },
            |t| format!("{}.test({})", regex_construction(literal, "", false), t[0]),
        ),
        StringOp::FindRegex => node_or_text(
            kids,
            |n| {
                let [recv] = take::<1, _>(n);
                find_regex_node(regex_node(literal, "", false), recv)
            },
            |t| {
                format!(
                    "((__m: globalThis.RegExpExecArray | null): Option<readonly Option<string>[]> => \
                     __m === null ? None : Some(({GROUPS_TEXT})(__m)))({}.exec({}))",
                    regex_construction(literal, "", false),
                    t[0]
                )
            },
        ),
        StringOp::FindAllRegex => node_or_text(
            kids,
            |n| {
                let [recv] = take::<1, _>(n);
                let scan = call(
                    member(receiver_object(recv), "matchAll"),
                    vec![regex_node(literal, "g", false)],
                );
                TsExpr::Paren(Box::new(TsExpr::As {
                    expr: Box::new(call(
                        member(TsExpr::array(vec![spread(scan)]), "map"),
                        vec![groups_node()],
                    )),
                    ty: TsType::readonly_array(TsType::readonly_array(option_string())),
                }))
            },
            |t| {
                format!(
                    "([...({}).matchAll({})].map({GROUPS_TEXT}) as readonly (readonly Option<string>[])[])",
                    t[0],
                    regex_construction(literal, "g", false)
                )
            },
        ),
        StringOp::ReplaceRegex => node_or_text(
            kids,
            |n| {
                let [recv, with] = take::<2, _>(n);
                call(
                    member(receiver_object(recv), "replace"),
                    vec![regex_node(literal, "g", false), with],
                )
            },
            |t| {
                format!(
                    "({}).replace({}, {})",
                    t[0],
                    regex_construction(literal, "g", false),
                    t[1]
                )
            },
        ),
        // JS `split` with a regex, minus the captured groups it splices in:
        // its own algorithm over a sticky scan, an empty match at the last
        // split point advancing by one code unit.
        //
        // 1A: the body is a statement block (a `while` over a sticky scan), which `bynk_ts`
        // cannot print inline in the one-line layout this arm has today; it stays text.
        StringOp::SplitRegex => Err(format!(
            "((__s: string, __r: globalThis.RegExp): readonly string[] => {{ \
             if (__s.length === 0) {{ return __r.exec(__s) === null ? [\"\"] : []; }} \
             const __out: string[] = []; let __p = 0; let __q = 0; \
             while (__q < __s.length) {{ __r.lastIndex = __q; const __hit = __r.exec(__s); \
             const __e = __r.lastIndex; \
             if (__hit === null || __e === __p) {{ __q++; }} \
             else {{ __out.push(__s.slice(__p, __q)); __p = __e; __q = __p; }} }} \
             __out.push(__s.slice(__p)); return __out; }})({}, {})",
            kids.into_iter()
                .next()
                .map(LoweredExpr::into_text)
                .unwrap_or_default(),
            regex_construction(literal, "y", false)
        )),
        _ => return None,
    })
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
        // Only `replaceRegex` carries a second operand, lowered after the receiver.
        let kids = match (op, args.len()) {
            (StringOp::ReplaceRegex, 2) => operands(&mut pre, cx, &[receiver, &args[1]]),
            (StringOp::ReplaceRegex, _) => return None,
            (_, 1) => operands(&mut pre, cx, &[receiver]),
            _ => return None,
        };
        return finish(pre, regex_arm(op, &literal, kids));
    }
    let built: Option<Built> = match (op, args) {
        (StringOp::Length, []) => {
            let [recv] = take::<1, _>(operands(&mut pre, cx, &[receiver]));
            Some(member_of(recv, "length", true))
        }
        (StringOp::Trim, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some(method_of(kids, "trim", false))
        }
        (StringOp::ToUpper, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some(method_of(kids, "toUpperCase", false))
        }
        (StringOp::ToLower, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some(method_of(kids, "toLowerCase", false))
        }
        (StringOp::Chars, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some(node_or_text(
                kids,
                |n| {
                    let [recv] = take::<1, _>(n);
                    TsExpr::array(vec![spread(recv)])
                },
                |t| format!("[...{}]", t[0]),
            ))
        }
        (StringOp::Split, [sep]) => {
            let kids = operands(&mut pre, cx, &[receiver, sep]);
            Some(method_of(kids, "split", false))
        }
        (StringOp::Contains, [sub]) => {
            let kids = operands(&mut pre, cx, &[receiver, sub]);
            Some(method_of(kids, "includes", false))
        }
        (StringOp::StartsWith | StringOp::EndsWith, [sub]) => {
            let kids = operands(&mut pre, cx, &[receiver, sub]);
            Some(method_of(kids, op.name(), false))
        }
        (StringOp::Concat, [other]) => {
            let kids = operands(&mut pre, cx, &[receiver, other]);
            Some(method_of(kids, "concat", false))
        }
        (StringOp::Replace, [from, to]) => {
            let kids = operands(&mut pre, cx, &[receiver, from, to]);
            // #1650: a *string* replacement is `$`-expanded by JS (`$&`, `$1`,
            // `` $` ``, `$'`, `$$`); a function replacer's return value never is,
            // so the replacement is inserted literally. `to` is a pure
            // expression, so its result is the same either way, but it is
            // evaluated once per match (and not at all with no match), so a
            // costly replacement is recomputed.
            Some(node_or_text(
                kids,
                |n| {
                    let [recv, from, to] = take::<3, _>(n);
                    call(
                        member(receiver_object(recv), "replaceAll"),
                        vec![from, arrow(Vec::new(), to)],
                    )
                },
                |t| format!("{}.replaceAll({}, () => {})", t[0], t[1], t[2]),
            ))
        }
        (StringOp::Slice, [lo, hi]) => {
            let kids = operands(&mut pre, cx, &[receiver, lo, hi]);
            Some(slice_clamped(kids, false))
        }
        (StringOp::IndexOf, [sub]) => {
            let kids = operands(&mut pre, cx, &[receiver, sub]);
            Some(node_or_text(
                kids,
                |n| {
                    let [recv, sub] = take::<2, _>(n);
                    let to_option = arrow(
                        vec![param("__i", Some(ts_named("number")))],
                        TsExpr::Conditional {
                            test: Box::new(TsExpr::Binary {
                                op: TsBinaryOp::LessThan,
                                left: Box::new(id("__i")),
                                right: Box::new(zero()),
                            }),
                            consequent: Box::new(id("None")),
                            alternate: Box::new(call(id("Some"), vec![id("__i")])),
                        },
                    );
                    call(
                        to_option,
                        vec![call(member(receiver_object(recv), "indexOf"), vec![sub])],
                    )
                },
                |t| {
                    format!(
                        "((__i: number) => __i < 0 ? None : Some(__i))({}.indexOf({}))",
                        t[0], t[1]
                    )
                },
            ))
        }
        // K2 (#2594): percent-encoding over the UTF-8 bytes.
        (StringOp::PercentEncode, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            cx.note_url_ops();
            Some(plain_call("__bynkPercentEncode", kids))
        }
        (StringOp::PercentDecode, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            cx.note_url_ops();
            Some(plain_call("__bynkPercentDecode", kids))
        }
        _ => None,
    };
    finish(pre, built)
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
    let mode = |a: &Expr| rounding_mode(a).map(|m| m.name());
    // A helper call whose last operand is the rounding mode, a string literal.
    let with_mode = |helper: &str, mut kids: Vec<LoweredExpr>, mode: &str| {
        kids.push(LoweredExpr::Node(str_node(mode)));
        node_or_text(
            kids,
            |n| call(id(helper), n),
            |t| format!("{helper}({})", t.join(", ")),
        )
    };
    let built: Option<Built> = match (op, args) {
        (DecimalOp::Scale, []) => {
            let [recv] = take::<1, _>(operands(&mut pre, cx, &[receiver]));
            Some(member_of(recv, "scale", true))
        }
        (DecimalOp::ToString, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            cx.note_decimal("__bynkDecimalToString");
            Some(plain_call("__bynkDecimalToString", kids))
        }
        (DecimalOp::Abs, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            cx.note_decimal("__bynkDecimalAbs");
            Some(plain_call("__bynkDecimalAbs", kids))
        }
        (DecimalOp::ToInt, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            cx.note_decimal("__bynkDecimalToInt");
            Some(plain_call("__bynkDecimalToInt", kids))
        }
        (DecimalOp::ToFloat, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            cx.note_decimal("__bynkDecimalToFloat");
            Some(plain_call("__bynkDecimalToFloat", kids))
        }
        (DecimalOp::Min | DecimalOp::Max, [other]) => {
            let kids = operands(&mut pre, cx, &[receiver, other]);
            let helper = if op == DecimalOp::Min {
                "__bynkDecimalMin"
            } else {
                "__bynkDecimalMax"
            };
            cx.note_decimal(helper);
            Some(plain_call(helper, kids))
        }
        (DecimalOp::Round, [scale, m]) => {
            let mode = mode(m)?;
            let kids = operands(&mut pre, cx, &[receiver, scale]);
            cx.note_decimal("__bynkDecimalRound");
            Some(with_mode("__bynkDecimalRound", kids, mode))
        }
        (DecimalOp::Div, [other, scale, m]) => {
            let mode = mode(m)?;
            let kids = operands(&mut pre, cx, &[receiver, other, scale]);
            cx.note_decimal("__bynkDecimalDiv");
            Some(with_mode("__bynkDecimalDiv", kids, mode))
        }
        _ => None,
    };
    finish(pre, built)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emitter::lower::TEXT_EXITS;
    use crate::testkit::emit_source;

    /// `(text exits, emitted TypeScript)` for a one-function commons whose signature is `sig`.
    fn exits(sig: &str, body: &str) -> (usize, String) {
        TEXT_EXITS.with(|c| c.set(0));
        let ts = emit_source(&format!("commons ks\nfn f({sig}) -> {body}\n"));
        (TEXT_EXITS.with(|c| c.get()), ts)
    }

    /// The returned expression of `export function f`.
    fn returned(ts: &str) -> String {
        let at = ts.find("export function f(").expect("f");
        let rest = &ts[at..];
        let ret = rest.find("  return ").expect("a return") + "  return ".len();
        let end = rest[ret..].find(";\n}\n").expect("end");
        rest[ret..ret + end].to_string()
    }

    /// One call per kernel op over identifier receivers and arguments: the exact print, with no
    /// text exit and the `note_*` import where the op has one (checked on the output's imports).
    #[test]
    fn every_scalar_op_prints_as_a_node_with_no_text_exit() {
        let int = "n: Int, m: Int, lo: Int, hi: Int";
        let float = "x: Float, y: Float, lo: Float, hi: Float";
        let rows: &[(&str, &str, &str, &str)] = &[
            (int, "Int { n.abs() }", "globalThis.Math.abs(n)", ""),
            (int, "Int { n.min(m) }", "globalThis.Math.min(n, m)", ""),
            (int, "Int { n.max(m) }", "globalThis.Math.max(n, m)", ""),
            (
                int,
                "Int { n.clamp(lo, hi) }",
                "globalThis.Math.min(globalThis.Math.max(n, lo), hi)",
                "",
            ),
            (int, "String { n.toString() }", "String(n)", ""),
            (
                int,
                "Int { n.div(m) }",
                "__bynkIntDivFloor(n, m)",
                "__bynkIntDivFloor",
            ),
            (
                int,
                "Int { n.rem(m) }",
                "__bynkIntRem(n, m)",
                "__bynkIntRem",
            ),
            (
                int,
                "Int { n.mod(m) }",
                "__bynkIntMod(n, m)",
                "__bynkIntMod",
            ),
            (
                int,
                "Int { n.pow(m) }",
                "__bynkIntPow(n, m)",
                "__bynkIntPow",
            ),
            (
                int,
                "Int { n.and(m) }",
                "__bynkIntAnd(n, m)",
                "__bynkIntAnd",
            ),
            (int, "Int { n.or(m) }", "__bynkIntOr(n, m)", "__bynkIntOr"),
            (
                int,
                "Int { n.xor(m) }",
                "__bynkIntXor(n, m)",
                "__bynkIntXor",
            ),
            (
                int,
                "Int { n.andNot(m) }",
                "__bynkIntAndNot(n, m)",
                "__bynkIntAndNot",
            ),
            (
                int,
                "Int { n.shl(m) }",
                "__bynkIntShl(n, m)",
                "__bynkIntShl",
            ),
            (
                int,
                "Int { n.shr(m) }",
                "__bynkIntShr(n, m)",
                "__bynkIntShr",
            ),
            (float, "Float { x.abs() }", "globalThis.Math.abs(x)", ""),
            (
                float,
                "Int { x.round() }",
                "__bynkToInt(globalThis.Math.round(x), \"round\")",
                "__bynkToInt",
            ),
            (
                float,
                "Int { x.floor() }",
                "__bynkToInt(globalThis.Math.floor(x), \"floor\")",
                "__bynkToInt",
            ),
            (
                float,
                "Int { x.ceil() }",
                "__bynkToInt(globalThis.Math.ceil(x), \"ceil\")",
                "__bynkToInt",
            ),
            (
                float,
                "Int { x.truncate() }",
                "__bynkToInt(globalThis.Math.trunc(x), \"truncate\")",
                "__bynkToInt",
            ),
            (float, "Float { x.min(y) }", "globalThis.Math.min(x, y)", ""),
            (float, "Float { x.max(y) }", "globalThis.Math.max(x, y)", ""),
            (
                float,
                "Float { x.clamp(lo, hi) }",
                "globalThis.Math.min(globalThis.Math.max(x, lo), hi)",
                "",
            ),
            (
                float,
                "Bool { x.isNaN() }",
                "globalThis.Number.isNaN(x)",
                "",
            ),
            (
                float,
                "Bool { x.isFinite() }",
                "globalThis.Number.isFinite(x)",
                "",
            ),
            (float, "String { x.toString() }", "String(x)", ""),
            (float, "Float { x.sqrt() }", "globalThis.Math.sqrt(x)", ""),
            (float, "Float { x.exp() }", "globalThis.Math.exp(x)", ""),
            (float, "Float { x.ln() }", "globalThis.Math.log(x)", ""),
            (float, "Float { x.log10() }", "globalThis.Math.log10(x)", ""),
            (float, "Float { x.log2() }", "globalThis.Math.log2(x)", ""),
            (float, "Float { x.sin() }", "globalThis.Math.sin(x)", ""),
            (float, "Float { x.cos() }", "globalThis.Math.cos(x)", ""),
            (float, "Float { x.tan() }", "globalThis.Math.tan(x)", ""),
            (float, "Float { x.asin() }", "globalThis.Math.asin(x)", ""),
            (float, "Float { x.acos() }", "globalThis.Math.acos(x)", ""),
            (float, "Float { x.atan() }", "globalThis.Math.atan(x)", ""),
            (float, "Float { x.pow(y) }", "globalThis.Math.pow(x, y)", ""),
            (
                float,
                "Float { x.atan2(y) }",
                "globalThis.Math.atan2(x, y)",
                "",
            ),
            ("d: Duration", "String { d.toString() }", "String(d)", ""),
            ("i: Instant", "String { i.toString() }", "String(i)", ""),
            (
                "i: Instant",
                "String { i.toIso() }",
                "__bynkInstantToIso(i)",
                "__bynkInstantToIso",
            ),
            ("b: Bytes", "Int { b.length() }", "b.length", ""),
            (
                "b: Bytes",
                "String { b.toBase64() }",
                "__bynkBytesToBase64(b)",
                "__bynkBytesToBase64",
            ),
            (
                "b: Bytes",
                "Option[String] { b.decodeUtf8() }",
                "__bynkBytesDecodeUtf8(b)",
                "__bynkBytesDecodeUtf8",
            ),
            (
                "b: Bytes, lo: Int, hi: Int",
                "Bytes { b.slice(lo, hi) }",
                "b.slice(globalThis.Math.max(0, lo), globalThis.Math.max(0, hi))",
                "",
            ),
            (
                "b: Bytes, c: Bytes",
                "Bytes { b.concat(c) }",
                "__bynkBytesConcat(b, c)",
                "__bynkBytesConcat",
            ),
            (
                "b: Bytes, c: Bytes",
                "Int { b.compare(c) }",
                "__bynkBytesCompare(b, c)",
                "__bynkBytesCompare",
            ),
            (
                "b: Bytes",
                "String { b.toHex() }",
                "__bynkBytesToHex(b)",
                "__bynkBytesToHex",
            ),
            (
                "b: Bytes",
                "String { b.toBase64Url() }",
                "__bynkBytesToBase64Url(b)",
                "__bynkBytesToBase64Url",
            ),
            ("s: String", "Int { s.length() }", "s.length", ""),
            ("s: String", "String { s.trim() }", "s.trim()", ""),
            ("s: String", "String { s.toUpper() }", "s.toUpperCase()", ""),
            ("s: String", "String { s.toLower() }", "s.toLowerCase()", ""),
            ("s: String", "List[String] { s.chars() }", "[...s]", ""),
            (
                "s: String, t: String",
                "List[String] { s.split(t) }",
                "s.split(t)",
                "",
            ),
            (
                "s: String, t: String",
                "Bool { s.contains(t) }",
                "s.includes(t)",
                "",
            ),
            (
                "s: String, t: String",
                "Bool { s.startsWith(t) }",
                "s.startsWith(t)",
                "",
            ),
            (
                "s: String, t: String",
                "Bool { s.endsWith(t) }",
                "s.endsWith(t)",
                "",
            ),
            (
                "s: String, t: String",
                "String { s.concat(t) }",
                "s.concat(t)",
                "",
            ),
            (
                "s: String, t: String, u: String",
                "String { s.replace(t, u) }",
                "s.replaceAll(t, () => u)",
                "",
            ),
            (
                "s: String, lo: Int, hi: Int",
                "String { s.slice(lo, hi) }",
                "s.slice(globalThis.Math.max(0, lo), globalThis.Math.max(0, hi))",
                "",
            ),
            (
                "s: String, t: String",
                "Option[Int] { s.indexOf(t) }",
                "((__i: number) => __i < 0 ? None : Some(__i))(s.indexOf(t))",
                "",
            ),
            (
                "s: String",
                "String { s.percentEncode() }",
                "__bynkPercentEncode(s)",
                "__bynkPercentEncode",
            ),
            (
                "s: String",
                "Option[String] { s.percentDecode() }",
                "__bynkPercentDecode(s)",
                "__bynkPercentDecode",
            ),
            ("d: Decimal", "Int { d.scale() }", "d.scale", ""),
            (
                "d: Decimal",
                "String { d.toString() }",
                "__bynkDecimalToString(d)",
                "__bynkDecimalToString",
            ),
            (
                "d: Decimal",
                "Decimal { d.abs() }",
                "__bynkDecimalAbs(d)",
                "__bynkDecimalAbs",
            ),
            (
                "d: Decimal",
                "Option[Int] { d.toInt() }",
                "__bynkDecimalToInt(d)",
                "__bynkDecimalToInt",
            ),
            (
                "d: Decimal",
                "Float { d.toFloat() }",
                "__bynkDecimalToFloat(d)",
                "__bynkDecimalToFloat",
            ),
            (
                "d: Decimal, e: Decimal",
                "Decimal { d.min(e) }",
                "__bynkDecimalMin(d, e)",
                "__bynkDecimalMin",
            ),
            (
                "d: Decimal, e: Decimal",
                "Decimal { d.max(e) }",
                "__bynkDecimalMax(d, e)",
                "__bynkDecimalMax",
            ),
            (
                "d: Decimal, k: Int",
                "Decimal { d.round(k, RoundingMode.HalfUp) }",
                "__bynkDecimalRound(d, k, \"HalfUp\")",
                "__bynkDecimalRound",
            ),
            (
                "d: Decimal, e: Decimal, k: Int",
                "Decimal { d.div(e, k, RoundingMode.Down) }",
                "__bynkDecimalDiv(d, e, k, \"Down\")",
                "__bynkDecimalDiv",
            ),
        ];
        for (sig, body, want, import) in rows {
            let (n, ts) = exits(sig, body);
            assert_eq!(returned(&ts), *want, "{body}");
            assert_eq!(n, 0, "{body}: text exits\n{ts}");
            if !import.is_empty() {
                let header: String = ts.lines().filter(|l| l.starts_with("import")).collect();
                assert!(
                    header.contains(import),
                    "{body}: missing import {import}\n{ts}"
                );
            }
        }
    }

    /// A receiver that is already a source-bracketed expression keeps its brackets (as the text form
    /// wrote them: the source `Paren` is a node), and one that is a call or a member loses the text
    /// form's unconditional ones.
    #[test]
    fn receivers_are_bracketed_only_where_precedence_needs_it() {
        for (sig, body, want) in [
            (
                "c: Bool, s: String, t: String",
                "Int { (if c { s } else { t }).length() }",
                "((c ? s : t)).length",
            ),
            (
                "c: Bool, s: String, t: String",
                "String { (if c { s } else { t }).trim() }",
                "((c ? s : t)).trim()",
            ),
            (
                "c: Bool, s: String, t: String, u: String",
                "String { (if c { s } else { t }).replace(u, u) }",
                "((c ? s : t)).replaceAll(u, () => u)",
            ),
            (
                "c: Bool, b: Bytes, d: Bytes",
                "Int { (if c { b } else { d }).length() }",
                "((c ? b : d)).length",
            ),
            (
                "c: Bool, d: Decimal, e: Decimal",
                "Int { (if c { d } else { e }).scale() }",
                "((c ? d : e)).scale",
            ),
            ("s: String", "Int { s.trim().length() }", "s.trim().length"),
            (
                "s: String, t: String",
                "Int { s.concat(t).length() }",
                "s.concat(t).length",
            ),
            (
                "s: String, t: String",
                "Option[Int] { s.concat(t).indexOf(t) }",
                "((__i: number) => __i < 0 ? None : Some(__i))(s.concat(t).indexOf(t))",
            ),
        ] {
            let (n, ts) = exits(sig, body);
            assert_eq!(returned(&ts), want, "{body}");
            assert_eq!(n, 0, "{body}: text exits\n{ts}");
        }
    }

    /// A text receiver or argument (a `splitRegex` list is still text) falls back to
    /// today's text byte for byte, with the text form's own bracketing.
    #[test]
    fn a_text_operand_falls_back_to_todays_text() {
        // `String.splitRegex`'s arm is a statement block, so a `List` kernel over it is text.
        let text_of = |ty: &str, y: &str| {
            let (n, ts) = exits(
                &format!("p: String, c: {ty}"),
                &format!("{ty} {{ p.splitRegex(\",\").map((x) => {y}).firstOrElse({y}) }}"),
            );
            assert!(n > 0, "{ts}");
            returned(&ts)
        };
        let s = text_of("String", "\"\"");
        let n = text_of("Int", "0");
        let b = text_of("Bytes", "c");
        let d = text_of("Decimal", "c");
        for (sig, body, want) in [
            (
                "p: String",
                "String { p.splitRegex(\",\").map((x) => \"\").firstOrElse(\"\").trim() }",
                format!("{s}.trim()"),
            ),
            (
                "p: String",
                "Int { p.splitRegex(\",\").map((x) => \"\").firstOrElse(\"\").length() }",
                format!("({s}).length"),
            ),
            (
                "p: String",
                "List[String] { p.splitRegex(\",\").map((x) => \"\").firstOrElse(\"\").chars() }",
                format!("[...{s}]"),
            ),
            (
                "p: String, t: String",
                "String { t.concat(p.splitRegex(\",\").map((x) => \"\").firstOrElse(\"\")) }",
                format!("t.concat({s})"),
            ),
            (
                "p: String, t: String",
                "String { t.replace(t, p.splitRegex(\",\").map((x) => \"\").firstOrElse(\"\")) }",
                format!("t.replaceAll(t, () => {s})"),
            ),
            (
                "p: String",
                "String { p.splitRegex(\",\").map((x) => \"\").firstOrElse(\"\").replaceRegex(\"a\", \"b\") }",
                format!("({s}).replace(new globalThis.RegExp(\"a\", \"g\"), \"b\")"),
            ),
            (
                "p: String",
                "Int { p.splitRegex(\",\").map((x) => 0).firstOrElse(0).abs() }",
                format!("globalThis.Math.abs({n})"),
            ),
            (
                "p: String",
                "Int { p.splitRegex(\",\").map((x) => 0).firstOrElse(0).clamp(0, 5) }",
                format!("globalThis.Math.min(globalThis.Math.max({n}, 0), 5)"),
            ),
            (
                "p: String, c: Bytes",
                "Int { p.splitRegex(\",\").map((x) => c).firstOrElse(c).length() }",
                format!("({b}).length"),
            ),
            (
                "p: String, c: Decimal",
                "Int { p.splitRegex(\",\").map((x) => c).firstOrElse(c).scale() }",
                format!("({d}).scale"),
            ),
            (
                "p: String, c: Decimal, k: Int",
                "Decimal { c.round(k, RoundingMode.Up).min(p.splitRegex(\",\").map((x) => c).firstOrElse(c)) }",
                format!("__bynkDecimalMin(__bynkDecimalRound(c, k, \"Up\"), {d})"),
            ),
        ] {
            let (exits, ts) = exits(sig, body);
            assert_eq!(returned(&ts), want, "{body}");
            assert!(exits > 0, "{body}: expected a text exit\n{ts}");
        }
    }

    /// Every regex arm over a plain, a flagged, an anchored and an escaped pattern, as a node
    /// with no text exit.
    #[test]
    fn the_regex_arms_print_as_nodes() {
        let groups = GROUPS_TEXT;
        for (body, want) in [
            (
                "Bool { s.matchesRegex(\"a+\") }",
                "new globalThis.RegExp(\"(?<![\\\\s\\\\S])(?:a+)(?![\\\\s\\\\S])\", \"\").test(s)".to_string(),
            ),
            (
                "Bool { s.matchesRegex(\"(?i)abc\") }",
                "new globalThis.RegExp(\"(?<![\\\\s\\\\S])(?:abc)(?![\\\\s\\\\S])\", \"i\").test(s)".to_string(),
            ),
            (
                "Bool { s.containsRegex(\"a\\\\.b\") }",
                "new globalThis.RegExp(\"a\\\\.b\", \"\").test(s)".to_string(),
            ),
            (
                "Bool { s.containsRegex(\"\\\"q\\\"$\") }",
                "new globalThis.RegExp(\"\\\"q\\\"$\", \"\").test(s)".to_string(),
            ),
            (
                "Option[List[Option[String]]] { s.findRegex(\"(a)(b)?\") }",
                format!(
                    "((__m: globalThis.RegExpExecArray | null): Option<readonly Option<string>[]> => __m === null ? None : Some(({groups})(__m)))(new globalThis.RegExp(\"(a)(b)?\", \"\").exec(s))"
                ),
            ),
            (
                "List[List[Option[String]]] { s.findAllRegex(\"\\\\d+\") }",
                format!(
                    "([...s.matchAll(new globalThis.RegExp(\"\\\\d+\", \"g\"))].map({groups}) as readonly (readonly Option<string>[])[])"
                ),
            ),
            (
                "String { s.replaceRegex(\"\\\\s+\", \" \") }",
                "s.replace(new globalThis.RegExp(\"\\\\s+\", \"g\"), \" \")".to_string(),
            ),
        ] {
            let (n, ts) = exits("s: String", body);
            assert_eq!(returned(&ts), want, "{body}");
            assert_eq!(n, 0, "{body}: text exits\n{ts}");
        }
    }

    /// `splitRegex` keeps its text (marked `// 1A:`): its body is a statement block.
    #[test]
    fn split_regex_stays_text() {
        let (n, ts) = exits("s: String", "List[String] { s.splitRegex(\",\") }");
        assert!(n > 0, "{ts}");
        assert!(
            returned(&ts).ends_with("(s, new globalThis.RegExp(\",\", \"y\"))"),
            "{ts}"
        );
    }

    /// Decision B's escaper check: the `Str` node the regex arm builds prints the very bytes the
    /// text form writes, for every literal pattern the fixtures hold and a corpus of hard ones
    /// (backslashes, quotes, `$`, non-ASCII, the anchored lookaround form).
    #[test]
    fn the_regex_node_and_text_agree_on_every_pattern() {
        let mut corpus: Vec<String> = [
            "a+",
            "\\d+",
            "a\\.b",
            "\"q\"",
            "'q'",
            "$1",
            "a$",
            "\\\\",
            "caf\u{e9}",
            "\u{1f600}+",
            "(?i)abc",
            "(?s)a.b",
            "(?m)^x$",
            "[^a-z0-9]+",
            "(?<first>\\w+)@(\\w+)",
            "tab\there",
            "nl\nthere",
            "\u{2028}",
            "(?:x)(?!y)(?<!z)",
            "\\/",
            "`x`",
            "${x}",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("bynk-tests")
            .join("tests")
            .join("fixtures");
        let mut stack = vec![fixtures];
        let mut seen_fixture_patterns = 0usize;
        while let Some(dir) = stack.pop() {
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "bynk")
                    && let Ok(src) = std::fs::read_to_string(&path)
                {
                    for name in [
                        "matchesRegex(\"",
                        "containsRegex(\"",
                        "findRegex(\"",
                        "findAllRegex(\"",
                        "replaceRegex(\"",
                        "splitRegex(\"",
                    ] {
                        for part in src.split(name).skip(1) {
                            // The literal runs to the first unescaped quote; `\\"` is an escaped backslash
                            // then the closing quote, `\"` an escaped quote.
                            let mut lit = String::new();
                            let mut chars = part.chars();
                            while let Some(c) = chars.next() {
                                match c {
                                    '\\' => match chars.next() {
                                        Some('\\') => lit.push('\\'),
                                        Some('"') => lit.push('"'),
                                        Some(other) => {
                                            lit.push('\\');
                                            lit.push(other);
                                        }
                                        None => lit.push('\\'),
                                    },
                                    '"' => break,
                                    c => lit.push(c),
                                }
                            }
                            corpus.push(lit);
                            seen_fixture_patterns += 1;
                        }
                    }
                }
            }
        }
        assert!(
            seen_fixture_patterns > 10,
            "the fixtures hold regex patterns"
        );
        for literal in &corpus {
            for (extra, anchored) in [("", true), ("", false), ("g", false), ("y", false)] {
                assert_eq!(
                    bynk_ts::print_expr(&regex_node(literal, extra, anchored)),
                    regex_construction(literal, extra, anchored),
                    "{literal:?} {extra:?} {anchored}"
                );
            }
        }
    }

    /// The identity kernels behind #3770 stay text. This pins today's (wrong) output for one case:
    /// `Instant.fromEpochMillis(a + b).toEpochMillis() * 2` prints `a + b * 2`, not `(a + b) * 2`.
    /// The fix PR for #3770 flips this assertion deliberately.
    #[test]
    fn the_3770_identity_arms_keep_their_current_text() {
        let (n, ts) = exits(
            "a: Int, b: Int",
            "Int { Instant.fromEpochMillis(a + b).toEpochMillis() * 2 }",
        );
        assert_eq!(returned(&ts), "a + b * 2");
        assert!(n > 0, "the identity arm is a text exit\n{ts}");
        let (_, ts) = exits(
            "a: Int, b: Int",
            "Int { Duration.millis(a + b).toMillis() * 2 }",
        );
        assert_eq!(returned(&ts), "a + b * 2");
        let (_, ts) = exits("n: Int", "Float { n.toFloat() }");
        assert_eq!(returned(&ts), "n");
    }

    /// An identity arm keeps its receiver's hoisted statements.
    #[test]
    fn an_identity_arm_keeps_its_hoists() {
        let (_, ts) = {
            TEXT_EXITS.with(|c| c.set(0));
            let ts = emit_source(
                "commons ks\nfn g() -> Result[Int, String] { Ok(3) }\nfn f() -> Result[Float, String] { Ok(g()?.toFloat()) }\n",
            );
            (0, ts)
        };
        assert!(ts.contains("const __r0 = g();"), "{ts}");
        assert!(ts.contains("return Ok(__r0.value);"), "{ts}");
    }
}
