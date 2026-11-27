//! #3959 (capability-parameters track §13, slice 4): completion at a binding's
//! right-hand side, `remember(key, given Store = |)`, offers only what the
//! caller holds of the parameter's bound kind:
//!
//! - in a context, its instances of that kind, and the bare kind where the kind
//!   allows a bare use;
//! - in a commons, the enclosing function's own parameters of that kind
//!   (forwarding): a commons holds no capability of its own;
//! - in a suite, its suite-local instances of that kind and, for a commons
//!   target, the kind's double (named by the kind); for a context target, that
//!   context's own candidates.
//!
//! Nothing is chosen for the author: the list is every candidate, as the
//! binding's fix is (track §11).

use crate::completion::{Completion, CompletionKind, for_each_unit, parse_source_unit};
use bynk_syntax::ast::{CapabilityDecl, CommonsItem, FnName, SourceUnit, SuiteDecl};
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
    let unit = parse_source_unit(text)?;
    let reachable = reachable_commons(&unit);
    let kind = bound_kind(&callee, &param, &reachable, text, files)?;
    let mut out = Candidates::default();
    match &unit {
        SourceUnit::Context(c) => out.context(&c.items, &kind),
        SourceUnit::Commons(c) => {
            for f in enclosing_fns(&c.items, offset) {
                for g in f
                    .given
                    .iter()
                    .filter(|g| g.bound.as_ref().is_some_and(|b| b.name == kind))
                {
                    out.push(
                        g.key().to_string(),
                        format!("this function's `{kind}` parameter"),
                    );
                }
            }
        }
        SourceUnit::Suite(s) => out.suite(s, &kind, text, files),
        SourceUnit::Adapter(_) => {}
    }
    (!out.0.is_empty()).then_some(out.0)
}

/// The candidate list, each label once, in the order found.
#[derive(Default)]
struct Candidates(Vec<Completion>);

impl Candidates {
    fn push(&mut self, name: String, detail: String) {
        if !self.0.iter().any(|c| c.label == name) {
            self.0.push(Completion::item(
                name,
                CompletionKind::Capability,
                Some(detail),
            ));
        }
    }

    /// The instances of `kind` among `caps`.
    fn instances<'a>(&mut self, caps: impl Iterator<Item = &'a CapabilityDecl>, kind: &str) {
        for c in caps.filter(|c| c.derived.as_ref().is_some_and(|d| d.base.name == kind)) {
            self.push(c.name.name.clone(), format!("an instance of `{kind}`"));
        }
    }

    /// A context's candidates: its instances of `kind`, then the bare kind
    /// where the kind allows one.
    fn context(&mut self, items: &[CommonsItem], kind: &str) {
        self.instances(capabilities(items), kind);
        if bare_allowed(kind) {
            self.push(kind.to_string(), format!("the bare `{kind}`, written out"));
        }
    }

    /// A suite's candidates: its suite-local instances, then what its target
    /// gives a case (the kind's double for a commons, the context's own
    /// candidates for a context).
    fn suite(
        &mut self,
        suite: &SuiteDecl,
        kind: &str,
        text: &str,
        files: Option<&HashMap<PathBuf, String>>,
    ) {
        self.instances(suite.capabilities.iter(), kind);
        let target = suite.target.joined();
        let mut done = false;
        for_each_unit(text, files, |unit| match unit {
            SourceUnit::Commons(c) if !done && c.name.joined() == target => {
                self.push(kind.to_string(), format!("this suite's `{kind}` double"));
                done = true;
            }
            SourceUnit::Context(c) if !done && c.name.joined() == target => {
                self.context(&c.items, kind);
                done = true;
            }
            _ => {}
        });
    }
}

fn capabilities(items: &[CommonsItem]) -> impl Iterator<Item = &CapabilityDecl> {
    items.iter().filter_map(|i| match i {
        CommonsItem::Capability(c) => Some(c),
        _ => None,
    })
}

/// The commons whose functions a call in `unit` can reach: those it `uses`,
/// and a commons itself or a suite's target.
fn reachable_commons(unit: &SourceUnit) -> Vec<String> {
    let (own, uses) = match unit {
        SourceUnit::Context(c) => (None, &c.uses),
        SourceUnit::Commons(c) => (Some(&c.name), &c.uses),
        SourceUnit::Suite(s) => (Some(&s.target), &s.uses),
        SourceUnit::Adapter(a) => (None, &a.uses),
    };
    own.into_iter()
        .chain(uses.iter().map(|u| &u.target))
        .map(|n| n.joined())
        .collect()
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

/// The bound kind of `callee`'s capability parameter `param`, from the commons
/// in `reachable` that declares `callee`.
fn bound_kind(
    callee: &str,
    param: &str,
    reachable: &[String],
    text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    let mut found = None;
    for_each_unit(text, files, |unit| {
        let SourceUnit::Commons(c) = unit else { return };
        if found.is_some() || !reachable.contains(&c.name.joined()) {
            return;
        }
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
    /// function's own parameters of the kind, and never the bare kind: a
    /// commons holds none (`bynk.given.kind_needs_parameter`).
    #[test]
    fn a_forwarding_binding_offers_the_callers_parameters() {
        let lib = format!(
            "{LIB}\nfn top(key: String) -> Effect[()] given Mid: Kv {{\n  remember(key, given Store = M)\n}}\n"
        );
        let at = lib.find("Store = M").unwrap() + "Store = M".len();
        let got = labels(candidates_at(&lib, at, None));
        assert_eq!(got, ["Mid"]);
    }

    /// #3959, track §9: in a commons suite, a binding offers the suite-local
    /// instances of the kind and the kind's double, also for a kind with no
    /// bare use (`D1`).
    #[test]
    fn a_commons_suite_offers_its_local_instances_and_the_double() {
        let lib = "commons lib.store\n\nconsumes bynk.cloudflare { D1 }\n\nfn save(id: String) -> Effect[()] given Db: D1 {\n  Effect.pure(())\n}\n";
        let files: HashMap<PathBuf, String> =
            [(PathBuf::from("src/lib/store.bynk"), lib.to_string())].into();
        let suite = "suite lib.store\n\nconsumes bynk.cloudflare { D1 }\n\ncapability Archive = D1\n\ncase \"saves\" {\n  let _ <- save(\"a\", given Db = )\n}\n";
        let at = suite.find("Db = ").unwrap() + "Db = ".len();
        let got = labels(candidates_at(suite, at, Some(&files)));
        assert_eq!(got, ["Archive", "D1"]);
    }

    /// #3959: the bound kind comes from the commons the caller `uses`, not
    /// from another library's function of the same name.
    #[test]
    fn the_bound_kind_comes_from_a_used_commons() {
        let other = "commons lib.other\n\nconsumes bynk.cloudflare { D1 }\n\nfn remember(key: String) -> Effect[()] given Store: D1 {\n  Effect.pure(())\n}\n";
        let mut files = files();
        files.insert(PathBuf::from("src/lib/other.bynk"), other.to_string());
        let shop = "context shop\n\nconsumes bynk.cloudflare { Kv, D1 }\n\nuses lib.cache\n\ncapability Sessions = Kv\ncapability Orders = D1\n\nservice api {\n  on call(key: String) -> Effect[()] by Caller given Sessions {\n    remember(key, given Store = )\n  }\n}\n";
        let at = shop.find("Store = ").unwrap() + "Store = ".len();
        assert_eq!(
            labels(candidates_at(shop, at, Some(&files))),
            ["Sessions", "Kv"]
        );
    }

    /// #3959: an ordinary argument is not a binding position.
    #[test]
    fn an_ordinary_argument_is_not_a_binding() {
        let shop = "context shop\n\nservice api {\n  on call(key: String) -> Effect[()] by Caller {\n    remember(x = )\n  }\n}\n";
        let at = shop.find("x = ").unwrap() + 4;
        assert!(candidates_at(shop, at, Some(&files())).is_none());
    }
}
