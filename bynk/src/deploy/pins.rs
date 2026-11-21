//! I4 (#1970): a declared foreign caller's pin, and the deploy gate that holds a
//! build to it (`design/archive/retired-tracks.md`, interop track D3, Q6).
//!
//! A Bynk caller's view of a callee's contract travels in its own
//! `bynk-contracts.json` (`expects`), and `deploy` refuses a build that would
//! leave it skewed (`bynk.deploy.contract_skew`). A Worker that is not Bynk has
//! no manifest, so the callee's project records, for each foreign caller, the
//! contract hashes that caller's generated client was built at: the **pin**,
//! in `bynk.foreign.toml` beside `bynk.toml`, checked in and written by the
//! user (the hashes are in the client's `CONTRACTS` doc, ready to paste):
//!
//! ```toml
//! ["shop.orders".Legacy]
//! ping = "4082ce9e2b74e919"
//! status = "226b9939f85e69ea"
//! ```
//!
//! `deploy` compares each pin with what the build's `bynk-contracts.json`
//! records for that caller (`foreign`) and **refuses** a mismatch: a changed
//! hash, a service the caller may no longer call, or a caller the context no
//! longer declares. Updating the pin is the override, and it is the user's
//! explicit acknowledgement that the foreign caller's client was regenerated
//! (§11 Q6: a warning on a production deploy is easily missed). A service a
//! declared caller may call with no pin is reported, never refused: the first
//! deploy of a new foreign caller has nothing to pin yet.

use super::*;

/// The pin file, beside `bynk.toml`.
pub(crate) const PIN_FILE: &str = "bynk.foreign.toml";

/// The refusal's diagnostic code, after `bynk.deploy.contract_skew`.
pub(crate) const FOREIGN_SKEW: &str = "bynk.deploy.foreign_contract_skew";

/// Context → actor → service → hash.
pub(crate) type ForeignPins = BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>;

/// Read the pin file. Absent is the empty answer: a project with no foreign
/// caller, or none pinned yet. A file that does not parse, or holds anything
/// but tables of hashes, is an error, named with the file.
pub(crate) fn read_pins(project_root: &Path) -> Result<ForeignPins, String> {
    let path = project_root.join(PIN_FILE);
    if !path.exists() {
        return Ok(ForeignPins::new());
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("could not read {PIN_FILE}: {e}"))?;
    parse_pins(&text).map_err(|e| format!("{PIN_FILE}: {e}"))
}

pub(crate) fn parse_pins(text: &str) -> Result<ForeignPins, String> {
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| e.message().to_string())?;
    let mut pins = ForeignPins::new();
    for (context, actors) in table {
        let toml::Value::Table(actors) = actors else {
            return Err(format!(
                "`{context}` must be a table of foreign callers, as `[\"{context}\".<Actor>]`"
            ));
        };
        for (actor, services) in actors {
            let toml::Value::Table(services) = services else {
                return Err(format!(
                    "`[\"{context}\".{actor}]` must be a table of `service = \"<hash>\"` lines"
                ));
            };
            let mut out = BTreeMap::new();
            for (service, hash) in services {
                // An unquoted dotted context (`[shop.orders.Legacy]`) is TOML
                // for nested tables, so the first value met where a hash
                // belongs is itself a table.
                if let toml::Value::Table(inner) = &hash {
                    let (spelled, meant) =
                        quoted_header(&[context.as_str(), actor.as_str(), service.as_str()], inner);
                    return Err(format!(
                        "`[{spelled}]` is read as nested tables, not a context and a caller: quote the dotted context name, as `{meant}`"
                    ));
                }
                let hash = hash.as_str().filter(|h| is_hash(h)).ok_or_else(|| {
                    format!(
                        "`{context}`'s `{actor}` pins `{service}` to something other than a contract hash (16 lowercase hex digits)"
                    )
                })?;
                out.insert(service, hash.to_string());
            }
            pins.entry(context.clone()).or_default().insert(actor, out);
        }
    }
    Ok(pins)
}

/// The dotted path an unquoted header spelled, down to the table that holds
/// the hashes, and the header it meant: its last segment the caller, the rest
/// the context (`["shop.orders".Legacy]`).
fn quoted_header(prefix: &[&str], mut table: &toml::Table) -> (String, String) {
    let mut path: Vec<String> = prefix.iter().map(|s| s.to_string()).collect();
    while table.len() == 1
        && let Some((key, toml::Value::Table(next))) = table.iter().next()
    {
        path.push(key.clone());
        table = next;
    }
    let spelled = path.join(".");
    let actor = path.pop().unwrap_or_default();
    (spelled, format!("[\"{}\".{actor}]", path.join(".")))
}

fn is_hash(s: &str) -> bool {
    s.len() == 16
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// One pin the build breaks.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BrokenPin {
    pub(crate) context: String,
    pub(crate) actor: String,
    pub(crate) service: String,
    pub(crate) pinned: String,
    /// What the build records for the caller, or why it records nothing.
    pub(crate) built: Built,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Built {
    /// The service's contract changed.
    Hash(String),
    /// The caller may no longer call the service (removed, renamed, or no
    /// longer admitting it).
    NotCallable,
    /// The context no longer declares the caller, or no handler admits it
    /// any more (either way the build records nothing for it).
    Undeclared,
    /// The project has no such context.
    NoContext,
}

/// One service a declared foreign caller may call, with no pin.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Unpinned {
    pub(crate) context: String,
    pub(crate) actor: String,
    pub(crate) service: String,
    pub(crate) hash: String,
}

/// Hold `pins` against `built`, each context's `foreign` record from this
/// build (context → actor → service → hash), for the contexts this run
/// deploys. `whole_project` says the run deploys every context, so a pin
/// naming a context the project does not have is broken rather than outside
/// the run.
pub(crate) fn check_pins(
    pins: &ForeignPins,
    built: &BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
    in_run: &BTreeSet<String>,
    whole_project: bool,
) -> (Vec<BrokenPin>, Vec<Unpinned>) {
    let mut broken = Vec::new();
    for (context, actors) in pins {
        // A context this run deploys, or (whole project) one it lacks.
        let checked = in_run.contains(context) || (whole_project && !built.contains_key(context));
        if !checked {
            continue;
        }
        let declared = built.get(context);
        for (actor, services) in actors {
            let callable = declared.and_then(|d| d.get(actor));
            for (service, pinned) in services {
                let found = match (declared, callable) {
                    (None, _) if !in_run.contains(context) => Built::NoContext,
                    (_, None) => Built::Undeclared,
                    (_, Some(c)) => match c.get(service) {
                        Some(h) if h == pinned => continue,
                        Some(h) => Built::Hash(h.clone()),
                        None => Built::NotCallable,
                    },
                };
                broken.push(BrokenPin {
                    context: context.clone(),
                    actor: actor.clone(),
                    service: service.clone(),
                    pinned: pinned.clone(),
                    built: found,
                });
            }
        }
    }
    let mut unpinned = Vec::new();
    for (context, actors) in built {
        if !in_run.contains(context) {
            continue;
        }
        for (actor, services) in actors {
            let pinned = pins.get(context).and_then(|a| a.get(actor));
            for (service, hash) in services {
                if pinned.is_none_or(|p| !p.contains_key(service)) {
                    unpinned.push(Unpinned {
                        context: context.clone(),
                        actor: actor.clone(),
                        service: service.clone(),
                        hash: hash.clone(),
                    });
                }
            }
        }
    }
    (broken, unpinned)
}

/// The refusal: each broken pin, then what to do.
pub(crate) fn refusal_message(broken: &[BrokenPin]) -> String {
    let mut out = format!(
        "bynk: error: this build breaks the contract a foreign caller's client was generated at ({FOREIGN_SKEW}):\n"
    );
    for b in broken {
        let what = match &b.built {
            Built::Hash(h) => format!("the build's is {h}"),
            Built::NotCallable => "the caller may no longer call it".to_string(),
            Built::Undeclared => {
                format!(
                    "`{}` no longer declares, or admits, `{}`",
                    b.context, b.actor
                )
            }
            Built::NoContext => format!("the project has no context `{}`", b.context),
        };
        out.push_str(&format!(
            "  {}.{} as `{}` — pinned at {}, {what}\n",
            b.context, b.service, b.actor, b.pinned
        ));
    }
    out.push_str(&format!(
        "  Deploying this would ship a callee the foreign caller's client is refused by (409 ContractMismatch, or 403).\n  Regenerate that caller's client (`bynk-client.ts`), then update its pin in {PIN_FILE}; the new hashes are in the client's `CONTRACTS`.\n"
    ));
    out
}

/// The report for services with no pin: the lines to add, grouped by caller.
pub(crate) fn unpinned_message(unpinned: &[Unpinned]) -> String {
    let mut groups: BTreeMap<(&str, &str), Vec<&Unpinned>> = BTreeMap::new();
    for u in unpinned {
        groups.entry((&u.context, &u.actor)).or_default().push(u);
    }
    let mut out = format!(
        "bynk: warning: {} foreign caller{} {} no pin in {PIN_FILE}, so a contract change that breaks {} client would not be refused. To pin, add:\n",
        groups.len(),
        if groups.len() == 1 { "" } else { "s" },
        if groups.len() == 1 { "has" } else { "have" },
        if groups.len() == 1 { "its" } else { "their" },
    );
    for ((context, actor), services) in groups {
        out.push_str(&format!("\n  [\"{context}\".{actor}]\n"));
        for u in services {
            out.push_str(&format!("  {} = \"{}\"\n", u.service, u.hash));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: &[(&str, &str, &str, &str)]) -> ForeignPins {
        let mut m = ForeignPins::new();
        for (c, a, s, h) in entries {
            m.entry(c.to_string())
                .or_default()
                .entry(a.to_string())
                .or_default()
                .insert(s.to_string(), h.to_string());
        }
        m
    }

    fn run(contexts: &[&str]) -> BTreeSet<String> {
        contexts.iter().map(|c| c.to_string()).collect()
    }

    const A: &str = "4082ce9e2b74e919";
    const B: &str = "226b9939f85e69ea";
    const C: &str = "26a626d0a1391c3d";

    #[test]
    fn parses_the_documented_shape() {
        let pins = parse_pins(&format!(
            "[\"shop.orders\".Legacy]\nping = \"{A}\"\nstatus = \"{B}\"\n\n[\"shop.orders\".Ledger]\nstatus = \"{B}\"\n"
        ))
        .unwrap();
        assert_eq!(
            pins,
            map(&[
                ("shop.orders", "Legacy", "ping", A),
                ("shop.orders", "Legacy", "status", B),
                ("shop.orders", "Ledger", "status", B),
            ])
        );
    }

    #[test]
    fn refuses_a_malformed_file() {
        assert!(
            parse_pins("[\"shop.orders\".Legacy]\nping = 3\n")
                .unwrap_err()
                .contains("contract hash")
        );
        assert!(parse_pins("[\"shop.orders\".Legacy]\nping = \"ABC\"\n").is_err());
        assert!(
            parse_pins("\"shop.orders\" = \"x\"\n")
                .unwrap_err()
                .contains("table of foreign callers")
        );
        assert!(
            parse_pins("[\"shop.orders\"]\nLegacy = \"x\"\n")
                .unwrap_err()
                .contains("service =")
        );
        assert!(parse_pins("not toml [").is_err());
    }

    #[test]
    fn an_unquoted_dotted_context_gets_a_hint_to_quote_it() {
        let e = parse_pins(&format!("[shop.orders.Legacy]\nping = \"{A}\"\n")).unwrap_err();
        assert!(e.contains("[\"shop.orders\".Legacy]"), "{e}");
        assert!(e.contains("quote the dotted context name"), "{e}");
        // A deeper context names the whole path.
        let e = parse_pins(&format!("[app.shop.orders.Legacy]\nping = \"{A}\"\n")).unwrap_err();
        assert!(e.contains("[\"app.shop.orders\".Legacy]"), "{e}");
    }

    #[test]
    fn a_caller_no_longer_admitted_reads_as_no_longer_admitted() {
        let pins = map(&[("shop.orders", "Legacy", "ping", A)]);
        // `Legacy` is still declared but no handler admits it: the build's
        // `foreign` record has nothing for it.
        let built = map(&[("shop.orders", "Ledger", "status", B)]);
        let (broken, _) = check_pins(&pins, &built, &run(&["shop.orders"]), true);
        assert_eq!(broken[0].built, Built::Undeclared);
        let message = refusal_message(&broken);
        assert!(
            message.contains("`shop.orders` no longer declares, or admits, `Legacy`"),
            "{message}"
        );
    }

    #[test]
    fn a_matching_pin_passes() {
        let built = map(&[("shop.orders", "Legacy", "ping", A)]);
        let (broken, unpinned) = check_pins(&built, &built, &run(&["shop.orders"]), true);
        assert!(broken.is_empty() && unpinned.is_empty());
    }

    #[test]
    fn a_changed_hash_breaks_the_pin() {
        let pins = map(&[("shop.orders", "Legacy", "ping", A)]);
        let built = map(&[("shop.orders", "Legacy", "ping", C)]);
        let (broken, _) = check_pins(&pins, &built, &run(&["shop.orders"]), true);
        assert_eq!(
            broken,
            vec![BrokenPin {
                context: "shop.orders".into(),
                actor: "Legacy".into(),
                service: "ping".into(),
                pinned: A.into(),
                built: Built::Hash(C.into()),
            }]
        );
        let message = refusal_message(&broken);
        assert!(message.contains(FOREIGN_SKEW), "{message}");
        assert!(
            message.contains(&format!("pinned at {A}, the build's is {C}")),
            "{message}"
        );
        assert!(message.contains(PIN_FILE), "{message}");
    }

    #[test]
    fn a_service_the_caller_may_no_longer_call_breaks_the_pin() {
        let pins = map(&[
            ("shop.orders", "Legacy", "ping", A),
            ("shop.orders", "Legacy", "status", B),
        ]);
        let built = map(&[("shop.orders", "Legacy", "status", B)]);
        let (broken, _) = check_pins(&pins, &built, &run(&["shop.orders"]), true);
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].service, "ping");
        assert_eq!(broken[0].built, Built::NotCallable);
    }

    #[test]
    fn a_caller_no_longer_declared_breaks_its_pins() {
        let pins = map(&[("shop.orders", "Legacy", "ping", A)]);
        let built = map(&[("shop.orders", "Ledger", "status", B)]);
        let (broken, unpinned) = check_pins(&pins, &built, &run(&["shop.orders"]), true);
        assert_eq!(broken[0].built, Built::Undeclared);
        assert_eq!(unpinned.len(), 1, "Ledger's service is unpinned");
    }

    #[test]
    fn a_context_with_no_foreign_caller_left_breaks_its_pins() {
        let pins = map(&[("shop.orders", "Legacy", "ping", A)]);
        let (broken, _) = check_pins(&pins, &ForeignPins::new(), &run(&["shop.orders"]), true);
        assert_eq!(broken[0].built, Built::Undeclared);
    }

    #[test]
    fn a_pinned_context_the_project_lacks_is_broken_in_a_whole_project_run() {
        let pins = map(&[("shop.gone", "Legacy", "ping", A)]);
        let (broken, _) = check_pins(&pins, &ForeignPins::new(), &run(&["shop.orders"]), true);
        assert_eq!(broken[0].built, Built::NoContext);
        // Under `--context`, a context outside the run is not this run's
        // business.
        let (broken, _) = check_pins(&pins, &ForeignPins::new(), &run(&["shop.orders"]), false);
        assert!(broken.is_empty());
    }

    #[test]
    fn contexts_outside_a_context_run_are_not_checked() {
        let pins = map(&[("shop.other", "Legacy", "ping", A)]);
        let built = map(&[("shop.other", "Legacy", "ping", C)]);
        let (broken, unpinned) = check_pins(&pins, &built, &run(&["shop.orders"]), false);
        assert!(broken.is_empty() && unpinned.is_empty());
    }

    #[test]
    fn an_unpinned_service_is_reported_with_the_lines_to_add() {
        let built = map(&[
            ("shop.orders", "Legacy", "ping", A),
            ("shop.orders", "Legacy", "status", B),
        ]);
        let pins = map(&[("shop.orders", "Legacy", "status", B)]);
        let (broken, unpinned) = check_pins(&pins, &built, &run(&["shop.orders"]), true);
        assert!(broken.is_empty());
        assert_eq!(
            unpinned,
            vec![Unpinned {
                context: "shop.orders".into(),
                actor: "Legacy".into(),
                service: "ping".into(),
                hash: A.into(),
            }]
        );
        let message = unpinned_message(&unpinned);
        assert!(
            message.contains(&format!("  [\"shop.orders\".Legacy]\n  ping = \"{A}\"\n")),
            "{message}"
        );
        // What it prints parses back as a pin.
        let pasted: String = message
            .lines()
            .skip(1)
            .map(|l| format!("{}\n", l.trim()))
            .collect();
        assert_eq!(
            parse_pins(&pasted).unwrap(),
            map(&[("shop.orders", "Legacy", "ping", A)])
        );
    }
}
