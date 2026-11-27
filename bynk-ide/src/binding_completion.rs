//! #3959 (capability-parameters track §13, slice 4): completion at a binding's
//! right-hand side, `remember(key, given Store = |)`, offers only what the
//! caller holds of the parameter's bound kind: its unit's instances of that
//! kind, the bare kind where the kind allows a bare use, and in a commons the
//! enclosing function's own parameters of that kind (forwarding).
//!
//! Nothing is chosen for the author: the list is every candidate, as the
//! binding's fix is (track §11).

use crate::completion::{Completion, CompletionKind, for_each_unit, parse_source_unit};
use bynk_syntax::ast::{CommonsItem, FnName, SourceUnit};
use std::collections::HashMap;
use std::path::PathBuf;

/// The completions at `offset` when it is the right-hand side of a binding, or
/// `None` elsewhere.
pub fn candidates_at(
    text: &str,
    offset: usize,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<Vec<Completion>> {
    let (callee, param) = binding_value_position(text, offset)?;
    let kind = bound_kind(&callee, &param, text, files)?;
    let mut out: Vec<Completion> = Vec::new();
    let mut push = |name: String, detail: String| {
        if !out.iter().any(|c| c.label == name) {
            out.push(Completion::item(
                name,
                CompletionKind::Capability,
                Some(detail),
            ));
        }
    };
    let unit = parse_source_unit(text);
    let items = match &unit {
        Some(SourceUnit::Context(c)) => &c.items[..],
        Some(SourceUnit::Commons(c)) => &c.items[..],
        _ => &[][..],
    };
    for item in items {
        if let CommonsItem::Capability(c) = item
            && c.derived.as_ref().is_some_and(|d| d.base.name == kind)
        {
            push(c.name.name.clone(), format!("an instance of `{kind}`"));
        }
    }
    if let Some(SourceUnit::Commons(c)) = &unit {
        for f in enclosing_fns(&c.items, offset) {
            for g in f
                .given
                .iter()
                .filter(|g| g.bound.as_ref().is_some_and(|b| b.name == kind))
            {
                push(
                    g.key().to_string(),
                    format!("this function's `{kind}` parameter"),
                );
            }
        }
    }
    if bare_allowed(&kind) {
        push(kind.clone(), format!("the bare `{kind}`, written out"));
    }
    (!out.is_empty()).then_some(out)
}

/// `(callee, parameter)` when `offset` is just after `Param =` in a call's
/// bindings, with at most a partial name typed.
fn binding_value_position(text: &str, offset: usize) -> Option<(String, String)> {
    let before = text.get(..offset)?;
    let open = crate::completion::innermost_open_paren(text, offset)?;
    let inside = &before[open + 1..];
    // The current item: what follows the last top-level comma.
    let item = last_top_level_item(inside);
    let (lhs, partial) = item.rsplit_once('=')?;
    if !partial
        .trim()
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    let lhs = lhs.trim();
    let lhs = lhs.strip_prefix("given").map_or(lhs, str::trim);
    if lhs.is_empty() || !lhs.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    // A binding: this item or an earlier one opens with a top-level `given`.
    if !has_top_level_given(inside) {
        return None;
    }
    let head = text[..open].trim_end();
    let callee: String = head
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    (!callee.is_empty()).then(|| (callee, lhs.to_string()))
}

fn last_top_level_item(s: &str) -> &str {
    let mut depth = 0i32;
    let mut last = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => last = i + 1,
            _ => {}
        }
    }
    &s[last..]
}

fn has_top_level_given(s: &str) -> bool {
    let mut depth = 0i32;
    let mut item_start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => item_start = i + 1,
            _ => {}
        }
        if depth == 0 && i == item_start {
            let rest = s[i..].trim_start();
            if rest.starts_with("given") && rest[5..].starts_with(char::is_whitespace) {
                return true;
            }
        }
    }
    let rest = s[item_start..].trim_start();
    rest.starts_with("given") && rest[5..].starts_with(char::is_whitespace)
}

/// The bound kind of `callee`'s capability parameter `param`, from any commons
/// in the project that declares `callee`.
fn bound_kind(
    callee: &str,
    param: &str,
    text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    let mut found = None;
    for_each_unit(text, files, |unit| {
        if found.is_some() {
            return;
        }
        let SourceUnit::Commons(c) = unit else { return };
        for item in &c.items {
            if let CommonsItem::Fn(f) = item
                && let FnName::Free(id) = &f.name
                && id.name == callee
                && let Some(g) = f
                    .given
                    .iter()
                    .find(|g| g.is_parameter() && g.key() == param)
            {
                found = g.bound.as_ref().map(|b| b.name.clone());
                return;
            }
        }
    });
    found
}

/// The commons free functions whose body holds `offset`.
fn enclosing_fns(
    items: &[CommonsItem],
    offset: usize,
) -> impl Iterator<Item = &bynk_syntax::ast::FnDecl> {
    items.iter().filter_map(move |i| match i {
        CommonsItem::Fn(f) if f.span.start <= offset && offset <= f.span.end => Some(f),
        _ => None,
    })
}

/// Whether a first-party instantiable kind may be used bare (`Kv`).
fn bare_allowed(kind: &str) -> bool {
    bynk_check::firstparty::UNITS
        .iter()
        .flat_map(|u| u.instantiable.iter())
        .any(|i| i.capability == kind && i.bare)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIB: &str = "commons lib.cache\n\nconsumes bynk.cloudflare { Kv, D1 }\n\nfn remember(key: String) -> Effect[()] given Store: Kv {\n  Store.put(key, \"v\")\n}\n";

    fn files() -> HashMap<PathBuf, String> {
        [(PathBuf::from("src/lib/cache.bynk"), LIB.to_string())].into()
    }

    fn labels(c: Option<Vec<Completion>>) -> Vec<String> {
        c.unwrap_or_default().into_iter().map(|c| c.label).collect()
    }

    /// #3959, track §13 slice 4: a binding's right-hand side offers only the
    /// caller's instances of the bound kind (and the bare kind it allows),
    /// never an instance of another kind.
    #[test]
    fn a_binding_offers_only_instances_of_the_bound_kind() {
        let shop = "context shop\n\nconsumes bynk.cloudflare { Kv, D1 }\n\nuses lib.cache\n\ncapability Sessions = Kv\ncapability Carts = Kv\ncapability Orders = D1\n\nservice api {\n  on call(key: String) -> Effect[()] by Caller given Sessions {\n    remember(key, given Store = )\n  }\n}\n";
        let at = shop.find("Store = ").unwrap() + "Store = ".len();
        let got = labels(candidates_at(shop, at, Some(&files())));
        assert_eq!(got, ["Sessions", "Carts", "Kv"]);
    }

    /// #3959: in a commons, a call that forwards offers the enclosing
    /// function's own parameters of the kind.
    #[test]
    fn a_forwarding_binding_offers_the_callers_parameters() {
        let lib = format!(
            "{LIB}\nfn top(key: String) -> Effect[()] given Mid: Kv {{\n  remember(key, given Store = M)\n}}\n"
        );
        let at = lib.find("Store = M").unwrap() + "Store = M".len();
        let got = labels(candidates_at(&lib, at, None));
        assert_eq!(got, ["Mid", "Kv"]);
    }

    /// #3959: an ordinary argument is not a binding position.
    #[test]
    fn an_ordinary_argument_is_not_a_binding() {
        let shop = "context shop\n\nservice api {\n  on call(key: String) -> Effect[()] by Caller {\n    remember(x = )\n  }\n}\n";
        let at = shop.find("x = ").unwrap() + 4;
        assert!(candidates_at(shop, at, Some(&files())).is_none());
    }
}
