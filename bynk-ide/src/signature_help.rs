//! v0.32 (ADR 0065): signature help — `textDocument/signatureHelp`.
//!
//! While typing a call's arguments, show the callee's signature with the active
//! parameter highlighted. **Call-context detection is lexical** (the innermost
//! unclosed `(` before the cursor, the callee before it, the active parameter
//! from a bracket-aware comma count); **signatures are semantic**, resolved from
//! the recovery parse + the static registries — the same name-vs-value split as
//! completion. This slice covers **name callees**: free functions, capability
//! operations, refined/opaque `of`/`unsafe`, built-in type statics
//! (`Int.parse`/`Json.decode`), and the `Ok`/`Err`/`Some` constructors. Value
//! receivers (`xs.fold(`) need the receiver typed → a later slice.
//!
//! The signature is rendered with `symbols::type_ref_str` — the same Bynk-syntax
//! renderer hover uses — so the two never diverge.

use bynk_syntax::ast::{BaseType, CommonsItem, FnName, SourceUnit, TypeBody};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use crate::completion::{BUILTIN_STATICS, for_each_unit};
use crate::symbols::type_ref_str;

/// The call under the cursor: the callee text, the active-parameter index, and
/// the byte offset of the call's opening `(`.
#[derive(Debug, PartialEq, Eq)]
pub struct CallContext {
    pub callee: String,
    pub active_param: usize,
    pub open_paren: usize,
}

/// The innermost unclosed `(` before `offset`, its callee, and the active
/// parameter (top-level commas between that `(` and the cursor). `None` when the
/// cursor is not inside a call's argument list.
pub fn call_context(text: &str, offset: usize) -> Option<CallContext> {
    let prefix = text.get(..offset)?;
    let open = innermost_unclosed_paren(prefix)?;
    let callee = callee_before(&prefix[..open])?;
    let active = top_level_commas(&prefix[open + 1..]);
    Some(CallContext {
        callee,
        active_param: active,
        open_paren: open,
    })
}

/// A value-receiver method callee — `recv.method` where `recv` is a single
/// lowercase-initial identifier (a value, not a type/capability name). The
/// signature comes from typing the receiver (a later slice's path).
pub fn value_receiver_method(callee: &str) -> Option<(&str, &str)> {
    let (recv, method) = callee.rsplit_once('.')?;
    let first = recv.chars().next()?;
    if (first.is_ascii_lowercase() || first == '_') && !recv.contains('.') {
        Some((recv, method))
    } else {
        None
    }
}

/// For a value-receiver callee `recv.method(` whose `(` is at `open_paren`,
/// rewrite the buffer so `recv` is a complete expression (the `.method(args`
/// dropped) and return it with the receiver byte offset to type — the same
/// mid-edit trick value-member completion uses.
pub fn value_receiver_rewrite(
    text: &str,
    callee: &str,
    open_paren: usize,
    cursor: usize,
) -> Option<(String, usize)> {
    let (recv, _) = value_receiver_method(callee)?;
    let before = strip_type_args(text.get(..open_paren)?).trim_end();
    let callee_start = before.len().checked_sub(callee.len())?;
    let dot = callee_start + recv.len();
    let rewritten = format!("{}{}", &text[..dot], &text[cursor..]);
    Some((rewritten, dot.saturating_sub(1)))
}

/// The kernel-method signature for `method` on receiver type `ty`, if any: the
/// first form when the method has several arities.
pub fn kernel_method_signature(
    ty: bynk_check::checker::TyId,
    tys: &bynk_check::checker::Types,
    method: &str,
) -> Option<String> {
    kernel_method_signatures(ty, tys, method).into_iter().next()
}

/// #4262: every form of the kernel method `method` on receiver type `ty`, one
/// signature per arity; empty when there is no such method.
pub fn kernel_method_signatures(
    ty: bynk_check::checker::TyId,
    tys: &bynk_check::checker::Types,
    method: &str,
) -> Vec<String> {
    // #3955: `compare` applies to every orderable receiver. A named type is
    // taken as orderable here (its `derives` is not known to this lookup; a
    // receiver that is not `Ord` is reported by the checker at the call).
    let ord = if bynk_check::kernel_methods::is_ord_receiver(ty, tys, &|_| true) {
        bynk_check::kernel_methods::ORD_RECEIVER_TABLE
    } else {
        &[]
    };
    // #4560: `wireName` applies to every enum; a named sum is taken as one here, as for `ord`
    // (a receiver that is not an enum is reported by the checker at the call).
    let enum_table = if bynk_check::kernel_methods::is_enum_receiver(ty, tys, &|_| true) {
        bynk_check::kernel_methods::ENUM_RECEIVER_TABLE
    } else {
        &[]
    };
    bynk_check::kernel_methods::methods_for(ty, tys)
        .iter()
        .chain(ord)
        .chain(enum_table)
        .find(|m| m.name == method)
        .map(|m| m.signatures.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

/// #4262: which of a method's forms signature help marks active while the
/// parameter at index `active_param` is being typed: the narrowest form that
/// has that parameter, else (the call has more arguments than any form takes)
/// the widest form, as the closest fit. Ties go to the earlier form.
pub fn active_signature(labels: &[String], active_param: usize) -> usize {
    let widths: Vec<usize> = labels.iter().map(|l| param_ranges(l).len()).collect();
    let fitting = widths
        .iter()
        .enumerate()
        .filter(|(_, w)| **w > active_param)
        .min_by_key(|(_, w)| **w);
    let widest = || {
        widths
            .iter()
            .enumerate()
            .max_by_key(|(i, w)| (**w, std::cmp::Reverse(*i)))
    };
    fitting.or_else(widest).map_or(0, |(i, _)| i)
}

/// Scan back for the `(` that is open at the cursor. A depth-0 `[` or `{` means
/// the cursor sits in a type-argument list / list literal / block, not a call.
fn innermost_unclosed_paren(prefix: &str) -> Option<usize> {
    let b = prefix.as_bytes();
    let mut depth = 0i32;
    for i in (0..b.len()).rev() {
        match b[i] {
            b')' | b']' | b'}' => depth += 1,
            b'(' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            b'[' | b'{' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Top-level (bracket-depth-0) commas in `s`.
fn top_level_commas(s: &str) -> usize {
    let mut depth = 0i32;
    let mut n = 0;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => n += 1,
            _ => {}
        }
    }
    n
}

/// #2795: drop a trailing explicit type-argument list — `b.map[String]` →
/// `b.map` — so `name[T](` finds the same callee as `name(`. The list's commas
/// are inside the brackets and never counted as argument separators.
fn strip_type_args(s: &str) -> &str {
    let t = s.trim_end();
    if !t.ends_with(']') {
        return s;
    }
    let mut depth = 0i32;
    for (i, c) in t.char_indices().rev() {
        match c {
            ']' => depth += 1,
            '[' => {
                depth -= 1;
                if depth == 0 {
                    return &t[..i];
                }
            }
            _ => {}
        }
    }
    s
}

/// The callee immediately before the `(` — a bare `name` or `Recv.member`.
fn callee_before(s: &str) -> Option<String> {
    let s = strip_type_args(s).trim_end();
    // Advance past the matched char by its UTF-8 length; a multi-byte
    // non-identifier char (`"`, `€`, `—`, …) would make `i + 1` land
    // mid-codepoint and panic the slice.
    let start = s
        .char_indices()
        .rfind(|&(_, c)| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let callee = &s[start..];
    if callee.is_empty() || callee.starts_with('.') || callee.ends_with('.') {
        return None;
    }
    Some(callee.to_string())
}

/// #3881: whether a unit in the project declares a sum variant named `name`, which
/// then shadows a built-in variant of the same name.
fn declares_variant(name: &str, doc_text: &str, files: Option<&HashMap<PathBuf, String>>) -> bool {
    let mut declared = false;
    for_each_unit(doc_text, files, |unit| {
        let items = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            SourceUnit::Suite(_) => return,
        };
        declared |= items.iter().any(|item| {
            matches!(item, CommonsItem::Type(t)
                if matches!(&t.body, bynk_syntax::ast::TypeBody::Sum(s)
                    if s.variants.iter().any(|v| v.name.name == name)))
        });
    });
    declared
}

/// Render the signature *label* for a name callee — `name(p: T, …) -> R`.
/// `None` if the callee can't be resolved (or is a value receiver — slice 2).
pub fn resolve_label(
    callee: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    if let Some((recv, member)) = callee.rsplit_once('.') {
        // #3894: `<context>.<service>.outcome(…)`.
        if member == "outcome"
            && let Some(label) = crate::call_outcome::outcome_signature(recv, doc_text, files)
        {
            return Some(label);
        }
        // #3446: `Record.field.cookie` / `Record.field.expire` on a cookie record.
        if let Some((record, field)) = recv.split_once('.') {
            return cookie_field_label(record, field, member, doc_text, files);
        }
        // #3147: `D1.peek(…)`, a case's handle on a participant's database.
        if callee == "D1.peek" {
            return Some(crate::completion::D1_PEEK_SIGNATURE.to_string());
        }
        // Built-in type statics — already display-ready signature strings.
        if let Some((_, statics)) = BUILTIN_STATICS.iter().find(|(n, _)| *n == recv)
            && let Some((_, sig)) = statics.iter().find(|(n, _)| *n == member)
        {
            return Some((*sig).to_string());
        }
        // #3880: a queue service's `svc.message(…)` test address.
        if member == "message"
            && let Some(label) = crate::queue_delivery::message_address_label(recv, doc_text, files)
        {
            return Some(label);
        }
        // #3879: `Instance.sendValue*(…)` on a queue the project consumes.
        if let Some(label) = crate::queue_delivery::send_value_label(recv, member, doc_text, files)
        {
            return Some(label);
        }
        // A refined/opaque type's `of`/`unsafe`, or a capability op.
        return resolve_qualified(recv, member, doc_text, files);
    }
    // Built-in constructors.
    match callee {
        "Ok" => return Some("Ok(value: T) -> Result[T, E]".to_string()),
        "Err" => return Some("Err(error: E) -> Result[T, E]".to_string()),
        "Some" => return Some("Some(value: T) -> Option[T]".to_string()),
        // #3881: a queue verdict that retries after a delay.
        "RetryAfter" if !declares_variant(callee, doc_text, files) => {
            return Some("RetryAfter(reason: String, delay: Duration) -> QueueResult".to_string());
        }
        _ => {}
    }
    // A free function.
    let mut found = None;
    for_each_unit(doc_text, files, |unit| {
        if found.is_some() {
            return;
        }
        // #2834: a suite's helper `fn`s are callable from its own cases.
        let suite_fns: Vec<CommonsItem> = match unit {
            SourceUnit::Suite(s) => s.fns.iter().cloned().map(CommonsItem::Fn).collect(),
            _ => Vec::new(),
        };
        let items = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            SourceUnit::Suite(_) => &suite_fns,
        };
        for item in items {
            if let CommonsItem::Fn(f) = item
                && let FnName::Free(id) = &f.name
                && id.name == callee
            {
                let params: Vec<String> = f
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)))
                    .collect();
                found = Some(format!(
                    "{callee}{}({}) -> {}{}{}",
                    crate::symbols::fn_type_params_str(f),
                    params.join(", "),
                    type_ref_str(&f.return_type),
                    crate::symbols::fn_given_str(f),
                    crate::symbols::fn_where_str(f)
                ));
                return;
            }
        }
    });
    // #2620: the ambient fault built-ins, unless a `fn` of that name shadows them.
    found.or_else(|| match callee {
        "unreachable" | "todo" => Some(format!("{callee}(message: String) -> never returns")),
        _ => None,
    })
}

/// #3446: the signature of `Record.field.cookie(value)` and `Record.field.expire()`.
fn cookie_field_label(
    record: &str,
    field: &str,
    member: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    let fields = crate::completion::cookie_record_fields_of(record, doc_text, files)?;
    let (_, ty, _) = fields.into_iter().find(|(n, _, _)| n == field)?;
    match member {
        "cookie" => {
            let value = match &ty {
                bynk_syntax::ast::TypeRef::Option(inner, _) => type_ref_str(inner),
                other => type_ref_str(other),
            };
            Some(format!("cookie(value: {value}) -> Cookie"))
        }
        "expire" => Some("expire() -> Cookie".to_string()),
        _ => None,
    }
}

fn resolve_qualified(
    recv: &str,
    member: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    resolve_qualified_from(recv, member, doc_text, files, &mut BTreeSet::new())
}

/// [`resolve_qualified`], with the derived capabilities already followed
/// (S4a, #1966). Signature help reads unchecked source, so a derivation cycle
/// (`capability A = B`, `capability B = A`) can be on screen: each receiver
/// is followed once.
fn resolve_qualified_from(
    recv: &str,
    member: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
    followed: &mut BTreeSet<String>,
) -> Option<String> {
    followed.insert(recv.to_string());
    let mut out = None;
    // S4a (#1966): a derived capability's (`capability Sessions = Kv`) base,
    // whose operations it has.
    let mut derived_base: Option<String> = None;
    for_each_unit(doc_text, files, |unit| {
        if out.is_some() {
            return;
        }
        let items = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            _ => return,
        };
        for item in items {
            match item {
                // `Type.of` / `Type.unsafe` for a refined/opaque type.
                CommonsItem::Type(t)
                    if t.name.name == recv && (member == "of" || member == "unsafe") =>
                {
                    let (base, total) = match &t.body {
                        TypeBody::Refined {
                            base, refinement, ..
                        } => (
                            base_name(*base),
                            bynk_syntax::ast::of_is_total(*base, refinement.as_ref(), false),
                        ),
                        // #4111: a `Secret`'s `of` cannot fail; every other opaque type's can.
                        TypeBody::Opaque { base, .. } => (
                            base_name(*base),
                            bynk_check::context_checks::is_secret_decl(t),
                        ),
                        _ => return,
                    };
                    out = Some(if member == "of" && total {
                        // #3946: a named type with no predicate cannot fail.
                        format!("of(value: {base}) -> {recv}")
                    } else if member == "of" {
                        format!("of(value: {base}) -> Result[{recv}, ValidationError]")
                    } else {
                        format!("unsafe(value: {base}) -> {recv}")
                    });
                    return;
                }
                CommonsItem::Capability(c) if c.name.name == recv && c.derived.is_some() => {
                    derived_base = c.derived.as_ref().map(|d| d.base.name.clone());
                }
                // #3386: `User.issue(identity)` / `User.expire()` on a `SignedCookie` actor.
                CommonsItem::Actor(a)
                    if a.name.name == recv && crate::completion::is_signed_cookie_actor(a) =>
                {
                    out = crate::completion::signed_cookie_members(a)
                        .into_iter()
                        .find(|(label, _)| *label == member)
                        .map(|(_, sig)| sig);
                    return;
                }
                // #3939: `Client.mint(identity, label)` on an `ApiKey` actor.
                CommonsItem::Actor(a)
                    if a.name.name == recv && crate::api_key_completion::is_api_key_actor(a) =>
                {
                    out = crate::api_key_completion::members(a)
                        .into_iter()
                        .find(|(label, _)| *label == member)
                        .map(|(_, sig)| sig);
                    return;
                }
                // `Cap.op` — a capability operation.
                CommonsItem::Capability(c) if c.name.name == recv => {
                    if let Some(op) = c.ops.iter().find(|o| o.name.name == member) {
                        let params: Vec<String> = op
                            .params
                            .iter()
                            .map(|p| format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)))
                            .collect();
                        // #926: `[T, …]` type parameters on the op itself.
                        let type_params = if op.type_params.is_empty() {
                            String::new()
                        } else {
                            let names: Vec<&str> = op
                                .type_params
                                .iter()
                                .map(|tp| tp.name.name.as_str())
                                .collect();
                            format!("[{}]", names.join(", "))
                        };
                        out = Some(format!(
                            "{member}{type_params}({}) -> {}",
                            params.join(", "),
                            type_ref_str(&op.return_type)
                        ));
                        return;
                    }
                }
                _ => {}
            }
        }
    });
    // #4502, #4099: `AnalyticsSql.select[T]` and `Ai.generateValue[T]` are compiler-lowered, so declared nowhere.
    if out.is_none()
        && matches!(recv, "AnalyticsSql" | "Ai")
        && crate::analytics_tooling::consumes_cloudflare(doc_text, recv)
    {
        out = typed_read(recv, member).map(str::to_string);
    }
    match (out, derived_base) {
        // #2865: a derived `D1`'s typed reads are lowered by the compiler, so
        // `D1` declares no operation for them.
        (None, Some(base)) if typed_read(&base, member).is_some() => {
            typed_read(&base, member).map(str::to_string)
        }
        (None, Some(base)) if !followed.contains(&base) => {
            resolve_qualified_from(&base, member, doc_text, files, followed)
        }
        (out, _) => out,
    }
}

/// #2865: the signature of `D1.select[T]` / `D1.firstAs[T]`, compiler-lowered
/// over `all`/`first` (so not declared operations of the capability).
/// Hyperdrive H3 (#3320): and of `Postgres`' (`base`), whose parameters are
/// `PgValue`s and whose failure is a `PgReadError`; H5 (#3322): and of
/// `MySql`'s, with `MySqlValue`s and a `MySqlReadError`.
pub(crate) fn typed_read(base: &str, member: &str) -> Option<&'static str> {
    match (base, member) {
        ("D1", "select") => Some(
            "select[T](tail: String, params: List[SqlValue]) -> Effect[Result[List[T], SqlReadError]]",
        ),
        ("D1", "firstAs") => Some(
            "firstAs[T](tail: String, params: List[SqlValue]) -> Effect[Result[Option[T], SqlReadError]]",
        ),
        ("Postgres", "select") => Some(
            "select[T](tail: String, params: List[PgValue]) -> Effect[Result[List[T], PgReadError]]",
        ),
        ("Postgres", "firstAs") => Some(
            "firstAs[T](tail: String, params: List[PgValue]) -> Effect[Result[Option[T], PgReadError]]",
        ),
        ("MySql", "select") => Some(
            "select[T](tail: String, params: List[MySqlValue]) -> Effect[Result[List[T], MySqlReadError]]",
        ),
        ("MySql", "firstAs") => Some(
            "firstAs[T](tail: String, params: List[MySqlValue]) -> Effect[Result[Option[T], MySqlReadError]]",
        ),
        // #4502: `Analytics.writeValue[T]` writes a record's mapped columns; `AnalyticsSql.select[T]`
        // reads them back (the SQL tail may continue the select list).
        // #4099: `Ai.generateValue[T]` asks the model for JSON that matches `T`'s schema.
        ("Ai", "generateValue") => Some(
            "generateValue[T](model: String, request: GenerateRequest) -> Effect[Result[T, AiError]]",
        ),
        ("Analytics", "writeValue") => Some("writeValue[T](value: T) -> Effect[()]"),
        ("AnalyticsSql", "select") => Some(
            "select[T](dataset: Analytics instance, tail: String, params: List[SqlValue]) -> Effect[Result[List[T], AnalyticsReadError]]",
        ),
        _ => None,
    }
}

/// The bases whose derived instances have the compiler-lowered typed reads.
pub(crate) const TYPED_READ_BASES: [&str; 6] =
    ["D1", "Postgres", "MySql", "Analytics", "AnalyticsSql", "Ai"];

/// Whether `label` is the typed-read signature of the member `callee` names, so a
/// user's own `select` operation is never taken for one.
pub(crate) fn is_typed_read_label(callee: &str, label: &str) -> bool {
    callee.rsplit_once('.').is_some_and(|(_, m)| {
        TYPED_READ_BASES
            .iter()
            .any(|b| typed_read(b, m) == Some(label))
    })
}

fn base_name(b: BaseType) -> &'static str {
    match b {
        BaseType::Int => "Int",
        BaseType::Float => "Float",
        BaseType::String => "String",
        BaseType::Bool => "Bool",
        BaseType::Duration => "Duration",
        BaseType::Instant => "Instant",
        BaseType::Bytes => "Bytes",
        BaseType::Decimal => "Decimal",
    }
}

/// The byte ranges of each top-level parameter within a signature `label`
/// (`name(p0, p1, …) -> R`) — for the LSP `ParameterInformation` offsets.
pub fn param_ranges(label: &str) -> Vec<(usize, usize)> {
    let Some(open) = label.find('(') else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    let mut depth = 0i32;
    let mut seg_start = open + 1;
    let bytes = label.as_bytes();
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    push_trimmed(label, seg_start, i, &mut ranges);
                    break;
                }
            }
            b',' if depth == 1 => {
                push_trimmed(label, seg_start, i, &mut ranges);
                seg_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    ranges
}

fn push_trimmed(label: &str, start: usize, end: usize, out: &mut Vec<(usize, usize)>) {
    let seg = &label[start..end];
    let trimmed = seg.trim();
    if trimmed.is_empty() {
        return;
    }
    let s = start + (seg.len() - seg.trim_start().len());
    out.push((s, s + trimmed.len()));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Constrained generics slice 0: the label shows a generic's bounds.
    #[test]
    fn free_fn_label_shows_bounds() {
        let doc = "commons m\n\nfn same[T: Eq](a: T, b: T) -> Bool { a == b }\n\nfn pick[T](a: T) -> T { a }\n";
        assert_eq!(
            resolve_label("same", doc, None).as_deref(),
            Some("same[T: Eq](a: T, b: T) -> Bool")
        );
        assert_eq!(
            resolve_label("pick", doc, None).as_deref(),
            Some("pick[T](a: T) -> T")
        );
        let label = resolve_label("same", doc, None).unwrap();
        let ranges = param_ranges(&label);
        assert_eq!(&label[ranges[0].0..ranges[0].1], "a: T");
    }

    /// #2868: the label shows a context function's `given`, ahead of any `where`;
    /// the hidden `deps` parameter is not in the AST, so it is never offered.
    #[test]
    fn free_fn_label_shows_given() {
        let doc = "context m\n\nfn warn(what: String) -> Effect[()] given Logger, B.Orders {\n  Logger.error(what)\n}\n";
        let label = resolve_label("warn", doc, None).expect("warn resolves");
        assert_eq!(
            label,
            "warn(what: String) -> Effect[()] given Logger, B.Orders"
        );
        assert_eq!(param_ranges(&label).len(), 1);
    }

    /// Constrained generics slice 1: the label shows `Ord` bounds. The hidden
    /// comparator-witness parameters the emitter adds are not in the AST, so
    /// they are never offered as a parameter or in the label.
    #[test]
    fn free_fn_label_shows_ord_bounds_and_never_the_witness() {
        let doc = "commons m\n\nfn top[T: Ord](xs: List[T], n: Int) -> List[T] { xs }\n\nfn both[A: Eq + Ord, B: Ord](a: A, b: B) -> A { a }\n";
        let label = resolve_label("top", doc, None).unwrap();
        assert_eq!(label, "top[T: Ord](xs: List[T], n: Int) -> List[T]");
        let ranges = param_ranges(&label);
        let params: Vec<&str> = ranges.iter().map(|&(s, e)| &label[s..e]).collect();
        assert_eq!(params, ["xs: List[T]", "n: Int"]);
        let both = resolve_label("both", doc, None).unwrap();
        assert_eq!(both, "both[A: Eq + Ord, B: Ord](a: A, b: B) -> A");
        for l in [&label, &both] {
            let lower = l.to_lowercase();
            assert!(
                !lower.contains("witness") && !lower.contains("cmp") && !lower.contains("compar"),
                "{l}"
            );
        }
        assert_eq!(param_ranges(&both).len(), 2);
    }

    #[test]
    fn call_context_finds_callee_and_active_param() {
        let t = "  let x = f(a, b";
        let ctx = call_context(t, t.len()).unwrap();
        assert_eq!(ctx.callee, "f");
        assert_eq!(ctx.active_param, 1); // after one comma
    }

    #[test]
    fn an_explicit_type_argument_list_is_not_the_callee_or_an_argument() {
        // #2795: `b.map[String](` resolves like `b.map(`, and the list's comma
        // is not an argument separator.
        let t = "  b.map[String](f, ";
        let ctx = call_context(t, t.len()).unwrap();
        assert_eq!(ctx.callee, "b.map");
        assert_eq!(ctx.active_param, 1);
        let t = "  b.pair[Int, String](";
        let ctx = call_context(t, t.len()).unwrap();
        assert_eq!(ctx.callee, "b.pair");
        assert_eq!(ctx.active_param, 0);
        let (rw, _) = value_receiver_rewrite(t, &ctx.callee, ctx.open_paren, t.len()).unwrap();
        assert_eq!(rw, "  b");
    }

    #[test]
    fn innermost_call_wins_in_nested_calls() {
        let t = "  outer(g(x";
        let ctx = call_context(t, t.len()).unwrap();
        assert_eq!(ctx.callee, "g");
        assert_eq!(ctx.active_param, 0);
        // back out to the outer call's second arg
        let t2 = "  outer(g(x), ";
        let ctx2 = call_context(t2, t2.len()).unwrap();
        assert_eq!(ctx2.callee, "outer");
        assert_eq!(ctx2.active_param, 1);
    }

    #[test]
    fn commas_inside_nested_brackets_dont_count() {
        let t = "  f(g(a, b), ";
        assert_eq!(call_context(t, t.len()).unwrap().active_param, 1);
    }

    #[test]
    fn qualified_callee_and_no_call_context() {
        let t = "    Clock.now(";
        assert_eq!(call_context(t, t.len()).unwrap().callee, "Clock.now");
        assert!(call_context("  let x = 1", 11).is_none()); // not in a call
        assert!(call_context("  xs[", 4).is_none()); // a list index, not a call
    }

    #[test]
    fn callee_extraction_survives_a_multibyte_char_before_the_callee() {
        // A multi-byte non-identifier char before the callee used to make the
        // `i + 1` byte offset land mid-codepoint → panic on slice (#715). `(`
        // is a trigger char, so this fired on signature help inside a string.
        assert_eq!(callee_before("\"Foo.bar"), Some("Foo.bar".to_string()));
        assert_eq!(callee_before("€f"), Some("f".to_string()));
        // The whole path: a call context whose prefix opens a string literal.
        let t = "  \"€greet(";
        let _ = call_context(t, t.len());
    }

    /// S4a (#1966): a derived capability's operation has its base's signature.
    #[test]
    fn a_derived_capability_op_has_its_bases_signature() {
        const DERIVED_DOC: &str =
            "context a\n\nconsumes bynk.cloudflare { Kv }\n\ncapability Sessions = Kv\n";
        assert_eq!(
            resolve_label("Sessions.put", DERIVED_DOC, None).as_deref(),
            Some("put(key: String, value: String) -> Effect[()]")
        );
    }

    /// #2865: a derived `D1` has the compiler-lowered typed reads.
    #[test]
    fn a_derived_d1_has_the_typed_reads() {
        const DOC: &str =
            "context a\n\nconsumes bynk.cloudflare { D1 }\n\ncapability Orders = D1\n";
        assert_eq!(
            resolve_label("Orders.select", DOC, None).as_deref(),
            Some(
                "select[T](tail: String, params: List[SqlValue]) -> Effect[Result[List[T], SqlReadError]]"
            )
        );
        assert!(resolve_label("Orders.firstAs", DOC, None).is_some());
        // #3147: and the case-only `D1.peek`, which is no operation.
        assert!(
            resolve_label("D1.peek", DOC, None)
                .is_some_and(|l| l.starts_with("peek(<participant>"))
        );
        assert!(resolve_label("Orders.all", DOC, None).is_some());
    }

    /// #4502: the typed Analytics operations have signature help for their type argument.
    #[test]
    fn the_typed_analytics_operations_have_signatures() {
        const DOC: &str = "context a\n\nconsumes bynk.cloudflare { Analytics, AnalyticsSql }\n\ncapability Signups = Analytics\n";
        assert_eq!(
            resolve_label("Signups.writeValue", DOC, None).as_deref(),
            Some("writeValue[T](value: T) -> Effect[()]")
        );
        let select = resolve_label("AnalyticsSql.select", DOC, None).expect("a signature");
        assert!(
            select.starts_with("select[T](dataset: Analytics instance, tail: String"),
            "{select}"
        );
        assert!(select.contains("AnalyticsReadError"), "{select}");
    }

    /// #4100: `Ai.generateStream` is a declared operation, so signature help reads it as any other.
    #[test]
    fn ai_generate_stream_has_a_signature() {
        const DOC: &str = "context a\n\nconsumes bynk.cloudflare { Ai }\n";
        assert_eq!(
            resolve_label("Ai.generateStream", DOC, None).as_deref(),
            Some(
                "generateStream(model: String, request: GenerateRequest) -> Effect[Result[Stream[String], AiError]]"
            )
        );
    }

    /// #4099: `Ai.generateValue[T]` has a signature for its type argument and its two parameters.
    #[test]
    fn ai_generate_value_has_a_signature() {
        const DOC: &str = "context a\n\nconsumes bynk.cloudflare { Ai }\n";
        assert_eq!(
            resolve_label("Ai.generateValue", DOC, None).as_deref(),
            Some(
                "generateValue[T](model: String, request: GenerateRequest) -> Effect[Result[T, AiError]]"
            )
        );
        // The declared operations keep theirs.
        assert!(resolve_label("Ai.generate", DOC, None).is_some());
        // A project's own `Ai` is not the first-party one, so it has no such operation.
        let own = "context a\n\ncapability Ai {\n  fn ask(q: String) -> Effect[String]\n}\n";
        assert!(resolve_label("Ai.generateValue", own, None).is_none());
    }

    /// Hyperdrive H3 (#3320): a derived `Postgres` has them too, over `PgValue`
    /// and `PgReadError`, and `transaction` is its declared operation.
    #[test]
    fn a_derived_postgres_has_the_typed_reads_and_transaction() {
        const DOC: &str =
            "context a\n\nconsumes bynk.cloudflare { Postgres }\n\ncapability Main = Postgres\n";
        assert_eq!(
            resolve_label("Main.select", DOC, None).as_deref(),
            Some(
                "select[T](tail: String, params: List[PgValue]) -> Effect[Result[List[T], PgReadError]]"
            )
        );
        assert_eq!(
            resolve_label("Main.firstAs", DOC, None).as_deref(),
            Some(
                "firstAs[T](tail: String, params: List[PgValue]) -> Effect[Result[Option[T], PgReadError]]"
            )
        );
        assert_eq!(
            resolve_label("Main.transaction", DOC, None).as_deref(),
            Some(
                "transaction(statements: List[PgStatement]) -> Effect[Result[List[PgRun], PgError]]"
            )
        );
    }

    /// Review of #1968: a derivation cycle in unchecked source resolves
    /// nothing rather than recursing without end.
    #[test]
    fn a_derivation_cycle_has_no_signature() {
        let doc = "context a\n\ncapability A = B\ncapability B = A\n";
        assert_eq!(resolve_label("A.put", doc, None), None);
    }

    #[test]
    fn fault_builtins_have_a_signature_a_user_fn_shadows() {
        // #2620
        assert_eq!(
            resolve_label("unreachable", "context a.b\n", None).as_deref(),
            Some("unreachable(message: String) -> never returns")
        );
        let doc = "commons m {\n  fn todo(n: Int) -> Int { n }\n}\n";
        assert_eq!(
            resolve_label("todo", doc, None).as_deref(),
            Some("todo(n: Int) -> Int")
        );
    }

    #[test]
    fn builtin_static_and_constructor_labels() {
        assert_eq!(
            resolve_label("Int.parse", "context a.b\n", None).as_deref(),
            Some("parse(s: String) -> Option[Int]")
        );
        assert!(
            resolve_label("Ok", "context a.b\n", None)
                .unwrap()
                .starts_with("Ok(value")
        );
    }

    #[test]
    fn yaml_decode_has_a_signature() {
        // #3214: `Yaml.decode` reads the same table as `Json.decode`.
        assert_eq!(
            resolve_label("Yaml.decode", "context a.b\n", None).as_deref(),
            Some("decode[T](s: String) -> Result[T, DecodeError]")
        );
        assert_eq!(resolve_label("Yaml.encode", "context a.b\n", None), None);
    }

    #[test]
    fn a_signed_cookie_actor_has_issue_and_expire_signatures() {
        let doc = "context a\n\ntype UserId = String where NonEmpty\n\nactor User { auth = SignedCookie(secret = \"K\", ttl = 1.hour), identity = UserId }\n";
        assert_eq!(
            resolve_label("User.issue", doc, None).as_deref(),
            Some("issue(identity: UserId) -> Effect[Cookie]")
        );
        assert_eq!(
            resolve_label("User.expire", doc, None).as_deref(),
            Some("expire() -> Cookie")
        );
        let other =
            "context a\n\nactor Admin { auth = Bearer(secret = \"K\"), identity = String }\n";
        assert_eq!(resolve_label("Admin.issue", other, None), None);
    }

    #[test]
    fn a_cookie_record_field_has_cookie_and_expire_signatures() {
        let doc = "context a.b\ntype Prefs = { visits: Option[Int] }\nservice s from http {\n  on GET(\"/p\") (cookies: Prefs) -> Effect[HttpResult[String]] by v: Visitor { Ok(\"x\") }\n}\n";
        assert_eq!(
            resolve_label("Prefs.visits.cookie", doc, None).as_deref(),
            Some("cookie(value: Int) -> Cookie")
        );
        assert_eq!(
            resolve_label("Prefs.visits.expire", doc, None).as_deref(),
            Some("expire() -> Cookie")
        );
        assert_eq!(resolve_label("Prefs.nope.cookie", doc, None), None);
        // a record no handler takes as `cookies` has no cookie signatures
        let plain = "context a.b\ntype P = { x: Int }\n";
        assert_eq!(resolve_label("P.x.cookie", plain, None), None);
        // the call context names the whole chain as the callee
        let call = "  Ok(\"x\").withCookie(Prefs.visits.cookie(";
        let ctx = call_context(call, call.len()).expect("a call");
        assert_eq!(ctx.callee, "Prefs.visits.cookie");
    }

    #[test]
    fn free_fn_and_capability_op_and_refined_labels() {
        let doc = "commons m {\n  fn add(a: Int, b: Int) -> Int { a }\n}\n";
        assert_eq!(
            resolve_label("add", doc, None).as_deref(),
            Some("add(a: Int, b: Int) -> Int")
        );
        let cap = "context a.b\n  capability Timer { fn after(label: String) -> Effect[Int] }\n";
        assert_eq!(
            resolve_label("Timer.after", cap, None).as_deref(),
            Some("after(label: String) -> Effect[Int]")
        );
        let refined = "commons m {\n  type Email = String where NonEmpty\n}\n";
        assert_eq!(
            resolve_label("Email.of", refined, None).as_deref(),
            Some("of(value: String) -> Result[Email, ValidationError]")
        );
    }

    #[test]
    fn value_receiver_callee_detection_and_rewrite() {
        assert_eq!(value_receiver_method("xs.fold"), Some(("xs", "fold")));
        assert_eq!(value_receiver_method("Int.parse"), None); // uppercase = name callee
        assert_eq!(value_receiver_method("a.b.fold"), None); // multi-segment
        assert_eq!(value_receiver_method("bar"), None); // no receiver

        let text = "  let r = xs.fold(0, ";
        let open = text.find('(').unwrap();
        let (rw, off) = value_receiver_rewrite(text, "xs.fold", open, text.len()).unwrap();
        assert_eq!(rw, "  let r = xs", "the `.fold(0, ` is dropped");
        assert_eq!(&text[off..=off], "s", "offset lands inside `xs`");
    }

    #[test]
    fn an_overloaded_kernel_method_has_one_signature_per_arity() {
        use bynk_check::checker::{Ty, Types};
        use bynk_syntax::ast::BaseType;
        let tys = &Types::new();
        let list = tys.intern(Ty::List(tys.intern(Ty::Base(BaseType::Int))));
        for (method, arities) in [("min", [1, 0]), ("max", [1, 0]), ("count", [0, 1])] {
            let forms = kernel_method_signatures(list, tys, method);
            let widths: Vec<usize> = forms.iter().map(|f| param_ranges(f).len()).collect();
            assert_eq!(widths, arities, "{method}: {forms:?}");
        }
        let join = kernel_method_signatures(list, tys, "join");
        assert_eq!(join.len(), 2, "{join:?}");
        assert!(join.iter().all(|f| !f.contains(", or")), "{join:?}");
    }

    #[test]
    fn the_active_form_follows_the_arguments_typed() {
        use bynk_check::checker::{Ty, Types};
        use bynk_syntax::ast::BaseType;
        let tys = &Types::new();
        let list = tys.intern(Ty::List(tys.intern(Ty::Base(BaseType::Int))));
        // `xs.min(`: only the keyed form has a first parameter.
        let min = kernel_method_signatures(list, tys, "min");
        assert_eq!(active_signature(&min, 0), 0);
        // `xs.count(`: the predicate form, not `count()`.
        let count = kernel_method_signatures(list, tys, "count");
        assert_eq!(active_signature(&count, 0), 1);
        // `xs.join(a`: the narrow separator form; `xs.join(a, ` the three-argument
        // form, whose second parameter is the one highlighted.
        let join = kernel_method_signatures(list, tys, "join");
        let sep = join.iter().position(|f| f.starts_with("join(sep")).unwrap();
        let on = 1 - sep;
        assert_eq!(active_signature(&join, 0), sep);
        assert_eq!(active_signature(&join, 1), on);
        // More arguments than any form takes: the widest form.
        assert_eq!(active_signature(&join, 5), on);
        assert_eq!(active_signature(&min, 3), 0);
    }

    #[test]
    fn kernel_method_signature_lookup() {
        use bynk_check::checker::{Ty, Types};
        use bynk_syntax::ast::BaseType;
        let tys = &Types::new();
        let list = tys.intern(Ty::List(tys.intern(Ty::Base(BaseType::Int))));
        assert!(
            kernel_method_signature(list, tys, "fold")
                .unwrap()
                .starts_with("fold(")
        );
        let string = tys.intern(Ty::Base(BaseType::String));
        assert!(
            kernel_method_signature(string, tys, "split")
                .unwrap()
                .starts_with("split(")
        );
        assert!(kernel_method_signature(string, tys, "nope").is_none());
    }

    /// #3955: `compare` has signature help on a `Bytes` receiver and on a named
    /// type, but not on a `Bool`.
    #[test]
    fn kernel_method_signature_offers_compare_on_orderable_receivers() {
        use bynk_check::checker::{NamedKind, Ty, Types};
        use bynk_syntax::ast::BaseType;
        let tys = &Types::new();
        let bytes = tys.intern(Ty::Base(BaseType::Bytes));
        assert_eq!(
            kernel_method_signature(bytes, tys, "compare").as_deref(),
            Some("compare(other: T) -> Ordering")
        );
        let version = tys.intern(Ty::Named {
            name: "Version".to_string(),
            kind: NamedKind::Record,
            args: Vec::new(),
        });
        assert!(kernel_method_signature(version, tys, "compare").is_some());
        let flag = tys.intern(Ty::Base(BaseType::Bool));
        assert!(kernel_method_signature(flag, tys, "compare").is_none());
    }

    #[test]
    fn param_ranges_split_top_level_only() {
        let label = "fold(init: U, step: (U, T) -> U) -> U";
        let r = param_ranges(label);
        assert_eq!(r.len(), 2, "two params, not split inside (U, T): {r:?}");
        assert_eq!(&label[r[0].0..r[0].1], "init: U");
        assert_eq!(&label[r[1].0..r[1].1], "step: (U, T) -> U");
    }

    #[test]
    fn retry_after_has_a_signature_unless_a_user_variant_shadows_it() {
        assert_eq!(
            resolve_label("RetryAfter", "context c\n", None).as_deref(),
            Some("RetryAfter(reason: String, delay: Duration) -> QueueResult")
        );
        let shadowed = "context c\n\ntype Job = | RetryAfter(n: Int) | Done\n";
        assert_eq!(resolve_label("RetryAfter", shadowed, None), None);
    }
}
