//! `messages` bundles: the per-locale renderer functions, ICU placeholders and sub-messages
//! (`emit_messages_bundle` is the entry point), with the template tests.

use super::join_plus;
use crate::emitter::{TemplateSegment, escape_ts_string, icu, split_template};
use crate::ts_build::{ident, index, member};
use bynk_syntax::ast::{MessageEntry, MessagesDecl};

/// One message entry's TS renderer: `(params) => <expr>`. A literal-only
/// template collapses to a plain string; a template with placeholders becomes
/// a `+`-concatenation, each placeholder substituted from `params` via
/// `renderArg` (imported from `bynk.locale`, never duplicated here) when
/// present, else left as the literal `{name}` token — total, never throws,
/// matching `render`'s own totality contract.
///
/// message-bundles slice 3 (#878): an ICU-dispatch placeholder (`inner`
/// contains a `,`) instead becomes `emit_icu_placeholder`'s codegen, which
/// needs `locale_tag` to construct `Intl.*` at runtime. `Err(_)` (a
/// malformed ICU construct) is unreachable in practice — the checker
/// (`bynk.messages.malformed_icu_syntax`) already rejects it before emission
/// runs on an error-free project — but is handled totally, matching this
/// function's own "never throws" contract, rather than `unreachable!()`.
///
/// Arc C, step (11) (#1388): returns a real `bynk_ts::TsExpr::Arrow` instead
/// of writing text — the caller (`emit_messages_bundle`, #1355) drops its
/// own opaque `TsExpr::Ident(renderer_text)` wrap and uses this value
/// directly as the object entry's own value.
fn emit_message_entry_renderer(entry: &MessageEntry, locale_tag: &str) -> bynk_ts::TsExpr {
    let segments = split_template(&entry.template);
    let parts: Vec<bynk_ts::TsExpr> = segments
        .iter()
        .map(|s| match s {
            TemplateSegment::Literal(lit) => {
                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str((*lit).to_string()))
            }
            TemplateSegment::Placeholder { inner, .. } => match inner.find(',') {
                None => {
                    // `(params.get(key) !== undefined ? renderArg(params.get(key) as MessageArg) : fallback)`
                    // — unconditionally parenthesised, matching the
                    // pre-conversion text exactly even when this is the
                    // template's own only segment (no `+`-join to borrow
                    // parens from in that case).
                    let get_call = bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Member {
                            object: Box::new(bynk_ts::TsExpr::Ident("params".to_string())),
                            property: "get".to_string(),
                        }),
                        args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            (*inner).to_string(),
                        ))],
                    };
                    bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Conditional {
                        test: Box::new(bynk_ts::TsExpr::Binary {
                            op: bynk_ts::TsBinaryOp::StrictNotEq,
                            left: Box::new(get_call.clone()),
                            right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                        }),
                        consequent: Box::new(bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Ident("__bynkRenderArg".to_string())),
                            args: vec![bynk_ts::TsExpr::As {
                                expr: Box::new(get_call),
                                ty: bynk_ts::TsType::named("__bynkMessageArg"),
                            }],
                        }),
                        alternate: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(format!(
                            "{{{inner}}}"
                        )))),
                    }))
                }
                Some(_) => match icu::parse_icu_placeholder(inner) {
                    Ok(p) => emit_icu_placeholder(&p, locale_tag),
                    Err(_) => {
                        let name = inner.split(',').next().unwrap_or(inner).trim();
                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(format!("{{{name}}}")))
                    }
                },
            },
        })
        .collect();
    let body = if parts.is_empty() {
        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(String::new()))
    } else {
        join_plus(parts)
    };
    bynk_ts::TsExpr::Arrow {
        params: vec![bynk_ts::TsParam::new(
            "params".to_string(),
            Some(bynk_ts::TsType::named_with_args(
                "ReadonlyMap",
                vec![
                    bynk_ts::TsType::named("string"),
                    bynk_ts::TsType::named("__bynkMessageArg"),
                ],
            )),
        )],
        is_async: false,
        generics: Vec::new(),
        return_type: Some(bynk_ts::TsType::named("string")),
        body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(body))),
    }
}

/// message-bundles slice 3 (#878): codegen for one ICU-dispatch placeholder.
/// Each kind emits as an IIFE (matching this file's own "non-tail dispatch
/// becomes an IIFE" idiom) guarding on the looked-up `MessageArg`'s `.tag`
/// before touching `.value` — TS narrows `__arg` to the matching variant
/// after the guard, so no cast is needed. Falls back to the literal
/// `"{name}"` text when the param is missing or the wrong `MessageArg`
/// variant, matching the plain-placeholder fallback's own convention.
///
/// Arc C, step (11) (#1388): `Plural`/`Number`/`Date` convert fully to real
/// `bynk_ts::TsExpr` nodes (`Call` over an `Arrow` — `Arrow` is already in
/// the printer's own `needs_parens_as_operand`, so the IIFE's own
/// `(...)(...)` wrapping comes free, no explicit `Paren`). `Select` stays
/// one opaque `TsExpr::Ident` — the only one of the four real shapes with a
/// BLOCK-bodied IIFE (at the time, `TsExpr::Arrow` had no block-body variant
/// at all, and extending it for this one real site was rejected as
/// disproportionate, the same "odd, one-off shape stays opaque text"
/// posture this track uses repeatedly), though its own arm VALUES are still
/// real `emit_sub_message` results, stringified back into that opaque host
/// text via the new `bynk_ts::print_expr` (#1388's own second real gap).
/// #1435 (Arc E slice 1) later gave `TsExpr::Arrow` a real block-body shape
/// (`TsArrowBody::Block`) for a different real site; this `Select` arm was
/// not reconverted along with it (out of that slice's own scope) and stays
/// exactly as it was.
fn emit_icu_placeholder(p: &icu::IcuPlaceholder<'_>, locale_tag: &str) -> bynk_ts::TsExpr {
    let tag_lit_str = || bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(locale_tag.to_string()));
    let fallback = || bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(format!("{{{}}}", p.name)));
    let arg_expr = || bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("params".to_string())),
            property: "get".to_string(),
        }),
        args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
            p.name.to_string(),
        ))],
    };
    let arg_tag = |property: &str| bynk_ts::TsExpr::Member {
        object: Box::new(bynk_ts::TsExpr::Ident("__arg".to_string())),
        property: property.to_string(),
    };
    let iife = |arrow_body: bynk_ts::TsExpr| bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Arrow {
            params: vec![bynk_ts::TsParam::new("__arg".to_string(), None)],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(arrow_body))),
        }),
        args: vec![arg_expr()],
    };
    match &p.kind {
        icu::PlaceholderKind::Plural { arms } => {
            let arms_obj: Vec<(String, bynk_ts::TsExpr)> = arms
                .iter()
                .map(|(cat, segs)| {
                    (
                        format!("\"{}\"", cat.as_str()),
                        emit_sub_message(segs, locale_tag),
                    )
                })
                .collect();
            // `__arg === undefined || (__arg.tag !== "Whole" && __arg.tag !== "Num")`
            // — the `&&` sub-expression is explicitly parenthesised in the
            // pre-conversion text even though `&&` already binds tighter
            // than `||` and needs no parens grammatically; reproduced via
            // an explicit `Paren`, matching the real bytes rather than the
            // precedence table's own (grammatically equivalent) choice.
            let test = bynk_ts::TsExpr::Binary {
                op: bynk_ts::TsBinaryOp::Or,
                left: Box::new(bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::StrictEq,
                    left: Box::new(bynk_ts::TsExpr::Ident("__arg".to_string())),
                    right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                }),
                right: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::And,
                    left: Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::StrictNotEq,
                        left: Box::new(arg_tag("tag")),
                        right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            "Whole".to_string(),
                        ))),
                    }),
                    right: Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::StrictNotEq,
                        left: Box::new(arg_tag("tag")),
                        right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            "Num".to_string(),
                        ))),
                    }),
                }))),
            };
            let call = bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__selectPluralArm".to_string())),
                args: vec![
                    tag_lit_str(),
                    arg_tag("value"),
                    bynk_ts::TsExpr::object(arms_obj),
                ],
            };
            iife(bynk_ts::TsExpr::Conditional {
                test: Box::new(test),
                consequent: Box::new(fallback()),
                alternate: Box::new(call),
            })
        }
        icu::PlaceholderKind::Select { arms } => {
            // The arm keys are quoted here, because a `Prop`'s key prints as given.
            let arms_obj: Vec<(String, bynk_ts::TsExpr)> = arms
                .iter()
                .map(|(k, segs)| {
                    (
                        format!("\"{}\"", escape_ts_string(k)),
                        emit_sub_message(segs, locale_tag),
                    )
                })
                .collect();
            // `Object.hasOwn` (not `?? __arms["other"]`): the arm table is an
            // object literal, so a runtime `__arg.value` naming an
            // `Object.prototype` member (`"constructor"`, `"toString"`,
            // `"__proto__"`) would resolve off the prototype chain and never
            // reach the mandatory `other` arm — `??` only fires on
            // null/undefined, and an inherited method is neither (#900). The
            // dispatch's real question is own-property presence. `Object.hasOwn`
            // is ES2022, which `emit_tsconfig` already targets.
            //
            // A block-bodied IIFE whose statements print on the one line, so its braced
            // `if` sits beside the rest (`TsArrowBody::Flat`, 1A.39a).
            let arms_ty = bynk_ts::TsType::named_with_args(
                "globalThis.Record",
                vec![
                    bynk_ts::TsType::named("string"),
                    bynk_ts::TsType::named("string"),
                ],
            );
            let arms_ref = || bynk_ts::TsExpr::Ident("__arms".to_string());
            let missing = bynk_ts::TsExpr::Binary {
                op: bynk_ts::TsBinaryOp::Or,
                left: Box::new(bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::StrictEq,
                    left: Box::new(bynk_ts::TsExpr::Ident("__arg".to_string())),
                    right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                }),
                right: Box::new(bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::StrictNotEq,
                    left: Box::new(arg_tag("tag")),
                    right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                        "Text".to_string(),
                    ))),
                }),
            };
            let has_own = bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("globalThis".to_string())),
                        property: "Object".to_string(),
                    }),
                    property: "hasOwn".to_string(),
                }),
                args: vec![arms_ref(), arg_tag("value")],
            };
            let index = |key: bynk_ts::TsExpr| bynk_ts::TsExpr::Index {
                object: Box::new(arms_ref()),
                index: Box::new(key),
            };
            let body = vec![
                bynk_ts::TsStmt::if_stmt(
                    missing,
                    bynk_ts::TsStmt::block(
                        vec![bynk_ts::TsStmt::return_stmt(Some(fallback()), None)],
                        None,
                    ),
                    None,
                ),
                bynk_ts::TsStmt::const_stmt(
                    bynk_ts::TsBindingName::Ident("__arms".to_string()),
                    Some(arms_ty),
                    bynk_ts::TsExpr::object(arms_obj),
                    None,
                ),
                bynk_ts::TsStmt::return_stmt(
                    Some(bynk_ts::TsExpr::Conditional {
                        test: Box::new(has_own),
                        consequent: Box::new(index(arg_tag("value"))),
                        alternate: Box::new(index(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            "other".to_string(),
                        )))),
                    }),
                    None,
                ),
            ];
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Arrow {
                    params: vec![bynk_ts::TsParam::new("__arg".to_string(), None)],
                    is_async: false,
                    generics: Vec::new(),
                    return_type: None,
                    body: Box::new(bynk_ts::TsArrowBody::Flat(body)),
                }),
                args: vec![arg_expr()],
            }
        }
        icu::PlaceholderKind::Number { style } => {
            let mut args = vec![tag_lit_str(), arg_tag("value")];
            if let Some(s) = style {
                args.push(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    s.as_str().to_string(),
                )));
            }
            iife(bynk_ts::TsExpr::Conditional {
                test: Box::new(bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::And,
                    left: Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::StrictNotEq,
                        left: Box::new(bynk_ts::TsExpr::Ident("__arg".to_string())),
                        right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                    }),
                    right: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::Or,
                        left: Box::new(bynk_ts::TsExpr::Binary {
                            op: bynk_ts::TsBinaryOp::StrictEq,
                            left: Box::new(arg_tag("tag")),
                            right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                "Whole".to_string(),
                            ))),
                        }),
                        right: Box::new(bynk_ts::TsExpr::Binary {
                            op: bynk_ts::TsBinaryOp::StrictEq,
                            left: Box::new(arg_tag("tag")),
                            right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                "Num".to_string(),
                            ))),
                        }),
                    }))),
                }),
                consequent: Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Ident("__formatIcuNumber".to_string())),
                    args,
                }),
                alternate: Box::new(fallback()),
            })
        }
        icu::PlaceholderKind::Date { style } => {
            let mut args = vec![tag_lit_str(), arg_tag("value")];
            if let Some(s) = style {
                args.push(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    s.as_str().to_string(),
                )));
            }
            iife(bynk_ts::TsExpr::Conditional {
                test: Box::new(bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::And,
                    left: Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::StrictNotEq,
                        left: Box::new(bynk_ts::TsExpr::Ident("__arg".to_string())),
                        right: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                    }),
                    right: Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::StrictEq,
                        left: Box::new(arg_tag("tag")),
                        right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            "Moment".to_string(),
                        ))),
                    }),
                }),
                consequent: Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Ident("__formatIcuDate".to_string())),
                    args,
                }),
                alternate: Box::new(fallback()),
            })
        }
    }
}

/// Turns one `plural`/`select` arm's sub-message into a `+`-joined TS
/// expression, evaluated inside `emit_icu_placeholder`'s IIFE where `__arg`
/// is already in scope and narrowed. `Hash` only ever occurs in a `plural`
/// arm (the parser's `allow_hash` guarantees a `select` arm never contains
/// one), where `__arg.value: number` after narrowing.
///
/// Arc C, step (11) (#1388): returns a real `bynk_ts::TsExpr` — `locale_tag`
/// (review of #1389: renamed from the pre-conversion `tag_lit`, which
/// pointed the wrong way once this stopped taking pre-quoted text) is now
/// the RAW locale tag, not pre-quoted/pre-escaped text, since `TsLit::Str`'s
/// own renderer applies the identical escaping itself (documented
/// byte-for-byte match with `escape_ts_string`).
fn emit_sub_message(segs: &[icu::SubSegment], locale_tag: &str) -> bynk_ts::TsExpr {
    if segs.is_empty() {
        return bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(String::new()));
    }
    let parts: Vec<bynk_ts::TsExpr> = segs
        .iter()
        .map(|seg| match seg {
            icu::SubSegment::Literal(s) => bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(s.clone())),
            // A bare `#` renders the argument as a number. Recorded here even
            // though the only arm that can contain one is `plural` (see this
            // function's doc), which records for itself: the note belongs with
            // the emission, so this stays correct if that invariant ever moves.
            icu::SubSegment::Hash => bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__formatIcuNumber".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(locale_tag.to_string())),
                    bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("__arg".to_string())),
                        property: "value".to_string(),
                    },
                ],
            },
        })
        .collect();
    join_plus(parts)
}

/// Compiles every `messages` block in one commons into one `code ->
/// renderer` lookup per locale, a shared dispatch object keyed by locale tag,
/// and one bundle-scoped `render(tag, msg)` that looks up the resolved
/// `tag`'s own table, else the `@reference` locale's, else falls back to
/// `bynk.locale`'s own `render` (imported under the private alias
/// `__bynkLocaleRender` — `emit_unit`, project.rs — since this file's own
/// `render` would otherwise collide with the imported name) for any code no
/// declared locale covers. Also exports the bundle's declared-locale set and
/// reference tag (message-bundles slice 2, #874, ADR 0273) — the
/// precondition Locale's own slice 2 (negotiation) needs before it can
/// start.
///
/// `blocks` is every `CommonsItem::Messages` in the commons, in source
/// order; `reference` is the one block among them carrying `@reference` —
/// the caller (`emit_project`) already found it to decide *whether* to call
/// this function at all, so it's passed in rather than re-derived here (PR
/// #875 review — a harmless but needless second scan).
/// #1355 (R7.1): converts this function's own outer construction to real
/// `bynk_ts` nodes, with two deliberate, named opaque carve-outs.
///
/// **`__messagesByLocale`'s own header/type-annotation/closing-brace stay
/// hand-written text** — `Record<string, Record<string, (params: ReadonlyMap
/// <string, MessageArg>) => string>>` needs its inner function type's one
/// parameter named `params`; `bynk_ts::TsType::Fn`'s own `params` are
/// deliberately anonymous, printer-numbered positionally (`a0`, `a1`, …) —
/// the same "an odd, one-off type shape stays opaque text" precedent P7.9
/// (#1315) already used for `Query[T]`'s own extra-paren-wrapped shape, not
/// a general `TsType::Fn` redesign for this one call site. Each LOCALE's own
/// entry (and each per-code entry nested inside it) IS a real
/// [`bynk_ts::TsObjectEntry`], though — printed through the shared
/// [`bynk_ts::print_object_entry`] fragment entry point directly into the
/// hand-written wrapper, the same "return/print real entries into a
/// still-hand-written enclosing literal" shape `emit_attached_methods`'s own
/// callers already use (#1337) — chosen here specifically because
/// `emit_doc_block`'s own per-locale doc comment has no real node shape to
/// intersperse with today (`TsObjectEntry::Prop` carries no `doc` field the
/// way `Method`'s own does; adding one for this single narrow need would be
/// disproportionate). Each per-code entry's own VALUE —
/// `emit_message_entry_renderer`'s output, one of step (11)'s own named,
/// not-yet-proposed ICU-formatting cluster — stays opaque, carried as a
/// [`bynk_ts::TsExpr::Ident`] wrapping already-formed JS, the established
/// "call an unconverted sibling helper, carry its text opaquely" pattern
/// this whole track uses.
///
/// **`__messagesReferenceLocale`/`__messagesLocales`/`render` convert fully** —
/// no opaque carve-outs, every shape they need (`TsExpr::As`, `TsExpr::
/// Index`, `TsExpr::Conditional`, `TsBinaryOp::NullishCoalescing`, `TsStmt::
/// If`/`Return`) already exists.
/// #1478: returns real [`bynk_ts::TsStmt`]s (was `out: &mut String`). The
/// `__messagesByLocale` header/entries/closing-brace block stays exactly the
/// opaque, hand-assembled text #1355's own doc above already carves out —
/// built into a local buffer instead of the caller's `out`, then carried as
/// one [`bynk_ts::TsStmtKind::Raw`] statement, the same "already real-node-
/// printed internally, still `String`-typed at its own call boundary"
/// carrier `emit_refined_type`'s own `checks_text` uses (#1478).
pub(crate) fn emit_messages_bundle(
    blocks: &[&MessagesDecl],
    reference: &MessagesDecl,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();

    // One `code -> renderer` table per locale, inlined into a single
    // `__messagesByLocale` object literal keyed by tag. No per-locale `const
    // __messages_<tag>` binding: a locale tag can be `"pt-BR"`, which is not a
    // valid TS identifier, so a named binding would be a syntax error — the
    // object is keyed by the tag *string* and needs no binding of its own.
    let record = |value: bynk_ts::TsType| {
        bynk_ts::TsType::named_with_args(
            "globalThis.Record",
            vec![bynk_ts::TsType::named("string"), value],
        )
    };
    let renderer_ty = bynk_ts::TsType::Fn {
        rest: false,
        params: vec![bynk_ts::TsType::named_with_args(
            "ReadonlyMap",
            vec![
                bynk_ts::TsType::named("string"),
                bynk_ts::TsType::named("__bynkMessageArg"),
            ],
        )],
        names: vec!["params".to_string()],
        ret: Box::new(bynk_ts::TsType::named("string")),
    };
    let locale_entries: Vec<bynk_ts::TsObjectEntry> = blocks
        .iter()
        .map(|m| {
            let code_entries: Vec<bynk_ts::TsObjectEntry> = m
                .entries
                .iter()
                .map(|entry| {
                    bynk_ts::TsObjectEntry::prop(
                        format!("\"{}\"", escape_ts_string(&entry.code)),
                        emit_message_entry_renderer(entry, &m.tag),
                    )
                })
                .collect();
            let locale_entry = bynk_ts::TsObjectEntry::prop(
                format!("\"{}\"", escape_ts_string(&m.tag)),
                bynk_ts::TsExpr::multiline_object_entries(code_entries),
            );
            match m.documentation.as_deref() {
                Some(doc) => locale_entry.with_doc(doc),
                None => locale_entry,
            }
        })
        .collect();
    stmts.push(bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::ConstDecl {
            name: "__messagesByLocale".to_string(),
            ty: Some(record(record(renderer_ty))),
            init: bynk_ts::TsExpr::multiline_object_entries(locale_entries),
        },
        None,
    ));

    // `("tag" as string) as LocaleTag` — the inner `as` is wrapped in an
    // explicit `Paren`: `as` is left-associative, so the un-parenthesised
    // `"tag" as string as LocaleTag` is the identical cast grammatically,
    // but the pre-conversion text always parenthesised it, and `As`'s own
    // renderer does not auto-add parens around a nested `As` operand. The
    // same "explicit `Paren` always prints its own literal parens"
    // precedent #1323 established.
    //
    // Review of #1356, finding 1: `tag` is the RAW locale tag, not
    // `escape_ts_string`-escaped — `TsLit::Str`'s own renderer already
    // escapes (byte-identical to `escape_ts_string`, deliberately, P7.8),
    // so pre-escaping here would double-escape any backslash/quote a tag
    // ever contains, corrupting the literal's own decoded value. Not
    // reachable today (`LocaleTag`'s own `Matches(...)` refinement rejects
    // anything but letters/digits/hyphens) but a real, latent bug had the
    // caller pre-escaped, the same class of hazard #1335's own `msg`
    // deviation was written to avoid. `__messagesByLocale`'s own `Prop` KEY a
    // few lines up is the opposite, correct case: a key is spliced
    // verbatim, never run through a printer escaper, so it needs the
    // explicit `escape_ts_string` call to be safe at all.
    let tag_cast = |tag: &str| bynk_ts::TsExpr::As {
        expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
            expr: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(tag.to_string()))),
            ty: bynk_ts::TsType::named("string"),
        }))),
        ty: bynk_ts::TsType::named("__bynkLocaleTag"),
    };
    let ref_locale_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: "__messagesReferenceLocale".to_string(),
            ty: Some(bynk_ts::TsType::named("__bynkLocaleTag")),
            init: tag_cast(&reference.tag),
        })),
        None,
    );
    stmts.push(ref_locale_decl);

    let locale_list: Vec<bynk_ts::TsExpr> = blocks.iter().map(|m| tag_cast(&m.tag)).collect();
    // #1486: no blank wanted between `ref_locale_decl` and `locales_decl` —
    // matches the pre-conversion hand-written text this crate's own
    // zero-diff discipline requires preserving exactly, not expressible
    // through `bynk_ts::print`'s own uniform "always one blank" policy
    // without this escape hatch (see `TsStmt::no_blank_before`'s own doc).
    let mut locales_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: "__messagesLocales".to_string(),
            ty: Some(bynk_ts::TsType::readonly_array(bynk_ts::TsType::named(
                "__bynkLocaleTag",
            ))),
            init: bynk_ts::TsExpr::array(locale_list),
        })),
        None,
    );
    locales_decl.no_blank_before = true;
    stmts.push(locales_decl);

    let not_undefined = |e: bynk_ts::TsExpr| bynk_ts::TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::StrictNotEq,
        left: Box::new(e),
        right: Box::new(ident("undefined")),
    };

    let local_table_decl = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__localeTable".to_string()),
        None,
        index(ident("__messagesByLocale"), ident("tag")),
        None,
    );
    let reference_table_decl = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__referenceTable".to_string()),
        None,
        index(
            ident("__messagesByLocale"),
            ident("__messagesReferenceLocale"),
        ),
        None,
    );
    let entry_ternary = bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Conditional {
        test: Box::new(not_undefined(ident("__localeTable"))),
        consequent: Box::new(index(ident("__localeTable"), member(ident("msg"), "code"))),
        alternate: Box::new(ident("undefined")),
    }));
    let entry_decl = bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__entry".to_string()),
        None,
        bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::NullishCoalescing,
            left: Box::new(entry_ternary),
            right: Box::new(index(
                ident("__referenceTable"),
                member(ident("msg"), "code"),
            )),
        },
        None,
    );
    let if_entry = bynk_ts::TsStmt::if_stmt(
        not_undefined(ident("__entry")),
        bynk_ts::TsStmt::block(
            vec![bynk_ts::TsStmt::return_stmt(
                Some(bynk_ts::TsExpr::Call {
                    callee: Box::new(ident("__entry")),
                    args: vec![member(ident("msg"), "params")],
                }),
                None,
            )],
            None,
        ),
        None,
    );
    let fallback_return = bynk_ts::TsStmt::return_stmt(
        Some(bynk_ts::TsExpr::Call {
            callee: Box::new(ident("__bynkLocaleRender")),
            args: vec![ident("tag"), ident("msg")],
        }),
        None,
    );

    let render_fn = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Function {
            name: "render".to_string(),
            generics: Vec::new(),
            params: vec![
                bynk_ts::TsParam::new(
                    "tag".to_string(),
                    Some(bynk_ts::TsType::named("__bynkLocaleTag")),
                ),
                bynk_ts::TsParam::new(
                    "msg".to_string(),
                    Some(bynk_ts::TsType::named("__bynkMessage")),
                ),
            ],
            return_type: Some(bynk_ts::TsType::named("string")),
            body: vec![
                local_table_decl,
                reference_table_decl,
                entry_decl,
                if_entry,
                fallback_return,
            ],
            is_async: false,
            inline: false,
        })),
        None,
    );
    stmts.push(render_fn);
    stmts
}

#[cfg(test)]
mod messages_template_tests {
    use crate::emitter::{TemplateSegment, placeholder_names, split_template};
    use std::collections::BTreeSet;

    fn kinds(template: &str) -> Vec<(&str, &str)> {
        split_template(template)
            .into_iter()
            .map(|s| match s {
                TemplateSegment::Literal(l) => ("lit", l),
                TemplateSegment::Placeholder { inner, .. } => ("ph", inner),
            })
            .collect()
    }

    #[test]
    fn literal_only_template_is_one_segment() {
        assert_eq!(kinds("Bye"), vec![("lit", "Bye")]);
    }

    #[test]
    fn empty_template_is_one_empty_literal_segment() {
        // emit_message_entry_renderer's `parts.is_empty()` guard exists
        // specifically because split_template never returns zero segments —
        // this pins that invariant.
        assert_eq!(kinds(""), vec![("lit", "")]);
    }

    #[test]
    fn single_placeholder_with_surrounding_literal_text() {
        assert_eq!(
            kinds("Hello, {name}!"),
            vec![("lit", "Hello, "), ("ph", "name"), ("lit", "!")],
        );
    }

    #[test]
    fn placeholder_at_the_very_start_or_end() {
        assert_eq!(kinds("{name}!"), vec![("ph", "name"), ("lit", "!")]);
        assert_eq!(kinds("Hi, {name}"), vec![("lit", "Hi, "), ("ph", "name")]);
    }

    #[test]
    fn back_to_back_placeholders() {
        assert_eq!(kinds("{a}{b}"), vec![("ph", "a"), ("ph", "b")],);
    }

    #[test]
    fn unmatched_brace_and_empty_braces_are_literal_text() {
        // No closing `}` at all.
        assert_eq!(kinds("a {b"), vec![("lit", "a {b")]);
        // Empty `{}` names nothing, so it's not a placeholder either.
        assert_eq!(kinds("a {} b"), vec![("lit", "a {} b")]);
    }

    #[test]
    fn multibyte_literal_text_around_a_placeholder() {
        // A non-ASCII literal must not panic the byte-index scan and must
        // round-trip intact (UTF-8 char-boundary safety, not just byte count).
        assert_eq!(
            kinds("caf\u{e9} {name} \u{1f980}"),
            vec![("lit", "caf\u{e9} "), ("ph", "name"), ("lit", " \u{1f980}")],
        );
    }

    // -- message-bundles slice 3 (#878): ICU-dispatch placeholders --

    #[test]
    fn icu_plural_placeholder_captures_full_inner_text_with_nested_braces() {
        assert_eq!(
            kinds("You have {n, plural, one {# item} other {# items}}."),
            vec![
                ("lit", "You have "),
                ("ph", "n, plural, one {# item} other {# items}"),
                ("lit", "."),
            ],
        );
    }

    #[test]
    fn icu_placeholder_and_plain_placeholder_side_by_side() {
        assert_eq!(
            kinds("{name}: {n, number}"),
            vec![("ph", "name"), ("lit", ": "), ("ph", "n, number")],
        );
    }

    #[test]
    fn icu_placeholder_with_no_true_close_falls_back_to_literal() {
        // A `,` precedes the naive first `}`, so this is ICU-dispatch — but
        // the arm's own `{` is never closed, so `find_icu_close` never
        // reaches depth 0. Same "just literal text" policy as an unmatched
        // plain `{`.
        assert_eq!(
            kinds("{n, plural, one {# item"),
            vec![("lit", "{n, plural, one {# item")],
        );
    }

    #[test]
    fn placeholder_names_trims_icu_names_but_not_plain_ones() {
        assert_eq!(placeholder_names("{ name }"), BTreeSet::from([" name "]));
        assert_eq!(
            placeholder_names("{ n , plural, one {#} other {#}}"),
            BTreeSet::from(["n"])
        );
    }
}
