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
    let callee_start = open_paren.checked_sub(callee.len())?;
    let dot = callee_start + recv.len();
    let rewritten = format!("{}{}", &text[..dot], &text[cursor..]);
    Some((rewritten, dot.saturating_sub(1)))
}

/// The kernel-method signature for `method` on receiver type `ty`, if any.
pub fn kernel_method_signature(
    ty: bynk_check::checker::TyId,
    tys: &bynk_check::checker::Types,
    method: &str,
) -> Option<String> {
    bynk_check::kernel_methods::methods_for(ty, tys)
        .iter()
        .find(|m| m.name == method)
        .map(|m| m.signature.to_string())
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

/// The callee immediately before the `(` — a bare `name` or `Recv.member`.
fn callee_before(s: &str) -> Option<String> {
    let s = s.trim_end();
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

/// Render the signature *label* for a name callee — `name(p: T, …) -> R`.
/// `None` if the callee can't be resolved (or is a value receiver — slice 2).
pub fn resolve_label(
    callee: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    if let Some((recv, member)) = callee.rsplit_once('.') {
        // Built-in type statics — already display-ready signature strings.
        if let Some((_, statics)) = BUILTIN_STATICS.iter().find(|(n, _)| *n == recv)
            && let Some((_, sig)) = statics.iter().find(|(n, _)| *n == member)
        {
            return Some((*sig).to_string());
        }
        // A refined/opaque type's `of`/`unsafe`, or a capability op.
        return resolve_qualified(recv, member, doc_text, files);
    }
    // Built-in constructors.
    match callee {
        "Ok" => return Some("Ok(value: T) -> Result[T, E]".to_string()),
        "Err" => return Some("Err(error: E) -> Result[T, E]".to_string()),
        "Some" => return Some("Some(value: T) -> Option[T]".to_string()),
        _ => {}
    }
    // A free function.
    let mut found = None;
    for_each_unit(doc_text, files, |unit| {
        if found.is_some() {
            return;
        }
        let items = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            _ => return,
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
                    "{callee}{}({}) -> {}{}",
                    crate::symbols::fn_type_params_str(f),
                    params.join(", "),
                    type_ref_str(&f.return_type),
                    crate::symbols::fn_where_str(f)
                ));
                return;
            }
        }
    });
    found
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
                    let base = match &t.body {
                        TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. } => {
                            base_name(*base)
                        }
                        _ => return,
                    };
                    out = Some(if member == "of" {
                        format!("of(value: {base}) -> Result[{recv}, ValidationError]")
                    } else {
                        format!("unsafe(value: {base}) -> {recv}")
                    });
                    return;
                }
                CommonsItem::Capability(c) if c.name.name == recv && c.derived.is_some() => {
                    derived_base = c.derived.as_ref().map(|d| d.base.name.clone());
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
    match (out, derived_base) {
        (None, Some(base)) if !followed.contains(&base) => {
            resolve_qualified_from(&base, member, doc_text, files, followed)
        }
        (out, _) => out,
    }
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

    /// Review of #1968: a derivation cycle in unchecked source resolves
    /// nothing rather than recursing without end.
    #[test]
    fn a_derivation_cycle_has_no_signature() {
        let doc = "context a\n\ncapability A = B\ncapability B = A\n";
        assert_eq!(resolve_label("A.put", doc, None), None);
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

    #[test]
    fn param_ranges_split_top_level_only() {
        let label = "fold(init: U, step: (U, T) -> U) -> U";
        let r = param_ranges(label);
        assert_eq!(r.len(), 2, "two params, not split inside (U, T): {r:?}");
        assert_eq!(&label[r[0].0..r[0].1], "init: U");
        assert_eq!(&label[r[1].0..r[1].1], "step: (U, T) -> U");
    }
}
