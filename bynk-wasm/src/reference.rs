//! `bynk_reference`: the compiler's own catalogue of the standard library, for a
//! browsable reference (#2698).
//!
//! Nothing here is a second copy of the data. Value methods come from
//! `bynk_check::kernel_methods`, type statics from `bynk_ide`'s
//! `BUILTIN_STATICS` (the table completion reads), and capabilities and module
//! functions are read out of the embedded first-party `.bynk` sources
//! (`FIRSTPARTY_SOURCES`), signatures rendered by the same code hover and
//! completion use, doc comments taken verbatim. Stability comes from the
//! first-party unit table, deprecation from the checker's own
//! `bynk.list.deprecated_function` table.
//!
//! The catalogue is fixed for a build, so it is built once and cached; the JSON
//! is a few tens of kilobytes (bounded by a test).

use std::sync::OnceLock;

use bynk_check::firstparty::{self, FIRSTPARTY_SOURCES, Stability, UNITS, UnitKind};
use bynk_check::kernel_methods::{self as km, KernelMethod};
use bynk_ide::completion::{BUILTIN_STATICS, capability_op_signature, free_fn_signature};
use bynk_syntax::ast::{CommonsItem, Doc, FnName, SourceUnit};
use bynk_syntax::{lexer, parser};
use serde::Serialize;

/// A member (static, method, operation or function) of something in the catalogue.
#[derive(Serialize)]
pub struct Member {
    pub name: String,
    pub signature: String,
    /// The `---` doc block, Markdown; absent when the item has none (the kernel
    /// method and static tables carry a signature only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Set on an experimental capability operation or function of a stable unit.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub experimental: bool,
    /// Why the member is deprecated, and what to use instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
}

/// A built-in type: its statics (`Int.parse`) and value methods (`n.toString()`).
#[derive(Serialize)]
pub struct TypeEntry {
    pub name: String,
    pub statics: Vec<Member>,
    pub methods: Vec<Member>,
}

/// A first-party capability and its operations.
#[derive(Serialize)]
pub struct CapabilityEntry {
    pub name: String,
    /// The unit a project `consumes` it from (`bynk`, `bynk.cloudflare`, ...).
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Needs `[unstable] allow` to use.
    pub experimental: bool,
    pub operations: Vec<Member>,
}

/// A first-party commons a project `uses`, and its free functions.
#[derive(Serialize)]
pub struct ModuleEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub experimental: bool,
    pub functions: Vec<Member>,
}

#[derive(Serialize)]
pub struct Reference {
    pub types: Vec<TypeEntry>,
    pub capabilities: Vec<CapabilityEntry>,
    pub modules: Vec<ModuleEntry>,
}

fn plain(name: &str, signature: &str) -> Member {
    Member {
        name: name.to_string(),
        signature: signature.to_string(),
        doc: None,
        experimental: false,
        deprecated: None,
    }
}

/// The value-receiver method tables, by the type a user writes.
fn method_tables() -> Vec<(&'static str, &'static [KernelMethod])> {
    vec![
        ("Int", km::INT_METHODS),
        ("Float", km::FLOAT_METHODS),
        ("Decimal", km::DECIMAL_METHODS),
        ("String", km::STRING_METHODS),
        ("Bytes", km::BYTES_METHODS),
        ("Duration", km::DURATION_METHODS),
        ("Instant", km::INSTANT_METHODS),
        ("List", km::LIST_METHODS),
        ("Map", km::MAP_METHODS),
        ("Set", km::SET_METHODS),
        ("Query", km::QUERY_METHODS),
        ("Option", km::OPTION_METHODS),
        ("Result", km::RESULT_METHODS),
        ("Effect", km::EFFECT_METHODS),
        ("Effect[Result[T, E]]", km::EFFECT_RESULT_METHODS),
        ("RequestHeaders", km::REQUEST_HEADERS_METHODS),
        ("TokenClaims", km::TOKEN_CLAIMS_METHODS),
    ]
}

fn types() -> Vec<TypeEntry> {
    let mut out: Vec<TypeEntry> = method_tables()
        .into_iter()
        .map(|(name, table)| TypeEntry {
            name: name.to_string(),
            statics: Vec::new(),
            methods: table.iter().map(|m| plain(m.name, m.signature)).collect(),
        })
        .collect();
    for &(name, statics) in BUILTIN_STATICS {
        let members: Vec<Member> = statics.iter().map(|&(n, sig)| plain(n, sig)).collect();
        match out.iter_mut().find(|t| t.name == name) {
            Some(t) => t.statics = members,
            // Statics with no value methods (`Json`, `RoundingMode`).
            None => out.push(TypeEntry {
                name: name.to_string(),
                statics: members,
                methods: Vec::new(),
            }),
        }
    }
    out
}

fn doc_text(d: &Option<Doc>) -> Option<String> {
    d.as_ref().map(|d| d.text.clone())
}

fn parse(src: &str) -> Option<SourceUnit> {
    let tokens = lexer::tokenize(src).ok()?;
    parser::parse_unit_with_recovery(&tokens, src).0
}

fn is_experimental(path: &str) -> bool {
    firstparty::stability_of(path) == Some(Stability::Experimental)
}

fn units() -> (Vec<CapabilityEntry>, Vec<ModuleEntry>) {
    let mut caps = Vec::new();
    let mut mods = Vec::new();
    for (unit_name, src) in FIRSTPARTY_SOURCES {
        let Some(kind) = UNITS.iter().find(|u| u.name == *unit_name).map(|u| u.kind) else {
            continue;
        };
        let Some(unit) = parse(src) else { continue };
        match (kind, &unit) {
            (UnitKind::Adapter, SourceUnit::Adapter(a)) => {
                for item in &a.items {
                    let CommonsItem::Capability(c) = item else {
                        continue;
                    };
                    // A derived capability has its base's operations.
                    if c.derived.is_some() {
                        continue;
                    }
                    let cap_path = format!("{unit_name}.{}", c.name.name);
                    caps.push(CapabilityEntry {
                        name: c.name.name.clone(),
                        unit: unit_name.to_string(),
                        doc: doc_text(&c.documentation),
                        experimental: is_experimental(&cap_path),
                        operations: c
                            .ops
                            .iter()
                            .map(|op| Member {
                                name: op.name.name.clone(),
                                signature: capability_op_signature(op),
                                doc: doc_text(&op.documentation),
                                experimental: is_experimental(&format!(
                                    "{cap_path}.{}",
                                    op.name.name
                                )),
                                deprecated: None,
                            })
                            .collect(),
                    });
                }
            }
            (UnitKind::Commons, SourceUnit::Commons(c)) => {
                let functions: Vec<Member> = c
                    .items
                    .iter()
                    .filter_map(|item| {
                        let CommonsItem::Fn(f) = item else {
                            return None;
                        };
                        let FnName::Free(id) = &f.name else {
                            return None;
                        };
                        if f.private.is_some() {
                            return None;
                        }
                        let deprecated = if *unit_name == "bynk.list" {
                            bynk_check::checker::BYNK_LIST_DEPRECATED
                                .iter()
                                .find(|(n, _)| *n == id.name)
                                .map(|(_, form)| format!("use the `List` method form `{form}`"))
                        } else {
                            None
                        };
                        Some(Member {
                            name: id.name.clone(),
                            signature: free_fn_signature(&id.name, f),
                            doc: doc_text(&f.documentation),
                            experimental: is_experimental(&format!("{unit_name}.{}", id.name)),
                            deprecated,
                        })
                    })
                    .collect();
                // A types-only commons (`bynk.locale.types`) has nothing to list.
                if !functions.is_empty() {
                    mods.push(ModuleEntry {
                        name: unit_name.to_string(),
                        doc: doc_text(&c.documentation),
                        experimental: is_experimental(unit_name),
                        functions,
                    });
                }
            }
            _ => {}
        }
    }
    (caps, mods)
}

/// Build the catalogue (uncached).
pub fn build() -> Reference {
    let (capabilities, modules) = units();
    Reference {
        types: types(),
        capabilities,
        modules,
    }
}

/// The catalogue as JSON: `{ types, capabilities, modules }`. Built once per
/// process; the library is fixed for a build.
pub fn reference_to_json() -> String {
    static JSON: OnceLock<String> = OnceLock::new();
    JSON.get_or_init(|| {
        serde_json::to_string(&build())
            .unwrap_or_else(|_| "{\"types\":[],\"capabilities\":[],\"modules\":[]}".to_string())
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json() -> serde_json::Value {
        serde_json::from_str(&reference_to_json()).expect("valid JSON")
    }

    fn find<'a>(v: &'a serde_json::Value, key: &str, name: &str) -> &'a serde_json::Value {
        member(v, key, name)
    }

    fn member<'a>(entry: &'a serde_json::Value, key: &str, name: &str) -> &'a serde_json::Value {
        entry[key]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("{key} `{name}` missing"))
    }

    #[test]
    fn value_methods_are_listed_with_signatures() {
        let v = json();
        for (name, table) in method_tables() {
            let t = find(&v, "types", name);
            assert_eq!(
                t["methods"].as_array().unwrap().len(),
                table.len(),
                "{name}"
            );
        }
        let len = member(find(&v, "types", "List"), "methods", "length");
        assert_eq!(len["signature"], "length() -> Int");
    }

    #[test]
    fn statics_include_instant_duration_bytes_and_json() {
        let v = json();
        member(find(&v, "types", "Instant"), "statics", "fromEpochMillis");
        member(find(&v, "types", "Instant"), "statics", "fromIso");
        member(find(&v, "types", "Duration"), "statics", "millis");
        member(find(&v, "types", "Bytes"), "statics", "fromUtf8");
        member(find(&v, "types", "Json"), "statics", "decode");
        // Statics merge with the type's methods rather than duplicating it.
        let n = v["types"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["name"] == "Instant")
            .count();
        assert_eq!(n, 1);
    }

    #[test]
    fn capabilities_carry_docs_and_signatures() {
        let v = json();
        let clock = find(&v, "capabilities", "Clock");
        assert_eq!(clock["unit"], "bynk");
        assert_eq!(clock["experimental"], false);
        assert!(clock["doc"].as_str().is_some_and(|d| !d.is_empty()));
        let now = member(clock, "operations", "now");
        assert!(now["signature"].as_str().unwrap().starts_with("now()"));
        // An op's doc is optional; a function with a doc block carries it.
        let list = find(&v, "modules", "bynk.list");
        assert!(member(list, "functions", "reverse")["doc"].is_string());
    }

    #[test]
    fn crypto_is_marked_experimental() {
        let v = json();
        let crypto: Vec<_> = v["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["unit"] == "bynk.crypto")
            .collect();
        assert!(!crypto.is_empty());
        assert!(crypto.iter().all(|c| c["experimental"] == true));
    }

    #[test]
    fn deprecated_list_functions_say_what_to_use() {
        let v = json();
        let list = find(&v, "modules", "bynk.list");
        let map = member(list, "functions", "map");
        assert_eq!(map["deprecated"], "use the `List` method form `xs.map(f)`");
        assert!(map["doc"].is_string());
        // `reverse` has no method form yet.
        assert!(
            member(list, "functions", "reverse")
                .get("deprecated")
                .is_none()
        );
    }

    #[test]
    fn every_first_party_unit_with_functions_appears() {
        let v = json();
        for name in [
            "bynk.list",
            "bynk.map",
            "bynk.string",
            "bynk.set",
            "bynk.url",
        ] {
            assert!(
                !find(&v, "modules", name)["functions"]
                    .as_array()
                    .unwrap()
                    .is_empty(),
                "{name}"
            );
        }
    }

    #[test]
    fn the_catalogue_stays_a_bounded_size() {
        // Docs ride along verbatim; a runaway here would slow the playground's
        // one call. Raise deliberately, not by accident.
        let n = reference_to_json().len();
        assert!(n < 200_000, "bynk_reference is {n} bytes");
    }
}
