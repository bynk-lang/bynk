//! `bynk-contracts.json` generation per Worker (v0.177, #643).
//!
//! The contract hash a context's Worker entry compares an incoming
//! `X-Bynk-Contract` against, per `on call` service — the same constants the
//! entry stamps, written where the **driver** can read them.
//!
//! Why the driver needs them: the runtime check fails closed, but it does so in
//! *production*, on live traffic, after the skewed pair is already deployed.
//! `deploy` can know sooner. It records each context's hashes when it pushes,
//! so a later `deploy --context A` can compare A's compiled view of its
//! dependencies against what those dependencies actually have live, and refuse
//! before the push rather than let requests 409 (ADR 0193's D4 gate already
//! checks a dependency *exists*; this extends it to *matches*).
//!
//! Why a file rather than an API: the driver has two compile paths, and under a
//! `bynkc` override the compiler is a child process handing back an exit status
//! — there is no in-memory model to consult. A fact the compiler knows must
//! reach the driver in the build output, or not at all (ADR 0195 D5, the same
//! reasoning as `bynk-secrets.json`).

use std::collections::BTreeMap;

use bynk_project::json_string;

/// The file the driver reads, beside each Worker's `wrangler.toml`.
pub const CONTRACTS_MANIFEST: &str = "bynk-contracts.json";

/// The manifest schema version. Bumped only by a breaking shape change; the
/// driver refuses a version it does not know rather than guessing, as the
/// deploy ledger and the secrets manifest do.
pub(crate) const MANIFEST_VERSION: u32 = 1;

/// #2485: the structured shape text ([`bynk_check::contract_compat`]) beside
/// each hash the manifest records: `provides` by service, `expects` by
/// dependency then service.
#[derive(Default)]
pub struct Shapes {
    pub provides: BTreeMap<String, String>,
    pub expects: BTreeMap<String, BTreeMap<String, String>>,
    /// #2554: service to the older contract hashes it also accepts, from the
    /// contracts lock. The driver holds a live caller's recorded hash against
    /// these before it lets a compatible skew through.
    pub accepts: BTreeMap<String, Vec<String>>,
}

/// Render a context's contract manifest: what it **provides**, and what it
/// **expects** of each context it consumes.
///
/// Both halves are needed, and they are not the same fact:
///
/// - `provides` is this context's own constant per `on call` service — what its
///   entry enforces. `deploy` records it when the Worker is pushed, so the
///   ledger knows what is *live*.
/// - `expects` is this context's compiled view of each dependency's contract —
///   the constant it stamps at each call site. `deploy` compares it against the
///   ledger's `provides` for that dependency, which is exactly the runtime check
///   moved earlier.
///
/// A gate built on `provides` alone could not work: it would compare a context
/// against itself.
///
/// Absent rather than empty when a context neither exposes nor calls an `on
/// call` service: a project that never crosses a context boundary must not grow
/// a file into every worker directory for a feature it does not use — and "no
/// file" is the same answer as "empty" to a driver that must tolerate a build
/// tree from a compiler predating this file anyway.
///
/// I4 (#1970): `foreign` is the third half, **what each declared foreign
/// caller's generated client embeds** — actor → service → hash, the services
/// that actor may call. A foreign caller has no `bynk-contracts.json` of its
/// own to state its `expects`, so `deploy` holds this against the pin the
/// callee's project records for it (`bynk.foreign.toml`) and refuses a build
/// that would break it (`design/archive/retired-tracks.md`, interop track D3, Q6). Additive
/// within version 1, and absent when empty, so every manifest of a context
/// with no foreign caller is byte-identical to before.
pub fn emit_contracts_manifest(
    provides: &BTreeMap<String, String>,
    expects: &BTreeMap<String, BTreeMap<String, String>>,
    foreign: &BTreeMap<String, BTreeMap<String, String>>,
    shapes: &Shapes,
) -> Option<String> {
    if provides.is_empty() && expects.is_empty() {
        return None;
    }
    // Hand-rendered rather than via serde: this crate does not depend on
    // serde_json, and the shape is three fields.
    let foreign_part = if foreign.is_empty() {
        String::new()
    } else {
        [",\n  \"foreign\": ", &nested_map(foreign)].concat()
    };
    // #2485: the structured form of each hash above, so `deploy` can say
    // whether a skew is compatible. Additive within version 1 and absent when
    // empty; a reader that ignores it sees the manifest it always did.
    let shapes_part = {
        let mut part = String::new();
        if !shapes.provides.is_empty() {
            part.push_str(",\n  \"provides_shapes\": ");
            part.push_str(&json_map(&shapes.provides, 2));
        }
        if !shapes.expects.is_empty() {
            part.push_str(",\n  \"expects_shapes\": ");
            part.push_str(&nested_map(&shapes.expects));
        }
        if !shapes.accepts.is_empty() {
            part.push_str(",\n  \"accepts\": ");
            part.push_str(&accepts_map(&shapes.accepts));
        }
        part
    };
    Some(format!(
        "{{\n  \"version\": {MANIFEST_VERSION},\n  \"provides\": {},\n  \"expects\": {}{foreign_part}{shapes_part}\n}}\n",
        json_map(provides, 2),
        nested_map(expects),
    ))
}

/// What one context's contract manifest says, before it is rendered, so a
/// `[deploy.workers.<key>]` group's members (one Worker, one file) can be merged
/// (#3562).
#[derive(Debug, Clone, Default)]
pub struct ContractParts {
    pub provides: BTreeMap<String, String>,
    pub expects: BTreeMap<String, BTreeMap<String, String>>,
    pub foreign: BTreeMap<String, BTreeMap<String, String>>,
    pub provides_shapes: BTreeMap<String, String>,
    pub expects_shapes: BTreeMap<String, BTreeMap<String, String>>,
    pub accepts: BTreeMap<String, Vec<String>>,
}

impl ContractParts {
    pub fn render(&self) -> Option<String> {
        emit_contracts_manifest(
            &self.provides,
            &self.expects,
            &self.foreign,
            &Shapes {
                provides: self.provides_shapes.clone(),
                expects: self.expects_shapes.clone(),
                accepts: self.accepts.clone(),
            },
        )
    }

    /// One Worker's contracts: what its members provide (a service name is unique
    /// within a group, so the union loses nothing) and what they expect of
    /// contexts **outside** the Worker. A member's call to another member is the
    /// same build and the same upload, so it has no contract to skew against and
    /// is left out of `expects`.
    pub fn merged(parts: impl IntoIterator<Item = Self>, members: &[String]) -> Self {
        let mut all = Self::default();
        for part in parts {
            all.provides.extend(part.provides);
            all.provides_shapes.extend(part.provides_shapes);
            all.accepts.extend(part.accepts);
            for (dep, services) in part.expects {
                if !members.contains(&dep) {
                    all.expects.entry(dep).or_default().extend(services);
                }
            }
            for (dep, services) in part.expects_shapes {
                if !members.contains(&dep) {
                    all.expects_shapes.entry(dep).or_default().extend(services);
                }
            }
            for (actor, services) in part.foreign {
                all.foreign.entry(actor).or_default().extend(services);
            }
        }
        all
    }
}

/// A two-level map at the manifest's top level, `{}` when empty.
fn nested_map(m: &BTreeMap<String, BTreeMap<String, String>>) -> String {
    if m.is_empty() {
        return "{}".to_string();
    }
    let body: Vec<String> = m
        .iter()
        .map(|(outer, inner)| ["    ", &json_string(outer), ": ", &json_map(inner, 4)].concat())
        .collect();
    ["{\n", &body.join(",\n"), "\n  }"].concat()
}

/// `{"charge": ["hash", …]}` for the manifest's top level.
fn accepts_map(m: &BTreeMap<String, Vec<String>>) -> String {
    let body: Vec<String> = m
        .iter()
        .map(|(service, hashes)| {
            let list: Vec<String> = hashes.iter().map(|h| json_string(h)).collect();
            ["    ", &json_string(service), ": [", &list.join(", "), "]"].concat()
        })
        .collect();
    ["{\n", &body.join(",\n"), "\n  }"].concat()
}

fn json_map(m: &BTreeMap<String, String>, indent: usize) -> String {
    if m.is_empty() {
        return "{}".to_string();
    }
    let pad = " ".repeat(indent + 2);
    let close = " ".repeat(indent);
    let entries: Vec<String> = m
        .iter()
        .map(|(k, v)| format!("{pad}{}: {}", json_string(k), json_string(v)))
        .collect();
    format!("{{\n{}\n{close}}}", entries.join(",\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_when_a_context_neither_provides_nor_consumes() {
        assert_eq!(
            emit_contracts_manifest(
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                &Shapes::default()
            ),
            None
        );
    }

    #[test]
    fn renders_sorted_service_hashes() {
        let mut h = BTreeMap::new();
        h.insert("whoami".to_string(), "317bdd3de84d2176".to_string());
        h.insert("ask".to_string(), "0011223344556677".to_string());
        let out =
            emit_contracts_manifest(&h, &BTreeMap::new(), &BTreeMap::new(), &Shapes::default())
                .unwrap();
        assert!(out.contains("\"version\": 1"));
        // BTreeMap ordering: the file is byte-stable across builds, which the
        // golden fixtures depend on.
        let ask = out.find("\"ask\"").unwrap();
        let whoami = out.find("\"whoami\"").unwrap();
        assert!(ask < whoami, "services render in sorted order:\n{out}");
    }

    #[test]
    fn foreign_callers_are_recorded_only_when_declared() {
        let mut provides = BTreeMap::new();
        provides.insert("status".to_string(), "226b9939f85e69ea".to_string());
        let none = emit_contracts_manifest(
            &provides,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &Shapes::default(),
        )
        .unwrap();
        assert!(!none.contains("foreign"), "{none}");
        let mut legacy = BTreeMap::new();
        legacy.insert("status".to_string(), "226b9939f85e69ea".to_string());
        let mut foreign = BTreeMap::new();
        foreign.insert("Legacy".to_string(), legacy);
        let out =
            emit_contracts_manifest(&provides, &BTreeMap::new(), &foreign, &Shapes::default())
                .unwrap();
        assert!(
            out.contains("\"foreign\": {\n    \"Legacy\": {\n      \"status\": \"226b9939f85e69ea\"\n    }\n  }"),
            "{out}"
        );
    }

    #[test]
    fn a_consumer_records_what_it_expects_of_each_dependency() {
        let mut inner = BTreeMap::new();
        inner.insert("whoami".to_string(), "317bdd3de84d2176".to_string());
        let mut expects = BTreeMap::new();
        expects.insert("app.b".to_string(), inner);
        let out = emit_contracts_manifest(
            &BTreeMap::new(),
            &expects,
            &BTreeMap::new(),
            &Shapes::default(),
        )
        .unwrap();
        assert!(out.contains("\"app.b\""), "{out}");
        assert!(out.contains("\"whoami\": \"317bdd3de84d2176\""), "{out}");
        assert!(out.contains("\"provides\": {}"), "{out}");
    }

    /// #3562: two members expecting the same outside dependency are one `expects`
    /// entry (services unioned), and a call between members is no expectation.
    #[test]
    fn a_groups_expects_union_outside_dependencies_and_drop_its_own_members() {
        let part = |expects: &[(&str, &str, &str)]| {
            let mut p = ContractParts::default();
            for (dep, service, hash) in expects {
                p.expects
                    .entry(dep.to_string())
                    .or_default()
                    .insert(service.to_string(), hash.to_string());
            }
            p
        };
        let merged = ContractParts::merged(
            [
                part(&[("shop.pay", "charge", "h1"), ("shop.b", "inner", "h2")]),
                part(&[("shop.pay", "refund", "h3")]),
            ],
            &["shop.a".to_string(), "shop.b".to_string()],
        );
        assert_eq!(
            merged.expects.keys().collect::<Vec<_>>(),
            ["shop.pay"],
            "a member is not an outside dependency"
        );
        assert_eq!(
            merged.expects["shop.pay"].keys().collect::<Vec<_>>(),
            ["charge", "refund"],
            "services of one dependency are unioned"
        );
    }
}
